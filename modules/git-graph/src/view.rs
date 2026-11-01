//! What the graph shows: a header, the commit list (only the rows near those showing) and,
//! once opened, the selected commit's details beside the list or, in a narrow window,
//! below it.

use wasm_mods_sdk::*;

use crate::data::{self, Commit, Details, GitRef, RefKind};
use crate::lanes::{At, Row};
use crate::{GitGraph, LIST, MARGIN};

/// Lanes beyond this many are cut (a wide graph would push the text away).
pub const MAX_LANES: usize = 10;
const LANE_WIDTH: f32 = 14.0;
/// A dense list row's height: the lanes run from its top to its bottom.
const ROW_HEIGHT: f32 = 26.0;
/// The right-hand columns: author, then age (`11mo`).
const AUTHOR_WIDTH: f32 = 110.0;
const AGE_WIDTH: f32 = 34.0;
/// From this width the details go beside the list; they take this share of it.
const WIDE: f32 = 820.0;
const DETAILS_SHARE: f32 = 0.4;

/// Lane colours: the theme's ANSI colours, accent first.
fn palette(t: &Theme) -> [Color; 8] {
    let a = |i: usize| ansi(t, i);
    [t.accent, a(5), a(6), a(2), a(3), a(1), a(13), a(14)]
}

fn ansi(t: &Theme, i: usize) -> Color {
    t.ansi.get(i).copied().unwrap_or(t.accent)
}

/// A lane's line within a row: straight down, or an S-curve between columns.
fn curve(a: (f32, f32), b: (f32, f32), color: Color) -> Shape {
    if (a.0 - b.0).abs() < 0.5 {
        return Shape::Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1, width: 1.6, color };
    }
    let mid = (a.1 + b.1) / 2.0;
    let points = (0..=8)
        .map(|i| {
            // The cubic Bézier through a, (a.x, mid), (b.x, mid), b.
            let s = i as f32 / 8.0;
            let u = 1.0 - s;
            let x = u * u * u * a.0 + 3.0 * u * u * s * a.0 + 3.0 * u * s * s * b.0 + s * s * s * b.0;
            let y = u * u * u * a.1 + 3.0 * u * u * s * mid + 3.0 * u * s * s * mid + s * s * s * b.1;
            (x, y)
        })
        .collect();
    Shape::Path { points, closed: false, fill: None, stroke: Some((1.6, color)) }
}

/// One row's lanes and the commit's dot: the checked-out commit ringed, a merge hollow.
fn lane_canvas(commit: &Commit, row: &Row, lanes: usize, t: &Theme) -> Node {
    let palette = palette(t);
    let x = |column: usize| column as f32 * LANE_WIDTH + LANE_WIDTH / 2.0;
    let y = |at: At| match at {
        At::Top => 0.0,
        At::Dot => ROW_HEIGHT / 2.0,
        At::Bottom => ROW_HEIGHT,
    };
    let mut shapes: Vec<Shape> = row
        .lines
        .iter()
        .filter(|l| l.from.0 < MAX_LANES && l.to.0 < MAX_LANES)
        .map(|l| curve((x(l.from.0), y(l.from.1)), (x(l.to.0), y(l.to.1)), palette[l.color % palette.len()]))
        .collect();
    let (cx, cy) = (x(row.column.min(MAX_LANES - 1)), ROW_HEIGHT / 2.0);
    let color = palette[row.color % palette.len()];
    let head = commit.refs.iter().any(|r| matches!(r.kind, RefKind::Current | RefKind::Head));
    let dot = |r: f32, fill: Color| Shape::Circle { x: cx, y: cy, r, fill };
    if head {
        shapes.extend([dot(6.5, t.text), dot(4.5, t.accent)]);
    } else if commit.parents.len() > 1 {
        shapes.extend([dot(4.9, color), dot(3.1, t.space)]);
    } else {
        shapes.push(dot(4.0, color));
    }
    let width = lanes as f32 * LANE_WIDTH;
    canvas(format!("lane/{}", &commit.hash[..12.min(commit.hash.len())]), width, ROW_HEIGHT, shapes)
}

/// A ref as a rounded pill. The checked-out branch (and a detached HEAD) is solid, the
/// others tinted: branches green, remotes magenta, tags yellow.
fn pill(r: &GitRef, t: &Theme) -> Node {
    let (fill, color) = match r.kind {
        RefKind::Current | RefKind::Head => (t.accent, t.space),
        RefKind::Branch => (with_alpha(ansi(t, 2), 0x2e), ansi(t, 2)),
        RefKind::Remote => (with_alpha(ansi(t, 5), 0x2e), ansi(t, 5)),
        RefKind::Tag => (with_alpha(ansi(t, 3), 0x2e), ansi(t, 3)),
    };
    let name = if r.kind == RefKind::Tag { format!("◆ {}", r.name) } else { r.name.clone() };
    let mut label = text(name).size(11.5).color(Paint::Rgba(color)).wrap_mode(Wrap::Extend);
    if matches!(r.kind, RefKind::Current | RefKind::Head) {
        label = label.bold();
    }
    row([label]).variant(Variant::Pill).bg(Paint::Rgba(fill)).wrap(false)
}

impl GitGraph {
    /// Lanes, hash, refs and subject (cut short), then author and age in fixed columns.
    fn commit_row(&self, i: usize, now: Option<i64>, wide: bool, t: &Theme) -> ListItem {
        let repo = &self.repo;
        let c = &repo.commits[i];
        let mut middle = vec![mono(c.hash[..7.min(c.hash.len())].to_owned()).size(12.0).role(Role::Muted).wrap_mode(Wrap::Extend)];
        middle.extend(c.refs.iter().map(|r| pill(r, t)));
        middle.push(text(c.subject.clone()).size(13.0).wrap_mode(Wrap::Truncate));
        let mut parts = vec![lane_canvas(c, &repo.graph[i], repo.lanes, t), row(middle).gap(8.0).wrap(false).width(Size::Fill)];
        if wide {
            let author = text(c.author.clone()).size(12.5).role(Role::Subtext).wrap_mode(Wrap::Truncate);
            parts.push(row([author]).width(Size::Fixed(AUTHOR_WIDTH)));
        }
        let age = now.map(|n| data::age(n - c.time)).unwrap_or_default();
        let age = mono(age).size(12.0).role(Role::Muted).wrap_mode(Wrap::Extend);
        parts.push(row([age]).justify(Justify::End).width(Size::Fixed(AGE_WIDTH)));
        row_item(c.subject.clone(), row(parts).gap(10.0).wrap(false).width(Size::Fill))
    }

    /// The repo's name, its branch, how many commits are read.
    fn header(&self, input: &Input) -> Node {
        let repo = &self.repo;
        let name = repo.folder.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_owned();
        let mut parts = vec![text(name).size(14.0).bold().role(Role::Heading).wrap_mode(Wrap::Extend)];
        if let Some(branch) = input.window.as_ref().and_then(|w| w.branch.clone()) {
            parts.push(mono(branch).size(12.0).role(Role::Subtext).wrap_mode(Wrap::Extend));
        }
        if repo.loaded {
            let more = if repo.commits.len() >= repo.limit { "+" } else { "" };
            parts.push(text(format!("{}{more} commits", repo.commits.len())).size(12.0).role(Role::Muted).wrap_mode(Wrap::Extend));
        }
        row(parts).gap(14.0).wrap(false).width(Size::Fill)
    }

    /// The commit list, built only around the rows showing. `width`: the room it has.
    fn list(&self, input: &Input, width: f32) -> Node {
        let total = self.repo.commits.len() as u64;
        let range = self.rows.range(total, MARGIN);
        let now = self.repo.now(input.time);
        let wide = width > 440.0;
        let items = range.clone().map(|i| self.commit_row(i as usize, now, wide, &input.theme)).collect();
        self.rows
            .view(LIST, items, total, range.start)
            .selected(self.selected_row())
            .list_style(ListStyle::Soft)
            .dense()
            .height(Size::Fill)
    }

    /// The selected commit: subject, hash, author, date, the rest of the message, files.
    fn details_pane(&self, input: &Input) -> Node {
        let Some(c) = self.selected_row().map(|i| &self.repo.commits[i]) else {
            return column([]);
        };
        let mut parts = vec![
            text(c.subject.clone()).size(14.0).bold().role(Role::Heading),
            mono(c.hash.clone()).size(12.0).role(Role::Muted).wrap_mode(Wrap::Extend),
        ];
        match self.repo.details(&c.hash) {
            None => parts.push(text("reading…").size(12.5).role(Role::Muted)),
            Some(Err(e)) => parts.push(text(e.clone()).role(Role::Danger)),
            Some(Ok(d)) => parts.extend(details_parts(d, &input.theme)),
        }
        column(parts).gap(4.0).scroll(Scroll::Vertical)
    }

    pub(crate) fn view(&self, input: &Input) -> Node {
        let repo = &self.repo;
        let header = self.header(input);
        if !repo.loaded || repo.commits.is_empty() {
            let note = match &repo.error {
                Some(e) => text(e.clone()).size(13.0).role(Role::Danger),
                None if repo.loaded => text("no commits yet").size(13.0).role(Role::Muted),
                None => text("reading history…").size(13.0).role(Role::Muted),
            };
            return column([header, note]).gap(10.0);
        }
        let mut parts = vec![header];
        if let Some(e) = &repo.error {
            parts.push(small(e.clone()).role(Role::Danger));
        }
        let open = self.details && self.selected_row().is_some();
        if open && input.width > WIDE {
            let share = (input.width * DETAILS_SHARE).round();
            let list = self.list(input, input.width - share).width(Size::Fill);
            let details = self.details_pane(input).width(Size::Fixed(share - 12.0)).height(Size::Fill);
            parts.push(row([list, details]).gap(12.0).dividers().wrap(false).align(Align::Start).width(Size::Fill).height(Size::Fill));
        } else if open {
            let share = (input.height * DETAILS_SHARE).round();
            let details = self.details_pane(input).width(Size::Fill).height(Size::Fixed(share));
            parts.extend([self.list(input, input.width), separator(), details]);
        } else {
            parts.push(self.list(input, input.width));
        }
        column(parts).gap(10.0).height(Size::Fill)
    }
}

/// Who and when, the message's body, and the files changed with their `+`/`−` lines.
fn details_parts(d: &Details, t: &Theme) -> Vec<Node> {
    let green = ansi(t, 2);
    let red = ansi(t, 1);
    let mut parts = vec![
        row([
            text(d.author.clone()).size(12.5).wrap_mode(Wrap::Extend),
            text(format!("<{}>", d.email)).size(12.5).role(Role::Muted).wrap_mode(Wrap::Truncate),
        ])
        .gap(6.0)
        .wrap(false)
        .width(Size::Fill),
        mono(d.date.replace('T', " ")).size(12.0).role(Role::Muted),
    ];
    let body = d.message.split_once('\n').map(|(_, rest)| rest.trim()).unwrap_or_default();
    if !body.is_empty() {
        parts.extend([space(4.0), text(body.to_owned()).size(13.0)]);
    }
    let (added, removed) = d.files.iter().fold((0, 0), |(a, r), f| (a + f.added.unwrap_or(0), r + f.removed.unwrap_or(0)));
    let files = if d.files.len() == 1 { "1 file".to_owned() } else { format!("{} files", d.files.len()) };
    parts.push(space(6.0));
    parts.push(
        row([
            text(files).size(12.5).bold().role(Role::Subtext).wrap_mode(Wrap::Extend),
            mono(format!("+{added}")).size(12.0).color(Paint::Rgba(green)).wrap_mode(Wrap::Extend),
            mono(format!("−{removed}")).size(12.0).color(Paint::Rgba(red)).wrap_mode(Wrap::Extend),
        ])
        .gap(8.0)
        .wrap(false),
    );
    for f in &d.files {
        // The folder muted, the file name bright; the counts at the right.
        let (dir, file) = f.path.rsplit_once('/').map_or(("", f.path.as_str()), |(d, n)| (d, n));
        let mut name = Vec::new();
        if !dir.is_empty() {
            name.push(Span { text: format!("{dir}/"), color: Some(t.muted), link: None, bg: None });
        }
        name.push(Span { text: file.to_owned(), color: None, link: None, bg: None });
        let path = spans(name).font(Font::Mono).size(12.0).wrap_mode(Wrap::Truncate);
        let stats = match (f.added, f.removed) {
            (Some(a), Some(r)) => vec![
                mono(format!("+{a}")).size(12.0).color(Paint::Rgba(green)).wrap_mode(Wrap::Extend),
                mono(format!("−{r}")).size(12.0).color(Paint::Rgba(red)).wrap_mode(Wrap::Extend),
            ],
            _ => vec![mono("binary").size(12.0).role(Role::Muted).wrap_mode(Wrap::Extend)],
        };
        parts.push(row([row([path]).width(Size::Fill), row(stats).gap(8.0).wrap(false)]).gap(6.0).wrap(false).width(Size::Fill));
    }
    parts
}
