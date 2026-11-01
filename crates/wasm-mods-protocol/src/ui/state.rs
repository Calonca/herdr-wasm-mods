//! Helpers a module keeps across calls: virtual rows and text field state.

use super::*;

// ---------------------------------------------------------------------------
// Virtual rows
// ---------------------------------------------------------------------------

/// The module's side of a list with virtual rows: which rows show (from
/// [`Event::VisibleRange`]) and scrolling by code. Build only [`VirtualRows::range`]:
///
/// ```ignore
/// self.rows.handle(&input.events, "lines");
/// let range = self.rows.range(total, 20);
/// let items = range.clone().map(|r| item(line(r))).collect();
/// self.rows.view("lines", items, total, range.start)
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct VirtualRows {
    /// The first row showing, and how many show (as the host last said).
    pub first: u64,
    pub count: u64,
    scroll: Option<ScrollTo>,
}

impl Default for VirtualRows {
    fn default() -> Self {
        // Before the host says: enough rows for a tall window.
        VirtualRows { first: 0, count: 80, scroll: None }
    }
}

impl VirtualRows {
    /// Takes in the host's [`Event::VisibleRange`] for list `id`; returns whether it moved.
    pub fn handle(&mut self, events: &[Event], id: &str) -> bool {
        let mut moved = false;
        for event in events {
            if let Event::VisibleRange { id: list, first, count } = event
                && list == id
                && (*first, *count) != (self.first, self.count)
            {
                self.first = *first;
                self.count = *count;
                moved = true;
            }
        }
        moved
    }

    /// The rows to build: those showing and `margin` either side, within `total`.
    pub fn range(&self, total: u64, margin: u64) -> std::ops::Range<u64> {
        let start = self.first.saturating_sub(margin).min(total);
        let end = self.first.saturating_add(self.count).saturating_add(margin).min(total);
        start..end.max(start)
    }

    /// Scrolls the list so `row` is at the top (`u64::MAX`: the end), on the next call.
    pub fn scroll_to(&mut self, row: u64) {
        let revision = self.scroll.map_or(1, |s| s.revision + 1);
        self.scroll = Some(ScrollTo { row, revision });
    }

    /// The list: `total` rows, `items` being rows `first..`.
    pub fn view(&self, id: impl Into<String>, items: Vec<ListItem>, total: u64, first: u64) -> Node {
        let mut n = list(id, items, None);
        if let Node::List { window, .. } = &mut n {
            *window = Some(ListWindow { total, first, scroll_to: self.scroll });
        }
        n
    }
}

// ---------------------------------------------------------------------------
// Text fields
// ---------------------------------------------------------------------------

/// A text field's text on the module's side. The host owns the text while the user types
/// and reports each change ([`Event::Edited`]); [`FieldState::handle`] takes those in.
/// To replace the text (clear it after sending, say), call [`FieldState::set`]: it
/// raises the revision, which is how the host knows to take the new value rather than keep
/// what the user typed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FieldState {
    pub text: String,
    pub revision: u64,
}

impl FieldState {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), revision: 1 }
    }

    /// Takes in the user's edits of field `id`; returns whether the text changed. Edits
    /// made before the last [`FieldState::set`] (an older revision) are dropped.
    pub fn handle(&mut self, events: &[Event], id: &str) -> bool {
        let mut changed = false;
        for event in events {
            if let Event::Edited { id: field, value, revision } = event
                && field == id
                && *revision == self.revision
                && *value != self.text
            {
                self.text.clone_from(value);
                changed = true;
            }
        }
        changed
    }

    /// Replaces the text (the user's typing is dropped).
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.revision += 1;
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    /// A one-line field.
    pub fn view(&self, id: impl Into<String>) -> Node {
        Node::TextField {
            id: id.into(),
            value: self.text.clone(),
            revision: self.revision,
            multiline: false,
            password: false,
            placeholder: String::new(),
            read_only: false,
            rows: Rows::default(),
            monospace: true,
            layout: Layout::default(),
        }
    }

    /// A multi-line field showing `rows` lines.
    pub fn area(&self, id: impl Into<String>, rows: Rows) -> Node {
        let mut n = self.view(id);
        if let Node::TextField { multiline, rows: r, .. } = &mut n {
            *multiline = true;
            *r = rows;
        }
        n
    }
}
