//! The component tree a module returns: the [`Node`] primitives and their style, layout,
//! list, canvas and image props.

use super::*;

/// Prebuilt components. The host lays them out and draws them in the active theme. Build
/// them with the [`ui`] helpers rather than by hand: every field past the first ones
/// defaults, so `{"Text":{"text":"hi"}}` is a whole node.
///
/// Eight primitives.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// Nothing. (A spacer is an empty `Row`/`Column` with a size: [`ui::space`].)
    #[default]
    Empty,
    /// Children top to bottom.
    Column {
        children: Vec<Node>,
        #[serde(default)]
        gap: f32,
        #[serde(default)]
        layout: Layout,
        #[serde(default)]
        style: Style,
        /// Makes it pressable: [`Event::Clicked`], [`Event::Secondary`], hover and pressed
        /// colours from `style.variant`, the pointer cursor.
        #[serde(default)]
        id: Option<String>,
        /// Shown while the pointer rests on it.
        #[serde(default)]
        tooltip: Option<Box<Node>>,
    },
    /// Children left to right, wrapping onto more lines when they don't fit (unless
    /// `layout.wrap` is `Some(false)` or `layout.justify` isn't `Start`).
    Row {
        children: Vec<Node>,
        #[serde(default)]
        gap: f32,
        #[serde(default)]
        layout: Layout,
        #[serde(default)]
        style: Style,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        tooltip: Option<Box<Node>>,
    },
    /// Text. With `spans` set, those coloured runs are shown instead of `text`.
    Text {
        text: String,
        #[serde(default)]
        style: TextStyle,
        #[serde(default)]
        spans: Vec<Span>,
        /// Overrides the style's colour.
        #[serde(default)]
        color: Option<Paint>,
        /// Points; overrides the style's size.
        #[serde(default)]
        size: Option<f32>,
        #[serde(default)]
        bold: bool,
        #[serde(default)]
        font: Font,
        #[serde(default)]
        wrap: Wrap,
    },
    /// Text the user types. The host keeps the text while the user edits it and reports
    /// each change ([`Event::Edited`]); `value` replaces it only when `revision` goes up
    /// (that is how a module clears or replaces the text). [`ui::FieldState`] does this.
    TextField {
        id: String,
        #[serde(default)]
        value: String,
        #[serde(default)]
        revision: u64,
        #[serde(default)]
        multiline: bool,
        #[serde(default)]
        password: bool,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        read_only: bool,
        /// Lines shown by a multiline field.
        #[serde(default)]
        rows: Rows,
        #[serde(default = "yes")]
        monospace: bool,
        #[serde(default)]
        layout: Layout,
    },
    /// Rows with a highlighted selection; emits [`Event::Selected`] and
    /// [`Event::Activated`]. The host scrolls it (wheel, scrollbar) and, with `keyboard`,
    /// moves its selection with the keys. A row is the item's fixed fields or, with
    /// [`ListItem::child`], any node (columns, badges, progress bars).
    List {
        id: String,
        items: Vec<ListItem>,
        selected: Option<usize>,
        #[serde(default)]
        layout: Layout,
        /// Tighter rows.
        #[serde(default)]
        dense: bool,
        #[serde(default)]
        style: ListStyle,
        #[serde(default)]
        font: Font,
        /// Virtual rows: the list is `total` rows long and `items` are rows `first..`;
        /// the host draws the scrollbar for all of them, keeps the scroll position, and says
        /// which rows show ([`Event::VisibleRange`]), so the module builds only those.
        /// `selected` and [`Event::Selected`] count from row 0. All rows are one height:
        /// the tallest row seen so far; a [`ListItem::child`] taller than that is cut.
        /// [`ui::VirtualRows`] does the module's side.
        #[serde(default)]
        window: Option<ListWindow>,
        /// The host does the keys: while the list has the keyboard, [`Action::Move`] (↑/↓,
        /// j/k, PgUp/PgDn, Home/End, the d-pad) moves the selection ([`Event::Selected`],
        /// scrolled into view) and [`Action::Open`] (Enter, A) sends
        /// [`Event::Activated`]; the module doesn't get those. The list with the keyboard
        /// is the one last clicked (a list in an open popup first), else the module's
        /// first one with `keyboard`, while its window is focused. [`ui::list`] sets it.
        #[serde(default)]
        keyboard: bool,
        /// The module's page: PgUp/PgDn scroll this (virtual) list by a page and
        /// Home/End to its ends, whichever list has the keyboard, so the module hears
        /// only the new [`Event::VisibleRange`] (a diff beside its file list).
        #[serde(default)]
        pages: bool,
    },
    /// A free drawing area; emits [`Event::Pointer`]. `width`/`height` are its size in
    /// points unless `layout` says otherwise (e.g. `Size::Fill` for a full-width bar).
    Canvas {
        id: String,
        width: f32,
        height: f32,
        shapes: Vec<Shape>,
        #[serde(default)]
        layout: Layout,
        /// `Unit`: positions and sizes are fractions (0..1) of the canvas.
        #[serde(default)]
        coords: Coords,
        /// Pan and zoom, done by the host: the module draws in world coordinates and the host
        /// shows them moved and scaled (wheel or two-finger drag pans, Ctrl+wheel or a
        /// pinch zooms at the pointer, middle-drag or Space-drag pans, double-click
        /// resets), with no call to the module needed to redraw. Each canvas keeps its own
        /// view, by `id`, across calls and live reloads. Over it, the wheel moves the view
        /// rather than scrolling the window. [`Event::View`] says what shows.
        #[serde(default)]
        view: Option<PanZoom>,
        /// Extra input it wants: pointer moves, the raw wheel.
        #[serde(default)]
        sense: CanvasSense,
    },
    /// A floating panel over the module: a menu, a dialog. A module's popups stay inside its
    /// window; `modal` ones dim the window and take its input until closed. It shows while
    /// it is in the tree; [`Event::Dismissed`] says the user closed it (Escape, or a click
    /// outside one that isn't modal).
    Popup {
        id: String,
        #[serde(default)]
        anchor: Anchor,
        #[serde(default)]
        modal: bool,
        child: Box<Node>,
        #[serde(default)]
        style: Style,
    },
}

fn yes() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextStyle {
    #[default]
    Body,
    /// Bold sans, a little larger.
    Heading,
    Muted,
    Accent,
    /// Monospace, for code and ids.
    Mono,
    /// Larger than a heading: a dialog's or page's title.
    Title,
    /// Smaller, muted: notes and captions.
    Small,
}

/// Which typeface a text or list uses. `Auto`: the UI font (theme.toml's `ui_font`, the
/// desktop's sans-serif by default), except [`TextStyle::Mono`], which is monospace;
/// `Mono`: the mono font (`mono_font`, JetBrains Mono by default); `Sans`: the UI font.
/// (Before the host had a UI font, `Auto` was monospace for everything but headings.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Font {
    #[default]
    Auto,
    Mono,
    Sans,
}

/// What text does when it doesn't fit its width. Wrapped text is never narrower than
/// its longest word, and what is still too wide scrolls (the nearest container with
/// `scroll`, else the window); only [`Wrap::Truncate`] cuts it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Wrap {
    /// More lines, broken between words.
    #[default]
    Wrap,
    /// One line, cut with `…` where the room ends: a row's part that gives way (a
    /// commit's subject). It takes the room its row leaves; in a container with a
    /// width, that width.
    Truncate,
    /// One line, as wide as it is: the nearest scrolling container (else the window)
    /// scrolls sideways when it's wider.
    Extend,
}

/// How many lines a multiline [`Node::TextField`] shows.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Rows {
    Fixed(u32),
    /// The height the layout gives it.
    Fill,
}

impl Default for Rows {
    fn default() -> Self {
        Rows::Fixed(4)
    }
}

/// A colour: one of the theme's roles (follows theme changes) or a fixed `0xRRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paint {
    Role(Role),
    Rgba(Color),
}

/// The theme's colours by what they're for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Text,
    Subtext,
    Muted,
    Heading,
    Accent,
    /// Red: destructive actions, errors.
    Danger,
    /// Green.
    Success,
    /// Yellow.
    Warning,
    /// Blue.
    Info,
    /// The deep background; text on an accent fill.
    Space,
    /// A window's fill.
    Surface,
    Border,
    /// Faint lines and hover fills.
    Divider,
}

/// How a container looks. Everything defaults to plain: no fill, no border.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub variant: Variant,
    /// Text colour inside (for text that doesn't set its own).
    pub fg: Option<Paint>,
    pub bg: Option<Paint>,
    pub border: Option<Paint>,
    pub rounding: Option<Rounding>,
    /// 0..1, for the container and everything in it.
    pub opacity: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Variant {
    #[default]
    Plain,
    /// A bordered box on a darker fill (a tile).
    Card,
    /// The look of a text field's box.
    Field,
    /// A small rounded badge.
    Pill,
    /// A button; give the container an `id` so it can be pressed.
    Button(ButtonKind),
    /// Highlighted (a toggle that is on, the chosen one of several).
    Selected,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ButtonKind {
    /// Accent fill.
    Primary,
    /// Outlined.
    #[default]
    Secondary,
    /// No outline until hovered: toolbar and bar items.
    Ghost,
    /// For destructive actions.
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rounding {
    None,
    S,
    M,
    /// The theme's window radius.
    L,
    /// Round ends.
    Full,
}

/// Where a container sits and how its children are laid out. Everything defaults to
/// "as the node does by default" (hug its content; a list fills the width).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    pub width: Size,
    pub height: Size,
    /// Space inside the border.
    pub padding: Pad,
    /// Across the main axis (a row's children vertically).
    pub align: Align,
    /// Along the main axis.
    pub justify: Justify,
    /// A row wraps its children onto more lines (default yes); `Some(false)` keeps
    /// them on one line, and the nearest scrolling container (else the window) scrolls
    /// sideways when it's wider.
    pub wrap: Option<bool>,
    /// Scrolls its children inside it (bars only when they don't fit). The window
    /// already scrolls both ways: this is for a part that scrolls on its own.
    pub scroll: Scroll,
    /// A thin line between children (replaces `Separator`).
    pub dividers: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Size {
    /// The node's default (containers hug their content, lists fill).
    #[default]
    Auto,
    Hug,
    Fill,
    /// Points. Bigger than the room it has, it scrolls (never squeezed or cut).
    Fixed(f32),
}

/// Padding tokens, in the theme's density.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pad {
    /// The variant's own (a card or button has some, plain none).
    #[default]
    Auto,
    None,
    Xs,
    S,
    M,
    L,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    /// A column's children at the start, a row's centred.
    #[default]
    Auto,
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    /// The first child at the start, the last at the end, the rest spread between.
    SpaceBetween,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scroll {
    #[default]
    None,
    Vertical,
    Horizontal,
    Both,
}

/// How a list highlights.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListStyle {
    /// The selected row inverted in the accent colour.
    #[default]
    Inverse,
    /// A soft accent tint: menus, pickers in dialogs.
    Soft,
}

/// Where a [`Node::Popup`] shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Anchor {
    /// The middle of the module's window.
    #[default]
    Center,
    /// Where the pointer was when it first showed (a context menu).
    Pointer,
    /// Next to the node with this `id` (a pressable container, text field or list).
    Node { id: String, side: Side },
    /// Its top-left corner at `(x, y)` from the module's top-left.
    At { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    /// Above it, their left edges lined up.
    Above,
    #[default]
    Below,
    Right,
    Left,
    /// Above it, their right edges lined up (for something at the right end).
    AboveEnd,
    BelowEnd,
}

/// A [`Node::List`]'s virtual rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ListWindow {
    /// How many rows the list has.
    pub total: u64,
    /// The row `items[0]` is.
    pub first: u64,
    /// Scrolls so this row is at the top (as far as it goes: `u64::MAX` is the end),
    /// once per new `revision`.
    pub scroll_to: Option<ScrollTo>,
}

/// A one-off scroll ([`ListWindow::scroll_to`]): done when `revision` goes up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrollTo {
    pub row: u64,
    pub revision: u64,
}

/// A canvas's pan and zoom ([`Node::Canvas::view`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanZoom {
    /// How far out and in it zooms (points on screen per world unit).
    pub min_zoom: f32,
    pub max_zoom: f32,
    /// The world's size, when known (a picture's): the view starts and resets fitted
    /// to it (and again when it or the canvas changes size, until the user moves the view), and can't
    /// be panned entirely off it. `None`: zoom 1 at (0, 0).
    pub content: Option<(f32, f32)>,
    /// The view resets (fits the content again) when this goes up: bind a key to it.
    pub reset: u64,
    /// The main button's drag pans too (a picture viewer). Off, drags go to the module
    /// as [`Event::Pointer`] (drawing), and middle-drag or Space-drag pans.
    pub drag_pans: bool,
}

impl Default for PanZoom {
    fn default() -> Self {
        PanZoom { min_zoom: 0.05, max_zoom: 64.0, content: None, reset: 0, drag_pans: false }
    }
}

/// Input a canvas asks for beyond presses and drags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasSense {
    /// [`PointerKind::Move`], `Enter` and `Leave`.
    pub hover: bool,
    /// [`Event::Wheel`] and [`Event::Zoom`] (without a `view`): over the canvas the wheel
    /// is its own, not the window's scroll.
    pub wheel: bool,
}

/// What [`Node::Canvas`] coordinates are in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Coords {
    /// Points from the top-left.
    #[default]
    Points,
    /// Fractions of the canvas (0..1): x and widths of its width, y and heights of its
    /// height. Radii, stroke widths and text sizes stay in points.
    Unit,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ListItem {
    pub label: String,
    /// Right-aligned, muted (or `detail_color`).
    pub detail: String,
    /// A dot before the label, e.g. an agent's status colour.
    pub dot: Option<Color>,
    /// Draw the dot as a ring (herdr's `○` for a pane with an agent).
    #[serde(default)]
    pub ring: bool,
    /// A short mark before the label, after the dot (`>_`, `★`). Muted, or `icon_color`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<Color>,
    /// Tree branch drawn muted before the dot (`├─ `, `└─ `), for nested rows.
    #[serde(default)]
    pub tree: String,
    /// A second, smaller line under the label (e.g. `main ↑5`), aligned with it.
    #[serde(default)]
    pub sub: Vec<Span>,
    /// A shortcut drawn at the right edge, in the accent colour (the host's picker: the
    /// key bound to the entry).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// Coloured runs shown instead of `label`.
    #[serde(default)]
    pub label_spans: Vec<Span>,
    #[serde(default)]
    pub bold: bool,
    /// `Danger`: the label in red (Close, Delete).
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub detail_color: Option<Color>,
    /// A small control at the right end (`×`, `▸`): clicking it sends
    /// [`Event::Clicked`] with its id instead of selecting the row.
    #[serde(default)]
    pub trailing: Option<Trailing>,
    /// Shown while the pointer rests on the row.
    #[serde(default)]
    pub tooltip: Option<String>,
    /// Greyed out and can't be chosen.
    #[serde(default)]
    pub disabled: bool,
    /// A section label rather than a row: muted, small, never selected.
    #[serde(default)]
    pub header: bool,
    /// The row's content as any node (a row of columns, badges, a progress bar), drawn
    /// in place of the fixed fields above, inside the row's highlight and click area.
    /// Its own pressable nodes (buttons) still take their clicks. [`ui::row_item`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child: Option<Box<Node>>,
}

/// A list item's control at its right end.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trailing {
    pub id: String,
    pub text: String,
}

/// A run of text with its own colour (`None`: muted in lists and tooltips, the text's
/// own colour in a [`Node::Text`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    pub color: Option<Color>,
    /// In a [`Node::Text`]: a link (an `http(s)` URL, a `file://` URL or an absolute or
    /// `~/` path). The host draws it as one (pointer, underline on hover, the target in a
    /// tooltip) and opens it itself on a click, as it opens links in terminals: the
    /// module isn't told. ([`ui::link`])
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// In a [`Node::Text`]: a fill behind the run, slightly rounded (a diff's changed
    /// words).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<Color>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Rect { x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Color, stroke: Option<(f32, Color)> },
    Circle { x: f32, y: f32, r: f32, fill: Color },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Color },
    /// Monospace text; `(x, y)` is its top-left corner. A glyph is about `0.6 × size`
    /// wide, so coloured runs can be laid side by side.
    Text { x: f32, y: f32, text: String, size: f32, color: Color },
    /// A polyline through `points` (closed: a polygon, filled with `fill`).
    Path {
        points: Vec<(f32, f32)>,
        #[serde(default)]
        closed: bool,
        #[serde(default)]
        fill: Option<Color>,
        #[serde(default)]
        stroke: Option<(f32, Color)>,
    },
    /// A picture in the box `(x, y, w, h)`, placed by `fit`. The host decodes it (PNG or
    /// JPEG) off the UI thread and keeps it as a texture while it is in use; at most
    /// 8192 pixels a side and 64 MB decoded per frame. Magnified a lot (a zoomed
    /// [`PanZoom`] view), its pixels show sharp. [`Event::Image`] says when it loaded
    /// (its size) or why not. A standalone picture is a canvas holding one image
    /// ([`ui::image`]).
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        src: ImageSrc,
        #[serde(default)]
        fit: Fit,
        #[serde(default)]
        rounding: f32,
        #[serde(default = "one")]
        opacity: f32,
    },
}

fn one() -> f32 {
    1.0
}

/// Where an [`Shape::Image`]'s pixels come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ImageSrc {
    /// A picture already sent as [`ImageSrc::Bytes`] under this key (or, in the host's own
    /// UI, one the host holds: `music-cover`).
    Cached(String),
    /// The file's bytes (PNG, JPEG) in [`Output::bytes`], kept under `key`: send them
    /// once, then show it as `Cached(key)` (or keep `data` empty) until
    /// [`ImageStatus::Dropped`].
    Bytes { key: String, data: Blob },
    /// A file the module may read (picked, or under `fs.read`), read and decoded by the host:
    /// its bytes never go through the module.
    File(String),
    /// A file open as [`FsOp::Open`]'s handle.
    Handle(u64),
}

impl ImageSrc {
    /// The key [`Event::Image`] names it by: the key, the path, or `handle:<n>`.
    pub fn key(&self) -> String {
        match self {
            ImageSrc::Cached(k) | ImageSrc::Bytes { key: k, .. } | ImageSrc::File(k) => k.clone(),
            ImageSrc::Handle(h) => format!("handle:{h}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    /// All of it, letterboxed.
    #[default]
    Contain,
    /// Filling the box, cropped.
    Cover,
    /// Stretched.
    Fill,
    /// At its own size, centred.
    None,
}
