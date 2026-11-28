//! The desktop application: shell assembly (QAT band, ribbon, stencil
//! pane, canvas, page tabs, status bar) and all interaction wiring over
//! the dc-core command layer.
//!
//! This file is the module root: every `App`-related type, constant, and
//! free helper lives HERE, so `crate::app::*` paths are stable. The
//! `impl App` method blocks are split across the sibling files below
//! (owner decomposition direction) — inherent impls merge, so the split
//! changes no paths and no visibility.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use dc_core::commands::{
    AddPage, AddShape, DuplicatePage, DuplicateShape, MovePage, RemovePage, RemoveShape,
    RenamePage, ReorderShape,
};
use dc_core::scope::{shape_chain, Scope};
use dc_core::{Editor, Macro, SetCell, SetRowCell};

use egui::{Color32, Context, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use vsdx::model::{Document, Shape};

use crate::canvas::{self, Handle, HandleId, PageGeom, View};
use crate::cmd::MoveShape;
use crate::recent::RecentFiles;
use crate::ribbon::{Action, Icon, ShowFrom, Tab, TABS};
use crate::stencil::{MasterKey, Stencil, StencilId, StencilSet};
use crate::svgimg::TextureCache;
use crate::theme;
use dc_core::connects::ConnectsHost;
use dc_core::AddMasterInstance;

/// Draft row for the Hyperlinks dialog (#28): `edit_row` names the
/// instance row being edited (None = adding). `from_master` marks a
/// draft opened on a master-inherited row (#139): OK materializes the
/// row onto the instance sheet instead of replacing a local one.
#[derive(Clone)]
struct HyperlinkDraft {
    edit_row: Option<i64>,
    from_master: bool,
    address: String,
    sub: String,
    desc: String,
}

/// One hyperlink row for list display: its values, its row ix, and
/// whether it lives on the instance sheet or is inherited from the
/// shape's master (#139).
#[derive(Clone)]
struct HyperRow {
    ix: i64,
    inherited: bool,
    addr: String,
    sub: String,
    desc: String,
}

/// One master ShapeSheet Actions row, evaluated for THIS instance
/// (#25): label, SETF formula, and the live Checked/Disabled flags.
#[derive(Clone)]
struct MasterActionRow {
    menu: String,
    action: String,
    checked: bool,
    disabled: bool,
    begin_group: bool,
    flyout_child: bool,
    display_only: bool,
}

/// Per-page UI state parked while another page is active (#54):
/// selection (primary + multi) and the view position return as they
/// were left. View chrome (rulers, grid, units) stays app-level like
/// Visio, so it is excluded on restore.
#[derive(Clone)]
struct PageUiState {
    sel: Option<(i64, i64)>,
    sel_set: Vec<(i64, i64)>,
    view: View,
}

/// #149 item 2: which Format-pane section a mini-toolbar Format icon
/// targets — the pane forces the section open and scrolls to it once,
/// then consumes the focus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FormatSection {
    Fill,
    Line,
    Text,
}

/// What a frame's line-jump overlay depends on (`App::jump_fx_cache`).
#[derive(Debug, Clone, PartialEq)]
struct JumpFxKey {
    version: u64,
    page_idx: usize,
    hidden: Vec<i64>,
    radius_bits: (u64, u64),
}

pub struct App {
    ed: Option<Editor>,
    path: PathBuf,
    dirty: bool,
    version: u64,
    view: View,
    page_idx: usize,
    /// Per-page UI state saved while another page is active (#54):
    /// selection and view position restore when the page comes back.
    page_states: std::collections::HashMap<i64, PageUiState>,
    /// Open page-tab context menu: (page index, anchor) (#54).
    tab_menu: Option<(usize, Pos2)>,
    /// Inline tab rename: (page id, draft) (#54).
    tab_rename: Option<(i64, String)>,
    /// Scroll the tab strip to the active tab once, after a switch
    /// (#54) — consumed by the next strip paint.
    tab_scroll_to_active: bool,
    /// The Reorder Pages dialog is open (#54, Visio mechanism).
    reorder_open: bool,
    /// The Hyperlinks dialog is open for this shape (#28).
    hyperlink_open: Option<i64>,
    /// Draft row being added/edited in the Hyperlinks dialog (#28).
    hyperlink_draft: Option<HyperlinkDraft>,
    /// The Insert Field dialog is open for this (page, shape) (#267)
    /// — the PAGE rides along because shape IDs collide across pages
    /// and documents, and the alive check pins the dialog to the page
    /// it opened on (CodeRabbit round 3).
    fields_open: Option<(i64, i64)>,
    /// The caret captured when the dialog opened from a live text
    /// edit — the opening click commits the edit (#22), the field
    /// still lands at the caret (#267): (page, shape, plain offset).
    fields_caret: Option<(i64, i64, usize)>,
    /// The caret of the LAST text edit this shape had, kept across the
    /// edit's commit — the ribbon journey to Insert ▸ Text Field
    /// crosses frames (the tab click commits the edit before the
    /// field button opens the dialog), and the field still lands
    /// where the caret sat (#267, CodeRabbit PR review).
    last_edit_caret: Option<(i64, i64, usize)>,
    /// The Insert Field dialog's edit state (#267).
    fields_draft: Option<crate::fields_dlg::FieldsDraft>,
    /// Check Spelling (#121): the session walk, language choice, and
    /// the app-level custom dictionary (the domain lives in
    /// `spellcheck.rs`; the app shell keeps only the state field +
    /// glue).
    spell: crate::spellcheck::SpellState,
    /// Find (#115 phase 2): the modeless Find window's session (the
    /// domain lives in `find_dlg.rs`; the app shell keeps the state
    /// field + glue).
    find: crate::find_dlg::FindState,
    /// #115 phase 3 (spec 3): the modeless Replace window's session
    /// (the domain lives in `replace_dlg.rs`).
    repl: crate::replace_dlg::ReplaceState,
    /// #115 phase 2 (FS 1 §3.2): the ribbon Find dropdown's anchor —
    /// `Some` while the menu is open (the Task Panes pattern).
    find_menu: Option<Pos2>,
    find_menu_fresh: bool,
    /// File ▸ Options (#268): the dialog's open flag and draft (the
    /// domain lives in `options.rs`; the app shell keeps the state
    /// fields + glue — the decomposition direction).
    options_open: bool,
    /// #304: the last language commit failed to persist — the Options
    /// dialog warns and the next Apply retries even though the
    /// in-memory language already matches the draft.
    lang_write_failed: bool,
    /// #305 review: the last `options.json` commit failed to persist
    /// — same contract: warn in the dialog, retry on Apply/OK.
    settings_write_failed: bool,
    options_draft: Option<crate::options::Draft>,
    /// Set for the single frame the Options dialog opened on
    /// (CodeRabbit round): the Enter that ACTIVATED the backstage
    /// button is still in that frame's input, so the dialog's own
    /// key checks must skip it once.
    options_opened_this_frame: bool,
    /// The app-level options store (#268): user name/initials (the
    /// Comments author field) and the Save page's AutoRecover
    /// settings. Production opens the persisted copy; headless ctors
    /// stay ephemeral.
    options_store: crate::options::Options,
    /// The Comments popup is open for this shape (#29 rework: the
    /// Visio 2016 thread view — author header, edit/delete/reply).
    comment_target: Option<i64>,
    /// The reply / new-comment input body (#29).
    comment_body: String,
    /// Inline edit buffer: (annotation id, body) (#29).
    comment_edit: Option<(i64, String)>,
    /// Annotation visibility (Review ▸ Show Comments, #29).
    show_comments: bool,
    /// The Shape Data dialog is open for this shape (#30).
    shape_data_open: Option<i64>,
    /// #210: the Size & Position window, and its edit buffers keyed by
    /// the selection they belong to.
    size_pos_open: bool,
    size_pos_bufs: (
        Option<(i64, i64)>,
        std::collections::HashMap<sizepos::SizePosField, String>,
    ),
    /// #211 (URS-1051): the Pan & Zoom window is open; the drag's
    /// grab offset (viewport center − pointer, page inches y-down)
    /// while its gesture runs; the zoom entry's text buffer.
    panzoom_open: bool,
    panzoom_grab: Option<(f64, f64)>,
    panzoom_pct: String,
    /// #211: the View ▸ Task Panes dropdown is open, anchored here
    /// (the dispatch site's pointer position). `task_panes_fresh`
    /// shields the menu's first painted frame from its own opening
    /// click's release.
    task_panes_menu: Option<Pos2>,
    task_panes_fresh: bool,
    /// The pointer's last position (updated once per frame in `draw`)
    /// — the anchor dispatch reads when it opens the Task Panes menu
    /// (dispatch has no Context; `mouse_page` is the same per-frame
    /// capture pattern).
    pointer_last: Pos2,
    /// #211 test seam: the thumbnail paper and the drawn viewport
    /// rectangle on screen while the Pan & Zoom window is open
    /// (NOTHING otherwise).
    panzoom_thumb: Rect,
    panzoom_vp: Rect,
    /// #211 (PR #413 review): the view (zoom, pan) the overview's
    /// rectangle was DRAWN from this frame. Wheel zoom/scroll, Fit
    /// Page and the zoom chrome write the view later in the same
    /// frame — `draw` compares this against the end-of-frame view and
    /// requests one repaint so the rectangle never sits an interaction
    /// behind (input-driven gestures self-correct on their release
    /// frame; these discrete writes have no follow-up event).
    panzoom_view_drawn: Option<(f32, f32, f32)>,
    /// The Format Shape pane is open (#31).
    format_pane_open: bool,
    /// #149 item 2: the pane section a mini-toolbar Format icon
    /// targets — forced open and scrolled to once, then consumed.
    format_pane_focus: Option<FormatSection>,
    /// The last section a Format icon targeted (diagnostic; unlike
    /// [`Self::format_pane_focus`] it persists after the pane
    /// consumed the one-shot scroll).
    format_pane_last_focus: Option<FormatSection>,
    /// Text buffers for the Format pane's numeric/text fields (#31).
    fmt_bufs: std::collections::HashMap<String, String>,
    /// Text buffers for the Shape Data string fields (#30) — commits
    /// on focus loss, one undo per change.
    prop_bufs: std::collections::HashMap<String, String>,
    /// The Shape Data "add choice" row is open for this variable-list
    /// (Type 4) Prop row (#30).
    prop_add: Option<String>,
    tab: Tab,
    /// #237: a pending zoom-to-selection (the ribbon action / Shift+Z)
    /// — the canvas frame centers + fills the selection's union bounds
    /// on the next pass, on the same view ledger as Fit Page.
    zoom_selection_pending: bool,
    sel: Option<(i64, i64)>, // (page id, shape id) — primary selection
    /// Multi-selection (marquee / shift-click); includes the primary.
    sel_set: Vec<(i64, i64)>,
    drag: Option<DragState>,
    /// Group-edit mode (GitHub #78): stack of open group ids, outermost
    /// first — `Open <name>` pushes, Esc/Close pops one level. View
    /// state only (never written into the document); validated at the
    /// top of `draw` so undo/ungroup/doc changes leave it coherent.
    group_edit: Vec<i64>,
    stencil: Stencil,
    /// The open-stencil set (Shapes pane, #63) — session state, never
    /// file content. Kept across document switches like Visio's
    /// workspace; the pane (`stencil`) renders it.
    stencils: StencilSet,
    /// Stencil panel content rect from the last frame (resize-handle
    /// target for the kittest re-wrap tests).
    stencil_panel_inner: Rect,
    tex: TextureCache,
    /// Background rasterization (#92): the parse/render jobs for page
    /// textures and stencil thumbnails run here; completions land in
    /// `draw`'s pool drain (see `svgimg::force_sync_raster_for_tests`
    /// for the kittest determinism seam).
    pool: crate::rasterpool::RasterPool,
    /// Per-shape canvas layers (#93): one quad per top-level shape
    /// subtree, composited in z-order; commits dirty only the touched
    /// shape (`dc_core::Touch` routed in `apply`/`do_undo`/`do_redo`).
    layers: crate::layers::ShapeLayers,
    /// Test-only: force the Info thumbnail's render to fail (#58).
    info_render_fail_for_test: bool,
    /// Live drag ghosts drawn on the last frame (#96, test hook).
    last_drag_ghosts: usize,
    /// #283 review: the live drag's primary ghost quad on screen.
    last_drag_ghost_rect: Option<egui::Rect>,
    mouse_page: Option<(f64, f64)>,
    status: String,
    /// #204: the Tell Me results panel is open under the tab-strip
    /// box (step 3's popup; the real query field that feeds it lands
    /// with step 4).
    tellme_open: bool,
    /// #204: the live Tell Me query.
    tellme_query: String,
    /// #204: the keyboard-selected result row — hover moves it too,
    /// Office-style.
    tellme_sel: usize,
    /// #204 (CodeRabbit PR review): an Enter captured by the input
    /// path, resolved AFTER the query field applies this frame's
    /// typing (see `tab_strip`) so a replace-text-and-Enter frame
    /// runs the new query's hit, not the stale one.
    tellme_enter: bool,
    /// #204 step 7: the basic help window stub, pre-filled with the
    /// query that opened it (real help content/pane depth is the
    /// follow-up issue per the plan's deferral).
    tellme_help: Option<String>,
    /// #204 (CodeRabbit PR review): the open generation of the
    /// floating chrome windows — egui raises a freshly-shown or
    /// clicked window to the top, so Esc must close the most
    /// recently OPENED one. `chrome_epoch` hands out generations;
    /// the two `_*_epoch` fields stamp each window's last open.
    chrome_epoch: u64,
    about_epoch: u64,
    tellme_help_epoch: u64,
    /// #192 spec: the open ShapeSheet windows, one per target (each
    /// its own child window; a target appears at most once). #272: a
    /// target is a shape, a page's PageSheet or the DocumentSheet.
    shapesheets: Vec<crate::shapesheet::SheetTarget>,
    /// #192 spec: the INACTIVE open documents — File > Open ADDS a
    /// document now; the Window switcher activates one.
    docs: Vec<DocSession>,
    /// #192: the Window switcher popup on the tab strip.
    winmenu_open: bool,
    /// #192 4a: a Window-menu pick to activate after the strip's
    /// borrow ends (the popup closure borrows self immutably).
    pending_activate: Option<usize>,
    /// Snap & glue settings (#109): the document's DocumentSettings,
    /// mirrored in UI state; every toggle writes through to the doc.
    snap: crate::snap::SnapSettings,
    /// The Snap & Glue dialog is open (View ▸ Visual Aids, #109).
    snap_dlg_open: bool,
    /// The Zoom dialog is open (#209 step 3 — the status-bar % button
    /// and the View ▸ Zoom… button).
    zoom_dlg_open: bool,
    /// The Zoom dialog's chosen option (#209 step 3); the dialog opens
    /// with the current zoom preselected.
    zoom_pick: dialogs::ZoomPick,
    /// #209 step 7: Presentation Mode — ribbon, tab strip, panes and
    /// backstage chrome hidden; Esc or the status-bar button restores.
    /// #130: it is full screen now — the toggle and its Esc restore
    /// drive the viewport command too.
    chrome_hidden: bool,
    /// #130: the active Slide Show — the read-only, page-at-a-time
    /// presentation. While set, `draw` paints the show instead of the
    /// shell; Esc restores the entry page and the windowed shell.
    slideshow: Option<slideshow::SlideShow>,
    /// A Fullscreen(on) viewport command waiting for the frame's
    /// Context (#130) — the dispatch paths that decide it run without
    /// one (the `os_clipboard_text` pattern).
    fullscreen_pending: Option<bool>,
    /// Test observability (#130): the last Fullscreen(on) command
    /// sent (None when none was) — kittest cannot read viewport
    /// output after a pass.
    debug_fullscreen: Option<bool>,
    /// #246: the Language dialog is open.
    /// #218 review: the Jump-size drag's draft — a DragValue commit
    /// per frame flooded history; the draft edits locally and commits
    /// once, when the drag stops (typed input commits immediately).
    line_jump_draft: Option<f32>,
    /// The twin draft for the Vertical size slider (#324 phase 5).
    line_jump_draft_y: Option<f32>,
    /// Dynamic-grid hints for the frame in progress (#109, URS-1088),
    /// recomputed by `update_drag` while a move drag is live and
    /// painted by the canvas from `DragFx::hints`.
    snap_live: Vec<crate::snap::Hint>,
    /// The last frame's connection-point affordances (#177): what the
    /// canvas painted — reveal symbols, glue-target highlight, glued
    /// sites of the selected connector. Kept as the test seam.
    conn_fx_last: crate::connectors::ConnPointFx,
    /// The active mouse tool (#108).
    tool: Tool,
    /// #110: align against the page instead of the first-selected
    /// shape (URS-1149's option).
    align_to_page: bool,
    /// The Select by Type dialog is open (#110, URS-1160).
    selby_open: bool,
    /// Select-by-Type criterion: 0 shape type, 1 master, 2 layer.
    selby_criterion: u8,
    /// Select-by-Type chosen value.
    selby_value: String,
    /// The Page Setup dialog is open (#113, URS-1122).
    pagesetup_open: bool,
    /// The Copy as Picture dialog is open (#113, URS-1200).
    copypic_open: bool,
    /// #113 remainder: the vector (SVG) flavor for copy-as-picture.
    copypic_svg: bool,
    /// #156: the Container Styles gallery dialog (Design ▸ Containers…).
    contgal_open: bool,
    /// #192 MDI: the central client rect (what `central`'s ui sees
    /// after the docked panels) — the region child windows constrain
    /// to. Captured every frame in `central`.
    mdi_client: egui::Rect,
    /// #192 spec: the child-window count last frame — any change
    /// re-fires the automatic arrangement (children tile to fit the
    /// client). While exactly ONE child exists it is decorator-free
    /// and pinned full-bleed ("treated as it is now").
    mdi_last_count: usize,
    mdi_arranging: bool,
    /// #192: the decorator/arrangement decisions for the drawing
    /// window, computed in `draw` AFTER central ran — central applies
    /// them on the frame after the count changes (one frame of lag,
    /// invisible: the count is stable by then).
    mdi_decorated: bool,
    mdi_drawing_tile: egui::Rect,
    /// Copy-as-picture quality: 1 screen, 2 high, 4 print (#113).
    copypic_factor: u32,
    /// Text waiting for the OS clipboard (#113, URS-1113): copied on
    /// the next frame's update (the copy path has no Context).
    os_clipboard_text: Option<String>,
    /// An in-flight OS clipboard image probe (#113): RGBA result.
    os_image_rx: Option<Receiver<Option<OsImage>>>,
    /// The Backgrounds dialog is open (#113, URS-1158).
    backgrounds_open: bool,
    /// Background-page underlay cache (#113): one raster per chain
    /// level, keyed (bg page, content version, ppi bucket) — the
    /// handle converts to a quad's TextureId per frame.
    bg_underlays: Vec<(i64, u64, u32, egui::TextureHandle)>,
    /// The last copy-as-picture PNG (test hook).
    last_picture: Option<Vec<u8>>,
    /// The Format Painter is armed for one target (#110, URS-1053):
    /// the source shape whose resolved formatting the next shape
    /// click receives.
    painter_armed: Option<i64>,
    /// #110 remainder: the painter stays armed across applications
    /// (Visio's double-click Format Painter) until Esc or a cancel.
    painter_sticky: bool,
    /// #108: the Connection Point tool's selected point — (shape, row
    /// ix); Delete removes the row.
    connpt_sel: Option<(i64, i64)>,
    /// #108: the line-jump hops painted on the last frame (test seam).
    jumps_last: usize,
    /// #389: the last jump pass and what it was computed for — the
    /// document version (bumped by every edit, undo, redo and open),
    /// the page, the hidden layers and the hop radii. An idle frame
    /// reuses it instead of recomputing the crossing set and every
    /// hop's backdrop (the same cache #347 kept on the old line-jumps
    /// branch).
    jump_fx_cache: Option<(JumpFxKey, Vec<canvas::JumpFx>)>,
    /// #226: whether the last frame painted the styled endpoint-drag
    /// ghost (test seam).
    last_endpoint_ghost: bool,
    /// Guide-glue pending on the live drag (#113): the guide the
    /// snapped axis came from and which pin axes to glue. Written by
    /// `apply_drag_snap` when glue-to-guides is on, consumed by the
    /// move commit as Visio's `PinX = Sheet.N!PinX` formulas.
    drag_glue: Option<(i64, bool, bool)>,
    /// #226: the live endpoint-drag preview — a document snapshot the
    /// per-frame ghost mutates through the SAME planner the release
    /// commits (what you drag is exactly what lands). Armed while a
    /// LineBegin/LineEnd handle drag runs, dropped when it ends.
    endpoint_preview: Option<EndpointPreview>,
    shot_frames: u32,
    /// #224: the splash moment's start — the branded boot overlay
    /// shows until loading::SPLASH_SECS elapse, then the shell takes
    /// over. Headless ctors pin None so kittest never sees it.
    splash: Option<std::time::Instant>,
    /// #224: the in-flight file open — the parse runs on a worker
    /// thread (the same pattern as the rfd pick); the overlay shows
    /// and the result lands through finish_load. The `Instant` drives
    /// the loading animation.
    loading: Option<InflightLoad>,
    /// #242: the document LANDED but its first render is still in
    /// flight — the scrim stays up (same card, "Rendering…") until the
    /// layer store drains and the minimum display elapses, so the
    /// reveal happens on the finished render. Carries the load-start
    /// Instant (the scrim clock) and the path (the final status).
    rendering: Option<(PathBuf, std::time::Instant)>,
    /// #242: the About window (the QAT Help button) — the Tarahi mark,
    /// wordmark, and the running version.
    about_open: bool,
    /// #242 test hook: inject a primary click into the next update's
    /// input (the splash skip reads raw input events, and the kittest
    /// widget clicks target the scrim's eaten rect).
    test_click: bool,
    /// Native file dialog in flight. rfd's sync API parks the calling
    /// thread until its private GTK thread finishes, so it must never
    /// run on the UI thread — otherwise the eframe event loop stalls
    /// and the compositor reports "not responding". The dialog runs on
    /// a worker thread; `poll_dialog` picks up the result each frame.
    dialog: Option<PendingDialog>,
    /// Canvas area from the last frame (stencil drop target).
    canvas_rect: Rect,
    /// File backstage view is open (green rail; 2010 three-band IA, #55).
    pub backstage: bool,
    backstage_page: BackPage,
    /// Live query for the backstage New page's template search (#55).
    backstage_search: String,
    /// Backstage Export page state (#35): format/scope/options/path
    /// and the in-flight worker — the pane module owns the domain,
    /// the app shell only forwards (owner's 2026-10-26 decomposition
    /// rule).
    export_pane: crate::export::ExportPane,
    /// "Save changes?" prompt before File → New discards a dirty doc.
    confirm_new: bool,
    /// Continue into a new blank drawing once the save-as dialog
    /// triggered by "Save & New" completes (or is cancelled).
    new_after_save: bool,
    /// Close decision pending: the "Save changes?" prompt a window
    /// close (✕ / Alt-F4) or File → Close must clear before a dirty
    /// document is lost (GitHub #65).
    confirm_close: Option<CloseTarget>,
    /// Close once the save-as dialog triggered by the close prompt's
    /// Save (an untitled document) completes; dropped when that dialog
    /// is cancelled — cancelling the save means staying open.
    close_after_save: Option<CloseTarget>,
    /// Test hook input: a close request as if the window's ✕ was
    /// clicked — kittest cannot synthesize a WM_CLOSE (GitHub #65).
    close_req_for_test: bool,
    /// A close was approved and issued. egui-winit turns
    /// `ViewportCommand::Close` into the NEXT frame's close-requested
    /// input (`egui-winit/src/lib.rs` `ViewportCommand::Close`), so
    /// that echo must pass through un-vetoed — otherwise "Don't Save"
    /// (document left dirty) resurrects the prompt forever (#65).
    closing: bool,
    /// Test seam for the native save-as dialog: when armed, the next
    /// SaveAs dialog resolves with the preset answer instead of
    /// opening rfd (headless kittest must not open GTK dialogs).
    /// `Some(None)` = the dialog is "cancelled".
    dialog_preset_for_test: Option<Option<PathBuf>>,
    /// Test observability (#65): viewport commands sent by the close
    /// path (kittest cannot read viewport output after a pass).
    debug_close_vetoes: u32,
    debug_close_issued: u32,
    /// Open shape context menu: (shape id, screen anchor, page-inches
    /// anchor y-up) — set by right-click, cleared on action/outside click.
    ctx_menu: Option<(i64, Pos2, (f64, f64))>,
    /// #272: the empty-page context menu (Visio: right-click the page
    /// with nothing under the cursor ▸ Show ShapeSheet opens the
    /// PageSheet) — its screen anchor.
    page_menu: Option<Pos2>,
    /// The Quick Shapes membership menu (#143): the right-clicked
    /// master cell (source key) and the popup anchor.
    quick_menu: Option<((StencilId, i64), egui::Pos2)>,
    /// The window title last pushed to the OS (sent on change only —
    /// the viewport command is not free per frame). Tracked so
    /// headless tests can assert it (#67: the OS draws the title, the
    /// app band no longer duplicates it).
    window_title: String,
    /// Last frame's context-menu rect in screen coords; None while the
    /// menu is closed (kept for the placement tests, issue #49).
    ctx_menu_rect: Option<egui::Rect>,
    /// In-app shape clipboard (Cut/Copy/Paste within Tarahi; rich
    /// OS-clipboard formats arrive with a later milestone).
    clipboard: Option<Shape>,
    /// In-place WYSIWYG text editor session (GitHub #22); the state
    /// and overlay live in `crate::textedit`, this is the shell glue.
    text_edit: Option<crate::textedit::TextEditSession>,
    /// The last canvas click placed a stencil master — a double-click
    /// completing that click pair must not open a text editor.
    placed_last_click: bool,
    /// Fill/Line color palette target (opened from the ribbon).
    color_pick: Option<ColorTarget>,
    /// Most-recently-used files (start screen + backstage Open page).
    /// Ephemeral in the headless constructors; `App::new` swaps in the
    /// persistent on-disk list.
    recent: RecentFiles,
    /// Per-user Quick Shapes membership (#143) — app-side, persisted
    /// beside the MRU; ephemeral in every headless constructor.
    quick_shapes: crate::quickshapes::QuickShapes,
    /// "Use relative paths for hyperlinks" per document (#71),
    /// app-side keyed by path (Visio keeps the toggle app-level).
    relative_paths: crate::hyperlink::RelativePaths,
    /// Default stencil set + category order (#105) — app config,
    /// never .vsdx content; ephemeral in every headless constructor.
    stencil_prefs: crate::stencil::prefs::StencilPrefs,
    /// Whether built-ins open from the extracted My Shapes files
    /// (#104). Production only: every headless constructor disables
    /// it so kittest never reads (or depends on) the user's real
    /// My Shapes — the embedded fallback keeps tests hermetic.
    stencil_disk: bool,
    /// The background first-run extraction (#104), polled for
    /// startup feedback (owner report 2026-10-30: the 43 MB write
    /// must neither block the first frame nor run silently).
    stencil_extract: Option<crate::stencil::disk::ExtractionHandle>,
    /// OS light/dark watcher (GitHub #52). Production only — the
    /// headless constructors leave it off so goldens are deterministic.
    os_theme: Option<crate::os_theme::OsThemeWatcher>,
    /// #184 Alt keytips: the armed mode plus this frame's badge
    /// captures (collected during ribbon layout, painted after the
    /// ribbon panels close — see `crate::keytips`).
    keytips: crate::keytips::Keytips,
}

/// Which cell a ribbon palette click targets.
#[derive(Clone, Copy, PartialEq)]
enum ColorTarget {
    Fill,
    FillBkgnd,
    Line,
    /// Character row-0 Color (#73): writes a section-row cell, not a
    /// plain one.
    Text,
}

/// #224: one in-flight document open — the picked path, the moment
/// the load began (the loading animation's clock), and the worker's
/// parse-result channel.
struct InflightLoad {
    path: PathBuf,
    started: std::time::Instant,
    rx: mpsc::Receiver<Result<vsdx::Document, String>>,
    /// The worker's result, drained from `rx` (#242): a result pulled
    /// while the minimum display still runs LIVES HERE — the channel
    /// has no push-back, so an early `try_recv` into a local dropped
    /// it and the next poll saw a dead channel.
    result: Option<Result<vsdx::Document, String>>,
}

enum PendingDialog {
    Open(Receiver<Option<PathBuf>>),
    SaveAs(Receiver<Option<PathBuf>>),
    /// "Open Existing Stencil…" (.vssx into the Shapes pane, #83).
    OpenStencil(Receiver<Option<PathBuf>>),
    /// Insert ▸ Pictures (#113): a picked PNG/JPEG.
    OpenPicture(Receiver<Option<PathBuf>>),
    /// Hyperlinks ▸ Address browse (#71): a picked target file for
    /// the draft's Address field.
    HyperlinkAddress(Receiver<Option<PathBuf>>),
    /// Backstage Export Browse… (#35): the picked output file.
    ExportSave(Receiver<Option<PathBuf>>),
}

#[derive(Clone, Copy)]
enum PendingKind {
    Open,
    SaveAs,
    /// "Open Existing Stencil…" from the Shapes pane (#83).
    OpenStencil,
    /// Insert ▸ Pictures (#113, URS-1012): a PNG/JPEG file picked to
    /// become a Foreign shape.
    OpenPicture,
    /// Hyperlinks ▸ Address browse (#71).
    HyperlinkAddress,
    /// Backstage Export page's Browse… (#35): the save dialog carries
    /// the pane's format so the extension filter matches.
    ExportSave(dc::export::Format),
}

/// Backstage rail command.
#[derive(Clone, Copy)]
enum BackAction {
    Save,
    SaveAs,
    CloseDoc,
}

/// What a confirmed "Save changes?" prompt closes (GitHub #65): the
/// whole app (window ✕ / Alt-F4) or just the document (File → Close).
#[derive(Clone, Copy, PartialEq)]
enum CloseTarget {
    App,
    Doc,
}

/// #192 spec: one INACTIVE open document (multi-document sessions).
/// The active document lives in the App's own fields; switching swaps
/// the session in. View/scroll state resets on switch (4a) — the
/// per-document view state is slice 4b.
struct DocSession {
    ed: Editor,
    path: PathBuf,
    dirty: bool,
    shapesheets: Vec<crate::shapesheet::SheetTarget>,
    /// #192 4b: the document's view (zoom/pan/unit/toggles), page
    /// selection, shape selection, and group-edit stack — switching
    /// loses nothing.
    view: View,
    page_idx: usize,
    sel: Option<(i64, i64)>,
    sel_set: Vec<(i64, i64)>,
    group_edit: Vec<i64>,
}

/// Backstage content page (GitHub #55: 2010 three-band rail — commands
/// above, destinations Info/Recent/New/Print/Share/Export, Options
/// pinned below; 2016 functions folded in).
#[derive(Clone, Copy, PartialEq)]
enum BackPage {
    Info,
    Recent,
    New,
    Print,
    Share,
    Export,
}

impl BackPage {
    /// Rail label for tests (`set_backstage_page_for_test`).
    fn from_test_name(name: &str) -> Option<BackPage> {
        Some(match name {
            "info" => BackPage::Info,
            "recent" => BackPage::Recent,
            "new" => BackPage::New,
            "print" => BackPage::Print,
            "share" => BackPage::Share,
            "export" => BackPage::Export,
            _ => return None,
        })
    }
}

/// Snap distance in screen px (Visio's default snap strength feels
/// like this at 100 %): converted to page inches per frame by
/// `snap_ctx_of` (#109).
const SNAP_STRENGTH_PX: f32 = 8.0;
/// How close a connector endpoint must come to a shape before its
/// connection points reveal (#177) — page inches, generous like
/// Visio's approach affordance.
const CONN_REVEAL_IN: f64 = 0.6;

/// An OS clipboard image probe's payload: (w, h, RGBA bytes).
type OsImage = (u32, u32, Vec<u8>);

/// #156: the SDCONT style names live beside the Container Tools tab
/// layout now (`crate::ribbon::container_tools`).
use crate::ribbon::container_tools::CONTAINER_STYLES;

/// The active mouse tool (Visio's Home ▸ Tools): the pointer selects
/// and drags; the Connector Tool (#108) draws connectors; the Lasso
/// (#110, Visio's Lasso Select) trails a freehand selection. Esc
/// returns to the pointer, like Visio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Pointer,
    Connector,
    Lasso,
    /// #108: the Connection Point tool — click a shape to add a
    /// connection point, click an existing one to select it (Delete
    /// removes).
    ConnectionPoint,
    /// #132 (URS-1225): the Freeform tool — a sampled pen stroke
    /// becomes a new shape's editable Geometry rows.
    Freeform,
    /// #283: the Pan tool — a plain drag moves the view (Visio's
    /// hand); nothing arms, Esc returns to the pointer.
    Pan,
}

struct DragState {
    shape: i64,
    grab_off: (f64, f64), // pin − pointer, inches (y-up)
    start_pin: (f64, f64),
    last_pin: (f64, f64),
    moved: bool,
    /// Other members of a multi-selection captured when the press began
    /// (the press on a selected member drags the whole group; a press on
    /// an unselected shape moves only that shape — GitHub #59).
    group: Vec<i64>,
}

/// The #226 live endpoint-drag preview's state: the document snapshot
/// the per-frame ghost mutates, the #175 row-translation anchor (the
/// pre-drag END — the plan's row deltas are relative to it), and the
/// last raster so motionless frames reuse the texture.
struct EndpointPreview {
    doc: vsdx::Document,
    conn_id: i64,
    pre_end: (f64, f64),
    pre_beg: (f64, f64),
    /// #218 review: the drag anchor's offset — handle origin minus the
    /// press position at arm time. The COMMIT path moves the endpoint
    /// by the drag DELTA from its own origin (the grab may start
    /// HANDLE_GRAB px off-center); the preview must apply the same
    /// offset to the pointer or it paints where the release won't land.
    grab_off: (f64, f64),
    last_to: (f64, f64),
    tex: Option<egui::TextureHandle>,
    bbox: (f64, f64, f64, f64),
}

/// #304: the comment-avatar initials fitted to the 16pt circle —
/// first by SHRINKING the font (10pt down to 8pt keeps a normal
/// two-letter pair whole), then by truncating, floor of one character
/// (the full author name stays in the header beside the avatar).
/// Returns the fitted text and its font size.
fn fit_initials_to_avatar(ctx: &egui::Context, initials: &str) -> (String, f32) {
    // Bound the candidate first (#305 review): nothing beyond a
    // handful of glyphs can ever clear the circle, and an unbounded
    // paste would make the truncation loop quadratic per frame.
    let initials: String = initials.chars().take(8).collect();
    let width = |text: &str, size: f32| -> f32 {
        ctx.fonts_mut(|f| {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                text,
                0.0,
                egui::TextFormat::simple(egui::FontId::proportional(size), Color32::WHITE),
            );
            f.layout_job(job)
        })
        .size()
        .x
    };
    for size in [10.0f32, 9.5, 9.0, 8.5, 8.0] {
        if width(&initials, size) <= 13.0 {
            return (initials.to_string(), size);
        }
    }
    let mut fitted = initials.to_string();
    while fitted.chars().count() > 1 {
        fitted = fitted.chars().take(fitted.chars().count() - 1).collect();
        if width(&fitted, 8.0) <= 13.0 {
            return (fitted, 8.0);
        }
    }
    (fitted, 8.0)
}

/// Placeholder template names on the backstage New page (generic
/// diagram-type names, ours — template bodies arrive with the M5 flow).
/// Template names translate at paint (#246): resolved per call, so
/// the backstage New page re-renders on a language switch.
fn new_template_entries() -> Vec<(String, Icon)> {
    vec![
        (crate::i18n::strs().msg_flowchart(), Icon::Swimlane),
        (crate::i18n::strs().msg_cross_functional(), Icon::Container),
        (crate::i18n::strs().msg_network_diagram(), Icon::Link),
        (crate::i18n::strs().msg_org_chart(), Icon::Group),
        (crate::i18n::strs().msg_floor_plan(), Icon::Grid),
    ]
}

/// One New-page gallery card: icon over the caption, 2010 tile metrics.
/// The caption is a real Label so the card stays in the accessibility
/// tree (kittest queries it); all colors ride theme tokens (#55).
fn backstage_card(ui: &mut Ui, icon: Icon, name: &str, enabled: bool) -> bool {
    let t = theme::shell_tokens(ui.visuals());
    ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
        let (tile, _) = ui.allocate_exact_size(Vec2::new(120.0, 96.0), Sense::hover());
        ui.painter().rect_filled(tile, 3.0, t.card_bg);
        ui.painter().rect_stroke(
            tile,
            3.0,
            Stroke::new(1.0, t.ribbon_group),
            egui::StrokeKind::Middle,
        );
        icon.paint(
            ui,
            Rect::from_center_size(tile.center(), Vec2::splat(44.0)),
            enabled,
        );
        let tip = if enabled {
            crate::i18n::strs().msg_start_an_empty_drawing()
        } else {
            crate::i18n::strs().msg_template_gallery_arrives_with_th()
        };
        let caption = ui
            .add(
                egui::Label::new(egui::RichText::new(name).size(12.0).color(t.back_title))
                    .selectable(false),
            )
            .on_hover_text(tip);
        let card = tile.union(caption.rect);
        let resp = ui.interact(
            card,
            ui.scope_id().with(("backstage_card", name)),
            Sense::click(),
        );
        if resp.hovered() && enabled {
            ui.painter().rect_stroke(
                card.shrink(1.0),
                3.0,
                Stroke::new(1.5, t.ribbon_hover_edge),
                egui::StrokeKind::Middle,
            );
        }
        enabled && resp.clicked()
    })
    .inner
}

/// elementary-style welcome action: icon beside a bold title with a
/// one-line description — one button keeps it one accessible widget.
fn welcome_action(ui: &mut Ui, icon: Icon, title: &str, desc: &str) -> bool {
    let t = theme::shell_tokens(ui.visuals());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &format!("    {title}"),
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(13.0), t.back_title),
    );
    job.append(
        &format!("\n    {desc}"),
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(10.5), t.ribbon_label),
    );
    let resp = ui.add(egui::Button::new(job).min_size(Vec2::new(280.0, 44.0)));
    let r = resp.rect;
    icon.paint(
        ui,
        Rect::from_center_size(Pos2::new(r.left() + 16.0, r.center().y), Vec2::splat(20.0)),
        true,
    );
    resp.clicked()
}

/// A labelled placeholder page: context sentence plus a clearly-named
/// disabled action — no dead ends (elementary), no vague labels.
fn backstage_stub(ui: &mut Ui, title: &str, context: &str, action: &str, tip: &str) {
    ui.heading(title);
    ui.add_space(6.0);
    ui.weak(context);
    ui.add_space(10.0);
    ui.add_enabled(false, egui::Button::new(action))
        .on_disabled_hover_text(tip);
}

/// Encode an egui ColorImage as PNG (via the raster stack we already ship).
/// One context-menu row: full-width flat button, optional MDI icon on the
/// left, a gray shortcut hint after the label (Visio shows it right-
/// aligned; a single text run keeps the row one accessible widget).
/// Label and hint colors come from the theme mapping over the live
/// visuals, so rows stay readable in dark mode (GitHub #48).
/// #257 item 3: the shared dialog-body scope. The planned RTL form
/// was `top_down(Align::RIGHT)` — but egui 0.37's right-aligned
/// vertical scope DEGENERATES inside nested children (each checkbox
/// label wraps one character per line; observed in the Persian Snap &
/// Glue golden), so the body scope keeps the plain LTR vertical
/// layout in every language. The RTL dialog parity that matters is
/// carried by [`row`]/[`row_rev`] (mirrored row, column and button
/// order) and egui's own bidi shaping of each label; full-body
/// right-alignment is recorded as an egui limitation on the issue.
pub(crate) fn dialog_dir() -> egui::Layout {
    egui::Layout::top_down(egui::Align::LEFT)
}

/// A mirrored row: the direction flip runs INSIDE a `ui::horizontal`
/// child (its wrap-aware measure shields the parent — a bare
/// `with_layout(left_to_right)` scope in a vertical parent reserves the
/// parent's full cross space, voiding everything below it; the
/// pre-existing end-pinned bars wrap the flip in horizontal for
/// exactly that reason).
pub(crate) fn row<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> egui::Response {
    let dir = if crate::i18n::rtl() {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    };
    ui.horizontal(|inner| inner.with_layout(dir, add_contents))
        .response
}

/// The INVERSE of [`row`] — an end-pinned control bar whose LTR form
/// was `right_to_left` and mirrors to `left_to_right` under RTL.
pub(crate) fn row_rev<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> egui::Response {
    let dir = if crate::i18n::rtl() {
        egui::Layout::left_to_right(egui::Align::Center)
    } else {
        egui::Layout::right_to_left(egui::Align::Center)
    };
    ui.horizontal(|inner| inner.with_layout(dir, add_contents))
        .response
}

/// The horizontal-row twin of [`dialog_dir`] — the `right_to_left` /
/// `left_to_right` pair the mirrored strips use.
fn ctx_item(
    ui: &mut Ui,
    icon: Option<crate::ribbon::Icon>,
    label: &str,
    hint: &str,
    enabled: bool,
) -> bool {
    let tokens = theme::menu_tokens(ui.visuals());
    // #257 item 3: an RTL menu mirrors the row — the icon hugs the
    // right edge, the label right-aligns (its icon gutter on the
    // right), and the accelerator hint pins the LEFT edge (Office's
    // RTL menus keep the shortcut column at the mirrored leading
    // edge).
    let rtl = crate::i18n::rtl();
    let mut job = egui::text::LayoutJob {
        halign: if rtl {
            egui::Align::RIGHT
        } else {
            egui::Align::LEFT
        },
        ..egui::text::LayoutJob::default()
    };
    let text = if icon.is_some() {
        format!("      {label}")
    } else {
        format!("  {label}")
    };
    job.append(
        &text,
        0.0,
        egui::TextFormat::simple(
            egui::FontId::proportional(12.5),
            if enabled {
                tokens.label
            } else {
                tokens.label_disabled
            },
        ),
    );
    let mut trailing_hint = false;
    if !hint.is_empty() && !rtl {
        job.append(
            &format!("   {hint}"),
            0.0,
            egui::TextFormat::simple(egui::FontId::proportional(11.0), tokens.hint),
        );
        trailing_hint = true;
    }
    let resp = ui.add_enabled(
        enabled,
        egui::Button::new(job).min_size(Vec2::new(ui.available_width().max(150.0), 21.0)),
    );
    if let Some(ic) = icon {
        let x = if rtl {
            resp.rect.right() - 23.0
        } else {
            resp.rect.left() + 7.0
        };
        let r = Rect::from_min_size(Pos2::new(x, resp.rect.center().y - 8.0), Vec2::splat(16.0));
        // icons take the theme-derived tint: the light Office palette
        // (the gray ShapeSheet glyph included) vanishes on a dark menu
        // (GitHub #48 review).
        ic.paint_tinted(ui, r, tokens.icon_tint(ic.office_color(), enabled));
    }
    if !trailing_hint && !hint.is_empty() {
        // RTL accelerator column: a left-pinned overlay galley (the
        // hint cannot ride inside the right-aligned label job).
        let gal = ui.painter().layout_no_wrap(
            hint.to_string(),
            egui::FontId::proportional(11.0),
            tokens.hint,
        );
        ui.painter().galley(
            Pos2::new(
                resp.rect.left() + 7.0,
                resp.rect.center().y - gal.size().y / 2.0,
            ),
            gal,
            tokens.hint,
        );
    }
    resp.clicked()
}

fn write_png(path: &std::path::Path, img: &egui::ColorImage) -> anyhow::Result<()> {
    let [w, h] = img.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for p in &img.pixels {
        rgba.extend_from_slice(p.to_srgba_unmultiplied().as_slice());
    }
    let pm = resvg::tiny_skia::Pixmap::from_vec(
        rgba,
        resvg::tiny_skia::IntSize::from_wh(w as u32, h as u32).unwrap(),
    )
    .ok_or_else(|| anyhow::anyhow!("pixmap from frame"))?;
    std::fs::write(path, pm.encode_png()?)?;
    Ok(())
}

/// Current UTC date as `YYYY-MM-DD` — zero-dep civil-from-days
/// (Hinnant's algorithm) over the Unix epoch seconds.
fn format_now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Whether a key carries a single-character name — in this egui's
/// `Key` that is exactly the letters ("A".."Z") and digits
/// ("0".."9"), the keytip alphabet (every other key names a word).
fn is_keytip_key(k: egui::Key) -> bool {
    k.name().chars().count() == 1
}

/// One QAT button; returns its laid-out rect. `digit` is the #184
/// keytip badge (Office numbers the QAT 1..=9 by position); the
/// action closure is a pure constructor, evaluated once for the
/// badge and shared with the click.
fn qat_button(
    ui: &mut Ui,
    icon: Icon,
    tip: &str,
    action: impl FnOnce() -> Action,
    app: &mut App,
    digit: u8,
) -> Rect {
    let a = action();
    let (r, resp) = ui.allocate_exact_size(Vec2::new(22.0, 22.0), Sense::click());
    // #184: the QAT digit badge (Office numbers the QAT 1..=9).
    app.keytips.badges.push(crate::keytips::Badge {
        rect: r,
        key: digit.to_string(),
        hit: crate::keytips::Hit::Action {
            action: a,
            enabled: true,
            label: String::new(),
        },
        anchor: None,
    });
    // The icon is painted, so the control needs its name attached;
    // screen readers read "Save", not "Save (Ctrl+S)" (GitHub #56).
    let name = tip.split(" (").next().unwrap_or(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
    let resp = resp.on_hover_text(tip);
    let hover = resp.hovered();
    if hover {
        ui.painter()
            .rect_filled(r, 3.0, Color32::from_white_alpha(46));
    }
    icon.paint(
        ui,
        Rect::from_center_size(r.center(), Vec2::splat(18.0)),
        true,
    );
    if resp.clicked() {
        app.dispatch(a);
    }
    r
}

/// Tab captions come from the catalog (#246): resolved per call, so
/// the strip re-renders on a language switch.
fn t_title(t: Tab) -> String {
    match t {
        Tab::File => crate::i18n::strs().msg_file(),
        Tab::Home => crate::i18n::strs().msg_home(),
        Tab::Insert => crate::i18n::strs().msg_insert(),
        Tab::Design => crate::i18n::strs().msg_design(),
        Tab::Data => crate::i18n::strs().msg_data(),
        Tab::Review => crate::i18n::strs().msg_review(),
        Tab::View => crate::i18n::strs().msg_view(),
        Tab::Developer => crate::i18n::strs().msg_developer(),
        Tab::ContainerTools => crate::i18n::strs().msg_container_tools(),
    }
}

/// A master's root-shape size in inches (for the drag ghost).
fn master_size(doc: &Document, id: i64) -> (f64, f64) {
    doc.master(id)
        .and_then(|m| {
            m.shapes.first().map(|s| {
                let g = |n: &str| s.sheet.cell(n).and_then(|c| c.numeric()).unwrap_or(1.0);
                (g("Width"), g("Height"))
            })
        })
        .unwrap_or((1.0, 1.0))
}

/// Point-in-bbox test with a small pad (~1 mm): 1D lines have a
/// degenerate dimension (Height 0 when horizontal), and a zero-thickness
/// bbox is otherwise unhittable (GitHub #59). Visio hit-tests lines with
/// a tolerance too.
fn hit_in_rect(pos: (f64, f64), x: f64, y: f64, w: f64, h: f64) -> bool {
    const PAD: f64 = 0.04; // inches
    pos.0 >= x - PAD && pos.0 <= x + w + PAD && pos.1 >= y - PAD && pos.1 <= y + h + PAD
}

/// Every shape of a subtree, document (z) order, recursive (#78):
/// the group-edit mode's hit-test and marquee candidate set.
/// Distance from `p` to the segment a→b within `threshold` (#108
/// connector hit-testing).
fn segment_hit(a: (f64, f64), b: (f64, f64), p: (f64, f64), threshold: f64) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-12 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    (p.0 - cx).hypot(p.1 - cy) <= threshold
}

/// Connection rows of one sheet → page-space connection points on
/// `t` (#109): rows carry shape-local X/Y (often cached formula
/// values, e.g. `Width*0.5`), mapped through the same unrotated
/// bbox frame as the #137 vertex handles.
fn collect_connection_points(t: &mut crate::snap::SnapTarget, sheet: &vsdx::model::Sheet) {
    for sec in sheet.sections_named("Connection") {
        for row in &sec.rows {
            if let (Some(x), Some(y)) = (
                row.cell("X").and_then(|c| c.numeric()),
                row.cell("Y").and_then(|c| c.numeric()),
            ) {
                t.connection_points.push((t.rect.0 + x, t.rect.1 + y));
            }
        }
    }
}

/// One-line description of a snap landing, for status notes (#109).
fn snap_hit_text(h: crate::snap::SnapHit) -> String {
    use crate::snap::SnapHit::*;
    match h {
        Grid => crate::i18n::strs().msg_the_grid(),
        Ruler => crate::i18n::strs().msg_a_ruler_subdivision(),
        Guide(id) => crate::i18n::strs().msg_guide_id(id),
        AlignmentBox(id) => crate::i18n::strs().msg_shape_id_s_alignment_box(id),
        ShapeExtension(id) => crate::i18n::strs().msg_shape_id_s_extension(id),
        ShapeIntersection => crate::i18n::strs().msg_a_shape_intersection(),
        ConnectionPoint(id) => crate::i18n::strs().msg_shape_id_s_connection_point(id),
        ShapeHandle(id) => crate::i18n::strs().msg_shape_id_s_handle(id),
        ShapeVertex(id) => crate::i18n::strs().msg_shape_id_s_vertex(id),
    }
}

fn collect_members<'a>(shapes: &'a [Shape], out: &mut Vec<&'a Shape>) {
    for s in shapes {
        out.push(s);
        collect_members(&s.sub_shapes, out);
    }
}

/// Whether a Protection-section flag (`LockWidth`, `LockHeight`,
/// `LockBegin`, `LockEnd`, …) is effectively ON for a shape: its own
/// sheet first, then the master's via the inheritance chain. Cells may
/// sit at section level or in the section's single row, so both spots
/// are checked (GitHub #59).
fn protection_flag(doc: &Document, sh: &Shape, chain: &[&Shape], name: &str) -> bool {
    fn sheet_locked(sheet: &vsdx::model::Sheet, name: &str) -> bool {
        let on = |c: Option<&vsdx::model::Cell>| {
            c.and_then(|c| c.numeric())
                .map(|v| v != 0.0)
                .unwrap_or(false)
        };
        on(sheet.cell(name))
            || sheet
                .sections_named("Protection")
                .iter()
                .any(|sec| on(sec.cell(name)) || sec.rows.iter().any(|r| on(r.cell(name))))
    }
    sheet_locked(&sh.sheet, name)
        || doc
            .instance_source(sh, chain)
            .map(|src| sheet_locked(&src.sheet, name))
            .unwrap_or(false)
}

/// Title of the stencil list entry: the open file's stem (a .vssx
/// stencil is the whole document), else the document's own master set
/// (GitHub #53: the pane lists its stencil like Visio does).
/// `My Shapes` under a home directory (pure half of
/// [`App::my_shapes_dir`] for testing).
fn my_shapes_path(home: Option<PathBuf>) -> PathBuf {
    home.unwrap_or_else(|| PathBuf::from("."))
        .join("Documents")
        .join("My Shapes")
}

fn stencil_title(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::i18n::strs().msg_document_masters())
}

/// What double-clicking a shape should do, from its `EventDblClick`
/// formula (`F` attribute, resolved instance → master).
#[derive(Debug, PartialEq)]
enum DblClick {
    /// Open the shape's text window: `OPENTEXTWIN()`, `DEFAULTEVENT()`
    /// (Visio's default action for a shape with text is editing it), or
    /// no formula / `No Formula`.
    OpenText,
    /// A formula whose action belongs to a later milestone (`RUNADDON`,
    /// `RUNMACRO`, `GOTOPAGE`, `DONOTHING`, …): report, open nothing.
    Deferred(String),
}

/// Classify an `EventDblClick` formula string.
fn classify_dbl_click(formula: Option<&str>) -> DblClick {
    let f = formula.unwrap_or("").trim();
    // Compare the function head (everything before the argument list).
    let head = f
        .split('(')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_uppercase();
    if f.is_empty()
        || f.eq_ignore_ascii_case("No Formula")
        || head == "OPENTEXTWIN"
        || head == "DEFAULTEVENT"
    {
        DblClick::OpenText
    } else {
        DblClick::Deferred(crate::i18n::strs().msg_eventdblclick_f_macro_and_naviga(f))
    }
}

/// #156: a stencil's master by NameU (SDCONT's gallery names).
fn find_master_by_name<'a>(doc: &'a Document, name: &str) -> Option<&'a vsdx::Master> {
    doc.masters
        .iter()
        .find(|m| m.name_u.as_deref() == Some(name))
}

// --- module split (move-only): the impl blocks live in the files below;
// --- every type and free helper above keeps its original path, so
// --- `crate::app::*` references inside and outside the crate are stable.
mod backstage;
mod central;
mod comments;
mod connectors;
mod construct;
mod containers;
mod ctxmenu;
mod debug;
mod dialogs;
mod doc;
mod draw;
mod editops;
mod files;
mod find;
mod format;
mod keys;
mod pages;
mod panzoom;
mod pictures;
mod pointer;
mod selection;
mod shell;
mod sizepos;
mod slideshow;
mod snap;
mod spell;

#[cfg(test)]
mod tests;
