//! The host → module: what a module gets each call ([`Input`]): its size, theme, events and
//! the host data it asked for.

use super::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Input {
    /// Seconds since the module was loaded.
    pub time: f64,
    /// Content area in points.
    pub width: f32,
    pub height: f32,
    /// Whether this module has keyboard focus.
    pub focused: bool,
    /// The active theme; modules must take every colour from here.
    pub theme: Theme,
    /// Input since the last call, in order. Events this SDK doesn't know yet (from a
    /// newer the host) are skipped rather than failing the whole input.
    #[serde(deserialize_with = "known_events")]
    pub events: Vec<Event>,
    /// Data the module asked for in [`Output::wants`].
    pub data: HostData,
    /// Where the module's tab works (its folder and branch), when `wants.window` and
    /// capability `window.folder`. `None` otherwise.
    #[serde(default)]
    pub window: Option<WindowContext>,
    /// What its window was opened for
    /// (a commit for the diff). `None` otherwise.
    #[serde(default)]
    pub arg: Option<String>,
    /// The modifier keys held during this call's input (for a click or drag on a
    /// canvas: shift-click, ctrl-drag).
    #[serde(default)]
    pub mods: Mods,
    /// What this window was opened to show, when the user opened a file or link with this
    /// module (it declares `handles:` patterns in its manifest), in every call. A file comes
    /// with read access to that one file (for this build, until the host exits); a link is
    /// just its URL.
    #[serde(default)]
    pub open: Option<OpenTarget>,
    /// The module this one was opened over, in the same window (the git graph, for the
    /// diff of a commit): [`Action::Back`] then goes back to it, which the module says by
    /// asking [`Effect::Close`](crate::Effect::Close). `None` in a window of its own.
    #[serde(default)]
    pub opened_from: Option<String>,
    /// Raw bytes [`Blob`]s in [`HostData::fs`] point into (file contents), passed beside
    /// the JSON rather than in it. Filled by the SDK from the host's second buffer.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

/// Modifier keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Input {
    /// The bytes `blob` stands for (empty when it doesn't fit: never panics).
    pub fn blob(&self, blob: Blob) -> &[u8] {
        let start = blob.start as usize;
        self.bytes.get(start..start.saturating_add(blob.len as usize)).unwrap_or_default()
    }

    /// The answer to the file request (or picker) with `id` in this call, if it came.
    pub fn fs_result(&self, id: &str) -> Option<&FsReply> {
        self.data.fs.iter().find(|r| r.id == id).map(|r| &r.reply)
    }
}

/// The events of `events` this build knows; the rest are skipped.
fn known_events<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Event>, D::Error> {
    let raw: Vec<serde_json::Value> = Deserialize::deserialize(d)?;
    Ok(raw.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
}

/// A run of bytes beside the JSON: [`Input::bytes`] (the host → module) or
/// [`Output::bytes`] (module → the host), `start..start + len`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blob {
    pub start: u32,
    pub len: u32,
}

/// Where the window a module belongs to works.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowContext {
    /// Its folder: the git checkout, else the shell's working directory.
    pub folder: String,
    /// The branch checked out there (`HEAD` when detached); `None` until it's read, or
    /// outside a git repo.
    #[serde(default)]
    pub branch: Option<String>,
    /// The saved herdr machine the window is on (its name), when it isn't on this
    /// computer: `folder` is a path there, and git runs there.
    #[serde(default)]
    pub machine: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub space: Color,
    pub surface: Color,
    pub text: Color,
    pub subtext: Color,
    pub muted: Color,
    pub accent: Color,
    pub border: Color,
    /// The 16 ANSI colours: black, red, green, yellow, blue, magenta, cyan, white,
    /// then the bright variants.
    pub ansi: Vec<Color>,
    /// Corners, padding and gaps, in points. The host already applies them to the
    /// components it renders; a module painting its own canvas can follow them too.
    #[serde(default)]
    pub density: Density,
    /// The app's fonts.
    #[serde(default)]
    pub fonts: Fonts,
}

/// The app's fonts by role (empty: unknown).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fonts {
    /// The UI font: `"system"` (the desktop's sans-serif) or a family name. Text in
    /// modules uses it, except [`TextStyle::Mono`](crate::TextStyle::Mono), [`Font::Mono`](crate::Font::Mono)
    /// and monospace fields.
    pub ui: String,
    /// The mono font (terminals, code): a family name.
    pub mono: String,
}
/// The sizes the components are drawn at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Density {
    /// Corner radius of windows and panels.
    pub window_radius: f32,
    /// Of tiles (cards).
    pub tile_radius: f32,
    /// Of buttons, fields and list rows.
    pub button_radius: f32,
    /// Of pills (255: round ends).
    pub pill_radius: f32,
    /// A window's header bar.
    pub header_height: f32,
    /// Inside a window, around its content.
    pub window_padding: f32,
    /// Inside a tile.
    pub tile_padding: f32,
    /// A button's padding left and right.
    pub button_padding: f32,
    /// A list row's height over its text.
    pub row_padding: f32,
    /// Between windows.
    pub gap: f32,
    /// What the host multiplies container gaps by (1 at the `default` preset).
    pub scale: f32,
}

impl Default for Density {
    fn default() -> Self {
        Density {
            window_radius: 12.0,
            tile_radius: 8.0,
            button_radius: 6.0,
            pill_radius: 255.0,
            header_height: 40.0,
            window_padding: 14.0,
            tile_padding: 10.0,
            button_padding: 6.0,
            row_padding: 10.0,
            gap: 10.0,
            scale: 1.0,
        }
    }
}

/// A file or link the user opened with a module ([`Input::open`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OpenTarget {
    /// A file the module may now read (`~/…` under the home folder, as the file requests
    /// take it), maybe at a line (and column).
    File {
        path: String,
        #[serde(default)]
        line: Option<u32>,
        #[serde(default)]
        col: Option<u32>,
    },
    /// An `http(s)` URL.
    Url(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// A pressable [`Node::Column`]/[`Node::Row`] (one with an `id`, e.g. a button) was
    /// clicked, or a list item's trailing control ([`Trailing`]). A click on a virtual
    /// list's row also sends `<list id>#<row>` (after its [`Event::Selected`]), which
    /// the keys never do: a tree's folder can open on one click and stay put under the
    /// arrows.
    Clicked { id: String },
    /// The selection of a [`Node::List`] moved to row `index`: a click, or (in a list
    /// with [`Node::List::keyboard`]) the keys.
    Selected { id: String, index: usize },
    /// A row of a [`Node::List`] was chosen: Enter on the selection (with
    /// [`Node::List::keyboard`]) or a double-click.
    Activated { id: String, index: usize },
    /// The pointer on a [`Node::Canvas`]: a press, drag or release of the main button,
    /// and with [`CanvasSense::hover`] its moves, entering and leaving. In the canvas's
    /// coordinates: with [`Node::Canvas::view`] those are world coordinates (the view's
    /// pan and zoom undone). Modifiers: [`Input::mods`].
    Pointer { id: String, x: f32, y: f32, kind: PointerKind },
    /// The wheel or a two-finger touchpad scroll over a canvas with
    /// [`CanvasSense::wheel`] (and no `view`, which uses it to pan): `dx`, `dy` in points
    /// (positive: content moves right / down, as a scroll), at `(x, y)` in canvas
    /// coordinates. Over the module's page (a [`Node::List`] with `pages`), a sideways
    /// scroll (or Shift with the wheel): `dx` only, `x` and `y` 0; the list scrolls up
    /// and down itself, and sideways is the module's (a diff's long lines).
    Wheel { id: String, dx: f32, dy: f32, x: f32, y: f32 },
    /// Ctrl+wheel or a pinch over a canvas with [`CanvasSense::wheel`] (and no `view`):
    /// zoom by `factor` (above 1: in) around `(x, y)`.
    Zoom { id: String, factor: f32, x: f32, y: f32 },
    /// What a canvas with a `view` shows now, in world coordinates: the area from
    /// `(x, y)`, `width` by `height`, at `zoom` (points on screen per world unit). Sent
    /// when it first shows and whenever the user pans, zooms or resizes it, so a module
    /// can build only the shapes in that area (with a margin: it arrives a frame later).
    View { id: String, x: f32, y: f32, width: f32, height: f32, zoom: f32 },
    /// The rows a [`Node::List`] with a `window` shows now: `first..first + count`.
    /// Sent when it first shows and whenever that changes; build those rows (and a few
    /// around them) for the next call.
    VisibleRange { id: String, first: u64, count: u64 },
    /// A picture of a canvas ([`Shape::Image`]) by its [`ImageSrc::key`] was decoded,
    /// failed, or was freed (after it went unused): send its bytes again to show it.
    Image { key: String, status: ImageStatus },
    /// What the user asked for, by whichever input they use: a key, a gamepad button, or
    /// a click on the hints the host draws under the window (see [`Action`]). The module
    /// never sees the physical key; it says which actions it offers in
    /// [`Output::actions`], and the host shows those, in the glyphs of the device the user
    /// last used. Arrives while the module has focus (or from a click on its hints).
    Action(Action),
    /// A right click on a pressable container, at `(x, y)` from the module's top-left
    /// (on a list row, `id` is `<list id>/<index>`).
    Secondary { id: String, x: f32, y: f32 },
    /// The user changed a [`Node::TextField`]: its whole text, and the `revision` of the
    /// value it was based on (see [`ui::FieldState`]).
    Edited { id: String, value: String, revision: u64 },
    /// Enter in a single-line [`Node::TextField`] (it also arrives as [`Action::Open`]).
    Submitted { id: String },
    /// A [`Node::TextField`] got (`true`) or lost the keyboard.
    Focus { id: String, focused: bool },
    /// A [`Node::Popup`] was closed by the user (Escape, or a click outside one that
    /// isn't modal).
    Dismissed { id: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerKind {
    Down,
    Drag,
    Up,
    /// Moved over the canvas, no button held ([`CanvasSense::hover`]).
    Move,
    Enter,
    Leave,
}

/// What became of a picture ([`Event::Image`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ImageStatus {
    /// Decoded: its size in pixels.
    Loaded { width: u32, height: u32 },
    /// Not shown, in plain words (`not a PNG or JPEG`, `too big: …`, `not allowed: …`).
    Failed(String),
    /// Freed after it went unused for a while (or for room): send its bytes again.
    Dropped,
}

/// The semantic actions a module gets ([`Event::Action`]) instead of keys. The host maps
/// every input onto them:
///
/// | action | keyboard | gamepad | |
/// |---|---|---|---|
/// | `Back` | Escape, Backspace | B | leave, close, cancel |
/// | `Open` | Enter | A | open, see more, details |
/// | `Primary` | Space (in a text field: Ctrl+Enter) | X | the first thing to do |
/// | `Secondary` | Tab (in a text field: Alt+Enter) | Y | the second |
/// | `Move` | arrows, PgUp/PgDn, Home/End, j/k | d-pad | selection, scrolling |
/// | `Key(name)` | the key the module names | | its own keys ([`Output::key`]) |
///
/// A [`Node::List`] with the keyboard takes `Move` (as [`Event::Selected`]) and `Open`
/// (as [`Event::Activated`]) itself; the rest arrive here. An open popup takes Escape
/// (as [`Event::Dismissed`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Back,
    Open,
    Primary,
    Secondary,
    Move(Move),
    /// One of the module's own named keys ([`Offer::key`]), while its pane has the
    /// keyboard.
    Key(String),
}

/// Where [`Action::Move`] goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    /// To the start (Home) / the end (End).
    Home,
    End,
}

/// Host data, filled only for what the module asked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostData {
    /// Finished [`Effect::RunTool`] commands since the last call, each delivered once.
    #[serde(default)]
    pub tool_runs: Vec<ToolResult>,
    /// Answers to [`Effect::Fs`] requests since the last call, each delivered once.
    #[serde(default)]
    pub fs: Vec<FsResult>,
    /// What changed in watched files and folders ([`FsOp::Watch`]), since the last call.
    #[serde(default)]
    pub fs_events: Vec<FsWatchEvent>,
}
