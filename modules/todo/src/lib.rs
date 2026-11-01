//! todo: a list of things to do, kept in a file. Type a thing and press Enter (or
//! "add"); open a row (Enter, a double-click) to tick it off or back on; × removes it.
//! The list lives in `~/.local/share/wasm-mods/todo/todo.json`, read and written through
//! the host: the mod asks for that one folder (`fs.read`, `fs.write`) and the user is
//! asked before it gets it.
//!
//! Build:   cargo build --release --target wasm32-unknown-unknown   (in modules/)
//! Install: ../scripts/install.sh todo

use wasm_mods_sdk::*;

const DIR: &str = "~/.local/share/wasm-mods/todo";
const FILE: &str = "~/.local/share/wasm-mods/todo/todo.json";
/// The list's id: its events (a row chosen or opened) carry it.
const LIST: &str = "todos";
const FIELD: &str = "new";

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Todo {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// Where the file stands.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
enum Disk {
    /// Not read yet: the first call asks for it.
    #[default]
    Unread,
    Reading,
    /// Read (or not there yet), and every change since saved or on its way.
    Ready,
    Failed(String),
}

/// The mod's state. The host keeps it across a reload of a new build (it's serialized),
/// so give fields added later `#[serde(default)]`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct TodoMod {
    todos: Vec<Todo>,
    /// The list's selection: moved with ↑/↓ (`Selected`); Enter or a double-click on a
    /// row is `Activated`.
    selected: usize,
    /// The text field's text: the host keeps it while the user types and sends each change.
    new: FieldState,
    disk: Disk,
    /// Saves on their way (answered `Done` or an error).
    saving: u32,
}

impl TodoMod {
    fn add(&mut self) {
        let text = self.new.text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.todos.push(Todo { text, done: false });
        self.selected = self.todos.len() - 1;
        self.new.clear();
    }

    /// Asks the host to write the list (making its folder first, the first time).
    fn save(&mut self, out: &mut Output) {
        let json = serde_json::to_vec_pretty(&self.todos).unwrap_or_default();
        out.effects.push(fs::create_dir("mkdir", DIR));
        fs::write(out, "save", FILE, &json);
        self.saving += 1;
    }

    /// Takes in the file's answers.
    fn read_answers(&mut self, input: &Input) {
        match input.fs_result("load") {
            Some(FsReply::Bytes { data, .. }) => match serde_json::from_slice(input.blob(*data)) {
                Ok(todos) => {
                    self.todos = todos;
                    self.disk = Disk::Ready;
                }
                Err(e) => self.disk = Disk::Failed(format!("{FILE} isn't a todo list: {e}")),
            },
            // Nothing saved yet: an empty list.
            Some(FsReply::Error(e)) if e.starts_with("no such file") => self.disk = Disk::Ready,
            Some(FsReply::Error(e)) => self.disk = Disk::Failed(e.clone()),
            _ => {}
        }
        for id in ["mkdir", "save"] {
            match input.fs_result(id) {
                Some(FsReply::Error(e)) => self.disk = Disk::Failed(e.clone()),
                Some(_) if id == "save" => self.saving = self.saving.saturating_sub(1),
                _ => {}
            }
        }
    }
}

impl Frame for TodoMod {
    fn update(&mut self, input: &Input) -> Output {
        self.read_answers(input);
        let before = self.todos.clone();
        // The field's text first: Enter in it comes after the typing.
        self.new.handle(&input.events, FIELD);
        for event in &input.events {
            match event {
                Event::Clicked { id } if id == "add" => self.add(),
                Event::Submitted { id } if id == FIELD => self.add(),
                Event::Clicked { id } if id.starts_with("remove/") => {
                    if let Some(i) = id["remove/".len()..].parse::<usize>().ok().filter(|i| *i < self.todos.len()) {
                        self.todos.remove(i);
                        self.selected = self.selected.min(self.todos.len().saturating_sub(1));
                    }
                }
                Event::Selected { id, index } if id == LIST && *index < self.todos.len() => self.selected = *index,
                Event::Activated { id, index } if id == LIST && *index < self.todos.len() => {
                    self.todos[*index].done = !self.todos[*index].done;
                }
                Event::Action(Action::Primary) if self.selected < self.todos.len() => {
                    let t = &mut self.todos[self.selected];
                    t.done = !t.done;
                }
                _ => {}
            }
        }

        let mut out = Output::new("todo", self.view(&input.theme));
        match self.disk {
            Disk::Unread => {
                out.effects.push(fs::read_file("load", FILE));
                self.disk = Disk::Reading;
            }
            Disk::Ready if self.todos != before => self.save(&mut out),
            _ => {}
        }
        out.action(Action::Open, "tick off").action(Action::Move(Move::Down), "choose")
    }
}

impl TodoMod {
    fn view(&self, t: &Theme) -> Node {
        // Every colour comes from the theme, so herdr-app's themes apply here too.
        let items: Vec<ListItem> = self
            .todos
            .iter()
            .enumerate()
            .map(|(i, todo)| ListItem {
                label: todo.text.clone(),
                detail: if todo.done { "done".into() } else { String::new() },
                dot: Some(if todo.done { t.accent } else { t.muted }),
                ring: !todo.done,
                trailing: Some(Trailing { id: format!("remove/{i}"), text: "×".into() }),
                ..Default::default()
            })
            .collect();
        let done = self.todos.iter().filter(|t| t.done).count();
        let total = self.todos.len();
        let status = match &self.disk {
            Disk::Unread | Disk::Reading => muted("reading the list…"),
            Disk::Failed(e) => text(e.clone()).role(Role::Danger),
            Disk::Ready if self.saving > 0 => muted("saving…"),
            Disk::Ready => muted(format!("saved in {FILE}")).size(11.0),
        };
        let list_or_hint = if total == 0 {
            muted("nothing to do yet: type something above and press Enter")
        } else {
            list(LIST, items, Some(self.selected.min(total - 1)))
        };
        // Blocks top to bottom at their natural height: the pane scrolls when they're taller.
        column([
            heading("todo"),
            row([self.new.view(FIELD).placeholder("something to do").width(Size::Fill), primary("add", "add")])
                .gap(8.0)
                .wrap(false),
            list_or_hint,
            progress(if total == 0 { 0.0 } else { done as f32 / total as f32 }, t.accent, t.surface),
            muted(format!("{done} of {total} done")),
            status,
        ])
        .gap(10.0)
    }
}

export_frame!(TodoMod);
// What it needs: the one folder its list lives in. The host asks the user to allow it
// before the mod runs, again after every rebuild.
manifest!(
    description: "a list of things to do, kept in a file",
    capabilities: ["fs.read:~/.local/share/wasm-mods/todo", "fs.write:~/.local/share/wasm-mods/todo"]
);

// A plain Rust crate too: `cargo test` runs these natively.
#[cfg(test)]
mod tests {
    use super::*;

    fn call(m: &mut TodoMod, events: Vec<Event>) -> Output {
        m.update(&Input { events, ..Default::default() })
    }

    #[test]
    fn reads_adds_ticks_off_and_saves() {
        let mut m = TodoMod::default();
        let first = call(&mut m, vec![]);
        assert!(matches!(&first.effects[..], [Effect::Fs { id, op: FsOp::ReadFile { .. } }] if id == "load"));
        // No file yet: an empty list, nothing written.
        let out = m.update(&Input {
            data: HostData {
                fs: vec![FsResult { id: "load".into(), reply: FsReply::Error("no such file or folder: x".into()) }],
                ..Default::default()
            },
            ..Default::default()
        });
        assert!(out.effects.is_empty() && m.disk == Disk::Ready);
        let out = call(
            &mut m,
            vec![Event::Edited { id: FIELD.into(), value: "water the plants".into(), revision: 0 }, Event::Submitted {
                id: FIELD.into(),
            }],
        );
        assert_eq!(m.todos, [Todo { text: "water the plants".into(), done: false }]);
        assert!(m.new.text.is_empty());
        assert!(out.effects.iter().any(|e| matches!(e, Effect::Fs { op: FsOp::Write { .. }, .. })));
        call(&mut m, vec![Event::Activated { id: LIST.into(), index: 0 }]);
        assert!(m.todos[0].done);
        let out = call(&mut m, vec![Event::Clicked { id: "remove/0".into() }]);
        assert!(m.todos.is_empty());
        assert!(format!("{:?}", out.ui).contains("0 of 0 done"));
    }
}
