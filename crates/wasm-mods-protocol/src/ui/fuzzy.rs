//! Fuzzy search: matching and ranking, highlighting matches, and a search box with its
//! list.

use super::*;

/// Where `query` matched in a text, and how well.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuzzyMatch {
    /// Higher is better.
    pub score: i32,
    /// The matched characters (char indices into the text), in order.
    pub positions: Vec<usize>,
}

/// Whether every character of `query` appears in `text` in order (ignoring case and
/// spaces in the query), scored: contiguous runs, word starts and the text's start count
/// most, a matching case a little, every skipped character a little against. `None`: no
/// match. An empty query matches everything with score 0.
pub fn fuzzy_match(query: &str, text: &str) -> Option<FuzzyMatch> {
    const HIT: i32 = 16;
    const RUN: i32 = 24;
    const WORD: i32 = 20;
    const FIRST: i32 = 12;
    const CASE: i32 = 2;
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let t: Vec<char> = text.chars().collect();
    let (m, n) = (q.len(), t.len());
    if m == 0 {
        return Some(FuzzyMatch::default());
    }
    if m > n {
        return None;
    }
    let same = |a: char, b: char| a == b || a.to_lowercase().eq(b.to_lowercase());
    // What a character earns where it is, besides the run bonus.
    let bonus = |qc: char, j: usize| -> i32 {
        let mut b = HIT;
        let prev = j.checked_sub(1).map(|p| t[p]);
        let word = match prev {
            None => true,
            Some(p) => !p.is_alphanumeric() || (p.is_lowercase() && t[j].is_uppercase()),
        };
        if word {
            b += WORD;
        }
        if j == 0 {
            b += FIRST;
        }
        if qc == t[j] {
            b += CASE;
        }
        b
    };
    // best[i][j]: the best score with q[..=i] matched and q[i] on t[j]; from[i][j] where
    // q[i-1] went.
    const NONE: i32 = i32::MIN / 2;
    let mut best = vec![vec![NONE; n]; m];
    let mut from = vec![vec![0usize; n]; m];
    for j in 0..n {
        if same(q[0], t[j]) {
            // Characters skipped before the first match count a little against.
            best[0][j] = bonus(q[0], j) - (j as i32).min(10);
        }
    }
    for i in 1..m {
        let (done, rest) = best.split_at_mut(i);
        let (prev, row) = (&done[i - 1], &mut rest[0]);
        for j in i..n {
            if !same(q[i], t[j]) {
                continue;
            }
            let mut top = NONE;
            for (k, &p) in prev.iter().enumerate().take(j).skip(i - 1) {
                if p == NONE {
                    continue;
                }
                let link = if k + 1 == j { RUN } else { -((j - k - 1) as i32).min(20) - 2 };
                if p + link > top {
                    top = p + link;
                    from[i][j] = k;
                }
            }
            if top != NONE {
                row[j] = top + bonus(q[i], j);
            }
        }
    }
    let (mut j, score) = (0..n).map(|j| (j, best[m - 1][j])).max_by_key(|&(j, s)| (s, std::cmp::Reverse(j)))?;
    if score == NONE {
        return None;
    }
    let mut positions = vec![0; m];
    for i in (0..m).rev() {
        positions[i] = j;
        j = from[i][j];
    }
    Some(FuzzyMatch { score, positions })
}

/// The indices of `texts` matching `query`, best first (ties: the shorter text, then
/// the earlier one), with where each matched. An empty query keeps them all, in order.
pub fn fuzzy_rank<'a>(query: &str, texts: impl IntoIterator<Item = &'a str>) -> Vec<(usize, FuzzyMatch)> {
    let mut hits: Vec<(usize, usize, FuzzyMatch)> = texts
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| fuzzy_match(query, t).map(|m| (i, t.chars().count(), m)))
        .collect();
    if query.trim().is_empty() {
        return hits.into_iter().map(|(i, _, m)| (i, m)).collect();
    }
    hits.sort_by(|a, b| b.2.score.cmp(&a.2.score).then(a.1.cmp(&b.1)).then(a.0.cmp(&b.0)));
    hits.into_iter().map(|(i, _, m)| (i, m)).collect()
}

/// `text` as runs with the characters at `positions` in `color`, the rest in the
/// text's own colour.
pub fn highlight(text: &str, positions: &[usize], color: Color) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (i, ch) in text.chars().enumerate() {
        let c = positions.contains(&i).then_some(color);
        match out.last_mut() {
            Some(last) if last.color == c => last.text.push(ch),
            _ => out.push(Span { text: ch.to_string(), color: c, link: None, bg: None }),
        }
    }
    out
}

/// A search field above a list that it filters as the user types: the field keeps the
/// keyboard, [`Action::Move`] (`↑/↓`, `PageUp/PageDown`, the d-pad) moves the selection,
/// [`Action::Open`] (`Enter`, A) or a click confirms, [`Action::Back`] (`Escape`, B)
/// clears the query and then closes. An empty query shows every item in its order
/// (section `header`s too); a query shows the matching items, best first, their matched
/// characters highlighted. Offer [`Fuzzy::offers`] so the host shows those.
///
/// The ids under `id`: the field is `<id>/query`, the list `<id>/list`.
///
/// ```ignore
/// for action in self.search.handle(&input.events, "search", &items) {
///     match action {
///         FuzzyAction::Confirmed(i) => …,
///         FuzzyAction::Focus => effects.push(Effect::Focus { id: Fuzzy::field("search") }),
///         FuzzyAction::Closed => effects.push(Effect::Close),
///         FuzzyAction::Moved(_) => {}
///     }
/// }
/// let out = Output::new("pick", self.search.view("search", items, input.theme.accent));
/// self.search.offers("apply").into_iter().fold(out, |o, (a, l)| o.action(a, l))
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fuzzy {
    pub query: FieldState,
    /// The selected item (an index into the items, not the rows shown).
    pub selected: usize,
    /// The field has been given the keyboard once.
    #[serde(default)]
    pub opened: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FuzzyAction {
    /// The selection moved to this item.
    Moved(usize),
    /// This item was chosen ([`Action::Open`] or a click).
    Confirmed(usize),
    /// [`Action::Back`] with an empty query.
    Closed,
    /// Give the keyboard to the field ([`Fuzzy::field`]): on open, and after open or
    /// back.
    Focus,
}

impl Fuzzy {
    /// The field's id for the search `id`.
    pub fn field(id: &str) -> String {
        format!("{id}/query")
    }

    /// The list's id for the search `id`.
    pub fn list(id: &str) -> String {
        format!("{id}/list")
    }

    /// The rows shown for the query: item indices, with where each matched.
    pub fn shown(&self, items: &[ListItem]) -> Vec<(usize, Vec<usize>)> {
        let q = self.query.text.trim();
        if q.is_empty() {
            return (0..items.len()).map(|i| (i, Vec::new())).collect();
        }
        let pickable: Vec<usize> = (0..items.len()).filter(|&i| !items[i].header && !items[i].disabled).collect();
        fuzzy_rank(q, pickable.iter().map(|&i| items[i].label.as_str()))
            .into_iter()
            .map(|(k, m)| (pickable[k], m.positions))
            .collect()
    }

    /// The row the selection is on (the first pickable row if the selected item isn't
    /// shown).
    fn row(&self, items: &[ListItem], shown: &[(usize, Vec<usize>)]) -> Option<usize> {
        let pickable = |r: &usize| !items[shown[*r].0].header && !items[shown[*r].0].disabled;
        shown.iter().position(|(i, _)| *i == self.selected).filter(pickable).or_else(|| (0..shown.len()).find(pickable))
    }

    /// The actions it takes, labelled, for [`Output::actions`]: back ("clear" while there
    /// is a query, else "close"), open (`confirm`) and move ("choose").
    pub fn offers(&self, confirm: &str) -> Vec<(Action, String)> {
        let back = if self.query.text.is_empty() { "close" } else { "clear" };
        vec![
            (Action::Back, back.into()),
            (Action::Open, confirm.into()),
            (Action::Move(Move::Down), "choose".into()),
        ]
    }

    /// Selects item `index` (e.g. the current value, on open).
    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }

    /// Applies this call's events for the search `id` over `items`.
    pub fn handle(&mut self, events: &[Event], id: &str, items: &[ListItem]) -> Vec<FuzzyAction> {
        let (field, list) = (Self::field(id), Self::list(id));
        let mut actions = Vec::new();
        if !self.opened {
            self.opened = true;
            actions.push(FuzzyAction::Focus);
        }
        for event in events {
            let before = self.selected;
            let shown = self.shown(items);
            let row = self.row(items, &shown);
            let step = |down: bool, by: usize| -> Option<usize> {
                let mut r = row?;
                let mut left = by;
                let mut last = r;
                while left > 0 {
                    r = match (down, r.checked_sub(1)) {
                        (true, _) => r + 1,
                        (false, Some(up)) => up,
                        (false, None) => break,
                    };
                    let Some((i, _)) = shown.get(r) else { break };
                    if !items[*i].header && !items[*i].disabled {
                        last = r;
                        left -= 1;
                    }
                }
                Some(shown[last].0)
            };
            let mut requery = false;
            match event {
                Event::Edited { id: f, .. } if *f == field => {
                    requery = self.query.handle(std::slice::from_ref(event), &field);
                }
                Event::Action(action) => {
                    let to = match action {
                        Action::Move(Move::Down) => step(true, 1),
                        Action::Move(Move::Up) => step(false, 1),
                        Action::Move(Move::PageDown) => step(true, 10),
                        Action::Move(Move::PageUp) => step(false, 10),
                        Action::Open => {
                            if let Some(r) = row {
                                actions.push(FuzzyAction::Confirmed(shown[r].0));
                            }
                            actions.push(FuzzyAction::Focus);
                            None
                        }
                        Action::Back => {
                            if self.query.text.is_empty() {
                                actions.push(FuzzyAction::Closed);
                            } else {
                                self.query.clear();
                                actions.push(FuzzyAction::Focus);
                            }
                            None
                        }
                        _ => None,
                    };
                    if let Some(i) = to {
                        self.selected = i;
                    }
                }
                Event::Selected { id: l, index } if *l == list => {
                    if let Some((i, _)) = shown.get(*index) {
                        self.selected = *i;
                        actions.push(FuzzyAction::Confirmed(*i));
                    }
                    continue;
                }
                _ => {}
            }
            if requery {
                // A new query starts on its best match.
                let shown = self.shown(items);
                if let Some(r) = (0..shown.len()).find(|r| !items[shown[*r].0].header && !items[shown[*r].0].disabled) {
                    self.selected = shown[r].0;
                }
            }
            if self.selected != before {
                actions.push(FuzzyAction::Moved(self.selected));
            }
        }
        actions
    }

    /// The field above the list of the rows shown, matched characters in `highlight`.
    pub fn view(&self, id: &str, items: Vec<ListItem>, highlight_color: Color) -> Node {
        let shown = self.shown(&items);
        let row = self.row(&items, &shown);
        let filtering = !self.query.text.trim().is_empty();
        let rows: Vec<ListItem> = shown
            .into_iter()
            .map(|(i, positions)| {
                let mut item = items[i].clone();
                if filtering {
                    item.label_spans = highlight(&item.label, &positions, highlight_color);
                }
                item
            })
            .collect();
        let empty = filtering && rows.is_empty();
        let mut children = vec![self.query.view(Self::field(id)).placeholder("search").width(Size::Fill)];
        if empty {
            children.push(muted("nothing matches"));
        }
        children.push(own_keys(list(Self::list(id), rows, row)).width(Size::Fill));
        column(children).gap(8.0).width(Size::Fill)
    }
}
