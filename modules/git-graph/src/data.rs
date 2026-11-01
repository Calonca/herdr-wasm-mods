//! What git says, read into commits, refs and details: the arguments of each git run the
//! graph makes, and the parsers of their answers. Pure: no git runs here.

/// A commit as the log lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub parents: Vec<String>,
    pub refs: Vec<GitRef>,
    pub subject: String,
    pub author: String,
    /// Author date, Unix seconds.
    pub time: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitRef {
    /// Short name: `main`, `origin/main`, `v1.0`, `HEAD`.
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// A detached `HEAD`.
    Head,
    /// The checked-out branch.
    Current,
    Branch,
    Remote,
    Tag,
}

/// A commit's message and changed files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub hash: String,
    pub author: String,
    pub email: String,
    /// Author date, ISO 8601.
    pub date: String,
    /// The full message.
    pub message: String,
    /// Against the first parent.
    pub files: Vec<FileChange>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Lines added and removed; `None` for binary files.
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// What the cheap check says: the time now, and a stamp that changes whenever a ref or
/// HEAD moves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Poll {
    /// Unix seconds.
    pub now: Option<i64>,
    pub stamp: String,
}

/// Whether `s` is a full commit hash (SHA-1 or SHA-256), the only thing handed back to git.
pub fn is_hash(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The cheap check: `rev-parse` turns `--since=now` into `--max-age=<now>` (the module has
/// no clock), then prints HEAD, every ref's commit and the branch checked out.
pub fn poll_args() -> Vec<&'static str> {
    vec!["rev-parse", "--since=now", "HEAD", "--all", "--symbolic-full-name", "HEAD"]
}

pub fn parse_poll(out: &str) -> Poll {
    let mut poll = Poll::default();
    for line in out.lines() {
        match line.strip_prefix("--max-age=") {
            Some(t) => poll.now = t.parse().ok(),
            None => {
                poll.stamp.push_str(line);
                poll.stamp.push('\n');
            }
        }
    }
    poll
}

/// The newest `limit` commits of every ref, children before parents. Fields end in NUL,
/// which no commit field can hold, so a subject can't fake a field or a record; the
/// subject goes last all the same.
pub fn log_args(limit: usize) -> Vec<String> {
    let mut args: Vec<String> = ["log", "--all", "--topo-order", "--decorate=full", "--no-color"].map(String::from).into();
    args.push(format!("-n{limit}"));
    args.push("--format=%H%x00%P%x00%D%x00%an%x00%at%x00%s%x00".into());
    args
}

/// Records of `hash␀parents␀decorations␀author␀time␀subject␀`, one per line. A record
/// whose hash or parents aren't commit hashes is dropped: they're handed back to git.
pub fn parse_log(out: &str) -> Vec<Commit> {
    let fields: Vec<&str> = out.split('\0').collect();
    fields
        .as_chunks::<6>()
        .0
        .iter()
        .filter_map(|f| {
            let hash = f[0].trim_start_matches('\n');
            let parents: Vec<String> = f[1].split(' ').filter(|p| !p.is_empty()).map(str::to_owned).collect();
            if !is_hash(hash) || !parents.iter().all(|p| is_hash(p)) {
                return None;
            }
            Some(Commit {
                hash: hash.to_owned(),
                parents,
                refs: parse_refs(f[2]),
                author: f[3].to_owned(),
                time: f[4].parse().unwrap_or(0),
                subject: f[5].to_owned(),
            })
        })
        .collect()
}

/// `%D` with `--decorate=full`: `HEAD -> refs/heads/main, refs/remotes/origin/main,
/// tag: refs/tags/v1.0`. The checked-out branch first, remotes' `HEAD` left out.
fn parse_refs(decorations: &str) -> Vec<GitRef> {
    let mut refs: Vec<GitRef> = Vec::new();
    for part in decorations.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        let (current, part) = match part.strip_prefix("HEAD -> ") {
            Some(rest) => (true, rest),
            None => (false, part),
        };
        let r = if part == "HEAD" {
            GitRef { name: "HEAD".into(), kind: RefKind::Head }
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            GitRef { name: tag.trim_start_matches("refs/tags/").into(), kind: RefKind::Tag }
        } else if let Some(branch) = part.strip_prefix("refs/heads/") {
            GitRef { name: branch.into(), kind: if current { RefKind::Current } else { RefKind::Branch } }
        } else if let Some(remote) = part.strip_prefix("refs/remotes/") {
            if remote.ends_with("/HEAD") {
                continue;
            }
            GitRef { name: remote.into(), kind: RefKind::Remote }
        } else {
            GitRef { name: part.into(), kind: RefKind::Branch }
        };
        refs.push(r);
    }
    refs.sort_by_key(|r| r.kind != RefKind::Current);
    refs
}

/// One commit's message and its changed files against the first parent, in one run: the
/// message ends in NUL (which it can't hold), so no message can fake the file list.
pub fn details_args(hash: &str) -> Vec<&str> {
    vec![
        "show",
        "--no-color",
        "--numstat",
        "-z",
        "--diff-merges=first-parent",
        "--format=%H%x00%an%x00%ae%x00%aI%x00%B%x00",
        "--end-of-options",
        hash,
        "--",
    ]
}

/// `hash␀author␀email␀date␀message␀` then `--numstat -z` (`added␉removed␉path␀`, renames
/// `added␉removed␉␀old␀new␀`, `-` for binary files).
pub fn parse_details(out: &str) -> Option<Details> {
    let mut f = out.splitn(6, '\0');
    let mut d = Details {
        hash: f.next()?.to_owned(),
        author: f.next()?.to_owned(),
        email: f.next()?.to_owned(),
        date: f.next()?.to_owned(),
        message: f.next()?.trim_end().to_owned(),
        files: Vec::new(),
    };
    if !is_hash(&d.hash) {
        return None;
    }
    let mut it = f.next().unwrap_or_default().split('\0');
    while let Some(entry) = it.next() {
        let mut cols = entry.trim_start_matches('\n').splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (cols.next(), cols.next(), cols.next()) else { continue };
        // A rename: the path is empty and the old and new names follow.
        let path = if path.is_empty() {
            let _old = it.next();
            it.next().unwrap_or_default()
        } else {
            path
        };
        d.files.push(FileChange { path: path.to_owned(), added: added.parse().ok(), removed: removed.parse().ok() });
    }
    Some(d)
}

/// `now`, `5m`, `3h`, `2d`, `3w`, `4mo`, `2y`.
pub fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        86_400..1_209_600 => format!("{}d", s / 86_400),
        1_209_600..5_184_000 => format!("{}w", s / 604_800),
        5_184_000..31_536_000 => format!("{}mo", s / 2_592_000),
        _ => format!("{}y", s / 31_536_000),
    }
}
