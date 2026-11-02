//! What one session holds open for its module: file handles (ranged and line reads),
//! watches, and what the user picked for it in the file chooser ([`portal`](super::portal)).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::os::unix::fs::FileExt;

use wasm_mods_protocol::{Blob, FsChange, FsKind, FsOp, FsReply, FsStat, FsWatchEvent};

use super::{Need, Root, home, list, locate, open_file, stat};

/// The most one ranged read returns.
pub const RANGE_MAX: u32 = 4 << 20;
/// The most lines one line read returns, and the longest line (longer ones are cut).
pub const LINES_MAX: u32 = 2000;
pub const LINE_LEN_MAX: usize = 16 << 10;
/// Handles and watches one module may hold.
pub const HANDLES_MAX: usize = 64;
pub const WATCHES_MAX: usize = 64;
/// How much of a file one line read indexes (it answers "ask again" past that).
const INDEX_STEP: u64 = 64 << 20;

struct Handle {
    file: File,
    path: String,
    /// Where each line starts, as far as `indexed` bytes; `done` when that's the end.
    starts: Vec<u64>,
    indexed: u64,
    done: bool,
}

enum Target {
    Path(String),
    Handle(u64),
}

struct Watch {
    id: String,
    target: Target,
    stat: Option<FsStat>,
    /// A folder's entries (name → size and time), to tell what came and went.
    entries: Option<BTreeMap<String, (u64, Option<f64>)>>,
}

#[derive(Default)]
pub struct Files {
    handles: HashMap<u64, Handle>,
    next: u64,
    watches: Vec<Watch>,
    /// Files and folders the user picked for the module (this session).
    pub picked: Vec<Root>,
}

impl Files {
    pub fn watching(&self) -> bool {
        !self.watches.is_empty()
    }

    /// Everything the module may reach: its grants and what was picked for it.
    pub fn roots(&self, granted: &[Root]) -> Vec<Root> {
        granted.iter().chain(&self.picked).cloned().collect()
    }

    /// A whole open file (a picture shown from its handle), at most `max` bytes.
    pub fn read_handle(&self, handle: u64, max: u64) -> Result<(String, Vec<u8>), String> {
        let h = self.handles.get(&handle).ok_or("no such handle")?;
        let size = h.file.metadata().map(|m| m.len()).unwrap_or(0);
        if size > max {
            return Err(format!("too big: {size} bytes (at most {max})"));
        }
        let mut data = vec![0; size as usize];
        let n = read_at(&h.file, &mut data, 0)?;
        data.truncate(n);
        Ok((h.path.clone(), data))
    }

    /// Carries out the requests that need this session's state; `None`: not one of them.
    /// `id` is the request's (a watch is known by it).
    pub fn run(&mut self, id: &str, op: &FsOp, roots: &[Root]) -> Option<(FsReply, Vec<u8>)> {
        let home = home();
        let at = |path: &str, need| locate(roots, path, need, home.as_deref());
        let result = match op {
            FsOp::Open { path } => {
                if self.handles.len() >= HANDLES_MAX {
                    Err(format!("too many open files (at most {HANDLES_MAX}): close some"))
                } else {
                    at(path, Need::Read).and_then(|l| open_file(&l, path)).map(|file| {
                        self.next += 1;
                        let handle = self.next;
                        let stat = file.metadata().map(|m| stat_of_meta(&m)).unwrap_or_default();
                        let h = Handle { file, path: path.clone(), starts: vec![0], indexed: 0, done: false };
                        self.handles.insert(handle, h);
                        FsReply::Opened { handle, stat }
                    })
                }
            }
            FsOp::Close { handle } => {
                self.watches.retain(|w| !matches!(w.target, Target::Handle(h) if h == *handle));
                self.handles.remove(handle).map(|_| FsReply::Done).ok_or_else(|| "no such handle".into())
            }
            FsOp::StatHandle { handle } => self.handle(*handle).and_then(|h| {
                h.file.metadata().map(|m| FsReply::Stat(stat_of_meta(&m))).map_err(|e| format!("{}: {e}", h.path))
            }),
            FsOp::ReadRange { handle, offset, len } => {
                return Some(match self.handle(*handle).and_then(|h| read_range(h, *offset, *len)) {
                    Ok((data, eof)) => {
                        let reply = FsReply::Bytes { offset: *offset, data: Blob { start: 0, len: data.len() as u32 }, eof };
                        (reply, data)
                    }
                    Err(e) => (FsReply::Error(e), Vec::new()),
                });
            }
            FsOp::ReadLines { handle, start, count } => match self.handles.get_mut(handle) {
                Some(h) => read_lines(h, *start, (*count).min(LINES_MAX)),
                None => Err("no such handle".into()),
            },
            FsOp::Watch { path } => {
                if self.watches.len() >= WATCHES_MAX {
                    Err(format!("too many watches (at most {WATCHES_MAX})"))
                } else {
                    at(path, Need::Read).and_then(|l| {
                        let st = stat(&l, path)?;
                        let entries = (st.kind == FsKind::Dir).then(|| entries_of(&l, path)).transpose()?;
                        self.watches.retain(|w| w.id != id);
                        self.watches.push(Watch { id: id.into(), target: Target::Path(path.clone()), stat: Some(st.clone()), entries });
                        Ok(FsReply::Watching(st))
                    })
                }
            }
            FsOp::WatchHandle { handle } => self.handle(*handle).and_then(|h| {
                let st = h.file.metadata().map(|m| stat_of_meta(&m)).map_err(|e| format!("{}: {e}", h.path))?;
                Ok(st)
            }).map(|st| {
                self.watches.retain(|w| w.id != id);
                self.watches.push(Watch { id: id.into(), target: Target::Handle(*handle), stat: Some(st.clone()), entries: None });
                FsReply::Watching(st)
            }),
            FsOp::Unwatch => {
                self.watches.retain(|w| w.id != id);
                Ok(FsReply::Done)
            }
            _ => return None,
        };
        Some(match result {
            Ok(reply) => (reply, Vec::new()),
            Err(e) => (FsReply::Error(e), Vec::new()),
        })
    }

    fn handle(&self, handle: u64) -> Result<&Handle, String> {
        self.handles.get(&handle).ok_or_else(|| "no such handle".into())
    }

    /// What changed in the watched files and folders since the last look. A watch whose
    /// file is gone (or no longer allowed) ends with [`FsChange::Removed`].
    pub fn poll(&mut self, roots: &[Root]) -> Vec<FsWatchEvent> {
        let home = home();
        let mut events = Vec::new();
        let handles = &mut self.handles;
        self.watches.retain_mut(|w| {
            let now = match &w.target {
                Target::Path(path) => locate(roots, path, Need::Read, home.as_deref()).and_then(|l| {
                    let st = stat(&l, path)?;
                    let entries = (st.kind == FsKind::Dir).then(|| entries_of(&l, path)).transpose()?;
                    Ok((st, entries))
                }),
                Target::Handle(h) => match handles.get(h) {
                    Some(h) => h.file.metadata().map(|m| (stat_of_meta(&m), None)).map_err(|e| e.to_string()),
                    None => Err("closed".into()),
                },
            };
            let Ok((st, entries)) = now else {
                events.push(FsWatchEvent { id: w.id.clone(), change: FsChange::Removed });
                return false;
            };
            let id = &w.id;
            match (&w.entries, &entries) {
                (Some(old), Some(new)) => {
                    let gone: Vec<&String> = old.keys().filter(|k| !new.contains_key(*k)).collect();
                    let came: Vec<&String> = new.keys().filter(|k| !old.contains_key(*k)).collect();
                    // One went and one came with the same size and time: a rename.
                    if let ([from], [to]) = (&gone[..], &came[..])
                        && old[*from] == new[*to]
                    {
                        events.push(FsWatchEvent { id: id.clone(), change: FsChange::Renamed { from: (*from).clone(), to: (*to).clone() } });
                    } else {
                        for name in gone {
                            events.push(FsWatchEvent { id: id.clone(), change: FsChange::Deleted { name: name.clone() } });
                        }
                        for name in came {
                            events.push(FsWatchEvent { id: id.clone(), change: FsChange::Created { name: name.clone() } });
                        }
                    }
                    // Still there under its name, but rewritten (or replaced by a rename
                    // over it): its size or time changed. Told as Created: it's new content.
                    for (name, now) in new {
                        if old.get(name).is_some_and(|before| before != now) {
                            events.push(FsWatchEvent { id: id.clone(), change: FsChange::Created { name: name.clone() } });
                        }
                    }
                }
                _ => {
                    let old = w.stat.as_ref();
                    if old.map(|o| (o.size, o.modified)) != Some((st.size, st.modified)) {
                        let change = FsChange::Changed { old_size: old.map(|o| o.size).unwrap_or(0), new_size: st.size, modified: st.modified };
                        events.push(FsWatchEvent { id: id.clone(), change });
                    }
                }
            }
            if let Target::Handle(h) = w.target
                && let Some(h) = handles.get_mut(&h)
                && w.stat.as_ref().is_some_and(|o| st.size < o.size)
            {
                // Shorter than before: what was indexed is stale.
                (h.starts, h.indexed, h.done) = (vec![0], 0, false);
            }
            w.stat = Some(st);
            w.entries = entries;
            true
        });
        events
    }
}

fn stat_of_meta(m: &std::fs::Metadata) -> FsStat {
    use std::os::unix::fs::MetadataExt;
    let kind = if m.is_dir() {
        FsKind::Dir
    } else if m.is_file() {
        FsKind::File
    } else {
        FsKind::Other
    };
    FsStat { kind, size: m.len(), modified: Some(m.mtime() as f64 + m.mtime_nsec() as f64 / 1e9) }
}

fn entries_of(l: &super::Located, path: &str) -> Result<BTreeMap<String, (u64, Option<f64>)>, String> {
    let (entries, _) = list(l, path)?;
    Ok(entries.into_iter().map(|e| (e.name, (e.stat.size, e.stat.modified))).collect())
}

fn read_at(file: &File, buf: &mut [u8], offset: u64) -> Result<usize, String> {
    let mut n = 0;
    while n < buf.len() {
        match file.read_at(&mut buf[n..], offset + n as u64) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(n)
}

fn read_range(h: &Handle, offset: u64, len: u32) -> Result<(Vec<u8>, bool), String> {
    let size = h.file.metadata().map(|m| m.len()).map_err(|e| e.to_string())?;
    let len = len.min(RANGE_MAX) as u64;
    let want = size.saturating_sub(offset).min(len) as usize;
    let mut data = vec![0; want];
    let n = read_at(&h.file, &mut data, offset)?;
    data.truncate(n);
    Ok((data, offset + n as u64 >= size))
}

/// Lines `start..start + count`, indexing as far as needed (at most [`INDEX_STEP`] more
/// bytes per request: past that it answers with no lines and how far it got).
fn read_lines(h: &mut Handle, start: u64, count: u32) -> Result<FsReply, String> {
    let size = h.file.metadata().map(|m| m.len()).map_err(|e| e.to_string())?;
    if size < h.indexed {
        (h.starts, h.indexed, h.done) = (vec![0], 0, false);
    }
    if h.done && size > h.indexed {
        h.done = false;
    }
    let need = start + count as u64 + 1;
    let limit = h.indexed + INDEX_STEP;
    let mut buf = vec![0u8; 1 << 20];
    while !h.done && (h.starts.len() as u64) < need && h.indexed < limit {
        let n = read_at(&h.file, &mut buf, h.indexed)?;
        if n == 0 {
            h.done = true;
            break;
        }
        for (i, b) in buf[..n].iter().enumerate() {
            if *b == b'\n' {
                h.starts.push(h.indexed + i as u64 + 1);
            }
        }
        h.indexed += n as u64;
    }
    // The last line (without a newline at the end) counts too.
    let total = h.done.then(|| if h.starts.last() == Some(&h.indexed) { h.starts.len() as u64 - 1 } else { h.starts.len() as u64 });
    let known = total.unwrap_or(h.starts.len().saturating_sub(1) as u64);
    let indexed = if size == 0 { 1.0 } else { (h.indexed as f64 / size as f64).min(1.0) as f32 };
    if start >= known {
        return Ok(FsReply::Lines { start, lines: Vec::new(), cut: Vec::new(), total, indexed });
    }
    let end = (start + count as u64).min(known);
    let mut lines = Vec::new();
    let mut cut = Vec::new();
    for i in start..end {
        let from = h.starts[i as usize];
        let to = h.starts.get(i as usize + 1).copied().unwrap_or(h.indexed);
        let mut len = (to - from) as usize;
        let long = len > LINE_LEN_MAX;
        len = len.min(LINE_LEN_MAX);
        let mut data = vec![0; len];
        let n = read_at(&h.file, &mut data, from)?;
        data.truncate(n);
        if !long {
            if data.last() == Some(&b'\n') {
                data.pop();
            }
            if data.last() == Some(&b'\r') {
                data.pop();
            }
        } else {
            cut.push((i - start) as u32);
        }
        lines.push(String::from_utf8_lossy(&data).into_owned());
    }
    Ok(FsReply::Lines { start, lines, cut, total, indexed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_read_ranges_and_lines_and_watches_see_changes() {
        let dir = std::env::temp_dir().join(format!("wasm-mods-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("log.txt");
        std::fs::write(&file, "one\r\ntwo\nthree").unwrap();
        let roots = [Root { dir: dir.clone(), only: None, read: true, write: false }];
        let path = file.to_str().unwrap();
        let mut f = Files::default();
        let Some((FsReply::Opened { handle, stat }, _)) = f.run("o", &FsOp::Open { path: path.into() }, &roots) else { panic!() };
        assert_eq!(stat.size, 14);
        let (reply, bytes) = f.run("r", &FsOp::ReadRange { handle, offset: 5, len: 100 }, &roots).unwrap();
        assert_eq!((bytes.as_slice(), reply), (&b"two\nthree"[..], FsReply::Bytes { offset: 5, data: Blob { start: 0, len: 9 }, eof: true }));
        let (reply, _) = f.run("l", &FsOp::ReadLines { handle, start: 1, count: 5 }, &roots).unwrap();
        assert_eq!(reply, FsReply::Lines { start: 1, lines: vec!["two".into(), "three".into()], cut: vec![], total: Some(3), indexed: 1.0 });
        // A folder watch sees a file come; a handle watch sees its file grow.
        f.run("w", &FsOp::Watch { path: dir.to_str().unwrap().into() }, &roots).unwrap();
        f.run("h", &FsOp::WatchHandle { handle }, &roots).unwrap();
        std::fs::write(dir.join("new"), "x").unwrap();
        std::fs::OpenOptions::new().append(true).open(&file).unwrap().write_all_at_end(b"\nfour");
        let events = f.poll(&roots);
        assert!(events.iter().any(|e| e.id == "w" && e.change == FsChange::Created { name: "new".into() }), "{events:?}");
        assert!(events.iter().any(|e| e.id == "h" && matches!(e.change, FsChange::Changed { old_size: 14, new_size: 19, .. })), "{events:?}");
        assert!(f.poll(&roots).is_empty());
        // Outside the grant: refused.
        let (reply, _) = f.run("x", &FsOp::Open { path: "/etc/passwd".into() }, &roots).unwrap();
        assert!(matches!(reply, FsReply::Error(e) if e.starts_with("not allowed")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder watch sees a file rewritten under its name (same size, as a status
    /// file rewritten every second) and one replaced by a rename over it.
    #[test]
    fn folder_watches_see_files_rewritten_or_renamed_over() {
        let dir = std::env::temp_dir().join(format!("wasm-mods-rewrite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let roots = [Root { dir: dir.clone(), only: None, read: true, write: false }];
        let at = |secs| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let write = |name: &str, text: &str, secs| {
            std::fs::write(dir.join(name), text).unwrap();
            File::options().write(true).open(dir.join(name)).unwrap().set_modified(at(secs)).unwrap();
        };
        write("status.json", "{\"n\":1}", 1_000);
        write("other", "x", 1_000);
        let mut f = Files::default();
        f.run("w", &FsOp::Watch { path: dir.to_str().unwrap().into() }, &roots).unwrap();
        assert!(f.poll(&roots).is_empty());
        // Rewritten in place, the same size.
        write("status.json", "{\"n\":2}", 1_001);
        assert_eq!(f.poll(&roots), vec![FsWatchEvent { id: "w".into(), change: FsChange::Created { name: "status.json".into() } }]);
        assert!(f.poll(&roots).is_empty());
        // Replaced by a rename over it (an atomic save).
        write(".status.tmp", "{\"n\":33}", 1_002);
        let _ = f.poll(&roots);
        std::fs::rename(dir.join(".status.tmp"), dir.join("status.json")).unwrap();
        let events = f.poll(&roots);
        assert!(events.contains(&FsWatchEvent { id: "w".into(), change: FsChange::Created { name: "status.json".into() } }), "{events:?}");
        assert!(!events.iter().any(|e| e.change == FsChange::Created { name: "other".into() }), "{events:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    trait AppendAll {
        fn write_all_at_end(&mut self, b: &[u8]);
    }
    impl AppendAll for File {
        fn write_all_at_end(&mut self, b: &[u8]) {
            std::io::Write::write_all(self, b).unwrap();
        }
    }
}
