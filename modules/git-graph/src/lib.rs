//! git graph: the history of the tab's repo as a commit graph. Each commit is a row: its
//! lanes, hash, its refs as pills, its subject, author and age. Enter (or clicking the
//! selected commit again) shows its details beside the list (below it in a narrow
//! window): the whole message, who and when, the files it changed; Esc hides them.
//! Space opens its diff in a new tab. Read-only: git runs through the host, which only
//! lets it read.
//!
//! The log is read once the user allowed the tab's folder, then again only when a ref or
//! HEAD moves: a cheap `git rev-parse` checks, at most every few seconds, on the calls
//! the module gets anyway (its own events, git's answers). The module has no timer, so a
//! commit made while nothing happens in the window shows on the next click, key or
//! "refresh". 500 commits come at first; scrolling near the end reads 500 more.
//!
//! Whether the details show is kept in `~/.local/share/wasm-mods/git-graph/settings.json`
//! (capabilities `fs.read`/`fs.write` on that folder) for the next graph opened. The diff
//! it opens keeps its own toggles the same way.
//!
//! Build:   cargo build --release --target wasm32-unknown-unknown   (in modules/)
//! Install: ../scripts/install.sh git-graph

use wasm_mods_sdk::*;

mod data;
mod lanes;
mod view;

use data::{Commit, Details, RefKind};

/// The commit list's id: its events (a row chosen, opened, scrolled) carry it.
const LIST: &str = "commits";
/// Commits read at first; scrolling near the end reads this many more.
const PAGE: usize = 500;
/// How many commits may be read on opening to find the checked-out one.
const MAX_SEEK: usize = 10 * PAGE;
/// Seconds between checks for moved refs.
const POLL_EVERY: f64 = 3.0;
/// Rows built either side of those showing.
const MARGIN: u64 = 10;
/// Rows showing before the host first says which: about a tall window's worth.
const FIRST_ROWS: u64 = 50;
/// Read more history when the rows showing come this close to the end of it.
const MORE_WITHIN: u64 = 50;
/// Commit details kept, most recent first.
const DETAILS_KEPT: usize = 32;
/// Where the settings are kept.
const SETTINGS_DIR: &str = "~/.local/share/wasm-mods/git-graph";
const SETTINGS_FILE: &str = "~/.local/share/wasm-mods/git-graph/settings.json";

/// The settings, as kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    details: bool,
}

/// Where the kept settings are at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Kept {
    /// Not asked for yet: the next call does.
    #[default]
    Unread,
    Reading,
    /// Read (or none kept yet), or last written: a change is written. (A write that
    /// fails, the folder not allowed, isn't tried again until the next change.)
    Read(Settings),
}

/// The module's state. Only the selection and the list's scroll survive a reload of a
/// new build; the history is read again.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GitGraph {
    /// The selected commit's full hash.
    #[serde(default)]
    selected: Option<String>,
    /// The rows of the list showing, as the host last said.
    #[serde(default = "first_rows")]
    rows: VirtualRows,
    /// The selected commit's details show.
    #[serde(default)]
    details: bool,
    #[serde(skip)]
    repo: Repo,
    /// git runs asked for so far, for their ids.
    #[serde(skip)]
    runs: u64,
    /// The kept settings, and whether the user changed one before they were read
    /// (theirs win).
    #[serde(skip)]
    kept: Kept,
    #[serde(skip)]
    changed: bool,
}

impl Default for GitGraph {
    fn default() -> Self {
        GitGraph { selected: None, rows: first_rows(), details: false, repo: Repo::default(), runs: 0, kept: Kept::Unread, changed: false }
    }
}

fn first_rows() -> VirtualRows {
    let mut rows = VirtualRows::default();
    rows.count = FIRST_ROWS;
    rows
}

/// One folder's history as read, and the git runs on their way.
#[derive(Default)]
struct Repo {
    folder: String,
    /// How many commits the log reads.
    limit: usize,
    /// Newest first, children before parents.
    commits: Vec<Commit>,
    /// One per commit.
    graph: Vec<lanes::Row>,
    /// The widest row's lanes (at most [`view::MAX_LANES`]).
    lanes: usize,
    /// The log was read at least once.
    loaded: bool,
    /// What the last check said (refs, HEAD, the branch); `None` until one answered.
    stamp: Option<String>,
    /// The stamp the log shown was read at; `None` reads it again.
    read_at: Option<String>,
    /// Unix seconds at `Input::time`, from the last check.
    clock: Option<(i64, f64)>,
    /// The check on its way (its id), and when the last one was asked for.
    poll: Option<String>,
    polled: Option<f64>,
    /// The log read on its way: its id and the stamp it was asked at.
    log: Option<(String, String)>,
    /// Details read, by hash, most recent first.
    details: Vec<(String, Result<Details, String>)>,
    /// The details read on its way (its id).
    reading: Option<String>,
    error: Option<String>,
    /// Opening: select the checked-out commit once it's read (on a busy repo it can be
    /// far down the list, under other people's newer branches).
    seek_head: bool,
}

impl Repo {
    fn new(folder: &str) -> Self {
        Repo { folder: folder.into(), limit: PAGE, lanes: 1, seek_head: true, ..Default::default() }
    }

    /// Unix seconds now, once a check said.
    fn now(&self, time: f64) -> Option<i64> {
        self.clock.map(|(at, then)| at + (time - then) as i64)
    }

    fn details(&self, hash: &str) -> Option<&Result<Details, String>> {
        self.details.iter().find(|(h, _)| h == hash).map(|(_, d)| d)
    }
}

/// git's first line of complaint.
fn complaint(r: &ToolResult) -> String {
    let line = r.stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed");
    line.trim_start_matches("fatal: ").to_owned()
}

/// The checked-out commit's row (its branch, or a detached `HEAD`).
fn head_row(commits: &[Commit]) -> Option<usize> {
    commits.iter().position(|c| c.refs.iter().any(|r| matches!(r.kind, RefKind::Current | RefKind::Head)))
}

impl GitGraph {
    /// Takes in the kept settings, once read: they're the graph's, unless the user
    /// changed one meanwhile.
    fn take_settings(&mut self, input: &Input) {
        let Some(reply) = input.fs_result("settings") else { return };
        let read = match reply {
            FsReply::Bytes { data, .. } => serde_json::from_slice::<Settings>(input.blob(*data)).ok(),
            _ => None,
        };
        if let Some(kept) = read
            && !self.changed
        {
            self.details = kept.details;
        }
        self.kept = Kept::Read(read.unwrap_or(Settings { details: self.details }));
    }

    /// Asks for the kept settings the first time; writes them when they change.
    fn keep_settings(&mut self, out: &mut Output) {
        let now = Settings { details: self.details };
        match self.kept {
            Kept::Unread => {
                out.effects.push(fs::read_file("settings", SETTINGS_FILE));
                self.kept = Kept::Reading;
            }
            Kept::Read(kept) if kept != now => {
                let json = serde_json::to_vec(&now).unwrap_or_default();
                out.effects.push(fs::create_dir("settings-dir", SETTINGS_DIR));
                fs::write(out, "settings-save", SETTINGS_FILE, &json);
                self.kept = Kept::Read(now);
            }
            _ => {}
        }
    }

    fn selected_row(&self) -> Option<usize> {
        let hash = self.selected.as_ref()?;
        self.repo.commits.iter().position(|c| c.hash == *hash)
    }

    fn select(&mut self, index: usize) {
        if let Some(c) = self.repo.commits.get(index) {
            self.selected = Some(c.hash.clone());
        }
    }

    fn id(&mut self, what: &str) -> String {
        self.runs += 1;
        format!("{what} {}", self.runs)
    }

    /// Takes in git's answers.
    fn answers(&mut self, input: &Input) {
        let branch = input.window.as_ref().and_then(|w| w.branch.clone()).unwrap_or_default();
        for r in input.data.tool_runs.iter().filter(|r| r.tool == "git") {
            let repo = &mut self.repo;
            if repo.poll.as_ref() == Some(&r.id) {
                repo.poll = None;
                if r.ok() {
                    let poll = data::parse_poll(&r.stdout);
                    repo.clock = poll.now.map(|now| (now, input.time)).or(repo.clock);
                    repo.stamp = Some(format!("{}{branch}", poll.stamp));
                } else {
                    // Read again once it answers.
                    repo.error = Some(complaint(r));
                    (repo.stamp, repo.read_at) = (None, None);
                }
            } else if repo.log.as_ref().is_some_and(|(id, _)| *id == r.id) {
                let (_, stamp) = repo.log.take().unwrap_or_default();
                repo.read_at = Some(stamp);
                if r.ok() {
                    repo.error = None;
                    self.set_commits(data::parse_log(&r.stdout));
                } else {
                    repo.error = Some(complaint(r));
                }
            } else if repo.reading.as_ref() == Some(&r.id) {
                repo.reading = None;
                let hash = r.id.trim_start_matches("show ").to_owned();
                let details = if r.ok() {
                    data::parse_details(&r.stdout).ok_or_else(|| "git's answer didn't read as a commit".to_owned())
                } else {
                    Err(complaint(r))
                };
                repo.details.insert(0, (hash, details));
                repo.details.truncate(DETAILS_KEPT);
            }
        }
    }

    /// New history: lay the graph out again, keeping the selected commit selected (on
    /// opening, selecting the checked-out one).
    fn set_commits(&mut self, commits: Vec<Commit>) {
        let repo = &mut self.repo;
        let hashes: Vec<&str> = commits.iter().map(|c| c.hash.as_str()).collect();
        let parents: Vec<Vec<&str>> = commits.iter().map(|c| c.parents.iter().map(String::as_str).collect()).collect();
        repo.graph = lanes::layout(&hashes, &parents);
        repo.lanes = repo.graph.iter().map(|r| r.width).max().unwrap_or(1).clamp(1, view::MAX_LANES);
        repo.loaded = true;
        if repo.seek_head {
            match head_row(&commits) {
                Some(i) => {
                    self.selected = Some(commits[i].hash.clone());
                    self.rows.scroll_to(i.saturating_sub(5) as u64);
                    repo.seek_head = false;
                }
                // Not among the commits read: read more, up to a point.
                None if commits.len() >= repo.limit && repo.limit < MAX_SEEK => {
                    repo.limit += PAGE;
                    repo.read_at = None;
                }
                None => repo.seek_head = false,
            }
        }
        if !self.selected.as_ref().is_some_and(|h| commits.iter().any(|c| c.hash == *h)) {
            self.selected = commits.first().map(|c| c.hash.clone());
        }
        repo.commits = commits;
    }

    /// Takes in the user's events; returns whether they asked for the selected commit's diff.
    fn events(&mut self, input: &Input) -> bool {
        self.rows.handle(&input.events, LIST);
        let mut diff = false;
        // A double-click is a click on the selected row and an activation: one toggle.
        let mut toggled = false;
        for event in &input.events {
            match event {
                Event::Selected { id, index } if id == LIST => {
                    if self.selected_row() == Some(*index) {
                        self.details = !self.details;
                        toggled = true;
                    }
                    self.select(*index);
                }
                Event::Activated { id, index } if id == LIST => {
                    self.select(*index);
                    if !toggled {
                        self.details = !self.details;
                    }
                }
                Event::Action(Action::Open) => self.details = !self.details,
                Event::Action(Action::Back) => self.details = false,
                Event::Action(Action::Primary) => diff = true,
                Event::Action(Action::Secondary) => self.refresh(),
                _ => {}
            }
        }
        diff
    }

    /// Checks now, and reads the log again even if nothing moved.
    fn refresh(&mut self) {
        self.repo.polled = None;
        self.repo.read_at = None;
    }

    /// Near the end of the history read: read more.
    fn more(&mut self) {
        let repo = &mut self.repo;
        let n = repo.commits.len();
        if repo.loaded && repo.log.is_none() && n >= repo.limit && self.rows.first + self.rows.count + MORE_WITHIN >= n as u64 {
            repo.limit += PAGE;
            repo.read_at = None;
        }
    }

    /// Asks git for what's due: a check, the log, the selected commit's details.
    fn ask(&mut self, time: f64, effects: &mut Vec<Effect>) {
        if self.repo.poll.is_none() && self.repo.polled.is_none_or(|t| time - t >= POLL_EVERY) {
            let id = self.id("poll");
            effects.push(run_tool("git", &id, data::poll_args()));
            self.repo.poll = Some(id);
            self.repo.polled = Some(time);
        }
        if self.repo.log.is_none()
            && let Some(stamp) = self.repo.stamp.clone()
            && self.repo.read_at.as_ref() != Some(&stamp)
        {
            let id = self.id("log");
            let args = data::log_args(self.repo.limit);
            effects.push(run_tool("git", &id, args.iter().map(String::as_str)));
            self.repo.log = Some((id, stamp));
        }
        if self.repo.reading.is_none()
            && let Some(hash) = self.selected.clone()
            && data::is_hash(&hash)
            && self.repo.details(&hash).is_none()
        {
            let id = format!("show {hash}");
            effects.push(run_tool("git", &id, data::details_args(&hash)));
            self.repo.reading = Some(id);
        }
    }
}

impl Frame for GitGraph {
    fn update(&mut self, input: &Input) -> Output {
        self.take_settings(input);
        let Some(window) = &input.window else {
            let ui = column([heading("git graph"), muted("waiting for the folder: allow this tab's folder to see its history")]);
            let mut out = Output::new("git graph", ui).wants_window();
            self.keep_settings(&mut out);
            return out;
        };
        if self.repo.folder != window.folder {
            self.repo = Repo::new(&window.folder);
        }
        self.answers(input);
        let mut effects = Vec::new();
        // Only in answer to the user's click or key: the host refuses it otherwise.
        let details = self.details;
        let diff = self.events(input);
        self.changed |= self.details != details;
        if diff
            && let Some(hash) = self.selected.clone()
        {
            effects.push(Effect::OpenDiff { commit: hash });
        }
        self.more();
        self.ask(input.time, &mut effects);
        let mut out = Output::new("git graph", self.view(input)).wants_window();
        out.effects = effects;
        self.keep_settings(&mut out);
        if self.details {
            out = out.action(Action::Back, "close details");
        }
        if self.selected.is_some() {
            out = out
                .action(Action::Open, if self.details { "hide details" } else { "details" })
                .action(Action::Primary, "diff")
                .action(Action::Move(Move::Down), "move");
        }
        out.action(Action::Secondary, "refresh")
    }
}

export_frame!(GitGraph);
manifest!(
    description: "the repo's history as a commit graph: branches, tags, each commit's files; opens a commit's diff",
    capabilities: ["window.folder", "git", "fs.read:~/.local/share/wasm-mods/git-graph", "fs.write:~/.local/share/wasm-mods/git-graph"]
);

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccccccccccc";
    const D: &str = "dddddddddddddddddddddddddddddddddddddddd";

    /// A merge `A` of `B` (main) and `C` (a side branch), both from `D`; then a record
    /// with an option for a hash, which is dropped.
    fn log() -> String {
        format!(
            "{A}\0{B} {C}\0HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, tag: refs/tags/v1.0\0Ada\01700000000\0merge side\0\n\
             {C}\0{D}\0refs/heads/side\0Bo\01690000200\0side: a\x1fb\0\n\
             {B}\0{D}\0\0Ada\01690000100\0fix\0\n\
             {D}\0\0\0Ada\01690000000\0first\0\n\
             --output=/tmp/x\0{D}\0\0Ada\01\0s\0\n"
        )
    }

    fn git(id: &str, stdout: String) -> ToolResult {
        ToolResult { tool: "git".into(), id: id.into(), status: Some(0), stdout, stderr: String::new() }
    }

    /// The id of the git run whose arguments start with `sub`.
    fn run_id(out: &Output, sub: &str) -> String {
        out.effects
            .iter()
            .find_map(|e| match e {
                Effect::RunTool { id, args, .. } if args[0] == sub => Some(id.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no git {sub} in {:?}", out.effects))
    }

    #[test]
    fn parses_the_log_into_commits_and_lanes() {
        let commits = data::parse_log(&log());
        assert_eq!(commits.iter().map(|c| c.hash.as_str()).collect::<Vec<_>>(), [A, C, B, D]);
        assert_eq!(commits[0].parents, [B, C]);
        let refs: Vec<(&str, RefKind)> = commits[0].refs.iter().map(|r| (r.name.as_str(), r.kind)).collect();
        assert_eq!(refs, [("main", RefKind::Current), ("origin/main", RefKind::Remote), ("v1.0", RefKind::Tag)]);
        assert_eq!(commits[1].subject, "side: a\x1fb");

        let hashes: Vec<&str> = commits.iter().map(|c| c.hash.as_str()).collect();
        let parents: Vec<Vec<&str>> = commits.iter().map(|c| c.parents.iter().map(String::as_str).collect()).collect();
        let rows = lanes::layout(&hashes, &parents);
        // The merge opens a second lane for C; B stays in the first; D closes both.
        assert_eq!(rows.iter().map(|r| r.column).collect::<Vec<_>>(), [0, 1, 0, 0]);
        assert_eq!(rows.iter().map(|r| r.width).collect::<Vec<_>>(), [2, 2, 2, 2]);
        let into_d = rows[3].lines.iter().filter(|l| l.to == (0, lanes::At::Dot)).count();
        assert_eq!(into_d, 2);

        let show = format!("{A}\0Ada\0ada@example.com\02026-10-18T10:00:00+02:00\0merge side\n\nbody\n3\t1\tfake.rs\n\0\0\n3\t1\tsrc/a.rs\0-\t-\tlogo.png\02\t0\t\0old.rs\0new.rs\0");
        let d = data::parse_details(&show).unwrap();
        assert_eq!(d.message, "merge side\n\nbody\n3\t1\tfake.rs");
        let files: Vec<(&str, Option<u32>)> = d.files.iter().map(|f| (f.path.as_str(), f.added)).collect();
        assert_eq!(files, [("src/a.rs", Some(3)), ("logo.png", None), ("new.rs", Some(2))]);
    }

    #[test]
    fn space_on_a_commit_opens_its_diff() {
        let mut g = GitGraph::default();
        let window = Some(WindowContext { folder: "/src/recipes".into(), branch: Some("main".into()), machine: None });
        let call = |g: &mut GitGraph, events: Vec<Event>, runs: Vec<ToolResult>| {
            let data = HostData { tool_runs: runs, ..Default::default() };
            g.update(&Input { time: 1.0, events, data, window: window.clone(), ..Default::default() })
        };
        let out = call(&mut g, vec![], vec![]);
        assert!(out.wants.window);
        let poll = run_id(&out, "rev-parse");
        // The check answers: the log is read, once.
        let out = call(&mut g, vec![], vec![git(&poll, format!("--max-age=1700000600\n{A}\n{A}\n{C}\nrefs/heads/main\n"))]);
        let log_id = run_id(&out, "log");
        let out = call(&mut g, vec![], vec![git(&log_id, log())]);
        assert!(!out.effects.iter().any(|e| matches!(e, Effect::RunTool { args, .. } if args[0] == "log")));
        // The checked-out commit is selected and its details asked for.
        assert_eq!(g.selected.as_deref(), Some(A));
        assert!(matches!(&out.effects[..], [Effect::RunTool { args, .. }] if args[0] == "show"));
        // A double-click on the third row shows its details, Space opens its diff.
        let out = call(&mut g, vec![Event::Selected { id: LIST.into(), index: 2 }, Event::Activated { id: LIST.into(), index: 2 }], vec![]);
        assert!(g.details && !out.effects.iter().any(|e| matches!(e, Effect::OpenDiff { .. })));
        let out = call(&mut g, vec![Event::Action(Action::Primary)], vec![]);
        assert!(out.effects.contains(&Effect::OpenDiff { commit: B.into() }), "{:?}", out.effects);
    }
}
