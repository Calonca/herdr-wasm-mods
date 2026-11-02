//! The diff viewer's tree, drawn as an earlier viewer drew it: the source pills, the
//! commit list and the view toggles on top, the range's summary under them; then the file
//! tree and the diff. A diff row is its line numbers, the sign and the code,
//! syntax-coloured, on a faint red or green for removed and added lines, the changed
//! words on a stronger one; a hunk starts with `⋯` and the enclosing function; a file
//! ends with a quiet rule, the all-files view with a doubled one. Both lists have virtual
//! rows: only the rows in view (and a margin) are built.

use crate::git::{DiffLine, FileDiff, FileEntry, LineKind, Source, is_commit_id, short};
use crate::highlight::{Token, Tokens};
use crate::rows::{Cell, Row, wrap};
use crate::{DiffMod, FILES, LINES, PICKER, PICKER_POPUP, Stage, tree};
use wasm_mods_sdk::*;

/// Rows built either side of those in view.
const MARGIN: u64 = 20;
const CODE_SIZE: f32 = 12.5;
/// The commit list's most height.
const PICKER_MAX: f32 = 360.0;
const TREE_WIDTH: f32 = 260.0;
/// Below this width the file list goes above the diff.
const NARROW: f32 = 640.0;
/// Below this width the header's toggles go on a second row.
const TWO_ROWS: f32 = 820.0;
/// A character of the code font, in points (for the wrapping width).
pub(crate) const CHAR: f32 = CODE_SIZE * 0.6;
/// The farthest the code scrolls sideways, in characters.
pub(crate) const PAN_MAX: usize = 4000;

/// The colours the diff uses, from the theme.
struct Palette {
    text: Color,
    muted: Color,
    subtext: Color,
    /// Line numbers: the muted colour, a little fainter.
    number: Color,
    border: Color,
    ansi: [Color; 8],
}

impl Palette {
    fn new(t: &Theme) -> Self {
        let fallback = [0x1d1f21ff, 0xe06c75ff, 0x98c379ff, 0xe5c07bff, 0x61afefff, 0xc678ddff, 0x56b6c2ff, 0xabb2bfff];
        let ansi = std::array::from_fn(|i| t.ansi.get(i).copied().unwrap_or(fallback[i]));
        Palette { text: t.text, muted: t.muted, subtext: t.subtext, number: with_alpha(t.muted, 0xcc), border: t.border, ansi }
    }

    fn red(&self) -> Color {
        self.ansi[1]
    }

    fn green(&self) -> Color {
        self.ansi[2]
    }

    /// A file status's colour: added green, deleted red, renamed blue, the rest yellow.
    fn status(&self, status: char) -> Color {
        match status {
            'A' => self.ansi[2],
            'D' => self.ansi[1],
            'R' | 'C' => self.ansi[4],
            '?' => self.muted,
            _ => self.ansi[3],
        }
    }

    /// A syntax token's colour.
    fn token(&self, token: Token) -> Color {
        let a = &self.ansi;
        match token {
            Token::Text => self.text,
            Token::Comment => self.muted,
            Token::String => a[2],
            Token::Number | Token::Constant | Token::Attribute => a[3],
            Token::Keyword => a[5],
            Token::Function => a[4],
            Token::Type => a[6],
            Token::Tag => a[1],
            Token::Punctuation => self.subtext,
        }
    }

    /// A line kind's fill, sign and the sign's (and changed words') colour.
    fn line(&self, kind: LineKind) -> (Option<Color>, &'static str, Color) {
        match kind {
            LineKind::Added => (Some(with_alpha(self.green(), 0x1c)), "+", self.green()),
            LineKind::Removed => (Some(with_alpha(self.red(), 0x1c)), "−", self.red()),
            LineKind::Context => (None, " ", self.muted),
        }
    }
}

fn run(text: impl Into<String>, color: Color) -> Span {
    Span { text: text.into(), color: Some(color), link: None, bg: None }
}

/// One line of runs in the code font, never wrapped.
fn code(runs: Vec<Span>) -> Node {
    spans(runs).size(CODE_SIZE).font(Font::Mono).wrap_mode(Wrap::Extend)
}

/// How many characters of code fit on a line of a diff in a window `width` wide (one
/// side's, side by side), to wrap at.
pub fn code_cols(width: f32, digits: usize, side_by_side: bool) -> usize {
    let diff = if width < NARROW { width - 28.0 } else { width - 28.0 - TREE_WIDTH - 12.0 };
    let column = if side_by_side { diff / 2.0 - 1.0 } else { diff };
    let gutter = (if side_by_side { digits + 3 } else { 2 * digits + 4 }) as f32 * CHAR;
    (((column - gutter - 16.0) / CHAR).floor() as usize).max(8)
}

impl DiffMod {
    pub(crate) fn view(&mut self, input: &Input) -> Node {
        let p = Palette::new(&input.theme);
        let mut top = vec![self.pills(input), self.summary(&p)];
        if let Some(c) = &self.repo.commit
            && matches!(self.source, Some(Source::Commit(_)))
        {
            top.push(spans(vec![run(c.subject.clone(), p.text), run(format!("  {}", c.author), p.muted)]).size(12.5).truncate());
        }
        let body = match (&self.repo.stage, &self.repo.range) {
            (Stage::Failed(why), _) => text(why.clone()).role(Role::Warning),
            (_, None) => muted("reading…"),
            _ if self.repo.files.is_empty() => muted("no changes in this range"),
            _ => self.body(input, &p),
        };
        let header = column(top).gap(6.0);
        let mut page = vec![header, body];
        if self.picking {
            page.push(self.picker(input, &p));
        }
        column(page).gap(10.0).width(Size::Fill).height(Size::Fill)
    }

    /// The sources (the commit one opening the commit list), then the view toggles and
    /// refresh on the right.
    fn pills(&self, input: &Input) -> Node {
        let mut pills: Vec<Node> = [Source::Uncommitted, Source::Unpushed, Source::Branch]
            .into_iter()
            .map(|s| toggle(format!("source/{}", s.name()), s.name(), self.source.as_ref() == Some(&s)))
            .collect();
        // One commit: the one shown (or handed over); the pill opens the list.
        let commit = match (&self.source, input.arg.as_deref()) {
            (Some(Source::Commit(h)), _) => Some(h.clone()),
            (_, Some(h)) if is_commit_id(h) => Some(h.to_owned()),
            _ => None,
        };
        let label = commit.as_deref().map_or("commit ▾".to_owned(), |h| format!("{} ▾", Source::Commit(h.to_owned()).name()));
        pills.push(toggle("commits", label, matches!(self.source, Some(Source::Commit(_)))));
        // Opened from the git graph: the commit before and after, in its order (greyed
        // out at either end).
        if let Some((older, newer)) = self.steps() {
            let step = |id: &str, label: &str, to: Option<&str>| match to {
                Some(to) => ghost(id, label).tooltip(text(format!("commit {}", short(to)))),
                None => row([text(label).wrap_mode(Wrap::Extend)]).variant(Variant::Button(ButtonKind::Ghost)).opacity(0.4).wrap(false),
            };
            pills.push(step("step/older", "← older", older));
            pills.push(step("step/newer", "newer →", newer));
        }
        let toggles = vec![
            toggle("all-files", "all files", self.all_files),
            toggle("side-by-side", "side by side", self.side_by_side),
            toggle("wrap", "wrap", self.wrap),
            ghost("refresh", "refresh"),
        ];
        // Narrow windows put the view toggles on a second row.
        if input.width > 0.0 && input.width < TWO_ROWS {
            let line = |nodes: Vec<Node>| row(nodes).gap(6.0).wrap(false).width(Size::Fill);
            return column([line(pills), line(toggles)]).gap(6.0).width(Size::Fill);
        }
        pills.push(spacer());
        pills.extend(toggles);
        row(pills).gap(6.0).wrap(false).width(Size::Fill)
    }

    /// The branch's recent commits, under the pills: a click shows one. A virtual list
    /// (it scrolls, within the popup, at most the pane's height less the header's), soft
    /// selection (the rows keep their colours), each row the hash, the subject cut
    /// with … before the time.
    fn picker(&self, input: &Input, p: &Palette) -> Node {
        let recent = self.recent();
        let current = match &self.source {
            Some(Source::Commit(h)) => recent.iter().position(|c| c.0.starts_with(h.as_str()) || h.starts_with(c.0.as_str())),
            _ => None,
        };
        let body = if recent.is_empty() {
            muted("reading…")
        } else {
            let items = recent
                .iter()
                .map(|(hash, when, subject)| {
                    let hash = code(vec![run(format!("{}  ", &hash[..7.min(hash.len())]), p.ansi[4])]).size(12.0);
                    let subject = text(subject.clone()).size(12.5).color(Paint::Rgba(p.text)).truncate().width(Size::Fill);
                    let when = text(format!("  {when}")).size(12.0).color(Paint::Rgba(p.muted));
                    row_item("", row([hash, subject, when]).gap(0.0).wrap(false).width(Size::Fill))
                })
                .collect();
            let room = if input.height > 0.0 { (input.height - 150.0).max(120.0) } else { PICKER_MAX };
            let height = (recent.len() as f32 * 26.0 + 8.0).min(PICKER_MAX).min(room);
            let total = recent.len() as u64;
            let list = self.picker_rows.view(PICKER, items, total, 0).dense().list_style(ListStyle::Soft).selected(current);
            list.width(Size::Fixed(560.0)).height(Size::Fixed(height))
        };
        popup(PICKER_POPUP, Anchor::Node { id: "commits".into(), side: Side::Below }, body)
    }

    /// `origin/main..HEAD · 3 commits · 5 files +10 −2`, and whether it's reading.
    fn summary(&self, p: &Palette) -> Node {
        let mut runs = Vec::new();
        if let Some(range) = &self.repo.range {
            let files = &self.repo.files;
            let (added, removed) = files.iter().fold((0, 0), |(a, r), f| (a + f.added.unwrap_or(0), r + f.removed.unwrap_or(0)));
            runs.push(run(range.label.clone(), p.subtext));
            let mut what = Vec::new();
            match self.repo.commits {
                Some(1) => what.push("1 commit".to_owned()),
                Some(n) => what.push(format!("{n} commits")),
                None => {}
            }
            let n = files.len();
            what.push(format!("{n} file{}", if n == 1 { "" } else { "s" }));
            runs.push(run(format!("   {}   ", what.join(" · ")), p.muted));
            runs.push(run(format!("+{added}"), p.green()));
            runs.push(run(format!(" −{removed}"), p.red()));
        }
        if matches!(self.repo.stage, Stage::Resolving | Stage::Listing) {
            runs.push(run("  reading…", p.muted));
        }
        // Diffs on their way (all files: those on screen and near it).
        let reading = self.repo.stage == Stage::Ready
            && (self.running.iter().chain(self.queue.iter().map(|(id, _)| id)).any(|id| id.contains("/diff/")));
        if reading {
            runs.push(run("   reading…", p.muted));
        }
        code(runs).size(12.0)
    }

    /// The file list and the diff, side by side (or the list on top when narrow).
    fn body(&mut self, input: &Input, p: &Palette) -> Node {
        let narrow = input.width > 0.0 && input.width < NARROW;
        let files = self.file_list(p);
        let diff = self.diff(p);
        if narrow {
            column([files.height(Size::Fixed(120.0)).width(Size::Fill), diff]).gap(8.0).width(Size::Fill).height(Size::Fill)
        } else {
            row([files.width(Size::Fixed(TREE_WIDTH)), diff]).gap(12.0).wrap(false).width(Size::Fill).height(Size::Fill)
        }
    }

    fn file_list(&self, p: &Palette) -> Node {
        let shown = self.shown();
        let range = self.file_rows.range(shown.len() as u64, MARGIN);
        let items = shown[range.start as usize..range.end as usize].iter().map(|&n| self.file_item(&self.repo.tree[n], p)).collect();
        self.file_rows.view(FILES, items, shown.len() as u64, range.start).dense().list_style(ListStyle::Soft).selected(self.selected_row())
    }

    /// A row of the file list: a folder (its arrow folds it) or a file (its status
    /// letter), the name, and its added and removed lines.
    fn file_item(&self, n: &tree::Node, p: &Palette) -> ListItem {
        let indent = gap(6.0 + n.depth as f32 * 14.0);
        let mut counts = Vec::new();
        if n.added > 0 {
            counts.push(run(format!("+{}", n.added), with_alpha(p.green(), 0xd9)));
        }
        if n.removed > 0 {
            counts.push(run(format!(" −{}", n.removed), with_alpha(p.red(), 0xd9)));
        }
        let counts = code(counts).size(11.5);
        match n.kind {
            tree::Kind::Dir => {
                let open = !self.folded.contains(&n.path);
                let arrow = row([text(if open { "▾" } else { "▸" }).size(11.0).color(Paint::Rgba(p.muted))])
                    .id(format!("fold/{}", n.path))
                    .width(Size::Fixed(14.0));
                let name = text(format!("{}/", n.name)).size(12.5).color(Paint::Rgba(p.subtext)).truncate();
                row_item(format!("{}/", n.name), row([indent, arrow, name, spacer(), counts]).gap(0.0).wrap(false).width(Size::Fill))
            }
            tree::Kind::File(i) => {
                let f = &self.repo.files[i];
                let tooltip = match &f.old_path {
                    Some(old) => format!("{old} → {}", f.path),
                    None if f.status == '?' => format!("{} (untracked)", f.path),
                    None => f.path.clone(),
                };
                let letter = row([code(vec![run(f.status.to_string(), p.status(f.status))]).size(11.5)]).width(Size::Fixed(16.0));
                let counts = if f.added.is_none() && f.status != '?' { code(vec![run("bin", p.muted)]).size(11.5) } else { counts };
                let name = text(n.name.clone()).size(12.5).truncate();
                let mut item = row_item(n.name.clone(), row([indent, letter, name, spacer(), counts]).gap(0.0).wrap(false).width(Size::Fill));
                item.tooltip = Some(tooltip);
                item
            }
        }
    }

    /// The diff: the file's bar on top (all files: the one scrolled to), then the rows.
    fn diff(&mut self, p: &Palette) -> Node {
        let Some(file) = self.selected_file().cloned() else { return muted("pick a file") };
        let bar = file_bar(&file, p);
        let total = self.rows().len() as u64;
        if total == 0 {
            let note = match self.repo.diffs.get(&file.path) {
                Some(Err(why)) => format!("can't read it: {why} (Enter reads it again)"),
                _ => "reading…".into(),
            };
            return column([bar, muted(note)]).gap(6.0).width(Size::Fill);
        }
        let range = self.line_range(total);
        self.colour_sent(range.start as usize..range.end as usize);
        let digits = self.digits();
        let (pan, wrapping) = (if self.wrap { 0 } else { self.pan }, self.cols);
        let _ = self.rows();
        let repo = &self.repo;
        let rows = repo.rows.as_ref().map_or(&[][..], |(_, rows)| rows);
        let cx = Draw { files: &repo.files, diffs: &repo.diffs, tokens: &repo.tokens, digits, pan, cols: wrapping, p };
        let items = rows[range.start as usize..range.end as usize].iter().map(|r| row_item("", cx.row(r))).collect();
        let mut lines = self.line_rows.view(LINES, items, total, range.start).dense().font(Font::Mono).pages();
        // The arrows stay with the file list until the user clicks into the diff.
        if let Node::List { keyboard, .. } = &mut lines {
            *keyboard = false;
        }
        column([bar, lines]).gap(6.0).width(Size::Fill).height(Size::Fill)
    }
}

/// A file's bar: its status, the folder muted, the name bright, its line counts.
fn file_bar(f: &FileEntry, p: &Palette) -> Node {
    let mut runs = vec![run(format!("{}  ", f.status), p.status(f.status))];
    if let Some(old) = &f.old_path {
        runs.push(run(format!("{old} → "), p.muted));
    }
    let (dir, name) = f.path.rsplit_once('/').map_or(("", f.path.as_str()), |(d, n)| (d, n));
    if !dir.is_empty() {
        runs.push(run(format!("{dir}/"), p.subtext));
    }
    let name = code(vec![run(name, p.text)]).size(12.0).bold();
    let counts = match (f.added, f.removed) {
        (Some(a), Some(d)) => code(vec![run(format!("−{d}"), p.red()), run(format!("  +{a}"), p.green())]).size(12.0),
        _ if f.status == '?' => code(vec![run("untracked", p.muted)]).size(12.0),
        _ => code(vec![run("binary", p.muted)]).size(12.0),
    };
    row([code(runs).size(12.0), name, spacer(), counts])
        .gap(0.0)
        .wrap(false)
        .width(Size::Fill)
        .padding(Pad::S)
        .bg(Paint::Rgba(with_alpha(p.text, 0x12)))
        .rounding(Rounding::S)
}

/// What drawing rows needs.
struct Draw<'a> {
    files: &'a [FileEntry],
    diffs: &'a std::collections::BTreeMap<String, Result<FileDiff, String>>,
    tokens: &'a std::collections::BTreeMap<(String, usize), crate::highlight::HunkTokens>,
    /// Digits of the widest line number.
    digits: usize,
    /// Characters the code is scrolled sideways.
    pan: usize,
    /// Wrapping at this many characters.
    cols: Option<usize>,
    p: &'a Palette,
}

impl Draw<'_> {
    fn diff(&self, file: usize) -> Option<&FileDiff> {
        self.diffs.get(&self.files.get(file)?.path)?.as_ref().ok()
    }

    fn tokens(&self, file: usize, hunk: usize, line: usize, kind: LineKind) -> Option<&Tokens> {
        let path = &self.files.get(file)?.path;
        self.tokens.get(&(path.clone(), hunk))?.line(line, kind)
    }

    fn row(&self, r: &Row) -> Node {
        let p = self.p;
        match r {
            Row::File { file } => self.files.get(*file).map_or_else(|| row([]), |f| file_bar(f, p)),
            Row::Hunk { file, hunk } => {
                let context = self.diff(*file).and_then(|d| d.hunks.get(*hunk)).map_or("", |h| h.context.as_str());
                let gutter = " ".repeat(2 * self.digits + 2);
                code(vec![run(format!("⋯{gutter}"), p.muted), run(context, p.muted)]).size(11.5)
            }
            Row::Notice { text, .. } => muted(format!("    {text}")),
            Row::Unified { file, hunk, cell, part } => {
                let Some(line) = self.diff(*file).and_then(|d| d.hunks.get(*hunk)?.lines.get(cell.line)) else { return row([]) };
                let (fill, sign, mark) = p.line(line.kind);
                let mut runs = if *part == 0 {
                    vec![
                        run(format!("{} {} ", number(line.old, self.digits), number(line.new, self.digits)), p.number),
                        run(format!("{sign} "), mark),
                    ]
                } else {
                    vec![run(" ".repeat(2 * self.digits + 4), p.number)]
                };
                runs.extend(self.code(line, *cell, *part, self.tokens(*file, *hunk, cell.line, line.kind), mark));
                filled(code(runs), fill)
            }
            Row::Split { file, hunk, left, right, part } => {
                let Some(lines) = self.diff(*file).and_then(|d| d.hunks.get(*hunk)).map(|h| &h.lines) else { return row([]) };
                let half = |cell: &Option<Cell>, old: bool| match cell {
                    None => row([]).bg(Paint::Rgba(with_alpha(p.text, 0x0b))).width(Size::Fill).height(Size::Fill),
                    Some(c) => {
                        let line = &lines[c.line];
                        let kind = if line.kind == LineKind::Context { LineKind::Context } else { line.kind };
                        let (fill, sign, mark) = p.line(kind);
                        let pieces = self.cols.map_or(1, |cols| wrap(&line.text, cols).len());
                        let mut runs = if *part == 0 {
                            vec![run(format!("{} ", number(if old { line.old } else { line.new }, self.digits)), p.number), run(format!("{sign} "), mark)]
                        } else {
                            vec![run(" ".repeat(self.digits + 3), p.number)]
                        };
                        if *part < pieces {
                            // Context lines: each side from its own file.
                            let side = if old { LineKind::Removed } else { LineKind::Added };
                            runs.extend(self.code(line, *c, *part, self.tokens(*file, *hunk, c.line, side), mark));
                        }
                        filled(code(runs), fill).width(Size::Fill)
                    }
                };
                let rule = row([]).width(Size::Fixed(1.0)).height(Size::Fill).bg(Paint::Rgba(p.border));
                row([half(left, true), rule, half(right, false)]).gap(0.0).wrap(false).width(Size::Fill)
            }
            Row::End { file, next } => {
                let name = self.files.get(*file).map_or("", |f| f.path.rsplit('/').next().unwrap_or_default());
                let label = if *next { format!("end of {name}  ·  tab for the next file") } else { format!("end of {name}") };
                end_marker(&label, false, p)
            }
            Row::DiffEnd => {
                let (added, removed) = self.files.iter().fold((0, 0), |(a, r), f| (a + f.added.unwrap_or(0), r + f.removed.unwrap_or(0)));
                let n = self.files.len();
                end_marker(&format!("end of diff  ·  {n} file{}  +{added} −{removed}", if n == 1 { "" } else { "s" }), true, p)
            }
        }
    }

    /// A line's code (piece `part` when wrapping, else what's right of the sideways
    /// scroll), syntax-coloured, the changed part (if any) on a stronger fill.
    fn code(&self, line: &DiffLine, cell: Cell, part: usize, tokens: Option<&Tokens>, mark: Color) -> Vec<Span> {
        let t = &line.text;
        let (s, e) = match self.cols {
            Some(cols) => wrap(t, cols).get(part).copied().unwrap_or((t.len(), t.len())),
            None => (t.char_indices().nth(self.pan).map_or(t.len(), |(i, _)| i), t.len()),
        };
        // Cut points: token edges and the change's, inside [s, e).
        let mut cuts = vec![s, e];
        if let Some(tokens) = tokens {
            cuts.extend(tokens.iter().flat_map(|&(a, b, _)| [a, b]));
        }
        let change = cell.change.filter(|&(a, b)| a < b && b <= t.len());
        if let Some((a, b)) = change {
            cuts.extend([a, b]);
        }
        cuts.retain(|&c| c >= s && c <= e && t.is_char_boundary(c));
        cuts.sort_unstable();
        cuts.dedup();
        let color_at = |at: usize| {
            let token = tokens.and_then(|ts| ts.iter().find(|&&(a, b, _)| a <= at && at < b)).map_or(Token::Text, |t| t.2);
            self.p.token(token)
        };
        let mut out: Vec<Span> = Vec::new();
        for w in cuts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (color, bg) = (color_at(a), change.is_some_and(|(x, y)| x <= a && b <= y).then(|| with_alpha(mark, 0x47)));
            // Next to a run of the same look: one run (fewer to send and draw).
            match out.last_mut() {
                Some(last) if last.color == Some(color) && last.bg == bg => last.text.push_str(&t[a..b]),
                _ => out.push(Span { bg, ..run(&t[a..b], color) }),
            }
        }
        out
    }
}

/// Room across a row, `w` points.
fn gap(w: f32) -> Node {
    row([]).width(Size::Fixed(w)).height(Size::Fixed(1.0))
}

fn number(n: Option<u32>, digits: usize) -> String {
    match n {
        Some(n) => format!("{n:>digits$}"),
        None => " ".repeat(digits),
    }
}

/// A line on its fill (removed and added lines), across the row.
fn filled(node: Node, fill: Option<Color>) -> Node {
    // The row's whole height: a line's fill meets the next one's.
    let r = row([node]).wrap(false).width(Size::Fill).height(Size::Fill);
    match fill {
        Some(c) => r.bg(Paint::Rgba(c)),
        None => r,
    }
}

/// A rule across the row with `label` in its middle: the end of a file, or (`last`,
/// doubled and brighter) of the whole diff.
fn end_marker(label: &str, last: bool, p: &Palette) -> Node {
    let color = if last { with_alpha(p.muted, 0xb3) } else { p.border };
    let rule = || {
        let line = || row([]).height(Size::Fixed(1.0)).width(Size::Fill).bg(Paint::Rgba(color));
        if last { column([line(), space(2.0), line()]).width(Size::Fill) } else { column([line()]).width(Size::Fill) }
    };
    let label = text(label.to_owned()).size(if last { 12.0 } else { 11.5 }).font(Font::Sans).color(Paint::Rgba(if last { p.subtext } else { p.muted }));
    row([gap(4.0), rule(), label, rule(), gap(4.0)]).gap(12.0).align(Align::Center).wrap(false).width(Size::Fill)
}
