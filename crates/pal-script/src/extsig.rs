//! Shared semantic table for Game.exe script extcalls.
//!
//! This table is intentionally evidence-oriented: it is not just a pretty
//! decompiler signature list.  The VM, decompiler, and writeup should all agree
//! on these records before an extcall is treated as implemented.

#[path = "extsig_auto.rs"]
mod extsig_auto;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Integer,
    Slot,
    SpriteSlot,
    ButtonSlot,
    SoundSlot,
    ResourceName,
    TextId,
    TextString,
    FileDatString,
    DynamicString,
    IniSection,
    IniKey,
    IniFilename,
    PointId,
    CoordinateX,
    CoordinateY,
    CoordinateZ,
    DurationMs,
    Volume,
    Color,
    Alpha,
    Flag,
    Mode,
    Handle,
    BufferPointer,
    Unknown,
    /// Compatibility alias used by the current decompiler renderer.
    ResourceStringFromFileDat,
    /// Compatibility alias used by the current decompiler renderer.
    TextStringFromTextDat,
    /// Compatibility alias used by the current decompiler renderer.
    Coordinate,
    /// Compatibility alias used by the current decompiler renderer.
    Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnKind {
    Void,
    Integer,
    Bool,
    Handle,
    StringId,
    Status,
    PointId,
    UnsupportedUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideEffect {
    CreatesSprite,
    MutatesSprite,
    DeletesSprite,
    CreatesTask,
    BlocksScript,
    ReadsIni,
    ReadsFile,
    WritesVmMemory,
    CreatesSound,
    PlaysSound,
    StopsSound,
    ChangesTextState,
    ChangesSelectState,
    ChangesSaveState,
    ChangesHistoryState,
    ChangesRunState,
    CreatesMovie,
    MutatesWindow,
    UnknownSideEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImplStatus {
    Verified,
    Partial,
    Blocked,
    StackDisciplineOnly,
    Stub,
    Unknown,
    WrongOrSuspicious,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceKind {
    GameSqlite,
    PalSqlite,
    RuntimeTrace,
    Writeup,
    Disassembly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub reference: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub kind: ParamKind,
    pub meaning: &'static str,
}

/// One parameter in display order. `pop_idx` indexes the runtime pop-order
/// argument vector: 0 is the last pushed value and matches `pop_ext_args()[0]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Param {
    pub name: &'static str,
    pub pop_idx: usize,
    pub kind: ParamKind,
    pub meaning: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtSig {
    pub category: u16,
    pub index: u16,
    pub canonical_name: &'static str,
    /// Compatibility field for existing decompiler code.
    pub name: &'static str,
    pub game_handler_ea: Option<&'static str>,
    pub pal_export_name: Option<&'static str>,
    pub pal_export_ea: Option<&'static str>,
    pub pop_count: usize,
    pub pop_order: &'static [ParamSpec],
    pub display_order: &'static [usize],
    /// Compatibility field: display-order parameters with pop-order indices.
    pub params: &'static [Param],
    pub return_kind: ReturnKind,
    /// Compatibility field for existing decompiler code.
    pub returns: bool,
    pub side_effects: &'static [SideEffect],
    pub purpose: &'static str,
    pub evidence: &'static [Evidence],
    pub implementation_status: ImplStatus,
    pub decompiler_status: ImplStatus,
}

/// Backward-compatible name used by pal-decompiler.
pub type ExtCallSig = ExtSig;

macro_rules! evidence {
    () => {
        &[]
    };
    ($($kind:ident : $reference:literal),+ $(,)?) => {
        &[$(Evidence { kind: EvidenceKind::$kind, reference: $reference }),+]
    };
}

macro_rules! sidefx {
    () => {
        &[]
    };
    ($($effect:ident),+ $(,)?) => {
        &[$(SideEffect::$effect),+]
    };
}

macro_rules! params {
    () => {
        &[]
    };
    ($($name:literal : $idx:literal = $kind:ident => $meaning:literal),+ $(,)?) => {
        &[$(Param {
            name: $name,
            pop_idx: $idx,
            kind: ParamKind::$kind,
            meaning: $meaning,
        }),+]
    };
    ($($name:literal : $idx:literal = $kind:ident),+ $(,)?) => {
        &[$(Param {
            name: $name,
            pop_idx: $idx,
            kind: ParamKind::$kind,
            meaning: "",
        }),+]
    };
}

macro_rules! pop_order_from_params {
    ($params:expr) => {
        &[]
    };
}

macro_rules! display_order {
    () => {
        &[]
    };
    ($($idx:literal),+ $(,)?) => {
        &[$($idx),+]
    };
}

macro_rules! sig {
    (
        $cat:expr, $idx_val:expr, $name:literal,
        pop=$pop:expr,
        params=[$($p:tt)*],
        return=$ret:ident,
        effects=[$($fx:ident),* $(,)?],
        purpose=$purpose:literal,
        status=$status:ident,
        decompiler=$decompiler:ident,
        evidence=[$($ev_kind:ident : $ev_ref:literal),* $(,)?]
        $(, game=$game:expr)?
        $(, pal=$pal_name:expr => $pal_ea:expr)?
    ) => {{
        const PARAMS: &[Param] = params!($($p)*);
        ExtSig {
            category: $cat,
            index: $idx_val,
            canonical_name: $name,
            name: $name,
            game_handler_ea: sig!(@opt None $(, $game)?),
            pal_export_name: sig!(@opt None $(, $pal_name)?),
            pal_export_ea: sig!(@opt None $(, $pal_ea)?),
            pop_count: $pop,
            pop_order: pop_order_from_params!(PARAMS),
            display_order: display_order!(),
            params: PARAMS,
            return_kind: ReturnKind::$ret,
            returns: !matches!(ReturnKind::$ret, ReturnKind::Void),
            side_effects: sidefx!($($fx),*),
            purpose: $purpose,
            evidence: evidence!($($ev_kind : $ev_ref),*),
            implementation_status: ImplStatus::$status,
            decompiler_status: ImplStatus::$decompiler,
        }
    }};
    (@opt $default:expr) => { $default };
    (@opt $default:expr, $value:expr) => { Some($value) };
}

// Category 2 - text / ADV message state.

/// category 2 index 2: text
///
/// Purpose: Set current ADV text line and channel without blocking the VM.
/// Caller is responsible for following with wait_click or text_w.
///
/// VM arguments (display order: mode, text_id, name_or_aux_id, voice_or_aux_id):
/// - pop[0]: voice_or_aux_id (TextId) — voice/auxiliary id, or -1.
/// - pop[1]: name_or_aux_id (TextId) — speaker name Text.dat id, or -1.
/// - pop[2]: text_id (TextId) — Text.dat byte offset for message body.
/// - pop[3]: mode (Mode) — ADV channel/mode selector.
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
/// - RuntimeTrace: text args=[mode,text_id,name,voice]
///
/// Engine: Blocked — queues text display; does not block.
///
/// Decompiler: Blocked — renders four args in display order.
static SIG_TEXT: ExtSig = sig!(2, 2, "text", pop=4,
    params=[
        "mode":3=Mode=>"text mode / ADV channel",
        "text_id":2=TextId=>"Text.dat byte offset or dynamic string id",
        "name_or_aux_id":1=TextId=>"speaker/name text id or -1",
        "voice_or_aux_id":0=TextId=>"voice/aux id or -1"
    ],
    return=Void, effects=[ChangesTextState], purpose="Set current ADV text without script blocking.",
    status=Blocked, decompiler=Blocked,
    evidence=[Writeup:"docs/writeup.md 24.17", RuntimeTrace:"text args=[mode,text_id,name,voice]"]);
/// category 2 index 3: text_hide
///
/// Purpose: Hide the ADV text window and associated UI state immediately.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
///
/// Engine: Blocked — dispatched through dispatch_text_stub, hides text window.
///
/// Decompiler: Blocked — renders as text_hide().
static SIG_TEXT_HIDE: ExtSig = sig!(2, 3, "text_hide", pop=0, params=[],
    return=Void, effects=[ChangesTextState], purpose="Hide the ADV text window/state.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.17"]);
/// category 2 index 4: text_show
///
/// Purpose: Show the ADV text window.  The visible flag is observed in script
/// wrappers but actual semantics of 0 vs non-zero are engine-defined.
///
/// VM arguments:
/// - pop[0]: visible (Flag) — non-zero to show; 0 to hide (exact semantics
///   engine-dependent; most call sites pass 1).
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
///
/// Engine: Blocked — shows text window/state.
///
/// Decompiler: Blocked — renders as text_show(visible).
static SIG_TEXT_SHOW: ExtSig = sig!(2, 4, "text_show", pop=1,
    params=["visible":0=Flag=>"show flag observed in script wrappers"],
    return=Void, effects=[ChangesTextState], purpose="Show the ADV text window/state.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.17"]);
/// category 2 index 8: text_clear
///
/// Purpose: Clear the current text line and schedule body/name sprite release
/// on the next text synchronization step.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: ChangesTextState, DeletesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
///
/// Engine: Blocked — clears text state and queues sprite release.
///
/// Decompiler: Blocked — renders as text_clear().
static SIG_TEXT_CLEAR: ExtSig = sig!(2, 8, "text_clear", pop=0, params=[],
    return=Void, effects=[ChangesTextState, DeletesSprite], purpose="Clear current text and release body/name sprites on the next text sync.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.17"]);
/// category 2 index 15: text_w
///
/// Purpose: Set current ADV line and start the character-by-character visual
/// reveal animation.  Does NOT block the VM; script typically follows with an
/// explicit wait_click to wait for player input.
///
/// VM arguments (same layout as text/text_a):
/// - pop[0]: voice_or_aux_id (TextId) — voice/aux id, or -1.
/// - pop[1]: name_or_aux_id (TextId) — speaker name id, or -1.
/// - pop[2]: text_id (TextId) — Text.dat byte offset.
/// - pop[3]: mode (Mode) — ADV channel selector.
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
/// - RuntimeTrace: text_wait index=15 followed by explicit wait_click
///
/// Engine: Blocked — queues reveal-mode text; reveal proceeds independently.
///
/// Decompiler: Blocked — renders four args in display order.
static SIG_TEXT_W: ExtSig = sig!(2, 15, "text_w", pop=4,
    params=[
        "mode":3=Mode=>"text mode / ADV channel",
        "text_id":2=TextId=>"Text.dat byte offset or dynamic string id",
        "name_or_aux_id":1=TextId=>"speaker/name text id or -1",
        "voice_or_aux_id":0=TextId=>"voice/aux id or -1"
    ],
    return=Void, effects=[ChangesTextState], purpose="Set current ADV line and start visual reveal; does not block VM.",
    status=Blocked, decompiler=Blocked,
    evidence=[Writeup:"docs/writeup.md 24.17", RuntimeTrace:"text_wait index=15 followed by explicit wait_click"]);
/// category 2 index 16: text_a
///
/// Purpose: Set current ADV line in fully-visible (no reveal) mode.  Does not
/// block the VM.
///
/// VM arguments (same layout as text/text_w):
/// - pop[0]: voice_or_aux_id (TextId) — voice/aux id, or -1.
/// - pop[1]: name_or_aux_id (TextId) — speaker name id, or -1.
/// - pop[2]: text_id (TextId) — Text.dat byte offset.
/// - pop[3]: mode (Mode) — ADV channel selector.
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
///
/// Engine: Blocked — shows text fully; no per-character reveal.
///
/// Decompiler: Blocked — renders four args in display order.
static SIG_TEXT_A: ExtSig = sig!(2, 16, "text_a", pop=4,
    params=["mode":3=Mode, "text_id":2=TextId, "name_or_aux_id":1=TextId, "voice_or_aux_id":0=TextId],
    return=Void, effects=[ChangesTextState], purpose="Set current ADV line fully visible; does not block VM.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.17"]);
/// category 2 index 17: text_wa
///
/// Purpose: Combined text_w + text_a variant; starts reveal and does not block.
/// Exact distinction from text_w is engine-internal.
///
/// VM arguments (same layout as text/text_w/text_a):
/// - pop[0]: voice_or_aux_id (TextId) — voice/aux id, or -1.
/// - pop[1]: name_or_aux_id (TextId) — speaker name id, or -1.
/// - pop[2]: text_id (TextId) — Text.dat byte offset.
/// - pop[3]: mode (Mode) — ADV channel selector.
///
/// Return: void.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.17
///
/// Engine: Blocked — dispatched through dispatch_text_stub.
///
/// Decompiler: Blocked — renders four args in display order.
static SIG_TEXT_WA: ExtSig = sig!(2, 17, "text_wa", pop=4,
    params=["mode":3=Mode, "text_id":2=TextId, "name_or_aux_id":1=TextId, "voice_or_aux_id":0=TextId],
    return=Void, effects=[ChangesTextState], purpose="Set current ADV line and start visual reveal; does not block VM.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.17"]);
/// category 2 index 22: text_set_base
///
/// Purpose: Select the ADV message-window base image from File.dat.  The main
/// route uses this after checking the text-window style flag and passes one of
/// the File.dat ids 1418/1419/1420.
///
/// VM arguments (display order: mode, x, y, base_resource):
/// - pop[0]: base_resource (ResourceStringFromFileDat) — File.dat id for the
///   text window skin/base image.
/// - pop[1]: y (CoordinateY) — optional override; 0x0fffffff means default.
/// - pop[2]: x (CoordinateX) — optional override; 0x0fffffff means default.
/// - pop[3]: mode (Mode) — text base/window mode.
///
/// Return: status integer.
///
/// Side effects: ChangesTextState.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000346D8..00034760 pushes four operands before
///   ext_0002_0016.
/// - RuntimeTrace: text_set_base args=[0,268435455,268435455,1418].
///
/// Engine: Blocked — loads the selected base image when syncing ADV text.
///
/// Decompiler: Blocked — renders all four args in display order.
static SIG_TEXT_SET_BASE: ExtSig = sig!(2, 22, "text_set_base", pop=4,
    params=[
        "mode":3=Mode=>"text base/window mode",
        "x":2=CoordinateX=>"optional base x override; 0x0fffffff means default",
        "y":1=CoordinateY=>"optional base y override; 0x0fffffff means default",
        "base_resource":0=ResourceStringFromFileDat=>"File.dat id for ADV text-window base image"
    ],
    return=Integer, effects=[ChangesTextState], purpose="Select ADV message-window base image/resource.",
    status=Blocked, decompiler=Blocked,
    evidence=[Disassembly:"docs/dis.txt 000346D8..00034760", RuntimeTrace:"text_set_base args=[0,268435455,268435455,1418]"]);

// Category 3 - sprite wrappers.

/// category 3 index 2: sp_set
///
/// Purpose: Load a sprite into slot from a File.dat resource name and position
/// it at (x, y).  Creates or replaces the slot.
///
/// VM arguments (display order: slot, name, x, y):
/// - pop[0]: name (ResourceStringFromFileDat) — File.dat slot string, e.g. "BG01".
/// - pop[1]: slot (SpriteSlot) — sprite slot index.
/// - pop[2]: x (CoordinateX) — horizontal position.
/// - pop[3]: y (CoordinateY) — vertical position.
///
/// Return: void.
///
/// Side effects: CreatesSprite, MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md sprite sections
///
/// Engine: Blocked — loads resource and sets sprite position.
///
/// Decompiler: Blocked — renders as sp_set(slot, name, x, y).
static SIG_SP_SET: ExtSig = sig!(3, 2, "sp_set", pop=4,
    params=["slot":1=SpriteSlot, "name":0=ResourceStringFromFileDat, "x":2=CoordinateX, "y":3=CoordinateY],
    return=Void, effects=[CreatesSprite, MutatesSprite], purpose="Load/set a sprite slot from a File.dat resource name.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md sprite sections"]);
/// category 3 index 3: sp_set_ex
///
/// Purpose: Load a sprite into slot with explicit z/priority in addition to
/// (x, y) position.  Extended form of sp_set.
///
/// VM arguments (display order: slot, name, x, y, z):
/// - pop[0]: name (ResourceStringFromFileDat) — File.dat slot string, e.g. "#FFFFFFFF".
/// - pop[1]: slot (SpriteSlot) — sprite slot index.
/// - pop[2]: x (CoordinateX) — horizontal position.
/// - pop[3]: y (CoordinateY) — vertical position.
/// - pop[4]: z (CoordinateZ) — depth/priority layer.
///
/// Return: void.
///
/// Side effects: CreatesSprite, MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.13
/// - RuntimeTrace: sample sp_set_ex(40, "#FFFFFFFF", 0, 0, 0)
///
/// Engine: Blocked — loads resource and sets slot with z priority.
///
/// Decompiler: Blocked — renders as sp_set_ex(slot, name, x, y, z).
static SIG_SP_SET_EX: ExtSig = sig!(3, 3, "sp_set_ex", pop=5,
    params=["slot":1=SpriteSlot, "name":0=ResourceStringFromFileDat, "x":2=CoordinateX, "y":3=CoordinateY, "z":4=CoordinateZ],
    return=Void, effects=[CreatesSprite, MutatesSprite], purpose="Load/set a sprite slot with explicit z/priority.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.13"]);
/// category 3 index 4: sp_set_pos
///
/// Purpose: Immediately move a sprite slot to (x, y, z) without animation.
///
/// VM arguments (display order: slot, x, y, z):
/// - pop[0]: slot (SpriteSlot) — sprite slot to reposition.
/// - pop[1]: x (CoordinateX) — new horizontal position.
/// - pop[2]: y (CoordinateY) — new vertical position.
/// - pop[3]: z (CoordinateZ) — new depth/priority.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md sprite sections
///
/// Engine: Blocked — sets sprite position immediately via PAL sprite API.
///
/// Decompiler: Blocked — renders as sp_set_pos(slot, x, y, z).
static SIG_SP_SET_POS: ExtSig = sig!(3, 4, "sp_set_pos", pop=4,
    params=["slot":0=SpriteSlot, "x":1=CoordinateX, "y":2=CoordinateY, "z":3=CoordinateZ],
    return=Void, effects=[MutatesSprite], purpose="Move a sprite slot immediately.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md sprite sections"]);
/// category 3 index 5: sp_cls
///
/// Purpose: Release/clear a sprite slot.  slot=-1 clears all sprites.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot index, or -1 for wildcard.
///
/// Return: void.
///
/// Side effects: DeletesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md sprite sections
/// - RuntimeTrace: pal-vm ext_sp_cls pops 1; slot=-1 branch clears all
///
/// Engine: Blocked — releases slot via PalSpriteRelease; -1 walks all slots.
///
/// Decompiler: Blocked — renders as sp_cls(slot).
static SIG_SP_CLS: ExtSig = sig!(3, 5, "sp_cls", pop=1,
    params=["slot":0=SpriteSlot], return=Void, effects=[DeletesSprite], purpose="Release/clear a sprite slot.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md sprite sections"]);
/// category 3 index 6: sp_set_alpha
///
/// Purpose: Set the transparency of a sprite slot immediately.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot index.
/// - pop[1]: alpha (Alpha) — alpha value (0=transparent, 255=opaque).
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md sprite sections
///
/// Engine: Blocked — delegates to PalSpriteSetColor or equivalent alpha API.
///
/// Decompiler: Blocked — renders as sp_set_alpha(slot, alpha).
static SIG_SP_SET_ALPHA: ExtSig = sig!(3, 6, "sp_set_alpha", pop=2,
    params=["slot":0=SpriteSlot, "alpha":1=Alpha], return=Void, effects=[MutatesSprite], purpose="Set sprite alpha.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md sprite sections"]);
/// category 3 index 12: set_filter
///
/// Purpose: Set a sprite render/filter mode flag.  The script uses this as a
/// render-pipeline selector; the VM accepts and models it as stack-disciplined
/// state without interpreting the mode value.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot index.
/// - pop[1]: filter_mode (Mode) — render pipeline mode integer.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite decompilation search for set_filter wrapper
/// - PAL.sqlite: PalSpriteSetRenderMode 0x1011B4F8 / Game import thunk 0x4506BE
/// - RuntimeTrace: pal-vm dispatch_sprite_ext index 12 pops 2
///
/// Engine: Blocked — pops 2 args; filter_mode passed to PalSpriteSetRenderMode.
///
/// Decompiler: Blocked — renders as set_filter(slot, filter_mode).
static SIG_SP_SET_FILTER: ExtSig = sig!(3, 12, "set_filter", pop=2,
    params=["slot":0=SpriteSlot, "filter_mode":1=Mode],
    return=Void, effects=[MutatesSprite],
    purpose="Set a sprite render/filter mode; current VM treats it as stack-disciplined state because the observed script mainly uses it as a render pipeline flag.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite decompilation search for set_filter wrapper", PalSqlite:"PalSpriteSetRenderMode 0x1011B4F8 / Game import thunk 0x4506BE", RuntimeTrace:"pal-vm dispatch_sprite_ext index 12 pops 2"]);
/// category 3 index 11: sp_cls_ex
///
/// Purpose: Extended sprite clear; VM shares the sp_cls release path.
/// Alias for sp_cls with the same single-slot pop convention.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot index to release.
///
/// Return: void.
///
/// Side effects: DeletesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sprite dispatch shares 3:0005/000B/000D clear path
/// - PAL.sqlite: PalSpriteRelease 0x10120D45 / Game import thunk 0x45049E
/// - RuntimeTrace: pal-vm dispatch_sprite_ext index 11 pops 1
///
/// Engine: Blocked — releases slot via PalSpriteRelease.
///
/// Decompiler: Blocked — renders as sp_cls_ex(slot).
static SIG_SP_CLS_EX: ExtSig = sig!(3, 11, "sp_cls_ex", pop=1,
    params=["slot":0=SpriteSlot],
    return=Void, effects=[DeletesSprite],
    purpose="Extended sprite clear alias; VM shares the sp_cls release path.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sprite dispatch shares 3:0005/000B/000D clear path", PalSqlite:"PalSpriteRelease 0x10120D45 / Game import thunk 0x45049E", RuntimeTrace:"pal-vm dispatch_sprite_ext index 11 pops 1"]);
/// category 3 index 17: sp_set_scale
///
/// Purpose: Set the display scale of a sprite slot.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot index.
/// - pop[1]: scale (Integer) — scale factor (100 = 100% / normal size).
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.14
/// - PAL.sqlite: PalSpriteSetScale 0x1011FC5B / Game import thunk 0x450C4E
///
/// Engine: Blocked — delegates to PalSpriteSetScale.
///
/// Decompiler: Blocked — renders as sp_set_scale(slot, scale).
static SIG_SP_SET_SCALE: ExtSig = sig!(3, 17, "sp_set_scale", pop=2,
    params=["slot":0=SpriteSlot, "scale":1=Integer], return=Void, effects=[MutatesSprite], purpose="Set sprite scale.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.14", PalSqlite:"PalSpriteSetScale 0x1011FC5B / Game import thunk 0x450C4E"]);
/// category 3 index 25: sp_set_pos_move
///
/// Purpose: Animate a sprite slot to (x, y, z) over time.  Spawns a task;
/// script may follow with a wait if blocking is needed.
///
/// VM arguments (display order: slot, x, y, z):
/// - pop[0]: slot (SpriteSlot) — sprite slot to animate.
/// - pop[1]: x (CoordinateX) — target x position.
/// - pop[2]: y (CoordinateY) — target y position.
/// - pop[3]: z (CoordinateZ) — target depth.
///
/// Return: void.
///
/// Side effects: MutatesSprite, CreatesTask.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.13
///
/// Engine: Blocked — spawns move-animation task.
///
/// Decompiler: Blocked — renders as sp_set_pos_move(slot, x, y, z).
static SIG_SP_SET_POS_MOVE: ExtSig = sig!(3, 25, "sp_set_pos_move", pop=4,
    params=["slot":0=SpriteSlot, "x":1=CoordinateX, "y":2=CoordinateY, "z":3=CoordinateZ],
    return=Void, effects=[MutatesSprite, CreatesTask], purpose="Animate/move a sprite position.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.13"]);
/// category 3 index 41: sp_set_anim
///
/// Purpose: Attach a named sheet or ANI animation resource to a sprite slot
/// and begin playback.
///
/// VM arguments (display order: slot, name, flag):
/// - pop[0]: slot (SpriteSlot) — sprite slot to animate.
/// - pop[1]: name (ResourceStringFromFileDat) — animation resource name.
/// - pop[2]: flag (Flag) — playback control flag (loop/oneshot).
///
/// Return: void.
///
/// Side effects: MutatesSprite, CreatesTask.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.13
///
/// Engine: Blocked — loads animation and attaches to sprite slot.
///
/// Decompiler: Blocked — renders as sp_set_anim(slot, name, flag).
static SIG_SP_SET_ANIM_41: ExtSig = sig!(3, 41, "sp_set_anim", pop=3,
    params=["slot":0=SpriteSlot, "name":1=ResourceStringFromFileDat, "flag":2=Flag],
    return=Void, effects=[MutatesSprite, CreatesTask], purpose="Attach sheet or named ANI animation to a sprite slot.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.13"]);
/// category 3 index 57: sp_set_anim (alias)
///
/// Purpose: Alias/wrapper for sp_set_anim at index 41; identical arity and
/// argument layout.  Exact behavioral difference between indices 41 and 57
/// is not confirmed.
///
/// VM arguments: identical to sp_set_anim (3,41).
/// - pop[0]: slot (SpriteSlot)
/// - pop[1]: name (ResourceStringFromFileDat)
/// - pop[2]: flag (Flag)
///
/// Return: void.
///
/// Side effects: MutatesSprite, CreatesTask.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.13
///
/// Engine: Blocked — shares implementation path with index 41.
///
/// Decompiler: Blocked — renders as sp_set_anim(slot, name, flag).
///
/// Open points: Exact difference from index 41 not reversed.
static SIG_SP_SET_ANIM_57: ExtSig = sig!(3, 57, "sp_set_anim", pop=3,
    params=["slot":0=SpriteSlot, "name":1=ResourceStringFromFileDat, "flag":2=Flag],
    return=Void, effects=[MutatesSprite, CreatesTask], purpose="Alias/wrapper for sprite animation setup.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.13"]);
/// category 3 index 39: sp_set_shake
///
/// Purpose: Apply a PAL-style shake (vibration) animation to a sprite slot.
/// The VM models it as alternating offset motion.
///
/// VM arguments (display order: slot, amplitude, count, axis):
/// - pop[0]: slot (SpriteSlot) — sprite slot to shake.
/// - pop[1]: amplitude (Integer) — shake displacement magnitude in pixels.
/// - pop[2]: count (Integer) — number of shake oscillations.
/// - pop[3]: axis (Mode) — shake axis: 0=x, 1=y, 2=both (engine-defined).
///
/// Return: void.
///
/// Side effects: MutatesSprite, CreatesTask.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sprite handler path around sub_425640/sub_4275F0 stack pops
/// - RuntimeTrace: pal-vm ext_sp_set_shake pops 4; operand_fixture slot/amplitude/count/axis
/// - PAL.sqlite: PalSpriteSetOffsetPos 0x1011C65A / PalSpriteMoveOffsetPos 0x1011E022
///
/// Engine: Blocked — spawns shake-animation task using offset-position APIs.
///
/// Decompiler: Blocked — renders as sp_set_shake(slot, amplitude, count, axis).
static SIG_SP_SET_SHAKE: ExtSig = sig!(3, 39, "sp_set_shake", pop=4,
    params=["slot":0=SpriteSlot, "amplitude":1=Integer, "count":2=Integer, "axis":3=Mode],
    return=Void, effects=[MutatesSprite, CreatesTask],
    purpose="Apply PAL-style shake animation to a sprite slot; VM currently models it as alternating offset motion.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sprite handler path around sub_425640/sub_4275F0 stack pops", RuntimeTrace:"pal-vm ext_sp_set_shake pops 4; operand_fixture slot/amplitude/count/axis", PalSqlite:"PalSpriteSetOffsetPos 0x1011C65A / PalSpriteMoveOffsetPos 0x1011E022"]);
/// category 3 index 46: sp_show
///
/// Purpose: Make a sprite slot visible.  Calls PalSpriteViewCtrl(slot, true).
/// Pop count corrected from 2 to 1: runtime pops only the slot; there is no
/// separate visible flag argument.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot to show.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sprite view-control handler
/// - PAL.sqlite: PalSpriteViewCtrl 0x1011B859 / Game import thunk 0x4503BE
/// - RuntimeTrace: pal-vm ext_sp_view_ctrl pops 1 (slot only)
///
/// Engine: Blocked — delegates to PalSpriteViewCtrl(slot, true).
///
/// Decompiler: Blocked — renders as sp_show(slot).
static SIG_SP_SHOW: ExtSig = sig!(3, 46, "sp_show", pop=1,
    params=["slot":0=SpriteSlot],
    return=Void, effects=[MutatesSprite],
    purpose="Show sprite; VM calls PalSpriteViewCtrl(slot, true) after popping only the slot.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sprite view-control handler", PalSqlite:"PalSpriteViewCtrl 0x1011B859 / Game import thunk 0x4503BE", RuntimeTrace:"pal-vm ext_sp_view_ctrl pops 1 (slot only)"]);
/// category 3 index 47: sp_hide
///
/// Purpose: Make a sprite slot invisible.  Calls PalSpriteViewCtrl(slot, false).
/// Pop count corrected from 2 to 1: runtime pops only the slot.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — sprite slot to hide.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sprite view-control handler
/// - PAL.sqlite: PalSpriteViewCtrl 0x1011B859 / Game import thunk 0x4503BE
/// - RuntimeTrace: pal-vm ext_sp_view_ctrl pops 1 (slot only)
///
/// Engine: Blocked — delegates to PalSpriteViewCtrl(slot, false).
///
/// Decompiler: Blocked — renders as sp_hide(slot).
static SIG_SP_HIDE: ExtSig = sig!(3, 47, "sp_hide", pop=1,
    params=["slot":0=SpriteSlot],
    return=Void, effects=[MutatesSprite],
    purpose="Hide sprite; VM calls PalSpriteViewCtrl(slot, false) after popping only the slot.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sprite view-control handler", PalSqlite:"PalSpriteViewCtrl 0x1011B859 / Game import thunk 0x4503BE", RuntimeTrace:"pal-vm ext_sp_view_ctrl pops 1 (slot only)"]);
/// category 3 index 22: sptext
///
/// Purpose: Create a transparent text sprite through the PAL font/text surface
/// path (AdvCommandSpText).  Renders a Text.dat string directly onto a named
/// sprite slot surface.
///
/// VM arguments (display order: slot, text_id, x, y, mode):
/// - pop[0]: slot (SpriteSlot) — target sprite slot.
/// - pop[1]: text_id (TextId) — Text.dat byte offset for the string.
/// - pop[2]: x (CoordinateX) — render position x.
/// - pop[3]: y (CoordinateY) — render position y.
/// - pop[4]: mode (Mode) — text render mode/channel.
///
/// Return: void.
///
/// Side effects: CreatesSprite, ChangesTextState.
///
/// Evidence:
/// - Writeup: docs/writeup.md AdvCommandSpText
/// - RuntimeTrace: AdvCommandSpText 5-arg path
///
/// Engine: Blocked — renders text surface into sprite slot.
///
/// Decompiler: Blocked — renders as sptext(slot, text_id, x, y, mode).
static SIG_SPTEXT: ExtSig = sig!(3, 22, "sptext", pop=5,
    params=["slot":0=SpriteSlot, "text_id":1=TextId, "x":2=CoordinateX, "y":3=CoordinateY, "mode":4=Mode],
    return=Void, effects=[CreatesSprite, ChangesTextState], purpose="Create a transparent text sprite through the PAL font/text surface path.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md AdvCommandSpText", RuntimeTrace:"AdvCommandSpText 5-arg path"]);

// Category 4/5/13 - audio.

/// category 4 index 0: bgm_play
///
/// Purpose: Load and play a BGM track into a slot with volume and flags.
///
/// VM arguments (display order: slot, unknown, name, flags, volume, unknown2, unknown3):
/// - pop[0]: slot (SoundSlot) — BGM slot index.
/// - pop[1]: unknown (Unknown) — purpose not reversed; likely loop mode.
/// - pop[2]: name (ResourceStringFromFileDat) — File.dat resource name.
/// - pop[3]: flags (Flag) — playback flags (loop/fade).
/// - pop[4]: volume (Volume) — initial volume.
/// - pop[5]: unknown2 (Unknown) — purpose not reversed.
/// - pop[6]: unknown3 (Unknown) — purpose not reversed.
///
/// Return: void.
///
/// Side effects: CreatesSound, PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — loads BGM resource and begins playback.
///
/// Decompiler: Blocked — renders 7 args; unknown2/unknown3 meanings TBD.
///
/// Open points: Exact semantics of pop[1], pop[5], pop[6] not reversed.
static SIG_BGM_PLAY: ExtSig = sig!(4, 0, "bgm_play", pop=7,
    params=["slot":0=SoundSlot, "unknown":1=Unknown, "name":2=ResourceStringFromFileDat, "flags":3=Flag, "volume":4=Volume, "unknown2":5=Unknown, "unknown3":6=Unknown],
    return=Void, effects=[CreatesSound, PlaysSound], purpose="Load/play BGM with slot and volume parameters.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 4 index 1: bgm_stop
///
/// Purpose: Stop a BGM slot, optionally with a fade-out duration.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — BGM slot to stop.
/// - pop[1]: fade_ms (DurationMs) — fade duration in milliseconds; 0 = immediate.
///
/// Return: void.
///
/// Side effects: StopsSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — stops slot with optional fade.
///
/// Decompiler: Blocked — renders as bgm_stop(slot, fade_ms).
static SIG_BGM_STOP: ExtSig = sig!(4, 1, "bgm_stop", pop=2,
    params=["slot":0=SoundSlot, "fade_ms":1=DurationMs], return=Void, effects=[StopsSound], purpose="Stop BGM slot, optionally with fade.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 4 index 2: bgm_set_volume
///
/// Purpose: Set the global BGM volume level.  Pop count corrected from 2 to 1:
/// runtime pops only volume; there is no per-slot argument.
///
/// VM arguments:
/// - pop[0]: volume (Volume) — new BGM volume (0–100).
///
/// Return: void.
///
/// Side effects: PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
/// - RuntimeTrace: pal-vm ext_bgm_set_volume pops 1
///
/// Engine: Blocked — sets global BGM volume.
///
/// Decompiler: Blocked — renders as bgm_set_volume(volume).
static SIG_BGM_SET_VOLUME: ExtSig = sig!(4, 2, "bgm_set_volume", pop=1,
    params=["volume":0=Volume], return=Void, effects=[PlaysSound],
    purpose="Set global BGM volume; runtime pops 1 arg (volume only, no per-slot).",
    status=Blocked, decompiler=Blocked,
    evidence=[Writeup:"docs/writeup.md audio", RuntimeTrace:"pal-vm ext_bgm_set_volume pops 1"]);
/// category 4 index 3: bgm_get_volume
///
/// Purpose: Read the current BGM volume level.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — BGM slot to query (runtime currently ignores
///   this and returns a cached/constant value).
///
/// Return: Integer — current BGM volume (0–100).  Runtime returns 100 as
/// placeholder until BGM is properly tracked.
///
/// Side effects: none.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
/// - RuntimeTrace: dispatch_bgm_ext(3) returns Value(100) without consuming slot
///
/// Engine: Blocked — returns constant 100; does not query real PAL state.
/// NOTE: runtime has a stack-leak bug: pop[0] is NOT consumed.
///
/// Decompiler: Blocked — renders as bgm_get_volume(slot).
///
/// Open points: Runtime must pop slot arg before returning (stack-leak bug at
/// dispatch_bgm_ext index 3).
static SIG_BGM_GET_VOLUME: ExtSig = sig!(4, 3, "bgm_get_volume", pop=1,
    params=["slot":0=SoundSlot], return=Integer, effects=[], purpose="Read BGM volume state.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 4 index 9: bgm_load
///
/// Purpose: Preload a BGM resource into a slot without starting playback.
///
/// VM arguments (display order: slot, name, unknown, unknown2, unknown3):
/// - pop[0]: slot (SoundSlot) — destination BGM slot.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
/// - pop[2]: unknown (Unknown) — purpose not reversed.
/// - pop[3]: unknown2 (Unknown) — purpose not reversed.
/// - pop[4]: unknown3 (Unknown) — purpose not reversed.
///
/// Return: void.
///
/// Side effects: CreatesSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — preloads BGM resource into slot.
///
/// Decompiler: Blocked — renders 5 args; unknown meanings TBD.
///
/// Open points: Exact semantics of pop[2..4] not reversed.
static SIG_BGM_LOAD: ExtSig = sig!(4, 9, "bgm_load", pop=5,
    params=["slot":0=SoundSlot, "name":1=ResourceStringFromFileDat, "unknown":2=Unknown, "unknown2":3=Unknown, "unknown3":4=Unknown],
    return=Void, effects=[CreatesSound], purpose="Preload BGM resource into a slot.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 5 index 1: se_play
///
/// Purpose: Load and play a sound effect resource.
///
/// VM arguments (display order: slot, name, unknown, flags, volume):
/// - pop[0]: slot (SoundSlot) — SE slot index.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
/// - pop[2]: unknown (Unknown) — purpose not reversed; likely pan/channel.
/// - pop[3]: flags (Flag) — playback flags.
/// - pop[4]: volume (Volume) — playback volume.
///
/// Return: void.
///
/// Side effects: CreatesSound, PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — pops 5 args; loads and plays SE resource.
///
/// Decompiler: Blocked — renders 5 args.
static SIG_SE_PLAY: ExtSig = sig!(5, 1, "se_play", pop=5,
    params=["slot":0=SoundSlot, "name":1=ResourceStringFromFileDat, "unknown":2=Unknown, "flags":3=Flag, "volume":4=Volume],
    return=Void, effects=[CreatesSound, PlaysSound], purpose="Play SE resource.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 5 index 2: se_play_ex
///
/// Purpose: Play a sound effect; extended variant observed before first text_w
/// in many scenes.  Shares the same 5-arg layout as se_play.
///
/// VM arguments (same as se_play):
/// - pop[0]: slot (SoundSlot)
/// - pop[1]: name (ResourceStringFromFileDat)
/// - pop[2]: unknown (Unknown)
/// - pop[3]: flags (Flag)
/// - pop[4]: volume (Volume)
///
/// Return: void.
///
/// Side effects: CreatesSound, PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.1
///
/// Engine: Blocked — shares se_play dispatch path.
///
/// Decompiler: Blocked — renders 5 args.
///
/// Open points: Exact behavioral difference from se_play (index 1) not reversed.
static SIG_SE_PLAY_EX: ExtSig = sig!(5, 2, "se_play_ex", pop=5,
    params=["slot":0=SoundSlot, "name":1=ResourceStringFromFileDat, "unknown":2=Unknown, "flags":3=Flag, "volume":4=Volume],
    return=Void, effects=[CreatesSound, PlaysSound], purpose="Play SE resource; observed before first text_w.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.1"]);
/// category 5 index 0: se_load
///
/// Purpose: Preload a sound effect resource into a slot without playing it.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — destination SE slot.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
///
/// Return: void.
///
/// Side effects: CreatesSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — dispatched through ext_se_play (index 0 path).
///
/// Decompiler: Blocked — renders as se_load(slot, name).
static SIG_SE_LOAD: ExtSig = sig!(5, 0, "se_load", pop=2,
    params=["slot":0=SoundSlot, "name":1=ResourceStringFromFileDat], return=Void, effects=[CreatesSound],
    purpose="Preload SE resource.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 5 index 3: se_stop
///
/// Purpose: Stop a sound effect slot, optionally with a fade.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — SE slot to stop.
/// - pop[1]: fade_ms (DurationMs) — fade duration in ms; 0 = immediate.
///
/// Return: void.
///
/// Side effects: StopsSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — stops slot with optional fade.
///
/// Decompiler: Blocked — renders as se_stop(slot, fade_ms).
static SIG_SE_STOP: ExtSig = sig!(5, 3, "se_stop", pop=2,
    params=["slot":0=SoundSlot, "fade_ms":1=DurationMs], return=Void, effects=[StopsSound],
    purpose="Stop SE slot.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 5 index 4: se_set_volume
///
/// Purpose: Set the volume for a sound effect slot.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — SE slot index.
/// - pop[1]: volume (Volume) — new volume (0–100).
///
/// Return: void.
///
/// Side effects: PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — sets SE slot volume.
///
/// Decompiler: Blocked — renders as se_set_volume(slot, volume).
static SIG_SE_SET_VOLUME: ExtSig = sig!(5, 4, "se_set_volume", pop=2,
    params=["slot":0=SoundSlot, "volume":1=Volume], return=Void, effects=[PlaysSound],
    purpose="Set SE volume.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 5 index 5: se_get_volume
///
/// Purpose: Read the current volume of a sound effect slot.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — SE slot to query.
///
/// Return: Integer — current SE volume (0–100).
///
/// Side effects: none.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — returns cached SE volume for slot.
///
/// Decompiler: Blocked — renders as se_get_volume(slot).
static SIG_SE_GET_VOLUME: ExtSig = sig!(5, 5, "se_get_volume", pop=1,
    params=["slot":0=SoundSlot], return=Integer, effects=[],
    purpose="Read SE volume.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 13 index 0: voice_play
///
/// Purpose: Play a voice/dialogue audio resource.
///
/// VM arguments (display order: slot, name, flags, volume):
/// - pop[0]: slot (SoundSlot) — voice slot index.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
/// - pop[2]: flags (Flag) — playback flags.
/// - pop[3]: volume (Volume) — playback volume.
///
/// Return: void.
///
/// Side effects: CreatesSound, PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — loads and plays voice resource.
///
/// Decompiler: Blocked — renders as voice_play(slot, name, flags, volume).
static SIG_VOICE_PLAY: ExtSig = sig!(13, 0, "voice_play", pop=4,
    params=["slot":0=SoundSlot, "name":1=ResourceStringFromFileDat, "flags":2=Flag, "volume":3=Volume],
    return=Void, effects=[CreatesSound, PlaysSound], purpose="Play voice resource.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 13 index 1: voice_stop
///
/// Purpose: Stop a voice slot, optionally with fade.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — voice slot to stop.
/// - pop[1]: fade_ms (DurationMs) — fade duration in ms; 0 = immediate.
///
/// Return: void.
///
/// Side effects: StopsSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
///
/// Engine: Blocked — stops voice slot.
///
/// Decompiler: Blocked — renders as voice_stop(slot, fade_ms).
static SIG_VOICE_STOP: ExtSig = sig!(13, 1, "voice_stop", pop=2,
    params=["slot":0=SoundSlot, "fade_ms":1=DurationMs], return=Void, effects=[StopsSound],
    purpose="Stop voice slot.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md audio"]);
/// category 13 index 2: voice_set_volume
///
/// Purpose: Set the global voice volume.  Pop count corrected from 2 to 1:
/// runtime pops only volume; there is no per-slot argument.
///
/// VM arguments:
/// - pop[0]: volume (Volume) — new voice volume (0–100).
///
/// Return: void.
///
/// Side effects: PlaysSound.
///
/// Evidence:
/// - Writeup: docs/writeup.md audio
/// - RuntimeTrace: pal-vm dispatch_voice_ext index 2 pops 1
///
/// Engine: Blocked — sets global voice volume.
///
/// Decompiler: Blocked — renders as voice_set_volume(volume).
static SIG_VOICE_SET_VOLUME: ExtSig = sig!(13, 2, "voice_set_volume", pop=1,
    params=["volume":0=Volume], return=Void, effects=[PlaysSound],
    purpose="Set global voice volume; runtime pops 1 arg (volume only, no per-slot).",
    status=Blocked, decompiler=Blocked,
    evidence=[Writeup:"docs/writeup.md audio", RuntimeTrace:"pal-vm dispatch_voice_ext index 2 pops 1"]);
/// category 13 index 3: voice_get_volume
///
/// Purpose: Read the current voice volume.
///
/// VM arguments: none.
///
/// Return: Integer — current voice volume (0–100).
///
/// Side effects: none.
///
/// Evidence:
/// - GameSqlite: sub_4445A0 writes ctx[ctx[184629] + 177710] and performs no
///   stack-top decrement.
///
/// Engine: Blocked — returns cached modeled volume; native derives it from the
/// PAL-scaled ctx voice volume field.
///
/// Decompiler: Blocked — renders as voice_get_volume().
static SIG_VOICE_GET_VOLUME: ExtSig = sig!(13, 3, "voice_get_volume", pop=0,
    params=[], return=Integer, effects=[],
    purpose="Read global voice volume.", status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_4445A0"]);
/// category 13 index 18: voice_wait
///
/// Purpose: Block the VM until the specified voice slot finishes playback.
///
/// VM arguments:
/// - pop[0]: slot (SoundSlot) — voice slot to wait on.
///
/// Return: void.
///
/// Side effects: BlocksScript.
///
/// Evidence:
/// - GameSqlite: sub_443800 pops slot only on first pass, ORs ctx[163819],
///   stores timing fields, and rewinds ctx[201015] by 12 while not skipped.
///
/// Engine: Blocked — runtime models the native first-pass pop plus PC rewind
/// polling against the portable audio backend.
///
/// Decompiler: Blocked — renders as voice_wait(slot).
///
/// Open points: Runtime must implement actual blocking via WaitRequest::Frame
/// polling the voice slot completion state.
static SIG_VOICE_WAIT: ExtSig = sig!(13, 18, "voice_wait", pop=1,
    params=["slot":0=SoundSlot], return=Void, effects=[BlocksScript],
    purpose="Wait for voice slot completion.", status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_443800"]);
/// category 13 index 16: set_voice_autopan_size_over
///
/// Purpose: Configure the voice auto-pan clipping rectangle used by Game.exe
/// for positional voice panning.
///
/// VM arguments (display order: width, height, limit_x, limit_y):
/// - pop[0]: width (Integer) — pan region width.
/// - pop[1]: height (Integer) — pan region height.
/// - pop[2]: limit_x (Integer) — x boundary/limit for auto-pan.
/// - pop[3]: limit_y (Integer) — y boundary/limit for auto-pan.
///
/// Return: void.
///
/// Side effects: PlaysSound (modifies pan configuration).
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite voice/autopan wrapper; pal-vm dispatch_text_stub index 64 pops 4
/// - RuntimeTrace: reachable 000D:0010 appears before voice pan setup
///
/// Engine: Blocked — pops 4 args; auto-pan state stored internally.
///
/// Decompiler: Blocked — renders 4 args in display order.
static SIG_VOICE_AUTOPAN_SIZE_OVER: ExtSig = sig!(13, 16, "set_voice_autopan_size_over", pop=4,
    params=["width":0=Integer, "height":1=Integer, "limit_x":2=Integer, "limit_y":3=Integer],
    return=Void, effects=[PlaysSound],
    purpose="Configure the voice auto-pan size/threshold used by Game.exe voice positioning.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite voice/autopan wrapper; pal-vm dispatch_text_stub index 64 pops 4", RuntimeTrace:"reachable 000D:0010 appears before voice pan setup"]);

// Category 7 - waits. These block only via TaskSystem, not through text calls.

/// category 7 index 0: wait
///
/// Purpose: Block the script for a fixed duration.  When skip_cancel is
/// non-zero the player can advance the wait by clicking.
///
/// VM arguments:
/// - pop[0]: duration_ms (DurationMs) — wait duration in milliseconds.
/// - pop[1]: skip_cancel (Flag) — non-zero allows click/key to skip.
///
/// Return: void.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Game.sqlite: sub_444F40 pops duration then skip/cancel flag, records
///   start time with PaltimeGetTime, rewinds PC until time/cancel completes,
///   and returns 1.
/// - RuntimeTrace: pal-vm dispatch_wait_ext index 0 pops 2 and emits
///   WaitRequest::Time.
///
/// Engine: Verified — emits a timed wait request and traces the decoded
/// skip/cancel flag.
///
/// Decompiler: Verified — renders as wait(duration_ms, skip_cancel).
static SIG_WAIT: ExtSig = sig!(7, 0, "wait", pop=2,
    params=["duration_ms":0=DurationMs, "skip_cancel":1=Flag], return=Void, effects=[CreatesTask, BlocksScript],
    purpose="Timed script wait with native cancel/skip flag.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_444F40 pops duration/skip flag and blocks by rewinding PC until timeout", RuntimeTrace:"pal-vm dispatch_wait_ext index 0 pops duration/skip flag and returns WaitRequest::Time"],
    game="0x00444F40");
/// category 7 index 1: wait_click
///
/// Purpose: Wait for player input.  duration_ms=-1 waits for click/key only;
/// non-negative values create a click-or-timeout wait.
///
/// VM arguments:
/// - pop[0]: duration_ms (DurationMs) — timeout in ms, or <= 0 for click-only.
///
/// Return: void.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Game.sqlite: sub_444DE0 pops duration; runtime traces through the title
///   path show 0 is used as the persistent click/key wait path, while positive
///   values record a click-or-timeout wait, rewind PC, and return 1.
/// - RuntimeTrace: pal-vm dispatch_wait_ext index 1 pops duration and emits
///   Click or ClickOrTime wait.
///
/// Engine: Verified — emits WaitRequest::Click for non-positive durations or
/// ClickOrTime for positive duration.
///
/// Decompiler: Verified — renders as wait_click(duration_ms).
static SIG_WAIT_CLICK: ExtSig = sig!(7, 1, "wait_click", pop=1,
    params=["duration_ms":0=DurationMs], return=Void, effects=[CreatesTask, BlocksScript],
    purpose="Non-positive durations wait for input only; positive values are click-or-time waits.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_444DE0 pops duration and implements wait_click timeout/click-only semantics", RuntimeTrace:"pal-vm dispatch_wait_ext index 1 returns Click for <=0 or ClickOrTime for positive waits"],
    game="0x00444DE0");
/// category 7 index 2: wait_sync_begin
///
/// Purpose: Begin a PAL wait-sync timing scope.  Marks the start of a timed
/// section; use wait_sync_release or wait_sync_step to fence.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: CreatesTask.
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — records sync start timestamp.
///
/// Decompiler: Blocked — renders as wait_sync_begin().
static SIG_WAIT_SYNC_BEGIN: ExtSig = sig!(7, 2, "wait_sync_begin", pop=0, params=[],
    return=Void, effects=[CreatesTask], purpose="Begin PAL wait-sync timing scope.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 3: wait_sync_release
///
/// Purpose: End a wait-sync scope and block until the elapsed time since
/// wait_sync_begin reaches duration_ms.
///
/// VM arguments:
/// - pop[0]: duration_ms (DurationMs) — minimum total scope duration.
///
/// Return: void.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — computes remaining time and emits a timed WaitRequest.
///
/// Decompiler: Blocked — renders as wait_sync_release(duration_ms).
static SIG_WAIT_SYNC_RELEASE: ExtSig = sig!(7, 3, "wait_sync_release", pop=1,
    params=["duration_ms":0=DurationMs], return=Void, effects=[CreatesTask, BlocksScript],
    purpose="Release wait-sync with timed block.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 4: wait_sync_end
///
/// Purpose: Close a wait-sync timing scope without blocking.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: none (timing scope metadata only).
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — resets sync timestamp.
///
/// Decompiler: Blocked — renders as wait_sync_end().
static SIG_WAIT_SYNC_END: ExtSig = sig!(7, 4, "wait_sync_end", pop=0, params=[],
    return=Void, effects=[], purpose="End wait-sync timing scope.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 5: wait_sync_step
///
/// Purpose: One-frame wait fence used inside script animation loops to yield
/// control for exactly one render frame.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — emits WaitRequest::Frame(1).
///
/// Decompiler: Blocked — renders as wait_sync_step().
static SIG_WAIT_SYNC_STEP: ExtSig = sig!(7, 5, "wait_sync_step", pop=0, params=[],
    return=Void, effects=[CreatesTask, BlocksScript], purpose="One-frame wait fence used by script animation loops.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 7: wait_click_no_anim
///
/// Purpose: Input wait without triggering the wait-icon animation; otherwise
/// identical to wait_click.
///
/// VM arguments:
/// - pop[0]: duration_ms (DurationMs) — timeout in ms, or <= 0 for click-only.
///
/// Return: void.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Game.sqlite: sub_444C90 pops duration, shares wait_click timeout
///   semantics, but logs wait_click_no_anim and bypasses wait-icon animation.
/// - RuntimeTrace: pal-vm dispatch_wait_ext index 7 pops duration and emits
///   Click or ClickOrTime wait without wait-icon side effects.
///
/// Engine: Verified — emits the same blocking wait as wait_click while
/// avoiding wait-icon animation state.
///
/// Decompiler: Verified — renders as wait_click_no_anim(duration_ms).
static SIG_WAIT_CLICK_NO_ANIM: ExtSig = sig!(7, 7, "wait_click_no_anim", pop=1,
    params=["duration_ms":0=DurationMs], return=Void, effects=[CreatesTask, BlocksScript],
    purpose="Input wait without wait-icon animation side effects; non-positive durations are click-only.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_444C90 implements wait_click_no_anim duration semantics", RuntimeTrace:"pal-vm dispatch_wait_ext index 7 returns Click for <=0 or ClickOrTime for positive waits"],
    game="0x00444C90");
/// category 7 index 8: wait_sync_get_time
///
/// Purpose: Read elapsed PAL wait-sync time since the last wait_sync_begin.
///
/// VM arguments: none.
///
/// Return: Integer — elapsed time in milliseconds.
///
/// Side effects: none.
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — returns elapsed ms since last sync_begin.
///
/// Decompiler: Blocked — renders as wait_sync_get_time().
static SIG_WAIT_SYNC_GET_TIME: ExtSig = sig!(7, 8, "wait_sync_get_time", pop=0, params=[],
    return=Integer, effects=[], purpose="Read elapsed PAL wait-sync time.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 9: wait_time_push
///
/// Purpose: Push current PAL wait-time state onto an internal stack, allowing
/// nested wait-time scopes.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: none (wait-time stack mutation only).
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — saves wait-time context.
///
/// Decompiler: Blocked — renders as wait_time_push().
static SIG_WAIT_TIME_PUSH: ExtSig = sig!(7, 9, "wait_time_push", pop=0, params=[],
    return=Void, effects=[], purpose="Push PAL wait time state.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);
/// category 7 index 10: wait_time_pop
///
/// Purpose: Restore previously pushed PAL wait-time state.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: none (wait-time stack mutation only).
///
/// Evidence:
/// - Writeup: docs/writeup.md wait
///
/// Engine: Blocked — restores wait-time context.
///
/// Decompiler: Blocked — renders as wait_time_pop().
static SIG_WAIT_TIME_POP: ExtSig = sig!(7, 10, "wait_time_pop", pop=0, params=[],
    return=Void, effects=[], purpose="Pop PAL wait time state.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md wait"]);

// Category 6 - select/menu choice state.

/// category 6 index 0: select_init
///
/// Purpose: Initialize the select/menu system with a prompt text and color
/// scheme for normal, hovered, and disabled button states.
///
/// VM arguments (display order: text_id, normal_color, hover_color, disabled_color):
/// - pop[0]: text_id (TextId) — prompt/header text id, or -1.
/// - pop[1]: normal_color (Color) — default option color.
/// - pop[2]: hover_color (Color) — highlighted option color.
/// - pop[3]: disabled_color (Color) — color for disabled options.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 0 pops 4
///
/// Engine: Blocked — stores select layout and color scheme.
///
/// Decompiler: Blocked — renders 4 args in display order.
static SIG_SELECT_INIT: ExtSig = sig!(6, 0, "select_init", pop=4,
    params=["text_id":0=TextId, "normal_color":1=Color, "hover_color":2=Color, "disabled_color":3=Color],
    return=Void, effects=[ChangesSelectState],
    purpose="Initialize select/menu layout and colors.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 0 pops 4"]);
/// category 6 index 2: select_set
///
/// Purpose: Register a single selectable menu option with its display text,
/// jump target, position, color, and flags.
///
/// VM arguments (display order: text_id, target, x, y, color, flags):
/// - pop[0]: text_id (TextId) — option label text id.
/// - pop[1]: target (PointId) — script jump target when selected.
/// - pop[2]: x (CoordinateX) — display position x.
/// - pop[3]: y (CoordinateY) — display position y.
/// - pop[4]: color (Color) — option-specific color override.
/// - pop[5]: flags (Flag) — option state flags (enabled/disabled/etc).
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 2 pops 6
/// - RuntimeTrace: select_option text/target/pos/color/flags
///
/// Engine: Blocked — appends option entry to select state.
///
/// Decompiler: Blocked — renders 6 args in display order.
static SIG_SELECT_SET: ExtSig = sig!(6, 2, "select_set", pop=6,
    params=["text_id":0=TextId, "target":1=PointId, "x":2=CoordinateX, "y":3=CoordinateY, "color":4=Color, "flags":5=Flag],
    return=Void, effects=[ChangesSelectState],
    purpose="Register one selectable option with target/process metadata.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 2 pops 6", RuntimeTrace:"select_option text/target/pos/color/flags"]);
/// category 6 index 4: select_clear
///
/// Purpose: Clear all registered select/menu options and reset menu state.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 4 pops 0
///
/// Engine: Blocked — clears option list.
///
/// Decompiler: Blocked — renders as select_clear().
static SIG_SELECT_CLEAR: ExtSig = sig!(6, 4, "select_clear", pop=0,
    params=[],
    return=Void, effects=[ChangesSelectState],
    purpose="Clear all select/menu state.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite select handler; pal-vm dispatch_select_stub index 4 pops 0"]);

// Category 8 - title buttons.

/// category 8 index 0: btn_init
///
/// Purpose: Initialize a button group for use.  Pop count corrected from 1
/// to 3: runtime pops group + 2 additional configuration args whose meaning
/// has not been reversed.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index to initialize.
/// - pop[1]: arg1 (Unknown) — configuration argument; purpose not reversed.
/// - pop[2]: arg2 (Unknown) — configuration argument; purpose not reversed.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Disassembly: docs/dis.txt category 8
/// - RuntimeTrace: pal-vm ext_btn_init pops 3
///
/// Engine: Blocked — pops 3 args; group is initialized.
///
/// Decompiler: Blocked — renders as btn_init(group, arg1, arg2).
///
/// Open points: Meaning of arg1 and arg2 not reversed.
static SIG_BTN_INIT: ExtSig = sig!(8, 0, "btn_init", pop=3,
    params=["group":0=ButtonSlot, "arg1":1=Unknown, "arg2":2=Unknown],
    return=Void, effects=[ChangesSelectState],
    purpose="Initialize a button group; runtime pops 3 args (group + 2 unknown config args).",
    status=Blocked, decompiler=Blocked,
    evidence=[Disassembly:"docs/dis.txt category 8", RuntimeTrace:"pal-vm ext_btn_init pops 3"]);
/// category 8 index 1: btn_uninit
///
/// Purpose: Release all buttons belonging to the specified group.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group to uninitialize.
///
/// Return: void.
///
/// Side effects: DeletesSprite.
///
/// Evidence:
/// - Game.sqlite: sub_40DB20 walks active button groups, releases their
///   sprites, calls PalButtonRelease, and clears native group storage.
/// - PAL.sqlite: PalButtonRelease 0x1011D72B / PalButtonDelete 0x1011ACF1.
/// - RuntimeTrace: pal-vm ext_btn_uninit pops one group argument and returns 1.
///
/// Engine: Verified — compatibility runtime filters by group, accepts -1 as
/// all-groups wildcard, clears queued reactions, and releases sprite handles.
///
/// Decompiler: Verified — renders as btn_uninit(group).
static SIG_BTN_UNINIT: ExtSig = sig!(8, 1, "btn_uninit", pop=1,
    params=["group":0=ButtonSlot], return=Void, effects=[DeletesSprite],
    purpose="Release a button group; group=-1 is treated as all groups by the compatibility runtime.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40DB20 releases active button sprites and calls PalButtonRelease", PalSqlite:"PalButtonRelease 0x1011D72B / PalButtonDelete 0x1011ACF1", RuntimeTrace:"pal-vm ext_btn_uninit pops group and returns 1"],
    game="0x0040DB20", pal="PalButtonRelease" => "0x1011D72B");
/// category 8 index 3: btn_set
///
/// Purpose: Create or register a button entry within a group, binding four
/// sprite resource names (normal, hover, push, disabled states) and an entry
/// flag.
///
/// VM arguments (display order: group, index, normal, hover, push, disabled, entry_flag):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
/// - pop[2]: normal (ResourceStringFromFileDat) — normal-state sprite.
/// - pop[3]: hover (ResourceStringFromFileDat) — hover/focus-state sprite.
/// - pop[4]: push (ResourceStringFromFileDat) — pressed-state sprite.
/// - pop[5]: disabled (ResourceStringFromFileDat) — disabled-state sprite.
/// - pop[6]: entry_flag (Flag) — button state/type flags.
///
/// Return: void.
///
/// Side effects: CreatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.10
///
/// Engine: Blocked — loads sprites for each button state.
///
/// Decompiler: Blocked — renders 7 args in display order.
static SIG_BTN_SET: ExtSig = sig!(8, 3, "btn_set", pop=7,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "normal":2=ResourceStringFromFileDat, "hover":3=ResourceStringFromFileDat, "push":4=ResourceStringFromFileDat, "disabled":5=ResourceStringFromFileDat, "entry_flag":6=Flag],
    return=Void, effects=[CreatesSprite], purpose="Create/register a button entry and its sprite states.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.10"]);
/// category 8 index 4: btn_hide
///
/// Purpose: Hide an already registered button entry by submitting a native
/// button sprite visibility command.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
///
/// Return: void/status 1.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40F290 pops group/index, builds command 0x80004, and
///   logs btn_hide.
/// - RuntimeTrace: pal-vm ext_btn_view_ctrl(false) pops group/index and
///   hides matching sprite handles.
///
/// Engine: Verified — toggles matching button sprites invisible.
///
/// Decompiler: Verified — renders as btn_hide(group, index).
static SIG_BTN_HIDE: ExtSig = sig!(8, 4, "btn_hide", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Hide a registered button entry.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40F290 pops group/index and queues native btn_hide command", RuntimeTrace:"pal-vm ext_btn_view_ctrl(false) pops group/index and hides matching sprites"],
    game="0x0040F290");
/// category 8 index 5: btn_show
///
/// Purpose: Show an already registered button entry by submitting a native
/// button sprite visibility command.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
///
/// Return: void/status 1.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40F1B0 pops group/index, builds command 0x80005, and
///   logs btn_show.
/// - RuntimeTrace: pal-vm ext_btn_view_ctrl(true) pops group/index and shows
///   matching sprite handles.
///
/// Engine: Verified — toggles matching button sprites visible.
///
/// Decompiler: Verified — renders as btn_show(group, index).
static SIG_BTN_SHOW: ExtSig = sig!(8, 5, "btn_show", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Show a registered button entry.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40F1B0 pops group/index and queues native btn_show command", RuntimeTrace:"pal-vm ext_btn_view_ctrl(true) pops group/index and shows matching sprites"],
    game="0x0040F1B0");
/// category 8 index 6: btn_set_pos
///
/// Purpose: Move a button entry's sprite to (x, y).
///
/// VM arguments (display order: group, index, x, y):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
/// - pop[2]: x (CoordinateX) — new horizontal position.
/// - pop[3]: y (CoordinateY) — new vertical position.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.10
///
/// Engine: Blocked — repositions button sprite.
///
/// Decompiler: Blocked — renders as btn_set_pos(group, index, x, y).
static SIG_BTN_SET_POS: ExtSig = sig!(8, 6, "btn_set_pos", pop=4,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "x":2=CoordinateX, "y":3=CoordinateY],
    return=Void, effects=[MutatesSprite], purpose="Position a button entry.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.10"]);
/// category 8 index 8: btn_release
///
/// Purpose: Release one or more button entries.  index=-1 releases all
/// entries in the group.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group, or -1 for all.
///
/// Return: void.
///
/// Side effects: DeletesSprite.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.10
///
/// Engine: Blocked — releases button sprite entries.
///
/// Decompiler: Blocked — renders as btn_release(group, index).
static SIG_BTN_RELEASE: ExtSig = sig!(8, 8, "btn_release", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot], return=Void, effects=[DeletesSprite],
    purpose="Release one or more button entries.", status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.10"]);
/// category 8 index 9: btn_slider_get
///
/// Purpose: Query/update a slider button offset from the current mouse
/// position.  Used by the SYSTEM/SOUND menu for text-window alpha and audio
/// volume sliders.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — slider button index within group.
/// - pop[2]: max_offset (Integer) — maximum pixel offset for the slider.
/// - pop[3]: axis (Mode) — 0=horizontal, 1=vertical.
/// - pop[4]: snap_to_100 (Flag) — non-zero enables native hundred-step snapping.
///
/// Return: Integer — current slider offset in pixels.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40EC10 pops five values, reads PalInputGetMouseX/Y, clamps
///   to the supplied max offset, stores the sprite offset, and writes dst.
/// - RuntimeTrace: setting menu calls ext_0008_0009 with five pushed args before
///   converting the result to alpha/volume percentages.
///
/// Engine: Verified — compatible runtime computes/stores slider offset and
/// returns it.
///
/// Decompiler: Verified — renders as btn_slider_get(group,index,max_offset,axis,snap_to_100).
static SIG_BTN_SLIDER_GET: ExtSig = sig!(8, 9, "btn_slider_get", pop=5,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "max_offset":2=Integer, "axis":3=Mode, "snap_to_100":4=Flag],
    return=Integer, effects=[MutatesSprite, ChangesSelectState],
    purpose="Query/update slider button offset from mouse position for settings sliders.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40EC10 pops group/index/max_offset/axis/snap_to_100 and writes slider offset to dst", RuntimeTrace:"docs/dis.txt 00030D20 and 00044124 push five args before ext_0008_0009"],
    game="0x0040EC10");
/// category 8 index 10: btn_slider_set
///
/// Purpose: Initialize/update a slider button offset and enabled state.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot)
/// - pop[1]: index (ButtonSlot)
/// - pop[2]: offset (Integer)
/// - pop[3]: axis (Mode)
/// - pop[4]: enabled (Flag)
///
/// Return: void/status 1.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence: docs/dis.txt 00030B80 and related settings procedures push five
/// arguments to ext_0008_000A before entering the slider poll loop.
static SIG_BTN_SLIDER_SET: ExtSig = sig!(8, 10, "btn_slider_set", pop=5,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "offset":2=Integer, "axis":3=Mode, "enabled":4=Flag],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Initialize/update compatible slider offset and button state.",
    status=Verified, decompiler=Verified,
    evidence=[Disassembly:"docs/dis.txt 00030B80 and 000337C4 push five args before ext_0008_000A", RuntimeTrace:"setting submenu initializes slider before polling"]);
/// category 8 index 11: btn_slider_begin
///
/// Purpose: Arm a slider drag loop for the specified group/index.
///
/// VM arguments: group, index.
///
/// Return: void/status 1.
///
/// Evidence: settings procedures push group/index only before ext_0008_000B;
/// previous auto pop_count=3 consumed unrelated stack values.
static SIG_BTN_SLIDER_BEGIN: ExtSig = sig!(8, 11, "btn_slider_begin", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot],
    return=Void, effects=[ChangesSelectState],
    purpose="Arm native slider polling for a button entry.",
    status=Verified, decompiler=Verified,
    evidence=[Disassembly:"docs/dis.txt 00030CD4/000440CC push group,index before ext_0008_000B", RuntimeTrace:"runtime stack pollution stopped by two-arg pop"]);
/// category 8 index 12: btn_on_check
///
/// Purpose: Return whether a concrete button is currently reacting/under mouse.
///
/// VM arguments: group, index.
///
/// Return: Bool.
///
/// Evidence: Game.sqlite sub_410CE0 pops group/index, calls
/// PalButtonGetReaction, and writes a boolean result to the extcall dst.
static SIG_BTN_ON_CHECK: ExtSig = sig!(8, 12, "btn_on_check", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot],
    return=Bool, effects=[ChangesSelectState],
    purpose="Query current native reaction state for one button.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_410CE0 pops group/index and writes PalButtonGetReaction != 0 to dst", PalSqlite:"PalButtonGetReaction 0x1011E1D0", RuntimeTrace:"settings slider loops call ext_0008_000C with group/index"],
    game="0x00410CE0", pal="PalButtonGetReaction" => "0x1011E1D0");
/// category 8 index 13: btn_set_toggle
///
/// Purpose: Set the visual toggle/selection state for a button.  Used to
/// indicate the currently selected option in a button group.
///
/// VM arguments (display order: group, index, toggle):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
/// - pop[2]: toggle (Flag) — 1=selected/on, 0=unselected/off.
///
/// Return: void.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite button dispatch uses PalButtonMode/PalSpriteRectSetPos path
/// - PAL.sqlite: PalButtonMode 0x101191D5 / PalSpriteRectSetPos 0x1011E865
/// - RuntimeTrace: pal-vm ext_btn_set_toggle pops 3
///
/// Engine: Verified — updates compatible toggle state and moves the button
/// sprite rect to the selected/unselected cell row.
///
/// Decompiler: Verified — renders as btn_set_toggle(group, index, toggle).
static SIG_BTN_SET_TOGGLE: ExtSig = sig!(8, 13, "btn_set_toggle", pop=3,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "toggle":2=Flag],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Select the button visual/toggle state for a group/index.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40E830 pops group/index/toggle and queues native btn_set_toggle render command", PalSqlite:"PalSpriteRectSetPos 0x1011E865 is used by the native button render command pipeline", RuntimeTrace:"pal-vm ext_btn_set_toggle pops 3 and updates rect cell"],
    game="0x0040E830", pal="PalSpriteRectSetPos" => "0x1011E865");
/// category 8 index 14: btn_set_state
///
/// Purpose: Select the control mode and visual state/cell for a button entry.
///
/// VM arguments: group, index, ctrl, state.
///
/// Return: void/status 1.
///
/// Evidence: Game.sqlite sub_410790 pops four arguments and calls
/// PalButtonCtrl(button, ctrl) followed by PalButtonSetPos(button, state).
static SIG_BTN_SET_STATE: ExtSig = sig!(8, 14, "btn_set_state", pop=4,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "ctrl":2=Mode, "state":3=Mode],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Set button control mode and visual cell/state for menus.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_410790 pops group/index/ctrl/state then calls PalButtonCtrl and PalButtonSetPos", RuntimeTrace:"save/load/system menu ext_0008_000E batches require four pops to preserve stack"]);
/// category 8 index 15: btn_enable
///
/// Purpose: Enable or disable a button's input and visual state.  index=-1
/// applies to all entries in the group.
///
/// VM arguments (display order: group, index, enabled):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group, or -1 for all.
/// - pop[2]: enabled (Flag) — 1=enable, 0=disable.
///
/// Return: void.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite button dispatch around PalButtonCtrl
/// - PAL.sqlite: PalButtonCtrl 0x1011AE09 / Game import thunk 0x45034E
/// - RuntimeTrace: pal-vm ext_btn_enable pops 3
///
/// Engine: Verified — updates compatible enable/disabled state and applies the
/// corresponding sprite cell/alpha behavior.
///
/// Decompiler: Verified — renders as btn_enable(group, index, enabled).
static SIG_BTN_ENABLE: ExtSig = sig!(8, 15, "btn_enable", pop=3,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "enabled":2=Flag],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Enable or disable button input/visual state for one entry or a group wildcard.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40E5B0 pops group/index/enabled, calls PalButtonCtrl, and updates disabled flag bits", PalSqlite:"PalButtonCtrl 0x1011AE09 / Game import thunk 0x45034E", RuntimeTrace:"pal-vm ext_btn_enable pops group/index/enabled and updates compatible button state"],
    game="0x0040E5B0", pal="PalButtonCtrl" => "0x1011AE09");
/// category 8 index 16: btn_set_alpha_0x
///
/// Purpose: Set the alpha transparency of a button's sprite.  index=-1
/// applies to all entries in the group.
///
/// VM arguments (display order: group, index, alpha):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group, or -1 for all.
/// - pop[2]: alpha (Alpha) — transparency value (0=transparent, 255=opaque).
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite button dispatch alpha path
/// - PAL.sqlite: PalSpriteSetColor 0x10119103
/// - RuntimeTrace: pal-vm ext_btn_set_alpha pops 3
///
/// Engine: Verified — writes alpha into compatible button entries and sprite
/// colors.
///
/// Decompiler: Verified — renders as btn_set_alpha_0x(group, index, alpha).
static SIG_BTN_SET_ALPHA: ExtSig = sig!(8, 16, "btn_set_alpha_0x", pop=3,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "alpha":2=Alpha],
    return=Void, effects=[MutatesSprite],
    purpose="Set button sprite alpha for one entry or a group wildcard.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40E4B0 pops group/index/alpha and writes alpha into button sprite color field", PalSqlite:"PalSpriteSetColor 0x10119103 is the PAL color primitive used by sprite alpha paths", RuntimeTrace:"pal-vm ext_btn_set_alpha pops 3 and updates compatible sprite alpha"],
    game="0x0040E4B0", pal="PalSpriteSetColor" => "0x10119103");
/// category 8 index 21: btn_set_anim
///
/// Purpose: Select the animation cell/state for an already-registered button
/// sprite.  Controls multi-frame button animations.
///
/// VM arguments (display order: group, index, anim_id, state):
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
/// - pop[2]: anim_id (Integer) — animation identifier or cell index.
/// - pop[3]: state (Mode) — animation state/channel selector.
///
/// Return: void.
///
/// Side effects: MutatesSprite, ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite button animation state path
/// - PAL.sqlite: PalSpriteRectSetPos 0x1011E865
/// - RuntimeTrace: pal-vm ext_btn_set_anim pops 4
///
/// Engine: Blocked — sets button animation frame/state.
///
/// Decompiler: Blocked — renders as btn_set_anim(group, index, anim_id, state).
static SIG_BTN_SET_ANIM: ExtSig = sig!(8, 21, "btn_set_anim", pop=4,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot, "anim_id":2=Integer, "state":3=Mode],
    return=Void, effects=[MutatesSprite, ChangesSelectState],
    purpose="Select button animation/cell state for an already registered button.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite button animation state path", PalSqlite:"PalSpriteRectSetPos 0x1011E865", RuntimeTrace:"pal-vm ext_btn_set_anim pops 4"]);
/// category 8 index 22: btn_set_hit
///
/// Purpose: Bind/update the native PAL reaction target for a concrete button
/// entry.  Game.exe pops group then index, looks up the stored PAL button cell,
/// and calls PalButtonSetReaction(native_group, *entry).
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: index (ButtonSlot) — button index within group.
///
/// Return: void/status 1.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40DD90 pops group/index and calls PalButtonSetReaction.
/// - PAL.sqlite: PalButtonSetReaction 0x101186DB.
/// - RuntimeTrace: pal-vm ext_btn_set_hit pops 2 and returns 1.
///
/// Engine: Verified — portable runtime clears the compatibility hit override
/// for matching entries so native-style reaction hit testing is used.
///
/// Decompiler: Verified — renders as btn_set_hit(group, index).
static SIG_BTN_SET_HIT: ExtSig = sig!(8, 22, "btn_set_hit", pop=2,
    params=["group":0=ButtonSlot, "index":1=ButtonSlot],
    return=Void, effects=[ChangesSelectState],
    purpose="Bind/update the PAL reaction target for an existing button entry.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40DD90 pops group/index and calls PalButtonSetReaction", PalSqlite:"PalButtonSetReaction 0x101186DB", RuntimeTrace:"pal-vm ext_btn_set_hit pops group/index and returns 1"],
    game="0x0040DD90", pal="PalButtonSetReaction" => "0x101186DB");
/// category 8 index 17: btn_get_push
///
/// Purpose: Consume the latched "pushed button" index for a group.  Returns
/// the index of the last activated button, then clears the latch.  The script
/// polls this to determine which choice the player made.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group to query.
///
/// Return: Integer — pushed button index, or 0 when no push/reaction is pending
/// in the portable runtime path.
///
/// Side effects: ChangesSelectState.  Native PalButtonGetReaction can update
/// the active script/message reaction path; the portable runtime consumes a
/// latched click queue or performs mouse hit testing.
///
/// Evidence:
/// - Game.sqlite: sub_40CDE0 polls PalButtonGetReaction and mutates script
///   reaction state when a button is active.
/// - PAL.sqlite: PalButtonGetReaction 0x1011E1D0 / Game import thunk 0x45029E.
/// - RuntimeTrace: pal-vm ext_btn_get_push pops group, consumes compatible
///   push queue state, and returns 0 for no push.
///
/// Engine: Verified — models the native reaction poll with mouse hit testing
/// plus a latched push queue used by the portable input layer.
///
/// Decompiler: Verified — renders as btn_get_push(group).
static SIG_BTN_GET_PUSH: ExtSig = sig!(8, 17, "btn_get_push", pop=1,
    params=["group":0=ButtonSlot], return=Integer, effects=[ChangesSelectState],
    purpose="Poll/consume the native button reaction for a group; compatible no-push sentinel is 0.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40CDE0 calls PalButtonGetReaction and mutates script reaction state", PalSqlite:"PalButtonGetReaction 0x1011E1D0 / Game import thunk 0x45029E", RuntimeTrace:"pal-vm ext_btn_get_push pops group and returns clicked index or 0"],
    game="0x0040CDE0", pal="PalButtonGetReaction" => "0x1011E1D0");
/// category 8 index 19: btn_lock
///
/// Purpose: Lock input for a button group and optionally start a native lock
/// timer/duration.  This is group-scoped in Game.exe, not per-entry.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group index.
/// - pop[1]: duration_ms (DurationMs) — non-zero lock duration/timer value.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40E0C0 pops group/duration, sets group lock fields, and
///   records PaltimeGetTime when duration is non-zero.
/// - RuntimeTrace: pal-vm ext_btn_lock pops group/duration; duration 0 is a
///   redraw gate and does not permanently disable compatible entries.
///
/// Engine: Verified — non-zero durations lock compatible entries; duration 0
/// only records the native redraw gate so SYSTEM/SOUND tab groups remain
/// clickable after their page rebuild.
///
/// Decompiler: Verified — renders as btn_lock(group, duration_ms).
static SIG_BTN_LOCK: ExtSig = sig!(8, 19, "btn_lock", pop=2,
    params=["group":0=ButtonSlot, "duration_ms":1=DurationMs],
    return=Void, effects=[ChangesSelectState],
    purpose="Lock input for a button group; second argument is the native duration/timer value.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40E0C0 pops group/duration, sets group lock timer fields, and stores start time only when duration is non-zero", RuntimeTrace:"pal-vm ext_btn_lock pops group/duration; duration 0 no longer locks matching group entries"],
    game="0x0040E0C0");
/// category 8 index 20: btn_unlock
///
/// Purpose: Unlock all button inputs for a group, re-enabling activation.
///
/// VM arguments:
/// - pop[0]: group (ButtonSlot) — button group to unlock.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_40E040 pops group and clears the native group lock timer
///   fields.
/// - RuntimeTrace: pal-vm ext_btn_unlock pops group and unlocks the compatible
///   group.
///
/// Engine: Verified — unlocks all compatible entries in the group.
///
/// Decompiler: Verified — renders as btn_unlock(group).
static SIG_BTN_UNLOCK: ExtSig = sig!(8, 20, "btn_unlock", pop=1,
    params=["group":0=ButtonSlot], return=Void, effects=[ChangesSelectState],
    purpose="Unlock button input for a group.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40E040 pops group and clears native button lock fields", RuntimeTrace:"pal-vm ext_btn_unlock pops group and unlocks matching entries"],
    game="0x0040E040");

/// category 9 index 9: font_system_query_9
///
/// Purpose: Zero-argument font/system query used during the settings/system
/// submenu setup path.  The script at docs/dis.txt 0002D3DC calls it without
/// any preceding push and writes the return value to dst_slot[1].
///
/// VM arguments: none.
///
/// Return: Integer — compatible runtime currently returns 0, matching the old
/// observed fallback value but without corrupting the VM stack.
///
/// Side effects: none.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 0002D3B0..0002D404 has no push before
///   ext_0009_0009 and then passes `!result` to ext_0009_0006.
/// - RuntimeTrace: previous auto fallback popped one stale value every submenu
///   frame, shrinking the stack and breaking system/load/save UI control flow.
///
/// Engine: Verified — pops zero args and returns 0.
///
/// Decompiler: Verified — renders as font_system_query_9().
static SIG_FONT_SYSTEM_QUERY_9: ExtSig = sig!(9, 9, "font_system_query_9", pop=0,
    params=[],
    return=Integer, effects=[],
    purpose="Zero-argument font/system query used by submenu setup; returns a compatibility integer without touching stack.",
    status=Verified, decompiler=Verified,
    evidence=[Disassembly:"docs/dis.txt 0002D3DC calls ext_0009_0009 with no pushed arguments", RuntimeTrace:"DEBUG_VM showed one-value stack leak from the old auto fallback"]);

// Category 12 - system buttons.

/// category 12 index 0: system_btn_set
///
/// Purpose: Configure a native system/menu button (e.g. Save, Load, Auto)
/// in Game.exe's window button table with a sprite resource and a state code.
///
/// VM arguments (display order: index, image, state):
/// - pop[0]: index (ButtonSlot) — system button slot index.
/// - pop[1]: image (ResourceStringFromFileDat) — sprite resource name.
/// - pop[2]: state (Mode) — button state/visibility code.
///
/// Return: void.
///
/// Side effects: ChangesSelectState, MutatesWindow.
///
/// Evidence:
/// - Game.sqlite: sub_439270 pops index/image/state, stores the sprite point
///   and enabled/default state in the system button table, then returns 1.
/// - RuntimeTrace: pal-vm dispatch_system_button_stub index 0 pops 3.
///
/// Engine: Verified — pops and records the system button configuration in the
/// compatible system-button table placeholder.
///
/// Decompiler: Verified — renders as system_btn_set(index, image, state).
static SIG_SYSTEM_BTN_SET: ExtSig = sig!(12, 0, "system_btn_set", pop=3,
    params=["index":0=ButtonSlot, "image":1=ResourceStringFromFileDat, "state":2=Mode],
    return=Void, effects=[ChangesSelectState, MutatesWindow],
    purpose="Configure native system/menu button state in Game.exe's window button table.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_439270 pops index/image/state and updates system button table", RuntimeTrace:"pal-vm dispatch_system_button_stub index 0 pops index/image/state"],
    game="0x00439270");
/// category 12 index 1: system_btn_release
///
/// Purpose: Release/remove a system button entry from the window button table.
///
/// VM arguments:
/// - pop[0]: index (ButtonSlot) — system button slot to release.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite system button handler; pal-vm dispatch_system_button_stub index 1 pops 1
///
/// Engine: Blocked — removes system button entry.
///
/// Decompiler: Blocked — renders as system_btn_release(index).
static SIG_SYSTEM_BTN_RELEASE: ExtSig = sig!(12, 1, "system_btn_release", pop=1,
    params=["index":0=ButtonSlot],
    return=Void, effects=[ChangesSelectState],
    purpose="Release a system/menu button entry.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite system button handler; pal-vm dispatch_system_button_stub index 1 pops 1"]);
/// category 12 index 2: system_btn_enable
///
/// Purpose: Enable or disable a system button's input and visual state.
///
/// VM arguments:
/// - pop[0]: index (ButtonSlot) — system button slot.
/// - pop[1]: enabled (Flag) — 1=enable, 0=disable.
///
/// Return: void.
///
/// Side effects: ChangesSelectState.
///
/// Evidence:
/// - Game.sqlite: sub_439100 pops index/enabled, writes enabled to one slot or
///   all 32 slots when index==0xFFFF, and returns 1.
/// - RuntimeTrace: pal-vm dispatch_system_button_stub index 2 pops 2.
///
/// Engine: Verified — pops the native two-argument shape and preserves status
/// return 1; exact platform window-button drawing remains intentionally
/// portable.
///
/// Decompiler: Verified — renders as system_btn_enable(index, enabled).
static SIG_SYSTEM_BTN_ENABLE: ExtSig = sig!(12, 2, "system_btn_enable", pop=2,
    params=["index":0=ButtonSlot, "enabled":1=Flag],
    return=Void, effects=[ChangesSelectState],
    purpose="Enable/disable a system/menu button entry.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_439100 pops index/enabled and supports index 0xFFFF wildcard", RuntimeTrace:"pal-vm dispatch_system_button_stub index 2 pops index/enabled"],
    game="0x00439100");

// Category 11 - MSprite/movie wrappers. The script table uses category 0x000B;
// PAL itself stores the decoder handle inside sprite object offset +0x14.

/// category 11 index 0: movie_play
///
/// Purpose: Open and begin playing a full-screen or layer movie/MSprite
/// resource.  PAL stores the decoder handle at sprite object offset +0x14.
///
/// VM arguments:
/// - pop[0]: layer (SpriteSlot) — target sprite layer/slot for the movie.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
///
/// Return: void.
///
/// Side effects: CreatesMovie, CreatesSprite.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000B:0000
/// - PAL.sqlite: PalMSpriteLoad 0x1011B39A
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — calls PalMSpriteLoad.
///
/// Decompiler: Blocked — renders as movie_play(layer, name).
static SIG_MOVIE_PLAY: ExtSig = sig!(11, 0, "movie_play", pop=2,
    params=["layer":0=SpriteSlot, "name":1=ResourceStringFromFileDat],
    return=Void, effects=[CreatesMovie, CreatesSprite], purpose="Open/play a full-screen or layer movie resource.",
    status=Blocked, decompiler=Blocked, evidence=[Disassembly:"docs/dis.txt 000B:0000", PalSqlite:"PalMSpriteLoad 0x1011B39A", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteLoad" => "0x1011B39A");
/// category 11 index 1: msp_set_loop_sp_ep
///
/// Purpose: Load a movie/MSprite into a sprite slot and configure loop
/// mode, loop start point, loop end point, and position.
///
/// VM arguments (display order: slot, name, loop_mode, loop_start, loop_end, x, y, z):
/// - pop[0]: slot (SpriteSlot) — target sprite slot.
/// - pop[1]: name (ResourceStringFromFileDat) — File.dat resource name.
/// - pop[2]: loop_mode (Mode) — 0=no loop, 1=loop, etc.
/// - pop[3]: loop_start (Integer) — loop start frame.
/// - pop[4]: loop_end (Integer) — loop end frame.
/// - pop[5]: x (CoordinateX) — display position x.
/// - pop[6]: y (CoordinateY) — display position y.
/// - pop[7]: z (CoordinateZ) — depth/priority.
///
/// Return: void.
///
/// Side effects: CreatesMovie, CreatesSprite, MutatesSprite.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000B:0001
/// - PAL.sqlite: PalMSpriteSetLoopPoint 0x1011B61A / PalMSpriteSetLoop 0x1011E923
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — calls PalMSpriteLoad + PalMSpriteSetLoopPoint.
///
/// Decompiler: Blocked — renders 8 args in display order.
static SIG_MSP_SET_LOOP_SP_EP: ExtSig = sig!(11, 1, "msp_set_loop_sp_ep", pop=8,
    params=["slot":0=SpriteSlot, "name":1=ResourceStringFromFileDat, "loop_mode":2=Mode, "loop_start":3=Integer, "loop_end":4=Integer, "x":5=CoordinateX, "y":6=CoordinateY, "z":7=CoordinateZ],
    return=Void, effects=[CreatesMovie, CreatesSprite, MutatesSprite], purpose="Load an MSprite/movie into a sprite slot and configure loop/start/end points.",
    status=Blocked, decompiler=Blocked, evidence=[Disassembly:"docs/dis.txt 000B:0001", PalSqlite:"PalMSpriteSetLoopPoint 0x1011B61A / PalMSpriteSetLoop 0x1011E923", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteSetLoopPoint" => "0x1011B61A");
/// category 11 index 2: msp_cls
///
/// Purpose: Stop and release an MSprite/movie slot.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — movie slot to release.
///
/// Return: void.
///
/// Side effects: DeletesSprite.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000B:0002
/// - PAL.sqlite: PalMSpriteStop 0x10119C7F
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — calls PalMSpriteStop.
///
/// Decompiler: Blocked — renders as msp_cls(slot).
static SIG_MSP_CLS: ExtSig = sig!(11, 2, "msp_cls", pop=1,
    params=["slot":0=SpriteSlot], return=Void, effects=[DeletesSprite], purpose="Release an MSprite/movie slot.",
    status=Blocked, decompiler=Blocked, evidence=[Disassembly:"docs/dis.txt 000B:0002", PalSqlite:"PalMSpriteStop 0x10119C7F", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteStop" => "0x10119C7F");
/// category 11 index 3: msp_wait
///
/// Purpose: Block the script until the MSprite/movie slot finishes (native
/// "finished" bit set in PalMSpriteGetState).
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — movie slot to wait on.
///
/// Return: void.
///
/// Side effects: BlocksScript.
///
/// Evidence:
/// - GameSqlite: sub_42E240 stores the pending MSprite slot pointer, disables
///   loop with PalMSpriteSetLoop(handle, 0), and rewinds ctx[201015] by 12 until
///   PalMSpriteGetState(handle) has bit 4 or skip/auto is active.
/// - Disassembly: docs/dis.txt 000B:0003
/// - PAL.sqlite: PalMSpriteGetState 0x10120BD3
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — portable runtime mirrors first-pass pop plus PC rewind
/// polling against MSpriteSystem state.
///
/// Decompiler: Blocked — renders as msp_wait(slot).
static SIG_MSP_WAIT: ExtSig = sig!(11, 3, "msp_wait", pop=1,
    params=["slot":0=SpriteSlot], return=Void, effects=[BlocksScript], purpose="Wait until an MSprite/movie slot reaches the native finished bit.",
    status=Blocked, decompiler=Blocked, evidence=[GameSqlite:"reverse/Game.sqlite sub_42E240", Disassembly:"docs/dis.txt 000B:0003", PalSqlite:"PalMSpriteGetState 0x10120BD3", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteGetState" => "0x10120BD3");
/// category 11 index 4: msp_lock
///
/// Purpose: Pause/lock frame advancement for an MSprite/movie slot.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — movie slot to lock.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000B:0004
/// - PAL.sqlite: PalMSpriteLock 0x1011A86E
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — calls PalMSpriteLock.
///
/// Decompiler: Blocked — renders as msp_lock(slot).
static SIG_MSP_LOCK: ExtSig = sig!(11, 4, "msp_lock", pop=1,
    params=["slot":0=SpriteSlot], return=Void, effects=[MutatesSprite], purpose="Lock/pause decoder frame advancement.",
    status=Blocked, decompiler=Blocked, evidence=[Disassembly:"docs/dis.txt 000B:0004", PalSqlite:"PalMSpriteLock 0x1011A86E", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteLock" => "0x1011A86E");
/// category 11 index 5: msp_unlock
///
/// Purpose: Resume/unlock frame advancement for an MSprite/movie slot.
///
/// VM arguments:
/// - pop[0]: slot (SpriteSlot) — movie slot to unlock.
///
/// Return: void.
///
/// Side effects: MutatesSprite.
///
/// Evidence:
/// - Disassembly: docs/dis.txt 000B:0005
/// - PAL.sqlite: PalMSpriteUnlock 0x10120CD2
/// - Writeup: docs/writeup.md MSprite
///
/// Engine: Blocked — calls PalMSpriteUnlock.
///
/// Decompiler: Blocked — renders as msp_unlock(slot).
static SIG_MSP_UNLOCK: ExtSig = sig!(11, 5, "msp_unlock", pop=1,
    params=["slot":0=SpriteSlot], return=Void, effects=[MutatesSprite], purpose="Unlock/resume decoder frame advancement.",
    status=Blocked, decompiler=Blocked, evidence=[Disassembly:"docs/dis.txt 000B:0005", PalSqlite:"PalMSpriteUnlock 0x10120CD2", Writeup:"docs/writeup.md MSprite"],
    pal="PalMSpriteUnlock" => "0x10120CD2");

// Category 17 - action/tween scheduler.

/// category 17 index 0: action_run_count_over
///
/// Purpose: Start or poll an asynchronous action counter without blocking the
/// VM.  Returns non-zero if the counter has elapsed.
///
/// VM arguments:
/// - pop[0]: action_id (Integer) — action slot/identifier, or -1 for active.
/// - pop[1]: duration_ms (DurationMs) — counter duration.
///
/// Return: Status — native handler returns 1 after scheduling, or 1 for
/// invalid action ids after logging.
///
/// Side effects: CreatesTask.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action handler decompilation around action_run_count_over
/// - RuntimeTrace: pal-vm dispatch_action_stub index 0 pops 2
///
/// Engine: Verified — starts a nonblocking compatible action timer, stores the
/// active action id, and returns native status 1.
///
/// Decompiler: Verified — renders as action_run_count_over(action_id, duration_ms).
static SIG_ACTION_RUN_COUNT_OVER: ExtSig = sig!(17, 0, "action_run_count_over", pop=2,
    params=["action_id":0=Integer, "duration_ms":1=DurationMs],
    return=Status, effects=[CreatesTask],
    purpose="Start/check an asynchronous action counter without blocking.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40B6E0 pops action_id then duration, schedules action state, and returns 1", RuntimeTrace:"pal-vm dispatch_action_stub index 0 pops 2 and returns native status 1"],
    game="0x0040B6E0");
/// category 17 index 1: action_sync_run_count_over
///
/// Purpose: Start an action counter and block the script until the duration
/// elapses.  Synchronous variant of action_run_count_over.
///
/// VM arguments:
/// - pop[0]: action_id (Integer) — action slot/identifier, or -1 for active.
/// - pop[1]: duration_ms (DurationMs) — counter duration.
///
/// Return: Bool — non-zero on completion.
///
/// Side effects: CreatesTask, BlocksScript.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action handler decompilation around action_sync_run_count_over
/// - RuntimeTrace: pal-vm dispatch_action_stub index 1 pops 2
///
/// Engine: Verified — schedules the compatible action timer and returns a VM
/// wait request for the decoded duration.
///
/// Decompiler: Verified — renders as action_sync_run_count_over(action_id, duration_ms).
static SIG_ACTION_SYNC_RUN_COUNT_OVER: ExtSig = sig!(17, 1, "action_sync_run_count_over", pop=2,
    params=["action_id":0=Integer, "duration_ms":1=DurationMs],
    return=Bool, effects=[CreatesTask, BlocksScript],
    purpose="Start an action counter and block until the duration elapses.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40B5C0 pops two values, schedules action state, sets blocking flag, and logs action_sync_run", RuntimeTrace:"pal-vm dispatch_action_stub index 1 pops 2 and returns WaitRequest::Time"],
    game="0x0040B5C0");
/// category 17 index 3: action_clear_count_over
///
/// Purpose: Clear/reset an action counter.  action_id=-1 clears the current
/// active counter; otherwise clears the indexed counter.
///
/// VM arguments:
/// - pop[0]: action_id (Integer) — action id, or -1 for current.
///
/// Return: void.
///
/// Side effects: CreatesTask (counter teardown).
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sub_40B430 pseudocode "action_clear count over" and one stack pop
/// - RuntimeTrace: pal-vm dispatch_action_stub index 3 pops 1
///
/// Engine: Verified — clears the current compatible action counter when
/// action_id is -1 and preserves native status return 1.
///
/// Decompiler: Verified — renders as action_clear_count_over(action_id).
static SIG_ACTION_CLEAR_COUNT_OVER: ExtSig = sig!(17, 3, "action_clear_count_over", pop=1,
    params=["action_id":0=Integer],
    return=Void, effects=[CreatesTask],
    purpose="-1 clears the current action counter; otherwise clear/check the indexed action counter.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40B430 pops one action id, resolves -1 to active id, clears action memory, and returns 1", RuntimeTrace:"pal-vm dispatch_action_stub index 3 pops 1 and clears compatible action state"],
    game="0x0040B430");
/// category 17 index 5: ext_0011_0005 (action tween, 6-arg)
///
/// Purpose: Configure a six-argument Game.exe action/tween entry.  Used for
/// scene animation timelines.  Canonical name pending full reverse.
///
/// VM arguments (display order: action_id, duration_ms, from, to, mode, flags):
/// - pop[0]: action_id (Integer) — action slot identifier.
/// - pop[1]: duration_ms (DurationMs) — tween duration.
/// - pop[2]: from (Integer) — start value.
/// - pop[3]: to (Integer) — end value.
/// - pop[4]: mode (Mode) — interpolation mode.
/// - pop[5]: flags (Flag) — tween flags.
///
/// Return: void.
///
/// Side effects: CreatesTask, MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action dispatch group 5/6/10/14/15/29 pops 6
/// - RuntimeTrace: pal-vm dispatch_action_stub index 5 pops 6
///
/// Engine: Blocked — stores 6-arg tween entry.
///
/// Decompiler: Blocked — renders 6 args.
///
/// Open points: Canonical function name not confirmed.
static SIG_ACTION_TIMELINE_5: ExtSig = sig!(17, 5, "ext_0011_0005", pop=6,
    params=["action_id":0=Integer, "duration_ms":1=DurationMs, "from":2=Integer, "to":3=Integer, "mode":4=Mode, "flags":5=Flag],
    return=Void, effects=[CreatesTask, MutatesSprite],
    purpose="Configure a six-argument Game.exe action/tween entry.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite action dispatch group 5/6/10/14/15/29 pops 6", RuntimeTrace:"pal-vm dispatch_action_stub index 5 pops 6"]);
/// category 17 index 8: action_alpha_delta
///
/// Purpose: Configure a timed alpha-delta action for a sprite slot.  The
/// shared title/system transition helper at point[3042] uses this to fade the
/// temporary slot-64 layer before clearing it.
///
/// VM arguments (display/source order: action_id, sprite_slot, alpha_delta, duration_ms):
/// - pop[0]: action_id (Integer) — action slot, usually 0 for wrapper fades.
/// - pop[1]: sprite_slot (Integer) — Game sprite slot to mutate.
/// - pop[2]: alpha_delta (Integer) — signed delta added to current alpha.
/// - pop[3]: duration_ms (DurationMs) — tween duration.
///
/// Return: void.
///
/// Side effects: CreatesTask, MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite `sub_446650` action bytecode case 3
///   constructs the `sub_402F30` timed color-lane delta, and
///   `sub_4494D0`/`sub_4498D0` clamp the lane into PalSpriteSetColor.
/// - PAL.sqlite: PalSpriteSetColor 0x10119103.
/// - RuntimeTrace: point[3042] pushes `1000, -255, 64, 0`, which pops as
///   action 0, slot 64, delta -255, duration 1000.
///
/// Engine: Verified — schedules the action timer and tweens compatible sprite
/// alpha from current to current+delta.
///
/// Decompiler: Verified — renders explicit action id, sprite slot, alpha delta,
/// and duration arguments.
static SIG_ACTION_TIMELINE_8: ExtSig = sig!(17, 8, "action_alpha_delta", pop=4,
    params=["action_id":0=Integer, "sprite_slot":1=SpriteSlot, "alpha_delta":2=Integer, "duration_ms":3=DurationMs],
    return=Void, effects=[CreatesTask, MutatesSprite],
    purpose="Configure a timed sprite alpha-delta action.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_446650 case 3 builds sub_402F30 timed alpha delta and sub_4494D0/sub_4498D0 commit through PalSpriteSetColor", PalSqlite:"PalSpriteSetColor 0x10119103", RuntimeTrace:"point[3042] pushes 1000,-255,64,0 before ext_0011_0008"]);
/// category 17 index 23: set_active_action
///
/// Purpose: Set the active action id that is used by subsequent action
/// query/clear calls when they receive action_id=-1.
///
/// VM arguments:
/// - pop[0]: action_id (Integer) — action id to set as current.
///
/// Return: void.
///
/// Side effects: CreatesTask (updates active action state).
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action handler set_active_action path
/// - RuntimeTrace: pal-vm dispatch_action_stub index 23 pops 1
///
/// Engine: Verified — stores action_id as current active action for later
/// -1 resolution in action clear/query calls.
///
/// Decompiler: Verified — renders as set_active_action(action_id).
static SIG_ACTION_SET_ACTIVE: ExtSig = sig!(17, 23, "set_active_action", pop=1,
    params=["action_id":0=Integer],
    return=Void, effects=[CreatesTask],
    purpose="Set the active action id used by later action query/clear calls.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_4095C0 pops action_id, validates <16, stores old/current active ids, and logs set_active_action", RuntimeTrace:"pal-vm dispatch_action_stub index 23 pops action_id and updates active action"],
    game="0x004095C0");
/// category 17 index 9: ext_0011_0009 (action 2-arg)
///
/// Purpose: Two-argument action helper; sets active action from the second
/// argument after receiving a value in the first.
///
/// VM arguments:
/// - pop[0]: value (Integer) — first argument; purpose not fully reversed.
/// - pop[1]: action_id (Integer) — action id to set active.
///
/// Return: void.
///
/// Side effects: CreatesTask.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action dispatch index 9 pops 2
/// - RuntimeTrace: pal-vm dispatch_action_stub index 9 pops 2
///
/// Engine: Blocked — stores 2-arg action entry.
///
/// Decompiler: Blocked — renders as ext_0011_0009(value, action_id).
///
/// Open points: Canonical name and exact role of pop[0] not confirmed.
static SIG_ACTION_TIMELINE_9: ExtSig = sig!(17, 9, "ext_0011_0009", pop=2,
    params=["value":0=Integer, "action_id":1=Integer],
    return=Void, effects=[CreatesTask],
    purpose="Set active action from the second argument after a two-argument action helper.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite action dispatch index 9 pops 2", RuntimeTrace:"pal-vm dispatch_action_stub index 9 pops 2"]);
/// category 17 index 11: ext_0011_000B (action tween, 4-arg)
///
/// Purpose: Four-argument action/tween helper sharing the index-7 dispatch
/// path.  Canonical name pending full reverse.
///
/// VM arguments: same layout as ext_0011_0008 (index 8).
/// - pop[0]: duration_ms (DurationMs)
/// - pop[1]: action_id (Integer)
/// - pop[2]: arg2 (Integer)
/// - pop[3]: arg3 (Integer)
///
/// Return: void.
///
/// Side effects: CreatesTask, MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action dispatch index 11 pops 4
/// - RuntimeTrace: pal-vm dispatch_action_stub index 11 pops 4
///
/// Engine: Blocked — stores 4-arg tween entry.
///
/// Decompiler: Blocked — renders 4 args.
static SIG_ACTION_TIMELINE_11: ExtSig = sig!(17, 11, "ext_0011_000B", pop=4,
    params=["duration_ms":0=DurationMs, "action_id":1=Integer, "arg2":2=Integer, "arg3":3=Integer],
    return=Void, effects=[CreatesTask, MutatesSprite],
    purpose="Four-argument action/tween helper sharing the index-7 path.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite action dispatch index 11 pops 4", RuntimeTrace:"pal-vm dispatch_action_stub index 11 pops 4"]);
/// category 17 index 29: ext_0011_001D (action tween, 6-arg)
///
/// Purpose: Six-argument action/tween helper sharing the index-5 dispatch
/// path.  Canonical name pending full reverse.
///
/// VM arguments: same layout as ext_0011_0005 (index 5).
/// - pop[0]: action_id (Integer)
/// - pop[1]: duration_ms (DurationMs)
/// - pop[2]: from (Integer)
/// - pop[3]: to (Integer)
/// - pop[4]: mode (Mode)
/// - pop[5]: flags (Flag)
///
/// Return: void.
///
/// Side effects: CreatesTask, MutatesSprite.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite action dispatch index 29 pops 6
/// - RuntimeTrace: pal-vm dispatch_action_stub index 29 pops 6
///
/// Engine: Blocked — stores 6-arg tween entry.
///
/// Decompiler: Blocked — renders 6 args.
static SIG_ACTION_TIMELINE_29: ExtSig = sig!(17, 29, "ext_0011_001D", pop=6,
    params=["action_id":0=Integer, "duration_ms":1=DurationMs, "from":2=Integer, "to":3=Integer, "mode":4=Mode, "flags":5=Flag],
    return=Void, effects=[CreatesTask, MutatesSprite],
    purpose="Six-argument action/tween helper sharing the index-5 path.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite action dispatch index 29 pops 6", RuntimeTrace:"pal-vm dispatch_action_stub index 29 pops 6"]);
/// category 17 index 30: set_action_clear
///
/// Purpose: Clear/reset an action entry.  action_id=-1 clears the current
/// active action state; otherwise sets/clears the indexed action flag.
///
/// VM arguments:
/// - pop[0]: action_id (Integer) — action id, or -1 for current.
///
/// Return: void.
///
/// Side effects: CreatesTask.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite sub_40B3A0 debug string "set_action_clear" and one stack pop
/// - RuntimeTrace: pal-vm dispatch_action_stub index 30 pops 1
///
/// Engine: Verified — resolves -1 to the active action id and sets the clear
/// marker for that action in the compatible action state.
///
/// Decompiler: Verified — renders as set_action_clear(action_id).
static SIG_ACTION_SET_CLEAR: ExtSig = sig!(17, 30, "set_action_clear", pop=1,
    params=["action_id":0=Integer],
    return=Void, effects=[CreatesTask],
    purpose="-1 clears current action state; otherwise sets/clears the indexed action flag.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_40B3A0 pops one action id, resolves -1 to active id, marks action clear, and returns 1", RuntimeTrace:"pal-vm dispatch_action_stub index 30 pops 1 and updates active action clear state"],
    game="0x0040B3A0");

// Category 18 - INI/file startup path.

/// category 18 index 6: arg_get
///
/// Purpose: Read a value from the current Game.exe extcall argument stack by
/// one-based index.  Used by wrapper scripts that need to re-inspect the
/// argument block of the calling extcall without re-popping it.
///
/// VM arguments:
/// - pop[0]: index (Integer) — 1-based argument position; runtime passes
///   index+1 to extcall_arg().
///
/// Return: Integer — argument value at position, or 0 if out of range.
///
/// Side effects: none (read-only inspection of argument frame).
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite argument-stack helper sub
/// - RuntimeTrace: pal-vm ext_arg_get pops 1 and returns extcall_arg(index+1)
///
/// Engine: Blocked — pops index and returns current extcall arg frame value.
///
/// Decompiler: Blocked — renders as arg_get(index).
static SIG_ARG_GET: ExtSig = sig!(18, 6, "arg_get", pop=1,
    params=["index":0=Integer],
    return=Integer, effects=[],
    purpose="Read a value from the current Game.exe extcall argument stack by index.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite argument-stack helper", RuntimeTrace:"pal-vm ext_arg_get pops 1 and returns extcall_arg(index+1)"]);
/// category 18 index 30: openfile
///
/// Purpose: Open a PAC-resident resource file by its resolved script string
/// name.  Returns an integer handle used by read_file / set_file_pointer.
/// Primary use is font loading (FONT*.DAT) during engine startup.
///
/// VM arguments:
/// - pop[0]: name (TextStringFromTextDat) — NUL-terminated resource filename
///   resolved from Text.dat.
///
/// Return: Handle — non-zero file handle on success; 0 on failure.
///
/// Side effects: ReadsFile — opens a resource entry in the PAC archive.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.9
/// - RuntimeTrace: pal-vm ext_openfile pops 1, opens PAC entry, returns handle
///
/// Engine: Blocked — opens PAC-resident resource; handle is opaque integer.
///
/// Decompiler: Blocked — renders as openfile(name).
///
/// Open points: Handle lifetime / close mechanism not reversed.
static SIG_OPENFILE: ExtSig = sig!(18, 30, "openfile", pop=1,
    params=["name":0=TextStringFromTextDat], return=Handle, effects=[ReadsFile],
    purpose="Open resource/file by resolved script string; returns runtime file handle or 0.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.9"]);
/// category 18 index 31: read_file
///
/// Purpose: Read bytes from an open resource handle into the VM's temporary
/// memory buffer.  Used by the font loader after openfile to pull glyph data.
///
/// VM arguments (display order: handle, temp_offset, count):
/// - pop[0]: handle (Handle) — open file handle from openfile.
/// - pop[1]: temp_offset (BufferPointer) — destination offset in VM temp buffer.
/// - pop[2]: count (Integer) — number of bytes to read.
///
/// Return: Integer — bytes actually read, or -1 on error.
///
/// Side effects: ReadsFile, WritesVmMemory — reads from resource and writes
/// into internal VM temp/scratch buffer.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.9
/// - RuntimeTrace: pal-vm ext_read_file pops 3
///
/// Engine: Blocked — reads from open handle into vm scratch buffer.
///
/// Decompiler: Blocked — renders as read_file(handle, temp_offset, count).
static SIG_READ_FILE: ExtSig = sig!(18, 31, "read_file", pop=3,
    params=["handle":0=Handle, "temp_offset":1=BufferPointer, "count":2=Integer],
    return=Integer, effects=[ReadsFile, WritesVmMemory], purpose="Read file/table bytes into VM temp memory.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.9"]);
/// category 18 index 33: set_file_pointer
///
/// Purpose: Seek an open resource handle to a given position.  Mirror of
/// Win32 SetFilePointer; origin constants match: 0=begin, 1=current, 2=end.
///
/// VM arguments (display order: handle, offset, origin):
/// - pop[0]: handle (Handle) — open file handle from openfile.
/// - pop[1]: offset (Integer) — seek offset (signed).
/// - pop[2]: origin (Mode) — 0=FILE_BEGIN, 1=FILE_CURRENT, 2=FILE_END.
///
/// Return: Integer — new file position from the beginning, or -1 on error.
///
/// Side effects: ReadsFile — mutates seek position of open handle.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.9
/// - RuntimeTrace: pal-vm ext_set_file_pointer pops 3
///
/// Engine: Blocked — delegates to internal seek on PAC resource handle.
///
/// Decompiler: Blocked — renders as set_file_pointer(handle, offset, origin).
static SIG_SET_FILE_POINTER: ExtSig = sig!(18, 33, "set_file_pointer", pop=3,
    params=["handle":0=Handle, "offset":1=Integer, "origin":2=Mode],
    return=Integer, effects=[ReadsFile], purpose="Seek an opened runtime file handle.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.9"]);
/// category 18 index 36: sz_buf
///
/// Purpose: Read an INI string value into the VM's dynamic string store.
/// Mirrors Win32 GetPrivateProfileString.  Non-.ini filename arguments are
/// normalized to SYSTEM.INI by ini_filename_or_system().
///
/// VM arguments (display order: section, key, default, filename, buffer_or_size):
/// - pop[0]: section (TextStringFromTextDat) — INI section name.
/// - pop[1]: key (TextStringFromTextDat) — INI key name.
/// - pop[2]: default (TextStringFromTextDat) — value returned when key absent.
/// - pop[3]: filename (IniFilename) — INI filename; non-.ini → SYSTEM.INI at
///   runtime.  Decompiler annotates with --[[runtime uses SYSTEM.INI: not *.ini]].
/// - pop[4]: buffer_or_size (BufferPointer) — destination dynamic string slot
///   or size hint.
///
/// Return: StringId — dynamic string id holding the result, or 0.
///
/// Side effects: ReadsIni, WritesVmMemory.
///
/// Evidence:
/// - Writeup: docs/writeup.md INI
/// - RuntimeTrace: pal-vm ext_sz_buf pops 5
///
/// Engine: Blocked — reads SYSTEM.INI (or redirected file) and stores result
/// in dynamic string buffer.
///
/// Decompiler: Blocked — IniFilename param renders with SYSTEM.INI annotation.
static SIG_SZ_BUF: ExtSig = sig!(18, 36, "sz_buf", pop=5,
    params=["section":0=TextStringFromTextDat, "key":1=TextStringFromTextDat, "default":2=TextStringFromTextDat, "filename":3=IniFilename, "buffer_or_size":4=BufferPointer],
    return=StringId, effects=[ReadsIni], purpose="Read INI string into dynamic string storage.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md INI", RuntimeTrace:"pal-vm ext_sz_buf pops 5"]);
/// category 18 index 37: getprivateprofileint
///
/// Purpose: Read an integer value from SYSTEM.INI (or a redirected .ini file).
/// PRIMARY RUNTIME BLOCKER: this extcall must return real SYSTEM.INI values
/// for Game.exe to proceed past the startup configuration phase.
/// Non-.ini filename arguments are normalized to SYSTEM.INI by
/// ini_filename_or_system() in the runtime.
///
/// VM arguments (display order: section, key, default, filename):
/// - pop[0]: section (TextStringFromTextDat) — INI section, e.g. "Config".
/// - pop[1]: key (TextStringFromTextDat) — INI key, e.g. "VoiceVolume".
/// - pop[2]: default (Integer) — fallback value when key is absent.
/// - pop[3]: filename (IniFilename) — INI filename; non-.ini strings such as
///   "HIP_BACK_G" are silently remapped to SYSTEM.INI at runtime.  Decompiler
///   annotates with --[[runtime uses SYSTEM.INI: not *.ini]].
///
/// Return: Integer — parsed integer from INI entry, or default.
///
/// Side effects: ReadsIni.
///
/// Evidence:
/// - Writeup: docs/writeup.md 24.9
/// - RuntimeTrace: pal-vm ext_get_private_profile_int pops 4, uses ini_filename_or_system()
///
/// Engine: Blocked — reads live SYSTEM.INI via Rust ini parser; filename
/// normalization is implemented.
///
/// Decompiler: Blocked — IniFilename param renders with SYSTEM.INI annotation
/// when applicable.
static SIG_GETPRIVATEPROFILEINT: ExtSig = sig!(18, 37, "getprivateprofileint", pop=4,
    params=["section":0=TextStringFromTextDat, "key":1=TextStringFromTextDat, "default":2=Integer, "filename":3=IniFilename],
    return=Integer, effects=[ReadsIni], purpose="Read an integer from SYSTEM.INI or requested INI.",
    status=Blocked, decompiler=Blocked, evidence=[Writeup:"docs/writeup.md 24.9"]);

// Category 10 - save/load UI state.

/// category 10 index 12: save_thumbnail_mosaic_set
///
/// Purpose: Set a flag controlling whether the save-slot thumbnail is rendered
/// with mosaic/pixelation processing during save UI display.
///
/// VM arguments:
/// - pop[0]: enabled (Flag) — 1 to enable mosaic on thumbnail, 0 to disable.
///
/// Return: void.
///
/// Side effects: ChangesSaveState.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite save dispatch index 12 pops 1
/// - RuntimeTrace: pal-vm dispatch_save_stub index 12 pops 1
///
/// Engine: Blocked — stores mosaic flag for save UI rendering.
///
/// Decompiler: Blocked — renders as save_thumbnail_mosaic_set(enabled).
static SIG_SAVE_THUMBNAIL_MOSAIC_SET: ExtSig = sig!(10, 12, "save_thumbnail_mosaic_set", pop=1,
    params=["enabled":0=Flag],
    return=Void, effects=[ChangesSaveState],
    purpose="Set save thumbnail mosaic/processing flag used by save UI rendering.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite save dispatch index 12 pops 1", RuntimeTrace:"pal-vm dispatch_save_stub index 12 pops 1"]);

// Category 16 - window/effect helpers.

/// category 16 index 1: ext_0010_0001 (window/effect helper)
///
/// Purpose: Four-argument window/effect setup helper used by Game.exe UI
/// effect initialization.  Canonical name and exact effect type not reversed.
///
/// VM arguments (display order: x, y, w, h):
/// - pop[0]: x (CoordinateX) — region or effect x origin.
/// - pop[1]: y (CoordinateY) — region or effect y origin.
/// - pop[2]: w (Integer) — width parameter.
/// - pop[3]: h (Integer) — height parameter.
///
/// Return: void.
///
/// Side effects: MutatesWindow.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite window/effect dispatch category 16 index 1 pops 4
/// - RuntimeTrace: pal-vm dispatch_window_effect_stub index 1 pops 4
///
/// Engine: Blocked — pops 4 args; window/effect state stored.
///
/// Decompiler: Blocked — renders as ext_0010_0001(x, y, w, h).
///
/// Open points: Canonical name and effect semantics not confirmed.
static SIG_WINDOW_EFFECT_1: ExtSig = sig!(16, 1, "ext_0010_0001", pop=4,
    params=["x":0=CoordinateX, "y":1=CoordinateY, "w":2=Integer, "h":3=Integer],
    return=Void, effects=[MutatesWindow],
    purpose="Four-argument window/effect helper used by Game.exe UI effect setup.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite window/effect dispatch category 16 index 1 pops 4", RuntimeTrace:"pal-vm dispatch_window_effect_stub index 1 pops 4"]);
/// category 16 index 2: ext_0010_0002 (window/effect helper)
///
/// Purpose: Same 4-arg window/effect helper at index 2.  May configure a
/// different effect layer or parameter set from index 1.
///
/// VM arguments: identical to ext_0010_0001 (index 1).
/// - pop[0]: x (CoordinateX)
/// - pop[1]: y (CoordinateY)
/// - pop[2]: w (Integer)
/// - pop[3]: h (Integer)
///
/// Return: void.
///
/// Side effects: MutatesWindow.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite window/effect dispatch category 16 index 2 pops 4
/// - RuntimeTrace: pal-vm dispatch_window_effect_stub index 2 pops 4
///
/// Engine: Blocked — pops 4 args.
///
/// Decompiler: Blocked — renders as ext_0010_0002(x, y, w, h).
///
/// Open points: Canonical name and difference from index 1 not confirmed.
static SIG_WINDOW_EFFECT_2: ExtSig = sig!(16, 2, "ext_0010_0002", pop=4,
    params=["x":0=CoordinateX, "y":1=CoordinateY, "w":2=Integer, "h":3=Integer],
    return=Void, effects=[MutatesWindow],
    purpose="Four-argument window/effect helper used by Game.exe UI effect setup.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite window/effect dispatch category 16 index 2 pops 4", RuntimeTrace:"pal-vm dispatch_window_effect_stub index 2 pops 4"]);

// Category 20 - random.

/// category 20 index 0: random
///
/// Purpose: Return a uniformly distributed random integer in the inclusive
/// range [min, max].
///
/// VM arguments:
/// - pop[0]: min (Integer) — lower bound (inclusive).
/// - pop[1]: max (Integer) — upper bound (inclusive).
///
/// Return: Integer — random value in [min, max].
///
/// Side effects: none.
///
/// Evidence:
/// - Game.sqlite: reverse/Game.sqlite random wrapper
/// - RuntimeTrace: pal-vm dispatch_random_ext index 0 pops 2
///
/// Engine: Blocked — uses Rust rand to generate inclusive range result.
///
/// Decompiler: Blocked — renders as random(min, max).
static SIG_RANDOM: ExtSig = sig!(20, 0, "random", pop=2,
    params=["min":0=Integer, "max":1=Integer],
    return=Integer, effects=[],
    purpose="Return an inclusive random integer between the two popped bounds.",
    status=Blocked, decompiler=Blocked,
    evidence=[GameSqlite:"reverse/Game.sqlite random wrapper", RuntimeTrace:"pal-vm dispatch_random_ext index 0 pops 2"]);

// Category 22 - run/effect transition state.

/// category 22 index 0: run
///
/// Purpose: Execute a scene/effect transition and wait for it to complete.
/// Blocks the script until the named effect finishes.
///
/// VM arguments (display order: effect_id, arg1, arg2):
/// - pop[0]: effect_id (Integer) — transition effect identifier.
/// - pop[1]: arg1 (Integer) — effect-specific parameter 1.
/// - pop[2]: arg2 (Integer) — effect-specific parameter 2.
///
/// Return: void.
///
/// Side effects: ChangesRunState, BlocksScript.
///
/// Evidence:
/// - Game.sqlite: sub_421FD0 pops effect_id/arg1/arg2, validates
///   effect_id<=100, calls PalEffectEx(effect_id,arg1,arg2,0), updates run
///   timing fields, and returns 1 (0 only on invalid/blocked native setup).
/// - PAL.sqlite: PalEffectEx 0x1011A1E8.
/// - RuntimeTrace: pal-vm dispatch_run_ext index 0 pops 3 and returns 1 after
///   setting the compatible transition pipeline.
///
/// Engine: Verified — stores the effect triple in the compatible run pipeline,
/// blocks through WaitRequest::Time, and mirrors native queued run-stack mode.
///
/// Decompiler: Verified — renders as run(effect_id, arg1, arg2).
static SIG_RUN: ExtSig = sig!(22, 0, "run", pop=3,
    params=["effect_id":0=Integer, "arg1":1=Integer, "arg2":2=Integer],
    return=Void, effects=[ChangesRunState, BlocksScript], purpose="Run a scene/effect transition and wait for it.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_421FD0 pops effect_id/arg1/arg2 and calls PalEffectEx(effect_id,arg1,arg2,0)", PalSqlite:"PalEffectEx 0x1011A1E8", RuntimeTrace:"pal-vm dispatch_run_ext index 0 pops 3 and updates run pipeline"],
    game="0x00421FD0", pal="PalEffectEx" => "0x1011A1E8");
/// category 22 index 1: run_no_wait
///
/// Purpose: Latch or flush a no-wait scene transition.  Does not pop any
/// transition parameters; the transition parameters are expected to have been
/// configured by a prior extcall.
///
/// VM arguments: none.
///
/// Return: void.
///
/// Side effects: ChangesRunState.
///
/// Evidence:
/// - Game.sqlite: sub_421F80 has no VM stack pop, toggles the native no-wait
///   transition latch, and either queues run-stack state or calls the run setup
///   helper.
/// - RuntimeTrace: pal-vm dispatch_run_ext index 1 pops 0 and latches the
///   compatible no-wait transition.
///
/// Engine: Verified — marks the compatible transition as no-wait without
/// consuming arguments.
///
/// Decompiler: Verified — renders as run_no_wait().
static SIG_RUN_NO_WAIT: ExtSig = sig!(22, 1, "run_no_wait", pop=0,
    params=[],
    return=Void, effects=[ChangesRunState], purpose="Latch/flush a no-wait scene transition; Game.exe category 22 index 1 does not pop transition parameters.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_421F80 has no VM pop and toggles run_no_wait transition state", RuntimeTrace:"pal-vm dispatch_run_ext index 1 pops 0 and sets no_wait_latch"],
    game="0x00421F80");
/// category 22 index 2: run_stack
///
/// Purpose: Toggle Game.exe run-stack transition mode.  When enabled, the
/// engine stacks effect layers rather than replacing them.
///
/// VM arguments:
/// - pop[0]: enabled (Flag) — 1 to enable stack mode, 0 to disable.
///
/// Return: void.
///
/// Side effects: ChangesRunState.
///
/// Evidence:
/// - Game.sqlite: sub_422160 pops enabled, toggles native run-stack mode, and
///   flushes a queued run transition through PalEffectEx when disabling.
/// - PAL.sqlite: PalEffectEx 0x1011A1E8.
/// - RuntimeTrace: pal-vm dispatch_run_ext index 2 pops one flag and flushes
///   the compatible pending run pipeline.
///
/// Engine: Verified — toggles compatible stack mode and flushes queued
/// transition state according to native sub_422160.
///
/// Decompiler: Verified — renders as run_stack(enabled).
static SIG_RUN_STACK: ExtSig = sig!(22, 2, "run_stack", pop=1,
    params=["enabled":0=Flag], return=Void, effects=[ChangesRunState], purpose="Toggle Game.exe run-stack transition mode.",
    status=Verified, decompiler=Verified,
    evidence=[GameSqlite:"reverse/Game.sqlite sub_422160 pops enabled, toggles run-stack, and conditionally calls PalEffectEx for queued effects", PalSqlite:"PalEffectEx 0x1011A1E8", RuntimeTrace:"pal-vm dispatch_run_ext index 2 pops enabled and flushes pending transition"],
    game="0x00422160", pal="PalEffectEx" => "0x1011A1E8");

pub fn lookup_sig(category: u16, index: u16) -> Option<&'static ExtSig> {
    match (category, index) {
        (2, 2) => Some(&SIG_TEXT),
        (2, 3) => Some(&SIG_TEXT_HIDE),
        (2, 4) => Some(&SIG_TEXT_SHOW),
        (2, 8) => Some(&SIG_TEXT_CLEAR),
        (2, 15) => Some(&SIG_TEXT_W),
        (2, 16) => Some(&SIG_TEXT_A),
        (2, 17) => Some(&SIG_TEXT_WA),
        (2, 22) => Some(&SIG_TEXT_SET_BASE),
        (3, 2) => Some(&SIG_SP_SET),
        (3, 3) => Some(&SIG_SP_SET_EX),
        (3, 4) => Some(&SIG_SP_SET_POS),
        (3, 5) => Some(&SIG_SP_CLS),
        (3, 6) => Some(&SIG_SP_SET_ALPHA),
        (3, 11) => Some(&SIG_SP_CLS_EX),
        (3, 12) => Some(&SIG_SP_SET_FILTER),
        (3, 17) => Some(&SIG_SP_SET_SCALE),
        (3, 22) => Some(&SIG_SPTEXT),
        (3, 25) => Some(&SIG_SP_SET_POS_MOVE),
        (3, 39) => Some(&SIG_SP_SET_SHAKE),
        (3, 41) => Some(&SIG_SP_SET_ANIM_41),
        (3, 46) => Some(&SIG_SP_SHOW),
        (3, 47) => Some(&SIG_SP_HIDE),
        (3, 57) => Some(&SIG_SP_SET_ANIM_57),
        (4, 0) => Some(&SIG_BGM_PLAY),
        (4, 1) => Some(&SIG_BGM_STOP),
        (4, 2) => Some(&SIG_BGM_SET_VOLUME),
        (4, 3) => Some(&SIG_BGM_GET_VOLUME),
        (4, 9) => Some(&SIG_BGM_LOAD),
        (5, 0) => Some(&SIG_SE_LOAD),
        (5, 1) => Some(&SIG_SE_PLAY),
        (5, 2) => Some(&SIG_SE_PLAY_EX),
        (5, 3) => Some(&SIG_SE_STOP),
        (5, 4) => Some(&SIG_SE_SET_VOLUME),
        (5, 5) => Some(&SIG_SE_GET_VOLUME),
        (7, 0) => Some(&SIG_WAIT),
        (7, 1) => Some(&SIG_WAIT_CLICK),
        (7, 2) => Some(&SIG_WAIT_SYNC_BEGIN),
        (7, 3) => Some(&SIG_WAIT_SYNC_RELEASE),
        (7, 4) => Some(&SIG_WAIT_SYNC_END),
        (7, 5) => Some(&SIG_WAIT_SYNC_STEP),
        (7, 7) => Some(&SIG_WAIT_CLICK_NO_ANIM),
        (7, 8) => Some(&SIG_WAIT_SYNC_GET_TIME),
        (7, 9) => Some(&SIG_WAIT_TIME_PUSH),
        (7, 10) => Some(&SIG_WAIT_TIME_POP),
        (6, 0) => Some(&SIG_SELECT_INIT),
        (6, 2) => Some(&SIG_SELECT_SET),
        (6, 4) => Some(&SIG_SELECT_CLEAR),
        (8, 0) => Some(&SIG_BTN_INIT),
        (8, 1) => Some(&SIG_BTN_UNINIT),
        (8, 3) => Some(&SIG_BTN_SET),
        (8, 4) => Some(&SIG_BTN_HIDE),
        (8, 5) => Some(&SIG_BTN_SHOW),
        (8, 6) => Some(&SIG_BTN_SET_POS),
        (8, 8) => Some(&SIG_BTN_RELEASE),
        (8, 9) => Some(&SIG_BTN_SLIDER_GET),
        (8, 10) => Some(&SIG_BTN_SLIDER_SET),
        (8, 11) => Some(&SIG_BTN_SLIDER_BEGIN),
        (8, 12) => Some(&SIG_BTN_ON_CHECK),
        (8, 13) => Some(&SIG_BTN_SET_TOGGLE),
        (8, 14) => Some(&SIG_BTN_SET_STATE),
        (8, 15) => Some(&SIG_BTN_ENABLE),
        (8, 16) => Some(&SIG_BTN_SET_ALPHA),
        (8, 17) => Some(&SIG_BTN_GET_PUSH),
        (8, 19) => Some(&SIG_BTN_LOCK),
        (8, 20) => Some(&SIG_BTN_UNLOCK),
        (8, 21) => Some(&SIG_BTN_SET_ANIM),
        (8, 22) => Some(&SIG_BTN_SET_HIT),
        (9, 9) => Some(&SIG_FONT_SYSTEM_QUERY_9),
        (12, 0) => Some(&SIG_SYSTEM_BTN_SET),
        (12, 1) => Some(&SIG_SYSTEM_BTN_RELEASE),
        (12, 2) => Some(&SIG_SYSTEM_BTN_ENABLE),
        (11, 0) => Some(&SIG_MOVIE_PLAY),
        (11, 1) => Some(&SIG_MSP_SET_LOOP_SP_EP),
        (11, 2) => Some(&SIG_MSP_CLS),
        (11, 3) => Some(&SIG_MSP_WAIT),
        (11, 4) => Some(&SIG_MSP_LOCK),
        (11, 5) => Some(&SIG_MSP_UNLOCK),
        (13, 0) => Some(&SIG_VOICE_PLAY),
        (13, 1) => Some(&SIG_VOICE_STOP),
        (13, 2) => Some(&SIG_VOICE_SET_VOLUME),
        (13, 3) => Some(&SIG_VOICE_GET_VOLUME),
        (13, 16) => Some(&SIG_VOICE_AUTOPAN_SIZE_OVER),
        (13, 18) => Some(&SIG_VOICE_WAIT),
        (10, 12) => Some(&SIG_SAVE_THUMBNAIL_MOSAIC_SET),
        (16, 1) => Some(&SIG_WINDOW_EFFECT_1),
        (16, 2) => Some(&SIG_WINDOW_EFFECT_2),
        (17, 0) => Some(&SIG_ACTION_RUN_COUNT_OVER),
        (17, 1) => Some(&SIG_ACTION_SYNC_RUN_COUNT_OVER),
        (17, 3) => Some(&SIG_ACTION_CLEAR_COUNT_OVER),
        (17, 5) => Some(&SIG_ACTION_TIMELINE_5),
        (17, 8) => Some(&SIG_ACTION_TIMELINE_8),
        (17, 9) => Some(&SIG_ACTION_TIMELINE_9),
        (17, 11) => Some(&SIG_ACTION_TIMELINE_11),
        (17, 23) => Some(&SIG_ACTION_SET_ACTIVE),
        (17, 29) => Some(&SIG_ACTION_TIMELINE_29),
        (17, 30) => Some(&SIG_ACTION_SET_CLEAR),
        (18, 6) => Some(&SIG_ARG_GET),
        (18, 30) => Some(&SIG_OPENFILE),
        (18, 31) => Some(&SIG_READ_FILE),
        (18, 33) => Some(&SIG_SET_FILE_POINTER),
        (18, 36) => Some(&SIG_SZ_BUF),
        (18, 37) => Some(&SIG_GETPRIVATEPROFILEINT),
        (20, 0) => Some(&SIG_RANDOM),
        (22, 0) => Some(&SIG_RUN),
        (22, 1) => Some(&SIG_RUN_NO_WAIT),
        (22, 2) => Some(&SIG_RUN_STACK),
        _ => extsig_auto::lookup_auto_sig(category, index),
    }
}

static ALL_SIGNATURES: &[&ExtSig] = &[
    &SIG_TEXT,
    &SIG_TEXT_HIDE,
    &SIG_TEXT_SHOW,
    &SIG_TEXT_CLEAR,
    &SIG_TEXT_W,
    &SIG_TEXT_A,
    &SIG_TEXT_WA,
    &SIG_TEXT_SET_BASE,
    &SIG_SP_SET,
    &SIG_SP_SET_EX,
    &SIG_SP_SET_POS,
    &SIG_SP_CLS,
    &SIG_SP_SET_ALPHA,
    &SIG_SP_CLS_EX,
    &SIG_SP_SET_FILTER,
    &SIG_SP_SET_SCALE,
    &SIG_SPTEXT,
    &SIG_SP_SET_POS_MOVE,
    &SIG_SP_SET_SHAKE,
    &SIG_SP_SET_ANIM_41,
    &SIG_SP_SHOW,
    &SIG_SP_HIDE,
    &SIG_SP_SET_ANIM_57,
    &SIG_BGM_PLAY,
    &SIG_BGM_STOP,
    &SIG_BGM_SET_VOLUME,
    &SIG_BGM_GET_VOLUME,
    &SIG_BGM_LOAD,
    &SIG_SE_PLAY,
    &SIG_SE_PLAY_EX,
    &SIG_SE_LOAD,
    &SIG_SE_STOP,
    &SIG_SE_SET_VOLUME,
    &SIG_SE_GET_VOLUME,
    &SIG_WAIT,
    &SIG_WAIT_CLICK,
    &SIG_WAIT_SYNC_BEGIN,
    &SIG_WAIT_SYNC_RELEASE,
    &SIG_WAIT_SYNC_END,
    &SIG_WAIT_SYNC_STEP,
    &SIG_WAIT_CLICK_NO_ANIM,
    &SIG_WAIT_SYNC_GET_TIME,
    &SIG_WAIT_TIME_PUSH,
    &SIG_WAIT_TIME_POP,
    &SIG_SELECT_INIT,
    &SIG_SELECT_SET,
    &SIG_SELECT_CLEAR,
    &SIG_BTN_INIT,
    &SIG_BTN_UNINIT,
    &SIG_BTN_SET,
    &SIG_BTN_HIDE,
    &SIG_BTN_SHOW,
    &SIG_BTN_SET_POS,
    &SIG_BTN_RELEASE,
    &SIG_BTN_SLIDER_GET,
    &SIG_BTN_SLIDER_SET,
    &SIG_BTN_SLIDER_BEGIN,
    &SIG_BTN_ON_CHECK,
    &SIG_BTN_SET_TOGGLE,
    &SIG_BTN_SET_STATE,
    &SIG_BTN_ENABLE,
    &SIG_BTN_SET_ALPHA,
    &SIG_BTN_GET_PUSH,
    &SIG_BTN_LOCK,
    &SIG_BTN_UNLOCK,
    &SIG_BTN_SET_ANIM,
    &SIG_BTN_SET_HIT,
    &SIG_FONT_SYSTEM_QUERY_9,
    &SIG_SYSTEM_BTN_SET,
    &SIG_SYSTEM_BTN_RELEASE,
    &SIG_SYSTEM_BTN_ENABLE,
    &SIG_MOVIE_PLAY,
    &SIG_MSP_SET_LOOP_SP_EP,
    &SIG_MSP_CLS,
    &SIG_MSP_WAIT,
    &SIG_MSP_LOCK,
    &SIG_MSP_UNLOCK,
    &SIG_VOICE_PLAY,
    &SIG_VOICE_STOP,
    &SIG_VOICE_SET_VOLUME,
    &SIG_VOICE_GET_VOLUME,
    &SIG_VOICE_AUTOPAN_SIZE_OVER,
    &SIG_VOICE_WAIT,
    &SIG_SAVE_THUMBNAIL_MOSAIC_SET,
    &SIG_WINDOW_EFFECT_1,
    &SIG_WINDOW_EFFECT_2,
    &SIG_ACTION_RUN_COUNT_OVER,
    &SIG_ACTION_SYNC_RUN_COUNT_OVER,
    &SIG_ACTION_CLEAR_COUNT_OVER,
    &SIG_ACTION_TIMELINE_5,
    &SIG_ACTION_TIMELINE_8,
    &SIG_ACTION_TIMELINE_9,
    &SIG_ACTION_TIMELINE_11,
    &SIG_ACTION_SET_ACTIVE,
    &SIG_ACTION_TIMELINE_29,
    &SIG_ACTION_SET_CLEAR,
    &SIG_ARG_GET,
    &SIG_OPENFILE,
    &SIG_READ_FILE,
    &SIG_SET_FILE_POINTER,
    &SIG_SZ_BUF,
    &SIG_GETPRIVATEPROFILEINT,
    &SIG_RANDOM,
    &SIG_RUN,
    &SIG_RUN_NO_WAIT,
    &SIG_RUN_STACK,
];

pub fn all_signatures() -> &'static [&'static ExtSig] {
    ALL_SIGNATURES
}

pub fn auto_signatures() -> &'static [ExtSig] {
    extsig_auto::auto_signatures()
}

/// Best-effort arity observed from `docs/dis.txt`.
///
/// This is not a replacement for a real Game.exe handler pass. It is a runtime
/// safety net for extcalls that are still `Unknown`/`Stub`: the VM can at least
/// consume the same number of pushed arguments that the current script uses,
/// and the decompiler can keep displaying raw arguments instead of pretending
/// their semantics are known.
pub fn observed_pop_count(category: u16, index: u16) -> Option<usize> {
    match (category, index) {
        (2, 0) => Some(8),
        (2, 1) => Some(4),
        (2, 2) => Some(4),
        (2, 3) => Some(2),
        (2, 4) => Some(1),
        (2, 5) => Some(1),
        (2, 8) => Some(0),
        (2, 9) => Some(1),
        (2, 10) => Some(2),
        (2, 11) => Some(2),
        (2, 12) => Some(3),
        (2, 13) => Some(1),
        (2, 14) => Some(2),
        (2, 15) => Some(5),
        (2, 16) => Some(3),
        (2, 17) => Some(3),
        (2, 18) => Some(4),
        (2, 19) => Some(4),
        (2, 20) => Some(3),
        (2, 21) => Some(0),
        (2, 22) => Some(4),
        (2, 23) => Some(1),
        (2, 24) => Some(0),
        (2, 25) => Some(5),
        (2, 26) => Some(1),
        (2, 27) => Some(0),
        (2, 29) => Some(0),
        (2, 30) => Some(1),
        (2, 31) => Some(4),
        (2, 32) => Some(1),
        (3, 2) => Some(4),
        (3, 3) => Some(5),
        (3, 5) => Some(1),
        (3, 6) => Some(2),
        (3, 7) => Some(1),
        (3, 8) => Some(2),
        (3, 9) => Some(3),
        (3, 11) => Some(2),
        (3, 12) => Some(9),
        (3, 13) => Some(4),
        (3, 14) => Some(4),
        (3, 15) => Some(3),
        (3, 16) => Some(2),
        (3, 17) => Some(2),
        (3, 18) => Some(5),
        (3, 19) => Some(5),
        (3, 20) => Some(4),
        (3, 21) => Some(1),
        (3, 22) => Some(5),
        (3, 23) => Some(2),
        (3, 24) => Some(5),
        (3, 25) => Some(4),
        (3, 26) => Some(1),
        (3, 27) => Some(2),
        (3, 28) => Some(4),
        (3, 29) => Some(1),
        (3, 30) => Some(1),
        (3, 31) => Some(9),
        (3, 32) => Some(6),
        (3, 34) => Some(2),
        (3, 35) => Some(2),
        (3, 36) => Some(1),
        (3, 37) => Some(2),
        (3, 38) => Some(9),
        (3, 39) => Some(4),
        (3, 40) => Some(6),
        (3, 44) => Some(1),
        (3, 46) => Some(1),
        (3, 47) => Some(1),
        (3, 48) => Some(1),
        (3, 49) => Some(5),
        (3, 50) => Some(4),
        (3, 51) => Some(1),
        (3, 52) => Some(3),
        (3, 53) => Some(2),
        (3, 54) => Some(1),
        (3, 55) => Some(9),
        (3, 56) => Some(9),
        (3, 57) => Some(3),
        (4, 0) => Some(7),
        (4, 1) => Some(2),
        (4, 2) => Some(1),
        (4, 3) => Some(2),
        (4, 4) => Some(2),
        (4, 6) => Some(2),
        (4, 8) => Some(3),
        (4, 11) => Some(1),
        (4, 12) => Some(3),
        (4, 13) => Some(1),
        (4, 14) => Some(1),
        (4, 15) => Some(1),
        (5, 0) => Some(3),
        (5, 1) => Some(4),
        (5, 2) => Some(5),
        (5, 3) => Some(2),
        (5, 4) => Some(2),
        (5, 5) => Some(3),
        (5, 6) => Some(1),
        (5, 7) => Some(1),
        (5, 14) => Some(2),
        (6, 0) => Some(4),
        (6, 1) => Some(1),
        (6, 2) => Some(6),
        (6, 3) => Some(0),
        (6, 4) => Some(0),
        (6, 6) => Some(3),
        (7, 0) => Some(2),
        (7, 1) => Some(2),
        (7, 2) => Some(0),
        (7, 3) => Some(1),
        (7, 4) => Some(0),
        (7, 5) => Some(0),
        (7, 6) => Some(0),
        (7, 8) => Some(0),
        (7, 9) => Some(0),
        (7, 10) => Some(0),
        (8, 0) => Some(3),
        (8, 1) => Some(1),
        (8, 3) => Some(7),
        (8, 4) => Some(2),
        (8, 5) => Some(2),
        (8, 6) => Some(4),
        (8, 8) => Some(2),
        (8, 9) => Some(5),
        (8, 10) => Some(5),
        (8, 11) => Some(2),
        (8, 12) => Some(2),
        (8, 13) => Some(3),
        (8, 14) => Some(4),
        (8, 15) => Some(3),
        (8, 16) => Some(3),
        (8, 17) => Some(2),
        (8, 18) => Some(3),
        (8, 19) => Some(3),
        (8, 20) => Some(1),
        (8, 21) => Some(4),
        (8, 22) => Some(2),
        (8, 23) => Some(2),
        (8, 24) => Some(2),
        (8, 26) => Some(1),
        (9, 0) => Some(2),
        (9, 1) => Some(1),
        (9, 2) => Some(2),
        (9, 3) => Some(0),
        (9, 4) => Some(1),
        (9, 5) => Some(0),
        (9, 6) => Some(1),
        (9, 7) => Some(1),
        (9, 8) => Some(1),
        (9, 9) => Some(0),
        (9, 10) => Some(0),
        (9, 12) => Some(2),
        (9, 15) => Some(1),
        (9, 17) => Some(1),
        (9, 18) => Some(3),
        (9, 19) => Some(2),
        (9, 20) => Some(1),
        (9, 21) => Some(1),
        (9, 22) => Some(0),
        (9, 23) => Some(1),
        (9, 24) => Some(1),
        (9, 25) => Some(1),
        (9, 26) => Some(2),
        (9, 27) => Some(1),
        (9, 28) => Some(0),
        (9, 29) => Some(0),
        (9, 30) => Some(1),
        (9, 31) => Some(0),
        (9, 35) => Some(0),
        (9, 36) => Some(4),
        (9, 48) => Some(3),
        (9, 49) => Some(1),
        (9, 51) => Some(1),
        (9, 52) => Some(1),
        (9, 53) => Some(0),
        (10, 0) => Some(3),
        (10, 1) => Some(3),
        (10, 4) => Some(2),
        (10, 5) => Some(4),
        (10, 7) => Some(1),
        (10, 9) => Some(1),
        (10, 11) => Some(2),
        (10, 12) => Some(1),
        (10, 13) => Some(5),
        (10, 14) => Some(5),
        (10, 15) => Some(2),
        (10, 16) => Some(4),
        (10, 17) => Some(1),
        (10, 22) => Some(0),
        (10, 24) => Some(1),
        (10, 25) => Some(0),
        (10, 26) => Some(2),
        (10, 27) => Some(2),
        (10, 28) => Some(1),
        (10, 29) => Some(3),
        (10, 30) => Some(3),
        (10, 32) => Some(2),
        (10, 33) => Some(1),
        (10, 34) => Some(1),
        (10, 35) => Some(0),
        (10, 36) => Some(1),
        (11, 0) => Some(2),
        (11, 1) => Some(8),
        (11, 2) => Some(1),
        (11, 3) => Some(2),
        (11, 4) => Some(1),
        (11, 5) => Some(1),
        (12, 0) => Some(3),
        (12, 1) => Some(1),
        (12, 2) => Some(2),
        (13, 0) => Some(4),
        (13, 1) => Some(2),
        (13, 2) => Some(1),
        (13, 3) => Some(2),
        (13, 4) => Some(4),
        (13, 5) => Some(3),
        (13, 6) => Some(1),
        (13, 7) => Some(1),
        (13, 8) => Some(4),
        (13, 11) => Some(1),
        (13, 12) => Some(3),
        (13, 14) => Some(1),
        (13, 15) => Some(1),
        (13, 16) => Some(4),
        (13, 17) => Some(0),
        (13, 18) => Some(2),
        (13, 19) => Some(3),
        (13, 20) => Some(1),
        (13, 21) => Some(1),
        (13, 22) => Some(2),
        (13, 24) => Some(1),
        (14, 0) => Some(9),
        (14, 1) => Some(0),
        (14, 2) => Some(1),
        (14, 3) => Some(0),
        (14, 4) => Some(1),
        (14, 5) => Some(0),
        (14, 6) => Some(0),
        (14, 7) => Some(0),
        (14, 8) => Some(0),
        (14, 9) => Some(0),
        (14, 10) => Some(4),
        (14, 11) => Some(0),
        (14, 12) => Some(1),
        (15, 2) => Some(3),
        (15, 4) => Some(8),
        (15, 5) => Some(1),
        (16, 0) => Some(2),
        (16, 1) => Some(4),
        (16, 2) => Some(4),
        (17, 0) => Some(2),
        (17, 1) => Some(2),
        (17, 3) => Some(1),
        (17, 5) => Some(6),
        (17, 6) => Some(5),
        (17, 7) => Some(5),
        (17, 8) => Some(4),
        (17, 9) => Some(2),
        (17, 10) => Some(6),
        (17, 11) => Some(4),
        (17, 14) => Some(6),
        (17, 15) => Some(6),
        (17, 17) => Some(6),
        (17, 20) => Some(5),
        (17, 21) => Some(7),
        (17, 23) => Some(1),
        (17, 24) => Some(1),
        (17, 28) => Some(0),
        (17, 29) => Some(6),
        (17, 30) => Some(1),
        (18, 1) => Some(1),
        (18, 3) => Some(2),
        (18, 4) => Some(2),
        (18, 5) => Some(3),
        (18, 6) => Some(2),
        (18, 7) => Some(3),
        (18, 8) => Some(1),
        (18, 9) => Some(10),
        (18, 10) => Some(1),
        (18, 12) => Some(9),
        (18, 13) => Some(1),
        (18, 14) => Some(1),
        (18, 15) => Some(1),
        (18, 17) => Some(1),
        (18, 18) => Some(3),
        (18, 21) => Some(1),
        (18, 28) => Some(1),
        (18, 29) => Some(0),
        (18, 30) => Some(1),
        (18, 31) => Some(3),
        (18, 32) => Some(1),
        (18, 33) => Some(3),
        (18, 34) => Some(3),
        (18, 36) => Some(5),
        (18, 37) => Some(4),
        (18, 40) => Some(16),
        (20, 0) => Some(3),
        (21, 0) => Some(1),
        (21, 2) => Some(1),
        (22, 0) => Some(3),
        (22, 1) => Some(0),
        (23, 0) => Some(1),
        (23, 1) => Some(0),
        (23, 2) => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn testcase_visible_extcalls_use_observed_categories() {
        assert_eq!(lookup_sig(11, 0).map(|s| s.name), Some("movie_play"));
        assert_eq!(
            lookup_sig(11, 1).map(|s| s.name),
            Some("msp_set_loop_sp_ep")
        );
        assert_eq!(lookup_sig(11, 3).map(|s| s.name), Some("msp_wait"));
        assert_eq!(lookup_sig(3, 88), None);

        assert_eq!(lookup_sig(8, 3).map(|s| s.name), Some("btn_set"));
        assert_eq!(lookup_sig(8, 3).map(|s| s.pop_count), Some(7));
        assert_eq!(lookup_sig(8, 6).map(|s| s.name), Some("btn_set_pos"));
        assert_eq!(lookup_sig(8, 17).map(|s| s.name), Some("btn_get_push"));
        assert_eq!(lookup_sig(8, 19).map(|s| s.pop_count), Some(2));
        assert_eq!(lookup_sig(8, 20).map(|s| s.pop_count), Some(1));

        assert_eq!(lookup_sig(4, 1).map(|s| s.name), Some("bgm_stop"));
        assert_eq!(lookup_sig(5, 0).map(|s| s.name), Some("se_load"));
        assert_eq!(lookup_sig(13, 18).map(|s| s.name), Some("voice_wait"));
        assert_eq!(lookup_sig(18, 33).map(|s| s.name), Some("set_file_pointer"));
    }

    #[test]
    fn observed_pop_counts_cover_current_testcase_hotspots() {
        assert_eq!(observed_pop_count(2, 15), Some(5));
        assert_eq!(observed_pop_count(7, 1), Some(2));
        assert_eq!(observed_pop_count(8, 18), Some(3));
        assert_eq!(observed_pop_count(11, 1), Some(8));
        assert_eq!(observed_pop_count(23, 2), Some(1));
        assert_eq!(observed_pop_count(99, 99), None);
    }

    #[test]
    fn all_signatures_have_unique_category_index_keys() {
        let mut seen = BTreeSet::new();
        for sig in all_signatures() {
            assert!(
                seen.insert((sig.category, sig.index)),
                "duplicate ext signature {:04X}:{:04X} {}",
                sig.category,
                sig.index,
                sig.name
            );
            assert_eq!(lookup_sig(sig.category, sig.index), Some(*sig));
            assert_eq!(
                sig.pop_count,
                sig.params.len(),
                "{} pop_count mismatch",
                sig.name
            );
        }
    }
}
