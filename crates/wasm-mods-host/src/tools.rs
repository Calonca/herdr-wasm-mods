//! Command-line tools a module may run ([`Effect::RunTool`]), each under its own
//! capability, in the folder of the tab the module is shown in ([`Input::window`]):
//! `git`, reading only. No shell: the arguments go to the program as they are.
//!
//! A tab on another machine (a herdr space's machine) has its folder there, so git
//! runs there: through `hc exec --on <machine>`, which hands the program and its
//! arguments over as they are (no shell on either side), under the same refusals,
//! forced options, environment, time limit and output limit as here.
//!
//! git runs a fixed set of subcommands that only read (`log`, `show`, `diff`, …), with
//! the options that would write a file, run another program or read outside the repo
//! refused (`--output`, `--ext-diff`, `--textconv`, `--no-index`, `--git-dir`, …), and
//! with external diff drivers, text conversion, fsmonitor and the pager turned off
//! whatever the user's configuration says. Each run gets a thread, a time limit and an
//! output limit; its answer comes back in [`HostData::tool_runs`] in a later call.
//!
//! [`Effect::RunTool`]: wasm_mods_protocol::Effect::RunTool
//! [`Input::window`]: wasm_mods_protocol::Input::window
//! [`HostData::tool_runs`]: wasm_mods_protocol::HostData::tool_runs

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use wasm_mods_protocol::ToolResult;

/// The longest a run may take.
const TIME_MAX: Duration = Duration::from_secs(30);
/// The most output kept (stdout; stderr gets a sixteenth of it).
const OUTPUT_MAX: usize = 16 << 20;
/// Runs a session may have going at once.
pub const RUNNING_MAX: usize = 4;

/// git subcommands that only read.
const GIT_READS: &[&str] = &[
    "log", "show", "diff", "rev-parse", "rev-list", "for-each-ref", "merge-base", "cat-file", "status", "ls-files",
    "ls-tree", "describe", "blame", "shortlog", "name-rev", "show-ref", "count-objects",
];

/// Options refused anywhere in the arguments (and their `=value` forms): they write
/// files, run programs, read outside the repo, or point git at another one.
const GIT_REFUSED: &[&str] = &[
    "--output", "--ext-diff", "--textconv", "--filters", "--exec", "--upload-pack", "--receive-pack", "--git-dir",
    "--work-tree", "--namespace", "--super-prefix", "--config-env", "--open-files-in-pager", "--no-index",
    "--contents", "--output-directory", "--batch-command",
];

/// Why `args` (a subcommand and its arguments) may not run, if not.
pub fn git_refusal(args: &[String]) -> Option<String> {
    let Some(sub) = args.first() else { return Some("git with nothing to do".into()) };
    if !GIT_READS.contains(&sub.as_str()) {
        return Some(format!("git {sub} isn't one of the commands a module may run (they only read: {})", GIT_READS.join(", ")));
    }
    for a in &args[1..] {
        if a.contains('\0') {
            return Some("an argument holds a NUL".into());
        }
        // git takes any unambiguous start of a long option (`--outp` is `--output`).
        let name = a.split('=').next().unwrap_or(a);
        if name.len() > 2 && name.starts_with("--") && GIT_REFUSED.iter().any(|r| r.starts_with(name)) {
            return Some(format!("git {sub} {name} isn't allowed here"));
        }
    }
    None
}

/// Where git runs: a folder on this computer, or one on a saved herdr machine.
#[derive(Clone, Debug, PartialEq)]
pub enum Place {
    Here(PathBuf),
    /// `folder` as the machine names it (`~` is that machine's home).
    On { machine: String, folder: String },
}

/// git's options before the subcommand, the same wherever it runs: no pager, no
/// fsmonitor (a program the repo's config could name), no optional locks.
const GIT_FIRST: &[&str] = &["--no-pager", "-c", "core.fsmonitor=false", "-c", "core.pager=cat", "--no-optional-locks"];

/// The subcommand and its arguments as git gets them: `log`, `show` and `diff` with
/// external diff drivers and text conversion off (programs the repo's config could name).
fn git_args(args: &[String]) -> Vec<String> {
    let mut full: Vec<String> = Vec::with_capacity(args.len() + 2);
    full.push(args[0].clone());
    if matches!(args[0].as_str(), "log" | "show" | "diff") {
        full.extend(["--no-ext-diff".into(), "--no-textconv".into()]);
    }
    full.extend(args[1..].iter().cloned());
    full
}

/// The command that runs `git args` (already checked by [`git_refusal`]) at `place`.
fn git_command(place: &Place, args: &[String]) -> Command {
    match place {
        Place::Here(folder) => {
            let mut cmd = Command::new("git");
            cmd.args(GIT_FIRST)
                .args(git_args(args))
                .current_dir(folder)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .env_remove("GIT_DIR")
                .env_remove("GIT_WORK_TREE");
            cmd
        }
        Place::On { machine, folder } => {
            // hc runs the program with the arguments as given, no shell, in the
            // folder (a relative one from that machine's home), ending it and all it
            // started at the time limit. The environment is set by `env` there: hc
            // can add variables but not remove them. `--on=`/`--cwd=` keep a name
            // starting with `-` from reading as an option.
            let cwd = match folder.as_str() {
                "~" => ".",
                f => f.strip_prefix("~/").unwrap_or(f),
            };
            let mut cmd = Command::new(hc_program());
            cmd.args(["exec", &format!("--on={machine}"), &format!("--cwd={cwd}"), &format!("--timeout={}", TIME_MAX.as_secs()), "--"])
                .args(["env", "-u", "GIT_DIR", "-u", "GIT_WORK_TREE", "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0", "git"])
                .args(GIT_FIRST)
                .args(git_args(args));
            cmd
        }
    }
}

/// herdr-core's `hc`: where its installer puts it, else on PATH.
fn hc_program() -> PathBuf {
    crate::fs::home().map(|h| h.join(".local/bin/hc")).filter(|p| p.is_file()).unwrap_or_else(|| "hc".into())
}

/// Starts `git args` at `place`; the answer (under `id`) arrives on the receiver.
pub fn run_git(id: String, place: Place, args: Vec<String>) -> mpsc::Receiver<ToolResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let failed = |stderr: String| ToolResult { tool: "git".into(), id: id.clone(), status: None, stdout: String::new(), stderr };
        let result = match git_refusal(&args) {
            Some(why) => failed(why),
            None => {
                let mut cmd = git_command(&place, &args);
                cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
                match cmd.spawn() {
                    Err(e) if matches!(place, Place::On { .. }) => failed(format!("hc (herdr-core) didn't start: {e}")),
                    Err(e) => failed(format!("git didn't start: {e}")),
                    Ok(child) => {
                        let mut r = finish(child, &id);
                        if let Place::On { machine, .. } = &place {
                            // hc's own failure (the machine unreachable, no such folder)
                            // is a JSON error on stderr and nothing on stdout: not git's.
                            if r.stdout.is_empty() && r.stderr.trim_start().starts_with("{\"error\"") {
                                let why = serde_json::from_str::<serde_json::Value>(r.stderr.trim())
                                    .ok()
                                    .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                                    .unwrap_or_else(|| r.stderr.trim().to_owned());
                                r.status = None;
                                r.stderr = format!("git on {machine}: {why}");
                            }
                        }
                        r
                    }
                }
            }
        };
        let _ = tx.send(result);
    });
    rx
}

/// Waits for `child` (up to [`TIME_MAX`]) and reads what it wrote (up to the limits).
fn finish(mut child: std::process::Child, id: &str) -> ToolResult {
    let read = |pipe: Option<Box<dyn Read + Send>>, max: usize| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut cut = false;
            if let Some(mut p) = pipe {
                let mut buf = [0u8; 64 << 10];
                while let Ok(n) = p.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let room = max.saturating_sub(out.len());
                    out.extend_from_slice(&buf[..n.min(room)]);
                    cut |= n > room;
                }
            }
            (out, cut)
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), OUTPUT_MAX);
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), OUTPUT_MAX / 16);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code(),
            Ok(None) if started.elapsed() > TIME_MAX => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let (out, out_cut) = stdout.join().unwrap_or_default();
    let (err, _) = stderr.join().unwrap_or_default();
    let mut stderr = String::from_utf8_lossy(&err).into_owned();
    if status.is_none() && started.elapsed() > TIME_MAX {
        stderr.push_str(&format!("\ngit took longer than {} s and was stopped", TIME_MAX.as_secs()));
    }
    if out_cut {
        stderr.push_str(&format!("\nthe output was cut at {} MB", OUTPUT_MAX >> 20));
    }
    ToolResult { tool: "git".into(), id: id.into(), status, stdout: String::from_utf8_lossy(&out).into_owned(), stderr }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(cmd: &Command) -> Vec<String> {
        std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn on_another_machine_git_runs_there_with_the_same_options_and_no_shell() {
        let on = Place::On { machine: "-devbox".into(), folder: "~/src/repo".into() };
        let a = argv(&git_command(&on, &args("log --format=%s;rm -rf ~ $(x)")));
        let tail: Vec<&str> = a[1..].iter().map(String::as_str).collect();
        assert_eq!(tail, [
            "exec", "--on=-devbox", "--cwd=src/repo", "--timeout=30", "--",
            "env", "-u", "GIT_DIR", "-u", "GIT_WORK_TREE", "GIT_TERMINAL_PROMPT=0", "GIT_OPTIONAL_LOCKS=0",
            "git", "--no-pager", "-c", "core.fsmonitor=false", "-c", "core.pager=cat", "--no-optional-locks",
            "log", "--no-ext-diff", "--no-textconv", "--format=%s;rm", "-rf", "~", "$(x)",
        ]);
        let home = Place::On { machine: "m".into(), folder: "~".into() };
        assert!(argv(&git_command(&home, &args("status"))).contains(&"--cwd=.".to_string()));
        let abs = Place::On { machine: "m".into(), folder: "/srv/repo".into() };
        assert!(argv(&git_command(&abs, &args("status"))).contains(&"--cwd=/srv/repo".to_string()));
    }

    fn args(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn git_reads_only_and_runs_in_the_folder() {
        for ok in ["log --all --format=%H", "diff -U3 --no-color a b --", "show -s HEAD", "cat-file -s HEAD:a"] {
            assert_eq!(git_refusal(&args(ok)), None, "{ok}");
        }
        for bad in ["commit -m x", "push", "config user.name x", "log --output=/tmp/x", "log --outp=/tmp/x", "diff --ext", "diff --ext-diff", "diff --no-index /etc/passwd x", "log --git-dir=/x", "branch -D main", "checkout ."] {
            assert!(git_refusal(&args(bad)).is_some(), "{bad}");
        }
        let dir = std::env::temp_dir().join(format!("wasm-mods-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |a: &str| Command::new("git").args(args(a)).current_dir(&dir).output().unwrap();
        git("init -q");
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git("add a.txt");
        git("-c user.name=t -c user.email=t@t commit -q -m first");
        let r = run_git("x".into(), Place::Here(dir.clone()), args("log --format=%s")).recv().unwrap();
        assert_eq!((r.status, r.stdout.as_str()), (Some(0), "first\n"));
        let r = run_git("y".into(), Place::Here(dir.clone()), args("push")).recv().unwrap();
        assert!(r.status.is_none() && r.stderr.contains("isn't one of"));
        // Refused the same on another machine: nothing is started.
        let on = Place::On { machine: "elsewhere".into(), folder: "~/repo".into() };
        let r = run_git("z".into(), on, args("log --output=/tmp/x")).recv().unwrap();
        assert!(r.status.is_none() && r.stderr.contains("isn't allowed"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
