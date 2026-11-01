//! module → the host: what a module returns ([`Output`]): its tree, what it wants next, and
//! the [`Effect`]s the host carries out.

use super::*;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// Shown in the window header.
    pub title: String,
    pub ui: Node,
    pub effects: Vec<Effect>,
    /// Host data to include in the next [`Input`].
    pub wants: Wants,
    /// The actions the module offers now, each with what it does here ("details",
    /// "close"): the host draws them as hints under the window, in the glyphs of the input
    /// the user last used (keys or gamepad buttons), and a click on one sends it. Offer
    /// only what does something right now; the order is the host's (back, open, primary,
    /// secondary, move, then keys).
    #[serde(default)]
    pub actions: Vec<Offer>,
    /// Raw bytes for [`FsOp::Write`]'s [`Blob`]s, passed beside the JSON.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl Output {
    pub fn new(title: impl Into<String>, ui: Node) -> Self {
        Self { title: title.into(), ui, ..Default::default() }
    }

    /// Offers `action`, labelled `label` (see [`Output::actions`]).
    pub fn action(mut self, action: Action, label: impl Into<String>) -> Self {
        self.actions.push(Offer { action, label: label.into(), key: None });
        self
    }

    /// Offers one of the module's own keys: `key`, a plain key on the keyboard (`"n"`,
    /// `"f2"`; not with Ctrl, Alt or Shift), sends [`Action::Key`] with `name` while the
    /// module's pane has the keyboard and no text field in it is being typed in. Offer
    /// at most [`MAX_KEYS`], and never a key the host keeps for the actions (Escape,
    /// Enter, Space, Tab, Backspace, the arrows, PageUp/PageDown, Home/End, j/k).
    pub fn key(mut self, name: impl Into<String>, key: impl Into<String>, label: impl Into<String>) -> Self {
        self.actions.push(Offer { action: Action::Key(name.into()), label: label.into(), key: Some(key.into()) });
        self
    }

    pub fn effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    pub fn wants_window(mut self) -> Self {
        self.wants.window = true;
        self
    }

    /// Appends `data` to [`Output::bytes`] and returns where it went.
    pub fn add_bytes(&mut self, data: &[u8]) -> Blob {
        let start = self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        Blob { start, len: data.len() as u32 }
    }
}

/// How many keys of its own a module may offer ([`Output::key`]).
pub const MAX_KEYS: usize = 5;

/// An action a module offers ([`Output::actions`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub action: Action,
    /// What it does here, a word or two.
    pub label: String,
    /// For [`Action::Key`]: the keyboard key (`"n"`, `"f2"`).
    #[serde(default)]
    pub key: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wants {
    /// [`Input::window`] (capability `window.folder`).
    #[serde(default)]
    pub window: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    /// Close this module.
    Close,
    /// Run `tool` with `args` (no shell) in the window's folder. Only `git` runs
    /// (capability `git`), and only commands that read: history, changes, files at a
    /// commit. The result comes back in [`HostData::tool_runs`] under `tool` and `id`,
    /// in a later call; an unknown or not-allowed tool is answered with an error there.
    RunTool { tool: String, id: String, args: Vec<String> },
    /// Show the host's diff viewer for one commit (full or abbreviated hash) of the
    /// window's repo, in this module's place; back returns to the module (capability
    /// `window.folder`).
    OpenDiff { commit: String },
    /// A file request or picker (see [`FsOp`]); the answer comes back in
    /// [`HostData::fs`] under `id`, in a later call.
    Fs { id: String, op: FsOp },
    /// Give the keyboard to the [`Node::TextField`] with this id.
    Focus { id: String },
    /// Open an `http(s)` link, as a click on it would (with the module that handles it,
    /// or the browser). In answer to the user's click or key in the same call it opens
    /// at once; otherwise the host asks the user first, showing the whole URL (no
    /// capability needed).
    OpenUrl { url: String },
}
