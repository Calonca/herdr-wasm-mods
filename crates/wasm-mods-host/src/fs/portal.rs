//! The desktop's own file chooser, for [`FsOp::PickFile`], [`FsOp::PickFolder`] and
//! [`FsOp::PickSave`]: asked through the XDG desktop portal
//! (`org.freedesktop.portal.FileChooser` on the session bus), so it's the dialog the
//! user knows (KDE's, GNOME's). Only what they choose becomes the module's to reach
//! ([`Files::picked`](super::Files::picked)); the module never sees the dialog, only
//! the answer. The dialog runs on its own thread: the answer comes in a later call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use wasm_mods_protocol::{FsKind, FsOp, FsReply};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{Root, display, home};

/// What a module asked to have chosen.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A file to read; `extensions` (`md`) narrow what the chooser lists.
    File { extensions: Vec<String> },
    Folder,
    /// Where to save one file.
    Save { suggested: String },
}

impl Ask {
    pub fn of(op: &FsOp) -> Option<Ask> {
        Some(match op {
            FsOp::PickFile { extensions } => Ask::File {
                extensions: extensions
                    .iter()
                    .map(|e| e.trim_start_matches('.').to_owned())
                    .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()))
                    .collect(),
            },
            FsOp::PickFolder => Ask::Folder,
            FsOp::PickSave { suggested } => Ask::Save { suggested: suggested.replace('/', "") },
            _ => return None,
        })
    }
}

/// What the user decided: the answer, and what the module may now reach.
#[derive(Debug, PartialEq)]
pub struct Picked {
    pub reply: FsReply,
    pub root: Option<Root>,
}

/// Opens the chooser on its own thread, titled for `module`; the answer arrives on the
/// receiver.
pub fn pick(module: &str, ask: Ask) -> mpsc::Receiver<Picked> {
    let (tx, rx) = mpsc::channel();
    let title = match ask {
        Ask::File { .. } => format!("{module}: open a file"),
        Ask::Folder => format!("{module}: choose a folder"),
        Ask::Save { .. } => format!("{module}: save as"),
    };
    std::thread::spawn(move || {
        let chosen = Connection::session()
            .map_err(|e| format!("no session bus for the file chooser: {e}"))
            .and_then(|conn| choose(&conn, &title, &ask));
        let _ = tx.send(answer(&ask, chosen));
    });
    rx
}

/// Asks the portal on `conn` and waits for the user: the chosen URIs, or `None` when
/// they cancelled.
pub fn choose(conn: &Connection, title: &str, ask: &Ask) -> Result<Option<Vec<String>>, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let token = format!("wasm_mods_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
    let fail = |e: zbus::Error| format!("the desktop's file chooser: {e}");
    // Listening before asking: the answer can come before the call returns.
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")
        .and_then(|b| b.member("Response"))
        .map_err(fail)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, conn, Some(16)).map_err(fail)?;
    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert("handle_token", Value::from(token));
    options.insert("modal", Value::from(true));
    let method = match ask {
        Ask::File { extensions } => {
            if !extensions.is_empty() {
                let patterns: Vec<(u32, String)> = extensions.iter().map(|e| (0, format!("*.{e}"))).collect();
                let label = extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ");
                options.insert("filters", Value::from(vec![(label, patterns)]));
            }
            "OpenFile"
        }
        Ask::Folder => {
            options.insert("directory", Value::from(true));
            "OpenFile"
        }
        Ask::Save { suggested } => {
            if !suggested.is_empty() {
                options.insert("current_name", Value::from(suggested.clone()));
            }
            "SaveFile"
        }
    };
    let reply = conn
        .call_method(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            Some("org.freedesktop.portal.FileChooser"),
            method,
            &("", title, options),
        )
        .map_err(fail)?;
    let handle: OwnedObjectPath = reply.body().deserialize().map_err(fail)?;
    for msg in signals {
        let msg = msg.map_err(fail)?;
        if msg.header().path().map(|p| p.as_str()) != Some(handle.as_str()) {
            continue;
        }
        let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().map_err(fail)?;
        return match code {
            0 => {
                let uris = results.get("uris").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default();
                Ok(Some(uris))
            }
            1 => Ok(None),
            _ => Err("the desktop's file chooser failed".into()),
        };
    }
    Err("the desktop's file chooser went away".into())
}

/// A `file://` URI's path (`%xx` decoded); `None` for anything else (a remote file).
pub fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let byte = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        if let Some(byte) = byte {
            out.push(byte);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// The module's answer to what the chooser gave, and what it may now reach.
pub fn answer(ask: &Ask, chosen: Result<Option<Vec<String>>, String>) -> Picked {
    let uris = match chosen {
        Ok(Some(uris)) => uris,
        Ok(None) => return Picked { reply: FsReply::Cancelled, root: None },
        Err(e) => return Picked { reply: FsReply::Error(e), root: None },
    };
    let Some(path) = uris.first().and_then(|u| uri_path(u)) else {
        return Picked { reply: FsReply::Error("the chooser gave no local file".into()), root: None };
    };
    // Resolved: the module may reach what the user chose, not where a link points next.
    let real = if let Ask::Save { .. } = ask {
        match (path.parent().and_then(|d| d.canonicalize().ok()), path.file_name()) {
            (Some(dir), Some(name)) => dir.join(name),
            _ => path,
        }
    } else {
        path.canonicalize().unwrap_or(path)
    };
    let shown = |p: &Path| display(p, home().as_deref());
    let one = |p: &Path, write: bool| {
        Some(Root { dir: p.parent()?.to_owned(), only: Some(p.file_name()?.to_owned()), read: true, write })
    };
    let (root, kind, access) = match ask {
        Ask::File { .. } => (one(&real, false), FsKind::File, "read"),
        Ask::Folder => (Some(Root { dir: real.clone(), only: None, read: true, write: false }), FsKind::Dir, "read"),
        Ask::Save { .. } => (one(&real, true), FsKind::File, "write"),
    };
    match root {
        Some(root) => Picked { reply: FsReply::Picked { path: shown(&real), kind, access: access.into() }, root: Some(root) },
        None => Picked { reply: FsReply::Error("can't use that".into()), root: None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_become_paths() {
        assert_eq!(uri_path("file:///home/u/My%20Notes/a.md"), Some(PathBuf::from("/home/u/My Notes/a.md")));
        assert_eq!(uri_path("file://localhost/tmp/x"), Some(PathBuf::from("/tmp/x")));
        assert_eq!(uri_path("sftp://host/tmp/x"), None);
        assert_eq!(uri_path("file://other/tmp/x"), None);
        assert_eq!(uri_path("file:///bad%2"), Some(PathBuf::from("/bad%2")));
    }

    /// The whole exchange against a stand-in portal over a socket pair: the request,
    /// its options, and the Response signal on the request's handle.
    #[test]
    #[allow(deprecated)] // unix_stream: this build has no tokio
    fn asks_the_portal_and_reads_its_answer() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let guid = zbus::Guid::generate();
        let server = std::thread::spawn(move || {
            let conn = zbus::blocking::connection::Builder::unix_stream(b).server(guid).unwrap().p2p().build().unwrap();
            let calls = MessageIterator::from(&conn);
            for msg in calls {
                let msg = msg.unwrap();
                if msg.header().member().map(|m| m.as_str()) != Some("OpenFile") {
                    continue;
                }
                let (parent, title, options): (String, String, HashMap<String, OwnedValue>) = msg.body().deserialize().unwrap();
                assert_eq!((parent.as_str(), title.as_str()), ("", "notes: open a file"));
                assert!(options.contains_key("filters") && options.contains_key("handle_token"));
                let handle = zbus::zvariant::ObjectPath::try_from("/org/freedesktop/portal/desktop/request/x/t1").unwrap();
                conn.reply(&msg.header(), &(handle.clone(),)).unwrap();
                let mut results: HashMap<&str, Value> = HashMap::new();
                results.insert("uris", Value::from(vec!["file:///tmp/a%20b.md".to_owned()]));
                conn.emit_signal(None::<&str>, &handle, "org.freedesktop.portal.Request", "Response", &(0u32, results)).unwrap();
                return;
            }
        });
        let conn = zbus::blocking::connection::Builder::unix_stream(a).p2p().build().unwrap();
        let got = choose(&conn, "notes: open a file", &Ask::File { extensions: vec!["md".into()] }).unwrap();
        assert_eq!(got, Some(vec!["file:///tmp/a%20b.md".to_owned()]));
        server.join().unwrap();
        // A file: only it, to read; cancelled: nothing.
        let picked = answer(&Ask::File { extensions: vec![] }, Ok(got));
        assert!(matches!(&picked.reply, FsReply::Picked { kind: FsKind::File, access, .. } if access == "read"));
        assert_eq!(picked.root.map(|r| (r.only, r.write)), Some((Some("a b.md".into()), false)));
        assert_eq!(answer(&Ask::Folder, Ok(None)), Picked { reply: FsReply::Cancelled, root: None });
    }
}
