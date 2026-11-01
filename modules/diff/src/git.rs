//! What the diff viewer reads from git, and how it reads git's answers: the sources (which
//! changes), the changed files (`--name-status` joined with `--numstat`), untracked files,
//! the default branch, one commit's header, and a file's `git diff -U3` as hunks and lines.
//! Pure parsing: the module sends the commands and hands the output here.

/// git's empty tree: the "before" of a root commit, or of a repo with no commit yet.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Which changes to show.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    /// The working tree (staged or not) against `HEAD`, with untracked files listed.
    Uncommitted,
    /// Commits not pushed yet: from where `@{upstream}` and `HEAD` meet to `HEAD`.
    Unpushed,
    /// Every commit of the branch: from where it left the default branch to `HEAD`.
    Branch,
    /// One commit against its first parent.
    Commit(String),
}

impl Source {
    /// The pill that picks it.
    pub fn name(&self) -> String {
        match self {
            Source::Uncommitted => "uncommitted".into(),
            Source::Unpushed => "unpushed".into(),
            Source::Branch => "branch".into(),
            Source::Commit(hash) => format!("commit {}", short(hash)),
        }
    }
}

/// A commit hash, full or abbreviated: only those reach git's command line, where
/// anything else could read as an option or a range.
pub fn is_commit_id(s: &str) -> bool {
    (4..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn short(hash: &str) -> &str {
    &hash[..hash.len().min(7)]
}

/// A source resolved: `base` against `head` (`None`: the working tree).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub base: String,
    pub head: Option<String>,
    /// How it reads: `origin/main..HEAD`, `a1b2c3d`.
    pub label: String,
}

impl Range {
    /// `base [head]`, as `git diff` takes them.
    pub fn revs(&self) -> Vec<String> {
        let mut revs = vec![self.base.clone()];
        revs.extend(self.head.clone());
        revs
    }
}

/// The one commit shown ([`Source::Commit`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    /// The first parent; `None` for a root commit.
    pub parent: Option<String>,
    pub author: String,
    pub subject: String,
}

/// `log -n1 --format=%H%x00%P%x00%an%x00%s`'s answer.
pub fn parse_commit(out: &str) -> Option<Commit> {
    let mut f = out.trim_end_matches('\n').splitn(4, '\0');
    let hash = f.next()?.trim().to_owned();
    let parent = f.next()?.split_whitespace().next().map(str::to_owned);
    let (author, subject) = (f.next()?.to_owned(), f.next().unwrap_or_default().to_owned());
    is_commit_id(&hash).then_some(Commit { hash, parent, author, subject })
}

/// The default branch from `for-each-ref --format=%(refname)%09%(symref:short)
/// refs/remotes/origin/HEAD refs/heads/main refs/heads/master`: where `origin/HEAD`
/// points, else `main`, else `master`.
pub fn parse_default_branch(out: &str) -> Option<String> {
    let refs: Vec<(&str, &str)> = out.lines().filter_map(|l| l.split_once('\t')).collect();
    let find = |name: &str| refs.iter().find(|(r, _)| *r == name);
    if let Some((_, target)) = find("refs/remotes/origin/HEAD")
        && !target.is_empty()
    {
        return Some((*target).to_owned());
    }
    ["main", "master"].into_iter().find(|b| find(&format!("refs/heads/{b}")).is_some()).map(Into::into)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    /// Where a renamed (or copied) file came from.
    pub old_path: Option<String>,
    /// `A`, `M`, `D`, `R`, `C`, `T`; `?` for an untracked file.
    pub status: char,
    /// `None` for binary files (and untracked ones: never read).
    pub added: Option<u32>,
    pub removed: Option<u32>,
}

/// `--name-status -z` (`M␀path␀`, `R100␀old␀new␀`) joined with `--numstat -z`
/// (`added␉removed␉path␀`, renames `added␉removed␉␀old␀new␀`, `-` for binary).
pub fn parse_files(status: &str, numstat: &str) -> Vec<FileEntry> {
    let mut files = Vec::new();
    let mut it = status.split('\0').filter(|s| !s.is_empty());
    while let Some(code) = it.next() {
        let kind = code.chars().next().unwrap_or('M');
        let Some(first) = it.next() else { break };
        let (old_path, path) = if matches!(kind, 'R' | 'C') {
            (Some(first.to_owned()), it.next().unwrap_or(first).to_owned())
        } else {
            (None, first.to_owned())
        };
        files.push(FileEntry { path, old_path, status: kind, added: None, removed: None });
    }
    let index: std::collections::HashMap<String, usize> = files.iter().enumerate().map(|(i, f)| (f.path.clone(), i)).collect();
    let mut it = numstat.split('\0');
    while let Some(entry) = it.next() {
        let mut cols = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (cols.next(), cols.next(), cols.next()) else { continue };
        // A rename: the path is empty and the old and new names follow.
        let path = if path.is_empty() {
            let _old = it.next();
            it.next().unwrap_or_default()
        } else {
            path
        };
        if let Some(f) = index.get(path).map(|&i| &mut files[i]) {
            f.added = added.parse().ok();
            f.removed = removed.parse().ok();
        }
    }
    files
}

/// `ls-files --others --exclude-standard -z`: untracked files.
pub fn parse_untracked(out: &str) -> Vec<FileEntry> {
    out.split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| FileEntry { path: p.to_owned(), status: '?', ..Default::default() })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hunk {
    /// `-12,5 +12,7`.
    pub ranges: String,
    /// What git shows after the `@@ … @@`, usually the enclosing function.
    pub context: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub hunks: Vec<Hunk>,
    pub binary: bool,
}

impl FileDiff {
    /// The widest line number, in digits (for the number columns).
    pub fn digits(&self) -> usize {
        let most = self.hunks.iter().flat_map(|h| &h.lines).flat_map(|l| [l.old, l.new]).flatten().max().unwrap_or(1);
        most.to_string().len()
    }
}

/// One file's `git diff -U3` output.
pub fn parse_diff(out: &str) -> FileDiff {
    let mut diff = FileDiff::default();
    let (mut old, mut new) = (0, 0);
    for line in out.lines() {
        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            diff.binary = true;
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            // `@@ -12,5 +12,7 @@ fn context`
            let (ranges, context) = rest.split_once(" @@").unwrap_or((rest, ""));
            let mut parts = ranges.split_whitespace();
            let start = |p: Option<&str>| -> u32 { p.and_then(|p| p.get(1..)?.split(',').next()?.parse().ok()).unwrap_or(1) };
            old = start(parts.next());
            new = start(parts.next());
            diff.hunks.push(Hunk { ranges: ranges.to_owned(), context: context.trim().to_owned(), lines: Vec::new() });
        } else if let Some(hunk) = diff.hunks.last_mut() {
            let (kind, text) = match line.chars().next() {
                Some('+') => (LineKind::Added, &line[1..]),
                Some('-') => (LineKind::Removed, &line[1..]),
                Some(' ') => (LineKind::Context, &line[1..]),
                // `\ No newline at end of file`
                _ => continue,
            };
            let (o, n) = match kind {
                LineKind::Context => (Some(old), Some(new)),
                LineKind::Added => (None, Some(new)),
                LineKind::Removed => (Some(old), None),
            };
            old += u32::from(o.is_some());
            new += u32::from(n.is_some());
            hunk.lines.push(DiffLine { kind, old: o, new: n, text: text.replace('\t', "    ") });
        }
    }
    diff
}

/// A path as git printed it: from the top of the checkout, taken literally (the folder
/// may be one inside it, a name may hold `*`).
pub fn pathspec(path: &str) -> String {
    format!(":(top,literal){path}")
}
