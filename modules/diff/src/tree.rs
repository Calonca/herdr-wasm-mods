//! The changed files as a folder tree, like an editor's: folders first, then files, by
//! name; a folder holding only one folder is shown as one (`src/ui`); each folder sums
//! its files' added and removed lines.

use std::collections::BTreeMap;

use crate::git::FileEntry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    /// Index into the range's files.
    File(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    /// Full path (folders without a trailing `/`): a folder's key for folding it.
    pub path: String,
    pub depth: usize,
    pub kind: Kind,
    pub added: u32,
    pub removed: u32,
}

#[derive(Default)]
struct Dir {
    dirs: BTreeMap<String, Dir>,
    files: Vec<(String, usize)>,
}

/// The tree in display order (depth-first).
pub fn build(files: &[FileEntry]) -> Vec<Node> {
    let mut root = Dir::default();
    for (i, f) in files.iter().enumerate() {
        let mut parts: Vec<&str> = f.path.split('/').collect();
        let name = parts.pop().unwrap_or_default();
        let mut dir = &mut root;
        for part in parts {
            dir = dir.dirs.entry(part.to_owned()).or_default();
        }
        dir.files.push((name.to_owned(), i));
    }
    let mut out = Vec::new();
    flatten(&root, "", 0, files, &mut out);
    out
}

/// The nodes shown with these folders folded, by their index in `nodes`.
pub fn visible(nodes: &[Node], folded: &[String]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut under: Option<&str> = None;
    for (i, n) in nodes.iter().enumerate() {
        if let Some(dir) = under {
            if n.path.starts_with(dir) && n.path[dir.len()..].starts_with('/') {
                continue;
            }
            under = None;
        }
        if n.kind == Kind::Dir && folded.contains(&n.path) {
            under = Some(&n.path);
        }
        out.push(i);
    }
    out
}

/// Appends `dir`'s children; returns its line totals.
fn flatten(dir: &Dir, prefix: &str, depth: usize, files: &[FileEntry], out: &mut Vec<Node>) -> (u32, u32) {
    let mut total = (0, 0);
    for (name, mut child) in &dir.dirs {
        // Fold chains of single folders into one row.
        let mut name = name.clone();
        while child.files.is_empty() && child.dirs.len() == 1 {
            let Some((next, grandchild)) = child.dirs.iter().next() else { break };
            name = format!("{name}/{next}");
            child = grandchild;
        }
        let path = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        let at = out.len();
        out.push(Node { name, path: path.clone(), depth, kind: Kind::Dir, added: 0, removed: 0 });
        let (a, r) = flatten(child, &path, depth + 1, files, out);
        out[at].added = a;
        out[at].removed = r;
        total = (total.0 + a, total.1 + r);
    }
    let mut sorted = dir.files.clone();
    sorted.sort();
    for (name, i) in sorted {
        let (a, r) = (files[i].added.unwrap_or(0), files[i].removed.unwrap_or(0));
        total = (total.0 + a, total.1 + r);
        out.push(Node { name, path: files[i].path.clone(), depth, kind: Kind::File(i), added: a, removed: r });
    }
    total
}
