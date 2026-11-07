//! libghostty-vt, the terminal itself: what the program in a pane has
//! written, kept as a grid of cells with its scrollback, and keys turned
//! into what the program expects to read. A thin, safe face on its C API,
//! with only what jterm uses.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

#[allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code, clippy::all)]
pub mod sys {
    include!("ghostty_sys.rs");
}
use sys::*;

pub const BOLD: u16 = 1 << 0;
pub const FAINT: u16 = 1 << 2;
pub const INVERSE: u16 = 1 << 4;
pub const INVISIBLE: u16 = 1 << 5;
pub const STRIKE: u16 = 1 << 6;
const UNDERLINE_MASK: u16 = 0x7 << 8;

/// What the terminal writes back to the program, as the answer to a query
/// it made (where the cursor is, what the terminal is): handed to the pane,
/// which writes it to the program's PTY.
pub type Reply = Box<dyn FnMut(&[u8])>;

/// A colour as 0x00RRGGBB.
pub type Rgb = u32;

fn rgb(c: GhosttyColorRgb) -> Rgb {
    (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
}

fn to_ghostty(c: Rgb) -> GhosttyColorRgb {
    GhosttyColorRgb { r: (c >> 16) as u8, g: (c >> 8) as u8, b: c as u8 }
}

/// One cell as it is to be drawn: its text (empty for a blank or for the
/// second half of a wide character), colours resolved, and its attributes.
#[derive(Clone, Default)]
pub struct Cell {
    pub text: String,
    pub fg: Option<Rgb>,
    pub bg: Option<Rgb>,
    pub flags: u16,
    pub selected: bool,
}

impl Cell {
    pub fn underline(&self) -> bool {
        self.flags & UNDERLINE_MASK != 0
    }
}

pub struct Cursor {
    pub x: u16,
    pub y: u16,
    /// On screen and not hidden by the program.
    pub visible: bool,
    /// Bar, block, underline or hollow block, libghostty's numbering.
    pub style: u32,
    pub color: Option<Rgb>,
}

pub struct Terminal {
    raw: GhosttyTerminal,
    render: GhosttyRenderState,
    rows: GhosttyRenderStateRowIterator,
    cells: GhosttyRenderStateRowCells,
    encoder: GhosttyKeyEncoder,
    event: GhosttyKeyEvent,
    /// Kept boxed so its address, handed to libghostty, holds still.
    reply: Box<Reply>,
}

unsafe extern "C" fn on_write_pty(_t: GhosttyTerminal, user: *mut c_void, data: *const u8, len: usize) {
    if user.is_null() || data.is_null() {
        return;
    }
    let reply = &mut *(user as *mut Reply);
    reply(std::slice::from_raw_parts(data, len));
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback: usize, fg: Rgb, bg: Rgb, reply: Reply) -> Terminal {
        unsafe {
            let mut raw = null_mut();
            assert_eq!(ghostty_terminal_new(null(), &mut raw, cols.max(1), rows.max(1)), GHOSTTY_SUCCESS, "libghostty: terminal_new");
            let mut render = null_mut();
            ghostty_render_state_new(null(), &mut render);
            let mut row_iter = null_mut();
            ghostty_render_state_row_iterator_new(null(), &mut row_iter);
            let mut cells = null_mut();
            ghostty_render_state_row_cells_new(null(), &mut cells);
            let mut encoder = null_mut();
            ghostty_key_encoder_new(null(), &mut encoder);
            let mut event = null_mut();
            ghostty_key_event_new(null(), &mut event);
            let mut term = Terminal { raw, render, rows: row_iter, cells, encoder, event, reply: Box::new(reply) };
            let user: *mut Reply = &mut *term.reply;
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_USERDATA, user as *const c_void);
            let f: GhosttyTerminalWritePtyFn = Some(on_write_pty);
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_WRITE_PTY, f.map_or(null(), |f| f as *const c_void));
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &scrollback as *const usize as *const c_void);
            let (fg, bg) = (to_ghostty(fg), to_ghostty(bg));
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &fg as *const _ as *const c_void);
            ghostty_terminal_set(raw, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &bg as *const _ as *const c_void);
            term
        }
    }

    /// What the program wrote, read in.
    pub fn write(&mut self, bytes: &[u8]) {
        unsafe { ghostty_terminal_vt_write(self.raw, bytes.as_ptr(), bytes.len()) }
    }

    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u32, cell_h: u32) {
        unsafe {
            ghostty_terminal_resize(self.raw, cols.max(1), rows.max(1), cell_w, cell_h);
        }
    }

    /// Scrolls the view by `delta` rows, up when negative.
    pub fn scroll(&mut self, delta: isize) {
        let mut how = GhosttyTerminalScrollViewport { tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA, value: unsafe { std::mem::zeroed() } };
        how.value.delta = delta;
        unsafe { ghostty_terminal_scroll_viewport(self.raw, how) };
    }

    pub fn scroll_to_bottom(&mut self) {
        let how = GhosttyTerminalScrollViewport { tag: GHOSTTY_TERMINAL_SCROLL_VIEWPORT_BOTTOM, value: unsafe { std::mem::zeroed() } };
        unsafe { ghostty_terminal_scroll_viewport(self.raw, how) };
    }

    /// Whether the view is at the bottom, where the program writes.
    pub fn at_bottom(&self) -> bool {
        let mut active = true;
        unsafe { ghostty_terminal_get(self.raw, GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, &mut active as *mut bool as *mut c_void) };
        active
    }

    /// Whether the program is on the alternate screen, as a pager or an
    /// editor is.
    pub fn alternate_screen(&self) -> bool {
        let mut screen: GhosttyTerminalScreen = 0;
        unsafe { ghostty_terminal_get(self.raw, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &mut screen as *mut _ as *mut c_void) };
        screen != 0
    }

    fn string(&self, data: GhosttyTerminalData) -> String {
        let mut s = GhosttyString { ptr: null(), len: 0 };
        unsafe {
            if ghostty_terminal_get(self.raw, data, &mut s as *mut _ as *mut c_void) != GHOSTTY_SUCCESS || s.ptr.is_null() {
                return String::new();
            }
            String::from_utf8_lossy(std::slice::from_raw_parts(s.ptr, s.len)).into_owned()
        }
    }

    /// The title the program gave the window, if it gave one.
    pub fn title(&self) -> String {
        self.string(GHOSTTY_TERMINAL_DATA_TITLE)
    }

    /// Clears the screen and the scrollback, the cursor to the top: Cmd-K.
    pub fn clear(&mut self) {
        self.write(b"\x1b[H\x1b[2J\x1b[3J");
    }

    /// A key, as the program expects to read it given the modes it has
    /// set (application cursor keys, the kitty keyboard protocol). Empty
    /// for a key that sends nothing.
    pub fn encode_key(&mut self, key: GhosttyKey, mods: u16, action: GhosttyKeyAction, text: &str, unshifted: u32) -> Vec<u8> {
        unsafe {
            ghostty_key_encoder_set_from_terminal(self.encoder, self.raw);
            ghostty_key_event_set_action(self.event, action);
            ghostty_key_event_set_key(self.event, key);
            ghostty_key_event_set_mods(self.event, mods);
            ghostty_key_event_set_consumed_mods(self.event, 0);
            ghostty_key_event_set_unshifted_codepoint(self.event, unshifted);
            // Control characters are left out: the encoder works them out
            // from the key and the modifiers.
            let text = if text.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f) { "" } else { text };
            ghostty_key_event_set_utf8(self.event, GhosttyString { ptr: text.as_ptr(), len: text.len() });
            let mut buf = [0u8; 128];
            let mut n = 0usize;
            let r = ghostty_key_encoder_encode(self.encoder, self.event, buf.as_mut_ptr(), buf.len(), &mut n);
            // Clear the text before it goes out of scope.
            ghostty_key_event_set_utf8(self.event, GhosttyString { ptr: null(), len: 0 });
            if r == GHOSTTY_SUCCESS {
                buf[..n].to_vec()
            } else {
                Vec::new()
            }
        }
    }

    /// Text pasted, wrapped in bracketed paste when the program asked for
    /// it.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        let bracketed = self.mode_on(2004);
        let mut out = Vec::with_capacity(text.len() + 12);
        if bracketed {
            out.extend_from_slice(b"\x1b[200~");
        }
        // Newlines go as carriage returns, as Enter does, and nothing in
        // the paste may end the bracket early.
        for b in text.replace("\r\n", "\r").replace('\n', "\r").replace("\x1b[201~", "").bytes() {
            out.push(b);
        }
        if bracketed {
            out.extend_from_slice(b"\x1b[201~");
        }
        out
    }

    /// Whether a DEC private mode is set (2004, bracketed paste).
    fn mode_on(&self, number: u16) -> bool {
        let mut q = GhosttyTerminalModeConfig { mode: number & 0x7fff, value: false };
        unsafe { ghostty_terminal_get(self.raw, GHOSTTY_TERMINAL_DATA_MODE, &mut q as *mut _ as *mut c_void) == GHOSTTY_SUCCESS && q.value }
    }

    /// Brings the snapshot to be drawn up to date. Returns 0 when nothing
    /// changed, 1 when some rows did, 2 when all of it should be drawn.
    pub fn update(&mut self) -> u32 {
        unsafe {
            ghostty_render_state_update(self.render, self.raw);
            let mut dirty: GhosttyRenderStateDirty = 0;
            ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_DIRTY, &mut dirty as *mut _ as *mut c_void);
            dirty
        }
    }

    pub fn colors(&self) -> (Rgb, Rgb) {
        unsafe {
            let mut c: GhosttyRenderStateColors = std::mem::zeroed();
            c.size = std::mem::size_of::<GhosttyRenderStateColors>();
            ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_COLORS, &mut c as *mut _ as *mut c_void);
            (rgb(c.foreground), rgb(c.background))
        }
    }

    pub fn cursor(&self) -> Cursor {
        unsafe {
            let mut c: GhosttyRenderStateCursor = std::mem::zeroed();
            c.size = std::mem::size_of::<GhosttyRenderStateCursor>();
            ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &mut c as *mut _ as *mut c_void);
            let mut colors: GhosttyRenderStateColors = std::mem::zeroed();
            colors.size = std::mem::size_of::<GhosttyRenderStateColors>();
            ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_COLORS, &mut colors as *mut _ as *mut c_void);
            Cursor {
                x: c.viewport_x,
                y: c.viewport_y,
                visible: c.visible && c.viewport_has_value,
                style: c.visual_style,
                color: colors.cursor_has_value.then(|| rgb(colors.cursor)),
            }
        }
    }

    /// Calls `draw` with each row to be drawn: every row when `all`, else
    /// the rows that changed and `extra` (where the cursor was and is).
    /// The cells are filled into `row`, which is reused.
    pub fn rows(&mut self, all: bool, extra: &[u16], row: &mut Vec<Cell>, mut draw: impl FnMut(u16, &[Cell])) {
        unsafe {
            ghostty_render_state_get(self.render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &mut self.rows as *mut _ as *mut c_void);
            let mut y: u16 = 0;
            let mut codepoints = [0u32; 16];
            while ghostty_render_state_row_iterator_next(self.rows) {
                let mut dirty = false;
                ghostty_render_state_row_get(self.rows, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &mut dirty as *mut bool as *mut c_void);
                if all || dirty || extra.contains(&y) {
                    row.clear();
                    ghostty_render_state_row_get(self.rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &mut self.cells as *mut _ as *mut c_void);
                    while ghostty_render_state_row_cells_next(self.cells) {
                        let mut cell = Cell::default();
                        let mut len: u32 = 0;
                        ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, &mut len as *mut u32 as *mut c_void);
                        if len > 0 {
                            let mut buf = GhosttyCodepointBuffer { ptr: codepoints.as_mut_ptr(), cap: codepoints.len(), len: 0 };
                            ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, &mut buf as *mut _ as *mut c_void);
                            cell.text = codepoints[..buf.len.min(buf.cap)].iter().filter_map(|&c| char::from_u32(c)).collect();
                        }
                        let mut styled = false;
                        ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_HAS_STYLING, &mut styled as *mut bool as *mut c_void);
                        if styled {
                            let mut style: GhosttyStyle = std::mem::zeroed();
                            ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &mut style as *mut _ as *mut c_void);
                            cell.flags = style.flags;
                            let mut c = GhosttyColorRgb { r: 0, g: 0, b: 0 };
                            if ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &mut c as *mut _ as *mut c_void) == GHOSTTY_SUCCESS {
                                cell.fg = Some(rgb(c));
                            }
                            if ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &mut c as *mut _ as *mut c_void) == GHOSTTY_SUCCESS {
                                cell.bg = Some(rgb(c));
                            }
                        }
                        ghostty_render_state_row_cells_get(self.cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED, &mut cell.selected as *mut bool as *mut c_void);
                        row.push(cell);
                    }
                    draw(y, row);
                }
                y += 1;
            }
            ghostty_render_state_clean(self.render);
        }
    }

    fn grid_ref(&self, x: u16, y: u16) -> Option<GhosttyGridRef> {
        unsafe {
            let point = GhosttyPoint { tag: GHOSTTY_POINT_TAG_VIEWPORT, value: GhosttyPointValue { coordinate: GhosttyPointCoordinate { x, y: u32::from(y) } } };
            let mut r: GhosttyGridRef = std::mem::zeroed();
            (ghostty_terminal_grid_ref(self.raw, point, &mut r) == GHOSTTY_SUCCESS).then_some(r)
        }
    }

    /// Selects from one cell of the view to another, both included.
    pub fn select(&mut self, from: (u16, u16), to: (u16, u16)) {
        let (Some(start), Some(end)) = (self.grid_ref(from.0, from.1), self.grid_ref(to.0, to.1)) else { return };
        let sel = GhosttySelection { start, end, rectangle: false };
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, &sel as *const _ as *const c_void) };
    }

    /// Selects the word under a cell, as a double click does.
    pub fn select_word(&mut self, at: (u16, u16)) {
        let Some(r) = self.grid_ref(at.0, at.1) else { return };
        unsafe {
            let mut opts: GhosttyTerminalSelectWordOptions = std::mem::zeroed();
            opts.size = std::mem::size_of::<GhosttyTerminalSelectWordOptions>();
            opts.ref_ = r;
            let mut sel: GhosttySelection = std::mem::zeroed();
            if ghostty_terminal_select_word(self.raw, &opts, &mut sel) == GHOSTTY_SUCCESS {
                ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, &sel as *const _ as *const c_void);
            }
        }
    }

    pub fn clear_selection(&mut self) {
        unsafe { ghostty_terminal_set(self.raw, GHOSTTY_TERMINAL_OPT_SELECTION, null()) };
    }

    /// The text selected, as plain text, or None when nothing is.
    pub fn selection_text(&self) -> Option<String> {
        unsafe {
            let mut sel: GhosttySelection = std::mem::zeroed();
            if ghostty_terminal_get(self.raw, GHOSTTY_TERMINAL_DATA_SELECTION, &mut sel as *mut _ as *mut c_void) != GHOSTTY_SUCCESS {
                return None;
            }
            let mut f = null_mut();
            ghostty_formatter_terminal_new(null(), &mut f, self.raw);
            let emit: GhosttyFormatterFormat = GHOSTTY_FORMATTER_FORMAT_PLAIN;
            ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_EMIT, &emit as *const _ as *const c_void);
            let (trim, unwrap) = (true, true);
            ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_TRIM, &trim as *const bool as *const c_void);
            ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_UNWRAP, &unwrap as *const bool as *const c_void);
            ghostty_formatter_set(f, GHOSTTY_FORMATTER_OPT_SELECTION, &sel as *const _ as *const c_void);
            let (mut ptr, mut len) = (null_mut(), 0usize);
            let r = ghostty_formatter_format_alloc(f, null(), &mut ptr, &mut len);
            let text = (r == GHOSTTY_SUCCESS && !ptr.is_null()).then(|| String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned());
            if !ptr.is_null() {
                ghostty_free(null(), ptr, len);
            }
            ghostty_formatter_free(f);
            text.filter(|t| !t.is_empty())
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ghostty_key_event_free(self.event);
            ghostty_key_encoder_free(self.encoder);
            ghostty_render_state_row_cells_free(self.cells);
            ghostty_render_state_row_iterator_free(self.rows);
            ghostty_render_state_free(self.render);
            ghostty_terminal_free(self.raw);
        }
    }
}
