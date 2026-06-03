use std::collections::BTreeMap;
use std::fmt;

use pal_asset::{AssetSource, Nls, ResourceManager};
use pal_script::opcodes::{ext_opcode, primary_opcode};
use pal_script::{Operand, OperandKind, PointTable, ScriptImage};

use crate::assets::CoreAssets;
use crate::audio::{AudioHandle, AudioSystem, PalSoundGroup, PalVolume};
use crate::config::{parse_ini_nls, IniFile};
use crate::effect::PalEffectSystem;
use crate::font::PalFontSystem;
use crate::image::decode_image;
use crate::msprite::{MSpriteHandle, MSpriteSystem, MSPRITE_STATE_FINISHED};
use crate::scene::{FrameScene, SceneTextureId, SolidQuad};
use crate::sprite::{
    PalAnimationFlags, PalColor, PalRect, PalVec3, SpriteDesc, SpriteHandle, SpriteSurface,
    SpriteSystem,
};
use crate::system::PalRandomState;
use crate::task::{TaskHandle, TaskSystem};
use crate::PalSheetAnimationDesc;

const DEFAULT_VAR_COUNT: usize = 0x10000;
const DEFAULT_STACK_LIMIT: usize = 0x10000;
/// Size of user_mem, system_mem, and temp_mem arrays (matches original engine).
const DEFAULT_MEM_SIZE: usize = 0x10000;
/// Maximum per-frame events recorded for PAL_DEBUG dump.
const MAX_FRAME_EVENTS: usize = 64;

#[derive(Clone, Debug)]
pub enum FrameEvent {
    ExtCallSkipped {
        pc: u32,
        category: u16,
        index: u16,
        name: Option<String>,
    },
    WaitEmitted {
        pc: u32,
        kind: WaitRequest,
    },
    UnsupportedCmd {
        pc: u32,
        opcode: u16,
        name: Option<String>,
    },
    UnsupportedExt {
        pc: u32,
        category: u16,
        index: u16,
        name: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct ScriptRuntimeConfig {
    pub instructions_per_frame: usize,
    pub trace: bool,
}

impl Default for ScriptRuntimeConfig {
    fn default() -> Self {
        Self {
            instructions_per_frame: 4096,
            trace: false,
        }
    }
}

/// Outcome returned by extcall dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExtCallOutcome {
    /// Handler ran and produced a return value.
    Value(i32),
    /// Handler produced a value and blocked the VM on a PAL wait task.
    Wait { value: i32, request: WaitRequest },
    /// No handler found; caller should write 0 to dst and continue.
    Skip,
    /// Handler cannot proceed; caller should set UnsupportedExtCall status.
    Block,
}

/// Request for a blocking wait task emitted by the script VM.
/// Engine creates the corresponding TaskSystem task after run_frame returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitRequest {
    /// Wait for N frame-ticks. 1 = one engine frame. -1 = forever.
    Frame(i32),
    /// Wait for N PAL milliseconds using the cached PAL clock.
    Time(u32),
    /// Wait for any key or mouse button push.
    Click,
}

#[derive(Debug)]
pub struct ScriptRuntime {
    pc: u32,
    entry_pc: u32,
    vars: Vec<i32>,
    stack: Vec<i32>,
    argument_stack: Vec<i32>,
    call_stack: Vec<u32>,
    status: RuntimeStatus,
    trace: bool,
    /// Handle to the active blocking wait task created for WaitFrame/WaitClick opcodes.
    /// Engine checks this handle after task_system.process() to detect task completion.
    wait_task_handle: Option<TaskHandle>,
    /// Cached PAL time in milliseconds, injected by Engine once per frame.
    pal_time_ms: u32,
    /// Game.exe wait_sync_begin stores PaltimeGetTime() at runtime offset +655248.
    wait_sync_begin_ms: u32,
    /// Game.exe wait_sync_release stores its active duration/start at +655244/+655248.
    wait_sync_release: Option<WaitSyncRelease>,
    /// Conservative model for wait_time_push/wait_time_pop extcalls.
    wait_time_stack: Vec<u32>,
    /// Game.exe category 22 run/run_stack state recovered from offsets around +804292.
    run_pipeline: RunPipelineState,
    effect_system: PalEffectSystem,
    msprite_system: MSpriteSystem,
    /// argument_base: saved/restored by call/ret (opcode 24). kind=0x9 reads/writes this.
    argument_base: i32,
    /// user_mem: kind 0x1 indirect — user_mem[vars[lo]].
    user_mem: Vec<i32>,
    /// system_mem: kind 0x2 indirect — system_mem[vars[lo]].
    system_mem: Vec<i32>,
    /// temp_mem: kind 0x5 indirect — temp_mem[(bank+argument_base)*bank_nonzero + vars[lo]].
    temp_mem: Vec<i32>,
    /// mem_dat_words: writable shadow of Mem.dat as i32 words (for MemDatDirect writes).
    mem_dat_words: Vec<i32>,
    /// Raw operand word for the current extcall's return destination (a1[184629]).
    extcall_dst_raw: u32,
    /// PAL file handles opened by category 18 file extcalls.
    file_handles: Vec<Option<RuntimeFile>>,
    /// Game script image slots mapped to PAL sprite handles.
    game_sprites: BTreeMap<i32, SpriteHandle>,
    /// PalAnimation tasks attached to Game script image slots.
    game_sprite_animations: BTreeMap<i32, TaskHandle>,
    /// Game.exe MSprite wrapper state keyed by script slot.
    game_msprites: BTreeMap<i32, GameMSpriteState>,
    /// Game button entries mapped to PAL sprite handles. Key is (button group, entry index).
    game_buttons: BTreeMap<(i32, i32), SpriteHandle>,
    /// Game script sound slots mapped to PAL audio handles. Key is (script category, slot).
    game_audio: BTreeMap<(u16, i32), AudioHandle>,
    font_state: PalFontSystem,
    text_state: TextSubsystemState,
    select_state: SelectSubsystemState,
    save_state: SaveSubsystemState,
    history_state: HistorySubsystemState,
    thread_state: ThreadSubsystemState,
    message_state: MessageSubsystemState,
    /// Dynamic string slots used by startup string-buffer extcalls.
    dynamic_strings: Vec<String>,
    /// SYSTEM.INI parsed through the selected external NLS.
    system_ini: Option<IniFile>,
    /// PAL random seed ring used by PalRandomEx and Game category 20 random.
    random_state: PalRandomState,
    /// Per-frame events accumulated during run_frame(); cleared at the start of each frame.
    frame_events: Vec<FrameEvent>,
}

#[derive(Clone, Debug)]
struct RuntimeFile {
    name: String,
    bytes: Vec<u8>,
    cursor: usize,
    table_cursor: usize,
    parsed_table: Option<ParsedFileTable>,
}

#[derive(Clone, Debug)]
struct ParsedFileTable {
    entries: Vec<i32>,
    strings: BTreeMap<i32, String>,
}

#[derive(Clone, Copy, Debug)]
struct WaitSyncRelease {
    duration_ms: u32,
    start_ms: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RunPipelineState {
    run_stack_enabled: bool,
    pending_kind: i32,
    pending_effect_id: i32,
    pending_arg1: i32,
    pending_arg2: i32,
    effect_active: bool,
    no_wait_latch: bool,
    last_run_time_ms: u32,
    last_run_arg1: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct GameMSpriteState {
    handle: Option<MSpriteHandle>,
    playing: bool,
    locked: bool,
    loop_mode: i32,
    loop_start: i32,
    loop_end: i32,
    last_play: i32,
    finished: bool,
}

#[derive(Clone, Debug, Default)]
struct TextSubsystemState {
    initialized: bool,
    visible: bool,
    rect: [i32; 4],
    alpha: i32,
    mode: i32,
    base: i32,
    icon: i32,
    button: i32,
    history_enabled: bool,
    voice_cut_enabled: bool,
    voice_enabled: bool,
    voice_volume: i32,
    bgv_enabled: bool,
    bgv_volume: i32,
    bgv_muted: bool,
    voice_autopan_enabled: bool,
    last_text_value: i32,
    last_event_time_ms: u32,
}

#[derive(Clone, Debug, Default)]
struct SelectSubsystemState {
    initialized: bool,
    locked: bool,
    rect: [i32; 4],
    text_value: i32,
    colors: [i32; 3],
    offsets: BTreeMap<i32, i32>,
    process: [i32; 3],
    last_key: i32,
}

#[derive(Clone, Debug, Default)]
struct SaveSubsystemState {
    title: i32,
    thumbnail_size: [i32; 2],
    text_rect: [i32; 4],
    font_size: i32,
    font_type: i32,
    font_effect: i32,
    font_color: i32,
    locked: bool,
    last_slot: i32,
    last_result: i32,
}

#[derive(Clone, Debug, Default)]
struct HistorySubsystemState {
    initialized: bool,
    rect: [i32; 4],
    colors: [i32; 2],
    layout: [i32; 7],
    current_text_value: i32,
    height: i32,
    active: bool,
    records: Vec<[i32; 9]>,
    skipped: bool,
}

#[derive(Clone, Debug, Default)]
struct ThreadSubsystemState {
    next_id: i32,
    active: BTreeMap<i32, bool>,
    last_id: i32,
}

#[derive(Clone, Debug, Default)]
struct MessageSubsystemState {
    next_id: i32,
    slots: [GameMessage; 8],
    queue: Vec<GameMessage>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct GameMessage {
    active: bool,
    id: i32,
    value: i32,
    param: i32,
}

impl ScriptRuntime {
    pub fn boot(entry_pc: u32, config: ScriptRuntimeConfig) -> Self {
        Self {
            pc: entry_pc,
            entry_pc,
            vars: vec![0; DEFAULT_VAR_COUNT],
            stack: Vec::new(),
            argument_stack: Vec::new(),
            call_stack: Vec::new(),
            status: RuntimeStatus::Running { pc: entry_pc },
            trace: config.trace,
            wait_task_handle: None,
            pal_time_ms: 0,
            wait_sync_begin_ms: 0,
            wait_sync_release: None,
            wait_time_stack: Vec::new(),
            run_pipeline: RunPipelineState::default(),
            effect_system: PalEffectSystem::new(),
            msprite_system: MSpriteSystem::new(),
            argument_base: 0,
            user_mem: vec![0; DEFAULT_MEM_SIZE],
            system_mem: vec![0; DEFAULT_MEM_SIZE],
            temp_mem: vec![0; DEFAULT_MEM_SIZE],
            mem_dat_words: Vec::new(),
            extcall_dst_raw: 0,
            file_handles: Vec::new(),
            game_sprites: BTreeMap::new(),
            game_sprite_animations: BTreeMap::new(),
            game_msprites: BTreeMap::new(),
            game_buttons: BTreeMap::new(),
            game_audio: BTreeMap::new(),
            font_state: PalFontSystem::new(),
            text_state: TextSubsystemState::default(),
            select_state: SelectSubsystemState::default(),
            save_state: SaveSubsystemState::default(),
            history_state: HistorySubsystemState::default(),
            thread_state: ThreadSubsystemState::default(),
            message_state: MessageSubsystemState::default(),
            dynamic_strings: Vec::new(),
            system_ini: None,
            random_state: PalRandomState::default(),
            frame_events: Vec::new(),
        }
    }

    /// Initialise the writable Mem.dat shadow from the raw asset bytes.
    /// Call once after boot, before running frames.
    pub fn load_mem_dat(&mut self, bytes: &[u8]) {
        let word_count = bytes.len() / 4;
        let mut words = Vec::with_capacity(word_count);
        for i in 0..word_count {
            let off = i * 4;
            words.push(i32::from_le_bytes([
                bytes[off],
                bytes[off + 1],
                bytes[off + 2],
                bytes[off + 3],
            ]));
        }
        self.mem_dat_words = words;
    }

    /// Mark the runtime as faulted with a descriptive message.
    /// The runtime will stop executing but the window stays open.
    pub fn set_faulted(&mut self, message: String) {
        self.status = RuntimeStatus::Faulted {
            pc: self.pc,
            message,
        };
    }

    pub fn pc(&self) -> u32 {
        self.pc
    }

    pub fn entry_pc(&self) -> u32 {
        self.entry_pc
    }

    pub fn status(&self) -> &RuntimeStatus {
        &self.status
    }

    pub fn parsed_system_ini(&self) -> Option<&IniFile> {
        self.system_ini.as_ref()
    }

    pub fn vars(&self) -> &[i32] {
        &self.vars
    }

    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }

    pub fn argument_stack_depth(&self) -> usize {
        self.argument_stack.len()
    }

    pub fn call_stack_depth(&self) -> usize {
        self.call_stack.len()
    }

    /// Returns the handle of the active blocking wait task, if any.
    pub fn pending_wait_handle(&self) -> Option<TaskHandle> {
        self.wait_task_handle
    }

    /// Called by Engine when the wait task has completed.
    /// Sets status to Running and clears the handle.
    pub fn resolve_pending_wait(&mut self) {
        self.wait_task_handle = None;
        if let Some(pc) = self.status.pc() {
            self.status = RuntimeStatus::Running { pc };
        }
    }

    /// Associate the runtime with a newly created wait task handle.
    pub fn set_wait_handle(&mut self, handle: TaskHandle) {
        self.wait_task_handle = Some(handle);
    }

    /// Inject the PAL cached frame time used by Game.exe wait-sync wrappers.
    pub fn set_pal_time(&mut self, ms: u32) {
        self.pal_time_ms = ms;
        self.effect_system.tick(ms);
    }

    pub fn effect_overlay(&self, logical_width: u32, logical_height: u32) -> Option<SolidQuad> {
        self.effect_system
            .overlay_quad(logical_width, logical_height, self.pal_time_ms)
    }

    pub fn advance_msprites(&mut self, sprites: &mut SpriteSystem, delta_ms: u32) {
        for update in self.msprite_system.advance(delta_ms) {
            let Some((&slot, _)) = self
                .game_msprites
                .iter()
                .find(|(_, state)| state.handle == Some(update.handle))
            else {
                continue;
            };
            let Some(sprite) = self.game_sprites.get(&slot).copied() else {
                continue;
            };
            sprites.replace_msprite_frame(
                sprite,
                update.handle,
                update.width,
                update.height,
                update.rgba,
                update.source_name,
            );
        }
    }

    pub fn run_frame(
        &mut self,
        assets: &CoreAssets,
        config: &ScriptRuntimeConfig,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.run_frame_with_resources(assets, None, None, None, None, config)
    }

    pub fn run_frame_with_resources(
        &mut self,
        assets: &CoreAssets,
        mut resource_manager: Option<&mut ResourceManager>,
        mut sprites: Option<&mut SpriteSystem>,
        mut task_system: Option<&mut TaskSystem>,
        mut audio: Option<&mut AudioSystem>,
        config: &ScriptRuntimeConfig,
    ) -> Result<RuntimeTick, RuntimeError> {
        match self.status {
            RuntimeStatus::Halted { .. }
            | RuntimeStatus::UnsupportedCommand { .. }
            | RuntimeStatus::UnsupportedExtCall { .. }
            | RuntimeStatus::Faulted { .. } => {
                return Ok(RuntimeTick {
                    executed: 0,
                    status: self.status.clone(),
                    wait_request: None,
                    frame_events: Vec::new(),
                });
            }
            // Still waiting on a task; don't re-emit wait_request.
            RuntimeStatus::WaitFrame { .. } | RuntimeStatus::WaitClick { .. } => {
                return Ok(RuntimeTick {
                    executed: 0,
                    status: self.status.clone(),
                    wait_request: None,
                    frame_events: Vec::new(),
                });
            }
            RuntimeStatus::NotBooted => {
                self.status = RuntimeStatus::Running { pc: self.pc };
            }
            RuntimeStatus::Running { .. } => {}
        }

        self.frame_events.clear();

        let script = assets
            .script_image()
            .map_err(|source| RuntimeError::ScriptParse {
                source: source.to_string(),
            })?;
        let mut executed = 0usize;
        let budget = config.instructions_per_frame.max(1);

        while executed < budget {
            match self.step(
                &script,
                &assets.point_table,
                &assets.mem_dat.bytes,
                assets,
                resource_manager.as_deref_mut(),
                sprites.as_deref_mut(),
                task_system.as_deref_mut(),
                audio.as_deref_mut(),
            ) {
                Ok(StepResult::Continue) => {
                    executed += 1;
                }
                Ok(StepResult::Blocked) => {
                    executed += 1;
                    let events = std::mem::take(&mut self.frame_events);
                    return Ok(RuntimeTick {
                        executed,
                        status: self.status.clone(),
                        wait_request: None,
                        frame_events: events,
                    });
                }
                Ok(StepResult::BlockedWithWait(req)) => {
                    executed += 1;
                    let events = std::mem::take(&mut self.frame_events);
                    return Ok(RuntimeTick {
                        executed,
                        status: self.status.clone(),
                        wait_request: Some(req),
                        frame_events: events,
                    });
                }
                Err(err) => {
                    self.status = RuntimeStatus::Faulted {
                        pc: self.pc,
                        message: err.to_string(),
                    };
                    return Err(err);
                }
            }
        }

        self.status = RuntimeStatus::Running { pc: self.pc };
        let events = std::mem::take(&mut self.frame_events);
        Ok(RuntimeTick {
            executed,
            status: self.status.clone(),
            wait_request: None,
            frame_events: events,
        })
    }

    fn step(
        &mut self,
        script: &ScriptImage<'_>,
        point_table: &PointTable,
        mem_dat: &[u8],
        assets: &CoreAssets,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
        audio: Option<&mut AudioSystem>,
    ) -> Result<StepResult, RuntimeError> {
        let insn_pc = self.pc;
        let word = self.fetch_u32(script)?;
        let hi = ((word >> 16) & 0xFFFF) as u16;
        let opcode = (word & 0xFFFF) as u16;

        if hi != 1 {
            return Err(RuntimeError::InvalidInstructionWord { pc: insn_pc, word });
        }

        if self.trace {
            let name = primary_opcode(opcode)
                .map_or_else(|| format!("op_{opcode:04X}"), |meta| meta.name.to_owned());
            log::debug!(
                "script pc=0x{insn_pc:08X} word=0x{word:08X} opcode={}({})",
                opcode,
                name
            );
        }

        match opcode {
            1 => {
                let dst = self.fetch_operand(script)?;
                let src = self.fetch_operand(script)?;
                let value = self.eval_operand(src, mem_dat)?;
                self.write_operand(dst, value)?;
            }
            2..=8 | 12..=19 | 26..=28 => {
                let dst = self.fetch_operand(script)?;
                let src = self.fetch_operand(script)?;
                let lhs = self.eval_operand(dst, mem_dat)?;
                let rhs = self.eval_operand(src, mem_dat)?;
                let value = self.eval_binary(opcode, lhs, rhs, insn_pc)?;
                self.write_operand(dst, value)?;
            }
            9 => {
                // jmp_operand: one operand whose VALUE is the point_id to jump to.
                let point_op = self.fetch_operand(script)?;
                let point_id = self.eval_operand(point_op, mem_dat)? as u32;
                self.jump_point(point_table, point_id)?;
            }
            10 => {
                let point_id = self.fetch_u32(script)?;
                let cond = self.fetch_operand(script)?;
                let value = self.eval_operand(cond, mem_dat)?;
                if value == 0 {
                    self.jump_point(point_table, point_id)?;
                }
            }
            11 => {
                let point_id = self.fetch_u32(script)?;
                self.call_stack.push(self.pc);
                self.jump_point(point_table, point_id)?;
            }
            20 => {
                let raw = self.fetch_u32(script)?;
                let slot = (raw & 0xFFFF) as usize;
                let value = self.read_var(slot)?;
                self.write_var(slot, if value == 0 { 1 } else { 0 })?;
            }
            21 => {
                self.status = RuntimeStatus::Halted { pc: self.pc };
                return Ok(StepResult::Blocked);
            }
            22 => {}
            23 => {
                let raw = self.fetch_u32(script)?;
                let dst_slot_raw = self.fetch_u32(script)?;
                let category = ((raw >> 16) & 0xFFFF) as u16;
                let index = (raw & 0xFFFF) as u16;
                let name = ext_opcode(category, index).and_then(|meta| meta.name);
                // Store for extcall handlers that need it
                self.extcall_dst_raw = dst_slot_raw;
                // Check if there is a Rust handler; if not, null-handler semantics: write 0 to
                // dst and continue (matches original behavior for null-category dispatches).
                let result = self.dispatch_extcall(
                    category,
                    index,
                    mem_dat,
                    assets,
                    point_table,
                    resource_manager,
                    sprites,
                    task_system,
                    audio,
                );
                match result {
                    ExtCallOutcome::Value(v) => {
                        let dst = Operand::decode(dst_slot_raw);
                        let _ = self.write_operand(dst, v);
                    }
                    ExtCallOutcome::Wait { value, request } => {
                        let dst = Operand::decode(dst_slot_raw);
                        let _ = self.write_operand(dst, value);
                        self.status = match request {
                            WaitRequest::Click => RuntimeStatus::WaitClick { pc: self.pc },
                            WaitRequest::Frame(_) | WaitRequest::Time(_) => {
                                RuntimeStatus::WaitFrame { pc: self.pc }
                            }
                        };
                        if self.frame_events.len() < MAX_FRAME_EVENTS {
                            self.frame_events.push(FrameEvent::WaitEmitted {
                                pc: insn_pc,
                                kind: request,
                            });
                        }
                        return Ok(StepResult::BlockedWithWait(request));
                    }
                    ExtCallOutcome::Skip => {
                        let name_str = name.unwrap_or("?");
                        log::warn!(
                            "unimplemented extcall ext_{:04X}_{:04X}.{} pc=0x{:08X} -> 0",
                            category,
                            index,
                            name_str,
                            insn_pc
                        );
                        if self.frame_events.len() < MAX_FRAME_EVENTS {
                            self.frame_events.push(FrameEvent::ExtCallSkipped {
                                pc: insn_pc,
                                category,
                                index,
                                name: name.map(str::to_owned),
                            });
                        }
                        let dst = Operand::decode(dst_slot_raw);
                        let _ = self.write_operand(dst, 0);
                    }
                    ExtCallOutcome::Block => {
                        let name_owned = name.map(str::to_owned);
                        if self.frame_events.len() < MAX_FRAME_EVENTS {
                            self.frame_events.push(FrameEvent::UnsupportedExt {
                                pc: insn_pc,
                                category,
                                index,
                                name: name_owned.clone(),
                            });
                        }
                        self.status = RuntimeStatus::UnsupportedExtCall {
                            pc: insn_pc,
                            category,
                            index,
                            name: name_owned,
                            dst_slot: dst_slot_raw,
                        };
                        return Ok(StepResult::Blocked);
                    }
                }
            }
            24 => {
                let Some(return_pc) = self.call_stack.pop() else {
                    return Err(RuntimeError::ReturnStackUnderflow { pc: insn_pc });
                };
                self.pc = return_pc;
            }
            29 => {
                let raw = self.fetch_u32(script)?;
                let slot = (raw & 0xFFFF) as usize;
                let value = self.read_var(slot)?;
                self.write_var(slot, value.wrapping_neg())?;
            }
            30 => {
                let dst = self.fetch_operand(script)?;
                let value = self
                    .stack
                    .pop()
                    .ok_or(RuntimeError::StackUnderflow { pc: insn_pc })?;
                self.write_operand(dst, value)?;
            }
            31 => {
                let src = self.fetch_operand(script)?;
                let value = self.eval_operand(src, mem_dat)?;
                self.push_stack(value, insn_pc)?;
            }
            32 => {
                let count_operand = self.fetch_operand(script)?;
                let count = self.eval_operand(count_operand, mem_dat)?;
                self.pack_args(count, insn_pc)?;
            }
            33 => {
                let count_operand = self.fetch_operand(script)?;
                let count = self.eval_operand(count_operand, mem_dat)?;
                self.drop_args(count, insn_pc)?;
            }
            252 => {
                // WaitFrame: create a 1-frame blocking task via TaskSystem.
                self.status = RuntimeStatus::WaitFrame { pc: self.pc };
                if self.frame_events.len() < MAX_FRAME_EVENTS {
                    self.frame_events.push(FrameEvent::WaitEmitted {
                        pc: insn_pc,
                        kind: WaitRequest::Frame(1),
                    });
                }
                return Ok(StepResult::BlockedWithWait(WaitRequest::Frame(1)));
            }
            253 => {
                // WaitClick: create a blocking input-wait task via TaskSystem.
                self.status = RuntimeStatus::WaitClick { pc: self.pc };
                if self.frame_events.len() < MAX_FRAME_EVENTS {
                    self.frame_events.push(FrameEvent::WaitEmitted {
                        pc: insn_pc,
                        kind: WaitRequest::Click,
                    });
                }
                return Ok(StepResult::BlockedWithWait(WaitRequest::Click));
            }
            _ => {
                let name = primary_opcode(opcode).map(|meta| meta.name.to_owned());
                if self.frame_events.len() < MAX_FRAME_EVENTS {
                    self.frame_events.push(FrameEvent::UnsupportedCmd {
                        pc: insn_pc,
                        opcode,
                        name: name.clone(),
                    });
                }
                self.status = RuntimeStatus::UnsupportedCommand {
                    pc: insn_pc,
                    opcode,
                    name,
                };
                return Ok(StepResult::Blocked);
            }
        }

        self.status = RuntimeStatus::Running { pc: self.pc };
        Ok(StepResult::Continue)
    }

    fn fetch_u32(&mut self, script: &ScriptImage<'_>) -> Result<u32, RuntimeError> {
        let pc = self.pc as usize;
        let end = pc
            .checked_add(4)
            .ok_or(RuntimeError::ArithmeticOverflow { pc: self.pc })?;
        if end > script.len() {
            return Err(RuntimeError::ReadOutOfBounds {
                pc: self.pc,
                len: script.len(),
            });
        }
        let bytes = script.bytes();
        let value = u32::from_le_bytes([bytes[pc], bytes[pc + 1], bytes[pc + 2], bytes[pc + 3]]);
        self.pc = self
            .pc
            .checked_add(4)
            .ok_or(RuntimeError::ArithmeticOverflow { pc: self.pc })?;
        Ok(value)
    }

    fn fetch_operand(&mut self, script: &ScriptImage<'_>) -> Result<Operand, RuntimeError> {
        Ok(Operand::decode(self.fetch_u32(script)?))
    }

    fn jump_point(&mut self, point_table: &PointTable, point_id: u32) -> Result<(), RuntimeError> {
        match point_table.resolve_target_pc(point_id) {
            Ok(Some(target)) => {
                self.pc = target;
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(source) => Err(RuntimeError::PointResolve {
                point_id,
                source: source.to_string(),
            }),
        }
    }

    fn eval_binary(&self, opcode: u16, lhs: i32, rhs: i32, pc: u32) -> Result<i32, RuntimeError> {
        let value = match opcode {
            2 => lhs.wrapping_add(rhs),
            3 => lhs.wrapping_sub(rhs),
            4 => lhs.wrapping_mul(rhs),
            5 => {
                if rhs == 0 {
                    return Err(RuntimeError::DivideByZero { pc });
                }
                lhs.wrapping_div(rhs)
            }
            6 => lhs & rhs,
            7 => lhs | rhs,
            8 => lhs ^ rhs,
            12 => bool_to_i32(lhs == rhs),
            13 => bool_to_i32(lhs != rhs),
            14 => bool_to_i32(lhs <= rhs),
            15 => bool_to_i32(lhs >= rhs),
            16 => bool_to_i32(lhs < rhs),
            17 => bool_to_i32(lhs > rhs),
            18 => bool_to_i32(lhs != 0 || rhs != 0),
            19 => bool_to_i32(lhs != 0 && rhs != 0),
            26 => {
                if rhs == 0 {
                    return Err(RuntimeError::DivideByZero { pc });
                }
                lhs.wrapping_rem(rhs)
            }
            27 => lhs.wrapping_shl((rhs & 31) as u32),
            28 => ((lhs as u32).wrapping_shr((rhs & 31) as u32)) as i32,
            _ => return Err(RuntimeError::UnsupportedInternalOpcode { pc, opcode }),
        };
        Ok(value)
    }

    fn eval_operand(&self, operand: Operand, mem_dat: &[u8]) -> Result<i32, RuntimeError> {
        if self.trace {
            log::debug!("  eval_operand {}", operand);
        }
        let result = match operand.kind {
            OperandKind::Immediate => Ok(operand.raw as i32),
            // kind >= 0xA: treat as a raw literal (same encoding as Immediate).
            OperandKind::LiteralSlot => Ok(operand.raw as i32),
            OperandKind::VariableSlot => self.read_var(operand.lo as usize),
            OperandKind::StackSlot => self.read_stack_slot(operand.lo as usize),
            OperandKind::ArgumentStack => self.read_argument_stack(operand.lo as usize),
            // kind 0x9: argument_base scalar field.
            OperandKind::ArgumentBase => Ok(self.argument_base),
            // kind 0x1: user_mem[vars[lo]]
            OperandKind::UserMemoryViaVar => {
                let idx = self.read_var(operand.lo as usize)? as usize;
                self.user_mem
                    .get(idx)
                    .copied()
                    .ok_or(RuntimeError::MemIndexOutOfRange {
                        index: idx,
                        len: self.user_mem.len(),
                        kind: "user_mem",
                    })
            }
            // kind 0x2: system_mem[vars[lo]]
            OperandKind::SystemMemoryViaVar => {
                let idx = self.read_var(operand.lo as usize)? as usize;
                self.system_mem
                    .get(idx)
                    .copied()
                    .ok_or(RuntimeError::MemIndexOutOfRange {
                        index: idx,
                        len: self.system_mem.len(),
                        kind: "system_mem",
                    })
            }
            // kind 0x5: temp_mem[(bank != 0 ? bank + argument_base : 0) + vars[lo]]
            // The index may alias other areas in the original flat "this" object.
            // Gracefully return 0 on out-of-range (the full aliasing semantics
            // require a unified flat memory model not yet implemented).
            OperandKind::TempMemoryViaVar => {
                let bank = operand.bank as i32;
                let base = if bank != 0 {
                    bank + self.argument_base
                } else {
                    0
                };
                let var_val = self.read_var(operand.lo as usize)?;
                let signed_idx = base.wrapping_add(var_val);
                if signed_idx < 0 || signed_idx as usize >= self.temp_mem.len() {
                    log::debug!(
                        "TempMemoryViaVar read out of range: idx={} bank={} var={}",
                        signed_idx,
                        bank,
                        var_val
                    );
                    return Ok(0);
                }
                Ok(self.temp_mem[signed_idx as usize])
            }
            // kind 0x6: MemDatDirect — read from Mem.dat bytes directly.
            OperandKind::MemDatDirect => self.read_mem_dat_i32(mem_dat, operand),
            // kind 0x7: MemDatIndirect — complex double-indirection, not yet implemented.
            OperandKind::MemDatIndirect => {
                log::warn!(
                    "MemDatIndirect operand not implemented (raw=0x{:08X}), returning 0",
                    operand.raw
                );
                Ok(0)
            }
        };
        if self.trace {
            if let Ok(v) = result {
                log::debug!("  => {v}");
            }
        }
        result
    }

    fn write_operand(&mut self, operand: Operand, value: i32) -> Result<(), RuntimeError> {
        if self.trace {
            log::debug!("  write_operand {} = {value}", operand);
        }
        match operand.kind {
            OperandKind::VariableSlot => self.write_var(operand.lo as usize, value),
            OperandKind::StackSlot => self.write_stack_slot(operand.lo as usize, value),
            // kind 0x9: argument_base scalar.
            OperandKind::ArgumentBase => {
                self.argument_base = value;
                Ok(())
            }
            // kind 0x1: user_mem[vars[lo]] = value
            OperandKind::UserMemoryViaVar => {
                let idx = self.read_var(operand.lo as usize)? as usize;
                let len = self.user_mem.len();
                let dst = self
                    .user_mem
                    .get_mut(idx)
                    .ok_or(RuntimeError::MemIndexOutOfRange {
                        index: idx,
                        len,
                        kind: "user_mem",
                    })?;
                *dst = value;
                Ok(())
            }
            // kind 0x2: system_mem[vars[lo]] = value
            OperandKind::SystemMemoryViaVar => {
                let idx = self.read_var(operand.lo as usize)? as usize;
                let len = self.system_mem.len();
                let dst = self
                    .system_mem
                    .get_mut(idx)
                    .ok_or(RuntimeError::MemIndexOutOfRange {
                        index: idx,
                        len,
                        kind: "system_mem",
                    })?;
                *dst = value;
                Ok(())
            }
            // kind 0x5: temp_mem[(bank != 0 ? bank + argument_base : 0) + vars[lo]] = value
            OperandKind::TempMemoryViaVar => {
                let bank = operand.bank as i32;
                let base = if bank != 0 {
                    bank + self.argument_base
                } else {
                    0
                };
                let var_val = self.read_var(operand.lo as usize)?;
                let signed_idx = base.wrapping_add(var_val);
                if signed_idx < 0 || signed_idx as usize >= self.temp_mem.len() {
                    log::debug!(
                        "TempMemoryViaVar write out of range: idx={} bank={} var={}",
                        signed_idx,
                        bank,
                        var_val
                    );
                    return Ok(());
                }
                self.temp_mem[signed_idx as usize] = value;
                Ok(())
            }
            // kind 0x6: MemDatDirect — write to the writable shadow copy.
            OperandKind::MemDatDirect => {
                let bank = operand.bank as i32;
                let var_val = self.read_var(operand.lo as usize)?;
                // "+16" skips the 16-byte header (4 words).
                let word_index = (bank.wrapping_add(var_val) + 4) as usize;
                let len = self.mem_dat_words.len();
                if let Some(dst) = self.mem_dat_words.get_mut(word_index) {
                    *dst = value;
                    Ok(())
                } else {
                    Err(RuntimeError::MemDatOutOfRange {
                        offset: word_index * 4,
                        len: len * 4,
                    })
                }
            }
            // kind 0x7: MemDatIndirect write — not yet implemented, log and ignore.
            OperandKind::MemDatIndirect => {
                log::warn!(
                    "MemDatIndirect operand write not implemented (raw=0x{:08X}), ignoring",
                    operand.raw
                );
                Ok(())
            }
            // kind 0x8: ArgumentStack — write to arg_area[arg_top - lo].
            OperandKind::ArgumentStack => {
                let lo = operand.lo as usize;
                let depth = self.argument_stack.len();
                if lo == 0 || lo > depth {
                    return Err(RuntimeError::ArgumentSlotOutOfRange { slot: lo, depth });
                }
                self.argument_stack[depth - lo] = value;
                Ok(())
            }
            // Immediate/LiteralSlot — write to the temp result slot (harmless, result is
            // discarded). Matches original where the pointer to a temp scratch slot is returned.
            OperandKind::Immediate | OperandKind::LiteralSlot => Ok(()),
            _ => Err(RuntimeError::UnsupportedOperandWrite {
                raw: operand.raw,
                kind: format!("{:?}", operand.kind),
            }),
        }
    }

    fn read_var(&self, slot: usize) -> Result<i32, RuntimeError> {
        self.vars
            .get(slot)
            .copied()
            .ok_or(RuntimeError::VariableOutOfRange { slot })
    }

    fn write_var(&mut self, slot: usize, value: i32) -> Result<(), RuntimeError> {
        let dst = self
            .vars
            .get_mut(slot)
            .ok_or(RuntimeError::VariableOutOfRange { slot })?;
        *dst = value;
        Ok(())
    }

    fn read_stack_slot(&self, slot: usize) -> Result<i32, RuntimeError> {
        self.stack
            .get(slot)
            .copied()
            .ok_or(RuntimeError::StackSlotOutOfRange {
                slot,
                depth: self.stack.len(),
            })
    }

    fn write_stack_slot(&mut self, slot: usize, value: i32) -> Result<(), RuntimeError> {
        let depth = self.stack.len();
        let dst = self
            .stack
            .get_mut(slot)
            .ok_or(RuntimeError::StackSlotOutOfRange { slot, depth })?;
        *dst = value;
        Ok(())
    }

    /// Read from argument_stack using 1-based lo index matching original arg_area layout.
    /// lo=1 means the last packed item (top of the group), lo=N means Nth from the top.
    /// argument_stack stores groups in reversed order (pack_args reverses before appending),
    /// so direct index = depth - lo.
    fn read_argument_stack(&self, lo: usize) -> Result<i32, RuntimeError> {
        let depth = self.argument_stack.len();
        let index = depth
            .checked_sub(lo)
            .ok_or(RuntimeError::ArgumentSlotOutOfRange { slot: lo, depth })?;
        Ok(self.argument_stack[index])
    }

    fn read_mem_dat_i32(&self, mem_dat: &[u8], operand: Operand) -> Result<i32, RuntimeError> {
        // Original formula: mem_dat_ptr + 4*(bank + vars[lo]) + 16
        // The "+16" skips the 16-byte header present in Mem.dat.
        let bank = operand.bank as i32;
        let var_val = self.read_var(operand.lo as usize)?;
        let word_index = bank.wrapping_add(var_val);
        // Convert to byte offset: 4 bytes per word, +16 for header.
        let byte_offset = (word_index as i64) * 4 + 16;
        if byte_offset < 0 {
            return Err(RuntimeError::MemDatOutOfRange {
                offset: byte_offset as usize,
                len: mem_dat.len(),
            });
        }
        let offset = byte_offset as usize;
        let end = offset
            .checked_add(4)
            .ok_or(RuntimeError::ArithmeticOverflow { pc: self.pc })?;
        if end > mem_dat.len() {
            return Err(RuntimeError::MemDatOutOfRange {
                offset,
                len: mem_dat.len(),
            });
        }
        Ok(i32::from_le_bytes([
            mem_dat[offset],
            mem_dat[offset + 1],
            mem_dat[offset + 2],
            mem_dat[offset + 3],
        ]))
    }

    fn push_stack(&mut self, value: i32, pc: u32) -> Result<(), RuntimeError> {
        if self.stack.len() >= DEFAULT_STACK_LIMIT {
            return Err(RuntimeError::StackOverflow {
                pc,
                limit: DEFAULT_STACK_LIMIT,
            });
        }
        self.stack.push(value);
        Ok(())
    }

    fn pack_args(&mut self, count: i32, pc: u32) -> Result<(), RuntimeError> {
        if count < 0 {
            return Err(RuntimeError::NegativeCount { pc, count });
        }
        let count = count as usize;
        if count > self.stack.len() {
            return Err(RuntimeError::StackUnderflow { pc });
        }
        let start = self.stack.len() - count;
        // split_off gives items in original push order (oldest first, newest last).
        // read_argument_stack uses index = depth - lo, so lo=1 returns the last element
        // (newest/top), which is the correct original-engine semantics.
        let mut packed = self.stack.split_off(start);
        self.argument_stack.append(&mut packed);
        Ok(())
    }

    fn drop_args(&mut self, count: i32, pc: u32) -> Result<(), RuntimeError> {
        if count < 0 {
            return Err(RuntimeError::NegativeCount { pc, count });
        }
        let count = count as usize;
        if count > self.argument_stack.len() {
            return Err(RuntimeError::ArgumentStackUnderflow { pc });
        }
        let new_len = self.argument_stack.len() - count;
        self.argument_stack.truncate(new_len);
        Ok(())
    }

    /// Read extcall argument by 1-based index.
    /// lo=1 is the most recently packed item (top of the argument group).
    pub fn extcall_arg(&self, lo: usize) -> i32 {
        let depth = self.argument_stack.len();
        if lo == 0 || lo > depth {
            return 0;
        }
        self.argument_stack[depth - lo]
    }

    /// Dispatch an extcall by category and index.
    /// Returns Value(n) if handled, Skip to continue with 0, or Block to pause execution.
    fn dispatch_extcall(
        &mut self,
        category: u16,
        index: u16,
        _mem_dat: &[u8],
        assets: &CoreAssets,
        point_table: &PointTable,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let nls = resource_manager
            .as_ref()
            .map(|manager| manager.nls())
            .unwrap_or(Nls::ShiftJis);

        if let Some(name) = ext_opcode(category, index).and_then(|opcode| opcode.name) {
            match name {
                "text_init" => return self.dispatch_text_stub(0),
                "text_set_icon" => return self.dispatch_text_stub(1),
                "text" => return self.dispatch_text_stub(2),
                "text_hide" => return self.dispatch_text_stub(3),
                "text_show" => return self.dispatch_text_stub(4),
                "text_set_btn" => return self.dispatch_text_stub(5),
                "text_uninit" => return self.dispatch_text_stub(6),
                "text_set_rect_invalid_param" => return self.dispatch_text_stub(7),
                "text_clear" => return self.dispatch_text_stub(8),
                "text_get_time" => return self.dispatch_text_stub(10),
                "text_window_set_alpha" => return self.dispatch_text_stub(11),
                "text_voice_play" => return self.dispatch_text_stub(12),
                "text_set_icon_animation_time" => return self.dispatch_text_stub(14),
                "text_w" => return self.dispatch_text_stub(15),
                "text_a" => return self.dispatch_text_stub(16),
                "text_wa" => return self.dispatch_text_stub(17),
                "text_n" => return self.dispatch_text_stub(18),
                "text_cat" => return self.dispatch_text_stub(19),
                "set_history" => return self.dispatch_text_stub(20),
                "is_text_visible" => return self.dispatch_text_stub(21),
                "text_set_base" => return self.dispatch_text_stub(22),
                "enable_voice_cut" => return self.dispatch_text_stub(23),
                "is_voice_cut" => return self.dispatch_text_stub(24),
                "texttimecheckset" => return self.dispatch_text_stub(25),
                "text_set_color" => return self.dispatch_text_stub(28),
                "textredraw" => return self.dispatch_text_stub(29),
                "set_text_mode" => return self.dispatch_text_stub(30),
                "text_init_visualnovelmode" => return self.dispatch_text_stub(31),
                "text_set_icon_mode" => return self.dispatch_text_stub(32),
                "text_vn_br" => return self.dispatch_text_stub(33),
                "voice_set_volume" => return self.dispatch_text_stub(50),
                "voice_get_volume" => return self.dispatch_text_stub(51),
                "voice_enable" => return self.dispatch_text_stub(53),
                "is_voice_enable" => return self.dispatch_text_stub(54),
                "bgv_enable" => return self.dispatch_text_stub(58),
                "get_voice_ex_volume" => return self.dispatch_text_stub(59),
                "set_voice_ex_volume" => return self.dispatch_text_stub(60),
                "voice_check_enable" => return self.dispatch_text_stub(61),
                "voice_autopan_initialize" => return self.dispatch_text_stub(62),
                "voice_autopan_enable" => return self.dispatch_text_stub(63),
                "set_voice_autopan_size_over" => return self.dispatch_text_stub(64),
                "is_voice_autopan_enable" => return self.dispatch_text_stub(65),
                "bgv_mute" => return self.dispatch_text_stub(68),
                "set_bgv_volume" => return self.dispatch_text_stub(69),
                "get_bgv_volume" => return self.dispatch_text_stub(70),
                "set_bgv_auto_volume" => return self.dispatch_text_stub(71),
                "voice_mute" => return self.dispatch_text_stub(72),
                "wait" => return self.dispatch_wait_ext(0),
                "wait_click" => return self.dispatch_wait_ext(1),
                "wait_sync_begin" => return self.dispatch_wait_ext(2),
                "wait_sync_release" => return self.dispatch_wait_ext(3),
                "wait_sync_end" => return self.dispatch_wait_ext(4),
                "wait_clear" => return self.dispatch_wait_ext(6),
                "wait_click_no_anim" => return self.dispatch_wait_ext(7),
                "wait_sync_get_time" => return self.dispatch_wait_ext(8),
                "wait_time_push" => return self.dispatch_wait_ext(9),
                "wait_time_pop" => return self.dispatch_wait_ext(10),
                "select_init" => return self.dispatch_select_stub(0),
                "select_clear" => return self.dispatch_select_stub(4),
                "select_set_offset" => return self.dispatch_select_stub(5),
                "select_set_process" => return self.dispatch_select_stub(6),
                "select_lock" => return self.dispatch_select_stub(7),
                "get_select_on_key" => return self.dispatch_select_stub(8),
                "get_select_pull_key" => return self.dispatch_select_stub(9),
                "get_select_push_key" => return self.dispatch_select_stub(10),
                "set_font_size" => return self.dispatch_font_system_stub(27),
                "get_font_size" => return self.dispatch_font_system_stub(28),
                "get_font_type" => return self.dispatch_font_system_stub(29),
                "set_font_effect" => return self.dispatch_font_system_stub(30),
                "get_font_effect" => return self.dispatch_font_system_stub(31),
                "set_font_color" => return self.dispatch_font_system_stub(19),
                "get_font_color" => return self.dispatch_font_system_stub(55),
                "input_clear" => return self.dispatch_font_system_stub(35),
                "change_window_size" => return self.dispatch_font_system_stub(36),
                "change_aspect_mode" => return self.dispatch_font_system_stub(37),
                "get_aspect_mode" => return self.dispatch_font_system_stub(40),
                "enable_window_change" => return self.dispatch_font_system_stub(46),
                "is_enable_window_change" => return self.dispatch_font_system_stub(47),
                "history_skip" => return self.dispatch_font_system_stub(57),
                "save" => return self.dispatch_save_stub(0),
                "load" => return self.dispatch_save_stub(1),
                "save_set_title" => return self.dispatch_save_stub(2),
                "save_data" => return self.dispatch_save_stub(3),
                "save_set_thumbnail_size" => return self.dispatch_save_stub(4),
                "save_set_font_size" => return self.dispatch_save_stub(7),
                "is_save" => return self.dispatch_save_stub(9),
                "savepoint" => return self.dispatch_save_stub(11),
                "save_set_text_rect" => return self.dispatch_save_stub(15),
                "get_new_savefile" => return self.dispatch_save_stub(17),
                "save_set_font_type" => return self.dispatch_save_stub(23),
                "set_load_after_process" => return self.dispatch_save_stub(24),
                "savesystemdata" => return self.dispatch_save_stub(25),
                "save_set_font_effect" => return self.dispatch_save_stub(26),
                "save_set_font_color_0x_0x" => return self.dispatch_save_stub(27),
                "save_lock_not_open_savefileno" => return self.dispatch_save_stub(32),
                "is_save_lock" => return self.dispatch_save_stub(33),
                "is_prev_data" => return self.dispatch_save_stub(34),
                "save_point_clear" => return self.dispatch_save_stub(35),
                "save_point_lock" => return self.dispatch_save_stub(36),
                "system_btn_set" => return self.dispatch_system_button_stub(0),
                "system_btn_release" => return self.dispatch_system_button_stub(1),
                "system_btn_enable" => return self.dispatch_system_button_stub(2),
                "history_init_0x_0x" => return self.dispatch_history_stub(0),
                "historybegin_lpbyte_ptagdata_sztext" => return self.dispatch_history_stub(1),
                "history_end" => return self.dispatch_history_stub(2),
                "history_get_height" => return self.dispatch_history_stub(5),
                "history_set_rect" => return self.dispatch_history_stub(10),
                "history_clear" => return self.dispatch_history_stub(11),
                "history_set" => return self.dispatch_history_stub(12),
                "history_get_text" => return self.dispatch_history_stub(20),
                "movie_play" => return self.ext_movie_play(assets, nls, resource_manager),
                "msp_set_loop_sp_ep" => {
                    return self.ext_msp_set_loop_sp_ep(
                        assets,
                        nls,
                        resource_manager,
                        sprites,
                        task_system,
                    );
                }
                "msp_cls" => return self.ext_msp_cls(),
                "msp_wait" => return self.ext_msp_wait(),
                "msp_lock" => return self.ext_msp_lock(),
                "msp_unlock" => return self.ext_msp_unlock(),
                "msp_play" => return self.ext_msp_play(),
                "msp_stop" => return self.ext_msp_stop(),
                "create_thread" => return self.dispatch_thread_stub(0),
                "exit_thread" => return self.dispatch_thread_stub(1),
                "get_thread" => return self.dispatch_thread_stub(3),
                "create_message" => return self.dispatch_message_stub(0, point_table),
                "get_message" => return self.dispatch_message_stub(1, point_table),
                "get_message_param" => return self.dispatch_message_stub(2, point_table),
                "run_no_wait" => return self.dispatch_run_ext(1),
                "run_stack" => return self.dispatch_run_ext(2),
                "random" => return self.dispatch_random_ext(0),
                _ => {}
            }
        }

        match category {
            3 => {
                self.dispatch_sprite_ext(index, assets, nls, resource_manager, sprites, task_system)
            }
            4 => self.dispatch_bgm_ext(index, assets, nls, resource_manager, audio),
            5 => self.dispatch_se_ext(index, assets, nls, resource_manager, audio),
            13 => self.dispatch_voice_ext(index, assets, nls, resource_manager, audio),
            18 => match index {
                3 => self.ext_arg_probe(2, 1),
                5 => self.ext_arg_probe(3, 1),
                6 => self.ext_arg_get(),
                15 => self.ext_arg_probe(0, 1),
                21 => self.ext_string_non_empty(assets, nls),
                28 => self.ext_arg_probe(1, 1),
                29 => self.ext_arg_probe(0, 1),
                30 => self.ext_open_file(assets, nls, resource_manager),
                31 => self.ext_read_file(),
                33 => self.ext_set_file_pointer(),
                34 => self.ext_file_string(),
                35 => self.ext_arg_probe(1, 1),
                36 => self.ext_sz_buf(assets, nls, resource_manager),
                37 => self.ext_get_private_profile_int(assets, nls, resource_manager),
                _ => ExtCallOutcome::Skip,
            },
            7 => self.dispatch_wait_ext(index),
            8 => self.dispatch_button_ext(index, assets, nls, resource_manager, sprites),
            9 => self.dispatch_font_system_stub(index),
            10 => self.dispatch_save_stub(index),
            12 => self.dispatch_system_button_stub(index),
            14 => self.dispatch_history_stub(index),
            2 => self.dispatch_text_stub(index),
            6 => self.dispatch_select_stub(index),
            15 => self.dispatch_misc_system_stub(index),
            21 => self.dispatch_thread_stub(index),
            22 => self.dispatch_run_ext(index),
            17 => self.dispatch_action_stub(index),
            20 => self.dispatch_random_ext(index),
            23 => self.dispatch_message_stub(index, point_table),
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_wait_ext(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(2);
                let duration_ms = args.first().copied().unwrap_or(1).max(1);
                let skip_cancel = args.get(1).copied().unwrap_or(0);
                log::debug!("[trace-script] wait duration_ms={duration_ms}");
                if skip_cancel != 0 {
                    log::debug!("[trace-script] wait skip_cancel={skip_cancel}");
                }
                ExtCallOutcome::Wait {
                    value: 1,
                    request: WaitRequest::Time(duration_ms as u32),
                }
            }
            1 => {
                let args = self.pop_ext_args(1);
                let duration_ms = args.first().copied().unwrap_or(-1);
                log::debug!("[trace-script] wait_click duration_ms={duration_ms}");
                if duration_ms < 0 {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Click,
                    }
                } else {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Time(duration_ms.max(1) as u32),
                    }
                }
            }
            2 => {
                self.pop_ext_args(0);
                self.wait_sync_begin_ms = self.pal_time_ms;
                self.wait_sync_release = None;
                log::debug!("[trace-script] wait_sync_begin");
                ExtCallOutcome::Value(1)
            }
            3 => {
                if let Some(active) = self.wait_sync_release {
                    let elapsed = self.pal_time_ms.wrapping_sub(active.start_ms);
                    if elapsed >= active.duration_ms {
                        self.wait_sync_release = None;
                        log::debug!("[trace-script] wait_sync_release elapsed_ms={elapsed}");
                        ExtCallOutcome::Value(1)
                    } else {
                        let remaining = active.duration_ms - elapsed;
                        log::debug!("[trace-script] wait_sync_release remaining_ms={remaining}");
                        ExtCallOutcome::Wait {
                            value: 1,
                            request: WaitRequest::Time(remaining.max(1)),
                        }
                    }
                } else {
                    let args = self.pop_ext_args(1);
                    let duration_ms = args.first().copied().unwrap_or(1).max(1) as u32;
                    self.wait_sync_release = Some(WaitSyncRelease {
                        duration_ms,
                        start_ms: self.pal_time_ms,
                    });
                    log::debug!("[trace-script] wait_sync duration_ms={duration_ms}");
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Time(duration_ms.max(1)),
                    }
                }
            }
            4 => {
                self.pop_ext_args(0);
                self.wait_sync_begin_ms = 0;
                self.wait_sync_release = None;
                log::debug!("[trace-script] wait_sync_end");
                ExtCallOutcome::Value(1)
            }
            6 => {
                self.pop_ext_args(0);
                self.wait_sync_begin_ms = 0;
                self.wait_sync_release = None;
                self.wait_time_stack.clear();
                log::debug!("[trace-script] wait_clear");
                ExtCallOutcome::Value(1)
            }
            7 => {
                let args = self.pop_ext_args(1);
                let duration_ms = args.first().copied().unwrap_or(1);
                log::debug!("[trace-script] wait_click_no_anim duration_ms={duration_ms}");
                if duration_ms < 0 {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Click,
                    }
                } else {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Time(duration_ms.max(1) as u32),
                    }
                }
            }
            8 => {
                self.pop_ext_args(0);
                let elapsed = self.pal_time_ms.wrapping_sub(self.wait_sync_begin_ms) as i32;
                log::debug!("[trace-script] wait_sync_get_time elapsed_ms={elapsed}");
                ExtCallOutcome::Value(elapsed)
            }
            9 => {
                self.pop_ext_args(0);
                self.wait_time_stack.push(self.pal_time_ms);
                log::debug!("[trace-script] wait_time_push time_ms={}", self.pal_time_ms);
                ExtCallOutcome::Value(1)
            }
            10 => {
                self.pop_ext_args(0);
                let elapsed = self
                    .wait_time_stack
                    .pop()
                    .map(|start| self.pal_time_ms.wrapping_sub(start) as i32)
                    .unwrap_or(0);
                log::debug!("[trace-script] wait_time_pop elapsed_ms={elapsed}");
                ExtCallOutcome::Value(elapsed)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_random_ext(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(2);
                if args.len() < 2 {
                    return ExtCallOutcome::Block;
                }
                let lo = args[0].min(args[1]);
                let hi = args[0].max(args[1]);
                let span = hi.saturating_sub(lo).saturating_add(1).max(1);
                let value = lo.saturating_add(
                    (self.random_state.random_ex().unsigned_abs() % span as u32) as i32,
                );
                ExtCallOutcome::Value(value)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_font_system_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 | 1 => {
                let args = self.pop_ext_args(1);
                let loaded = args.first().copied().unwrap_or(0) != 0;
                if index == 1 {
                    self.font_state.set_ex_font_loaded(loaded);
                }
                ExtCallOutcome::Value(1)
            }
            2 => {
                self.pop_ext_args(1);
                self.font_state.set_ex_font_loaded(false);
                ExtCallOutcome::Value(1)
            }
            3 => {
                let args = self.pop_ext_args(1);
                let effect = args.first().copied().unwrap_or(0).max(0) as u16;
                self.font_state.set_effect(effect);
                ExtCallOutcome::Value(1)
            }
            7 | 17 | 27 => {
                let args = self.pop_ext_args(1);
                let old = i32::from(self.font_state.font_size());
                if let Some(size) = args.first().copied() {
                    self.font_state.set_font_size(size.max(1) as u16);
                }
                ExtCallOutcome::Value(old)
            }
            8 | 18 => {
                let args = self.pop_ext_args(1);
                let old = i32::from(self.font_state.font_type());
                if let Some(font_type) = args.first().copied() {
                    self.font_state.set_type(font_type.max(0) as u16);
                }
                ExtCallOutcome::Value(old)
            }
            11 | 12 | 14 | 15 | 52 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            19 | 20 => {
                let args = self.pop_ext_args(2);
                if args.len() >= 2 {
                    self.font_state.set_color(args[0] as u32, args[1] as u32);
                }
                ExtCallOutcome::Value(1)
            }
            28 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.font_state.font_size()))
            }
            29 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.font_state.font_type()))
            }
            30 => {
                let args = self.pop_ext_args(1);
                let old = i32::from(self.font_state.effect());
                if let Some(effect) = args.first().copied() {
                    self.font_state.set_effect(effect.max(0) as u16);
                }
                ExtCallOutcome::Value(old)
            }
            31 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.font_state.effect()))
            }
            32 | 33 | 34 | 54 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(0)
            }
            35 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(1)
            }
            36 | 37 | 38 | 43 | 44 | 45 | 46 | 48 | 49 | 51 | 57 | 60 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            40 | 47 | 50 | 55 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(match index {
                    40 => 0,
                    47 => 1,
                    50 => 0,
                    55 => self.font_state.color().0 as i32,
                    _ => 0,
                })
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_text_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(1);
                self.text_state.initialized = true;
                self.text_state.visible = true;
                self.text_state.mode = args.first().copied().unwrap_or(0);
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            1 => {
                let args = self.pop_ext_args(1);
                self.text_state.icon = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            2 => {
                let args = self.pop_ext_args(1);
                self.text_state.last_text_value = args.first().copied().unwrap_or(0);
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            3 => {
                self.pop_ext_args(0);
                self.text_state.visible = false;
                ExtCallOutcome::Value(1)
            }
            4 => {
                self.pop_ext_args(0);
                self.text_state.visible = true;
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            5 => {
                let args = self.pop_ext_args(1);
                self.text_state.button = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            6 => {
                self.pop_ext_args(0);
                self.text_state = TextSubsystemState::default();
                ExtCallOutcome::Value(1)
            }
            8 => {
                self.pop_ext_args(0);
                self.text_state.last_text_value = 0;
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            10 => {
                self.pop_ext_args(1);
                let elapsed = self
                    .pal_time_ms
                    .wrapping_sub(self.text_state.last_event_time_ms)
                    as i32;
                ExtCallOutcome::Value(elapsed)
            }
            11 => {
                let args = self.pop_ext_args(2);
                self.text_state.alpha = args.first().copied().unwrap_or(self.text_state.alpha);
                ExtCallOutcome::Value(1)
            }
            12 => {
                let args = self.pop_ext_args(1);
                self.text_state.last_text_value = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            14 => {
                let args = self.pop_ext_args(1);
                self.text_state.icon = args.first().copied().unwrap_or(self.text_state.icon);
                ExtCallOutcome::Value(1)
            }
            15 | 16 | 17 | 18 | 19 => {
                self.pop_ext_args(match index {
                    15 | 16 | 17 => 1,
                    _ => 0,
                });
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            20 => {
                let args = self.pop_ext_args(1);
                self.text_state.history_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            21 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.visible))
            }
            22 => {
                let args = self.pop_ext_args(1);
                self.text_state.base = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            23 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_cut_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            24 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.voice_cut_enabled))
            }
            25 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            28 => {
                let args = self.pop_ext_args(1);
                self.text_state.last_text_value = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            29 => {
                self.pop_ext_args(0);
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            30 => {
                let args = self.pop_ext_args(1);
                self.text_state.mode = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            31 => {
                self.pop_ext_args(0);
                self.text_state.initialized = true;
                self.text_state.visible = true;
                ExtCallOutcome::Value(1)
            }
            32 => {
                let args = self.pop_ext_args(1);
                self.text_state.icon = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            33 => {
                self.pop_ext_args(0);
                self.text_state.last_event_time_ms = self.pal_time_ms;
                ExtCallOutcome::Value(1)
            }
            40 | 43 | 45 | 46 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(1)
            }
            38 | 39 | 41 | 42 | 44 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(0)
            }
            49 | 57 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(1)
            }
            50 | 60 | 69 | 71 => {
                let args = self.pop_ext_args(1);
                let value = args.first().copied().unwrap_or(100);
                match index {
                    50 | 60 => self.text_state.voice_volume = value,
                    69 | 71 => self.text_state.bgv_volume = value,
                    _ => {}
                }
                ExtCallOutcome::Value(1)
            }
            51 | 59 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.text_state.voice_volume)
            }
            53 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            54 | 61 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.voice_enabled))
            }
            58 => {
                let args = self.pop_ext_args(1);
                self.text_state.bgv_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            62 => {
                self.pop_ext_args(0);
                self.text_state.voice_autopan_enabled = false;
                ExtCallOutcome::Value(1)
            }
            63 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_autopan_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            64 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            65 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.voice_autopan_enabled))
            }
            68 | 72 => {
                let args = self.pop_ext_args(1);
                self.text_state.bgv_muted = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            70 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.text_state.bgv_volume)
            }
            48 | 52 | 56 | 66 | 67 | 73 | 74 => {
                self.pop_ext_args(match index {
                    48 | 56 | 73 => 2,
                    52 => 3,
                    66 => 1,
                    _ => 0,
                });
                ExtCallOutcome::Value(1)
            }
            _ => return ExtCallOutcome::Skip,
        }
    }

    fn dispatch_select_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(4);
                if args.len() < 4 {
                    return ExtCallOutcome::Block;
                }
                self.select_state.initialized = true;
                self.select_state.text_value = args[0];
                self.select_state.colors = [
                    args[1] | 0xFF00_0000u32 as i32,
                    args[2] | 0xFF00_0000u32 as i32,
                    args[3] | 0xFF00_0000u32 as i32,
                ];
                ExtCallOutcome::Value(1)
            }
            1 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(if self.select_state.locked { -1 } else { 0 })
            }
            4 => {
                self.pop_ext_args(0);
                self.select_state = SelectSubsystemState::default();
                ExtCallOutcome::Value(1)
            }
            5 => {
                let args = self.pop_ext_args(2);
                if args.len() < 2 {
                    return ExtCallOutcome::Block;
                }
                self.select_state.offsets.insert(args[0], args[1] * 3);
                ExtCallOutcome::Value(1)
            }
            6 => {
                let args = self.pop_ext_args(3);
                if args.len() < 3 {
                    return ExtCallOutcome::Block;
                }
                self.select_state.process = [args[0], args[1], args[2]];
                ExtCallOutcome::Value(1)
            }
            7 => {
                let args = self.pop_ext_args(1);
                self.select_state.locked = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            8 | 9 | 10 => {
                self.pop_ext_args(0);
                if self.select_state.locked || self.select_state.last_key == 0 {
                    ExtCallOutcome::Value(-1)
                } else {
                    ExtCallOutcome::Value(self.select_state.last_key)
                }
            }
            12 | 14 => {
                let args = self.pop_ext_args(1);
                self.select_state.locked = args.first().copied().unwrap_or(0) != 0;
                ExtCallOutcome::Value(1)
            }
            13 | 15 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.select_state.locked))
            }
            17 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(0)
            }
            _ => return ExtCallOutcome::Skip,
        }
    }

    fn dispatch_button_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        match index {
            1 => return self.ext_btn_uninit(sprites),
            3 => return self.ext_btn_set(assets, nls, resource_manager, sprites),
            4 | 8 | 19 | 20 => return self.ext_btn_release(index, sprites),
            6 => return self.ext_btn_set_pos(sprites),
            15 | 16 | 18 | 21 => return self.ext_btn_ctrl(index, sprites),
            _ => {}
        }
        let arity = match index {
            0 => 3,
            10 => 5,
            13 => 2,
            12 => 0,
            23 => 1,
            41 => 2,
            42 => 3,
            43 | 45 | 50 | 52 | 53 | 54 | 57 | 58 => 1,
            44 | 46 => 0,
            47 | 48 | 49 | 51 | 55 | 56 => 2,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(match index {
            43 | 45 | 50 | 52 | 53 | 54 | 57 | 58 => 0,
            _ => 1,
        })
    }

    fn dispatch_save_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 | 1 => {
                let args = self.pop_ext_args(1);
                self.save_state.last_slot = args.first().copied().unwrap_or(0);
                self.save_state.last_result = if self.save_state.locked { 0 } else { 1 };
                ExtCallOutcome::Value(self.save_state.last_result)
            }
            2 => {
                let args = self.pop_ext_args(1);
                self.save_state.title = args.first().copied().unwrap_or(0);
                log::debug!(
                    "[trace-save] save_set_title value={}",
                    self.save_state.title
                );
                ExtCallOutcome::Value(1)
            }
            3 | 5 | 6 | 12 | 14 | 16 | 21 | 22 | 25 | 28 | 29 | 30 | 31 | 35 => {
                self.pop_ext_args(match index {
                    3 | 5 | 6 | 12 | 13 | 14 | 16 | 21 | 22 | 25 | 28 | 29 | 30 | 31 => 1,
                    _ => 0,
                });
                ExtCallOutcome::Value(1)
            }
            4 => {
                let args = self.pop_ext_args(2);
                self.save_state.thumbnail_size = [
                    args.first().copied().unwrap_or(0),
                    args.get(1).copied().unwrap_or(0),
                ];
                ExtCallOutcome::Value(1)
            }
            7 => {
                let args = self.pop_ext_args(1);
                self.save_state.font_size = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            8 | 13 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(0)
            }
            9 => {
                let args = self.pop_ext_args(1);
                self.save_state.last_slot = args.first().copied().unwrap_or(0);
                self.save_state.last_result = if self.save_state.locked { 0 } else { 1 };
                ExtCallOutcome::Value(self.save_state.last_result)
            }
            10 | 17 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(0)
            }
            15 => {
                let args = self.pop_ext_args(2);
                self.save_state.text_rect = [
                    args.first().copied().unwrap_or(0),
                    args.get(1).copied().unwrap_or(0),
                    self.save_state.text_rect[2],
                    self.save_state.text_rect[3],
                ];
                ExtCallOutcome::Value(1)
            }
            11 | 32 => {
                let args = self.pop_ext_args(1);
                self.save_state.last_slot = args.first().copied().unwrap_or(0);
                self.save_state.last_result = if self.save_state.locked { 0 } else { 1 };
                ExtCallOutcome::Value(self.save_state.last_result)
            }
            23 => {
                let args = self.pop_ext_args(1);
                self.save_state.font_type = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            24 => {
                let args = self.pop_ext_args(1);
                self.save_state.last_result = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            26 => {
                let args = self.pop_ext_args(1);
                self.save_state.font_effect = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            27 => {
                let args = self.pop_ext_args(1);
                self.save_state.font_color = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            33 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.save_state.locked))
            }
            34 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.save_state.last_result != 0))
            }
            36 => {
                self.pop_ext_args(0);
                self.save_state.locked = true;
                ExtCallOutcome::Value(1)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_system_button_stub(&mut self, index: u16) -> ExtCallOutcome {
        let arity = match index {
            0 | 1 | 2 => 1,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(1)
    }

    fn dispatch_history_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(9);
                if args.len() < 9 {
                    return ExtCallOutcome::Block;
                }
                self.history_state.initialized = true;
                self.history_state.layout = [
                    args[0], args[1], args[2], args[3], args[4], args[5], args[6],
                ];
                self.history_state.colors = [
                    args[7] | 0xFF00_0000u32 as i32,
                    args.get(8).copied().unwrap_or(0) | 0xFF00_0000u32 as i32,
                ];
                self.history_state.rect = [0, 0, 1920, 1080];
                ExtCallOutcome::Value(1)
            }
            1 => {
                self.pop_ext_args(0);
                self.history_state.active = true;
                ExtCallOutcome::Value(1)
            }
            2 => {
                self.pop_ext_args(0);
                self.history_state.active = false;
                ExtCallOutcome::Value(1)
            }
            5 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.history_state.height)
            }
            10 => {
                let args = self.pop_ext_args(4);
                if args.len() < 4 {
                    return ExtCallOutcome::Block;
                }
                self.history_state.rect = [args[0], args[1], args[2], args[3]];
                ExtCallOutcome::Value(1)
            }
            11 => {
                self.pop_ext_args(0);
                self.history_state.records.clear();
                ExtCallOutcome::Value(1)
            }
            12 => {
                let args = self.pop_ext_args(1);
                self.history_state.current_text_value = args.first().copied().unwrap_or(0);
                self.history_state.records.push([
                    self.history_state.current_text_value,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    self.pal_time_ms as i32,
                ]);
                self.history_state.height = self
                    .history_state
                    .height
                    .saturating_add(self.font_state.font_size() as i32);
                ExtCallOutcome::Value(1)
            }
            17 | 18 | 19 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            20 => {
                self.pop_ext_args(4);
                ExtCallOutcome::Value(self.history_state.records.len() as i32)
            }
            _ => return ExtCallOutcome::Skip,
        }
    }

    fn dispatch_thread_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                self.pop_ext_args(0);
                self.thread_state.next_id = self.thread_state.next_id.saturating_add(1).max(1);
                let id = self.thread_state.next_id;
                self.thread_state.active.insert(id, true);
                self.thread_state.last_id = id;
                ExtCallOutcome::Value(id)
            }
            2 => {
                self.pop_ext_args(0);
                if self.thread_state.last_id != 0 {
                    self.thread_state
                        .active
                        .insert(self.thread_state.last_id, false);
                }
                ExtCallOutcome::Value(1)
            }
            3 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.thread_state.last_id)
            }
            _ => return ExtCallOutcome::Skip,
        }
    }

    fn dispatch_run_ext(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(3);
                if args.len() < 3 {
                    return ExtCallOutcome::Block;
                }
                let effect_id = args[0];
                let arg1 = args[1];
                let arg2 = args[2];
                if !(0..=100).contains(&effect_id) {
                    log::warn!("[trace-run] run effect_id out of range: {effect_id}");
                    return ExtCallOutcome::Value(0);
                }
                if self.run_pipeline.run_stack_enabled {
                    self.run_pipeline.pending_effect_id = effect_id;
                    self.run_pipeline.pending_arg1 = arg1;
                    self.run_pipeline.pending_arg2 = arg2;
                    self.run_pipeline.pending_kind = 1;
                    log::debug!(
                        "[trace-run] run queued stack effect={effect_id} arg1={arg1} arg2={arg2}"
                    );
                    return ExtCallOutcome::Value(1);
                }
                self.run_pipeline.effect_active = effect_id != 0;
                self.run_pipeline.last_run_time_ms = self.pal_time_ms;
                self.run_pipeline.last_run_arg1 = arg1;
                let _ = self.effect_system.effect_ex(
                    effect_id.max(0) as u32,
                    arg1.max(1) as u32,
                    arg2,
                    false,
                    self.pal_time_ms,
                );
                log::debug!("[trace-run] run effect={effect_id} arg1={arg1} arg2={arg2}");
                ExtCallOutcome::Value(1)
            }
            1 => {
                self.pop_ext_args(0);
                if self.run_pipeline.run_stack_enabled {
                    self.run_pipeline.pending_kind = 2;
                    log::debug!("[trace-run] run_no_wait queued stack");
                } else {
                    self.run_pipeline.no_wait_latch = true;
                    log::debug!("[trace-run] run_no_wait");
                }
                ExtCallOutcome::Value(1)
            }
            2 => {
                let args = self.pop_ext_args(1);
                let enabled = args.first().copied().unwrap_or(0) != 0;
                self.run_pipeline.run_stack_enabled = enabled;
                if enabled {
                    self.run_pipeline.pending_kind = 0;
                } else {
                    match self.run_pipeline.pending_kind {
                        1 => {
                            self.run_pipeline.effect_active =
                                self.run_pipeline.pending_effect_id != 0;
                            self.run_pipeline.last_run_time_ms = self.pal_time_ms;
                            self.run_pipeline.last_run_arg1 = self.run_pipeline.pending_arg1;
                            let _ = self.effect_system.effect_ex(
                                self.run_pipeline.pending_effect_id.max(0) as u32,
                                self.run_pipeline.pending_arg1.max(1) as u32,
                                self.run_pipeline.pending_arg2,
                                false,
                                self.pal_time_ms,
                            );
                            log::debug!(
                                "[trace-run] run_stack flush effect={} arg1={} arg2={}",
                                self.run_pipeline.pending_effect_id,
                                self.run_pipeline.pending_arg1,
                                self.run_pipeline.pending_arg2
                            );
                        }
                        2 => {
                            self.run_pipeline.no_wait_latch = true;
                            log::debug!("[trace-run] run_stack flush no_wait");
                        }
                        _ => {}
                    }
                    self.run_pipeline.pending_kind = 0;
                }
                log::debug!("[trace-run] run_stack enabled={enabled}");
                ExtCallOutcome::Value(1)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_misc_system_stub(&mut self, index: u16) -> ExtCallOutcome {
        let arity = match index {
            2 => 1,
            4 | 5 => 0,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(1)
    }

    fn dispatch_action_stub(&mut self, index: u16) -> ExtCallOutcome {
        let arity = match index {
            1 => 2,
            3 => 1,
            8 => 4,
            23 => 1,
            24 => 0,
            25 => 0,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(match index {
            24 => 0,
            _ => 1,
        })
    }

    fn dispatch_message_stub(&mut self, index: u16, point_table: &PointTable) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(1);
                let value = args.first().copied().unwrap_or(0);
                self.message_state.slots = [GameMessage::default(); 8];
                self.message_state.next_id = self.message_state.next_id.saturating_add(1).max(1);
                let slot = (value.max(0) as usize).min(7);
                let message = GameMessage {
                    active: true,
                    id: self.message_state.next_id,
                    value,
                    param: if value != 0 {
                        point_table
                            .resolve_target_pc(value as u32)
                            .ok()
                            .flatten()
                            .unwrap_or(0) as i32
                    } else {
                        0
                    },
                };
                self.message_state.slots[slot] = message;
                self.message_state.queue.push(message);
                ExtCallOutcome::Value(1)
            }
            1 => {
                self.pop_ext_args(0);
                for (idx, message) in self.message_state.slots.iter_mut().enumerate() {
                    if message.active {
                        message.active = false;
                        return ExtCallOutcome::Value(idx as i32);
                    }
                }
                ExtCallOutcome::Value(-1)
            }
            2 => {
                let args = self.pop_ext_args(1);
                let slot = args.first().copied().unwrap_or(-1);
                if (0..8).contains(&slot) {
                    ExtCallOutcome::Value(self.message_state.slots[slot as usize].param)
                } else {
                    ExtCallOutcome::Value(0)
                }
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_sprite_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
    ) -> ExtCallOutcome {
        match index {
            2 => self.ext_sp_set(4, assets, nls, resource_manager, sprites, task_system),
            3 => self.ext_sp_set(5, assets, nls, resource_manager, sprites, task_system),
            4 => self.ext_sp_set_pos_ex(sprites),
            5 | 11 | 13 => self.ext_sp_cls(sprites, task_system),
            6 => self.ext_sp_set_alpha(sprites),
            7 => self.ext_sp_set_priority(sprites),
            9 => self.ext_sp_set_center(sprites),
            12 => {
                self.pop_ext_args(2);
                ExtCallOutcome::Value(1)
            }
            14 => self.ext_sp_set_pos_ex(sprites),
            15 => self.ext_sp_set_rect_pos(sprites),
            16 => {
                self.pop_ext_args(2);
                ExtCallOutcome::Value(1)
            }
            17 => self.ext_sp_set_scale(sprites),
            18 => self.ext_sp_set_rotate(sprites),
            19 => {
                self.pop_ext_args(5);
                ExtCallOutcome::Value(1)
            }
            20 => {
                self.pop_ext_args(5);
                ExtCallOutcome::Value(1)
            }
            21 => {
                self.pop_ext_args(3);
                ExtCallOutcome::Value(0)
            }
            22 => self.ext_sp_text(assets, nls, sprites),
            23 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            24 => self.ext_sp_set_rect(sprites),
            25 => self.ext_sp_set_pos_move(sprites),
            37 => self.ext_sp_set_color(sprites),
            41 => self.ext_sp_set_anim(assets, nls, sprites, task_system),
            88 => self.ext_movie_play(assets, nls, resource_manager),
            89 => self.ext_msp_set_loop_sp_ep(assets, nls, resource_manager, sprites, task_system),
            90 => self.ext_msp_cls(),
            91 => self.ext_msp_wait(),
            92 => self.ext_msp_lock(),
            93 => self.ext_msp_unlock(),
            94 => self.ext_msp_play(),
            95 => self.ext_msp_stop(),
            _ => ExtCallOutcome::Skip,
        }
    }

    fn ext_btn_set(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(7);
        if args.len() < 7 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let name_value = args[2];
        let entry_flag = args[6];
        let Some(name) = self.resolve_resource_string(name_value, assets, nls) else {
            return ExtCallOutcome::Value(0);
        };
        if name.is_empty() {
            return ExtCallOutcome::Value(1);
        }
        let (Some(resource_manager), Some(sprites)) = (resource_manager, sprites) else {
            return ExtCallOutcome::Value(0);
        };
        if let Some(old) = self.game_buttons.remove(&(group, index)) {
            sprites.release(old);
        }
        let asset = match open_resource_variant(resource_manager, &name, IMAGE_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                log::warn!(
                    "[trace-button] btn_set group={group} index={index} name={name:?} open failed: {err}"
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let decoded = match decode_image(&asset.bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                log::warn!(
                    "[trace-button] btn_set group={group} index={index} asset={:?} decode failed: {err}",
                    asset.name
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let surface_id = sprites.allocate_surface_id();
        let surface = match SpriteSurface::rgba8(
            surface_id,
            1,
            decoded.width,
            decoded.height,
            decoded.rgba,
        ) {
            Ok(surface) => surface,
            Err(err) => {
                log::warn!(
                    "[trace-button] btn_set group={group} index={index} surface failed: {err}"
                );
                return ExtCallOutcome::Value(0);
            }
        };
        sprites.insert_surface(surface);
        let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), decoded.width, decoded.height);
        desc.cell_width = decoded.cell_width;
        desc.cell_height = decoded.cell_height;
        desc.position = PalVec3::new(decoded.offset_x, decoded.offset_y, 0);
        desc.visible = entry_flag != 0;
        desc.source_name = asset.name.clone();
        let handle = sprites.create(desc);
        self.game_buttons.insert((group, index), handle);
        log::debug!(
            "[trace-button] btn_set group={group} index={index} name={name:?} asset={:?} size={}x{} cell={}x{} pos=({}, {}) entry_flag={entry_flag}",
            asset.name,
            decoded.width,
            decoded.height,
            decoded.cell_width,
            decoded.cell_height,
            decoded.offset_x,
            decoded.offset_y
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_uninit(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        self.pop_ext_args(0);
        let Some(sprites) = sprites else {
            self.game_buttons.clear();
            return ExtCallOutcome::Value(1);
        };
        let handles = self.game_buttons.values().copied().collect::<Vec<_>>();
        self.game_buttons.clear();
        for handle in handles {
            sprites.release(handle);
        }
        log::debug!("[trace-button] btn_uninit");
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_release(
        &mut self,
        opcode_index: u16,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let group = args.first().copied().unwrap_or(-1);
        let index = args.get(1).copied().unwrap_or(-1);
        let Some(sprites) = sprites else {
            self.forget_button_handles(group, index);
            return ExtCallOutcome::Value(1);
        };
        let keys = self
            .game_buttons
            .keys()
            .copied()
            .filter(|(button_group, button_index)| {
                (group < 0 || *button_group == group) && (index < 0 || *button_index == index)
            })
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(handle) = self.game_buttons.remove(&key) {
                sprites.release(handle);
            }
        }
        log::debug!("[trace-button] btn_release opcode={opcode_index} group={group} index={index}");
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_set_pos(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let x = args[2];
        let y = args[3];
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                sprites.set_pos(handle, x, y, 0);
            }
        }
        log::debug!("[trace-button] btn_set_pos group={group} index={index} pos=({x}, {y})");
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_ctrl(
        &mut self,
        opcode_index: u16,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let value = args[2];
        if let Some(sprites) = sprites {
            match opcode_index {
                21 => {
                    for handle in self.matching_button_handles(group, index) {
                        if let Some(sprite) = sprites.get_mut(handle) {
                            let raw = sprite.color.0 & 0x00FF_FFFF;
                            sprite.color =
                                PalColor::from_argb(raw | ((value.clamp(0, 255) as u32) << 24));
                        }
                    }
                }
                _ => {
                    for handle in self.matching_button_handles(group, index) {
                        sprites.view_ctrl(handle, value != 0);
                    }
                }
            }
        }
        log::debug!(
            "[trace-button] btn_ctrl opcode={opcode_index} group={group} index={index} value={value}"
        );
        ExtCallOutcome::Value(1)
    }

    fn dispatch_bgm_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        match index {
            0 => self.ext_bgm_play(assets, nls, resource_manager, audio),
            1 => self.ext_audio_stop(4, PalSoundGroup::GROUP3, audio),
            2 => self.ext_bgm_set_volume(audio),
            3 => ExtCallOutcome::Value(100),
            8 => self.ext_bgm_filename(assets, nls),
            9 => self.ext_bgm_load(assets, nls, resource_manager, audio),
            10 => self.ext_bgm_play_loaded(audio),
            16 => ExtCallOutcome::Value(0),
            17 => ExtCallOutcome::Value(0),
            18 => ExtCallOutcome::Value(0),
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_se_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        match index {
            0 | 1 => self.ext_se_play(assets, nls, resource_manager, audio),
            3 => self.ext_audio_stop(5, PalSoundGroup::GROUP4, audio),
            _ => ExtCallOutcome::Skip,
        }
    }

    fn dispatch_voice_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        match index {
            0 => self.ext_voice_play(assets, nls, resource_manager, audio),
            1 => self.ext_audio_stop(13, PalSoundGroup::GROUP5, audio),
            16 => {
                self.pop_ext_args(4);
                ExtCallOutcome::Value(1)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    fn ext_sp_set(
        &mut self,
        arg_count: usize,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(arg_count);
        if args.len() < arg_count {
            return ExtCallOutcome::Block;
        }
        let name_value = args[0];
        let slot = args[1];
        let raw_x = args.get(2).copied().unwrap_or(0);
        let raw_y = args.get(3).copied().unwrap_or(0);
        let raw_z = args.get(4).copied().unwrap_or(0);
        if name_value == 0x0FFF_FFFF {
            return ExtCallOutcome::Value(1);
        }
        let Some(name) = self.resolve_resource_string(name_value, assets, nls) else {
            return ExtCallOutcome::Value(0);
        };
        if name.is_empty() {
            return ExtCallOutcome::Value(1);
        }
        let (Some(resource_manager), Some(sprites)) = (resource_manager, sprites) else {
            return ExtCallOutcome::Value(0);
        };
        if let (Some(old_anim), Some(task_system)) =
            (self.game_sprite_animations.remove(&slot), task_system)
        {
            task_system.animation_release(old_anim);
        }
        if let Some(old_state) = self.game_msprites.remove(&slot) {
            if let Some(handle) = old_state.handle {
                self.msprite_system.release(handle);
            }
        }
        if let Some(old) = self.game_sprites.remove(&slot) {
            sprites.release(old);
        }
        if let Some((r, g, b)) = parse_solid_color_name(&name) {
            let (logical_width, logical_height) = self.logical_size();
            let width = logical_width;
            let height = logical_height;
            let (x, y, z) = place_sprite(
                arg_count,
                raw_x,
                raw_y,
                raw_z,
                width,
                height,
                0,
                0,
                logical_width,
                logical_height,
            );
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            for px in pixels.chunks_exact_mut(4) {
                px.copy_from_slice(&[r, g, b, 255]);
            }
            let surface_id = sprites.allocate_surface_id();
            let surface = match SpriteSurface::rgba8(surface_id, 1, width, height, pixels) {
                Ok(surface) => surface,
                Err(err) => {
                    log::warn!("[trace-sprite] sp_set slot={slot} solid surface failed: {err}");
                    return ExtCallOutcome::Value(0);
                }
            };
            sprites.insert_surface(surface);
            let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), width, height);
            desc.position = PalVec3::new(x, y, z);
            desc.base_priority = z;
            desc.visible = true;
            desc.source_name = name.clone();
            let handle = sprites.create(desc);
            self.game_sprites.insert(slot, handle);
            return ExtCallOutcome::Value(1);
        }
        let asset = match open_resource_variant(resource_manager, &name, IMAGE_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                log::warn!("[trace-sprite] sp_set slot={slot} name={name:?} open failed: {err}");
                return ExtCallOutcome::Value(0);
            }
        };
        let decoded = match decode_image(&asset.bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                log::warn!(
                    "[trace-sprite] sp_set slot={slot} asset={:?} decode failed: {err}",
                    asset.name
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let surface_id = sprites.allocate_surface_id();
        let (logical_width, logical_height) = self.logical_size();
        let (x, y, z) = place_sprite(
            arg_count,
            raw_x,
            raw_y,
            raw_z,
            decoded.width,
            decoded.height,
            decoded.offset_x,
            decoded.offset_y,
            logical_width,
            logical_height,
        );
        let surface = match SpriteSurface::rgba8(
            surface_id,
            1,
            decoded.width,
            decoded.height,
            decoded.rgba,
        ) {
            Ok(surface) => surface,
            Err(err) => {
                log::warn!("[trace-sprite] sp_set slot={slot} surface failed: {err}");
                return ExtCallOutcome::Value(0);
            }
        };
        sprites.insert_surface(surface);
        let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), decoded.width, decoded.height);
        desc.cell_width = decoded.cell_width;
        desc.cell_height = decoded.cell_height;
        desc.position = PalVec3::new(x, y, z);
        desc.base_priority = z;
        desc.visible = true;
        desc.source_name = asset.name.clone();
        let handle = sprites.create(desc);
        self.game_sprites.insert(slot, handle);
        log::debug!(
            "[trace-sprite] sp_set slot={slot} name={name:?} asset={:?} size={}x{} raw=({}, {}, {}) pos=({}, {}, {}) priority={}",
            asset.name,
            decoded.width,
            decoded.height,
            raw_x,
            raw_y,
            raw_z,
            x,
            y,
            z,
            z
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_text(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let text_value = args[1];
        let x = args[2];
        let y = args[3];
        let z = args[4];
        if !(-1..=128).contains(&slot) {
            log::warn!("[trace-sprite] sptext slot out of range: {slot}");
            return ExtCallOutcome::Value(0);
        }
        let Some(text) = self
            .resolve_script_string(text_value, assets, nls)
            .or_else(|| self.resolve_resource_string(text_value, assets, nls))
        else {
            return ExtCallOutcome::Value(0);
        };
        let Some(sprites) = sprites else {
            return ExtCallOutcome::Value(0);
        };
        let (text, temporary_size) = parse_pal_text_directives(&text);
        if text.is_empty() {
            return ExtCallOutcome::Value(0);
        }
        let saved_size = self.font_state.font_size();
        if let Some(size) = temporary_size {
            self.font_state.set_font_size(size);
        }
        let (width, height, rgba) = self.font_state.rasterize(text);
        if temporary_size.is_some() {
            self.font_state.set_font_size(saved_size);
        }
        if let Some(old_state) = self.game_msprites.remove(&slot) {
            if let Some(handle) = old_state.handle {
                self.msprite_system.release(handle);
            }
        }
        if let Some(handle) = self.game_sprites.get(&slot).copied() {
            if !sprites.replace_sprite_surface(handle, width, height, rgba, format!("text:{text}"))
            {
                return ExtCallOutcome::Value(0);
            }
            sprites.set_pos(handle, x, y, z);
            sprites.set_priority(handle, z);
        } else {
            let Some(handle) = sprites.create_rgba_sprite(
                width,
                height,
                rgba,
                PalVec3::new(x, y, z),
                z,
                format!("text:{text}"),
            ) else {
                return ExtCallOutcome::Value(0);
            };
            self.game_sprites.insert(slot, handle);
        }
        self.text_state.last_text_value = text_value;
        self.text_state.last_event_time_ms = self.pal_time_ms;
        log::debug!(
            "[trace-sprite] sptext slot={slot} text_value={text_value} size={}x{} pos=({}, {}, {})",
            width,
            height,
            x,
            y,
            z
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_anim(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let name_value = args[1];
        let entry_flag = args[2];
        let name = self
            .resolve_resource_string(name_value, assets, nls)
            .unwrap_or_else(|| format!("0x{name_value:08X}"));

        let Some(handle) = self.game_sprites.get(&slot).copied() else {
            log::debug!("[trace-sprite] sp_set_anim slot={slot} name={name:?} missing sprite");
            return ExtCallOutcome::Value(1);
        };
        let (Some(sprites), Some(task_system)) = (sprites, task_system) else {
            return ExtCallOutcome::Value(1);
        };

        if let Some(old_anim) = self.game_sprite_animations.remove(&slot) {
            task_system.animation_release(old_anim);
        }

        let flags = match sprites.get(handle) {
            Some(sprite) if sprite.frame_count(crate::sprite::PalAnimationAxis::Horizontal) > 1 => {
                PalAnimationFlags::HORIZONTAL
            }
            Some(sprite) if sprite.frame_count(crate::sprite::PalAnimationAxis::Vertical) > 1 => {
                PalAnimationFlags::VERTICAL
            }
            Some(_) => {
                log::debug!(
                    "[trace-sprite] sp_set_anim slot={slot} name={name:?} single-frame sprite"
                );
                return ExtCallOutcome::Value(1);
            }
            None => return ExtCallOutcome::Value(1),
        };

        let desc = PalSheetAnimationDesc {
            sprite: handle,
            flags,
            frame_delay_ms: 100,
            running: true,
        };
        match task_system.create_animation_sheet(sprites, desc, None) {
            Ok(anim_handle) => {
                self.game_sprite_animations.insert(slot, anim_handle);
                log::debug!(
                    "[trace-sprite] sp_set_anim slot={slot} name={name:?} entry_flag={entry_flag} flags=0x{:02X} handle=0x{:08X}",
                    flags.raw(),
                    anim_handle.0
                );
                ExtCallOutcome::Value(1)
            }
            Err(err) => {
                log::warn!("[trace-sprite] sp_set_anim slot={slot} name={name:?} failed: {err}");
                ExtCallOutcome::Value(0)
            }
        }
    }

    fn ext_movie_play(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        if args.len() < 2 {
            return ExtCallOutcome::Block;
        }
        let name_value = args[0];
        let layer = args[1];
        let Some(name) = self.resolve_resource_string(name_value, assets, nls) else {
            return ExtCallOutcome::Value(0);
        };
        let Some(resource_manager) = resource_manager else {
            return ExtCallOutcome::Value(0);
        };
        match open_resource_variant(resource_manager, &name, MOVIE_EXTENSIONS) {
            Ok(asset) => {
                self.msprite_system.start_movie(asset.name.clone(), layer);
                log::debug!(
                    "[trace-msprite] movie_play layer={layer} asset={:?}",
                    asset.name
                );
                ExtCallOutcome::Value(1)
            }
            Err(err) => {
                log::warn!("[trace-msprite] movie_play name={name:?} open failed: {err}");
                ExtCallOutcome::Value(0)
            }
        }
    }

    fn ext_msp_set_loop_sp_ep(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(8);
        if args.len() < 8 {
            return ExtCallOutcome::Block;
        }

        let name_value = args[0];
        let slot = args[1];
        let raw_x = args[2];
        let raw_y = args[3];
        let raw_z = args[4];
        let loop_mode = args[5];
        let loop_start = args[6];
        let loop_end = args[7];
        let Some(name) = self.resolve_resource_string(name_value, assets, nls) else {
            return ExtCallOutcome::Value(0);
        };
        let (Some(resource_manager), Some(sprites)) = (resource_manager, sprites) else {
            return ExtCallOutcome::Value(0);
        };

        if let Some(old_anim) = self.game_sprite_animations.remove(&slot) {
            if let Some(task_system) = task_system {
                task_system.animation_release(old_anim);
            }
        }
        if let Some(old_state) = self.game_msprites.remove(&slot) {
            if let Some(handle) = old_state.handle {
                self.msprite_system.release(handle);
            }
        }
        if let Some(old) = self.game_sprites.remove(&slot) {
            sprites.release(old);
        }

        let asset = match open_resource_variant(resource_manager, &name, MOVIE_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                log::warn!(
                    "[trace-msprite] msp_set_loop_sp_ep slot={slot} name={name:?} open failed: {err}"
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let loaded = match self
            .msprite_system
            .load_wmv(asset.name.clone(), asset.bytes)
        {
            Ok(loaded) => loaded,
            Err(err) => {
                log::warn!(
                    "[trace-msprite] msp_set_loop_sp_ep slot={slot} asset={:?} decode failed: {err}",
                    asset.name
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let (logical_width, logical_height) = self.logical_size();
        let (x, y, z) = place_sprite(
            5,
            raw_x,
            raw_y,
            raw_z,
            loaded.width,
            loaded.height,
            0,
            0,
            logical_width,
            logical_height,
        );
        let Some(sprite) = sprites.create_msprite(
            loaded.handle,
            loaded.width,
            loaded.height,
            loaded.rgba,
            PalVec3::new(x, y, z),
            z,
            loaded.name.clone(),
        ) else {
            self.msprite_system.release(loaded.handle);
            return ExtCallOutcome::Value(0);
        };
        self.msprite_system
            .set_loop_point(loaded.handle, loop_start, loop_end);
        self.msprite_system.play(loaded.handle, loop_mode);
        self.game_sprites.insert(slot, sprite);
        self.game_msprites.insert(
            slot,
            GameMSpriteState {
                handle: Some(loaded.handle),
                playing: true,
                locked: false,
                loop_mode,
                loop_start,
                loop_end,
                last_play: loop_mode,
                finished: false,
            },
        );
        log::debug!(
            "[trace-msprite] msp_set_loop_sp_ep slot={slot} asset={:?} loop_mode={loop_mode} loop=({loop_start}, {loop_end}) pos=({}, {}, {})",
            loaded.name,
            x,
            y,
            z
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_cls(&mut self) -> ExtCallOutcome {
        self.pop_ext_args(0);
        self.game_msprites.clear();
        self.msprite_system.clear();
        log::debug!("[trace-msprite] msp_cls");
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_wait(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        if args.is_empty() {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let Some(state) = self.game_msprites.get_mut(&slot) else {
            log::debug!("[trace-msprite] msp_wait slot={slot} missing");
            return ExtCallOutcome::Value(1);
        };
        if let Some(handle) = state.handle {
            let native_finished = (self.msprite_system.state(handle) & MSPRITE_STATE_FINISHED) != 0;
            if native_finished {
                state.playing = false;
                state.finished = true;
                return ExtCallOutcome::Value(1);
            }
            self.msprite_system.set_loop(handle, 0);
        }
        if state.playing && !state.finished {
            state.playing = false;
            state.finished = true;
            log::debug!("[trace-msprite] msp_wait slot={slot} wait_frame=1");
            return ExtCallOutcome::Wait {
                value: 1,
                request: WaitRequest::Frame(1),
            };
        }
        log::debug!("[trace-msprite] msp_wait slot={slot} already_finished");
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_lock(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        if args.is_empty() {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        if let Some(state) = self.game_msprites.get_mut(&slot) {
            state.locked = true;
            if let Some(handle) = state.handle {
                self.msprite_system.lock(handle);
            }
        }
        log::debug!("[trace-msprite] msp_lock slot={slot}");
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_unlock(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        if args.is_empty() {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        if let Some(state) = self.game_msprites.get_mut(&slot) {
            state.locked = false;
            if let Some(handle) = state.handle {
                self.msprite_system.unlock(handle);
            }
        }
        log::debug!("[trace-msprite] msp_unlock slot={slot}");
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_play(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        if args.len() < 2 {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let play = args[1];
        if let Some(state) = self.game_msprites.get_mut(&slot) {
            state.playing = true;
            state.finished = false;
            state.last_play = play;
            if let Some(handle) = state.handle {
                self.msprite_system.play(handle, play);
            }
        }
        log::debug!("[trace-msprite] msp_play slot={slot} play={play}");
        ExtCallOutcome::Value(1)
    }

    fn ext_msp_stop(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        if args.is_empty() {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        if let Some(state) = self.game_msprites.get_mut(&slot) {
            state.playing = false;
            state.finished = true;
            if let Some(handle) = state.handle {
                self.msprite_system.stop(handle);
            }
        }
        log::debug!("[trace-msprite] msp_stop slot={slot}");
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_cls(
        &mut self,
        sprites: Option<&mut SpriteSystem>,
        task_system: Option<&mut TaskSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let slot = args.first().copied().unwrap_or(-1);
        if let Some(task_system) = task_system {
            if slot == -1 {
                let handles = self
                    .game_sprite_animations
                    .values()
                    .copied()
                    .collect::<Vec<_>>();
                self.game_sprite_animations.clear();
                for handle in handles {
                    task_system.animation_release(handle);
                }
            } else if let Some(handle) = self.game_sprite_animations.remove(&slot) {
                task_system.animation_release(handle);
            }
        } else if slot == -1 {
            self.game_sprite_animations.clear();
        } else {
            self.game_sprite_animations.remove(&slot);
        }
        if slot == -1 {
            self.game_msprites.clear();
            self.msprite_system.clear();
        } else {
            if let Some(old_state) = self.game_msprites.remove(&slot) {
                if let Some(handle) = old_state.handle {
                    self.msprite_system.release(handle);
                }
            }
        }
        let Some(sprites) = sprites else {
            return ExtCallOutcome::Value(1);
        };
        if slot == -1 {
            let handles = self.game_sprites.values().copied().collect::<Vec<_>>();
            self.game_sprites.clear();
            for handle in handles {
                sprites.release(handle);
            }
        } else if let Some(handle) = self.game_sprites.remove(&slot) {
            sprites.release(handle);
        }
        log::debug!("[trace-sprite] sp_cls slot={slot}");
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_alpha(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let (Some(slot), Some(alpha)) = (args.first().copied(), args.get(1).copied()) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            if let Some(sprite) = sprites.get_mut(handle) {
                let raw = sprite.color.0 & 0x00FF_FFFF;
                sprite.color = PalColor::from_argb(raw | ((alpha.clamp(0, 255) as u32) << 24));
            }
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_color(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let (Some(slot), Some(rgb)) = (args.first().copied(), args.get(1).copied()) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            if let Some(sprite) = sprites.get_mut(handle) {
                let raw = rgb as u32;
                let alpha = sprite.color.0 & 0xFF00_0000;
                sprite.color = PalColor::from_argb(alpha | (raw & 0x00FF_FFFF));
            }
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_priority(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let (Some(slot), Some(priority)) = (args.first().copied(), args.get(1).copied()) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_priority(handle, priority);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_center(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        let (Some(slot), Some(x), Some(y)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_center_offset(handle, x, y);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_pos_ex(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        let (Some(slot), Some(x), Some(y), Some(z)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
            args.get(3).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_pos(handle, x, y, z);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_pos_move(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        let (Some(slot), Some(x), Some(y), Some(z)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
            args.get(3).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.move_pos(handle, (x as f32) * 1.5, (y as f32) * 1.5, z as f32);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_rect_pos(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        let (Some(slot), Some(cell_x), Some(cell_y)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.rect_set_pos(handle, cell_x.max(0) as u16, cell_y.max(0) as u16);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_rect(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        let (Some(slot), Some(left), Some(top), Some(right), Some(bottom)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
            args.get(3).copied(),
            args.get(4).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_rect(handle, Some(PalRect::new(left, top, right, bottom)));
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_scale(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let (Some(slot), Some(scale)) = (args.first().copied(), args.get(1).copied()) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_scale(handle, scale as f32 / 100.0);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_rotate(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        let (Some(slot), Some(x), Some(y), Some(z)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
            args.get(3).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.set_rotate_ex(handle, x as f32, y as f32, z as f32);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_bgm_play(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(7);
        if args.len() < 7 {
            return ExtCallOutcome::Block;
        }
        let slot = if args[0] == -1 { 0 } else { args[0] };
        let name_value = args[2];
        let flags = args[3];
        let volume = if args[4] == 0 { 100 } else { args[4] };
        self.audio_load_and_play(
            4,
            slot,
            PalSoundGroup::GROUP3,
            name_value,
            flags,
            volume,
            true,
            assets,
            nls,
            resource_manager,
            audio,
        )
    }

    fn ext_bgm_load(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let slot = if args[0] == -1 { 0 } else { args[0] };
        self.audio_load_and_play(
            4,
            slot,
            PalSoundGroup::GROUP3,
            args[1],
            0x2000_0000u32 as i32,
            100,
            false,
            assets,
            nls,
            resource_manager,
            audio,
        )
    }

    fn ext_bgm_play_loaded(&mut self, audio: Option<&mut AudioSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let flags = args[1];
        if let (Some(handle), Some(audio)) = (self.game_audio.get(&(4, slot)).copied(), audio) {
            let looping = (flags & 1) != 0;
            let _ = audio.play(handle, looping);
        }
        ExtCallOutcome::Value(slot)
    }

    fn ext_bgm_set_volume(&mut self, audio: Option<&mut AudioSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let volume = args.first().copied().unwrap_or(100).saturating_mul(100);
        if let Some(audio) = audio {
            let _ = audio.set_group_volume(PalSoundGroup::GROUP3, PalVolume::from_raw(volume));
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_bgm_filename(&mut self, assets: &CoreAssets, nls: Nls) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let value = args.first().copied().unwrap_or(0);
        let handle = self
            .resolve_resource_string(value, assets, nls)
            .map(|s| self.store_dynamic_string(s))
            .unwrap_or(0);
        ExtCallOutcome::Value(handle)
    }

    fn ext_se_play(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let slot = if args[0] == -1 {
            self.next_free_audio_slot(5, 16)
        } else {
            args[0]
        };
        self.audio_load_and_play(
            5,
            slot,
            PalSoundGroup::GROUP4,
            args[1],
            args[3],
            args[4],
            true,
            assets,
            nls,
            resource_manager,
            audio,
        )
    }

    fn ext_voice_play(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let slot = if args[0] == -1 {
            self.next_free_audio_slot(13, 8)
        } else {
            args[0]
        };
        self.audio_load_and_play(
            13,
            slot,
            PalSoundGroup::GROUP5,
            args[1],
            args[2],
            args[3],
            true,
            assets,
            nls,
            resource_manager,
            audio,
        )
    }

    fn ext_audio_stop(
        &mut self,
        category: u16,
        group: PalSoundGroup,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let slot = args.first().copied().unwrap_or(-1);
        let Some(audio) = audio else {
            return ExtCallOutcome::Value(1);
        };
        if slot == -1 {
            let keys = self
                .game_audio
                .keys()
                .filter(|(cat, _)| *cat == category)
                .copied()
                .collect::<Vec<_>>();
            for key in keys {
                if let Some(handle) = self.game_audio.remove(&key) {
                    let _ = audio.release(handle);
                }
            }
            let _ = audio.release_group(group);
        } else if let Some(handle) = self.game_audio.remove(&(category, slot)) {
            let _ = audio.release(handle);
        }
        ExtCallOutcome::Value(1)
    }

    #[allow(clippy::too_many_arguments)]
    fn audio_load_and_play(
        &mut self,
        category: u16,
        slot: i32,
        group: PalSoundGroup,
        name_value: i32,
        flags: i32,
        volume_percent: i32,
        play: bool,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        let Some(name) = self.resolve_resource_string(name_value, assets, nls) else {
            return ExtCallOutcome::Value(0);
        };
        if name.is_empty() {
            return ExtCallOutcome::Value(slot);
        }
        let (Some(resource_manager), Some(audio)) = (resource_manager, audio) else {
            return ExtCallOutcome::Value(0);
        };
        if let Some(old) = self.game_audio.remove(&(category, slot)) {
            let _ = audio.release(old);
        }
        let asset = match open_resource_variant(resource_manager, &name, AUDIO_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                log::warn!("[trace-audio] open category={category} slot={slot} name={name:?} failed: {err}");
                return ExtCallOutcome::Value(0);
            }
        };
        let handle = match audio.load_static_asset(asset.clone(), group) {
            Ok(handle) => handle,
            Err(err) => {
                log::warn!(
                    "[trace-audio] load category={category} slot={slot} asset={:?} failed: {err}",
                    asset.name
                );
                return ExtCallOutcome::Value(0);
            }
        };
        let volume = PalVolume::from_raw(volume_percent.clamp(0, 100).saturating_mul(100));
        let _ = audio.set_channel_volume(handle, volume);
        if play {
            let looping = (flags & 1) != 0;
            if let Err(err) = audio.play(handle, looping) {
                log::warn!(
                    "[trace-audio] play category={category} slot={slot} asset={:?} failed: {err}",
                    asset.name
                );
            }
        }
        self.game_audio.insert((category, slot), handle);
        log::debug!(
            "[trace-audio] category={category} slot={slot} asset={:?} group={:?} play={play} flags=0x{flags:08X}",
            asset.name,
            group
        );
        ExtCallOutcome::Value(slot)
    }

    fn next_free_audio_slot(&self, category: u16, limit: i32) -> i32 {
        (0..limit)
            .find(|slot| !self.game_audio.contains_key(&(category, *slot)))
            .unwrap_or(0)
    }

    fn ext_arg_probe(&mut self, arity: usize, value: i32) -> ExtCallOutcome {
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(value)
    }

    fn ext_arg_get(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let index = args.first().copied().unwrap_or(0).max(0) as usize;
        let value = self.extcall_arg(index + 1);
        log::debug!("[trace-script] ext_0012_0006.arg_get index={index} -> {value}");
        ExtCallOutcome::Value(value)
    }

    fn ext_string_non_empty(&mut self, assets: &CoreAssets, nls: Nls) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(value) = args.first().copied() else {
            return ExtCallOutcome::Value(0);
        };
        let ok = self
            .resolve_script_string(value, assets, nls)
            .is_some_and(|s| !s.is_empty());
        log::debug!("[trace-script] ext_0012_0015.string_non_empty value={value} -> {ok}");
        ExtCallOutcome::Value(if ok { 1 } else { 0 })
    }

    fn ext_get_private_profile_int(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() != 4 {
            return ExtCallOutcome::Block;
        }
        let section = self
            .resolve_script_string(args[0], assets, nls)
            .unwrap_or_default();
        let key = self
            .resolve_script_string(args[1], assets, nls)
            .unwrap_or_default();
        let default = args[2];
        let requested_filename = self
            .resolve_script_string(args[3], assets, nls)
            .unwrap_or_else(|| "SYSTEM.INI".to_owned());
        let filename = ini_filename_or_system(&requested_filename);

        let value = match self.system_ini(resource_manager) {
            Some(ini) => ini
                .get(&section.to_ascii_lowercase())
                .and_then(|section| section.get(&key.to_ascii_lowercase()))
                .and_then(|value| value.as_int())
                .map(|value| value as i32)
                .unwrap_or(default),
            None => default,
        };

        log::debug!(
            "[trace-script] ext_0012_0025.getprivateprofileint section={section:?} key={key:?} default={default} requested_filename={requested_filename:?} used_filename={filename:?} -> {value}"
        );
        ExtCallOutcome::Value(value)
    }

    fn ext_sz_buf(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() != 5 {
            return ExtCallOutcome::Block;
        }
        let section = self
            .resolve_script_string(args[0], assets, nls)
            .unwrap_or_default();
        let key = self
            .resolve_script_string(args[1], assets, nls)
            .unwrap_or_default();
        let default = self
            .resolve_script_string(args[2], assets, nls)
            .unwrap_or_default();
        let requested_filename = self
            .resolve_script_string(args[4], assets, nls)
            .unwrap_or_else(|| "SYSTEM.INI".to_owned());
        let filename = ini_filename_or_system(&requested_filename);
        let value = match self.system_ini(resource_manager) {
            Some(ini) => ini
                .get(&section.to_ascii_lowercase())
                .and_then(|section| section.get(&key.to_ascii_lowercase()))
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or(default),
            None => default,
        };
        let handle = self.store_dynamic_string(value.clone());
        self.write_temp_mem_relative(128, handle);
        log::debug!(
            "[trace-script] ext_0012_0024.sz_buf section={section:?} key={key:?} requested_filename={requested_filename:?} used_filename={filename:?} -> {value:?}"
        );
        ExtCallOutcome::Value(handle)
    }

    fn ext_open_file(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(name) = args
            .first()
            .and_then(|arg| self.resolve_script_string(*arg, assets, nls))
        else {
            return ExtCallOutcome::Value(0);
        };
        let Some(resource_manager) = resource_manager else {
            return ExtCallOutcome::Value(0);
        };
        match resource_manager.open(&name) {
            Ok(asset) => {
                let handle = self.insert_file_handle(RuntimeFile {
                    name: name.clone(),
                    parsed_table: parse_file_table(&asset.bytes, resource_manager.nls()).ok(),
                    bytes: asset.bytes,
                    cursor: 0,
                    table_cursor: 0,
                });
                log::debug!(
                    "[trace-assets] openfile name={name:?} source={} handle={handle}",
                    asset_source_label(&asset.source)
                );
                ExtCallOutcome::Value(handle)
            }
            Err(err) => {
                log::warn!("[trace-assets] openfile name={name:?} failed: {err}");
                ExtCallOutcome::Value(0)
            }
        }
    }

    fn ext_read_file(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() != 3 {
            return ExtCallOutcome::Block;
        }
        let handle = args[0];
        let temp_offset = args[1];
        let count = args[2].max(0) as usize;
        let (name, entries, read_len) = {
            let Some(file) = self.file_handle_mut(handle) else {
                return ExtCallOutcome::Value(0);
            };
            if let Some(table) = file.parsed_table.as_ref() {
                let entry_start = file.table_cursor / 4;
                let available = table.entries.len().saturating_sub(entry_start);
                let read_len = available.min(count);
                let entries = table.entries[entry_start..entry_start + read_len].to_vec();
                file.table_cursor += read_len * 4;
                (file.name.clone(), entries, read_len)
            } else {
                let available = file.bytes.len().saturating_sub(file.cursor);
                let read_len = available.min(count);
                let start = file.cursor;
                let end = start + read_len;
                let entries = file.bytes[start..end]
                    .iter()
                    .map(|byte| i32::from(*byte))
                    .collect::<Vec<_>>();
                file.cursor = end;
                (file.name.clone(), entries, read_len)
            }
        };
        for (i, value) in entries.iter().enumerate() {
            self.write_temp_mem_relative(temp_offset + i as i32, *value);
        }
        log::debug!(
            "[trace-assets] read_file handle={handle} name={:?} count={count} -> {read_len}",
            name
        );
        ExtCallOutcome::Value(if read_len == count { 1 } else { 0 })
    }

    fn ext_set_file_pointer(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() != 3 {
            return ExtCallOutcome::Block;
        }
        let handle = args[0];
        let offset = args[1];
        let origin = args[2];
        let Some(file) = self.file_handle_mut(handle) else {
            return ExtCallOutcome::Value(0);
        };
        let base = match origin {
            0 => 0i64,
            1 => file.table_cursor as i64,
            2 => file
                .parsed_table
                .as_ref()
                .map_or(file.bytes.len(), |table| table.entries.len() * 4) as i64,
            _ => file.table_cursor as i64,
        };
        let next = base.saturating_add((offset as i64) * 4).max(0) as usize;
        let table_len = file
            .parsed_table
            .as_ref()
            .map_or(file.bytes.len(), |table| table.entries.len() * 4);
        file.table_cursor = next.min(table_len);
        file.cursor = file.table_cursor.min(file.bytes.len());
        log::debug!(
            "[trace-assets] set_file_pointer handle={handle} name={:?} offset={offset} origin={origin} table_cursor={}",
            file.name,
            file.table_cursor
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_file_string(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() != 3 {
            return ExtCallOutcome::Block;
        }
        let handle = args[0];
        let dst_slot = args[1];
        let entry = args[2];
        let Some(file) = self.file_handle_mut(handle) else {
            return ExtCallOutcome::Value(0);
        };
        let Some(table) = file.parsed_table.as_ref() else {
            return ExtCallOutcome::Value(0);
        };
        let offset = entry & 0x7FFF_FFFF;
        let value = table.strings.get(&offset).cloned().unwrap_or_default();
        let dyn_value = if is_dynamic_string_handle(dst_slot) {
            self.replace_dynamic_string(dst_slot, value.clone());
            dst_slot
        } else {
            self.store_dynamic_string(value.clone())
        };
        log::debug!(
            "[trace-script] ext_0012_0022.file_string handle={handle} entry=0x{entry:08X} offset={offset} -> {value:?}"
        );
        ExtCallOutcome::Value(dyn_value)
    }

    fn pop_ext_args(&mut self, count: usize) -> Vec<i32> {
        let available = count.min(self.stack.len());
        let mut args = Vec::with_capacity(available);
        for _ in 0..available {
            if let Some(value) = self.stack.pop() {
                args.push(value);
            }
        }
        args
    }

    fn resolve_script_string(&self, value: i32, assets: &CoreAssets, nls: Nls) -> Option<String> {
        if value == 0x0FFF_FFFF {
            return Some(String::new());
        }
        if let Some(index) = dynamic_string_index(value) {
            return self.dynamic_strings.get(index).cloned();
        }
        let offset = value.try_into().ok()?;
        read_text_record_string(&assets.text_dat.bytes, offset, nls)
            .or_else(|| read_c_string(&assets.text_dat.bytes, offset, nls))
            .or_else(|| read_c_string(&assets.file_dat.bytes, offset, nls))
    }

    fn resolve_resource_string(&self, value: i32, assets: &CoreAssets, nls: Nls) -> Option<String> {
        if value == 0x0FFF_FFFF {
            return Some(String::new());
        }
        if let Some(index) = dynamic_string_index(value) {
            return self.dynamic_strings.get(index).cloned();
        }
        let offset = value.try_into().ok()?;
        read_file_slot_string(&assets.file_dat.bytes, value, nls)
            .or_else(|| read_padded_c_string(&assets.file_dat.bytes, offset, nls))
            .or_else(|| read_text_record_string(&assets.file_dat.bytes, offset, nls))
            .or_else(|| read_text_record_string(&assets.text_dat.bytes, offset, nls))
            .or_else(|| read_c_string(&assets.text_dat.bytes, offset, nls))
    }

    fn system_ini(&mut self, resource_manager: Option<&mut ResourceManager>) -> Option<&IniFile> {
        if self.system_ini.is_none() {
            let Some(resource_manager) = resource_manager else {
                return None;
            };
            match resource_manager.open("SYSTEM.INI") {
                Ok(asset) => {
                    log::debug!(
                        "[trace-assets] ini open name=\"SYSTEM.INI\" source={}",
                        asset_source_label(&asset.source)
                    );
                    match parse_ini_nls(&asset.bytes, resource_manager.nls()) {
                        Ok(ini) => self.system_ini = Some(ini),
                        Err(err) => {
                            log::warn!("[trace-assets] SYSTEM.INI parse failed: {err}");
                            return None;
                        }
                    }
                }
                Err(err) => {
                    log::warn!("[trace-assets] SYSTEM.INI open failed: {err}");
                    return None;
                }
            }
        }
        self.system_ini.as_ref()
    }

    fn store_dynamic_string(&mut self, value: String) -> i32 {
        let index = self.dynamic_strings.len();
        self.dynamic_strings.push(value);
        0x1000_0000u32.wrapping_add(index as u32) as i32
    }

    fn replace_dynamic_string(&mut self, handle: i32, value: String) -> bool {
        let Some(index) = dynamic_string_index(handle) else {
            return false;
        };
        if index >= self.dynamic_strings.len() {
            self.dynamic_strings.resize(index + 1, String::new());
        }
        self.dynamic_strings[index] = value;
        true
    }

    fn write_temp_mem_relative(&mut self, offset: i32, value: i32) {
        let index = self.argument_base.wrapping_add(offset);
        if index < 0 {
            return;
        }
        if let Some(slot) = self.temp_mem.get_mut(index as usize) {
            *slot = value;
        }
    }

    fn insert_file_handle(&mut self, file: RuntimeFile) -> i32 {
        for (index, slot) in self.file_handles.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file);
                return (index + 1) as i32;
            }
        }
        self.file_handles.push(Some(file));
        self.file_handles.len() as i32
    }

    fn file_handle_mut(&mut self, handle: i32) -> Option<&mut RuntimeFile> {
        if handle <= 0 {
            return None;
        }
        self.file_handles
            .get_mut(handle as usize - 1)
            .and_then(Option::as_mut)
    }

    fn matching_button_handles(&self, group: i32, index: i32) -> Vec<SpriteHandle> {
        self.game_buttons
            .iter()
            .filter(|((button_group, button_index), _)| {
                (group < 0 || *button_group == group) && (index < 0 || *button_index == index)
            })
            .map(|(_, handle)| *handle)
            .collect()
    }

    fn forget_button_handles(&mut self, group: i32, index: i32) {
        let keys = self
            .game_buttons
            .keys()
            .copied()
            .filter(|(button_group, button_index)| {
                (group < 0 || *button_group == group) && (index < 0 || *button_index == index)
            })
            .collect::<Vec<_>>();
        for key in keys {
            self.game_buttons.remove(&key);
        }
    }

    fn logical_size(&self) -> (u32, u32) {
        let width = self
            .system_ini
            .as_ref()
            .and_then(|ini| ini.get("graphics"))
            .and_then(|section| section.get("def_cg_width"))
            .and_then(|value| value.as_int())
            .filter(|value| *value > 0)
            .map(|value| value as u32)
            .unwrap_or(FrameScene::PAL_DEFAULT_WIDTH);
        let height = self
            .system_ini
            .as_ref()
            .and_then(|ini| ini.get("graphics"))
            .and_then(|section| section.get("def_cg_height"))
            .and_then(|value| value.as_int())
            .filter(|value| *value > 0)
            .map(|value| value as u32)
            .unwrap_or(FrameScene::PAL_DEFAULT_HEIGHT);
        (width, height)
    }
}

fn dynamic_string_index(value: i32) -> Option<usize> {
    let raw = value as u32;
    if (raw & 0x1000_0000) == 0 {
        return None;
    }
    Some((raw & 0x0FFF_FFFF) as usize)
}

fn is_dynamic_string_handle(value: i32) -> bool {
    dynamic_string_index(value).is_some()
}

const IMAGE_EXTENSIONS: &[&str] = &["", ".PGD", ".pgd"];
const AUDIO_EXTENSIONS: &[&str] = &["", ".OGG", ".ogg", ".WAV", ".wav"];
const MOVIE_EXTENSIONS: &[&str] = &["", ".WMV", ".wmv", ".MPG", ".mpg", ".MP4", ".mp4"];

fn place_sprite(
    arg_count: usize,
    raw_x: i32,
    raw_y: i32,
    raw_z: i32,
    width: u32,
    height: u32,
    default_x: i32,
    default_y: i32,
    logical_width: u32,
    logical_height: u32,
) -> (i32, i32, i32) {
    if arg_count >= 5 {
        if raw_x == 0xFFFF && raw_y == 0xFFFF {
            return (default_x, default_y, 0);
        }
        return (scale_script_coord(raw_x), scale_script_coord(raw_y), raw_z);
    }
    (
        place_sprite_x(raw_x, width as i32, logical_width as i32),
        place_sprite_y(raw_y, height as i32, logical_height as i32),
        0,
    )
}

fn scale_script_coord(value: i32) -> i32 {
    if value <= 4096 {
        ((value as f32) * 1.5) as i32
    } else {
        value
    }
}

fn place_sprite_x(mode: i32, width: i32, logical_width: i32) -> i32 {
    if mode < 0 {
        -width
    } else if mode <= 20 {
        ((logical_width * mode) / 20) - (width >> 1)
    } else {
        logical_width
    }
}

fn place_sprite_y(mode: i32, height: i32, logical_height: i32) -> i32 {
    match mode {
        0 => 0,
        1 => logical_height - (height >> 1),
        2 => logical_height - height,
        other => other - 2,
    }
}

fn open_resource_variant(
    resource_manager: &mut ResourceManager,
    name: &str,
    extensions: &[&str],
) -> pal_asset::Result<pal_asset::LoadedAsset> {
    let has_extension = name
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|leaf| leaf.contains('.'));
    if has_extension {
        return resource_manager.open(name);
    }
    let mut last_error = None;
    for ext in extensions {
        let candidate = format!("{name}{ext}");
        match resource_manager.open(&candidate) {
            Ok(asset) => return Ok(asset),
            Err(err) => last_error = Some(err),
        }
    }
    Err(
        last_error.unwrap_or_else(|| pal_asset::AssetError::AssetNotFound {
            name: name.to_owned(),
        }),
    )
}

fn parse_solid_color_name(name: &str) -> Option<(u8, u8, u8)> {
    if name.eq_ignore_ascii_case("BK_BLACK") {
        return Some((0, 0, 0));
    }
    let hex = name.strip_prefix('#').unwrap_or(name);
    let raw = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some((
            ((raw >> 16) & 0xFF) as u8,
            ((raw >> 8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        )),
        8 => Some((
            ((raw >> 16) & 0xFF) as u8,
            ((raw >> 8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        )),
        _ => None,
    }
}

fn parse_pal_text_directives(text: &str) -> (&str, Option<u16>) {
    let Some(rest) = text.strip_prefix('<') else {
        return (text, None);
    };
    let Some(end) = rest.find('>') else {
        return (text, None);
    };
    let directive = &rest[..end];
    let body = &rest[end + 1..];
    let mut size = None;
    for part in directive.split(|ch| ch == ';' || ch == ',' || ch == ' ') {
        let Some(raw_size) = part.strip_prefix("size=") else {
            continue;
        };
        if let Ok(value) = raw_size.parse::<u16>() {
            size = Some(value.max(1));
        }
    }
    (body, size)
}

fn read_text_record_string(bytes: &[u8], offset: usize, nls: Nls) -> Option<String> {
    let start = offset.checked_add(4)?;
    read_c_string(bytes, start, nls)
}

fn read_c_string(bytes: &[u8], offset: usize, nls: Nls) -> Option<String> {
    if offset >= bytes.len() {
        return None;
    }
    let end = bytes[offset..]
        .iter()
        .position(|b| *b == 0)
        .map(|rel| offset + rel)
        .unwrap_or(bytes.len());
    if end == offset {
        return Some(String::new());
    }
    nls.decode(&bytes[offset..end]).ok()
}

fn read_padded_c_string(bytes: &[u8], mut offset: usize, nls: Nls) -> Option<String> {
    let start = offset;
    while offset < bytes.len() && bytes[offset] == 0 && offset.saturating_sub(start) < 4 {
        offset += 1;
    }
    read_c_string(bytes, offset, nls).filter(|value| is_plausible_resource_name(value))
}

fn read_file_slot_string(bytes: &[u8], value: i32, nls: Nls) -> Option<String> {
    let index: usize = value.try_into().ok()?;
    let offset = 0x10usize.checked_add(index.checked_mul(0x20)?)?;
    read_c_string(bytes, offset, nls).filter(|value| is_plausible_resource_name(value))
}

fn is_plausible_resource_name(value: &str) -> bool {
    if value.is_empty() || value.starts_with('$') || value.contains('*') || value.contains("__3I") {
        return false;
    }
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'#' | b'.'))
}

fn asset_source_label(source: &AssetSource) -> String {
    match source {
        AssetSource::Loose { path } => format!("loose:{}", path.display()),
        AssetSource::Pac { pac_path, .. } => format!("pac:{}", pac_path.display()),
    }
}

fn ini_filename_or_system(requested: &str) -> &str {
    if requested.to_ascii_lowercase().ends_with(".ini") {
        requested
    } else {
        "SYSTEM.INI"
    }
}

fn parse_file_table(bytes: &[u8], nls: Nls) -> anyhow::Result<ParsedFileTable> {
    let text = nls
        .decode(bytes)
        .map_err(|err| anyhow::anyhow!("file table NLS decode failed: {err}"))?;
    let compact = compact_csv_text(&text);
    let tokens = split_csv_tokens(&compact);
    let mut entries = Vec::with_capacity(tokens.len());
    let mut strings = BTreeMap::new();
    let mut string_offset = 0i32;
    for token in tokens {
        if token.is_empty() {
            continue;
        }
        if let Some(stripped) = token.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            let value = stripped.replace("\"\"", "\"");
            entries.push(0x8000_0000u32.wrapping_add(string_offset as u32) as i32);
            strings.insert(string_offset, value.clone());
            string_offset = string_offset
                .saturating_add(2)
                .saturating_add(value.as_bytes().len().min(i32::MAX as usize) as i32);
        } else {
            entries.push(parse_table_int(&token));
        }
    }
    Ok(ParsedFileTable { entries, strings })
}

fn compact_csv_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_quote = false;
    while let Some(ch) = chars.next() {
        if !in_quote && ch == '/' && chars.peek() == Some(&'/') {
            while let Some(next) = chars.next() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
            out.push(ch);
            continue;
        }
        if !in_quote && ch.is_whitespace() {
            if ch == '\n' {
                out.push('\n');
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn split_csv_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut in_quote = false;
    while let Some(ch) = chars.next() {
        if ch == '"' {
            current.push(ch);
            if in_quote && chars.peek() == Some(&'"') {
                current.push(chars.next().expect("peeked quote"));
            } else {
                in_quote = !in_quote;
            }
            continue;
        }
        if !in_quote && (ch == ',' || ch == '\n') {
            tokens.push(current.trim().to_owned());
            current.clear();
            continue;
        }
        current.push(ch);
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_owned());
    }
    tokens
}

fn parse_table_int(token: &str) -> i32 {
    if let Some(hex) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("0X"))
    {
        i32::from_str_radix(hex, 16).unwrap_or(0)
    } else {
        token.parse::<i32>().unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeTick {
    pub executed: usize,
    pub status: RuntimeStatus,
    /// A blocking wait task requested by the VM this tick. Engine creates the task
    /// and assigns the handle back to the runtime via set_wait_handle().
    pub wait_request: Option<WaitRequest>,
    /// Per-frame events: skipped extcalls, wait requests, unsupported ops.
    pub frame_events: Vec<FrameEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeStatus {
    NotBooted,
    Running {
        pc: u32,
    },
    /// Waiting on a 1-frame task created in TaskSystem (opcode 252).
    WaitFrame {
        pc: u32,
    },
    /// Waiting on an input-push task created in TaskSystem (opcode 253).
    WaitClick {
        pc: u32,
    },
    Halted {
        pc: u32,
    },
    UnsupportedCommand {
        pc: u32,
        opcode: u16,
        name: Option<String>,
    },
    UnsupportedExtCall {
        pc: u32,
        category: u16,
        index: u16,
        name: Option<String>,
        dst_slot: u32,
    },
    Faulted {
        pc: u32,
        message: String,
    },
}

impl RuntimeStatus {
    pub fn pc(&self) -> Option<u32> {
        match self {
            Self::NotBooted => None,
            Self::Running { pc }
            | Self::WaitFrame { pc }
            | Self::WaitClick { pc }
            | Self::Halted { pc }
            | Self::UnsupportedCommand { pc, .. }
            | Self::UnsupportedExtCall { pc, .. }
            | Self::Faulted { pc, .. } => Some(*pc),
        }
    }

    pub fn is_blocked(&self) -> bool {
        matches!(
            self,
            Self::WaitFrame { .. }
                | Self::WaitClick { .. }
                | Self::Halted { .. }
                | Self::UnsupportedCommand { .. }
                | Self::UnsupportedExtCall { .. }
                | Self::Faulted { .. }
        )
    }
}

impl fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBooted => write!(f, "not booted"),
            Self::Running { pc } => write!(f, "running pc=0x{pc:08X}"),
            Self::WaitFrame { pc } => write!(f, "wait frame (task) pc=0x{pc:08X}"),
            Self::WaitClick { pc } => write!(f, "wait click (task) pc=0x{pc:08X}"),
            Self::Halted { pc } => write!(f, "halted pc=0x{pc:08X}"),
            Self::UnsupportedCommand { pc, opcode, name } => match name {
                Some(name) => write!(f, "unsupported command pc=0x{pc:08X} opcode={}({})", opcode, name),
                None => write!(f, "unsupported command pc=0x{pc:08X} opcode={}", opcode),
            },
            Self::UnsupportedExtCall { pc, category, index, name, dst_slot } => match name {
                Some(name) => write!(
                    f,
                    "unsupported extcall pc=0x{pc:08X} ext_{category:04X}_{index:04X}.{} dst_slot={}",
                    name, dst_slot
                ),
                None => write!(f, "unsupported extcall pc=0x{pc:08X} ext_{category:04X}_{index:04X} dst_slot={}", dst_slot),
            },
            Self::Faulted { pc, message } => write!(f, "faulted pc=0x{pc:08X}: {message}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    ScriptParse {
        source: String,
    },
    ReadOutOfBounds {
        pc: u32,
        len: usize,
    },
    InvalidInstructionWord {
        pc: u32,
        word: u32,
    },
    PointResolve {
        point_id: u32,
        source: String,
    },
    DivideByZero {
        pc: u32,
    },
    ReturnStackUnderflow {
        pc: u32,
    },
    StackUnderflow {
        pc: u32,
    },
    StackOverflow {
        pc: u32,
        limit: usize,
    },
    ArgumentStackUnderflow {
        pc: u32,
    },
    NegativeCount {
        pc: u32,
        count: i32,
    },
    VariableOutOfRange {
        slot: usize,
    },
    StackSlotOutOfRange {
        slot: usize,
        depth: usize,
    },
    ArgumentSlotOutOfRange {
        slot: usize,
        depth: usize,
    },
    MemDatOutOfRange {
        offset: usize,
        len: usize,
    },
    MemIndexOutOfRange {
        index: usize,
        len: usize,
        kind: &'static str,
    },
    UnsupportedOperandRead {
        raw: u32,
        kind: String,
    },
    UnsupportedOperandWrite {
        raw: u32,
        kind: String,
    },
    UnsupportedInternalOpcode {
        pc: u32,
        opcode: u16,
    },
    ArithmeticOverflow {
        pc: u32,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ScriptParse { source } => write!(f, "failed to parse Script.src: {source}"),
            Self::ReadOutOfBounds { pc, len } => {
                write!(
                    f,
                    "script read out of bounds at pc=0x{pc:08X}, len=0x{len:X}"
                )
            }
            Self::InvalidInstructionWord { pc, word } => {
                write!(f, "invalid instruction word at pc=0x{pc:08X}: 0x{word:08X}")
            }
            Self::PointResolve { point_id, source } => {
                write!(f, "failed to resolve point id {}: {}", point_id, source)
            }
            Self::DivideByZero { pc } => write!(f, "script divide by zero at pc=0x{pc:08X}"),
            Self::ReturnStackUnderflow { pc } => {
                write!(f, "script return stack underflow at pc=0x{pc:08X}")
            }
            Self::StackUnderflow { pc } => write!(f, "script stack underflow at pc=0x{pc:08X}"),
            Self::StackOverflow { pc, limit } => {
                write!(f, "script stack overflow at pc=0x{pc:08X}, limit={limit}")
            }
            Self::ArgumentStackUnderflow { pc } => {
                write!(f, "script argument stack underflow at pc=0x{pc:08X}")
            }
            Self::NegativeCount { pc, count } => {
                write!(f, "negative count {} at pc=0x{pc:08X}", count)
            }
            Self::VariableOutOfRange { slot } => {
                write!(f, "variable slot {} is out of range", slot)
            }
            Self::StackSlotOutOfRange { slot, depth } => {
                write!(f, "stack slot {} is out of range for depth {}", slot, depth)
            }
            Self::ArgumentSlotOutOfRange { slot, depth } => {
                write!(
                    f,
                    "argument stack reverse slot {} is out of range for depth {}",
                    slot, depth
                )
            }
            Self::MemDatOutOfRange { offset, len } => {
                write!(
                    f,
                    "Mem.dat read out of range at offset=0x{offset:X}, len=0x{len:X}"
                )
            }
            Self::MemIndexOutOfRange { index, len, kind } => {
                write!(f, "{kind} index {index} out of range (len={len})")
            }
            Self::UnsupportedOperandRead { raw, kind } => {
                write!(f, "unsupported operand read kind={} raw=0x{raw:08X}", kind)
            }
            Self::UnsupportedOperandWrite { raw, kind } => {
                write!(f, "unsupported operand write kind={} raw=0x{raw:08X}", kind)
            }
            Self::UnsupportedInternalOpcode { pc, opcode } => {
                write!(
                    f,
                    "internal opcode {} is unsupported at pc=0x{pc:08X}",
                    opcode
                )
            }
            Self::ArithmeticOverflow { pc } => {
                write!(f, "arithmetic overflow near pc=0x{pc:08X}")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

fn bool_to_i32(value: bool) -> i32 {
    if value {
        1
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepResult {
    Continue,
    Blocked,
    BlockedWithWait(WaitRequest),
}
