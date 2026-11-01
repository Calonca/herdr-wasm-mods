//! diff: what changed in the tab's folder. On top, the source: uncommitted changes (the
//! working tree against `HEAD`, the default), the unpushed commits, the whole branch, or
//! one commit (opened with a hash, as the git graph does). On the left, the changed files
//! as a tree with their line counts (Enter on a folder folds it); on the right, the
//! selected file's diff, or every file's one after another ("all files", a), unified or
//! side by side (s), syntax-coloured, with the changed words marked; long lines scroll
//! sideways (←/→) or wrap (w). Tab goes to the next file (Shift+Tab back), Enter
//! (or "refresh") reads it all again; while the window has focus it also looks again
//! every few seconds. "commit ▾" picks one of the branch's recent commits.
//!
//! Everything comes from git, run by the host in the tab's folder (capability `git`,
//! reading commands only): the mod asks with `run_tool("git", …)` and reads the answers
//! in a later call. Untracked files are listed but not shown: their content is outside
//! git, and the commands that would read it (`diff --no-index`) aren't allowed.
//!
//! Build:   cargo build --release --target wasm32-unknown-unknown   (in modules/)
//! Install: ../scripts/install.sh diff

mod git;
mod highlight;
mod rows;
mod tree;
mod view;

use std::collections::{BTreeMap, BTreeSet};

use git::*;
use rows::Row;
use wasm_mods_sdk::*;

/// The file list's and the diff lines' ids: their events carry them.
const FILES: &str = "files";
const LINES: &str = "lines";
/// The commit list's popup, and its list.
const PICKER_POPUP: &str = "commit-list";
const PICKER: &str = "commit-list/items";
/// The file list's row a click landed on (`files#<row>`, from the host).
fn folder_click(id: &str) -> Option<usize> {
    id.strip_prefix(FILES)?.strip_prefix('#')?.parse().ok()
}

/// How far ←/→ scroll the code sideways, in characters.
const PAN: usize = 8;
/// The header's toggles as keys of their own: (name, key, hint).
const TOGGLES: [(&str, &str, &str); 3] = [("all-files", "a", "all files"), ("side-by-side", "s", "side by side"), ("wrap", "w", "wrap")];
/// git runs the host lets a module have going at once.
const RUNS_MAX: usize = 4;
/// All files: diffs are read for the rows on screen and this many rows either side.
const AHEAD_ROWS: usize = 60;
/// How often (seconds) it reads the changes again while the window has focus.
const POLL_SECS: f64 = 4.0;

/// How far reading the current source has got.
#[derive(Clone, Debug, Default, PartialEq)]
enum Stage {
    /// Nothing asked yet: the next call starts.
    #[default]
    Idle,
    /// Turning the source into a range (`base`, `head`).
    Resolving,
    /// Reading the files the range changes.
    Listing,
    Ready,
    Failed(String),
}

/// What has been read from git. Not kept across a reload of a new build: read again.
#[derive(Default)]
struct Repo {
    stage: Stage,
    /// The answers of this generation's runs, by name, and the names asked.
    got: BTreeMap<String, ToolResult>,
    asked: BTreeSet<String>,
    /// The range shown, and the one being read (shown once its files are in).
    range: Option<Range>,
    next: Option<Range>,
    commit: Option<Commit>,
    /// Commits in the range, when it's one.
    commits: Option<u32>,
    files: Vec<FileEntry>,
    tree: Vec<tree::Node>,
    /// The file list's rows (the tree's nodes outside folded folders), worked out once
    /// per change of the tree or the folds.
    shown: std::cell::OnceCell<Vec<usize>>,
    /// What the files list was read from: unchanged, the diffs read stay.
    stamp: String,
    /// File diffs read, by path, and a count raised whenever one comes in.
    diffs: BTreeMap<String, Result<FileDiff, String>>,
    version: u64,
    /// The rows shown, and what they were built for.
    rows: Option<(RowKey, Vec<Row>)>,
    /// Syntax tokens of the hunks shown so far, by `(path, hunk)`.
    tokens: BTreeMap<(String, usize), highlight::HunkTokens>,
}

/// What the rows depend on: all files, side by side, the file (one-file view), the
/// wrapping width, the diffs read.
type RowKey = (bool, bool, Option<String>, Option<usize>, u64);

/// The mod's state. The host keeps it across a reload of a new build (it's serialized),
/// so give fields added later `#[serde(default)]`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct DiffMod {
    /// `None` until the first call picks it (a commit handed over, else uncommitted).
    source: Option<Source>,
    #[serde(default)]
    side_by_side: bool,
    /// Every file's diff one after another, each after its name bar.
    #[serde(default)]
    all_files: bool,
    /// Long lines wrap instead of scrolling sideways.
    #[serde(default)]
    wrap: bool,
    /// Folders folded in the file list, by path.
    #[serde(default)]
    folded: Vec<String>,
    /// Characters the code is scrolled sideways (not wrapping).
    #[serde(default)]
    pan: usize,
    /// The wrapping width in characters of code, worked out from the window's width.
    #[serde(skip)]
    cols: Option<usize>,
    /// The commit list ("commit ▾") is open.
    #[serde(skip)]
    picking: bool,
    /// The selected file, by path, so it stays selected when the list is read again.
    #[serde(default)]
    selected: Option<String>,
    /// The folder last read: another one is read from scratch.
    #[serde(default)]
    folder: Option<String>,
    /// Raised on every read; answers to an older one are dropped.
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    file_rows: VirtualRows,
    #[serde(default)]
    line_rows: VirtualRows,
    #[serde(skip)]
    picker_rows: VirtualRows,
    #[serde(skip)]
    polled: f64,
    #[serde(skip)]
    repo: Repo,
    /// Runs waiting for a free slot (`id`, arguments), and those on their way.
    #[serde(skip)]
    queue: Vec<(String, Vec<String>)>,
    #[serde(skip)]
    running: BTreeSet<String>,
    /// Runs at once: [`RUNS_MAX`], less after the host refused one (the module under this
    /// one, the git graph, has runs of its own on the same limit). Back up on a reload.
    #[serde(skip)]
    cap: Option<usize>,
    /// Esc, opened over another module: go back to it (this call's answer says so).
    #[serde(skip)]
    back: bool,
}

impl Frame for DiffMod {
    fn update(&mut self, input: &Input) -> Output {
        self.take_answers(input);
        let Some(window) = &input.window else {
            let ui = column([heading("diff"), muted("waiting for the folder: allow this mod to see the tab's folder")]);
            return Output::new("diff", ui).wants_window();
        };
        if self.source.is_none() {
            self.source = Some(match input.arg.as_deref() {
                Some(hash) if is_commit_id(hash) => Source::Commit(hash.to_owned()),
                _ => Source::Uncommitted,
            });
        }
        if self.folder.as_deref() != Some(window.folder.as_str()) {
            self.folder = Some(window.folder.clone());
            self.repo = Repo::default();
        }
        self.handle(input);
        if self.repo.stage == Stage::Idle {
            self.reload(true);
            self.polled = input.time;
        }
        self.poll(input);
        self.advance();
        self.cols = self.wrap.then(|| view::code_cols(input.width, self.digits(), self.side_by_side));

        let title = match &self.repo.range {
            Some(r) => format!("diff · {}", r.label),
            None => "diff".to_owned(),
        };
        let mut out = Output::new(title, self.view(input)).wants_window();
        self.flush(&mut out);
        if std::mem::take(&mut self.back) {
            out.effects.push(Effect::Close);
        }
        if let Some(from) = &input.opened_from {
            out = out.action(Action::Back, if from == "git_graph" { "graph" } else { from.as_str() });
        }
        out = out
            .action(Action::Open, "refresh")
            .action(Action::Secondary, "next file")
            .action(Action::Move(Move::Down), "files");
        // The header's toggles, as keys of their own.
        for (name, key, label) in TOGGLES {
            out.actions.push(Offer { action: Action::Key(name.into()), label: label.into(), key: Some(key.into()) });
        }
        out
    }
}

impl DiffMod {
    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    fn handle(&mut self, input: &Input) {
        self.file_rows.handle(&input.events, FILES);
        self.line_rows.handle(&input.events, LINES);
        self.picker_rows.handle(&input.events, PICKER);
        // A double-click's second click comes with its Activated: it changes nothing more
        // (the first click opened or closed the folder).
        let clicked_rows: Vec<usize> = input.events.iter()
            .filter_map(|e| if let Event::Clicked { id } = e { folder_click(id) } else { None })
            .collect();
        let activated_rows: Vec<usize> = input.events.iter()
            .filter_map(|e| match e { Event::Activated { id, index } if id == FILES => Some(*index), _ => None })
            .collect();
        for event in &input.events {
            match event {
                Event::Clicked { id } => match id.strip_prefix("source/") {
                    Some("uncommitted") => self.set_source(Source::Uncommitted),
                    Some("unpushed") => self.set_source(Source::Unpushed),
                    Some("branch") => self.set_source(Source::Branch),
                    Some(hash) if is_commit_id(hash) => self.set_source(Source::Commit(hash.to_owned())),
                    _ if id == "refresh" => self.reload(true),
                    _ if id == "commits" => self.picking = !self.picking,
                    _ => match id.strip_prefix("fold/") {
                        Some(path) => self.fold(path),
                        // A click on a folder's row in the file list: it opens or closes
                        // (the arrows only move over it).
                        None => match folder_click(id) {
                            Some(i) if activated_rows.contains(&i) => {}
                            Some(i) => {
                                if let Some(path) = self.folder_at(i) {
                                    self.fold(&path);
                                }
                            }
                            None => self.toggle(id),
                        },
                    },
                },
                // Enter on a folder (a double-click is its first click's).
                Event::Activated { id, index } if id == FILES && self.folder_at(*index).is_some() => {
                    if let Some(path) = self.folder_at(*index)
                        && !clicked_rows.contains(index)
                    {
                        self.fold(&path);
                    }
                }
                Event::Selected { id, index } | Event::Activated { id, index } if id == FILES => self.pick_row(*index),
                Event::Selected { id, index } | Event::Activated { id, index } if id == PICKER => {
                    let hash = self.recent().get(*index).map(|c| c.0.clone());
                    self.picking = false;
                    if let Some(hash) = hash {
                        self.set_source(Source::Commit(hash));
                    }
                }
                Event::Dismissed { id } if id == PICKER_POPUP => self.picking = false,
                Event::Action(Action::Key(name)) => self.toggle(name),
                Event::Action(Action::Open) => self.reload(true),
                // Tab; Shift+Tab goes back.
                Event::Action(Action::Secondary) => self.step_file(if input.mods.shift { -1 } else { 1 }),
                Event::Action(Action::Move(Move::Right)) if !self.wrap => self.pan += PAN,
                Event::Action(Action::Move(Move::Left)) if !self.wrap => self.pan = self.pan.saturating_sub(PAN),
                Event::Action(Action::Move(Move::Home)) => self.pan = 0,
                // Opened over another module (the git graph's Space): back to it.
                Event::Action(Action::Back) if input.opened_from.is_some() => self.back = true,
                _ => {}
            }
        }
        // The commit list: the branch's last 40 commits, asked once it opens.
        if self.picking {
            self.need("recent", &["log", "-n40", "--format=%H%x00%cr%x00%s", "HEAD", "--"]);
        }
        // All files: the file whose rows are at the top is the selected one.
        let scrolled = input.events.iter().any(|e| matches!(e, Event::VisibleRange { id, .. } if id == LINES));
        if self.all_files && scrolled {
            let top = self.line_rows.first as usize;
            if let Some(file) = self.rows().get(top).and_then(Row::file)
                && let Some(f) = self.repo.files.get(file)
                && self.selected.as_deref() != Some(&f.path)
            {
                self.selected = Some(f.path.clone());
                self.reveal_selected();
            }
        }
    }

    /// Scrolls the file list to the selected file if it's out of sight (rows at the
    /// edges of the range may be only partly showing).
    fn reveal_selected(&mut self) {
        let Some(row) = self.selected_row() else { return };
        let (row, first, count) = (row as u64, self.file_rows.first, self.file_rows.count);
        if count > 0 && (row <= first || row + 2 >= first + count) {
            self.file_rows.scroll_to(row);
        }
    }

    /// A header toggle, by name (its pill's id, its key's action).
    fn toggle(&mut self, name: &str) {
        match name {
            "side-by-side" => self.side_by_side = !self.side_by_side,
            "wrap" => {
                self.wrap = !self.wrap;
                self.pan = 0;
            }
            "all-files" => {
                self.all_files = !self.all_files;
                // Keep the file shown: its bar on top.
                if let Some(i) = self.selected_file().and_then(|f| self.repo.files.iter().position(|g| g.path == f.path)) {
                    self.select(i);
                }
            }
            _ => {}
        }
    }

    /// Folds a folder of the file list, or unfolds it.
    fn fold(&mut self, path: &str) {
        match self.folded.iter().position(|p| p == path) {
            Some(i) => {
                self.folded.remove(i);
            }
            None => self.folded.push(path.to_owned()),
        }
        self.repo.shown = Default::default();
    }

    /// The file list's rows: the tree's nodes outside folded folders (indices into it).
    fn shown(&self) -> &[usize] {
        self.repo.shown.get_or_init(|| tree::visible(&self.repo.tree, &self.folded))
    }

    /// The folder at row `index` of the file list, if it's one.
    fn folder_at(&self, index: usize) -> Option<String> {
        let n = &self.repo.tree[*self.shown().get(index)?];
        (n.kind == tree::Kind::Dir).then(|| n.path.clone())
    }

    /// The branch's recent commits (the commit list): hash, when, subject.
    fn recent(&self) -> Vec<(String, String, String)> {
        let Some(r) = self.repo.got.get("recent") else { return Vec::new() };
        let Ok(out) = answer(r) else { return Vec::new() };
        let fields: Vec<&str> = out.split(['\0', '\n']).collect();
        fields.as_chunks::<3>().0.iter().filter(|f| is_commit_id(f[0].trim())).map(|f| (f[0].trim().to_owned(), f[1].to_owned(), f[2].to_owned())).collect()
    }

    /// Digits of the widest line number in the diffs shown (at least three).
    fn digits(&self) -> usize {
        let diffs: Box<dyn Iterator<Item = &FileDiff>> = if self.all_files {
            Box::new(self.repo.diffs.values().filter_map(|d| d.as_ref().ok()))
        } else {
            Box::new(self.selected.as_ref().and_then(|p| self.repo.diffs.get(p)?.as_ref().ok()).into_iter())
        };
        diffs.map(FileDiff::digits).max().unwrap_or(1).max(3)
    }

    fn set_source(&mut self, source: Source) {
        if self.source.as_ref() == Some(&source) {
            return;
        }
        self.source = Some(source);
        self.repo = Repo::default();
        self.selected = None;
        self.reload(true);
    }

    /// Row `index` of the file list chosen: a file shows; a folder hands on to the file
    /// next to it in the direction the selection moved.
    fn pick_row(&mut self, index: usize) {
        let tree: Vec<&tree::Node> = self.shown().iter().map(|&i| &self.repo.tree[i]).collect();
        let here = self.selected_row().unwrap_or(0);
        let file = |n: &&tree::Node| if let tree::Kind::File(i) = n.kind { Some(i) } else { None };
        let found = if index >= here {
            tree.iter().skip(index).find_map(file)
        } else {
            tree.iter().take(index + 1).rev().find_map(file).or_else(|| tree.iter().skip(index).find_map(file))
        };
        if let Some(i) = found {
            self.select(i);
        }
    }

    /// The selected file's row in the file list.
    fn selected_row(&self) -> Option<usize> {
        let path = self.selected.as_deref()?;
        let tree = &self.repo.tree;
        self.shown().iter().position(|&n| matches!(tree[n].kind, tree::Kind::File(i) if self.repo.files[i].path == path))
    }

    fn select(&mut self, file: usize) {
        let Some(f) = self.repo.files.get(file) else { return };
        if self.all_files {
            self.selected = Some(f.path.clone());
            let at = self.rows().iter().position(|r| *r == Row::File { file }).unwrap_or(0) as u64;
            self.line_rows.scroll_to(at);
            self.line_rows.first = at;
        } else if self.selected.as_deref() != Some(f.path.as_str()) {
            self.selected = Some(f.path.clone());
            self.line_rows.scroll_to(0);
            self.line_rows.first = 0;
        }
    }

    /// Selects the file `by` files on in tree order (wrapping), and scrolls the list to it.
    fn step_file(&mut self, by: isize) {
        let tree = &self.repo.tree;
        let files: Vec<(usize, usize)> = (self.shown().iter().enumerate())
            .filter_map(|(row, &n)| if let tree::Kind::File(i) = tree[n].kind { Some((row, i)) } else { None })
            .collect();
        if files.is_empty() {
            return;
        }
        let at = self.selected_row().and_then(|r| files.iter().position(|(row, _)| *row == r)).unwrap_or(0);
        let (_, i) = files[(at as isize + by).rem_euclid(files.len() as isize) as usize];
        self.select(i);
        self.reveal_selected();
    }

    /// The rows shown: the selected file's, or (all files) every file's after its name
    /// bar; built again only when what they depend on changes ([`RowKey`]).
    fn rows(&mut self) -> &[Row] {
        let one = if self.all_files { None } else { self.selected.clone() };
        let key: RowKey = (self.all_files, self.side_by_side, one.clone(), self.cols, self.repo.version);
        if self.repo.rows.as_ref().is_none_or(|(k, _)| *k != key) {
            let rows = self.build_rows(one.as_deref());
            self.repo.rows = Some((key, rows));
        }
        self.repo.rows.as_ref().map_or(&[], |(_, rows)| rows)
    }

    fn build_rows(&self, one: Option<&str>) -> Vec<Row> {
        let repo = &self.repo;
        // Files in the list's order (folded ones too).
        let order: Vec<usize> = repo.tree.iter().filter_map(|n| if let tree::Kind::File(i) = n.kind { Some(i) } else { None }).collect();
        let file_rows = |i: usize| -> Vec<Row> {
            let f = &repo.files[i];
            if f.status == '?' {
                return vec![Row::Notice { file: i, text: "untracked: git doesn't have its content, so it can't be shown here".into() }];
            }
            match repo.diffs.get(&f.path) {
                Some(Ok(diff)) => rows::rows(i, diff, self.side_by_side, self.cols),
                Some(Err(why)) => vec![Row::Notice { file: i, text: format!("can't read it: {why} (Enter reads it again)") }],
                None => vec![Row::Notice { file: i, text: "reading…".into() }],
            }
        };
        let mut out = Vec::new();
        match one {
            Some(path) => {
                let Some(i) = repo.files.iter().position(|f| f.path == path) else { return out };
                if !repo.diffs.contains_key(path) && repo.files[i].status != '?' {
                    return out;
                }
                out.extend(file_rows(i));
                let next = order.iter().position(|&o| o == i).is_some_and(|at| at + 1 < order.len());
                out.push(Row::End { file: i, next });
            }
            None => {
                for &i in &order {
                    out.push(Row::File { file: i });
                    out.extend(file_rows(i));
                    out.push(Row::End { file: i, next: false });
                }
                if !order.is_empty() {
                    out.push(Row::DiffEnd);
                }
            }
        }
        out
    }

    /// The syntax tokens of the lines of rows `range`, worked out for those not yet, at
    /// most [`highlight::BUDGET`] lines a call (the rest stay plain until a later one).
    fn highlight(&mut self, range: std::ops::Range<usize>) {
        // Each hunk's lines in these rows, in the order they show.
        let mut wanted: Vec<((usize, usize), std::ops::Range<usize>)> = Vec::new();
        for r in self.rows().get(range).unwrap_or_default() {
            let (key, lines) = match r {
                Row::Unified { file, hunk, cell, .. } => ((*file, *hunk), [Some(cell.line), None]),
                Row::Split { file, hunk, left, right, .. } => ((*file, *hunk), [left.map(|c| c.line), right.map(|c| c.line)]),
                _ => continue,
            };
            for line in lines.into_iter().flatten() {
                match wanted.last_mut() {
                    Some((k, span)) if *k == key => {
                        span.start = span.start.min(line);
                        span.end = span.end.max(line + 1);
                    }
                    _ => wanted.push((key, line..line + 1)),
                }
            }
        }
        let repo = &mut self.repo;
        let mut budget = highlight::BUDGET;
        for ((file, hunk), lines) in wanted {
            let path = &repo.files[file].path;
            let Some(Ok(diff)) = repo.diffs.get(path) else { continue };
            let Some(h) = diff.hunks.get(hunk) else { continue };
            let tokens = repo.tokens.entry((path.clone(), hunk)).or_insert_with(|| highlight::HunkTokens::new(path, h));
            if !tokens.has(lines.clone()) {
                budget = budget.saturating_sub(tokens.parse(h, lines, budget));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Reading
    // -----------------------------------------------------------------------

    /// Reads the source again; `force` also drops the file diffs already read (else they
    /// stay while the files list comes back the same).
    fn reload(&mut self, force: bool) {
        self.generation += 1;
        self.queue.clear();
        self.cap = None;
        let repo = &mut self.repo;
        repo.got.clear();
        repo.asked.clear();
        repo.next = None;
        repo.stage = Stage::Resolving;
        if force {
            repo.diffs.clear();
            repo.tokens.clear();
            repo.version += 1;
            repo.stamp.clear();
        }
    }

    /// While the window has focus, the uncommitted changes and the branch are read again
    /// every few seconds (a commit never changes).
    fn poll(&mut self, input: &Input) {
        let due = input.time - self.polled >= POLL_SECS || input.time < self.polled;
        let still = matches!(self.source, Some(Source::Commit(_)));
        if due && !still && input.focused && self.repo.stage == Stage::Ready && self.running.is_empty() {
            self.polled = input.time;
            self.reload(false);
        }
    }

    /// Takes in git's answers to this generation's runs.
    fn take_answers(&mut self, input: &Input) {
        let prefix = format!("{}/", self.generation);
        for r in input.data.tool_runs.iter().filter(|r| r.tool == "git") {
            let ours = self.running.remove(&r.id);
            let Some(name) = r.id.strip_prefix(&prefix) else { continue };
            // Refused, not run (too many at once): asked again later, never an answer.
            if refused(r) {
                self.repo.asked.remove(name);
                if ours {
                    self.cap = Some(self.running.len().max(1));
                }
                continue;
            }
            self.repo.got.insert(name.to_owned(), r.clone());
        }
    }

    /// Asks git `args` under `name`, once per generation; whether the answer is in.
    fn need(&mut self, name: &str, args: &[&str]) -> bool {
        if self.repo.asked.insert(name.to_owned()) {
            let id = format!("{}/{name}", self.generation);
            self.queue.push((id, args.iter().map(|a| (*a).to_owned()).collect()));
        }
        self.repo.got.contains_key(name)
    }

    /// Sends what's queued, as far as the host's limit allows.
    fn flush(&mut self, out: &mut Output) {
        while self.running.len() < self.cap.unwrap_or(RUNS_MAX) && !self.queue.is_empty() {
            let (id, args) = self.queue.remove(0);
            out.effects.push(run_tool("git", id.clone(), args.iter().map(String::as_str)));
            self.running.insert(id);
        }
    }

    /// Moves reading on as far as the answers allow.
    fn advance(&mut self) {
        if self.repo.stage == Stage::Resolving {
            match self.resolve() {
                None => {}
                Some(Err(why)) => self.repo.stage = Stage::Failed(why),
                Some(Ok((range, commit))) => {
                    self.repo.next = Some(range);
                    self.repo.commit = commit;
                    self.repo.stage = Stage::Listing;
                }
            }
        }
        if self.repo.stage == Stage::Listing
            && let Some(Err(why)) = self.list()
        {
            self.repo.stage = Stage::Failed(why);
        }
        if self.repo.stage == Stage::Ready {
            self.read_diffs();
        }
    }

    /// The source as a range, once git has answered: `None` while waiting.
    fn resolve(&mut self) -> Option<Result<(Range, Option<Commit>), String>> {
        let head_args = ["rev-parse", "--verify", "--quiet", "HEAD"];
        match self.source.clone()? {
            Source::Uncommitted => {
                if !self.need("head", &head_args) {
                    return None;
                }
                let head = &self.repo.got["head"];
                let base = match head.status {
                    Some(0) => head.stdout.trim().to_owned(),
                    // No commit yet: everything is new.
                    Some(1) => EMPTY_TREE.to_owned(),
                    _ => return Some(Err(failure(head))),
                };
                Some(Ok((Range { base, head: None, label: "uncommitted".into() }, None)))
            }
            Source::Unpushed => {
                let up = ["rev-parse", "--verify", "--quiet", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"];
                let asked = [
                    self.need("upstream", &up),
                    self.need("base", &["merge-base", "@{upstream}", "HEAD"]),
                    self.need("head", &head_args),
                ];
                if !asked.iter().all(|a| *a) {
                    return None;
                }
                let got = &self.repo.got;
                let upstream = match answer(&got["upstream"]) {
                    Ok(u) => u,
                    Err(e) if got["upstream"].status.is_none() || e.contains("not a git") => return Some(Err(e)),
                    Err(_) => return Some(Err("this branch has no upstream: nothing counts as unpushed".into())),
                };
                let Ok(base) = answer(&got["base"]) else {
                    return Some(Err(format!("{upstream} and HEAD share no history")));
                };
                let head = match answer(&got["head"]) {
                    Ok(h) => h,
                    Err(e) => return Some(Err(e)),
                };
                Some(Ok((Range { base, head: Some(head), label: format!("{upstream}..HEAD") }, None)))
            }
            Source::Branch => {
                let refs = ["refs/remotes/origin/HEAD", "refs/heads/main", "refs/heads/master"];
                let mut args = vec!["for-each-ref", "--format=%(refname)%09%(symref:short)"];
                args.extend(refs);
                let asked = [self.need("default", &args), self.need("head", &head_args)];
                if !asked.iter().all(|a| *a) {
                    return None;
                }
                let default = match answer(&self.repo.got["default"]) {
                    Ok(out) => match parse_default_branch(&out) {
                        Some(d) => d,
                        None => return Some(Err("can't tell the default branch (no origin/HEAD, main or master)".into())),
                    },
                    Err(e) => return Some(Err(e)),
                };
                if !self.need("base", &["merge-base", "--end-of-options", &default, "HEAD"]) {
                    return None;
                }
                let Ok(base) = answer(&self.repo.got["base"]) else {
                    return Some(Err(format!("{default} and HEAD share no history")));
                };
                let head = match answer(&self.repo.got["head"]) {
                    Ok(h) => h,
                    Err(e) => return Some(Err(e)),
                };
                Some(Ok((Range { base, head: Some(head), label: format!("{default}..HEAD") }, None)))
            }
            Source::Commit(hash) if !is_commit_id(&hash) => Some(Err("not a commit hash".into())),
            Source::Commit(hash) => {
                let args = ["log", "-n1", "--format=%H%x00%P%x00%an%x00%s", "--end-of-options", &hash, "--"];
                if !self.need("commit", &args) {
                    return None;
                }
                let r = &self.repo.got["commit"];
                let Some(commit) = parse_commit(&r.stdout).filter(|_| r.status == Some(0)) else {
                    let e = failure(r);
                    return Some(Err(if e.contains("not a git") { e } else { format!("no commit {hash} in this repo") }));
                };
                let base = commit.parent.clone().unwrap_or_else(|| EMPTY_TREE.to_owned());
                let range = Range { base, head: Some(commit.hash.clone()), label: short(&commit.hash).to_owned() };
                Some(Ok((range, Some(commit))))
            }
        }
    }

    /// Reads the files the range being read changes; once in, it becomes the range shown.
    fn list(&mut self) -> Option<Result<(), String>> {
        let range = self.repo.next.clone()?;
        let revs = range.revs();
        let diff = |what: &'static str| {
            let mut args = vec!["diff", what, "-z", "-M", "--end-of-options"];
            args.extend(revs.iter().map(String::as_str));
            args.push("--");
            args
        };
        let worktree = range.head.is_none();
        let count = (!worktree && range.base != EMPTY_TREE && !matches!(self.source, Some(Source::Commit(_))))
            .then(|| format!("{}..{}", range.base, range.head.as_deref().unwrap_or("HEAD")));
        let mut asked = vec![self.need("status", &diff("--name-status")), self.need("numstat", &diff("--numstat"))];
        if worktree {
            asked.push(self.need("untracked", &["ls-files", "--others", "--exclude-standard", "-z"]));
        }
        if let Some(c) = &count {
            asked.push(self.need("count", &["rev-list", "--count", c]));
        }
        if !asked.iter().all(|a| *a) {
            return None;
        }
        let got = &self.repo.got;
        let (status, numstat) = match (answer(&got["status"]), answer(&got["numstat"])) {
            (Ok(s), Ok(n)) => (s, n),
            (Err(e), _) | (_, Err(e)) => return Some(Err(e)),
        };
        let untracked = got.get("untracked").and_then(|r| answer(r).ok()).unwrap_or_default();
        let commits = got.get("count").and_then(|r| answer(r).ok()?.trim().parse().ok());
        let stamp = format!("{range:?}\n{status}\n{numstat}\n{untracked}");
        let repo = &mut self.repo;
        if stamp != repo.stamp {
            let mut files = parse_files(&status, &numstat);
            files.extend(parse_untracked(&untracked));
            repo.tree = tree::build(&files);
            repo.shown = Default::default();
            repo.files = files;
            repo.diffs.clear();
            repo.tokens.clear();
            repo.version += 1;
            repo.stamp = stamp;
        }
        repo.commits = if matches!(self.source, Some(Source::Commit(_))) { Some(1) } else { commits };
        repo.range = Some(range);
        repo.stage = Stage::Ready;
        // Keep the selection while its file is still there; else the first file.
        if !self.selected.as_ref().is_some_and(|p| self.repo.files.iter().any(|f| f.path == *p)) {
            self.selected = None;
            self.step_file(0);
        }
        Some(Ok(()))
    }

    /// Takes in file diffs read, and asks for those shown: the selected file's; all
    /// files, those whose rows are on screen or near it (a file not read yet is one
    /// "reading…" row), never all of them. Runs queued for files gone from sight are
    /// dropped.
    fn read_diffs(&mut self) {
        let names: Vec<String> = self.repo.got.keys().filter(|k| k.starts_with("diff/")).cloned().collect();
        for name in names {
            if let Some(r) = self.repo.got.remove(&name) {
                let path = name["diff/".len()..].to_owned();
                self.repo.tokens.retain(|(p, _), _| *p != path);
                self.repo.diffs.insert(path, answer(&r).map(|out| parse_diff(&out)));
                self.repo.version += 1;
            }
        }
        let mut wanted: Vec<usize> = self.selected_file().and_then(|f| self.repo.files.iter().position(|g| g.path == f.path)).into_iter().collect();
        if self.all_files {
            let first = self.line_rows.first as usize;
            let count = (self.line_rows.count as usize).max(40);
            let rows = self.rows();
            let mut files: Vec<usize> = Vec::new();
            for r in rows.iter().skip(first.saturating_sub(AHEAD_ROWS)).take(count + 2 * AHEAD_ROWS) {
                if let Some(f) = r.file()
                    && files.last() != Some(&f)
                {
                    files.push(f);
                }
            }
            wanted.extend(files);
        }
        let wanted: Vec<FileEntry> = wanted.into_iter().filter_map(|i| self.repo.files.get(i).cloned()).collect();
        let names: BTreeSet<String> = wanted.iter().map(|f| format!("{}/diff/{}", self.generation, f.path)).collect();
        let repo = &mut self.repo;
        self.queue.retain(|(id, _)| {
            let keep = !id.contains("/diff/") || names.contains(id);
            if !keep && let Some((_, name)) = id.split_once('/') {
                repo.asked.remove(name);
            }
            keep
        });
        for file in wanted {
            self.read_diff(&file);
        }
    }

    /// Asks for a file's diff, unless it's in, asked, or untracked.
    fn read_diff(&mut self, file: &FileEntry) {
        let name = format!("diff/{}", file.path);
        if file.status == '?' || self.repo.diffs.contains_key(&file.path) || self.repo.asked.contains(&name) {
            return;
        }
        let Some(range) = self.repo.range.clone() else { return };
        let specs: Vec<String> = file.old_path.iter().chain([&file.path]).map(|p| pathspec(p)).collect();
        let mut args = vec!["diff", "-U3", "--no-color", "-M", "--end-of-options"];
        let revs = range.revs();
        args.extend(revs.iter().map(String::as_str));
        args.push("--");
        args.extend(specs.iter().map(String::as_str));
        self.need(&name, &args);
    }

    fn selected_file(&self) -> Option<&FileEntry> {
        let path = self.selected.as_deref()?;
        self.repo.files.iter().find(|f| f.path == path)
    }
}

/// The host didn't run it (too many runs at once in this window): ask again.
fn refused(r: &ToolResult) -> bool {
    r.status.is_none() && r.stderr.starts_with("at most ")
}

/// A run's output, or why it failed.
fn answer(r: &ToolResult) -> Result<String, String> {
    if r.status == Some(0) { Ok(r.stdout.trim_end_matches('\n').to_owned()) } else { Err(failure(r)) }
}

/// Why a run failed, in git's words (its first line).
fn failure(r: &ToolResult) -> String {
    let first = r.stderr.lines().map(str::trim).find(|l| !l.is_empty());
    match (first, r.status) {
        (Some(line), _) => line.trim_start_matches("fatal: ").to_owned(),
        (None, Some(code)) => format!("git stopped with code {code}"),
        (None, None) => "git didn't run".to_owned(),
    }
}

export_frame!(DiffMod);
// What it needs: the tab's folder, and git to read it with.
manifest!(description: "what changed: uncommitted, unpushed, the branch, a commit", capabilities: ["window.folder", "git"]);

// A plain Rust crate too: `cargo test` runs these natively.
#[cfg(test)]
mod tests;
