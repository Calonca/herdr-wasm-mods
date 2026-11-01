//! Builders for [`Node`] trees, used by modules (the SDK re-exports them) and by the host
//! for its Allow prompt, so both build the same components the same way. Pure: no host
//! calls.
//!
//! ```ignore
//! column([
//!     heading("recipe box"),
//!     muted("3 recipes"),
//!     row([primary("add", "add recipe"), button("import", "import…")]),
//!     self.search.view("search").placeholder("search"),
//! ])
//! ```

use crate::*;

mod fuzzy;
mod glyph;
mod modifiers;
mod state;

pub use fuzzy::*;
pub use glyph::*;
pub use state::*;

// ---------------------------------------------------------------------------
// Containers
// ---------------------------------------------------------------------------

/// Children top to bottom, 6 points apart.
pub fn column(children: impl IntoIterator<Item = Node>) -> Node {
    Node::Column {
        children: children.into_iter().collect(),
        gap: 6.0,
        layout: Layout::default(),
        style: Style::default(),
        id: None,
        tooltip: None,
    }
}

/// Children left to right, 8 points apart, wrapping when they don't fit.
pub fn row(children: impl IntoIterator<Item = Node>) -> Node {
    Node::Row {
        children: children.into_iter().collect(),
        gap: 8.0,
        layout: Layout::default(),
        style: Style::default(),
        id: None,
        tooltip: None,
    }
}

/// A rounded, bordered box with a small header line (`label` left, `meta` right).
pub fn tile(label: impl Into<String>, meta: impl Into<String>, child: Node) -> Node {
    let label: String = label.into();
    let header = row([small(label.to_lowercase()), small(meta)])
        .justify(Justify::SpaceBetween)
        .wrap(false)
        .width(Size::Fill);
    column([header, child]).variant(Variant::Card).width(Size::Fill).gap(8.0)
}

/// A thin horizontal line across the width.
pub fn separator() -> Node {
    row([]).width(Size::Fill).height(Size::Fixed(9.0)).dividers()
}

/// Empty space: `points` tall in a column, wide in a row.
pub fn space(points: f32) -> Node {
    let mut n = row([]);
    if let Some(l) = n.layout_mut() {
        l.width = Size::Fixed(points);
        l.height = Size::Fixed(points);
    }
    n
}

/// Empty space taking whatever room is left (pushes what follows to the end).
pub fn spacer() -> Node {
    row([]).width(Size::Fill)
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

pub fn styled(text: impl Into<String>, style: TextStyle) -> Node {
    Node::Text {
        text: text.into(),
        style,
        spans: Vec::new(),
        color: None,
        size: None,
        bold: false,
        font: Font::Auto,
        wrap: Wrap::Wrap,
    }
}

pub fn text(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Body)
}

pub fn heading(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Heading)
}

pub fn title(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Title)
}

pub fn muted(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Muted)
}

pub fn accent(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Accent)
}

pub fn mono(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Mono)
}

pub fn small(text: impl Into<String>) -> Node {
    styled(text, TextStyle::Small)
}

/// One line of coloured runs (`None`: the text's own colour).
pub fn spans(runs: Vec<Span>) -> Node {
    let mut n = text("");
    if let Node::Text { spans, .. } = &mut n {
        *spans = runs;
    }
    n
}

/// `(text, colour)` runs as [`Span`]s.
pub fn runs<'a>(runs: impl IntoIterator<Item = (&'a str, Option<Color>)>) -> Vec<Span> {
    runs.into_iter().map(|(text, color)| Span { text: text.to_owned(), color, link: None, bg: None }).collect()
}

/// A link run for [`spans`]: `text` shown, `target` opened by the host on a click (an
/// `http(s)` or `file://` URL, or an absolute or `~/` path).
pub fn link(text: impl Into<String>, target: impl Into<String>) -> Span {
    Span { text: text.into(), color: None, link: Some(target.into()), bg: None }
}

/// A small rounded badge.
pub fn pill(text: impl Into<String>) -> Node {
    row([small(text)]).variant(Variant::Pill).wrap(false)
}

/// Keys and what they do, one pair per line, the keys in a column `key_width` wide.
pub fn keys_table<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>, key_width: f32, size: f32) -> Node {
    let lines = pairs.into_iter().map(|(key, what)| {
        let key = accent(key).size(size).wrap_mode(Wrap::Extend);
        row([row([key]).width(Size::Fixed(key_width)), text(what).size(size).wrap_mode(Wrap::Extend)]).wrap(false).gap(0.0)
    });
    column(lines)
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// A pressable row holding `label`, styled as a `kind` button.
pub fn button_kind(id: impl Into<String>, label: impl Into<String>, kind: ButtonKind) -> Node {
    row([text(label).wrap_mode(Wrap::Extend)]).id(id).variant(Variant::Button(kind)).wrap(false)
}

/// Emits [`Event::Clicked`] with `id`.
pub fn button(id: impl Into<String>, label: impl Into<String>) -> Node {
    button_kind(id, label, ButtonKind::Secondary)
}

/// The main action: accent fill.
pub fn primary(id: impl Into<String>, label: impl Into<String>) -> Node {
    button_kind(id, label, ButtonKind::Primary)
}

/// No outline until hovered.
pub fn ghost(id: impl Into<String>, label: impl Into<String>) -> Node {
    button_kind(id, label, ButtonKind::Ghost)
}

/// A destructive action.
pub fn danger(id: impl Into<String>, label: impl Into<String>) -> Node {
    button_kind(id, label, ButtonKind::Danger)
}

/// A button that stays highlighted while `on`.
pub fn toggle(id: impl Into<String>, label: impl Into<String>, on: bool) -> Node {
    let n = button(id, label);
    if on { n.variant(Variant::Selected) } else { n }
}

/// `☑ label` / `☐ label`, pressable.
pub fn checkbox(id: impl Into<String>, label: impl Into<String>, on: bool) -> Node {
    let mark = if on { "☑" } else { "☐" };
    row([accent(mark).wrap_mode(Wrap::Extend), text(label).wrap_mode(Wrap::Extend)])
        .id(id)
        .variant(Variant::Button(ButtonKind::Ghost))
        .gap(6.0)
        .wrap(false)
}

// ---------------------------------------------------------------------------
// Lists, canvases, popups
// ---------------------------------------------------------------------------

pub fn list(id: impl Into<String>, items: Vec<ListItem>, selected: Option<usize>) -> Node {
    Node::List {
        id: id.into(),
        items,
        selected,
        layout: Layout::default(),
        dense: false,
        style: ListStyle::Inverse,
        font: Font::Auto,
        window: None,
        keyboard: true,
    }
}

/// A list item with a label (and nothing else yet).
pub fn item(label: impl Into<String>) -> ListItem {
    ListItem { label: label.into(), ..Default::default() }
}

/// A list row drawn as `child` (columns, badges, a progress bar): see [`ListItem::child`].
/// `label` is its plain text (for tooltips' sake).
pub fn row_item(label: impl Into<String>, child: Node) -> ListItem {
    ListItem { label: label.into(), child: Some(Box::new(child)), ..Default::default() }
}

/// Without the host's keys (see [`Node::List::keyboard`]): for lists whose module moves the
/// selection itself on [`Action::Move`] ([`menu`], [`Fuzzy`]).
fn own_keys(mut n: Node) -> Node {
    if let Node::List { keyboard, .. } = &mut n {
        *keyboard = false;
    }
    n
}

pub fn canvas(id: impl Into<String>, width: f32, height: f32, shapes: Vec<Shape>) -> Node {
    Node::Canvas {
        id: id.into(),
        width,
        height,
        shapes,
        layout: Layout::default(),
        coords: Coords::Points,
        view: None,
        sense: CanvasSense::default(),
    }
}

/// An [`Shape::Image`] filling the box `(x, y, w, h)` by `fit`.
pub fn image_shape(x: f32, y: f32, w: f32, h: f32, src: ImageSrc, fit: Fit) -> Shape {
    Shape::Image { x, y, w, h, src, fit, rounding: 0.0, opacity: 1.0 }
}

/// A picture, `width` by `height` points (`.width(Size::Fill)` for the full width),
/// fitted in whole: a canvas holding one image. `id` names the canvas (its clicks).
pub fn image(id: impl Into<String>, src: ImageSrc, width: f32, height: f32) -> Node {
    canvas(id, width, height, vec![image_shape(0.0, 0.0, width, height, src, Fit::Contain)])
}

/// A picture the user pans and zooms (drag, wheel, pinch; double-click fits it again),
/// in a canvas `width` by `height` points. `size`: the picture's pixels, once known
/// ([`ImageStatus::Loaded`]); until then it's fitted to the canvas.
pub fn zoomable_image(id: impl Into<String>, src: ImageSrc, size: Option<(u32, u32)>, width: f32, height: f32) -> Node {
    let (w, h) = size.map_or((width, height), |(w, h)| (w as f32, h as f32));
    let view = PanZoom { content: Some((w, h)), drag_pans: true, ..PanZoom::default() };
    canvas(id, width, height, vec![image_shape(0.0, 0.0, w, h, src, Fit::Fill)]).pan_zoom_with(view)
}

/// A thin progress bar across the width: `fraction` (0..1) of it in `color` over `track`.
pub fn progress(fraction: f32, color: Color, track: Color) -> Node {
    let f = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let shapes = vec![
        Shape::Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0, radius: 2.0, fill: track, stroke: None },
        Shape::Rect { x: 0.0, y: 0.0, w: f, h: 1.0, radius: 2.0, fill: color, stroke: None },
    ];
    let mut n = canvas("", 0.0, 4.0, shapes).width(Size::Fill);
    if let Node::Canvas { coords, .. } = &mut n {
        *coords = Coords::Unit;
    }
    n
}

/// A floating panel next to `anchor`.
pub fn popup(id: impl Into<String>, anchor: Anchor, child: Node) -> Node {
    Node::Popup { id: id.into(), anchor, modal: false, child: Box::new(child), style: Style::default() }
}

/// A context menu at the pointer: a [`ListStyle::Soft`] list in a popup.
/// [`Event::Selected`] picks, [`Event::Dismissed`] closes; for the keys, move your
/// `selected` on [`Action::Move`] and pick it on [`Action::Open`].
pub fn menu(id: impl Into<String>, items: Vec<ListItem>, selected: Option<usize>) -> Node {
    let id = id.into();
    let list = own_keys(list(format!("{id}/items"), items, selected)).list_style(ListStyle::Soft).width(Size::Hug);
    popup(id, Anchor::Pointer, list)
}

/// A modal dialog in the middle of the window, `width` points wide.
pub fn dialog(id: impl Into<String>, width: f32, children: impl IntoIterator<Item = Node>) -> Node {
    let body = column(children).gap(8.0).width(Size::Fixed(width)).padding(Pad::L);
    let mut n = popup(id, Anchor::Center, body);
    if let Node::Popup { modal, .. } = &mut n {
        *modal = true;
    }
    n
}
