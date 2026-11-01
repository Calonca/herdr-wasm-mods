//! Pictures on canvases ([`Shape::Image`]). herdr-app's mods can only load images from
//! the app's own folder, so the host checks each picture (PNG or JPEG, its size) and
//! writes it into `data/wasm-mods/images` there (not the mod's own folder: a change
//! there reloads the mod), named by its content's hash; the `frame` message says which
//! file each picture's key is ([`Shown`]). Pictures from files
//! are read under the module's permissions, like any other read.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use wasm_mods_protocol::{Event, ImageSrc, ImageStatus, ListItem, Node, Shape};

use crate::fs::{Files, Need, Root, home, locate, open_file};

/// The biggest picture file, and the most pixels a side.
pub const FILE_MAX: u64 = 64 << 20;
pub const SIDE_MAX: u32 = 8192;
/// Cached pictures not used for this long are deleted when a host starts (drawn
/// canvases sooner: each change makes a new one).
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);
const KEEP_DRAWN: Duration = Duration::from_secs(24 * 3600);

pub use wasm_mods_protocol::wire::Shown;

/// A picture as it was made ready (or why not), and for a file the (size, time) it was
/// read at.
type Known = (Result<Shown, String>, Option<(u64, SystemTime)>);

#[derive(Default)]
pub struct Images {
    /// The images folder; `None`: pictures can't be shown.
    dir: Option<PathBuf>,
    /// Key → the picture, or why not; for files, the (size, time) it was read at.
    known: HashMap<String, Known>,
    /// What the module hears about next ([`Event::Image`]).
    pub events: Vec<Event>,
}

impl Images {
    /// `dir`: where the mod said to put pictures (an absolute folder ending in
    /// `wasm-mods/images`).
    pub fn new(dir: Option<&str>) -> Images {
        let dir = dir.map(PathBuf::from).filter(|d| d.is_absolute() && d.ends_with("wasm-mods/images"));
        if let Some(d) = &dir {
            let _ = std::fs::create_dir_all(d);
            prune(d);
        }
        Images { dir, ..Default::default() }
    }

    /// The images folder, when pictures can be shown.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// The pictures `ui` shows, made ready; `bytes` are the output's, `roots` what the
    /// module may read now.
    pub fn resolve(&mut self, ui: &Node, bytes: &[u8], files: &Files, roots: &[Root]) -> BTreeMap<String, Shown> {
        let mut srcs = Vec::new();
        walk(ui, &mut |s| srcs.push(s.clone()));
        let mut out = BTreeMap::new();
        for src in srcs {
            let key = src.key();
            if let Some(shown) = self.one(&key, &src, bytes, files, roots) {
                out.insert(key, shown);
            }
        }
        out
    }

    fn one(&mut self, key: &str, src: &ImageSrc, bytes: &[u8], files: &Files, roots: &[Root]) -> Option<Shown> {
        let data = match src {
            ImageSrc::Cached(_) => None,
            ImageSrc::Bytes { data, .. } if data.len == 0 => None,
            ImageSrc::Bytes { data, .. } => {
                let start = data.start as usize;
                Some(bytes.get(start..start.saturating_add(data.len as usize)).map(|b| (b.to_vec(), None)).ok_or_else(|| "its bytes aren't in the output".to_string()))
            }
            ImageSrc::File(path) => {
                let at = locate(roots, path, Need::Read, home().as_deref());
                let file = at.and_then(|l| open_file(&l, path));
                let stamp = file.as_ref().ok().and_then(|f| f.metadata().ok()).map(|m| (m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
                // Unchanged since it was read: as it was.
                if stamp.is_some() && self.known.get(key).is_some_and(|(_, s)| *s == stamp) {
                    None
                } else {
                    Some(file.and_then(|f| read_all(f, path)).map(|d| (d, stamp)))
                }
            }
            ImageSrc::Handle(h) => {
                if self.known.contains_key(key) {
                    None
                } else {
                    Some(files.read_handle(*h, FILE_MAX).map(|(_, d)| (d, None)))
                }
            }
        };
        if let Some(data) = data {
            let (result, stamp) = match data {
                Ok((d, stamp)) => (self.store(&d), stamp),
                Err(e) => (Err(e), None),
            };
            let status = match &result {
                Ok(s) => ImageStatus::Loaded { width: s.width, height: s.height },
                Err(e) => ImageStatus::Failed(e.clone()),
            };
            let changed = self.known.get(key).map(|(r, _)| r) != Some(&result);
            if changed {
                self.events.push(Event::Image { key: key.into(), status });
            }
            self.known.insert(key.into(), (result, stamp));
        } else if !self.known.contains_key(key) {
            // Cached under a key it never sent: tell it to send the bytes.
            self.known.insert(key.into(), (Err("not sent yet".into()), None));
            self.events.push(Event::Image { key: key.into(), status: ImageStatus::Dropped });
        }
        self.known.get(key).and_then(|(r, _)| r.as_ref().ok().cloned())
    }

    /// Checks a picture and writes it into the cache (once: named by its hash).
    fn store(&self, data: &[u8]) -> Result<Shown, String> {
        let dir = self.dir.as_ref().ok_or("pictures can't be shown here")?;
        let (ext, width, height) = size_of(data).ok_or("not a PNG or JPEG")?;
        if width == 0 || height == 0 || width > SIDE_MAX || height > SIDE_MAX {
            return Err(format!("too big: {width}×{height} (at most {SIDE_MAX} a side)"));
        }
        let hash = Sha256::digest(data);
        let name: String = hash[..12].iter().map(|b| format!("{b:02x}")).collect::<String>() + "." + ext;
        let path = dir.join(&name);
        if path.exists() {
            // Used again: kept by the next prune.
            let _ = std::fs::File::open(&path).and_then(|f| f.set_modified(SystemTime::now()));
        } else {
            let tmp = dir.join(format!(".{name}.{}", std::process::id()));
            std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("can't keep it: {e}")
            })?;
        }
        Ok(Shown { file: name, width, height })
    }
}

fn read_all(file: std::fs::File, path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    if size > FILE_MAX {
        return Err(format!("too big: {path} is {size} bytes (at most {FILE_MAX})"));
    }
    let mut out = Vec::with_capacity(size as usize);
    (&file).take(FILE_MAX).read_to_end(&mut out).map_err(|e| format!("{path}: {e}"))?;
    Ok(out)
}

/// Every picture source in a tree.
fn walk(n: &Node, f: &mut impl FnMut(&ImageSrc)) {
    match n {
        Node::Column { children, .. } | Node::Row { children, .. } => children.iter().for_each(|c| walk(c, f)),
        Node::List { items, .. } => items.iter().filter_map(|i: &ListItem| i.child.as_deref()).for_each(|c| walk(c, f)),
        Node::Popup { child, .. } => walk(child, f),
        Node::Canvas { shapes, .. } => {
            for s in shapes {
                if let Shape::Image { src, .. } = s {
                    f(src);
                }
            }
        }
        _ => {}
    }
}

/// A PNG's or JPEG's kind and size in pixels, from its header.
pub fn size_of(d: &[u8]) -> Option<(&'static str, u32, u32)> {
    if d.len() >= 24 && d.starts_with(b"\x89PNG\r\n\x1a\n") && &d[12..16] == b"IHDR" {
        let be = |i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        return Some(("png", be(16), be(20)));
    }
    if d.starts_with(&[0xff, 0xd8]) {
        let mut i = 2;
        while i + 9 < d.len() {
            if d[i] != 0xff {
                return None;
            }
            let marker = d[i + 1];
            if marker == 0xff {
                i += 1;
                continue;
            }
            let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
            // Start of frame (not DHT, JPG, DAC): height, then width.
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                let h = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
                let w = u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32;
                return Some(("jpg", w, h));
            }
            i += 2 + len;
        }
    }
    None
}

/// Deletes cached pictures unused for a week.
fn prune(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let keep = if e.path().extension().is_some_and(|x| x == "svg") { KEEP_DRAWN } else { KEEP };
        let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| now.duration_since(t).unwrap_or_default() > keep);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_headers_give_their_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(16u32.to_be_bytes());
        png.extend(9u32.to_be_bytes());
        assert_eq!(size_of(&png), Some(("png", 16, 9)));
        // SOI, an APP0 of 4 bytes, then SOF0: 8 bits, height 300, width 400.
        let jpg = [0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 17, 8, 1, 44, 1, 144, 3, 0, 0, 0];
        assert_eq!(size_of(&jpg), Some(("jpg", 400, 300)));
        assert_eq!(size_of(b"GIF89a......."), None);
    }
}
