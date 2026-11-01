//! Diffs as display rows: unified (removed and added lines interleaved, as git prints
//! them) or side by side (old left, new right, aligned, with blank filler). Runs of
//! removed lines followed by added lines are paired one to one, and a pair gets the
//! changed part of each line marked (the words that differ), when most of it is shared.
//! Wrapping, a long line takes a row per piece ([`wrap`]); each file ends with a quiet
//! marker, and the all-files view with a bolder one.

use crate::git::{FileDiff, LineKind};

/// A line of a hunk, with the byte range that changed against its pair, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub line: usize,
    pub change: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A file's name bar (all-files view).
    File { file: usize },
    /// Between hunks: the enclosing function.
    Hunk { file: usize, hunk: usize },
    /// `part`: which piece of a wrapped line (0: its first, with the numbers).
    Unified { file: usize, hunk: usize, cell: Cell, part: usize },
    Split { file: usize, hunk: usize, left: Option<Cell>, right: Option<Cell>, part: usize },
    /// "binary file", "no changes", "reading…".
    Notice { file: usize, text: String },
    /// After a file's last line; `next` when Tab has another file to go to.
    End { file: usize, next: bool },
    /// After the last file of the all-files view.
    DiffEnd,
}

impl Row {
    /// The file it belongs to (none for the diff's end).
    pub fn file(&self) -> Option<usize> {
        match self {
            Row::File { file }
            | Row::Hunk { file, .. }
            | Row::Unified { file, .. }
            | Row::Split { file, .. }
            | Row::Notice { file, .. }
            | Row::End { file, .. } => Some(*file),
            Row::DiffEnd => None,
        }
    }
}

/// The rows of file `file`'s diff (without its bar and end), wrapped at `cols`
/// characters of code if given.
pub fn rows(file: usize, diff: &FileDiff, side_by_side: bool, cols: Option<usize>) -> Vec<Row> {
    if diff.binary {
        return vec![Row::Notice { file, text: "binary file".into() }];
    }
    if diff.hunks.is_empty() {
        return vec![Row::Notice { file, text: "no changes to show (a rename or a mode change)".into() }];
    }
    let pieces = |text: &str| cols.map_or(1, |c| wrapped_lines(text, c));
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        out.push(Row::Hunk { file, hunk: h });
        let lines = &hunk.lines;
        let split = |out: &mut Vec<Row>, left: Option<Cell>, right: Option<Cell>| {
            let n = [left, right].iter().flatten().map(|c| pieces(&lines[c.line].text)).max().unwrap_or(1);
            out.extend((0..n).map(|part| Row::Split { file, hunk: h, left, right, part }));
        };
        let mut i = 0;
        while i < lines.len() {
            if lines[i].kind == LineKind::Context {
                let cell = Cell { line: i, change: None };
                if side_by_side {
                    split(&mut out, Some(cell), Some(cell));
                } else {
                    out.extend((0..pieces(&lines[i].text)).map(|part| Row::Unified { file, hunk: h, cell, part }));
                }
                i += 1;
                continue;
            }
            // A change: removed lines, then the added lines replacing them.
            let removed = lines[i..].iter().take_while(|l| l.kind == LineKind::Removed).count();
            let after = i + removed;
            let added = lines[after..].iter().take_while(|l| l.kind == LineKind::Added).count();
            let mut left: Vec<Cell> = (i..after).map(|line| Cell { line, change: None }).collect();
            let mut right: Vec<Cell> = (after..after + added).map(|line| Cell { line, change: None }).collect();
            i = after + added;
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                if let Some((a, b)) = changed(&lines[l.line].text, &lines[r.line].text) {
                    l.change = Some(a);
                    r.change = Some(b);
                }
            }
            if side_by_side {
                for k in 0..left.len().max(right.len()) {
                    split(&mut out, left.get(k).copied(), right.get(k).copied());
                }
            } else {
                for cell in left.into_iter().chain(right) {
                    out.extend((0..pieces(&lines[cell.line].text)).map(|part| Row::Unified { file, hunk: h, cell, part }));
                }
            }
        }
    }
    out
}

/// The part of `old` and `new` that differs (byte ranges), once their common start and end
/// are taken off; `None` when they share too little for that to help.
fn changed(old: &str, new: &str) -> Option<((usize, usize), (usize, usize))> {
    let prefix: usize = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
    let (old_rest, new_rest) = (&old[prefix..], &new[prefix..]);
    let suffix: usize =
        old_rest.chars().rev().zip(new_rest.chars().rev()).take_while(|(a, b)| a == b).map(|(a, _)| a.len_utf8()).sum();
    let shared = prefix + suffix;
    let longest = old.len().max(new.len());
    if longest == 0 || shared * 10 < longest * 3 {
        return None;
    }
    // Widen to whole words, so a changed name is marked whole.
    let widen = |s: &str, start: usize, end: usize| {
        let word = |c: char| c.is_alphanumeric() || c == '_';
        let start = s[..start].char_indices().rev().take_while(|(_, c)| word(*c)).last().map_or(start, |(i, _)| i);
        let end = end + s[end..].chars().take_while(|c| word(*c)).map(char::len_utf8).sum::<usize>();
        (start, end)
    };
    Some((widen(old, prefix, old.len() - suffix), widen(new, prefix, new.len() - suffix)))
}

/// `text` cut into lines of at most `cols` characters, as byte ranges covering it all;
/// lines break after a space or a `,`/`;` when one is in the second two thirds of the line,
/// else mid-word. An empty text is one empty line.
pub fn wrap(text: &str, cols: usize) -> Vec<(usize, usize)> {
    let cols = cols.max(1);
    // Short lines (bytes count at least the characters) need no work.
    if text.len() <= cols {
        return vec![(0, text.len())];
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let byte = |k: usize| chars.get(k).map_or(text.len(), |c| c.0);
    let mut out = Vec::new();
    let mut start = 0;
    while chars.len() - start > cols {
        let end = start + cols;
        let cut = (start + cols / 3 + 1..=end).rev().find(|&k| matches!(chars[k - 1].1, ' ' | ',' | ';')).unwrap_or(end);
        out.push((byte(start), byte(cut)));
        start = cut;
    }
    out.push((byte(start), text.len()));
    out
}

/// How many lines `text` takes wrapped at `cols` (as [`wrap`] cuts it).
pub fn wrapped_lines(text: &str, cols: usize) -> usize {
    if text.len() <= cols.max(1) { 1 } else { wrap(text, cols).len() }
}
