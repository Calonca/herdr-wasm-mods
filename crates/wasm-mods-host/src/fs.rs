//! Files for WASM mods: the host reads, lists and writes files on a module's behalf
//! ([`FsOp`]); modules themselves never touch the filesystem (they have no imports).
//!
//! # What a module may reach
//!
//! A module's [`Root`]s are the folders of its granted `fs.read:<dir>` /
//! `fs.write:<dir>` capabilities. A request names an absolute or `~/` path; it is
//! allowed when, with `~` expanded and `.` and `..` resolved by name, it lies inside a
//! root that allows the operation (reading, listing, stat: read; writing, making
//! folders, deleting: write).
//!
//! # How a path is checked (and why it can't be fooled by symlinks)
//!
//! The name check only picks the root. The access itself walks down from the root, one
//! component at a time, with file descriptors: the root folder is opened (following
//! symlinks in the root's own path: that is the folder the user allowed), then each
//! component below it with `openat(parent_fd, name, O_NOFOLLOW | O_DIRECTORY)`, and the
//! last one with `O_NOFOLLOW` (and `O_NONBLOCK`, so a FIFO can't hang a read). A symlink
//! anywhere below the root fails the walk and the request is refused, even when it
//! points back inside. Every step is relative to a descriptor already open on the real
//! folder, so swapping folders between the check and the use can't redirect it. Writes
//! go to a temporary file made with `O_CREAT | O_EXCL | O_NOFOLLOW` in the parent
//! descriptor, then `renameat` over the name (atomic).
//!
//! Limits: a hard link inside a granted folder to a file outside it is readable, as on
//! any Unix; a bind mount inside the folder is followed like a folder.
//!
//! Requests run in order, after the call that made them; their answers come in the
//! next call ([`wasm_mods_protocol::Input::fs_result`]), bytes beside the JSON. Open
//! handles, line reads and watches live in the session ([`Files`]); watches are polled
//! (about four times a second). The file pickers are the desktop's own chooser, through
//! the XDG portal ([`portal`]): what the user picks is added to the roots for that
//! session.

use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use wasm_mods_protocol::{Blob, DirEntry, FsKind, FsOp, FsReply, FsStat};

use crate::permissions::{Capability, Gate};

mod at;
mod files;
pub mod portal;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod osascript;

pub use at::*;
pub use files::Files;

/// The most a whole-file read returns, and the most one write may hold.
pub const READ_FILE_MAX: u64 = 8 << 20;
pub const WRITE_MAX: usize = 8 << 20;
/// The most entries a listing returns.
pub const LIST_MAX: usize = 5000;
/// File requests one call may make.
pub const REQUESTS_MAX: usize = 64;

/// Somewhere a module may go.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Root {
    /// Absolute, `~` expanded, `.`/`..` resolved by name.
    pub dir: PathBuf,
    /// Only this entry of `dir`.
    pub only: Option<OsString>,
    pub read: bool,
    pub write: bool,
}

/// What a request needs of a root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Read,
    Write,
    /// Read or write (stat).
    Either,
}

impl Root {
    fn allows(&self, need: Need) -> bool {
        match need {
            Need::Read => self.read,
            Need::Write => self.write,
            Need::Either => self.read || self.write,
        }
    }
}

/// The home folder with its symlinks resolved (`/home` is a link on some systems), so
/// `~` means one thing everywhere.
pub fn home() -> Option<PathBuf> {
    static RESOLVED: Mutex<Option<(OsString, PathBuf)>> = Mutex::new(None);
    let raw = std::env::var_os("HOME")?;
    let mut cache = RESOLVED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((was, resolved)) = cache.as_ref()
        && *was == raw
    {
        return Some(resolved.clone());
    }
    let home = PathBuf::from(&raw);
    let resolved = home.canonicalize().unwrap_or(home);
    *cache = Some((raw, resolved.clone()));
    Some(resolved)
}

/// `path` absolute and normalized: `~` expanded (with `home`), `.` and `..` resolved by
/// name (never above `/`). Relative paths, `~user` and NUL bytes are refused.
pub fn expand(path: &str, home: Option<&Path>) -> Result<PathBuf, String> {
    if path.contains('\0') {
        return Err("not a valid path".into());
    }
    let full = if path == "~" || path.starts_with("~/") {
        let home = home.ok_or("no home folder to expand ~ in")?;
        home.join(path.trim_start_matches('~').trim_start_matches('/'))
    } else if path.starts_with('~') {
        return Err("only ~ (your home) can start a path".into());
    } else {
        PathBuf::from(path)
    };
    if !full.is_absolute() {
        return Err(format!("{path}: a path must be absolute or start with ~/"));
    }
    let mut out = PathBuf::from("/");
    for c in full.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Ok(out)
}

/// `path` as a module sees it: `~/…` under the home folder.
pub fn display(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// The root of a granted capability.
pub fn root_of_capability(cap: &Capability, home: Option<&Path>) -> Option<Root> {
    let (dir, read, write) = match cap {
        Capability::FsRead(d) => (d, true, false),
        Capability::FsWrite(d) => (d, false, true),
        _ => return None,
    };
    Some(Root { dir: expand(dir, home).ok()?, only: None, read, write })
}

/// Everything module `gate` may reach right now: its granted `fs` capabilities.
pub fn roots(gate: &mut Gate) -> Vec<Root> {
    let home = home();
    let mut out = Vec::new();
    for cap in gate.declared() {
        if let Some(root) = root_of_capability(&cap, home.as_deref())
            && gate.allows(&cap)
        {
            out.push(root);
        }
    }
    out
}

/// Where a request lands: the root and the components below it.
#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    pub root: Root,
    pub rel: Vec<OsString>,
}

/// The root (allowing `need`) that `path` lies in, and the rest of the path. Matched by
/// name against each root as written and as resolved; the access walk then checks the
/// real thing.
pub fn locate(roots: &[Root], path: &str, need: Need, home: Option<&Path>) -> Result<Located, String> {
    let full = expand(path, home)?;
    for root in roots.iter().filter(|r| r.allows(need)) {
        let resolved = root.dir.canonicalize().ok().filter(|c| *c != root.dir);
        for dir in std::iter::once(&root.dir).chain(resolved.as_ref()) {
            let Ok(rest) = full.strip_prefix(dir) else { continue };
            let rel: Vec<OsString> = rest.components().map(|c| c.as_os_str().to_owned()).collect();
            match &root.only {
                Some(only) if rel.len() != 1 || rel[0] != *only => continue,
                _ => return Ok(Located { root: root.clone(), rel }),
            }
        }
    }
    let what = match need {
        Need::Read => "read",
        Need::Write => "write",
        Need::Either => "reach",
    };
    Err(format!("not allowed: this mod may not {what} {path}"))
}

/// Carries out one request with what the module may reach now. `bytes` are the raw
/// bytes beside the output (what a write writes). The answer, and bytes to go beside
/// the next input (its `Blob` counts from 0: the caller moves it).
pub fn run(op: &FsOp, bytes: &[u8], roots: &[Root]) -> (FsReply, Vec<u8>) {
    let home = home();
    let at = |path: &str, need| locate(roots, path, need, home.as_deref());
    let result = match op {
        FsOp::Stat { path } => at(path, Need::Either).and_then(|l| stat(&l, path)).map(FsReply::Stat),
        FsOp::ReadFile { path } => {
            return match at(path, Need::Read).and_then(|l| read_file(&l, path)) {
                Ok(data) => (FsReply::Bytes { offset: 0, data: Blob { start: 0, len: data.len() as u32 }, eof: true }, data),
                Err(e) => (FsReply::Error(e), Vec::new()),
            };
        }
        FsOp::List { path } => {
            at(path, Need::Read).and_then(|l| list(&l, path)).map(|(entries, truncated)| FsReply::Entries { entries, truncated })
        }
        FsOp::Write { path, data } => {
            let start = data.start as usize;
            match bytes.get(start..start.saturating_add(data.len as usize)) {
                Some(b) => at(path, Need::Write).and_then(|l| write(&l, path, b)).map(|_| FsReply::Done),
                None => Err("the bytes to write aren't in the output".into()),
            }
        }
        FsOp::CreateDir { path } => at(path, Need::Write).and_then(|l| create_dir(&l, path)).map(|_| FsReply::Done),
        FsOp::Delete { path } => at(path, Need::Write).and_then(|l| delete(&l, path)).map(|_| FsReply::Done),
        // The session's own (handles, watches, the picker): never here.
        _ => Err("not available here".into()),
    };
    match result {
        Ok(reply) => (reply, Vec::new()),
        Err(e) => (FsReply::Error(e), Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wasm-mods-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn requests_stay_inside_their_grant() {
        let base = tmp("grant");
        let notes = base.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::write(base.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(base.join("secret"), notes.join("link")).unwrap();
        let roots = [
            Root { dir: notes.clone(), only: None, read: true, write: false },
            Root { dir: notes.clone(), only: None, read: false, write: true },
        ];
        let p = |rel: &str| format!("{}/{rel}", notes.display());
        let write = FsOp::Write { path: p("a.txt"), data: Blob { start: 1, len: 2 } };
        assert_eq!(run(&write, b"xhiy", &roots).0, FsReply::Done);
        let (reply, data) = run(&FsOp::ReadFile { path: p("a.txt") }, b"", &roots);
        assert!(matches!(reply, FsReply::Bytes { eof: true, .. }));
        assert_eq!(data, b"hi");
        // Out by `..`, by a symlink, or outside the root: refused.
        for path in [p("../secret"), p("link"), base.join("secret").display().to_string(), "notes/a.txt".into()] {
            let (reply, data) = run(&FsOp::ReadFile { path: path.clone() }, b"", &roots);
            assert!(matches!(reply, FsReply::Error(_)), "{path}: {reply:?}");
            assert!(data.is_empty());
        }
        // Read-only grants don't write.
        let read_only = [roots[0].clone()];
        assert!(matches!(run(&write, b"xhiy", &read_only).0, FsReply::Error(e) if e.starts_with("not allowed")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
