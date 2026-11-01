//! Modifiers: `Node` methods that set style, layout and behaviour on a built node.

use super::*;

impl Node {
    /// The layout of a node that has one.
    pub fn layout(&self) -> Option<&Layout> {
        match self {
            Node::Column { layout, .. }
            | Node::Row { layout, .. }
            | Node::TextField { layout, .. }
            | Node::List { layout, .. }
            | Node::Canvas { layout, .. } => Some(layout),
            _ => None,
        }
    }

    /// The layout of a node that has one, to change.
    pub fn layout_mut(&mut self) -> Option<&mut Layout> {
        match self {
            Node::Column { layout, .. }
            | Node::Row { layout, .. }
            | Node::TextField { layout, .. }
            | Node::List { layout, .. }
            | Node::Canvas { layout, .. } => Some(layout),
            _ => None,
        }
    }

    fn style_mut(&mut self) -> Option<&mut Style> {
        match self {
            Node::Column { style, .. } | Node::Row { style, .. } | Node::Popup { style, .. } => Some(style),
            _ => None,
        }
    }

    fn with_layout(mut self, f: impl FnOnce(&mut Layout)) -> Self {
        if let Some(l) = self.layout_mut() {
            f(l);
        }
        self
    }

    fn with_style(mut self, f: impl FnOnce(&mut Style)) -> Self {
        if let Some(s) = self.style_mut() {
            f(s);
        }
        self
    }

    /// Makes a container pressable ([`Event::Clicked`], [`Event::Secondary`]).
    pub fn id(mut self, new: impl Into<String>) -> Self {
        if let Node::Column { id, .. } | Node::Row { id, .. } = &mut self {
            *id = Some(new.into());
        }
        self
    }

    /// Shown while the pointer rests on a container.
    pub fn tooltip(mut self, tip: Node) -> Self {
        if let Node::Column { tooltip, .. } | Node::Row { tooltip, .. } = &mut self {
            *tooltip = Some(Box::new(tip));
        }
        self
    }

    pub fn style(self, new: Style) -> Self {
        self.with_style(|s| *s = new)
    }

    pub fn variant(self, variant: Variant) -> Self {
        self.with_style(|s| s.variant = variant)
    }

    /// Text colour inside a container.
    pub fn fg(self, paint: Paint) -> Self {
        self.with_style(|s| s.fg = Some(paint))
    }

    pub fn bg(self, paint: Paint) -> Self {
        self.with_style(|s| s.bg = Some(paint))
    }

    pub fn border(self, paint: Paint) -> Self {
        self.with_style(|s| s.border = Some(paint))
    }

    pub fn rounding(self, r: Rounding) -> Self {
        self.with_style(|s| s.rounding = Some(r))
    }

    pub fn opacity(self, o: f32) -> Self {
        self.with_style(|s| s.opacity = Some(o))
    }

    pub fn width(self, size: Size) -> Self {
        self.with_layout(|l| l.width = size)
    }

    pub fn height(self, size: Size) -> Self {
        self.with_layout(|l| l.height = size)
    }

    pub fn padding(self, pad: Pad) -> Self {
        self.with_layout(|l| l.padding = pad)
    }

    pub fn align(self, align: Align) -> Self {
        self.with_layout(|l| l.align = align)
    }

    pub fn justify(self, justify: Justify) -> Self {
        self.with_layout(|l| l.justify = justify)
    }

    pub fn wrap(self, wrap: bool) -> Self {
        self.with_layout(|l| l.wrap = Some(wrap))
    }

    pub fn scroll(self, scroll: Scroll) -> Self {
        self.with_layout(|l| l.scroll = scroll)
    }

    pub fn dividers(self) -> Self {
        self.with_layout(|l| l.dividers = true)
    }

    /// Space between a container's children.
    pub fn gap(mut self, points: f32) -> Self {
        if let Node::Column { gap, .. } | Node::Row { gap, .. } = &mut self {
            *gap = points;
        }
        self
    }

    /// A text's colour.
    pub fn color(mut self, paint: Paint) -> Self {
        if let Node::Text { color, .. } = &mut self {
            *color = Some(paint);
        }
        self
    }

    /// A text's colour by theme role.
    pub fn role(self, role: Role) -> Self {
        self.color(Paint::Role(role))
    }

    /// A text's size in points.
    pub fn size(mut self, points: f32) -> Self {
        if let Node::Text { size, .. } = &mut self {
            *size = Some(points);
        }
        self
    }

    pub fn bold(mut self) -> Self {
        if let Node::Text { bold, .. } = &mut self {
            *bold = true;
        }
        self
    }

    /// A text's or list's typeface.
    pub fn font(mut self, new: Font) -> Self {
        if let Node::Text { font, .. } | Node::List { font, .. } = &mut self {
            *font = new;
        }
        self
    }

    pub fn wrap_mode(mut self, new: Wrap) -> Self {
        if let Node::Text { wrap, .. } = &mut self {
            *wrap = new;
        }
        self
    }

    /// One line, cut with `…` where the room ends ([`Wrap::Truncate`]).
    pub fn truncate(self) -> Self {
        self.wrap_mode(Wrap::Truncate)
    }

    pub fn list_style(mut self, new: ListStyle) -> Self {
        if let Node::List { style, .. } = &mut self {
            *style = new;
        }
        self
    }

    pub fn dense(mut self) -> Self {
        if let Node::List { dense, .. } = &mut self {
            *dense = true;
        }
        self
    }

    /// A list's selected row.
    pub fn selected(mut self, new: Option<usize>) -> Self {
        if let Node::List { selected, .. } = &mut self {
            *selected = new;
        }
        self
    }

    /// A list of `total` virtual rows whose `items` start at row `first` (see
    /// [`VirtualRows`]).
    pub fn window(mut self, total: u64, first: u64) -> Self {
        if let Node::List { window, .. } = &mut self {
            *window = Some(ListWindow { total, first, scroll_to: window.and_then(|w| w.scroll_to) });
        }
        self
    }

    /// A canvas's coordinates: `Coords::Unit` for fractions (0..1) of its size.
    pub fn coords(mut self, new: Coords) -> Self {
        if let Node::Canvas { coords, .. } = &mut self {
            *coords = new;
        }
        self
    }

    /// A canvas the user pans and zooms; draw in world coordinates (see [`PanZoom`]).
    pub fn pan_zoom(self) -> Self {
        self.pan_zoom_with(PanZoom::default())
    }

    pub fn pan_zoom_with(mut self, new: PanZoom) -> Self {
        if let Node::Canvas { view, .. } = &mut self {
            *view = Some(new);
        }
        self
    }

    /// Fits a pan/zoom canvas's view again whenever `revision` goes up (bind a key or a
    /// button to raising it).
    pub fn reset_view(mut self, revision: u64) -> Self {
        if let Node::Canvas { view: Some(v), .. } = &mut self {
            v.reset = revision;
        }
        self
    }

    /// A canvas that hears pointer moves (`Move`, `Enter`, `Leave`).
    pub fn sense_hover(mut self) -> Self {
        if let Node::Canvas { sense, .. } = &mut self {
            sense.hover = true;
        }
        self
    }

    /// A canvas that takes the wheel as [`Event::Wheel`] and [`Event::Zoom`].
    pub fn sense_wheel(mut self) -> Self {
        if let Node::Canvas { sense, .. } = &mut self {
            sense.wheel = true;
        }
        self
    }

    /// A popup that dims its window and takes its input until it closes.
    pub fn modal(mut self) -> Self {
        if let Node::Popup { modal, .. } = &mut self {
            *modal = true;
        }
        self
    }

    /// A text field's placeholder.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        if let Node::TextField { placeholder, .. } = &mut self {
            *placeholder = text.into();
        }
        self
    }

    /// A text field shown as dots.
    pub fn password(mut self) -> Self {
        if let Node::TextField { password, .. } = &mut self {
            *password = true;
        }
        self
    }

    pub fn read_only(mut self) -> Self {
        if let Node::TextField { read_only, .. } = &mut self {
            *read_only = true;
        }
        self
    }

    /// In the host's mono font (theme.toml's `mono_font`), for code, ids and columns that
    /// must line up: a text, a list or a text field.
    pub fn monospace(mut self) -> Self {
        match &mut self {
            Node::TextField { monospace, .. } => *monospace = true,
            Node::Text { font, .. } | Node::List { font, .. } => *font = Font::Mono,
            _ => {}
        }
        self
    }

    /// In the UI font (theme.toml's `ui_font`). Text and lists already are, unless
    /// they're [`TextStyle::Mono`]; text fields are monospace until this.
    pub fn sans(mut self) -> Self {
        match &mut self {
            Node::TextField { monospace, .. } => *monospace = false,
            Node::Text { font, .. } | Node::List { font, .. } => *font = Font::Sans,
            _ => {}
        }
        self
    }
}
