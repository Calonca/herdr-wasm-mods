//! Files and the other host calls a module can make: filesystem ops and replies, watch
//! events, tool (git) results, and what access a module was granted.

use super::*;

/// A file request the host carries out for a module ([`Effect::Fs`]). Paths are absolute or
/// start with `~/`; each is allowed only inside a folder the module was granted
/// (`fs.read:<dir>` for reading, listing, stat and watching; `fs.write:<dir>` for
/// writing, making folders and deleting) or a file or folder the user picked for it. No
/// symlinks are followed below a granted folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FsOp {
    /// Opens a file for reading: [`FsReply::Opened`] with a handle for ranged and line
    /// reads (the path is checked once, here). The handle stops working when the access
    /// it was opened under is taken back, or the module closes or reloads.
    Open { path: String },
    Close { handle: u64 },
    /// [`FsReply::Stat`]: kind, size, modification time (a symlink is reported, not followed).
    Stat { path: String },
    StatHandle { handle: u64 },
    /// [`FsReply::Bytes`]: at most 4 MB from `offset` (fewer at the end of the file).
    ReadRange { handle: u64, offset: u64, len: u32 },
    /// [`FsReply::Lines`]: up to 2000 lines from line `start` (0-based), from a line index
    /// the host builds in the background when the file is first asked for lines.
    ReadLines { handle: u64, start: u64, count: u32 },
    /// A whole file, at most 8 MB: [`FsReply::Bytes`].
    ReadFile { path: String },
    /// A folder's entries, at most 5000: [`FsReply::Entries`].
    List { path: String },
    /// Replaces (or creates) a file with [`Output::bytes`] `data` (at most 8 MB),
    /// atomically: a temporary file in the same folder, then renamed over it.
    Write { path: String, data: Blob },
    /// Makes a folder and any missing parents (inside the granted folder).
    CreateDir { path: String },
    /// Deletes a file or an empty folder.
    Delete { path: String },
    /// Tells the module what changes in a file or folder it may read, as
    /// [`HostData::fs_events`] under the request's `id`, from the next calls on, until
    /// [`FsOp::Unwatch`] with that id. Folders: only their own entries (not recursive).
    Watch { path: String },
    WatchHandle { handle: u64 },
    Unwatch,
    /// The host's own file picker: the user chooses a file, which the module may then read
    /// ([`FsReply::Picked`] or [`FsReply::Cancelled`]). `extensions` (`["md", "txt"]`)
    /// narrow what it lists; empty: everything.
    PickFile {
        #[serde(default)]
        extensions: Vec<String>,
    },
    /// The user chooses a folder, which the module may then list and read inside.
    PickFolder,
    /// The user chooses where to save one file (suggested name `suggested`), which the
    /// module may then write.
    PickSave {
        #[serde(default)]
        suggested: String,
    },
}

/// An answer to an [`Effect::Fs`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsResult {
    /// The effect's `id`.
    pub id: String,
    pub reply: FsReply,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FsReply {
    /// Refused or failed, in plain words (`not allowed: …`, `no such file`, `too big`).
    Error(String),
    /// Written, made, deleted, closed or unwatched.
    Done,
    Opened { handle: u64, stat: FsStat },
    Stat(FsStat),
    /// `data` (in [`Input::bytes`]) starts at `offset`; `eof`: nothing follows it.
    Bytes { offset: u64, data: Blob, eof: bool },
    /// Lines `start..start + lines.len()`, without their line ends (`\n` or `\r\n`);
    /// invalid UTF-8 replaced by `�`. A line longer than 16 KB is cut there and listed in
    /// `cut`. While the index isn't as far as `start` yet, `lines` is empty and `indexed`
    /// (0..1) says how far it got: ask again. `total` is the file's line count once known.
    Lines {
        start: u64,
        lines: Vec<String>,
        #[serde(default)]
        cut: Vec<u32>,
        total: Option<u64>,
        indexed: f32,
    },
    Entries { entries: Vec<DirEntry>, truncated: bool },
    /// What the user picked: `path` (`~/…` under the home folder) works in later
    /// requests; `kind` says file or folder, `access` `read` or `write`.
    Picked { path: String, kind: FsKind, access: String },
    /// The picker was closed without a choice.
    Cancelled,
    /// A watch is set up; what it watches is like this now.
    Watching(FsStat),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsKind {
    #[default]
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FsStat {
    pub kind: FsKind,
    pub size: u64,
    /// Seconds since 1970 (UTC).
    pub modified: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub stat: FsStat,
}

/// Something that happened to a watched file or folder (bursts within about 100 ms come
/// as one event).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FsWatchEvent {
    /// The [`Effect::Fs`] id the watch was set up under.
    pub id: String,
    pub change: FsChange,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FsChange {
    /// The watched file's contents or size changed: from `old_size` to `new_size` (a
    /// growing log: read `old_size..new_size`).
    Changed { old_size: u64, new_size: u64, modified: Option<f64> },
    /// An entry of the watched folder appeared, went away or was renamed.
    Created { name: String },
    Deleted { name: String },
    Renamed { from: String, to: String },
    /// The watched file or folder itself is gone; the watch ends.
    Removed,
}

/// How an [`Effect::RunTool`] went.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The effect's `tool` (`git`).
    pub tool: String,
    /// The effect's `id`.
    pub id: String,
    /// The exit code; `None` when the tool didn't run or didn't finish (unknown tool,
    /// not allowed, refused command, not installed, timed out: `stderr` says which).
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl ToolResult {
    pub fn ok(&self) -> bool {
        self.status == Some(0)
    }
}

/// An installed module and what it may do (`wasm-mods-host --permissions`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleAccess {
    pub name: String,
    /// `wasm-mods`.
    pub origin: String,
    pub description: String,
    pub version: String,
    /// Fingerprint of the `.wasm` build.
    pub hash: String,
    /// Built with a manifest; without one, capabilities show up as it asks for them.
    pub has_manifest: bool,
    pub capabilities: Vec<CapabilityAccess>,
    /// The protocol it was built for (the host runs only its own: [`PROTOCOL_VERSION`]).
    #[serde(default)]
    pub protocol: u32,
    /// Not turned off (on the permissions screen, or `wasm-mods-host --disable`): it loads.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// It may be turned off.
    #[serde(default)]
    pub can_disable: bool,
    /// It may be removed (`wasm-mods-host --remove`).
    #[serde(default)]
    pub removable: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapabilityAccess {
    /// e.g. `git`, `fs.read:~/notes`.
    pub id: String,
    /// In plain words.
    pub about: String,
    /// `granted`, `denied` or `ask`.
    pub state: String,
    /// The host can provide it today.
    pub available: bool,
    /// The user may change it: deny it, or have it asked again (it's allowed only from
    /// the module's own Allow prompt, for that build).
    pub changeable: bool,
}
