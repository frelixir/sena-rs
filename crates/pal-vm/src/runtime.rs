use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use pal_asset::{AssetSource, Nls, ResourceManager};
use pal_script::extsig::{lookup_sig, observed_pop_count};
use pal_script::opcodes::{ext_opcode, primary_opcode};
use pal_script::{Operand, OperandKind, PointTable, ScriptImage};

use crate::assets::CoreAssets;
use crate::audio::{AudioHandle, AudioSystem, PalSoundGroup, PalVolume};
use crate::config::{ini_graphics_size, parse_ini_nls, IniFile};
use crate::effect::PalEffectSystem;
use crate::font::PalFontSystem;
use crate::image::{decode_image, DecodedImage};
use crate::input::{PalInputState, PalMouseButton};
use crate::msprite::{MSpriteHandle, MSpriteSystem, MSPRITE_STATE_FINISHED};
use crate::scene::{rasterize_scene_rgba, FrameScene, SceneTextureId, SolidQuad};
use crate::sprite::{
    PalAnimationFlags, PalColor, PalRect, PalVec3, SpriteDesc, SpriteHandle, SpriteSurface,
    SpriteSystem, SpriteTransitionHandle,
};
use crate::system::PalRandomState;
use crate::system::PalSystemState;
use crate::task::{TaskHandle, TaskSystem};
use crate::PalSheetAnimationDesc;

const DEFAULT_VAR_COUNT: usize = 0x10000;
const DEFAULT_STACK_LIMIT: usize = 0x10000;
/// Size of user_mem, system_mem, and temp_mem arrays (matches original engine).
const DEFAULT_MEM_SIZE: usize = 0x10000;
/// Maximum per-frame events recorded for PAL_DEBUG dump.
const MAX_FRAME_EVENTS: usize = 64;

fn debug_vm_enabled() -> bool {
    std::env::var("DEBUG_VM")
        .ok()
        .as_deref()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn debug_vm_pc_bound(name: &str) -> Option<u32> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u32>().ok()
    }
}

fn clamp_percent(value: i32) -> i32 {
    value.clamp(0, 100)
}

fn percent_to_volume(value: i32) -> PalVolume {
    PalVolume::from_raw(clamp_percent(value).saturating_mul(100))
}

fn volume_to_percent(volume: PalVolume) -> i32 {
    (volume.raw() / 100).clamp(0, 100)
}

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
    /// Wait until either a click/input push arrives or N PAL milliseconds elapse.
    ClickOrTime(u32),
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
    action_state: ActionSubsystemState,
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
    /// PAL transition handles keyed by script transition slot.
    game_sprite_transitions: BTreeMap<i32, SpriteTransitionHandle>,
    /// PalAnimation tasks attached to Game script image slots.
    game_sprite_animations: BTreeMap<i32, TaskHandle>,
    /// Named ANI records that target a sprite slot before that slot has a concrete surface.
    game_sprite_pending_named_animations: BTreeMap<i32, PendingNamedSpriteAnimation>,
    /// Alpha actions that targeted a normal script sprite before it had a surface.
    game_sprite_pending_alpha: BTreeMap<i32, Vec<PendingAlphaAction>>,
    /// Game.exe MSprite wrapper state keyed by script slot.
    game_msprites: BTreeMap<i32, GameMSpriteState>,
    /// Native msp_wait stores the slot state and rewinds PC until PalMSpriteGetState has bit 4.
    pending_msp_wait_slot: Option<i32>,
    /// Game button entries keyed by (button group, entry index).
    game_buttons: BTreeMap<(i32, i32), GameButtonEntry>,
    /// Latched button pushes keyed by group; consumed by btn_get_push(group).
    button_push_queue: BTreeMap<i32, VecDeque<i32>>,
    /// Game script sound slots mapped to PAL audio handles. Key is (script category, slot).
    game_audio: BTreeMap<(u16, i32), AudioHandle>,
    /// Native voice_wait stores a wait mask and rewinds PC until the voice checker reports idle.
    pending_voice_wait_slot: Option<i32>,
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
    system_state: PalSystemState,
    /// PAL random seed ring used by PalRandomEx and Game category 20 random.
    random_state: PalRandomState,
    /// Per-frame events accumulated during run_frame(); cleared at the start of each frame.
    frame_events: Vec<FrameEvent>,
    /// Button callback gosub to inject at the top of the next script frame (point ID, not PC).
    pending_gosub_point: Option<u32>,
    /// Category 9:23 continuation target.  Unlike button callbacks, this is a
    /// process/menu jump and must not push a return PC or the cleanup routine
    /// returns to the old per-frame wait loop.
    pending_jump_point: Option<u32>,
    /// Game category 9:24 stores the menu transition mode consumed by the
    /// subsequent category 9:23 continuation-point switch.  The script still
    /// owns the selected page state in Mem.dat[158].
    menu_transition_mode: i32,
    /// Set by ext_000F_0005(nonzero) to mirror the native misc-system latch.
    /// The title wait loop owns its `memdat[1100]` flag in script; clearing it
    /// from wait_sync_step makes the final title screen fall through without a
    /// button callback.
    misc_system_pending_complete: bool,
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

#[derive(Clone, Debug)]
struct GameButtonEntry {
    handle: SpriteHandle,
    name: String,
    enabled: bool,
    locked: bool,
    toggle: i32,
    alpha: u8,
    slider_offset: i32,
    hit_rect: Option<[i32; 4]>,
    /// Callback point registered by btn_set arg[3]; injected as gosub on click.
    gosub_point: Option<u32>,
}

#[derive(Clone, Debug)]
struct PendingNamedSpriteAnimation {
    asset_name: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct PendingAlphaAction {
    alpha_delta: i32,
    duration_ms: u32,
    started_ms: u32,
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

#[derive(Clone, Debug)]
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
    last_text_args: [i32; 4],
    init_args: [i32; 8],
    last_event_time_ms: u32,
    reveal_start_ms: u32,
    reveal_duration_ms: u32,
    reveal_enabled: bool,
    sprite: Option<SpriteHandle>,
    name_sprite: Option<SpriteHandle>,
    /// Slot 255 alpha actions can arrive while text_clear has removed the
    /// concrete text sprites; keep them until the next ADV text surface exists.
    pending_alpha: Vec<PendingAlphaAction>,
    dirty: bool,
}

impl Default for TextSubsystemState {
    fn default() -> Self {
        Self {
            initialized: false,
            visible: false,
            rect: [0; 4],
            alpha: 0,
            mode: 0,
            base: 0,
            icon: 0,
            button: 0,
            history_enabled: false,
            voice_cut_enabled: false,
            voice_enabled: true,
            // VmCtx_Init initializes PAL sound volume fields to 5000; native
            // voice_get_volume returns 100 * field / 10000, so script-visible
            // default volume is 50.
            voice_volume: 50,
            bgv_enabled: true,
            bgv_volume: 50,
            bgv_muted: false,
            voice_autopan_enabled: false,
            last_text_value: 0,
            last_text_args: [0; 4],
            init_args: [0; 8],
            last_event_time_ms: 0,
            reveal_start_ms: 0,
            reveal_duration_ms: 0,
            reveal_enabled: false,
            sprite: None,
            name_sprite: None,
            pending_alpha: Vec::new(),
            dirty: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SelectSubsystemState {
    initialized: bool,
    locked: bool,
    rect: [i32; 4],
    text_value: i32,
    colors: [i32; 3],
    offsets: BTreeMap<i32, i32>,
    options: Vec<SelectOption>,
    process: [i32; 3],
    last_key: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SelectOption {
    text_value: i32,
    target_value: i32,
    x: i32,
    y: i32,
    color: i32,
    flags: i32,
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

#[derive(Clone, Debug, Default)]
struct ActionSubsystemState {
    active_id: i32,
    last_duration_ms: u32,
    started_ms: u32,
    clear_flags: [bool; 16],
}

impl ActionSubsystemState {
    fn clear(&mut self) {
        self.last_duration_ms = 0;
        self.started_ms = 0;
        self.clear_flags = [false; 16];
    }

    fn set_active(&mut self, id: i32) {
        self.active_id = id;
    }

    fn set_clear(&mut self, id: i32) {
        let resolved = if id == -1 { self.active_id } else { id };
        if (0..16).contains(&resolved) {
            self.clear_flags[resolved as usize] = true;
        }
    }

    fn schedule(&mut self, pal_time_ms: u32, duration_ms: u32) {
        self.started_ms = pal_time_ms;
        self.last_duration_ms = duration_ms;
    }

    fn is_over(&self, pal_time_ms: u32, duration_ms: u32) -> bool {
        let duration = duration_ms.max(self.last_duration_ms);
        duration == 0 || pal_time_ms.wrapping_sub(self.started_ms) >= duration
    }
}

fn action_duration_from_args(args: &[i32]) -> u32 {
    args.iter()
        .copied()
        .filter(|value| *value > 0)
        .max()
        .unwrap_or(0) as u32
}

fn ext_args_source_order<const N: usize>(args: &[i32]) -> [i32; N] {
    let mut ordered = [0; N];
    for (dst, value) in args.iter().rev().take(N).enumerate() {
        ordered[dst] = *value;
    }
    ordered
}

fn checked_script_mem_index(kind: &str, signed_idx: i32, len: usize) -> Option<usize> {
    if signed_idx < 0 {
        log::debug!(
            "[trace-vm] {kind} signed index {signed_idx} out of range (len={len}); returning zero / ignoring write"
        );
        return None;
    }
    let idx = signed_idx as usize;
    if idx >= len {
        log::debug!(
            "[trace-vm] {kind} index {idx} out of range (len={len}); returning zero / ignoring write"
        );
        return None;
    }
    Some(idx)
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
            action_state: ActionSubsystemState::default(),
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
            game_sprite_transitions: BTreeMap::new(),
            game_sprite_animations: BTreeMap::new(),
            game_sprite_pending_named_animations: BTreeMap::new(),
            game_sprite_pending_alpha: BTreeMap::new(),
            game_msprites: BTreeMap::new(),
            pending_msp_wait_slot: None,
            game_buttons: BTreeMap::new(),
            button_push_queue: BTreeMap::new(),
            game_audio: BTreeMap::new(),
            pending_voice_wait_slot: None,
            font_state: PalFontSystem::new(),
            text_state: TextSubsystemState::default(),
            select_state: SelectSubsystemState::default(),
            save_state: SaveSubsystemState::default(),
            history_state: HistorySubsystemState::default(),
            thread_state: ThreadSubsystemState::default(),
            message_state: MessageSubsystemState::default(),
            dynamic_strings: Vec::new(),
            system_ini: None,
            system_state: PalSystemState::new(),
            random_state: PalRandomState::default(),
            frame_events: Vec::new(),
            pending_gosub_point: None,
            pending_jump_point: None,
            menu_transition_mode: 0,
            misc_system_pending_complete: false,
        }
    }

    pub fn set_system_ini(&mut self, ini: IniFile) {
        let (width, height) = ini_graphics_size(
            Some(&ini),
            FrameScene::PAL_DEFAULT_WIDTH,
            FrameScene::PAL_DEFAULT_HEIGHT,
        );
        self.system_state
            .set_logical_size(width as i32, height as i32);
        self.system_ini = Some(ini);
    }

    pub fn set_window_size(&mut self, width: u32, height: u32) {
        self.system_state
            .set_window_size(width.max(1) as i32, height.max(1) as i32);
    }

    fn vm_trace_enabled(&self) -> bool {
        if !self.trace && !debug_vm_enabled() {
            return false;
        }
        let pc = self.pc;
        if let Some(start) = debug_vm_pc_bound("DEBUG_VM_PC_START") {
            if pc < start {
                return false;
            }
        }
        if let Some(end) = debug_vm_pc_bound("DEBUG_VM_PC_END") {
            if pc > end {
                return false;
            }
        }
        true
    }

    fn vm_trace(&self, args: fmt::Arguments<'_>) {
        if !self.vm_trace_enabled() {
            return;
        }
        if debug_vm_enabled() {
            eprintln!("[DEBUG_VM] {args}");
        } else if self.trace {
            log::debug!("{args}");
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
        let old_handle = self.wait_task_handle;
        let old_status = self.status.clone();
        self.wait_task_handle = None;
        if let Some(pc) = self.status.pc() {
            self.status = RuntimeStatus::Running { pc };
        }
        if debug_vm_enabled() || matches!(old_status, RuntimeStatus::WaitClick { .. }) {
            log::debug!(
                "[trace-wait] resolve_pending_wait handle={old_handle:?} old_status={old_status} new_status={}",
                self.status
            );
        }
    }

    /// Associate the runtime with a newly created wait task handle.
    pub fn set_wait_handle(&mut self, handle: TaskHandle) {
        self.wait_task_handle = Some(handle);
        if debug_vm_enabled() || matches!(self.status, RuntimeStatus::WaitClick { .. }) {
            log::debug!(
                "[trace-wait] set_wait_handle handle={handle:?} status={}",
                self.status
            );
        }
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

    pub fn sync_text_sprite(
        &mut self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: &mut SpriteSystem,
    ) {
        if !self.text_state.initialized || !self.text_state.visible {
            if let Some(handle) = self.text_state.sprite.take() {
                let _ = sprites.release(handle);
            }
            if let Some(handle) = self.text_state.name_sprite.take() {
                let _ = sprites.release(handle);
            }
            self.text_state.dirty = false;
            return;
        }
        let reveal_complete = !self.text_state.reveal_enabled
            || self
                .pal_time_ms
                .wrapping_sub(self.text_state.reveal_start_ms)
                >= self.text_state.reveal_duration_ms;
        if !self.text_state.dirty && reveal_complete {
            return;
        }
        let log_sync = self.text_state.dirty;
        let (body, name) = self.adv_text_parts_for_render(assets, nls);
        if log_sync {
            log::debug!(
                "[trace-text] sync visible={} args={:?} name={name:?} body={body:?}",
                self.text_state.visible,
                self.text_state.last_text_args
            );
        }
        if body.is_empty() {
            if let Some(handle) = self.text_state.sprite.take() {
                let _ = sprites.release(handle);
            }
            if let Some(handle) = self.text_state.name_sprite.take() {
                let _ = sprites.release(handle);
            }
            self.text_state.dirty = false;
            return;
        }
        let saved_size = self.font_state.font_size();
        let saved_color = self.font_state.color();
        if saved_size < 22 {
            self.font_state.set_font_size(24);
        }
        self.font_state.set_color(0xFFFF_FFFF, saved_color.1);
        let (text_body, temporary_size) = parse_pal_text_directives(&body);
        let full_text = text_body;
        let mut text = full_text.clone();
        if let Some(size) = temporary_size {
            self.font_state.set_font_size(size);
        }
        if self.text_state.reveal_enabled {
            let elapsed = self
                .pal_time_ms
                .wrapping_sub(self.text_state.reveal_start_ms);
            let duration = self.text_state.reveal_duration_ms.max(1);
            if elapsed < duration {
                let total = text.chars().count().max(1);
                let visible = ((elapsed as u64 * total as u64) / duration as u64)
                    .clamp(1, total as u64) as usize;
                text = text.chars().take(visible).collect();
            } else {
                self.text_state.reveal_enabled = false;
            }
        }
        let (panel_text_width, panel_text_height) = self.font_state.measure(&full_text);
        let (text_width, text_height, text_rgba) = self.font_state.rasterize(&text);
        let base_image =
            self.load_text_base_image(assets, nls, resource_manager, self.text_state.base);
        let has_base_image = base_image.is_some();
        let x = if self.text_state.init_args[3] != 0 {
            self.text_state.init_args[3]
        } else if self.text_state.rect[0] != 0 {
            self.text_state.rect[0]
        } else {
            72
        };
        let text_draw_x = if self.text_state.init_args[6] != 0 {
            self.text_state.init_args[6]
        } else {
            x + 24
        };
        let text_draw_y = if self.text_state.init_args[5] != 0 {
            self.text_state.init_args[5]
        } else {
            self.text_state.rect[1].saturating_add(18)
        };
        let y = if has_base_image && self.text_state.init_args[1] != 0 {
            self.text_state.init_args[1]
        } else if text_draw_y != 0 {
            text_draw_y.saturating_sub(18)
        } else if self.text_state.rect[1] != 0 {
            self.text_state.rect[1]
        } else {
            558
        };
        let text_origin_x = text_draw_x.saturating_sub(x).max(0) as u32;
        let text_origin_y = text_draw_y.saturating_sub(y).max(0) as u32;
        let z = 30_000;
        let min_width = self.text_state.init_args[4].max(760) as u32;
        let (width, height, rgba) = compose_adv_text_panel(
            text_width,
            text_height,
            text_rgba,
            panel_text_width,
            panel_text_height,
            min_width,
            88,
            base_image,
            self.text_state.alpha,
            text_origin_x,
            text_origin_y,
        );
        if let Some(handle) = self.text_state.sprite {
            let _ =
                sprites.replace_sprite_surface(handle, width, height, rgba, format!("adv:{text}"));
            let _ = sprites.set_pos(handle, x, y, z);
            let _ = sprites.set_priority(handle, z);
            let _ = sprites.view_ctrl(handle, true);
        } else if let Some(handle) = sprites.create_rgba_sprite(
            width,
            height,
            rgba,
            PalVec3::new(x, y, z),
            z,
            format!("adv:{text}"),
        ) {
            self.text_state.sprite = Some(handle);
        }
        if name.is_empty() {
            if let Some(handle) = self.text_state.name_sprite.take() {
                let _ = sprites.release(handle);
            }
        } else {
            self.font_state.set_font_size(22);
            let (name, _) = parse_pal_text_directives(&name);
            let (name_width, name_height, name_rgba) = self.font_state.rasterize(&name);
            let name_surface_width = name_width.max(1);
            let name_surface_height = name_height.max(1);
            let name_surface = name_rgba;
            let name_x = if self.text_state.init_args[2] != 0 {
                self.text_state.init_args[2]
            } else {
                x + 18
            };
            let name_y = if self.text_state.init_args[1] != 0 {
                self.text_state.init_args[1]
            } else {
                y.saturating_sub(name_surface_height as i32 + 8)
            };
            let name_z = z + 1;
            if let Some(handle) = self.text_state.name_sprite {
                let _ = sprites.replace_sprite_surface(
                    handle,
                    name_surface_width,
                    name_surface_height,
                    name_surface,
                    format!("adv-name:{name}"),
                );
                let _ = sprites.set_pos(handle, name_x, name_y, name_z);
                let _ = sprites.set_priority(handle, name_z);
                let _ = sprites.view_ctrl(handle, true);
            } else if let Some(handle) = sprites.create_rgba_sprite(
                name_surface_width,
                name_surface_height,
                name_surface,
                PalVec3::new(name_x, name_y, name_z),
                name_z,
                format!("adv-name:{name}"),
            ) {
                self.text_state.name_sprite = Some(handle);
            }
        }
        self.apply_pending_text_alpha_actions(sprites);
        self.font_state.set_font_size(saved_size);
        self.font_state.set_color(saved_color.0, saved_color.1);
        self.text_state.dirty = false;
    }

    fn adv_text_parts_for_render(&self, assets: &CoreAssets, nls: Nls) -> (String, String) {
        let body = self
            .resolved_dialog_text_arg(self.text_state.last_text_args[1], assets, nls)
            .unwrap_or_default();
        let name = self
            .resolved_dialog_text_arg(self.text_state.last_text_args[2], assets, nls)
            .unwrap_or_default();
        (body, name)
    }

    fn load_text_base_image(
        &self,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        base_value: i32,
    ) -> Option<DecodedImage> {
        if base_value == 0 || base_value == 0x0FFF_FFFF {
            return None;
        }
        let Some(name) = self.resolve_resource_string(base_value, assets, nls) else {
            log::debug!("[trace-text] text_set_base unresolved value={base_value}");
            return None;
        };
        if name.is_empty() {
            return None;
        }
        let Some(resource_manager) = resource_manager else {
            return None;
        };
        let asset = match open_resource_variant(resource_manager, &name, IMAGE_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                log::debug!("[trace-text] text_set_base name={name:?} image open failed: {err}");
                return None;
            }
        };
        match decode_image(&asset.bytes) {
            Ok(decoded) => {
                log::debug!(
                    "[trace-text] text_set_base name={name:?} asset={:?} size={}x{}",
                    asset.name,
                    decoded.width,
                    decoded.height
                );
                Some(decoded)
            }
            Err(err) => {
                log::debug!(
                    "[trace-text] text_set_base name={name:?} asset={:?} decode failed: {err}",
                    asset.name
                );
                None
            }
        }
    }

    fn resolved_dialog_text_arg(
        &self,
        value: i32,
        assets: &CoreAssets,
        nls: Nls,
    ) -> Option<String> {
        if value == 0 || value == 0x0FFF_FFFF {
            return None;
        }
        let text = self.resolve_script_string(value, assets, nls)?;
        let text = parse_pal_text_directives(&text).0.trim().to_owned();
        if text.is_empty() || looks_like_media_or_resource_name(&text) {
            None
        } else {
            Some(text)
        }
    }

    fn text_wait_duration_ms(&self) -> u32 {
        let chars = self
            .text_state
            .last_text_args
            .iter()
            .copied()
            .skip(1)
            .filter(|value| *value != 0x0FFF_FFFF)
            .count()
            .max(1) as u32;
        500_u32.saturating_add(chars.saturating_mul(350)).min(3500)
    }

    fn text_reveal_duration_ms(&self, text_value: i32) -> u32 {
        let char_count = dynamic_string_index(text_value)
            .and_then(|idx| self.dynamic_strings.get(idx))
            .map(|text| parse_pal_text_directives(text).0.chars().count())
            .unwrap_or(0)
            .max(1) as u32;
        char_count
            .saturating_mul(45)
            .clamp(120, 2200)
            .max(self.text_wait_duration_ms().min(1600))
    }

    pub fn update_button_input_state(&mut self, sprites: &mut SpriteSystem, input: &PalInputState) {
        let (mouse_x, mouse_y) = input.mouse_position();
        let hovered = self.button_hit_at(sprites, mouse_x, mouse_y, -1);
        if input.mouse_push(PalMouseButton::Left) {
            if let Some((group, index)) = hovered {
                self.button_push_queue
                    .entry(group)
                    .or_default()
                    .push_back(index);
                self.hide_title_buttons_for_modal_entry(group, index, sprites);
                self.dispatch_button_push_compat(group, index);
                log::debug!(
                    "[trace-button] push latch group={group} index={index} pos=({mouse_x},{mouse_y})"
                );
            }
        }

        let mouse_down = input.mouse_on(PalMouseButton::Left);
        let keys = self.game_buttons.keys().copied().collect::<Vec<_>>();
        for key @ (group, index) in keys {
            let Some(entry) = self.game_buttons.get(&key).cloned() else {
                continue;
            };
            let Some(sprite) = sprites.get(entry.handle) else {
                continue;
            };
            if !sprite.visible {
                continue;
            }
            let row = if !entry.enabled || entry.locked {
                3
            } else if hovered == Some((group, index)) {
                if mouse_down {
                    2
                } else {
                    1
                }
            } else if entry.toggle != 0 {
                1
            } else {
                0
            };
            sprites.rect_set_pos(entry.handle, 0, row);
        }
    }

    pub fn run_frame(
        &mut self,
        assets: &CoreAssets,
        config: &ScriptRuntimeConfig,
    ) -> Result<RuntimeTick, RuntimeError> {
        self.run_frame_with_resources(assets, None, None, None, None, None, config)
    }

    pub fn run_frame_with_resources(
        &mut self,
        assets: &CoreAssets,
        mut resource_manager: Option<&mut ResourceManager>,
        mut sprites: Option<&mut SpriteSystem>,
        mut task_system: Option<&mut TaskSystem>,
        mut audio: Option<&mut AudioSystem>,
        input: Option<&PalInputState>,
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

        // Inject pending script continuations before the script resumes.
        if let Some(point_id) = self.pending_jump_point.take() {
            match assets.point_table.resolve_target_pc(point_id) {
                Ok(Some(target_pc)) => {
                    self.pc = target_pc;
                    log::debug!(
                        "[trace-system] injected jump point[{point_id}] -> 0x{target_pc:08X}"
                    );
                }
                Ok(None) => {
                    log::warn!(
                        "[trace-system] jump point[{point_id}] resolved to no-op target, skipped"
                    );
                }
                Err(err) => {
                    log::warn!(
                        "[trace-system] jump point[{point_id}] resolve failed: {err}, skipped"
                    );
                }
            }
        }

        // Inject a pending button callback gosub before the script resumes.
        // The callback was stored by dispatch_button_push_compat when the user
        // clicked a button while the script was suspended in a wait_sync_step loop.
        if let Some(point_id) = self.pending_gosub_point.take() {
            match assets.point_table.resolve_target_pc(point_id) {
                Ok(Some(target_pc)) => {
                    self.call_stack.push(self.pc);
                    self.pc = target_pc;
                    log::debug!(
                        "[trace-button] injected gosub point[{point_id}] -> 0x{target_pc:08X} return=0x{:08X}",
                        self.call_stack.last().copied().unwrap_or(0)
                    );
                }
                Ok(None) => {
                    log::warn!(
                        "[trace-button] gosub point[{point_id}] resolved to no-op target, skipped"
                    );
                }
                Err(err) => {
                    log::warn!(
                        "[trace-button] gosub point[{point_id}] resolve failed: {err}, skipped"
                    );
                }
            }
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
                input,
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
        input: Option<&PalInputState>,
    ) -> Result<StepResult, RuntimeError> {
        let insn_pc = self.pc;
        let word = self.fetch_u32(script)?;
        let hi = ((word >> 16) & 0xFFFF) as u16;
        let opcode = (word & 0xFFFF) as u16;

        if hi != 1 {
            return Err(RuntimeError::InvalidInstructionWord { pc: insn_pc, word });
        }

        if self.vm_trace_enabled() {
            let name = primary_opcode(opcode)
                .map_or_else(|| format!("op_{opcode:04X}"), |meta| meta.name.to_owned());
            self.vm_trace(format_args!(
                "opcode pc=0x{insn_pc:08X} word=0x{word:08X} opcode={}({}) stack_len={} arg_len={} call_depth={}",
                opcode,
                name,
                self.stack.len(),
                self.argument_stack.len(),
                self.call_stack.len()
            ));
        }

        match opcode {
            // run_no_wait(): Game.exe sub_421F80 performs no VM pop and only
            // toggles/queues transition state.  It must not consume the
            // previous run(effect,arg1,arg2) arguments.
            1 => {
                let dst = self.fetch_operand(script)?;
                let src = self.fetch_operand(script)?;
                let value = self.eval_operand(src, mem_dat)?;
                self.vm_trace(format_args!("  op mov dst={dst} src={src} value={value}"));
                self.write_operand(dst, value)?;
            }
            2..=8 | 12..=19 | 26..=28 => {
                let dst = self.fetch_operand(script)?;
                let src = self.fetch_operand(script)?;
                let lhs = self.eval_operand(dst, mem_dat)?;
                let rhs = self.eval_operand(src, mem_dat)?;
                let value = self.eval_binary(opcode, lhs, rhs, insn_pc)?;
                self.vm_trace(format_args!(
                    "  op binary opcode={opcode} dst={dst} src={src} lhs={lhs} rhs={rhs} result={value}"
                ));
                self.write_operand(dst, value)?;
            }
            9 => {
                // jmp_operand: one operand whose VALUE is the point_id to jump to.
                let point_op = self.fetch_operand(script)?;
                let point_id = self.eval_operand(point_op, mem_dat)? as u32;
                self.vm_trace(format_args!(
                    "  op jmp point_operand={point_op} point={point_id}"
                ));
                self.jump_point(point_table, point_id)?;
            }
            10 => {
                let point_id = self.fetch_u32(script)?;
                let cond = self.fetch_operand(script)?;
                let value = self.eval_operand(cond, mem_dat)?;
                self.vm_trace(format_args!(
                    "  op jf point={point_id} cond={cond} value={value} taken={}",
                    value == 0
                ));
                if value == 0 {
                    self.jump_point(point_table, point_id)?;
                }
            }
            11 => {
                let point_id = self.fetch_u32(script)?;
                self.vm_trace(format_args!(
                    "  op gosub point={point_id} return_pc=0x{:08X}",
                    self.pc
                ));
                self.call_stack.push(self.pc);
                self.jump_point(point_table, point_id)?;
            }
            20 => {
                let raw = self.fetch_u32(script)?;
                let slot = (raw & 0xFFFF) as usize;
                let value = self.read_var(slot)?;
                self.vm_trace(format_args!(
                    "  op not slot={slot} old={value} new={}",
                    if value == 0 { 1 } else { 0 }
                ));
                self.write_var(slot, if value == 0 { 1 } else { 0 })?;
            }
            21 => {
                self.vm_trace(format_args!("  op halt next_pc=0x{:08X}", self.pc));
                self.status = RuntimeStatus::Halted { pc: self.pc };
                return Ok(StepResult::Blocked);
            }
            22 => {
                self.vm_trace(format_args!("  op nop"));
            }
            23 => {
                let raw = self.fetch_u32(script)?;
                let dst_slot_raw = self.fetch_u32(script)?;
                let category = ((raw >> 16) & 0xFFFF) as u16;
                let index = (raw & 0xFFFF) as u16;
                let name = ext_opcode(category, index).and_then(|meta| meta.name);
                self.vm_trace(format_args!(
                    "  op extcall raw=0x{raw:08X} ext_{category:04X}_{index:04X}.{} dst_raw=0x{dst_slot_raw:08X} stack_before={:?}",
                    name.unwrap_or("?"),
                    self.stack
                ));
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
                    input,
                );
                self.vm_trace(format_args!(
                    "  extcall result ext_{category:04X}_{index:04X}.{} => {:?} stack_after={:?}",
                    name.unwrap_or("?"),
                    result,
                    self.stack
                ));
                match result {
                    ExtCallOutcome::Value(v) => {
                        self.vm_trace(format_args!(
                            "  extcall write dst_slot[{dst_slot_raw}] value={v}"
                        ));
                        let _ = self.write_extcall_dst(dst_slot_raw, v);
                    }
                    ExtCallOutcome::Wait { value, request } => {
                        self.vm_trace(format_args!(
                            "  extcall wait dst_slot[{dst_slot_raw}] value={value} request={request:?}"
                        ));
                        let _ = self.write_extcall_dst(dst_slot_raw, value);
                        self.status = match request {
                            WaitRequest::Click | WaitRequest::ClickOrTime(_) => {
                                RuntimeStatus::WaitClick { pc: self.pc }
                            }
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
                        let cleanup_count = lookup_sig(category, index)
                            .map(|sig| sig.pop_count)
                            .or_else(|| observed_pop_count(category, index));
                        if let Some(count) = cleanup_count {
                            let dropped = self.pop_ext_args(count);
                            self.vm_trace(format_args!(
                                "  extcall skip cleanup argc={count} args={dropped:?}"
                            ));
                        }
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
                        self.vm_trace(format_args!(
                            "  extcall skip write dst_slot[{dst_slot_raw}] value=0"
                        ));
                        let _ = self.write_extcall_dst(dst_slot_raw, 0);
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
                self.vm_trace(format_args!("  op ret target=0x{return_pc:08X}"));
                self.pc = return_pc;
            }
            29 => {
                let raw = self.fetch_u32(script)?;
                let slot = (raw & 0xFFFF) as usize;
                let value = self.read_var(slot)?;
                self.vm_trace(format_args!(
                    "  op neg_slot slot={slot} old={value} new={}",
                    value.wrapping_neg()
                ));
                self.write_var(slot, value.wrapping_neg())?;
            }
            30 => {
                let dst = self.fetch_operand(script)?;
                let value = self
                    .stack
                    .pop()
                    .ok_or(RuntimeError::StackUnderflow { pc: insn_pc })?;
                self.vm_trace(format_args!(
                    "  op pop dst={dst} value={value} stack_len={}",
                    self.stack.len()
                ));
                self.write_operand(dst, value)?;
            }
            31 => {
                let src = self.fetch_operand(script)?;
                let value = self.eval_operand(src, mem_dat)?;
                self.vm_trace(format_args!("  op push src={src} value={value}"));
                self.push_stack(value, insn_pc)?;
            }
            32 => {
                let count_operand = self.fetch_operand(script)?;
                let count = self.eval_operand(count_operand, mem_dat)?;
                self.vm_trace(format_args!(
                    "  op pack_args count_operand={count_operand} count={count}"
                ));
                self.pack_args(count, insn_pc)?;
            }
            33 => {
                let count_operand = self.fetch_operand(script)?;
                let count = self.eval_operand(count_operand, mem_dat)?;
                self.vm_trace(format_args!(
                    "  op drop_args count_operand={count_operand} count={count}"
                ));
                self.drop_args(count, insn_pc)?;
            }
            252 => {
                // WaitFrame: create a 1-frame blocking task via TaskSystem.
                self.vm_trace(format_args!("  op wait_frame"));
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
                self.vm_trace(format_args!("  op wait_click"));
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
                self.vm_trace(format_args!(
                    "  op unsupported opcode={opcode} name={}",
                    name.as_deref().unwrap_or("?")
                ));
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
        let fetch_pc = self.pc;
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
        self.vm_trace(format_args!(
            "  fetch_u32 pc=0x{fetch_pc:08X} value=0x{value:08X} ({})",
            value as i32
        ));
        Ok(value)
    }

    fn fetch_operand(&mut self, script: &ScriptImage<'_>) -> Result<Operand, RuntimeError> {
        let raw = self.fetch_u32(script)?;
        let operand = Operand::decode(raw);
        self.vm_trace(format_args!(
            "  fetch_operand raw=0x{raw:08X} decoded={operand}"
        ));
        Ok(operand)
    }

    fn jump_point(&mut self, point_table: &PointTable, point_id: u32) -> Result<(), RuntimeError> {
        match point_table.resolve_target_pc(point_id) {
            Ok(Some(target)) => {
                self.vm_trace(format_args!(
                    "  jump_point point={point_id} target=0x{target:08X}"
                ));
                self.pc = target;
                Ok(())
            }
            Ok(None) => {
                self.vm_trace(format_args!("  jump_point point={point_id} target=<none>"));
                Ok(())
            }
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

    fn eval_operand(&self, operand: Operand, _mem_dat: &[u8]) -> Result<i32, RuntimeError> {
        if self.vm_trace_enabled() {
            self.vm_trace(format_args!("  eval_operand {operand}"));
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
                let signed_idx = self.read_var(operand.lo as usize)?;
                let Some(idx) =
                    checked_script_mem_index("user_mem", signed_idx, self.user_mem.len())
                else {
                    return Ok(0);
                };
                Ok(self.user_mem[idx])
            }
            // kind 0x2: system_mem[vars[lo]]
            OperandKind::SystemMemoryViaVar => {
                let signed_idx = self.read_var(operand.lo as usize)?;
                let Some(idx) =
                    checked_script_mem_index("system_mem", signed_idx, self.system_mem.len())
                else {
                    return Ok(0);
                };
                Ok(self.system_mem[idx])
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
            // kind 0x6: MemDatDirect — read from writable Mem.dat shadow.
            OperandKind::MemDatDirect => self.read_mem_dat_i32(operand),
            // kind 0x7: MemDatIndirect — complex double-indirection, not yet implemented.
            OperandKind::MemDatIndirect => {
                log::warn!(
                    "MemDatIndirect operand not implemented (raw=0x{:08X}), returning 0",
                    operand.raw
                );
                Ok(0)
            }
        };
        if self.vm_trace_enabled() {
            if let Ok(v) = result {
                self.vm_trace(format_args!("  eval_operand_result {operand} => {v}"));
            }
        }
        result
    }

    fn write_operand(&mut self, operand: Operand, value: i32) -> Result<(), RuntimeError> {
        if self.vm_trace_enabled() {
            self.vm_trace(format_args!("  write_operand {operand} = {value}"));
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
                let signed_idx = self.read_var(operand.lo as usize)?;
                let Some(idx) =
                    checked_script_mem_index("user_mem", signed_idx, self.user_mem.len())
                else {
                    return Ok(());
                };
                self.user_mem[idx] = value;
                Ok(())
            }
            // kind 0x2: system_mem[vars[lo]] = value
            OperandKind::SystemMemoryViaVar => {
                let signed_idx = self.read_var(operand.lo as usize)?;
                let Some(idx) =
                    checked_script_mem_index("system_mem", signed_idx, self.system_mem.len())
                else {
                    return Ok(());
                };
                self.system_mem[idx] = value;
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
                if signed_idx < 0 {
                    log::debug!(
                        "TempMemoryViaVar write out of range: idx={} bank={} var={}",
                        signed_idx,
                        bank,
                        var_val
                    );
                    return Ok(());
                }
                let idx = signed_idx as usize;
                if idx >= self.temp_mem.len() {
                    self.temp_mem.resize(idx + 1, 0);
                }
                self.temp_mem[idx] = value;
                Ok(())
            }
            // kind 0x6: MemDatDirect — write to the writable shadow copy.
            OperandKind::MemDatDirect => {
                let word_index = self.mem_dat_word_index(operand)?;
                if word_index >= self.mem_dat_words.len() {
                    self.mem_dat_words.resize(word_index + 1, 0);
                }
                self.mem_dat_words[word_index] = value;
                Ok(())
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

    fn write_extcall_dst(&mut self, dst_slot_raw: u32, value: i32) -> Result<(), RuntimeError> {
        self.write_var(dst_slot_raw as usize, value)
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

    fn read_mem_dat_i32(&self, operand: Operand) -> Result<i32, RuntimeError> {
        let word_index = self.mem_dat_word_index(operand)?;
        Ok(self.mem_dat_words.get(word_index).copied().unwrap_or(0))
    }

    fn write_mem_dat_word(&mut self, word_index: usize, value: i32) {
        if word_index >= self.mem_dat_words.len() {
            self.mem_dat_words.resize(word_index + 1, 0);
        }
        self.mem_dat_words[word_index] = value;
    }

    fn mem_dat_word_index(&self, operand: Operand) -> Result<usize, RuntimeError> {
        // Original formula: mem_dat_ptr + 4*(bank + vars[lo]) + 16.
        // The "+16" skips the 16-byte header present in Mem.dat. Scripts also
        // use this area as mutable work storage, so writes can extend the shadow.
        let bank = operand.bank as i32;
        let var_val = self.read_var(operand.lo as usize)?;
        let signed_word_index = bank.wrapping_add(var_val).wrapping_add(4);
        if signed_word_index < 0 {
            return Err(RuntimeError::MemDatOutOfRange {
                offset: 0,
                len: self.mem_dat_words.len() * 4,
            });
        }
        Ok(signed_word_index as usize)
    }

    fn push_stack(&mut self, value: i32, pc: u32) -> Result<(), RuntimeError> {
        if self.stack.len() >= DEFAULT_STACK_LIMIT {
            return Err(RuntimeError::StackOverflow {
                pc,
                limit: DEFAULT_STACK_LIMIT,
            });
        }
        self.stack.push(value);
        self.vm_trace(format_args!(
            "  stack_push value={value} stack_len={}",
            self.stack.len()
        ));
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
        self.vm_trace(format_args!(
            "  pack_args count={count} values={packed:?} stack_len={} arg_len_before={}",
            self.stack.len(),
            self.argument_stack.len()
        ));
        self.argument_stack.append(&mut packed);
        self.vm_trace(format_args!(
            "  pack_args_done arg_len={}",
            self.argument_stack.len()
        ));
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
        let dropped = self.argument_stack[new_len..].to_vec();
        self.argument_stack.truncate(new_len);
        self.vm_trace(format_args!(
            "  drop_args count={count} dropped={dropped:?} arg_len={new_len}"
        ));
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
        input: Option<&PalInputState>,
    ) -> ExtCallOutcome {
        let nls = resource_manager
            .as_ref()
            .map(|manager| manager.nls())
            .unwrap_or(Nls::ShiftJis);

        if category == 13 {
            return self.dispatch_voice_ext(index, assets, nls, resource_manager, audio);
        }

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

        let outcome = match category {
            3 => {
                self.dispatch_sprite_ext(index, assets, nls, resource_manager, sprites, task_system)
            }
            4 => self.dispatch_bgm_ext(index, assets, nls, resource_manager, audio),
            5 => self.dispatch_se_ext(index, assets, nls, resource_manager, audio),
            18 => match index {
                3 => self.ext_arg_probe(2, 1),
                5 => self.ext_arg_probe(3, 1),
                6 => self.ext_arg_get(),
                9 => self.ext_wsprint_compat(),
                15 => self.ext_arg_probe(0, 1),
                21 => self.ext_string_non_empty(assets, nls),
                28 => self.ext_arg_probe(1, 1),
                29 => self.ext_arg_probe(0, 1),
                30 => self.ext_open_file(assets, nls, resource_manager),
                31 => self.ext_read_file(),
                32 => self.ext_close_file_not_handle(),
                33 => self.ext_set_file_pointer(),
                34 => self.ext_file_string(),
                35 => self.ext_arg_probe(1, 1),
                36 => self.ext_sz_buf(assets, nls, resource_manager),
                37 => self.ext_get_private_profile_int(assets, nls, resource_manager),
                _ => ExtCallOutcome::Skip,
            },
            7 => self.dispatch_wait_ext(index),
            8 => self.dispatch_button_ext(index, assets, nls, resource_manager, sprites, input),
            9 => self.dispatch_font_system_stub(index),
            10 => self.dispatch_save_stub(index),
            12 => self.dispatch_system_button_stub(index),
            14 => self.dispatch_history_stub(index),
            2 => self.dispatch_text_stub(index),
            6 => self.dispatch_select_stub(index),
            15 => self.dispatch_misc_system_stub(index),
            16 => self.dispatch_window_effect_stub(index),
            21 => self.dispatch_thread_stub(index),
            22 => self.dispatch_run_ext(index),
            17 => self.dispatch_action_stub(index, sprites),
            20 => self.dispatch_random_ext(index),
            23 => self.dispatch_message_stub(index, point_table),
            _ => ExtCallOutcome::Skip,
        };

        if matches!(outcome, ExtCallOutcome::Skip) {
            if let Some(sig) = lookup_sig(category, index) {
                let args = self.pop_ext_args(sig.pop_count);
                self.vm_trace(format_args!(
                    "  extcall shared-signature fallback name={} argc={} args={args:?} -> 0",
                    sig.name, sig.pop_count
                ));
                return ExtCallOutcome::Value(0);
            }
        }

        outcome
    }

    fn dispatch_wait_ext(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            // wait(duration_ms,skip_cancel): Game.exe sub_444F40 pops two
            // values, records start time, and rewinds the PC until the duration
            // expires or the native cancel path completes.
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
            // wait_click(duration_ms): sub_444DE0 treats non-positive durations
            // as click/key-only waits. Positive values are click-or-timeout waits.
            // Title/menu scripts pass 0 for a persistent input gate; clamping it to
            // 1ms makes the title immediately fall through into scene startup.
            1 => {
                let args = self.pop_ext_args(1);
                let duration_ms = args.first().copied().unwrap_or(-1);
                log::debug!("[trace-script] wait_click duration_ms={duration_ms}");
                if duration_ms <= 0 {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Click,
                    }
                } else {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::ClickOrTime(duration_ms as u32),
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
            5 => {
                self.pop_ext_args(0);
                self.wait_sync_begin_ms = self.pal_time_ms;
                self.wait_sync_release = None;
                log::debug!("[trace-script] wait_sync_step");
                ExtCallOutcome::Wait {
                    value: 1,
                    request: WaitRequest::Frame(1),
                }
            }
            6 => {
                self.pop_ext_args(0);
                self.wait_sync_begin_ms = 0;
                self.wait_sync_release = None;
                self.wait_time_stack.clear();
                log::debug!("[trace-script] wait_clear");
                ExtCallOutcome::Value(1)
            }
            // wait_click_no_anim(duration_ms): sub_444C90 shares wait_click
            // blocking semantics while bypassing wait-icon animation state.
            7 => {
                let args = self.pop_ext_args(1);
                let duration_ms = args.first().copied().unwrap_or(1);
                log::debug!("[trace-script] wait_click_no_anim duration_ms={duration_ms}");
                if duration_ms <= 0 {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Click,
                    }
                } else {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::ClickOrTime(duration_ms as u32),
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
            17 => {
                let args = self.pop_ext_args(1);
                let old = i32::from(self.font_state.font_size());
                if let Some(size) = args.first().copied() {
                    self.font_state.set_font_size(size.max(1) as u16);
                }
                ExtCallOutcome::Value(old)
            }
            9 => {
                // Game script 0002D3DC calls category 9 index 9 as a zero-arg
                // query and writes the result to dst_slot[1].  The previous
                // auto fallback popped one stale value every settings-frame,
                // corrupting submenu stacks before button polling could run.
                self.pop_ext_args(0);
                ExtCallOutcome::Value(0)
            }
            4 | 6 | 7 | 8 | 10 | 21 | 22 | 27 => {
                // IDB direct dump maps these reachable zero-argument system/font
                // helpers to Game.exe handlers:
                //   9:4  VmOpc_Op112       @ 0x004389F0
                //   9:6  VmOpc_Op114       @ 0x00438950
                //   9:7  VmOpc_Op115       @ 0x004388F0
                //   9:8  VmOpc_Op116       @ 0x00438890
                //   9:10 VmOpc_Op118       @ 0x00438830
                //   9:21 VmOpc_Op129       @ 0x00437E10
                //   9:22 reachable UI transition cleanup hook used after
                //        system/save/load/sound page teardown
                // The title/settings/save scripts use them as polling/status
                // hooks.  The shared ExtSig report confirms the reachable
                // callsites are zero-argument; popping a stale value here breaks
                // button callbacks after the first click.
                // Until their PAL-side UI side effects are fully named, the
                // portable runtime preserves stack/writeback behavior and keeps
                // the path out of the shared-signature fallback.
                self.pop_ext_args(0);
                ExtCallOutcome::Value(0)
            }
            23 => {
                // VmOpc_Op131 @ 0x00437CD0.  The common SAVE/LOAD/SYSTEM/SOUND
                // tab callback first writes the selected page to Mem.dat[158],
                // then calls 9:24(mode) and 9:23(point=2137).  Native behavior
                // resumes the menu dispatcher on a later VM turn; doing the same
                // here lets the script's own dispatcher create LOAD_BASE,
                // SAVE_BASE, SYS_BASE, or SOUND_BASE instead of overlaying the
                // new tab controls on the old title scene.
                let args = self.pop_ext_args(1);
                let point_id = args.first().copied().unwrap_or(0);
                if point_id > 0 {
                    self.pending_jump_point = Some(point_id as u32);
                    log::debug!(
                        "[trace-system] scheduled menu continuation point[{point_id}] mode={}",
                        self.menu_transition_mode
                    );
                }
                ExtCallOutcome::Value(0)
            }
            24 => {
                // sub_437C00 @ 0x00437C00.  docs/dis.txt 0003A324 pushes one
                // transition mode before this call.  Keep the value so the next
                // 9:23 continuation can be traced with the same state the native
                // handler receives.
                let args = self.pop_ext_args(1);
                self.menu_transition_mode = args.first().copied().unwrap_or(0);
                log::debug!(
                    "[trace-system] menu transition mode={}",
                    self.menu_transition_mode
                );
                ExtCallOutcome::Value(0)
            }
            5 => {
                // VmOpc_AutoGetTime @ 0x004389B0. Native code returns the
                // configured auto-advance interval; the current compatible
                // state stores no separate auto timer yet, so expose the
                // conservative disabled value while preserving the real zero-arg
                // stack discipline.
                self.pop_ext_args(0);
                ExtCallOutcome::Value(0)
            }
            18 => {
                let args = self.pop_ext_args(1);
                let old = i32::from(self.font_state.font_type());
                if let Some(font_type) = args.first().copied() {
                    self.font_state.set_type(font_type.max(0) as u16);
                }
                ExtCallOutcome::Value(old)
            }
            11 | 12 | 14 | 15 => {
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
                ExtCallOutcome::Value(self.pal_time_ms as i32)
            }
            36 => {
                let args = self.pop_ext_args(2);
                if args.len() < 2 {
                    return ExtCallOutcome::Block;
                }
                self.system_state.set_window_size(args[0], args[1]);
                ExtCallOutcome::Value(1)
            }
            37 => {
                let args = self.pop_ext_args(1);
                let mode = args.first().copied().unwrap_or(0);
                self.system_state.change_aspect_mode(mode);
                ExtCallOutcome::Value(1)
            }
            38 => {
                let args = self.pop_ext_args(1);
                self.system_state
                    .set_aspect_position_enabled(args.first().copied().unwrap_or(0));
                ExtCallOutcome::Value(1)
            }
            39 | 40 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.system_state.aspect_mode())
            }
            41 => {
                self.pop_ext_args(2);
                ExtCallOutcome::Value(1)
            }
            42 | 43 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(0)
            }
            44 => {
                self.pop_ext_args(1);
                self.system_state.clear_system_paths();
                ExtCallOutcome::Value(1)
            }
            45 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            46 => {
                let args = self.pop_ext_args(1);
                self.system_state
                    .set_window_change_enabled(args.first().copied().unwrap_or(1));
                ExtCallOutcome::Value(1)
            }
            47 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.system_state.window_change_enabled())
            }
            48 => {
                self.pop_ext_args(1);
                self.system_state.set_cursor_null(true);
                ExtCallOutcome::Value(1)
            }
            49 => {
                let args = self.pop_ext_args(1);
                self.system_state
                    .set_hide_cursor_time(args.first().copied().unwrap_or(0));
                ExtCallOutcome::Value(1)
            }
            50 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.system_state.hide_cursor_time())
            }
            51 | 52 | 57 | 60 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            55 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.font_state.color().0 as i32)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    /// Game category 2 text/message-window extcalls.
    ///
    /// Evidence: shared `ExtSig` entries plus Game.sqlite text handler traces show
    /// these calls pop their parameters from the script argument stack and write a
    /// small integer status/result to the extcall destination.  PAL.dll text
    /// rendering is not a single export here; the VM mutates `TextSubsystemState`,
    /// and the renderer consumes that state later when building text/name sprites.
    /// Remaining gap: exact native glyph-reveal timing and icon animation curves
    /// are modeled, not byte-for-byte verified.
    fn dispatch_text_stub(&mut self, index: u16) -> ExtCallOutcome {
        match index {
            0 => {
                let args = self.pop_ext_args(8);
                let ordered = ext_args_source_order::<8>(&args);
                self.text_state.initialized = true;
                self.text_state.visible = true;
                self.text_state.mode = ordered[7];
                self.text_state.init_args = ordered;
                self.text_state.last_event_time_ms = self.pal_time_ms;
                self.text_state.dirty = true;
                log::debug!("[trace-text] text_init args={ordered:?}");
                ExtCallOutcome::Value(1)
            }
            1 => {
                let args = self.pop_ext_args(4);
                let ordered = ext_args_source_order::<4>(&args);
                self.text_state.icon = ordered[3];
                log::debug!("[trace-text] text_set_icon args={ordered:?}");
                ExtCallOutcome::Value(1)
            }
            2 => {
                let args = self.pop_ext_args(4);
                let ordered = ext_args_source_order::<4>(&args);
                self.text_state.last_text_args = ordered;
                self.text_state.last_text_value = ordered[1];
                self.text_state.last_event_time_ms = self.pal_time_ms;
                self.text_state.reveal_start_ms = self.pal_time_ms;
                self.text_state.reveal_duration_ms = 0;
                self.text_state.reveal_enabled = false;
                self.text_state.visible = true;
                self.text_state.dirty = true;
                log::debug!("[trace-text] text args={ordered:?}");
                ExtCallOutcome::Value(1)
            }
            3 => {
                self.pop_ext_args(0);
                self.text_state.visible = false;
                self.text_state.dirty = true;
                log::debug!("[trace-text] text_hide");
                ExtCallOutcome::Value(1)
            }
            4 => {
                self.pop_ext_args(1);
                self.text_state.visible = true;
                self.text_state.last_event_time_ms = self.pal_time_ms;
                self.text_state.dirty = true;
                ExtCallOutcome::Value(1)
            }
            5 => {
                let args = self.pop_ext_args(1);
                self.text_state.button = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            6 => {
                self.pop_ext_args(0);
                let sprite = self.text_state.sprite;
                let name_sprite = self.text_state.name_sprite;
                self.text_state = TextSubsystemState {
                    sprite,
                    name_sprite,
                    dirty: true,
                    ..TextSubsystemState::default()
                };
                log::debug!("[trace-text] text_uninit");
                ExtCallOutcome::Value(1)
            }
            8 => {
                self.pop_ext_args(0);
                self.text_state.last_text_value = 0;
                self.text_state.last_text_args = [0; 4];
                self.text_state.reveal_enabled = false;
                self.text_state.last_event_time_ms = self.pal_time_ms;
                self.text_state.dirty = true;
                log::debug!("[trace-text] text_clear");
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
                self.text_state.dirty = true;
                log::debug!(
                    "[trace-text] text_window_set_alpha args={args:?} alpha={}",
                    self.text_state.alpha
                );
                ExtCallOutcome::Value(1)
            }
            12 => {
                let args = self.pop_ext_args(1);
                self.text_state.last_text_value = args.first().copied().unwrap_or(0);
                ExtCallOutcome::Value(1)
            }
            14 => {
                let args = self.pop_ext_args(2);
                let ordered = ext_args_source_order::<2>(&args);
                self.text_state.icon = ordered[1];
                log::debug!("[trace-text] text_set_icon_animation_time args={ordered:?}");
                ExtCallOutcome::Value(1)
            }
            15 | 16 | 17 => {
                let args = self.pop_ext_args(4);
                let ordered = ext_args_source_order::<4>(&args);
                self.text_state.last_text_args = ordered;
                self.text_state.last_text_value = ordered[1];
                self.text_state.last_event_time_ms = self.pal_time_ms;
                self.text_state.reveal_start_ms = self.pal_time_ms;
                self.text_state.reveal_duration_ms = if index == 16 {
                    0
                } else {
                    self.text_reveal_duration_ms(ordered[1])
                };
                self.text_state.reveal_enabled = self.text_state.reveal_duration_ms > 0;
                self.text_state.visible = true;
                self.text_state.dirty = true;
                log::debug!(
                    "[trace-text] text_wait index={index} args={ordered:?} reveal_ms={}",
                    self.text_state.reveal_duration_ms
                );
                ExtCallOutcome::Value(1)
            }
            18 | 19 => {
                self.pop_ext_args(0);
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
                let args = self.pop_ext_args(4);
                let ordered = ext_args_source_order::<4>(&args);
                self.text_state.base = ordered[3];
                log::debug!("[trace-text] text_set_base args={ordered:?}");
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
            26 => {
                // VmExtcall_TextTaskFree @ 0x0043E550. The native handler frees
                // pending text animation/task state. In this renderer the text
                // reveal task is represented by flags in TextSubsystemState, so
                // clearing those flags matches the externally visible effect.
                self.pop_ext_args(0);
                self.text_state.reveal_enabled = false;
                self.text_state.dirty = true;
                ExtCallOutcome::Value(1)
            }
            28 => {
                let args = self.pop_ext_args(1);
                self.text_state.last_text_value = args.first().copied().unwrap_or(0);
                self.text_state.reveal_enabled = false;
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
                let args = self.pop_ext_args(4);
                let ordered = ext_args_source_order::<4>(&args);
                self.text_state.initialized = true;
                self.text_state.visible = true;
                self.text_state.rect = [ordered[0], ordered[1], ordered[2], ordered[3]];
                self.text_state.dirty = true;
                log::debug!("[trace-text] text_init_visualnovelmode args={ordered:?}");
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
                self.pop_ext_args(4);
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

    /// Game category 6 select/menu extcalls.
    ///
    /// Parameters are stored in `SelectSubsystemState` in the same pop order as
    /// the semantic ExtSig table.  Query calls return the current selection index
    /// or `-1` when locked/no selection, matching the observed Game handler
    /// status style.  No PAL.dll export is assigned: this is Game.exe UI state.
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
                self.select_state.last_key = -1;
                ExtCallOutcome::Value(-1)
            }
            2 => {
                let args = self.pop_ext_args(6);
                if args.len() < 6 {
                    return ExtCallOutcome::Block;
                }
                self.select_state.options.push(SelectOption {
                    text_value: args[0],
                    target_value: args[1],
                    x: args[2],
                    y: args[3],
                    color: args[4],
                    flags: args[5],
                });
                log::debug!(
                    "[trace-select] select_option text={} target={} pos=({}, {}) color={} flags={}",
                    args[0],
                    args[1],
                    args[2],
                    args[3],
                    args[4],
                    args[5]
                );
                ExtCallOutcome::Value(1)
            }
            3 => {
                self.pop_ext_args(0);
                log::debug!(
                    "[trace-select] select_commit options={}",
                    self.select_state.options.len()
                );
                ExtCallOutcome::Value(1)
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

    /// Game category 8 button extcalls backed by PAL button/sprite APIs.
    ///
    /// Creation and visual state calls ultimately map to PAL concepts such as
    /// `PalButtonCreate`, `PalButtonCtrl`, `PalButtonGetReaction`, and sprite
    /// rect/visibility updates.  Return value is `1` for successful mutation and
    /// integer button indices for reaction queries.  Remaining gap: some high
    /// index button helpers are stack-safe state shims until their exact native
    /// PAL export sequence is named.
    fn dispatch_button_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        sprites: Option<&mut SpriteSystem>,
        input: Option<&PalInputState>,
    ) -> ExtCallOutcome {
        match index {
            1 => return self.ext_btn_uninit(sprites),
            3 => return self.ext_btn_set(assets, nls, resource_manager, sprites),
            4 => return self.ext_btn_view_ctrl(false, sprites),
            8 => return self.ext_btn_release(index, sprites),
            5 => return self.ext_btn_view_ctrl(true, sprites),
            6 => return self.ext_btn_set_pos(sprites),
            9 => return self.ext_btn_slider_get(input, sprites.as_deref()),
            10 => return self.ext_btn_slider_set(sprites),
            11 => return self.ext_btn_slider_begin(),
            12 => return self.ext_btn_on_check(input, sprites.as_deref()),
            13 => return self.ext_btn_set_toggle(sprites),
            14 => return self.ext_btn_set_state(sprites),
            15 => return self.ext_btn_enable(sprites),
            16 => return self.ext_btn_set_alpha(sprites),
            17 => return self.ext_btn_get_push(input, sprites.as_deref()),
            18 => return self.ext_btn_expansion(sprites),
            19 => return self.ext_btn_lock(),
            20 => return self.ext_btn_unlock(),
            21 => return self.ext_btn_set_anim(sprites),
            22 => return self.ext_btn_set_hit(),
            _ => {}
        }
        let arity = match index {
            0 => 3,
            13 => 2,
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

    fn dispatch_window_effect_stub(&mut self, index: u16) -> ExtCallOutcome {
        let arity = match index {
            1 | 2 => 4,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(1)
    }

    /// Game category 10 save/load extcalls.
    ///
    /// These calls are Game.exe save-UI state operations rather than direct
    /// filesystem writes in the current runtime.  They pop the script arity from
    /// shared ExtSig/handler evidence, update `SaveSubsystemState`, and return
    /// PAL-style integer success/query values.  Remaining gap: actual save-file
    /// serialization and thumbnail pixel capture are not yet native-equivalent.
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
        // Game category 12 system buttons are Game.exe-managed window/menu
        // controls, not PAL.dll exports.  sub_439270 pops
        // system_btn_set(index,image,state); sub_439100 pops
        // system_btn_enable(index,enabled) and supports index 0xFFFF as an
        // all-slot wildcard.  The portable runtime records stack/return
        // semantics here while platform window drawing stays outside PAL.
        let arity = match index {
            0 => 3,
            1 => 1,
            2 => 2,
            _ => return ExtCallOutcome::Skip,
        };
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(1)
    }

    /// Game category 14 history/backlog extcalls.
    ///
    /// The original handlers mutate backlog layout, active state, records, and
    /// query height/text data.  This implementation records enough state for
    /// script control flow and UI sizing, returning integer status or record
    /// counts.  Remaining gap: exact native text wrapping and scroll rendering.
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
                let (logical_width, logical_height) = self.logical_size();
                self.history_state.rect = [0, 0, logical_width as i32, logical_height as i32];
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

    /// Game category 21 thread extcalls.
    ///
    /// These are modeled as Game-side lightweight script thread identifiers:
    /// create returns a new id, exit marks the last id inactive, and get returns
    /// the last id.  No PAL.dll export is known for this path.
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

    /// Game category 22 run/transition extcalls.
    ///
    /// `run` consumes effect id/duration/mode and drives the PAL-compatible
    /// effect pipeline; `run_no_wait` is a zero-argument latch, and `run_stack`
    /// queues/flushed stacked transition state.  Return value is integer success;
    /// blocking runs emit a wait request.  Evidence comes from Game.sqlite run
    /// handlers, PAL effect exports, and runtime trace compare.
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
                let wait = arg2 != 0;
                let _ = self.effect_system.effect_ex(
                    effect_id.max(0) as u32,
                    arg1.max(1) as u32,
                    arg2,
                    wait,
                    self.pal_time_ms,
                );
                log::debug!(
                    "[trace-run] run effect={effect_id} arg1={arg1} arg2={arg2} wait={wait}"
                );
                if wait && effect_id != 0 && arg1 > 0 {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Time(arg1 as u32),
                    }
                } else {
                    ExtCallOutcome::Value(1)
                }
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
        match index {
            2 => {
                self.pop_ext_args(1);
            }
            4 => {
                self.pop_ext_args(0);
            }
            5 => {
                let args = self.pop_ext_args(1);
                let slot = args.first().copied().unwrap_or(0);
                if slot != 0 {
                    self.misc_system_pending_complete = true;
                } else {
                    self.misc_system_pending_complete = false;
                }
            }
            _ => return ExtCallOutcome::Skip,
        }
        ExtCallOutcome::Value(1)
    }

    /// Game category 17 action/tween scheduler extcalls.
    ///
    /// These handlers pop action ids, durations, and tween parameters from the
    /// VM argument stack, mutate `ActionSubsystemState`, and return integer
    /// status/query values.  The simple timer calls only update scheduler
    /// state; index 8 also installs a sprite alpha-delta action because the
    /// Game/PAL action bytecode path (`sub_446650` case 3 -> `sub_402F30`)
    /// writes a timed delta into the sprite color accumulator.
    fn dispatch_action_stub(
        &mut self,
        index: u16,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        match index {
            // action_run_count_over(action_id,duration_ms): Game.exe sub_40B6E0
            // pops action id first, duration second, schedules the action, and
            // returns status 1 without blocking.  It does not return an elapsed
            // flag; separate query extcalls poll completion.
            0 => {
                let args = self.pop_ext_args(2);
                let action_id = args.first().copied().unwrap_or(-1);
                let duration = args.get(1).copied().filter(|value| *value > 0).unwrap_or(0) as u32;
                if action_id >= 0 {
                    self.action_state.set_active(action_id);
                }
                self.action_state.schedule(self.pal_time_ms, duration);
                ExtCallOutcome::Value(1)
            }
            // action_sync_run_count_over(action_id,duration_ms): sub_40B5C0 is
            // the synchronous variant.  It schedules the same action state and
            // sets the native blocking flag; the compatibility VM represents
            // that with WaitRequest::Time.
            1 => {
                let args = self.pop_ext_args(2);
                let action_id = args.first().copied().unwrap_or(-1);
                let duration = args.get(1).copied().filter(|value| *value > 0).unwrap_or(0) as u32;
                if action_id >= 0 {
                    self.action_state.set_active(action_id);
                }
                self.action_state.schedule(self.pal_time_ms, duration);
                if duration == 0 {
                    ExtCallOutcome::Value(1)
                } else {
                    ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Time(duration),
                    }
                }
            }
            3 => {
                let args = self.pop_ext_args(1);
                if args.first().copied().unwrap_or(-1) == -1 {
                    self.action_state.clear();
                }
                ExtCallOutcome::Value(1)
            }
            5 | 6 | 10 | 14 | 15 | 20 | 29 => {
                let arity = match index {
                    20 => 5,
                    _ => 6,
                };
                let args = self.pop_ext_args(arity);
                let duration = action_duration_from_args(&args);
                self.action_state.schedule(self.pal_time_ms, duration);
                if let Some(action_id) = args.first().copied() {
                    self.action_state.set_active(action_id);
                }
                ExtCallOutcome::Value(1)
            }
            7 | 11 => {
                let args = self.pop_ext_args(4);
                let duration = action_duration_from_args(&args);
                self.action_state.schedule(self.pal_time_ms, duration);
                if let Some(action_id) = args.first().copied() {
                    self.action_state.set_active(action_id);
                }
                ExtCallOutcome::Value(1)
            }
            8 => {
                let args = self.pop_ext_args(4);
                let action_id = args.first().copied().unwrap_or(-1);
                let sprite_slot = args.get(1).copied().unwrap_or(-1);
                let alpha_delta = args.get(2).copied().unwrap_or(0);
                let duration = args.get(3).copied().filter(|value| *value > 0).unwrap_or(0) as u32;
                if action_id >= 0 {
                    self.action_state.set_active(action_id);
                }
                self.action_state.schedule(self.pal_time_ms, duration);
                if let Some(sprites) = sprites {
                    self.apply_action_alpha_delta(sprites, sprite_slot, alpha_delta, duration);
                }
                ExtCallOutcome::Value(1)
            }
            9 => {
                let args = self.pop_ext_args(2);
                if let Some(action_id) = args.get(1).copied() {
                    self.action_state.set_active(action_id);
                }
                ExtCallOutcome::Value(1)
            }
            // set_active_action(action_id): Game.exe sub_4095C0 validates the
            // id, keeps the previous active id in script state, and installs
            // the new current action.  The portable state only needs current
            // active id for -1 resolution in later action calls.
            23 => {
                let args = self.pop_ext_args(1);
                self.action_state
                    .set_active(args.first().copied().unwrap_or(-1));
                ExtCallOutcome::Value(1)
            }
            24 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(
                    self.action_state
                        .is_over(self.pal_time_ms, self.action_state.last_duration_ms),
                ))
            }
            25 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.action_state.active_id)
            }
            28 => {
                self.pop_ext_args(0);
                self.action_state.clear();
                ExtCallOutcome::Value(1)
            }
            // set_action_clear(action_id): Game.exe sub_40B3A0 pops one id,
            // resolves -1 through the active action id, marks that action's
            // clear flag, and returns 1.  This is a state mutation, not a full
            // action memory wipe; action_clear_count_over handles teardown.
            30 => {
                let args = self.pop_ext_args(1);
                self.action_state
                    .set_clear(args.first().copied().unwrap_or(-1));
                ExtCallOutcome::Value(1)
            }
            _ => ExtCallOutcome::Skip,
        }
    }

    /// `ext_0011_0008(action_id, sprite_slot, alpha_delta, duration_ms)`.
    ///
    /// Evidence: title SYSTEM calls push `1000, -255, 64, 0` before
    /// `ext_0011_0008`; the VM pops this as action 0, sprite slot 64, alpha
    /// delta -255, duration 1000.  Game.sqlite shows the underlying action
    /// bytecode parser case 3 creates `sub_402F30`, which accumulates the
    /// timed value into color lane offset +244 while running and +420 when
    /// complete; `sub_4494D0`/`sub_4498D0` clamps that lane into
    /// `PalSpriteSetColor`.  The portable renderer maps that to a tween from
    /// the current alpha to `current + delta`.
    fn apply_action_alpha_delta(
        &mut self,
        sprites: &mut SpriteSystem,
        sprite_slot: i32,
        alpha_delta: i32,
        duration_ms: u32,
    ) {
        let Some(handle) = self.game_sprites.get(&sprite_slot).copied() else {
            if sprite_slot == 255 {
                self.apply_text_layer_alpha_delta(sprites, alpha_delta, duration_ms);
                return;
            }
            if (0..10_000).contains(&sprite_slot) {
                self.queue_pending_alpha_action(sprite_slot, alpha_delta, duration_ms);
                return;
            }
            log::debug!(
                "[trace-action] action_alpha_delta missing sprite_slot={sprite_slot} delta={alpha_delta} duration_ms={duration_ms}"
            );
            return;
        };
        let Some(sprite) = sprites.get(handle) else {
            return;
        };
        let current = sprite.color.alpha() as i32;
        let target = current.saturating_add(alpha_delta).clamp(0, 255) as u8;
        if duration_ms > 0 {
            sprites.tween_alpha_to(handle, target, duration_ms);
        } else {
            sprites.set_alpha(handle, target);
        }
        log::debug!(
            "[trace-action] action_alpha_delta slot={sprite_slot} handle={:?} current={current} delta={alpha_delta} target={target} duration_ms={duration_ms}",
            handle
        );
    }

    fn queue_pending_alpha_action(&mut self, slot: i32, alpha_delta: i32, duration_ms: u32) {
        self.game_sprite_pending_alpha
            .entry(slot)
            .or_default()
            .push(PendingAlphaAction {
                alpha_delta,
                duration_ms,
                started_ms: self.pal_time_ms,
            });
        log::debug!(
            "[trace-action] action_alpha_delta queued sprite_slot={slot} delta={alpha_delta} duration_ms={duration_ms}"
        );
    }

    fn apply_pending_alpha_actions(
        &mut self,
        sprites: &mut SpriteSystem,
        slot: i32,
        handle: SpriteHandle,
    ) {
        let Some(actions) = self.game_sprite_pending_alpha.remove(&slot) else {
            return;
        };
        for action in actions {
            let Some(sprite) = sprites.get(handle) else {
                continue;
            };
            let current = sprite.color.alpha() as i32;
            let target = current.saturating_add(action.alpha_delta).clamp(0, 255) as u8;
            let elapsed = self.pal_time_ms.wrapping_sub(action.started_ms);
            let remaining = action.duration_ms.saturating_sub(elapsed);
            if remaining > 0 {
                sprites.tween_alpha_to(handle, target, remaining);
            } else {
                sprites.set_alpha(handle, target);
            }
            log::debug!(
                "[trace-action] action_alpha_delta applied pending slot={slot} handle={:?} delta={} target={target} remaining_ms={remaining}",
                handle,
                action.alpha_delta
            );
        }
    }

    /// PAL script wrappers use sprite slot 255 for the ADV text render layer.
    ///
    /// Evidence: the main scenario `text_w` path reaches point[417]/point[959]
    /// and then calls category 17 index 8 with `sprite_slot=255` immediately
    /// before `wait_click`/`wait`.  Native PAL routes that through its internal
    /// text render object rather than the public Game sprite table, so looking
    /// only in `game_sprites` drops text-window fade/typewriter animation.
    fn apply_text_layer_alpha_delta(
        &mut self,
        sprites: &mut SpriteSystem,
        alpha_delta: i32,
        duration_ms: u32,
    ) {
        let mut applied = false;
        for handle in [self.text_state.sprite, self.text_state.name_sprite]
            .into_iter()
            .flatten()
        {
            if let Some(sprite) = sprites.get(handle) {
                let current = sprite.color.alpha() as i32;
                let target = current.saturating_add(alpha_delta).clamp(0, 255) as u8;
                if duration_ms > 0 {
                    sprites.tween_alpha_to(handle, target, duration_ms);
                } else {
                    sprites.set_alpha(handle, target);
                }
                applied = true;
            }
        }

        if !applied {
            self.text_state.pending_alpha.push(PendingAlphaAction {
                alpha_delta,
                duration_ms,
                started_ms: self.pal_time_ms,
            });
            self.text_state.dirty = true;
        }
        log::debug!(
            "[trace-action] action_alpha_delta text_layer slot=255 delta={alpha_delta} duration_ms={duration_ms} applied={applied}"
        );
    }

    fn apply_pending_text_alpha_actions(&mut self, sprites: &mut SpriteSystem) {
        if self.text_state.pending_alpha.is_empty() {
            return;
        }
        let handles = [self.text_state.sprite, self.text_state.name_sprite]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        if handles.is_empty() {
            return;
        }
        for action in self.text_state.pending_alpha.drain(..) {
            for handle in handles.iter().copied() {
                let Some(sprite) = sprites.get(handle) else {
                    continue;
                };
                let current = sprite.color.alpha() as i32;
                let target = current.saturating_add(action.alpha_delta).clamp(0, 255) as u8;
                let elapsed = self.pal_time_ms.wrapping_sub(action.started_ms);
                let remaining = action.duration_ms.saturating_sub(elapsed);
                if remaining > 0 {
                    sprites.tween_alpha_to(handle, target, remaining);
                } else {
                    sprites.set_alpha(handle, target);
                }
                log::debug!(
                    "[trace-action] action_alpha_delta applied pending text_layer handle={:?} delta={} target={target} remaining_ms={remaining}",
                    handle,
                    action.alpha_delta
                );
            }
        }
    }

    /// Game category 23 message-queue extcalls.
    ///
    /// `create_message` creates a small Game-side message record, `get_message`
    /// consumes the first active message id, and `get_message_param` returns the
    /// resolved point/parameter value.  These are Game VM scheduling helpers; no
    /// PAL.dll export is assigned.
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
                // face_init(name, slot, x, y, z) is the face-layer spelling of
                // sp_set_ex in the Game handler. It loads the portrait base sprite
                // into the same VM sprite table so later face_set/sp animation calls
                // can mutate the already-created node.
                self.ext_sp_set(5, assets, nls, resource_manager, sprites, task_system)
            }
            20 => {
                // face_set uses the same five-value payload as face_init but may be
                // issued after the face slot exists. Reusing sp_set_ex preserves the
                // original replace/load behavior and resource decoding path.
                self.ext_sp_set(5, assets, nls, resource_manager, sprites, task_system)
            }
            21 => {
                self.pop_ext_args(3);
                ExtCallOutcome::Value(0)
            }
            22 => self.ext_sp_text(assets, nls, sprites),
            23 => self.ext_sp_cls(sprites, task_system),
            24 => self.ext_sp_set_rect(sprites),
            25 => self.ext_sp_set_pos_move(sprites),
            28 => self.ext_sp_surface_op(4),
            29 => self.ext_sp_get_dimension(sprites, true),
            30 => self.ext_sp_get_dimension(sprites, false),
            32 => self.ext_sp_create(sprites),
            36 => self.ext_sp_get_scale(sprites),
            37 => self.ext_sp_set_color(sprites),
            38 => self.ext_sp_bitblt(sprites),
            39 => self.ext_sp_set_shake(sprites),
            40 => {
                // sp_paint(): native flushes pending sprite drawing.  The
                // portable renderer builds the scene every frame, so the exact
                // side effect is already represented by keeping the render tree
                // live; this hook exists to keep menu refresh scripts from
                // falling through the shared fallback path.
                self.pop_ext_args(0);
                ExtCallOutcome::Value(1)
            }
            41 => self.ext_sp_set_anim(assets, nls, resource_manager, sprites, task_system),
            44 => self.ext_sp_wait_draw(),
            46 => self.ext_sp_view_ctrl(sprites, true),
            47 => self.ext_sp_view_ctrl(sprites, false),
            50 => self.ext_sp_set_transition(sprites),
            51 => self.ext_sp_copy_image(sprites),
            52 => self.ext_sp_transition(sprites),
            54 => self.ext_sp_copy_image(sprites),
            57 => self.ext_sp_set_anim(assets, nls, resource_manager, sprites, task_system),
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
            sprites.release(old.handle);
        }
        let surface_id = sprites.allocate_surface_id();
        let mut pending_button_animation = None;
        let (mut desc, source_name, log_size) = if let Some((a, r, g, b)) =
            parse_solid_color_name_argb(&name)
        {
            // Native sub_447FE0 accepts names beginning with '#'/BK_* as
            // synthetic solid surfaces.  Save/load menus use these through
            // btn_set for list-cell backgrounds; treating them as PGD files
            // leaves the UI with missing slot panels.
            let width = 288;
            let height = 96;
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            for px in pixels.chunks_exact_mut(4) {
                px.copy_from_slice(&[r, g, b, a]);
            }
            let surface = match SpriteSurface::rgba8(surface_id, 1, width, height, pixels) {
                Ok(surface) => surface,
                Err(err) => {
                    log::warn!(
                            "[trace-button] btn_set group={group} index={index} solid surface failed: {err}"
                        );
                    return ExtCallOutcome::Value(0);
                }
            };
            sprites.insert_surface(surface);
            let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), width, height);
            desc.cell_width = width;
            desc.cell_height = height;
            desc.position = PalVec3::new(0, 0, 0);
            (desc, name.clone(), format!("{width}x{height} solid"))
        } else if is_named_animation_resource(&name) {
            let asset = match open_resource_variant(resource_manager, &name, ANIMATION_EXTENSIONS) {
                Ok(asset) => asset,
                Err(err) => {
                    log::warn!(
                        "[trace-button] btn_set group={group} index={index} name={name:?} animation open failed: {err}"
                    );
                    return ExtCallOutcome::Value(0);
                }
            };
            // Some save/load button entries are pure named animations (for
            // scroll/highlight movement) rather than PGD images.  Native
            // `sub_40F550` resolves these through `sub_448710` before loading
            // the underlying sprite info; the portable path keeps a transparent
            // hit surface and applies the movement/alpha tween parsed from ANI.
            let width = 288;
            let height = 96;
            let pixels = vec![0u8; width as usize * height as usize * 4];
            let surface = match SpriteSurface::rgba8(surface_id, 1, width, height, pixels) {
                Ok(surface) => surface,
                Err(err) => {
                    log::warn!(
                        "[trace-button] btn_set group={group} index={index} animation surface failed: {err}"
                    );
                    return ExtCallOutcome::Value(0);
                }
            };
            sprites.insert_surface(surface);
            pending_button_animation = Some((asset.name.clone(), asset.bytes));
            let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), width, height);
            desc.cell_width = width;
            desc.cell_height = height;
            desc.position = PalVec3::new(0, 0, 0);
            (
                desc,
                asset.name.clone(),
                format!("{width}x{height} named-animation"),
            )
        } else {
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
            let mut desc =
                SpriteDesc::new(SceneTextureId(surface_id.0), decoded.width, decoded.height);
            desc.cell_width = decoded.cell_width;
            desc.cell_height = decoded.cell_height;
            desc.position = PalVec3::new(decoded.offset_x, decoded.offset_y, 0);
            (
                desc,
                asset.name.clone(),
                format!(
                    "{}x{} cell={}x{}",
                    decoded.width, decoded.height, decoded.cell_width, decoded.cell_height
                ),
            )
        };
        desc.visible = entry_flag != 0;
        desc.base_priority = 100;
        desc.source_name = source_name.clone();
        let handle = sprites.create(desc);
        if let Some((asset_name, bytes)) = pending_button_animation {
            if self.apply_named_sprite_animation(sprites, handle, &bytes) {
                log::debug!(
                    "[trace-button] btn_set group={group} index={index} asset={asset_name:?} applied named animation"
                );
            } else {
                log::warn!(
                    "[trace-button] btn_set group={group} index={index} asset={asset_name:?} unsupported named animation"
                );
            }
        }
        // args[3] is the script callback point invoked as a gosub when this button is clicked.
        let gosub_point = args.get(3).copied().filter(|&v| v > 0).map(|v| v as u32);
        self.game_buttons.insert(
            (group, index),
            GameButtonEntry {
                handle,
                name: name.clone(),
                enabled: true,
                locked: false,
                toggle: 0,
                alpha: 255,
                slider_offset: 0,
                hit_rect: None,
                gosub_point,
            },
        );
        log::debug!(
            "[trace-button] btn_set group={group} index={index} name={name:?} asset={source_name:?} size={log_size} entry_flag={entry_flag} gosub_point={gosub_point:?}",
        );
        ExtCallOutcome::Value(1)
    }

    /// `btn_uninit(group)` releases all registered buttons for `group` (`-1`
    /// means all groups), clears queued reactions, and returns `1`.  This is the
    /// runtime counterpart of Game category 8 index 1; native evidence points to
    /// PAL button release/delete plus sprite release side effects.
    fn ext_btn_uninit(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let group = args.first().copied().unwrap_or(-1);
        let keys: Vec<_> = self
            .game_buttons
            .keys()
            .copied()
            .filter(|(button_group, _)| group < 0 || *button_group == group)
            .collect();
        let Some(sprites) = sprites else {
            for key in &keys {
                self.game_buttons.remove(key);
            }
            if group < 0 {
                self.button_push_queue.clear();
            } else {
                self.button_push_queue.remove(&group);
            }
            return ExtCallOutcome::Value(1);
        };
        for key in keys {
            if let Some(entry) = self.game_buttons.remove(&key) {
                sprites.release(entry.handle);
            }
        }
        if group < 0 {
            self.button_push_queue.clear();
        } else {
            self.button_push_queue.remove(&group);
        }
        log::debug!("[trace-button] btn_uninit group={group}");
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
            if let Some(entry) = self.game_buttons.remove(&key) {
                sprites.release(entry.handle);
            }
        }
        if group < 0 {
            self.button_push_queue.clear();
        } else {
            self.button_push_queue.remove(&group);
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

    /// Category 8 index 9 is the native slider-position query used by the
    /// SYSTEM/SOUND menu.  Game.sqlite sub_40EC10 pops
    /// `(group,index,max_offset,axis,snap_to_100)` and returns the live offset,
    /// using the current mouse position when available.  The returned value is
    /// later converted into text-window/audio volume percentages by script.
    fn ext_btn_slider_get(
        &mut self,
        input: Option<&PalInputState>,
        sprites: Option<&SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let max_offset = args[2].max(0);
        let axis = args[3];
        let snap_to_100 = args[4] != 0;
        let mut offset = self
            .game_buttons
            .get(&(group, index))
            .map_or(0, |entry| entry.slider_offset);
        if let (Some(input), Some(sprites), Some(entry)) =
            (input, sprites, self.game_buttons.get(&(group, index)))
        {
            let (mouse_x, mouse_y) = input.mouse_position();
            if mouse_x >= 0 && mouse_y >= 0 {
                if let Some(sprite) = sprites.get(entry.handle) {
                    let anchor = self
                        .slider_anchor_position(sprites, group, index)
                        .unwrap_or_else(|| {
                            let pos = sprite.effective_position();
                            (pos.x, pos.y)
                        });
                    let size = sprite.source_rect;
                    let raw = if axis == 1 {
                        mouse_y - anchor.1 - (size.height() / 2)
                    } else {
                        mouse_x - anchor.0 - (size.width() / 2)
                    };
                    offset = raw.clamp(0, max_offset);
                    if snap_to_100 && max_offset >= 100 && offset != max_offset {
                        let step = (max_offset / 100).max(1);
                        let rem = offset % step;
                        if rem != 0 {
                            offset = (offset + rem).min(max_offset);
                        }
                    }
                }
            }
        }
        if let Some(entry) = self.game_buttons.get_mut(&(group, index)) {
            entry.slider_offset = offset;
        }
        log::debug!(
            "[trace-button] btn_slider_get group={group} index={index} max={max_offset} axis={axis} snap={snap_to_100} -> {offset}"
        );
        ExtCallOutcome::Value(offset)
    }

    /// Category 8 index 10 initializes or updates a slider button position.
    /// Observed scripts pass `(group,index,offset,axis,enabled)` before entering
    /// a poll loop; native code stores the offset and moves the PAL button cell.
    fn ext_btn_slider_set(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let offset = args[2].max(0);
        let axis = args[3];
        let enabled = args[4] != 0;
        for entry in self.matching_button_entries_mut(group, index) {
            entry.slider_offset = offset;
            entry.enabled = enabled;
        }
        if let Some(sprites) = sprites {
            let anchor = self.slider_anchor_position(sprites, group, index);
            for handle in self.matching_button_handles(group, index) {
                if let Some(sprite) = sprites.get_mut(handle) {
                    if axis == 1 {
                        if let Some((_, base_y)) = anchor {
                            sprite.position.y = (base_y + offset) as f32;
                        } else {
                            sprite.position.y = offset as f32;
                        }
                    } else {
                        if let Some((base_x, _)) = anchor {
                            sprite.position.x = (base_x + offset) as f32;
                        } else {
                            sprite.position.x = offset as f32;
                        }
                    }
                }
            }
        }
        log::debug!(
            "[trace-button] btn_slider_set group={group} index={index} offset={offset} axis={axis} enabled={enabled}"
        );
        ExtCallOutcome::Value(1)
    }

    /// Category 8 index 11 starts/arms a slider drag.  Game.sqlite evidence
    /// shows the reachable menu scripts pass only `(group,index)`; returning 1
    /// keeps the script in its slider polling loop without consuming unrelated
    /// stack values.
    fn ext_btn_slider_begin(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let group = args.first().copied().unwrap_or(-1);
        let index = args.get(1).copied().unwrap_or(-1);
        log::debug!("[trace-button] btn_slider_begin group={group} index={index}");
        ExtCallOutcome::Value(1)
    }

    /// Category 8 index 12 checks whether the given button is currently under
    /// the mouse/pushed.  Native sub_410CE0 calls PalButtonGetReaction and
    /// writes a boolean to the extcall destination; the portable path mirrors
    /// that through the existing button hit-test.
    fn ext_btn_on_check(
        &mut self,
        input: Option<&PalInputState>,
        sprites: Option<&SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        if args.len() < 2 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let active = if let (Some(input), Some(sprites)) = (input, sprites) {
            let (mouse_x, mouse_y) = input.mouse_position();
            self.button_hit_at(sprites, mouse_x, mouse_y, group)
                .is_some_and(|(_, hit_index)| index < 0 || hit_index == index)
        } else {
            false
        };
        log::debug!("[trace-button] btn_on_check group={group} index={index} -> {active}");
        ExtCallOutcome::Value(i32::from(active))
    }

    /// `btn_get_push(group)` is the portable counterpart of Game category 8
    /// reaction polling around sub_40CDE0 / PalButtonGetReaction.  Native code
    /// may redirect script/message state when a reaction fires; here we consume
    /// a latched click first, then fall back to current mouse hit testing, and
    /// use `0` as the no-push sentinel observed by title/menu scripts.
    fn ext_btn_get_push(
        &mut self,
        input: Option<&PalInputState>,
        sprites: Option<&SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let group = args.first().copied().unwrap_or(-1);
        let Some(input) = input else {
            log::debug!("[trace-button] btn_get_push group={group} -> 0 no-input");
            return ExtCallOutcome::Value(0);
        };
        if let Some(hit) = self.pop_latched_button_push(group) {
            log::debug!("[trace-button] btn_get_push group={group} -> {hit} latched");
            return ExtCallOutcome::Value(hit);
        }
        if !input.mouse_push(PalMouseButton::Left) {
            log::debug!("[trace-button] btn_get_push group={group} -> 0 no-push");
            return ExtCallOutcome::Value(0);
        }
        let Some(sprites) = sprites else {
            log::debug!("[trace-button] btn_get_push group={group} -> 0 no-sprites");
            return ExtCallOutcome::Value(0);
        };
        let (mouse_x, mouse_y) = input.mouse_position();
        let hit = self
            .button_hit_at(sprites, mouse_x, mouse_y, group)
            .map_or(0, |(_, index)| index);
        log::debug!("[trace-button] btn_get_push group={group} pos=({mouse_x},{mouse_y}) -> {hit}");
        ExtCallOutcome::Value(hit)
    }

    /// `btn_hide(group,index)` / `btn_show(group,index)` correspond to
    /// Game.exe sub_40F290/sub_40F1B0.  The native handlers pop group/index and
    /// enqueue visibility commands; the portable renderer applies visibility
    /// directly to matching sprite handles.
    fn ext_btn_view_ctrl(
        &mut self,
        visible: bool,
        sprites: Option<&mut SpriteSystem>,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let group = args.first().copied().unwrap_or(-1);
        let index = args.get(1).copied().unwrap_or(-1);
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                sprites.view_ctrl(handle, visible);
            }
        }
        log::debug!(
            "[trace-button] btn_{} group={group} index={index}",
            if visible { "show" } else { "hide" }
        );
        ExtCallOutcome::Value(1)
    }

    /// `btn_enable(group,index,enabled)` matches Game.exe sub_40E5B0 /
    /// PalButtonCtrl.  index=-1 applies to all entries in the group; native
    /// state flips a disabled bit and PAL button control state, while the
    /// portable renderer also moves disabled sprites to their disabled cell.
    fn ext_btn_enable(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let enabled = args[2] != 0;
        for entry in self.matching_button_entries_mut(group, index) {
            entry.enabled = enabled;
            if enabled && entry.alpha == 0 {
                entry.alpha = 255;
            }
        }
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                if enabled {
                    if let Some(sprite) = sprites.get_mut(handle) {
                        let raw = sprite.color.0 & 0x00FF_FFFF;
                        let alpha = if sprite.color.0 >> 24 == 0 {
                            255
                        } else {
                            sprite.color.0 >> 24
                        };
                        sprite.color = PalColor::from_argb(raw | (alpha << 24));
                    }
                } else {
                    sprites.rect_set_pos(handle, 0, 3);
                }
            }
        }
        log::debug!("[trace-button] btn_enable group={group} index={index} enabled={enabled}");
        ExtCallOutcome::Value(1)
    }

    /// `btn_lock(group,duration_ms)` matches Game.exe sub_40E0C0.  Native state
    /// is group-scoped: it stores a lock flag, duration/timer value, and start
    /// time, with `duration_ms == 0` storing a zero start time.  That zero case
    /// is used around menu redraws and must not become a permanent per-entry
    /// disable in the portable button table; otherwise SYSTEM/SOUND/LOAD tabs
    /// accept one click and then stop reacting.
    fn ext_btn_lock(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let group = args.first().copied().unwrap_or(-1);
        let duration_ms = args.get(1).copied().unwrap_or(0);
        if duration_ms > 0 {
            for entry in self.matching_button_entries_mut(group, -1) {
                entry.locked = true;
            }
        }
        log::debug!("[trace-button] btn_lock group={group} duration_ms={duration_ms}");
        ExtCallOutcome::Value(1)
    }

    /// `btn_unlock(group)` matches Game.exe sub_40E040 and clears native
    /// group-level lock fields.  The compatibility runtime unlocks every entry
    /// in the group.
    fn ext_btn_unlock(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let group = args.first().copied().unwrap_or(-1);
        for entry in self.matching_button_entries_mut(group, -1) {
            entry.locked = false;
        }
        log::debug!("[trace-button] btn_unlock group={group}");
        ExtCallOutcome::Value(1)
    }

    /// `btn_set_toggle(group,index,toggle)` matches sub_40E830.  Native code
    /// queues a render command that ultimately changes the button cell; we
    /// store the toggle value and set the rect row directly.
    fn ext_btn_set_toggle(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let toggle = args[2];
        for entry in self.matching_button_entries_mut(group, index) {
            entry.toggle = toggle;
        }
        if let Some(sprites) = sprites {
            let cell_y = if toggle != 0 { 1 } else { 0 };
            for handle in self.matching_button_handles(group, index) {
                sprites.rect_set_pos(handle, 0, cell_y);
            }
        }
        log::debug!("[trace-button] btn_set_toggle group={group} index={index} toggle={toggle}");
        ExtCallOutcome::Value(1)
    }

    /// Category 8 index 14 is the native button state/cell setter.  Game.sqlite
    /// sub_410790 pops `(group,index,ctrl,state)` and calls `PalButtonCtrl`
    /// followed by `PalButtonSetPos`; settings/load/save scripts call this in
    /// tight batches, so the fourth pop is part of the stack contract.
    fn ext_btn_set_state(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let ctrl = args[2];
        let state = args[3];
        for entry in self.matching_button_entries_mut(group, index) {
            entry.toggle = ctrl;
        }
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                sprites.rect_set_pos(handle, 0, state.clamp(0, u16::MAX as i32) as u16);
            }
        }
        log::debug!(
            "[trace-button] btn_set_state group={group} index={index} ctrl={ctrl} state={state}"
        );
        ExtCallOutcome::Value(1)
    }

    /// `btn_set_alpha_0x(group,index,alpha)` matches sub_40E4B0.  It writes
    /// the alpha byte into the button sprite color field; the portable path
    /// clamps to 0..255 and updates the live sprite color.
    fn ext_btn_set_alpha(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let alpha = args[2].clamp(0, 255) as u8;
        for entry in self.matching_button_entries_mut(group, index) {
            entry.alpha = alpha;
        }
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                if let Some(sprite) = sprites.get_mut(handle) {
                    let raw = sprite.color.0 & 0x00FF_FFFF;
                    sprite.color = PalColor::from_argb(raw | ((alpha as u32) << 24));
                }
            }
        }
        log::debug!("[trace-button] btn_set_alpha group={group} index={index} alpha={alpha}");
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_set_anim(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let group = args[0];
        let index = args[1];
        let anim_id = args.get(2).copied().unwrap_or(0);
        let state = args.get(3).copied().unwrap_or(0).max(0) as u16;
        if let Some(sprites) = sprites {
            for handle in self.matching_button_handles(group, index) {
                sprites.rect_set_pos(handle, 0, state);
            }
        }
        log::debug!(
            "[trace-button] btn_set_anim group={group} index={index} anim_id={anim_id} state={state}"
        );
        ExtCallOutcome::Value(1)
    }

    fn ext_btn_expansion(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        if args.len() < 3 {
            return ExtCallOutcome::Block;
        }
        let mode = args[0];
        let index = args[1];
        let state = args[2];
        if mode == 0 {
            match index {
                // MAIN_BTN_SKIP/AUTO are registered as compact sprite buttons; the
                // expansion call selects their visual row/state after the script
                // has made the base controls transparent.
                0 | 1 => {
                    for entry in self.matching_button_entries_mut(0, index) {
                        entry.toggle = state;
                    }
                    if let Some(sprites) = sprites {
                        for handle in self.matching_button_handles(0, index) {
                            sprites.rect_set_pos(handle, 0, state.max(0) as u16);
                        }
                    }
                }
                // MAIN_BTN_VOICE is a wide pop-out strip. State 2/4 in the normal
                // ADV setup means it is registered for later use but not drawn as
                // the default text-window chrome.
                14 => {
                    for entry in self.matching_button_entries_mut(0, 14) {
                        entry.enabled = false;
                        entry.alpha = 0;
                    }
                    if let Some(sprites) = sprites {
                        for handle in self.matching_button_handles(0, 14) {
                            sprites.view_ctrl(handle, false);
                            if let Some(sprite) = sprites.get_mut(handle) {
                                sprite.color = PalColor::from_argb(sprite.color.0 & 0x00FF_FFFF);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        log::debug!(
            "[trace-button] btn_expansion mode={} index={} state={}",
            mode,
            index,
            state
        );
        ExtCallOutcome::Value(1)
    }

    /// `btn_set_hit(group, index)` matches Game category 8 index 22
    /// (sub_40DD90): native code calls PalButtonSetReaction for the stored
    /// button cell.  The portable renderer keeps reaction geometry in the
    /// sprite/button entry itself, so this clears any compatibility hit-rect
    /// override and lets normal button bounds drive future reactions.
    fn ext_btn_set_hit(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(2);
        let group = args.first().copied().unwrap_or(-1);
        let index = args.get(1).copied().unwrap_or(-1);
        for entry in self.matching_button_entries_mut(group, index) {
            entry.hit_rect = None;
        }
        log::debug!("[trace-button] btn_set_hit group={group} index={index}");
        ExtCallOutcome::Value(1)
    }

    /// Game category 4 BGM extcalls.
    ///
    /// Calls dispatch to PAL-style audio groups: play/load/stop mutate
    /// `game_audio`, volume calls mutate group volume and return integer status
    /// or current volume.  Evidence: shared ExtSig audio rows, Game.sqlite audio
    /// handlers, and PAL audio-group behavior captured in runtime fixtures.
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
            3 => {
                // bgm_get_volume(slot) returns a percent-style integer.  The
                // current engine keeps group volume globally, so this returns
                // the PAL default until per-slot BGM volume is fully modeled.
                self.pop_ext_args(1);
                let value = audio
                    .as_ref()
                    .map(|audio| volume_to_percent(audio.group_volume(PalSoundGroup::GROUP3)))
                    .unwrap_or(100);
                ExtCallOutcome::Value(value)
            }
            4 => {
                // bgm_get_auto_volume(): SOUND menu queries this zero-arg value
                // to seed the auto/BGM slider.  Until auto-ducking has a
                // distinct state, mirror the current BGM group volume.
                self.pop_ext_args(0);
                let value = audio
                    .as_ref()
                    .map(|audio| volume_to_percent(audio.group_volume(PalSoundGroup::GROUP3)))
                    .unwrap_or(100);
                ExtCallOutcome::Value(value)
            }
            8 => self.ext_bgm_filename(assets, nls),
            9 => self.ext_bgm_load(assets, nls, resource_manager, audio),
            10 => self.ext_bgm_play_loaded(audio),
            12 => {
                // get_master_volume(): native returns the PAL primary volume as
                // a script percent.  The setting page expects 0..100.
                self.pop_ext_args(0);
                let value = audio
                    .as_ref()
                    .map(|audio| volume_to_percent(audio.primary_volume()))
                    .unwrap_or(100);
                ExtCallOutcome::Value(value)
            }
            16 => ExtCallOutcome::Value(0),
            17 => ExtCallOutcome::Value(0),
            18 => ExtCallOutcome::Value(0),
            _ => ExtCallOutcome::Skip,
        }
    }

    /// Game category 5 SE extcalls.
    ///
    /// SE play/load/stop/wait operations map to PAL sound-effect group state.
    /// Wait/query calls return integer completion status; exact native async
    /// completion flags are approximated by current audio handle state.
    fn dispatch_se_ext(
        &mut self,
        index: u16,
        assets: &CoreAssets,
        nls: Nls,
        resource_manager: Option<&mut ResourceManager>,
        audio: Option<&mut AudioSystem>,
    ) -> ExtCallOutcome {
        match index {
            0 | 1 | 2 => self.ext_se_play(assets, nls, resource_manager, audio),
            3 => self.ext_audio_stop(5, PalSoundGroup::GROUP4, audio),
            4 => {
                let args = self.pop_ext_args(2);
                let volume = args.get(1).copied().unwrap_or(100);
                if let Some(audio) = audio {
                    let _ =
                        audio.set_group_volume(PalSoundGroup::GROUP4, percent_to_volume(volume));
                }
                ExtCallOutcome::Value(1)
            }
            5 => {
                self.pop_ext_args(1);
                let value = audio
                    .as_ref()
                    .map(|audio| volume_to_percent(audio.group_volume(PalSoundGroup::GROUP4)))
                    .unwrap_or(100);
                ExtCallOutcome::Value(value)
            }
            6 => self.ext_audio_stop(5, PalSoundGroup::GROUP4, audio),
            7 => self.ext_se_wait(audio),
            _ => ExtCallOutcome::Skip,
        }
    }

    /// Game category 13 voice/BGV extcalls.
    ///
    /// Game category 13 voice/BGV extcalls.
    ///
    /// Evidence: Game.sqlite `sub_4445F0` pops one percent value and stores the
    /// PAL-scaled voice volume; `sub_4445A0` pops no script argument and writes
    /// that cached volume to the extcall destination; `sub_443800` pops the slot
    /// only on the first `voice_wait` pass, then rewinds the script PC by 12
    /// bytes until the voice wait mask is cleared.
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
            2 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_volume = clamp_percent(args.first().copied().unwrap_or(100));
                if let Some(audio) = audio {
                    let _ = audio.set_group_volume(
                        PalSoundGroup::GROUP5,
                        percent_to_volume(self.text_state.voice_volume),
                    );
                }
                ExtCallOutcome::Value(1)
            }
            3 => {
                // voice_get_volume(): native sub_4445A0 does not pop a slot.
                ExtCallOutcome::Value(self.text_state.voice_volume)
            }
            4 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            5 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            6 | 13 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.voice_enabled))
            }
            8 => self.ext_voice_play(assets, nls, resource_manager, audio),
            9 => self.ext_audio_stop(13, PalSoundGroup::GROUP6, audio),
            10 => {
                let args = self.pop_ext_args(1);
                self.text_state.bgv_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            11 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(self.text_state.voice_volume)
            }
            12 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_volume = clamp_percent(args.first().copied().unwrap_or(100));
                ExtCallOutcome::Value(1)
            }
            14 => {
                self.pop_ext_args(0);
                self.text_state.voice_autopan_enabled = false;
                ExtCallOutcome::Value(1)
            }
            15 => {
                let args = self.pop_ext_args(1);
                self.text_state.voice_autopan_enabled = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            16 => {
                self.pop_ext_args(4);
                ExtCallOutcome::Value(1)
            }
            17 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(i32::from(self.text_state.voice_autopan_enabled))
            }
            18 => {
                let slot = match self.pending_voice_wait_slot {
                    Some(slot) => slot,
                    None => {
                        let args = self.pop_ext_args(1);
                        args.first().copied().unwrap_or(-1)
                    }
                };
                let handles = self
                    .game_audio
                    .iter()
                    .filter_map(|((category, audio_slot), handle)| {
                        (*category == 13 && (slot < 0 || *audio_slot == slot)).then_some(*handle)
                    })
                    .collect::<Vec<_>>();
                let any_playing = audio.is_some_and(|audio| {
                    handles
                        .iter()
                        .copied()
                        .any(|handle| audio.is_playing(handle).unwrap_or(false))
                });
                if any_playing {
                    self.pending_voice_wait_slot = Some(slot);
                    self.pc = self.pc.saturating_sub(12);
                    log::debug!("[trace-audio] voice_wait slot={slot} still playing");
                    return ExtCallOutcome::Wait {
                        value: 1,
                        request: WaitRequest::Frame(1),
                    };
                }
                self.pending_voice_wait_slot = None;
                log::debug!("[trace-audio] voice_wait slot={slot} complete");
                ExtCallOutcome::Value(1)
            }
            19 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            20 | 24 => {
                let args = self.pop_ext_args(1);
                self.text_state.bgv_muted = args.first().copied().unwrap_or(1) != 0;
                ExtCallOutcome::Value(1)
            }
            21 | 23 => {
                self.pop_ext_args(1);
                ExtCallOutcome::Value(1)
            }
            22 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(100)
            }
            25 => {
                self.pop_ext_args(2);
                ExtCallOutcome::Value(1)
            }
            26 => {
                self.pop_ext_args(0);
                ExtCallOutcome::Value(1)
            }
            28 => self.dispatch_wait_ext(0),
            29 => self.dispatch_wait_ext(1),
            30 => self.dispatch_wait_ext(2),
            31 => self.dispatch_wait_ext(3),
            32 => self.dispatch_wait_ext(4),
            34 => self.dispatch_wait_ext(6),
            35 => self.dispatch_wait_ext(7),
            36 => self.dispatch_wait_ext(8),
            _ => ExtCallOutcome::Skip,
        }
    }

    fn ext_sp_create(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        if args.is_empty() {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let Some(sprites) = sprites else {
            return ExtCallOutcome::Value(0);
        };
        if let Some(old) = self.game_sprites.remove(&slot) {
            sprites.release(old);
        }
        let surface_id = sprites.allocate_surface_id();
        let pixels = vec![0u8; 4];
        let surface = match SpriteSurface::rgba8(surface_id, 1, 1, 1, pixels) {
            Ok(surface) => surface,
            Err(err) => {
                log::warn!("[trace-sprite] sp_create slot={slot} surface failed: {err}");
                return ExtCallOutcome::Value(0);
            }
        };
        sprites.insert_surface(surface);
        let mut desc = SpriteDesc::new(SceneTextureId(surface_id.0), 1, 1);
        desc.visible = false;
        desc.source_name = format!("sp_create:{slot}");
        let handle = sprites.create(desc);
        self.game_sprites.insert(slot, handle);
        self.apply_pending_alpha_actions(sprites, slot, handle);
        log::debug!("[trace-sprite] sp_create slot={slot}");
        ExtCallOutcome::Value(1)
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
        if is_named_animation_resource(&name) {
            match open_resource_variant(resource_manager, &name, ANIMATION_EXTENSIONS) {
                Ok(asset) => {
                    if let (Some(old_anim), Some(task_system)) =
                        (self.game_sprite_animations.remove(&slot), task_system)
                    {
                        task_system.animation_release(old_anim);
                    }
                    if let Some(handle) = self.game_sprites.get(&slot).copied() {
                        if self.apply_named_sprite_animation(sprites, handle, &asset.bytes) {
                            log::debug!(
                                "[trace-sprite] sp_set slot={slot} name={name:?} asset={:?} applied named animation to existing sprite",
                                asset.name
                            );
                            return ExtCallOutcome::Value(1);
                        }
                        log::warn!(
                            "[trace-sprite] sp_set slot={slot} name={name:?} asset={:?} unsupported named animation",
                            asset.name
                        );
                        return ExtCallOutcome::Value(0);
                    }
                    self.game_sprite_pending_named_animations.insert(
                        slot,
                        PendingNamedSpriteAnimation {
                            asset_name: asset.name.clone(),
                            bytes: asset.bytes,
                        },
                    );
                    log::debug!(
                        "[trace-sprite] sp_set slot={slot} name={name:?} queued named animation asset={:?}",
                        asset.name
                    );
                    return ExtCallOutcome::Value(1);
                }
                Err(err) => {
                    log::warn!(
                        "[trace-sprite] sp_set slot={slot} name={name:?} animation open failed: {err}"
                    );
                    return ExtCallOutcome::Value(0);
                }
            }
        }
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
        if name.eq_ignore_ascii_case("BGM_SECRET") {
            return self.create_solid_sprite(
                sprites, slot, arg_count, raw_x, raw_y, raw_z, 0, 0, 0, &name,
            );
        }
        let asset = match open_resource_variant(resource_manager, &name, IMAGE_EXTENSIONS) {
            Ok(asset) => asset,
            Err(err) => {
                if let Some((r, g, b)) = parse_solid_color_name(&name) {
                    return self.create_solid_sprite(
                        sprites, slot, arg_count, raw_x, raw_y, raw_z, r, g, b, &name,
                    );
                }
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
        self.apply_pending_named_sprite_animation(sprites, slot, handle);
        self.apply_pending_alpha_actions(sprites, slot, handle);
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

    fn create_solid_sprite(
        &mut self,
        sprites: &mut SpriteSystem,
        slot: i32,
        arg_count: usize,
        raw_x: i32,
        raw_y: i32,
        raw_z: i32,
        r: u8,
        g: u8,
        b: u8,
        source_name: &str,
    ) -> ExtCallOutcome {
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
        desc.source_name = source_name.to_owned();
        let handle = sprites.create(desc);
        self.game_sprites.insert(slot, handle);
        self.apply_pending_named_sprite_animation(sprites, slot, handle);
        self.apply_pending_alpha_actions(sprites, slot, handle);
        log::debug!(
            "[trace-sprite] sp_set slot={slot} name={source_name:?} solid=rgb({r},{g},{b}) raw=({}, {}, {}) pos=({}, {}, {}) priority={}",
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
        let (width, height, rgba) = self.font_state.rasterize(&text);
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
            self.apply_pending_alpha_actions(sprites, slot, handle);
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
            self.apply_pending_alpha_actions(sprites, slot, handle);
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
        resource_manager: Option<&mut ResourceManager>,
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

        if let Some(resource_manager) = resource_manager {
            match open_resource_variant(resource_manager, &name, ANIMATION_EXTENSIONS) {
                Ok(asset) => {
                    if self.apply_named_sprite_animation(sprites, handle, &asset.bytes) {
                        log::debug!(
                            "[trace-sprite] sp_set_anim slot={slot} name={name:?} asset={:?} named entry_flag={entry_flag}",
                            asset.name
                        );
                        return ExtCallOutcome::Value(1);
                    }
                    log::debug!(
                        "[trace-sprite] sp_set_anim slot={slot} name={name:?} asset={:?} unsupported named animation",
                        asset.name
                    );
                }
                Err(err) => {
                    log::debug!(
                        "[trace-sprite] sp_set_anim slot={slot} name={name:?} animation open failed: {err}"
                    );
                }
            }
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

    fn apply_named_sprite_animation(
        &self,
        sprites: &mut SpriteSystem,
        handle: SpriteHandle,
        bytes: &[u8],
    ) -> bool {
        let Some(anim) = parse_named_sprite_animation(bytes) else {
            return false;
        };
        match anim {
            NamedSpriteAnimation::MoveBy {
                dx,
                dy,
                dz,
                duration_ms,
            } => sprites.tween_pos_by(handle, dx as f32, dy as f32, dz as f32, duration_ms),
            NamedSpriteAnimation::ScaleBy {
                delta_percent,
                duration_ms,
            } => {
                let Some(sprite) = sprites.get(handle) else {
                    return false;
                };
                let to = (sprite.scale + delta_percent as f32 / 100.0).max(0.01);
                sprites.tween_scale_to(handle, to, duration_ms)
            }
            NamedSpriteAnimation::AlphaBy { delta, duration_ms } => {
                let Some(sprite) = sprites.get(handle) else {
                    return false;
                };
                let alpha = i32::from(sprite.color.alpha())
                    .saturating_add(delta)
                    .clamp(0, 255) as u8;
                sprites.tween_alpha_to(handle, alpha, duration_ms)
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

    fn ext_sp_wait_draw(&mut self) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let frames = args.first().copied().unwrap_or(0).max(0);
        log::debug!("[trace-sprite] sp_wait_draw frames={frames}");
        if frames == 0 {
            ExtCallOutcome::Value(1)
        } else {
            ExtCallOutcome::Wait {
                value: 1,
                request: WaitRequest::Frame(frames),
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
        let slot = match self.pending_msp_wait_slot {
            Some(slot) => slot,
            None => {
                let args = self.pop_ext_args(1);
                if args.is_empty() {
                    return ExtCallOutcome::Block;
                }
                args[0]
            }
        };
        let Some(state) = self.game_msprites.get_mut(&slot) else {
            self.pending_msp_wait_slot = None;
            log::debug!("[trace-msprite] msp_wait slot={slot} missing");
            return ExtCallOutcome::Value(1);
        };
        if let Some(handle) = state.handle {
            let native_finished = (self.msprite_system.state(handle) & MSPRITE_STATE_FINISHED) != 0;
            if native_finished {
                state.playing = false;
                state.finished = true;
                self.pending_msp_wait_slot = None;
                return ExtCallOutcome::Value(1);
            }
            // Native sub_42E240 disables loop before entering the wait-retry path.
            self.msprite_system.set_loop(handle, 0);
        }
        if state.playing && !state.finished {
            self.pending_msp_wait_slot = Some(slot);
            self.pc = self.pc.saturating_sub(12);
            log::debug!("[trace-msprite] msp_wait slot={slot} wait_frame=1");
            return ExtCallOutcome::Wait {
                value: 1,
                request: WaitRequest::Frame(1),
            };
        }
        self.pending_msp_wait_slot = None;
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
            self.game_sprite_transitions.clear();
            self.game_sprite_pending_named_animations.clear();
            self.game_sprite_pending_alpha.clear();
        } else {
            let had_existing_sprite =
                self.game_sprites.contains_key(&slot) || self.game_msprites.contains_key(&slot);
            self.game_sprite_pending_named_animations.remove(&slot);
            if let Some(old_state) = self.game_msprites.remove(&slot) {
                if let Some(handle) = old_state.handle {
                    self.msprite_system.release(handle);
                }
            }
            if had_existing_sprite {
                self.game_sprite_pending_alpha.remove(&slot);
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
            let alpha = alpha.clamp(0, 255) as u8;
            if self.action_state.last_duration_ms > 0 {
                sprites.tween_alpha_to(handle, alpha, self.action_state.last_duration_ms);
            } else {
                sprites.set_alpha(handle, alpha);
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

    fn ext_sp_view_ctrl(
        &mut self,
        sprites: Option<&mut SpriteSystem>,
        visible: bool,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(slot) = args.first().copied() else {
            return ExtCallOutcome::Block;
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            sprites.view_ctrl(handle, visible);
        }
        log::debug!(
            "[trace-sprite] sp_{} slot={slot}",
            if visible { "show" } else { "hide" }
        );
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
            let dx = (x as f32) * 1.5;
            let dy = (y as f32) * 1.5;
            let dz = z as f32;
            if self.action_state.last_duration_ms > 0 {
                sprites.tween_pos_by(handle, dx, dy, dz, self.action_state.last_duration_ms);
            } else {
                sprites.move_pos(handle, dx, dy, dz);
            }
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
            let scale = scale as f32 / 100.0;
            if self.action_state.last_duration_ms > 0 {
                sprites.tween_scale_to(handle, scale, self.action_state.last_duration_ms);
            } else {
                sprites.set_scale(handle, scale);
            }
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

    fn ext_sp_surface_op(&mut self, arity: usize) -> ExtCallOutcome {
        self.pop_ext_args(arity);
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_get_dimension(
        &mut self,
        sprites: Option<&mut SpriteSystem>,
        width: bool,
    ) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(slot) = args.first().copied() else {
            return ExtCallOutcome::Block;
        };
        let value = if let (Some(handle), Some(sprites)) =
            (self.game_sprites.get(&slot).copied(), sprites)
        {
            (if width {
                sprites.get_width(handle).unwrap_or(0)
            } else {
                sprites.get_height(handle).unwrap_or(0)
            }) as i32
        } else {
            0
        };
        ExtCallOutcome::Value(value)
    }

    fn ext_sp_get_scale(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(slot) = args.first().copied() else {
            return ExtCallOutcome::Block;
        };
        let value = if let (Some(handle), Some(sprites)) =
            (self.game_sprites.get(&slot).copied(), sprites)
        {
            sprites
                .get(handle)
                .map(|sprite| (sprite.scale * 100.0).round() as i32)
                .unwrap_or(100)
        } else {
            100
        };
        ExtCallOutcome::Value(value)
    }

    fn ext_sp_bitblt(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(5);
        if args.len() < 5 {
            return ExtCallOutcome::Block;
        }
        let dst_slot = args[0];
        let src_slot = args[1];
        let x = args[2];
        let y = args[3];
        if let Some(sprites) = sprites {
            if let (Some(dst), Some(src)) = (
                self.game_sprites.get(&dst_slot).copied(),
                self.game_sprites.get(&src_slot).copied(),
            ) {
                if let Some(src_sprite) = sprites.get(src).cloned() {
                    let right = x.saturating_add(src_sprite.cell_size.width as i32);
                    let bottom = y.saturating_add(src_sprite.cell_size.height as i32);
                    let _ = sprites.set_rect(dst, Some(PalRect::new(x, y, right, bottom)));
                }
            }
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_shake(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let slot = args[0];
        let amplitude = args[1].abs().min(32);
        let phase = ((self.pal_time_ms / 33) & 1) as i32;
        let offset = if amplitude == 0 {
            0
        } else if phase == 0 {
            amplitude
        } else {
            -amplitude
        };
        if let (Some(handle), Some(sprites)) = (self.game_sprites.get(&slot).copied(), sprites) {
            let _ = sprites.set_offset_pos(handle, offset, 0);
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_set_transition(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(4);
        if args.len() < 4 {
            return ExtCallOutcome::Block;
        }
        let transition_slot = args[0];
        let sprite_slot = args[1];
        let transition_id = args[2].max(0) as u32;
        let duration_ms = args[3].max(1) as u32;
        if let Some(sprites) = sprites {
            let handle = *self
                .game_sprite_transitions
                .entry(transition_slot)
                .or_insert_with(|| sprites.create_transition_handle());
            let to = self.game_sprites.get(&sprite_slot).copied();
            if to.is_some() {
                if let Some(handle) = to {
                    if sprites.is_screen_copy(handle) {
                        let _ = sprites.view_ctrl(handle, true);
                    }
                }
                let _ = sprites.set_transition(
                    handle,
                    transition_slot.max(0) as u32,
                    None,
                    to,
                    transition_id,
                    duration_ms,
                    0,
                );
            }
        }
        ExtCallOutcome::Value(1)
    }

    fn ext_sp_copy_image(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let Some(slot) = args.first().copied() else {
            return ExtCallOutcome::Block;
        };
        let Some(sprites) = sprites else {
            return ExtCallOutcome::Value(1);
        };
        let (width, height) = self.logical_size();
        let existing = self.game_sprites.get(&slot).copied();
        let restore_visible = existing.and_then(|handle| sprites.get(handle).map(|sp| sp.visible));
        if let Some(handle) = existing {
            let _ = sprites.view_ctrl(handle, false);
        }

        let mut scene = FrameScene::boot().with_logical_size(width, height);
        for texture in sprites.textures() {
            scene.textures.push(texture);
        }
        for command in sprites.commands() {
            scene.commands.push(command);
        }
        for command in sprites.transition_commands() {
            scene.commands.push(command);
        }
        if let Some(quad) = self.effect_overlay(width, height) {
            scene
                .commands
                .push(crate::scene::DrawCommand::SolidQuad(quad));
        }
        let rgba = rasterize_scene_rgba(&scene);

        if let (Some(handle), Some(visible)) = (existing, restore_visible) {
            let _ = sprites.view_ctrl(handle, visible);
        }
        let mut updated_handle = None;
        if let Some(handle) = self.game_sprites.get(&slot).copied() {
            let _ = sprites.replace_sprite_surface(
                handle,
                width,
                height,
                rgba,
                "screen-copy:backbuffer",
            );
            let _ = sprites.set_pos(handle, 0, 0, 0);
            let _ = sprites.set_priority(handle, 0);
            let _ = sprites.view_ctrl(handle, false);
            updated_handle = Some(handle);
        } else if let Some(handle) = sprites.create_rgba_sprite(
            width,
            height,
            rgba,
            PalVec3::new(0, 0, 0),
            0,
            "screen-copy:backbuffer",
        ) {
            let _ = sprites.view_ctrl(handle, false);
            self.game_sprites.insert(slot, handle);
            updated_handle = Some(handle);
        }
        if let Some(handle) = updated_handle {
            self.apply_pending_named_sprite_animation(sprites, slot, handle);
        }
        log::debug!("[trace-sprite] sp_copy_image slot={slot} size={width}x{height}");
        ExtCallOutcome::Value(1)
    }

    fn apply_pending_named_sprite_animation(
        &mut self,
        sprites: &mut SpriteSystem,
        slot: i32,
        handle: SpriteHandle,
    ) {
        let Some(pending) = self.game_sprite_pending_named_animations.remove(&slot) else {
            return;
        };
        if self.apply_named_sprite_animation(sprites, handle, &pending.bytes) {
            log::debug!(
                "[trace-sprite] sp_set slot={slot} asset={:?} applied queued named animation",
                pending.asset_name
            );
        } else {
            log::warn!(
                "[trace-sprite] sp_set slot={slot} asset={:?} queued named animation unsupported",
                pending.asset_name
            );
        }
    }

    fn ext_sp_transition(&mut self, sprites: Option<&mut SpriteSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(3);
        let (Some(sprite_slot), Some(transition_id), Some(duration_ms)) = (
            args.first().copied(),
            args.get(1).copied(),
            args.get(2).copied(),
        ) else {
            return ExtCallOutcome::Block;
        };
        if let Some(sprites) = sprites {
            let handle = *self
                .game_sprite_transitions
                .entry(sprite_slot)
                .or_insert_with(|| sprites.create_transition_handle());
            let to = self.game_sprites.get(&sprite_slot).copied();
            if to.is_some() {
                if let Some(handle) = to {
                    if sprites.is_screen_copy(handle) {
                        let _ = sprites.view_ctrl(handle, true);
                    }
                }
                let _ = sprites.set_transition(
                    handle,
                    sprite_slot.max(0) as u32,
                    None,
                    to,
                    transition_id.max(0) as u32,
                    duration_ms.max(1) as u32,
                    0,
                );
            }
        }
        log::debug!(
            "[trace-sprite] sp_transition slot={sprite_slot} id={transition_id} duration={duration_ms}"
        );
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

    /// `bgm_set_volume(volume)` maps Game category 4 index 2 to PAL group-3
    /// volume.  It pops one percent argument, writes no extra VM state, and
    /// returns `1` for PAL-style success.
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

    fn ext_se_wait(&mut self, audio: Option<&mut AudioSystem>) -> ExtCallOutcome {
        let args = self.pop_ext_args(1);
        let slot = args.first().copied().unwrap_or(-1);
        let handles = self
            .game_audio
            .iter()
            .filter_map(|((category, audio_slot), handle)| {
                (*category == 5 && (slot < 0 || *audio_slot == slot)).then_some(*handle)
            })
            .collect::<Vec<_>>();
        let Some(audio) = audio else {
            log::debug!("[trace-audio] se_wait slot={slot} no audio backend");
            return ExtCallOutcome::Value(1);
        };
        let any_playing = handles
            .iter()
            .copied()
            .any(|handle| audio.is_playing(handle).unwrap_or(false));
        if any_playing {
            log::debug!("[trace-audio] se_wait slot={slot} still playing");
            return ExtCallOutcome::Wait {
                value: 1,
                request: WaitRequest::Frame(1),
            };
        }
        log::debug!("[trace-audio] se_wait slot={slot} complete");
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

    fn ext_wsprint_compat(&mut self) -> ExtCallOutcome {
        // Category 18 index 9, `sub_419560` in the current Game.exe IDB.
        // The reachable scripts call it with zero direct extcall arguments as
        // a formatting/status helper. The full native wsprintf-style buffer
        // write still needs a per-callsite argument-stack reverse, but a
        // dedicated handler is important: it preserves the exact zero-pop
        // discipline and makes remaining text/system failures visible instead
        // of hiding them behind shared-signature fallback.
        self.pop_ext_args(0);
        ExtCallOutcome::Value(0)
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
                if is_known_non_asset_probe_name(&name) {
                    log::debug!("[trace-assets] openfile probe name={name:?} ignored: {err}");
                } else {
                    log::warn!("[trace-assets] openfile name={name:?} failed: {err}");
                }
                ExtCallOutcome::Value(0)
            }
        }
    }

    fn ext_close_file_not_handle(&mut self) -> ExtCallOutcome {
        // close_file_not_handle(): reachable zero-argument Game category 18
        // cleanup helper used after File.dat/table probes.  Native closes
        // transient file objects that are not retained by handle; the portable
        // VM has no separate transient pool, so clearing invalid/empty slots and
        // returning success preserves the visible save/load/settings flow.
        self.pop_ext_args(0);
        log::debug!(
            "[trace-assets] close_file_not_handle retained_handles={}",
            self.file_handles.len()
        );
        ExtCallOutcome::Value(1)
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
        self.vm_trace(format_args!(
            "  ext_args requested={count} available={available} popped={args:?} stack_len={}",
            self.stack.len()
        ));
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
            .or_else(|| {
                read_text_record_string(&assets.file_dat.bytes, offset, nls)
                    .filter(|value| is_plausible_resource_name(value))
            })
            .or_else(|| {
                read_text_record_string(&assets.text_dat.bytes, offset, nls)
                    .filter(|value| is_plausible_resource_name(value))
            })
            .or_else(|| {
                read_c_string(&assets.text_dat.bytes, offset, nls)
                    .filter(|value| is_plausible_resource_name(value))
            })
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
                        Ok(ini) => self.set_system_ini(ini),
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
            .map(|(_, entry)| entry.handle)
            .collect()
    }

    fn matching_button_entries_mut(&mut self, group: i32, index: i32) -> Vec<&mut GameButtonEntry> {
        self.game_buttons
            .iter_mut()
            .filter(|((button_group, button_index), _)| {
                (group < 0 || *button_group == group) && (index < 0 || *button_index == index)
            })
            .map(|(_, entry)| entry)
            .collect()
    }

    fn slider_anchor_position(
        &self,
        sprites: &SpriteSystem,
        group: i32,
        index: i32,
    ) -> Option<(i32, i32)> {
        let mut candidates = Vec::with_capacity(4);
        if index >= 10 {
            candidates.push(index - 10);
        }
        if index >= 2 {
            candidates.push(index - 2);
        }
        if index >= 1 {
            candidates.push(index - 1);
        }
        candidates.push(index);
        candidates.into_iter().find_map(|base_index| {
            let entry = self.game_buttons.get(&(group, base_index))?;
            let sprite = sprites.get(entry.handle)?;
            let pos = sprite.effective_position();
            Some((pos.x, pos.y))
        })
    }

    /// Title menu callbacks (Game script points 3030/3031/3032) only store the
    /// requested modal page in `memdat[158]` before entering the shared system
    /// menu dispatcher at point 3081.  Native PAL button state removes the title
    /// group from the active view at that transition; without mirroring that
    /// side effect, DATA LOAD / SYSTEM controls are painted on top of the still
    /// interactive title menu.
    fn hide_title_buttons_for_modal_entry(
        &mut self,
        group: i32,
        index: i32,
        sprites: &mut SpriteSystem,
    ) {
        if group != 1 || !matches!(index, 3..=6) {
            return;
        }

        let keys = self
            .game_buttons
            .keys()
            .copied()
            .filter(|(button_group, _)| *button_group == 1)
            .collect::<Vec<_>>();

        for key in keys {
            let Some(entry) = self.game_buttons.get_mut(&key) else {
                continue;
            };
            entry.enabled = false;
            entry.alpha = 0;
            sprites.view_ctrl(entry.handle, false);
            if let Some(sprite) = sprites.get_mut(entry.handle) {
                sprite.color = PalColor::from_argb(sprite.color.0 & 0x00FF_FFFF);
            }
        }

        log::debug!("[trace-button] title modal entry hid group=1 source_index={index}");
    }

    fn dispatch_button_push_compat(&mut self, group: i32, index: i32) {
        // Native `btn_set` stores a callback point in arg[3] for any button
        // group.  Title, save/load, and system tabs all depend on the PAL button
        // system injecting a gosub when that entry is clicked.
        let Some(point_id) = self
            .game_buttons
            .get(&(group, index))
            .and_then(|e| e.gosub_point)
        else {
            return;
        };
        self.pending_gosub_point = Some(point_id);
        log::debug!(
            "[trace-button] queued gosub for group={group} index={index} -> point[{point_id}]"
        );
    }

    fn button_hit_at(
        &self,
        sprites: &SpriteSystem,
        mouse_x: i32,
        mouse_y: i32,
        group: i32,
    ) -> Option<(i32, i32)> {
        let mut best: Option<(i32, i32, i32)> = None;
        for ((button_group, button_index), entry) in &self.game_buttons {
            if group >= 0 && *button_group != group {
                continue;
            }
            if !entry.enabled || entry.locked {
                continue;
            }
            let Some(sprite) = sprites.get(entry.handle) else {
                continue;
            };
            if !sprite.visible {
                continue;
            }
            let rect = entry.hit_rect.unwrap_or_else(|| {
                let pos = sprite.effective_position();
                [
                    pos.x,
                    pos.y,
                    pos.x.saturating_add(sprite.source_rect.width()),
                    pos.y.saturating_add(sprite.source_rect.height()),
                ]
            });
            if mouse_x < rect[0] || mouse_x >= rect[2] || mouse_y < rect[1] || mouse_y >= rect[3] {
                continue;
            }
            let priority = sprite.effective_priority();
            if best.is_none_or(|(_, _, best_priority)| priority >= best_priority) {
                best = Some((*button_group, *button_index, priority));
            }
        }
        best.map(|(group, index, _)| (group, index))
    }

    pub fn dump_button_states(
        &self,
        sprites: &SpriteSystem,
        frame: u64,
        mouse_x: i32,
        mouse_y: i32,
    ) {
        if self.game_buttons.is_empty() {
            return;
        }
        let hovered = self.button_hit_at(sprites, mouse_x, mouse_y, -1);
        log::debug!(
            "[trace-buttons] frame={frame} mouse=({mouse_x},{mouse_y}) hovered={hovered:?} count={}",
            self.game_buttons.len()
        );
        for ((group, index), entry) in &self.game_buttons {
            let sprite = sprites.get(entry.handle);
            let visible = sprite.map_or(false, |s| s.visible);
            let sprite_alpha = sprite.map_or(0u8, |s| (s.color.0 >> 24) as u8);
            let priority = sprite.map_or(0, |s| s.effective_priority());
            let rect = entry.hit_rect.unwrap_or_else(|| {
                sprite.map_or([0, 0, 0, 0], |s| {
                    let pos = s.effective_position();
                    [
                        pos.x,
                        pos.y,
                        pos.x.saturating_add(s.source_rect.width()),
                        pos.y.saturating_add(s.source_rect.height()),
                    ]
                })
            });
            let hit = hovered == Some((*group, *index));
            let why_no_hit = if hit {
                ""
            } else if !entry.enabled {
                " [disabled]"
            } else if entry.locked {
                " [locked]"
            } else if !visible {
                " [not_visible]"
            } else if sprite_alpha == 0 {
                " [alpha=0]"
            } else {
                " [out_of_rect]"
            };
            log::debug!(
                "[trace-buttons]   group={group} index={index} name={:?} \
                 rect=[{},{},{},{}] vis={} en={} lock={} alpha={}/{} tog={} prio={} handle={}{hit_str}",
                entry.name,
                rect[0], rect[1], rect[2], rect[3],
                visible, entry.enabled, entry.locked,
                entry.alpha, sprite_alpha,
                entry.toggle, priority, entry.handle.0,
                hit_str = if hit { " [HIT]" } else { why_no_hit },
            );
        }
    }

    fn pop_latched_button_push(&mut self, group: i32) -> Option<i32> {
        if group >= 0 {
            let queue = self.button_push_queue.get_mut(&group)?;
            let value = queue.pop_front();
            if queue.is_empty() {
                self.button_push_queue.remove(&group);
            }
            return value;
        }
        let first_group = self
            .button_push_queue
            .iter()
            .find_map(|(button_group, queue)| (!queue.is_empty()).then_some(*button_group))?;
        let queue = self.button_push_queue.get_mut(&first_group)?;
        let value = queue.pop_front();
        if queue.is_empty() {
            self.button_push_queue.remove(&first_group);
        }
        value
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
        if group < 0 {
            self.button_push_queue.clear();
        } else {
            self.button_push_queue.remove(&group);
        }
    }

    fn logical_size(&self) -> (u32, u32) {
        ini_graphics_size(
            self.system_ini.as_ref(),
            FrameScene::PAL_DEFAULT_WIDTH,
            FrameScene::PAL_DEFAULT_HEIGHT,
        )
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
const ANIMATION_EXTENSIONS: &[&str] = &["", ".ANI", ".ani"];
const AUDIO_EXTENSIONS: &[&str] = &["", ".OGG", ".ogg", ".WAV", ".wav"];
const MOVIE_EXTENSIONS: &[&str] = &["", ".WMV", ".wmv", ".MPG", ".mpg", ".MP4", ".mp4"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NamedSpriteAnimation {
    MoveBy {
        dx: i32,
        dy: i32,
        dz: i32,
        duration_ms: u32,
    },
    ScaleBy {
        delta_percent: i32,
        duration_ms: u32,
    },
    AlphaBy {
        delta: i32,
        duration_ms: u32,
    },
}

fn parse_named_sprite_animation(bytes: &[u8]) -> Option<NamedSpriteAnimation> {
    if bytes.len() >= 0x1a && bytes[0] == 0xfc && bytes[1] == 0x13 && bytes[2] == 0xfd {
        let property = bytes[3];
        return match property {
            0 => Some(NamedSpriteAnimation::MoveBy {
                dx: read_i32_le_at(bytes, 0x04)?,
                dy: read_i32_le_at(bytes, 0x08)?,
                dz: read_i32_le_at(bytes, 0x0c)?,
                duration_ms: read_duration_le_at(bytes, 0x10)?,
            }),
            2 => Some(NamedSpriteAnimation::ScaleBy {
                delta_percent: read_i32_le_at(bytes, 0x04)?,
                duration_ms: read_duration_le_at(bytes, 0x08)?,
            }),
            3 => Some(NamedSpriteAnimation::AlphaBy {
                delta: read_i32_le_at(bytes, 0x04)?,
                duration_ms: read_duration_le_at(bytes, 0x08)?,
            }),
            other => {
                log::debug!("[trace-sprite] unsupported named animation property={other}");
                None
            }
        };
    }

    if bytes.len() >= 0x10 && bytes[0] == 0xfd && bytes[1] == 3 {
        return Some(NamedSpriteAnimation::AlphaBy {
            delta: i32::from(i8::from_ne_bytes([bytes[2]])),
            duration_ms: read_duration_le_at(bytes, 0x06)?,
        });
    }

    None
}

fn read_i32_le_at(bytes: &[u8], offset: usize) -> Option<i32> {
    let raw = bytes.get(offset..offset + 4)?;
    Some(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn read_duration_le_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(read_i32_le_at(bytes, offset)?.max(1) as u32)
}

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
    if (0..=4096).contains(&value) {
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

fn is_named_animation_resource(name: &str) -> bool {
    name.rsplit(['/', '\\']).next().is_some_and(|leaf| {
        leaf.eq_ignore_ascii_case("ANI")
            || leaf.to_ascii_uppercase().starts_with("ANI_")
            || leaf.to_ascii_uppercase().ends_with(".ANI")
    })
}

fn parse_solid_color_name(name: &str) -> Option<(u8, u8, u8)> {
    if name.eq_ignore_ascii_case("BK_BLACK") {
        return Some((0, 0, 0));
    }
    if name.eq_ignore_ascii_case("BGM_SECRET") {
        return Some((0, 0, 0));
    }
    if name.eq_ignore_ascii_case("BK_WHITE") {
        return Some((255, 255, 255));
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

fn parse_solid_color_name_argb(name: &str) -> Option<(u8, u8, u8, u8)> {
    if name == "#" {
        return Some((0, 0, 0, 0));
    }
    if name.eq_ignore_ascii_case("BK_BLACK") || name.eq_ignore_ascii_case("BGM_SECRET") {
        return Some((255, 0, 0, 0));
    }
    if name.eq_ignore_ascii_case("BK_WHITE") {
        return Some((255, 255, 255, 255));
    }
    let hex = name.strip_prefix('#')?;
    let normalized;
    let hex = match hex.len() {
        6 | 8 => hex,
        7 => {
            normalized = format!("0{hex}");
            normalized.as_str()
        }
        _ => return None,
    };
    let raw = u32::from_str_radix(hex, 16).ok()?;
    Some(match hex.len() {
        6 => (
            255,
            ((raw >> 16) & 0xFF) as u8,
            ((raw >> 8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        ),
        8 => (
            ((raw >> 24) & 0xFF) as u8,
            ((raw >> 16) & 0xFF) as u8,
            ((raw >> 8) & 0xFF) as u8,
            (raw & 0xFF) as u8,
        ),
        _ => return None,
    })
}

fn parse_pal_text_directives(text: &str) -> (String, Option<u16>) {
    let (directive, body) = if let Some(rest) = text.strip_prefix('<') {
        if let Some(end) = rest.find('>') {
            (Some(&rest[..end]), &rest[end + 1..])
        } else {
            (None, text)
        }
    } else {
        (None, text)
    };
    let mut size = None;
    if let Some(directive) = directive {
        for part in directive.split(|ch| ch == ';' || ch == ',' || ch == ' ') {
            let Some(raw_size) = part.strip_prefix("size=") else {
                continue;
            };
            if let Ok(value) = raw_size.parse::<u16>() {
                size = Some(value.max(1));
            }
        }
    }
    (strip_pal_inline_tags(body), size)
}

fn strip_pal_inline_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '>' {
                    closed = true;
                    break;
                }
                tag.push(next);
            }
            if closed && is_pal_text_tag(&tag) {
                continue;
            }
            out.push('<');
            out.push_str(&tag);
            if closed {
                out.push('>');
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn is_pal_text_tag(tag: &str) -> bool {
    let normalized = tag.trim().trim_start_matches('/').to_ascii_lowercase();
    normalized.is_empty()
        || normalized.starts_with("s")
        || normalized.starts_with("size=")
        || normalized.starts_with("color")
        || normalized.starts_with("font")
        || normalized.starts_with("ruby")
        || normalized.starts_with("r=")
}

fn compose_adv_text_panel(
    src_width: u32,
    src_height: u32,
    text_rgba: Vec<u8>,
    panel_text_width: u32,
    panel_text_height: u32,
    min_width: u32,
    min_height: u32,
    base_image: Option<DecodedImage>,
    window_alpha: i32,
    text_origin_x: u32,
    text_origin_y: u32,
) -> (u32, u32, Vec<u8>) {
    let pad_x = 24_u32;
    let pad_y = 18_u32;
    let (width, height, mut rgba, fallback_origin_x, fallback_origin_y) =
        if let Some(base) = base_image {
            let mut rgba = base.rgba;
            let alpha = window_alpha.clamp(0, 255) as u8;
            if alpha < 255 {
                for px in rgba.chunks_exact_mut(4) {
                    px[3] = ((u16::from(px[3]) * u16::from(alpha) + 127) / 255) as u8;
                }
            }
            (base.width, base.height, rgba, pad_x, pad_y)
        } else {
            let width = panel_text_width.saturating_add(pad_x * 2).max(min_width);
            let height = panel_text_height.saturating_add(pad_y * 2).max(min_height);
            let mut rgba = vec![0_u8; (width * height * 4) as usize];
            let alpha = if window_alpha > 0 {
                window_alpha.clamp(0, 255) as u8
            } else {
                184
            };
            for px in rgba.chunks_exact_mut(4) {
                px[0] = 16;
                px[1] = 16;
                px[2] = 20;
                px[3] = alpha;
            }
            (width, height, rgba, pad_x, pad_y)
        };
    let text_origin_x = if text_origin_x == 0 {
        fallback_origin_x
    } else {
        text_origin_x
    };
    let text_origin_y = if text_origin_y == 0 {
        fallback_origin_y
    } else {
        text_origin_y
    };
    let src_width = src_width.max(1);
    let src_height = src_height.max(1);
    for sy in 0..src_height {
        for sx in 0..src_width {
            let si = ((sy * src_width + sx) * 4) as usize;
            let Some(src) = text_rgba.get(si..si + 4) else {
                continue;
            };
            let alpha = src[3];
            if alpha == 0 {
                continue;
            }
            let dx = sx + text_origin_x;
            let dy = sy + text_origin_y;
            if dx >= width || dy >= height {
                continue;
            }
            let di = ((dy * width + dx) * 4) as usize;
            alpha_blend_rgba(&mut rgba[di..di + 4], src);
        }
    }
    (width, height, rgba)
}

fn alpha_blend_rgba(dst: &mut [u8], src: &[u8]) {
    let sa = src[3] as u32;
    let da = dst[3] as u32;
    let out_a = sa + (da * (255 - sa) + 127) / 255;
    if out_a == 0 {
        dst.fill(0);
        return;
    }
    for channel in 0..3 {
        let sc = src[channel] as u32;
        let dc = dst[channel] as u32;
        let premul = sc * sa + (dc * da * (255 - sa) + 127) / 255;
        dst[channel] = ((premul + out_a / 2) / out_a).min(255) as u8;
    }
    dst[3] = out_a.min(255) as u8;
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

fn looks_like_media_or_resource_name(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.ends_with(".pgd")
        || lower.ends_with(".png")
        || lower.ends_with(".jpg")
        || lower.ends_with(".bmp")
        || lower.ends_with(".ogg")
        || lower.ends_with(".wav")
        || lower.ends_with(".wmv")
        || (value.is_ascii() && is_plausible_resource_name(value))
}

fn is_known_non_asset_probe_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    matches!(upper.as_str(), "ACK_R" | "_HIDE" | "ISC_VOLUMELABEL_NAME")
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
    let text = decode_file_table_text(bytes, nls);
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

fn decode_file_table_text(bytes: &[u8], nls: Nls) -> String {
    if let Ok(text) = nls.decode(bytes) {
        return text;
    }

    for fallback in [Nls::ShiftJis, Nls::Gbk, Nls::Utf8] {
        if fallback == nls {
            continue;
        }
        if let Ok(text) = fallback.decode(bytes) {
            log::debug!(
                "[trace-assets] file table decoded with fallback nls={} after {} failed",
                fallback.name(),
                nls.name()
            );
            return text;
        }
    }

    let (text, _encoding_used, had_errors) = nls.encoding().decode(bytes);
    if had_errors {
        log::debug!(
            "[trace-assets] file table decoded lossily with nls={}",
            nls.name()
        );
    }
    text.into_owned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_table_decode_falls_back_when_configured_nls_rejects_comments() {
        let bytes = b"// invalid comment byte for some NLS: \x80\n\"vo01\",\"vo01_test\",1\n";
        let table = parse_file_table(bytes, Nls::Gbk).expect("ASCII CSV rows should parse");

        assert_eq!(table.entries.len(), 3);
        assert_eq!(table.entries[2], 1);
    }
}
