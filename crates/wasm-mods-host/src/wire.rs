//! The host's side of the wire ([`wasm_mods_protocol::wire`]): what clients send is the
//! protocol's [`In`]; what the host sends is [`Out`], the protocol's
//! [`wire::Message`](wasm_mods_protocol::wire::Message) written from borrowed parts (what
//! a module shows isn't copied to be sent).

use serde::Serialize;
pub use wasm_mods_protocol::wire::{In, Shown, module_name_ok};
use wasm_mods_protocol::{Node, Offer};

use std::collections::BTreeMap;

/// Host → client.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Out<'a> {
    /// The module loaded: its name and description, and how long loading took.
    Ready { name: &'a str, description: &'a str, load_ms: f64 },
    /// What the module shows now. `ack`: the last `seq` this answers; `call_ms`: the time in
    /// the module (all calls this answer took); `got`: when the host read the events it
    /// answers, `sent`: when it wrote this (milliseconds since the epoch: where a round
    /// trip's time goes).
    /// `images`: each picture's key → its file in the mod's folder and size.
    Frame {
        title: &'a str,
        ui: &'a Node,
        actions: &'a [Offer],
        ack: u64,
        call_ms: f64,
        got: f64,
        sent: f64,
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        images: &'a BTreeMap<String, Shown>,
    },
    /// Give a text field the keyboard.
    Focus { id: &'a str },
    /// Open an http(s) link (asked in answer to the user's own click or key).
    OpenUrl { url: &'a str },
    /// Close the module's pane.
    Close,
    /// Open module `module` with `arg` in a new tab.
    Open { module: &'a str, arg: &'a str },
    /// The module can't run (or stopped): why.
    Error { message: &'a str },
}

/// Milliseconds since the epoch.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1e3).unwrap_or(0.0)
}

/// One line of JSON for `out`.
pub fn line(out: &Out) -> Vec<u8> {
    let mut v = serde_json::to_vec(out).unwrap_or_default();
    v.push(b'\n');
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_parse() {
        let hello: In = serde_json::from_str(r#"{"hello":{"token":"t","module":"todo","open":"~/a.md"}}"#).unwrap();
        assert!(matches!(hello, In::Hello { module, .. } if module == "todo"));
        let ev: In =
            serde_json::from_str(r#"{"events":{"seq":4,"events":[{"Clicked":{"id":"add"}},{"Nope":1}]}}"#).unwrap();
        assert!(matches!(ev, In::Events { seq: 4, events, .. } if events.len() == 2));
        assert!(serde_json::from_str::<In>(r#"{"size":{"width":"wide"}}"#).is_err());
        for bad in ["", "../todo", "/tmp/x", "a/b", "Todo", "x.wasm", "a b", &"x".repeat(65)] {
            assert!(!module_name_ok(bad), "{bad}");
        }
        assert!(module_name_ok("my-todo") && module_name_ok("todo_2"));
        let out = String::from_utf8(line(&Out::Close)).unwrap();
        assert_eq!(out, "\"close\"\n");
        // What the host writes is what a client reads as the protocol's Message.
        use wasm_mods_protocol::wire::Message;
        let images = BTreeMap::from([("canvas:w".to_owned(), Shown { file: "a.svg".into(), width: 2, height: 3 })]);
        let ui = wasm_mods_protocol::ui::text("hi");
        let sent = [
            Out::Ready { name: "todo", description: "d", load_ms: 1.5 },
            Out::Frame { title: "t", ui: &ui, actions: &[], ack: 3, call_ms: 0.5, got: 1.0, sent: 2.0, images: &images },
            Out::Focus { id: "f" },
            Out::Error { message: "no" },
        ];
        let read: Vec<Message> = sent.iter().map(|o| serde_json::from_slice(&line(o)).unwrap()).collect();
        assert!(matches!(&read[0], Message::Ready { name, .. } if name == "todo"));
        assert!(matches!(&read[1], Message::Frame { ack: 3, ui: u, images: i, .. } if *u == ui && *i == images));
        assert!(matches!(&read[3], Message::Error { message } if message == "no"));
    }
}
