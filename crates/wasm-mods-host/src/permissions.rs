//! What WASM mods may do.
//!
//! A module names the capabilities it needs in a manifest, a custom wasm section
//! `wasm_mods.manifest` the SDK's `manifest!` writes (`key=value` lines: `name`,
//! `description`, `version`, `protocol`, one `capability` each). The host reads it
//! before running the module and asks the user about every capability nobody has
//! answered yet, with an Allow prompt drawn in the module's place. The answers live in
//! `~/.config/wasm-mods/permissions.toml`, per module (its file name) and capability.
//!
//! Every capability is granted only from that prompt and only for the build it was
//! allowed for: a rebuilt or swapped module is asked again (its SHA-256 differs). A
//! module without a manifest gets nothing.
//!
//! The host enforces it: data is only filled in for granted capabilities, and effects
//! needing others are refused (files answer with an error) or dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use wasm_mods_protocol::{Event, Node};

/// Something a module may be allowed to do.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Read, list and stat files inside a folder (`~/notes`).
    FsRead(String),
    /// Write, make folders and delete inside a folder.
    FsWrite(String),
    /// Know the folder (and git branch) of the tab it's shown in.
    WindowFolder,
    /// Run git there, only commands that read (history, changes, files at a commit).
    Git,
    /// Declared but not provided here (another host's, or one for later): never granted
    /// and never asked.
    Unavailable(String),
}

/// A folder an `fs.read:`/`fs.write:` capability may name: not empty, absolute or under
/// `~`, no `..` component, no control, bidi or invisible characters (it is shown to the
/// user as is).
pub fn fs_dir_ok(dir: &str) -> bool {
    !dir.is_empty()
        && (dir.starts_with('/') || dir == "~" || dir.starts_with("~/"))
        && !dir.split('/').any(|c| c == "..")
        && !dir.chars().any(|c| c.is_control() || is_bidi_control(c) || is_invisible(c))
}

/// Zero-width and other invisible format characters.
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200d}' | '\u{2060}'..='\u{2064}' | '\u{feff}' | '\u{00ad}')
}

/// Unicode's bidirectional overrides and isolates: they'd let text pretend to say
/// something else.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// A capability's folder as the host will use it (`~` expanded, then shown `~/…`).
fn resolved(dir: &str) -> String {
    let home = crate::fs::home();
    match crate::fs::expand(dir, home.as_deref()) {
        Ok(p) => crate::fs::display(&p, home.as_deref()),
        Err(_) => dir.into(),
    }
}

/// Whether an `fs` capability's folder reaches far: the home folder or anything
/// holding it, or keys and settings (`~/.ssh`, `~/.config`).
pub fn broad_dir(dir: &str, home: Option<&Path>) -> bool {
    let Ok(p) = crate::fs::expand(dir, home) else { return true };
    if p == Path::new("/") {
        return true;
    }
    home.is_some_and(|h| h.starts_with(&p) || p.starts_with(h.join(".ssh")) || p.starts_with(h.join(".config")))
}

impl Capability {
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        match s {
            "window.folder" => Self::WindowFolder,
            "git" => Self::Git,
            _ => match s.split_once(':') {
                Some(("fs.read", dir)) if fs_dir_ok(dir) => Self::FsRead(dir.into()),
                Some(("fs.write", dir)) if fs_dir_ok(dir) => Self::FsWrite(dir.into()),
                _ => Self::Unavailable(s.into()),
            },
        }
    }

    pub fn id(&self) -> String {
        match self {
            Self::FsRead(dir) => format!("fs.read:{dir}"),
            Self::FsWrite(dir) => format!("fs.write:{dir}"),
            Self::WindowFolder => "window.folder".into(),
            Self::Git => "git".into(),
            Self::Unavailable(s) => s.clone(),
        }
    }

    /// In plain words, for the Allow prompt.
    pub fn about(&self) -> String {
        match self {
            Self::FsRead(dir) => format!("read files in {}", resolved(dir)),
            Self::FsWrite(dir) => format!("change files in {}", resolved(dir)),
            Self::WindowFolder => "know the folder and git branch of the tab it's in".into(),
            Self::Git => "read the git history and changes of that folder".into(),
            Self::Unavailable(s) => format!("{s} (not available here)"),
        }
    }
}

/// What a module says about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub capabilities: Vec<Capability>,
    /// The protocol it was built for (`protocol=` line).
    pub protocol: u32,
}

/// The manifest section of a wasm binary, if it has one.
pub fn read_manifest(wasm: &[u8]) -> Option<Manifest> {
    let text = crate::engine::manifest_text(wasm)?;
    let mut m = Manifest::default();
    for line in text.lines() {
        match line.split_once('=') {
            Some(("name", v)) => m.name = v.into(),
            Some(("description", v)) => m.description = v.into(),
            Some(("version", v)) => m.version = v.into(),
            Some(("capability", v)) if !v.trim().is_empty() => m.capabilities.push(Capability::parse(v)),
            Some(("protocol", v)) => m.protocol = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(m)
}

/// SHA-256 of a module build, in hex: which build an answer was given for.
pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A module: its file name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId {
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Granted,
    Denied,
    /// Nobody answered yet (or the grant was for another build).
    Ask,
}

/// A running module's view of its permissions.
#[derive(Clone, Debug)]
pub struct Gate {
    pub id: ModuleId,
    pub hash: String,
    pub manifest: Option<Manifest>,
    /// Undeclared capabilities already logged, to log each once.
    logged: Vec<Capability>,
}

impl Gate {
    pub fn new(id: ModuleId, wasm: &[u8]) -> Self {
        Gate { id, hash: hash(wasm), manifest: read_manifest(wasm), logged: Vec::new() }
    }

    pub fn declared(&self) -> Vec<Capability> {
        self.manifest.as_ref().map(|m| m.capabilities.clone()).unwrap_or_default()
    }

    /// Whether the module may use `cap` now: declared, and granted for this build.
    pub fn allows(&mut self, cap: &Capability) -> bool {
        if !self.declared().contains(cap) {
            if !self.logged.contains(cap) {
                eprintln!("wasm-mods: {} used {} without declaring it; refused", self.id.name, cap.id());
                self.logged.push(cap.clone());
            }
            return false;
        }
        with(|g| g.decide(&self.id, &self.hash, cap)) == Decision::Granted
    }

    /// Declared capabilities nobody has answered for yet (for this build).
    pub fn pending(&self) -> Vec<Capability> {
        with(|g| self.declared().into_iter().filter(|c| g.decide(&self.id, &self.hash, c) == Decision::Ask).collect())
    }

    /// Allow or deny everything pending: the answer to the Allow prompt.
    pub fn answer(&self, allow: bool) {
        let pending = self.pending();
        save(|g| {
            for cap in &pending {
                g.set(&self.id, &self.hash, cap, allow);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// The answers on disk
// ---------------------------------------------------------------------------

/// One module's answers in `permissions.toml`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    name: String,
    /// The build last answered for.
    #[serde(default)]
    hash: String,
    #[serde(default)]
    granted: Vec<String>,
    #[serde(default)]
    denied: Vec<String>,
    /// For each granted capability, the build (SHA-256) it was allowed for.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    builds: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grants {
    /// Modules turned off (by name): they don't load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    off: Vec<String>,
    #[serde(default, rename = "module")]
    entries: Vec<Entry>,
}

impl Grants {
    fn entry(&self, id: &ModuleId) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == id.name)
    }

    /// The answer for `cap` of module `id`, build `hash`. A grant counts only for the
    /// build it was given for; a denial holds until it's taken out of the file.
    pub fn decide(&self, id: &ModuleId, hash: &str, cap: &Capability) -> Decision {
        if matches!(cap, Capability::Unavailable(_)) {
            return Decision::Denied;
        }
        let key = cap.id();
        if let Some(e) = self.entry(id) {
            if e.denied.contains(&key) {
                return Decision::Denied;
            }
            if e.granted.contains(&key) {
                let same_build = !hash.is_empty() && e.builds.get(&key).is_some_and(|h| h == hash);
                if same_build {
                    return Decision::Granted;
                }
            }
        }
        Decision::Ask
    }

    pub fn is_off(&self, id: &ModuleId) -> bool {
        self.off.contains(&id.name)
    }

    /// Forgets the answer for `cap`: the module's prompt asks again.
    pub fn forget(&mut self, id: &ModuleId, cap: &Capability) {
        let key = cap.id();
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == id.name) {
            e.granted.retain(|c| *c != key);
            e.denied.retain(|c| *c != key);
            e.builds.remove(&key);
        }
    }

    /// Forgets everything about a module (it was removed).
    pub fn forget_module(&mut self, id: &ModuleId) {
        self.entries.retain(|e| !(e.name == id.name));
        self.off.retain(|k| *k != id.name);
    }

    pub fn set_off(&mut self, id: &ModuleId, off: bool) {
        self.off.retain(|k| *k != id.name);
        if off {
            self.off.push(id.name.clone());
        }
    }

    /// Records the user's answer for `cap` of build `hash`.
    pub fn set(&mut self, id: &ModuleId, hash: &str, cap: &Capability, granted: bool) {
        let index = match self.entries.iter().position(|e| e.name == id.name) {
            Some(i) => i,
            None => {
                self.entries.push(Entry { name: id.name.clone(), ..Default::default() });
                self.entries.len() - 1
            }
        };
        let e = &mut self.entries[index];
        let key = cap.id();
        e.granted.retain(|c| *c != key);
        e.denied.retain(|c| *c != key);
        e.builds.remove(&key);
        if granted {
            e.builds.insert(key.clone(), hash.into());
            e.granted.push(key);
        } else {
            e.denied.push(key);
        }
        if !hash.is_empty() {
            e.hash = hash.into();
        }
    }
}

#[cfg(not(test))]
fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("wasm-mods").join("permissions.toml")
}

/// Tests never read or write the user's answers.
#[cfg(test)]
fn path() -> PathBuf {
    std::env::temp_dir().join(format!("wasm-mods-permissions-test-{}.toml", std::process::id()))
}

struct Store {
    grants: Grants,
    modified: Option<SystemTime>,
    checked: Option<std::time::Instant>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// Runs `f` on the current grants, re-reading the file when it changed (checked at most
/// once a second, so hand edits apply too).
pub fn with<R>(f: impl FnOnce(&Grants) -> R) -> R {
    let mut store = STORE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = path();
    let stale = store.as_ref().is_none_or(|s| s.checked.is_none_or(|t| t.elapsed().as_secs_f32() > 1.0));
    if stale {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if store.as_ref().is_none_or(|s| s.modified != modified) {
            let grants: Grants =
                std::fs::read_to_string(&path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();
            *store = Some(Store { grants, modified, checked: None });
        }
        if let Some(s) = store.as_mut() {
            s.checked = Some(std::time::Instant::now());
        }
    }
    f(&store.as_ref().expect("loaded above").grants)
}

/// Saves one at a time: each reads the grants, changes and writes them, so two at once
/// would lose one's change (two prompts answered together).
static SAVING: Mutex<()> = Mutex::new(());

/// Changes the grants with `change` and saves `permissions.toml`.
pub fn save(change: impl FnOnce(&mut Grants)) {
    let _one = SAVING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut grants = with(Grants::clone);
    change(&mut grants);
    let text = format!(
        "# What each WASM mod may do, per module and capability: the answers to its Allow\n\
         # prompt. A grant holds only for the build (sha-256 under `builds`) it was\n\
         # allowed for. Delete a module's entry to be asked again.\n\n{}",
        toml::to_string(&grants).unwrap_or_default()
    );
    let path = path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).is_err() {
        eprintln!("wasm-mods: couldn't save {}", path.display());
    }
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    *STORE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(Store { grants, modified, checked: Some(std::time::Instant::now()) });
}

// ---------------------------------------------------------------------------
// The Allow prompt
// ---------------------------------------------------------------------------

const ALLOW: &str = "wasm-mods-allow";
const DENY: &str = "wasm-mods-deny";

/// A module's own words, for the Allow prompt: one line, no control characters, at most
/// `max` characters.
pub fn one_line(text: &str, max: usize) -> String {
    let line: String = text
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .collect();
    let line = line.trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The Allow prompt for a module with unanswered capabilities: its name and description
/// (one line each, cut short: a module's own words), each capability in plain words,
/// Allow / Deny. Drawn in the module's place; [`prompt_answer`] reads the answer.
pub fn prompt(gate: &Gate, pending: &[Capability]) -> Node {
    use wasm_mods_protocol::Role;
    use wasm_mods_protocol::ui::*;
    let name = one_line(&gate.id.name, 48);
    let mut lines = vec![text(format!("{name} asks to:")).size(16.0).bold()];
    let description = gate.manifest.as_ref().map(|m| one_line(&m.description, 120)).unwrap_or_default();
    if !description.is_empty() {
        lines.push(text(description).size(13.0).role(Role::Subtext));
    }
    lines.push(space(4.0));
    for cap in pending {
        let fs_dir = match cap {
            Capability::FsRead(d) | Capability::FsWrite(d) => Some(d),
            _ => None,
        };
        let max = if fs_dir.is_some() { 1024 } else { 200 };
        lines.push(row([accent("•"), text(one_line(&cap.about(), max)), muted(one_line(&cap.id(), max)).size(11.0)]).gap(6.0));
        if let Some(d) = fs_dir
            && broad_dir(d, crate::fs::home().as_deref())
        {
            lines.push(text("⚠ a broad grant: your home, the whole disk, or your keys and settings").size(13.0).role(Role::Danger));
        }
    }
    lines.push(space(6.0));
    lines.push(row([primary(ALLOW, "Allow"), button(DENY, "Deny")]).gap(8.0));
    lines.push(muted("remembered for this build; a new build asks again").size(11.0));
    column(lines).gap(4.0).padding(wasm_mods_protocol::Pad::M)
}

/// The answer given in the [`prompt`] that produced `events`, if any.
pub fn prompt_answer(events: &[Event]) -> Option<bool> {
    events.iter().find_map(|e| match e {
        Event::Clicked { id } if id == ALLOW => Some(true),
        Event::Clicked { id } if id == DENY => Some(false),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> ModuleId {
        ModuleId { name: name.into() }
    }

    #[test]
    fn capabilities_parse_and_folders_are_checked() {
        assert_eq!(Capability::parse("fs.read:~/notes"), Capability::FsRead("~/notes".into()));
        assert_eq!(Capability::parse(" git "), Capability::Git);
        for bad in ["fs.read:", "fs.read:notes", "fs.read:~/a/../..", "fs.write:/tmp/\u{202e}x", "fs.read:~/a\u{200b}"] {
            assert!(matches!(Capability::parse(bad), Capability::Unavailable(_)), "{bad}");
        }
        assert!(matches!(Capability::parse("net:example.org"), Capability::Unavailable(_)));
        let home = Path::new("/home/u");
        assert!(broad_dir("~", Some(home)) && broad_dir("/", Some(home)) && broad_dir("~/.ssh/x", Some(home)));
        assert!(!broad_dir("~/notes", Some(home)));
    }

    #[test]
    fn grants_hold_for_one_build_and_denials_stick() {
        let mut g = Grants::default();
        let notes = Capability::FsWrite("~/notes".into());
        assert_eq!(g.decide(&id("todo"), "aa", &notes), Decision::Ask);
        g.set(&id("todo"), "aa", &notes, true);
        assert_eq!(g.decide(&id("todo"), "aa", &notes), Decision::Granted);
        // A new build is asked again.
        assert_eq!(g.decide(&id("todo"), "bb", &notes), Decision::Ask);
        // Another module has nothing.
        assert_eq!(g.decide(&id("other"), "aa", &notes), Decision::Ask);
        g.set(&id("todo"), "bb", &notes, false);
        assert_eq!(g.decide(&id("todo"), "aa", &notes), Decision::Denied);
        assert_eq!(g.decide(&id("todo"), "cc", &Capability::Unavailable("net:x".into())), Decision::Denied);
        // It survives the file.
        let text = toml::to_string(&g).unwrap();
        assert_eq!(toml::from_str::<Grants>(&text).unwrap(), g);
    }

    #[test]
    fn the_prompt_shows_the_module_words_on_one_line() {
        assert_eq!(one_line("a\u{202e}b\nAllow everything", 40), "ab");
        assert_eq!(one_line(&"y".repeat(10), 5), "yyyy…");
        assert_eq!(prompt_answer(&[Event::Clicked { id: ALLOW.into() }]), Some(true));
        assert_eq!(prompt_answer(&[Event::Clicked { id: "other".into() }]), None);
    }
}
