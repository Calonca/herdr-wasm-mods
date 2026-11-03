use super::*;

const DIFF: &str = "diff --git a/src/list.rs b/src/list.rs\n\
                    index 1..2 100644\n\
                    --- a/src/list.rs\n\
                    +++ b/src/list.rs\n\
                    @@ -3,4 +3,5 @@ fn list_recipes() {\n\
                    \x20   let tags = load();\n\
                    -    show(tags);\n\
                    +    show(sorted);\n\
                    +    count(tags);\n\
                    \x20}\n\
                    \\ No newline at end of file\n";

/// git's status, numstat and diff output read into files, a tree, hunks and rows.
#[test]
fn parses_git_output_into_files_hunks_and_rows() {
    let status = "M\0src/list.rs\0R087\0src/old.rs\0src/new.rs\0A\0assets/cover.png\0";
    let numstat = "2\t1\tsrc/list.rs\0\
                   2\t2\t\0src/old.rs\0src/new.rs\0\
                   -\t-\tassets/cover.png\0";
    let files = parse_files(status, numstat);
    assert_eq!(files[0], FileEntry { path: "src/list.rs".into(), old_path: None, status: 'M', added: Some(2), removed: Some(1) });
    assert_eq!((files[1].status, files[1].old_path.as_deref(), files[1].added), ('R', Some("src/old.rs"), Some(2)));
    assert_eq!((files[2].added, files[2].removed), (None, None));
    // Folders first, by name, each summing its files.
    let tree = tree::build(&files);
    let names: Vec<(&str, usize)> = tree.iter().map(|n| (n.name.as_str(), n.depth)).collect();
    assert_eq!(names, [("assets", 0), ("cover.png", 1), ("src", 0), ("list.rs", 1), ("new.rs", 1)]);

    let d = parse_diff(DIFF);
    assert_eq!((d.hunks.len(), d.hunks[0].ranges.as_str(), d.hunks[0].context.as_str()), (1, "-3,4 +3,5", "fn list_recipes() {"));
    let l = &d.hunks[0].lines;
    assert_eq!(l.len(), 5);
    assert_eq!((l[1].kind, l[1].old, l[1].new), (git::LineKind::Removed, Some(4), None));
    assert_eq!((l[2].kind, l[2].old, l[2].new), (git::LineKind::Added, None, Some(4)));
    assert_eq!((l[4].old, l[4].new), (Some(5), Some(6)));

    // Unified: as git prints it; the removed line and its replacement get the changed word.
    let unified = rows::rows(0, &d, false, None);
    assert_eq!(unified.len(), 6);
    let Row::Unified { cell, .. } = unified[2] else { panic!("{:?}", unified[2]) };
    assert_eq!((cell.line, cell.change.map(|(a, b)| &l[1].text[a..b])), (1, Some("tags")));
    // Side by side: the removed line beside the first added one, the second alone.
    let split = rows::rows(0, &d, true, None);
    assert_eq!(split.len(), 5);
    assert!(matches!(split[3], Row::Split { left: None, right: Some(_), .. }));
    assert!(parse_diff("Binary files a/x.png and b/x.png differ\n").binary);
    assert_eq!(parse_default_branch("refs/heads/main\t\nrefs/remotes/origin/HEAD\torigin/trunk\n").as_deref(), Some("origin/trunk"));
}

fn git_answer(id: &str, stdout: &str) -> ToolResult {
    ToolResult { tool: "git".into(), id: id.into(), status: Some(0), stdout: stdout.into(), stderr: String::new() }
}

fn call(m: &mut DiffMod, events: Vec<Event>, answers: Vec<ToolResult>) -> Output {
    let window = Some(WindowContext { folder: "/src/recipes".into(), ..Default::default() });
    m.update(&Input { events, window, data: HostData { tool_runs: answers, ..Default::default() }, ..Default::default() })
}

/// The git runs an output asks for, by id.
fn runs(out: &Output) -> Vec<(String, Vec<String>)> {
    (out.effects.iter())
        .filter_map(|e| match e {
            Effect::RunTool { tool, id, args } if tool == "git" => Some((id.clone(), args.clone())),
            _ => None,
        })
        .collect()
}

/// Uncommitted changes: HEAD, then the files, then the first file's diff, each asked
/// once; Tab moves to the next file and asks for its diff.
#[test]
fn reads_the_uncommitted_changes_and_shows_a_file() {
    let mut m = DiffMod::default();
    let waiting = m.update(&Input::default());
    // Nothing but the kept toggles asked for until the folder is known.
    assert!(waiting.wants.window && runs(&waiting).is_empty());

    let out = call(&mut m, vec![], vec![]);
    assert_eq!(runs(&out), [("1/head".to_owned(), vec!["rev-parse".into(), "--verify".into(), "--quiet".into(), "HEAD".into()])]);
    // Nothing asked twice while the answer is on its way.
    assert!(runs(&call(&mut m, vec![], vec![])).is_empty());

    let head = "a".repeat(40);
    let out = call(&mut m, vec![], vec![git_answer("1/head", &format!("{head}\n"))]);
    let asked: Vec<String> = runs(&out).into_iter().map(|(id, _)| id).collect();
    assert_eq!(asked, ["1/status", "1/numstat", "1/untracked"]);
    assert!(runs(&out)[0].1.contains(&head) && runs(&out)[0].1.contains(&"--name-status".into()));

    let out = call(&mut m, vec![], vec![
        git_answer("1/status", "M\0src/list.rs\0M\0src/main.rs\0"),
        git_answer("1/numstat", "2\t1\tsrc/list.rs\x001\t0\tsrc/main.rs\0"),
        git_answer("1/untracked", "notes.txt\0"),
    ]);
    let r = runs(&out);
    assert_eq!(r[0].0, "1/diff/src/list.rs");
    assert_eq!(r[0].1.last().map(String::as_str), Some(":(top,literal)src/list.rs"));
    assert!(format!("{:?}", out.ui).contains("notes.txt"));

    let out = call(&mut m, vec![], vec![git_answer("1/diff/src/list.rs", DIFF)]);
    let ui = format!("{:?}", out.ui);
    assert!(ui.contains("fn list_recipes() {") && ui.contains("count") && ui.contains("end of list.rs"), "{ui}");

    let out = call(&mut m, vec![Event::Action(Action::Secondary)], vec![]);
    assert_eq!(m.selected.as_deref(), Some("src/main.rs"));
    assert_eq!(runs(&out)[0].0, "1/diff/src/main.rs");
}

/// Rust and JavaScript lines get their kinds; a line inside a block comment that opened
/// earlier in the hunk is a comment; context lines are on both sides, removed lines only
/// on the old one.
#[test]
fn highlights_a_hunk_per_side() {
    use highlight::Token;
    let d = parse_diff(DIFF);
    let t = highlight::hunk("src/list.rs", &d.hunks[0]);
    let kinds = |tokens: &highlight::Tokens, text: &str| -> Vec<(String, Token)> {
        tokens.iter().map(|&(a, b, k)| (text[a..b].to_owned(), k)).filter(|(s, _)| !s.trim().is_empty()).collect()
    };
    let lines = &d.hunks[0].lines;
    // `    let tags = load();`: `let` (storage.type in the Rust grammar), then a call.
    let first = kinds(t.line(0, git::LineKind::Context).unwrap(), &lines[0].text);
    assert_eq!(first[0], ("let".to_owned(), Token::Type), "{first:?}");
    assert!(first.iter().any(|(s, k)| s == "load" && *k == Token::Function), "{first:?}");
    // The removed line is on the old side only, the added ones on the new.
    assert!(t.old.lines[1].is_some() && t.new.lines[1].is_none() && t.old.lines[2].is_none() && t.new.lines[2].is_some());

    let js = parse_diff("@@ -1,3 +1,3 @@\n /* a comment\n-   still */ const a = 1;\n+   still */ const a = \"x\";\n");
    let t = highlight::hunk("web/app.js", &js.hunks[0]);
    let l = &js.hunks[0].lines;
    let comment = kinds(t.line(0, git::LineKind::Context).unwrap(), &l[0].text);
    assert!(comment.iter().all(|(_, k)| *k == Token::Comment), "{comment:?}");
    let added = kinds(t.line(2, git::LineKind::Added).unwrap(), &l[2].text);
    assert_eq!(added.first().map(|a| a.1), Some(Token::Comment), "{added:?}");
    assert!(added.iter().any(|(s, k)| s.contains("x") && *k == Token::String), "{added:?}");
    assert!(added.iter().any(|(s, k)| s == "const" && *k != Token::Text), "{added:?}");
    // An unknown language: no tokens.
    assert!(highlight::hunk("notes.unknownext", &js.hunks[0]).line(0, git::LineKind::Context).is_none());
}

/// The whole range's diff is cut into each file's part, under the path the file list
/// has: a change, a rename, a deletion, a mode change, a binary file; a quoted path is
/// left to be read on its own, and a part cut short is dropped.
#[test]
fn splits_a_whole_diff_by_file() {
    let out = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-old\n+new\n\
diff --git a/old name.rs b/new name.rs\nsimilarity index 90%\nrename from old name.rs\nrename to new name.rs\n--- a/old name.rs\n+++ b/new name.rs\n@@ -1 +1 @@\n-a\n+b\n\
diff --git a/gone.rs b/gone.rs\ndeleted file mode 100644\n--- a/gone.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n--- a/looks like a header\n\
diff --git a/run b/run\nold mode 100644\nnew mode 100755\n\
diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n\
diff --git \"a/t\\tab.rs\" \"b/t\\tab.rs\"\n--- \"a/t\\tab.rs\"\n+++ \"b/t\\tab.rs\"\n@@ -1 +1 @@\n-x\n+y\n\
diff --git a/last.rs b/last.rs\n--- a/last.rs\n+++ b/last.rs\n@@ -1,3 +1,3 @@\n-cut sho";
    let parts = split_diff(out, false);
    let paths: Vec<Option<&str>> = parts.iter().map(|(p, _)| p.as_deref()).collect();
    assert_eq!(paths, [Some("src/a.rs"), Some("new name.rs"), Some("gone.rs"), Some("run"), Some("logo.png"), None, Some("last.rs")]);
    // Each part parses as that file's diff alone: the deleted file's last line isn't
    // taken for the next file's header, nor the next header for its lines.
    let gone = parse_diff(parts[2].1);
    assert_eq!(gone.hunks[0].lines.len(), 1);
    assert_eq!(gone.hunks[0].lines[0].text, "-- a/looks like a header");
    assert_eq!(parse_diff(parts[0].1).hunks[0].lines.len(), 2);
    assert!(parse_diff(parts[4].1).binary);
    // Cut short: the last part is dropped (that file is read on its own).
    assert_eq!(split_diff(out, true).len(), 6);
}

/// The view toggles kept from the last diff are this one's; a change is written; a
/// toggle the user changed before they were read stays theirs.
#[test]
fn keeps_the_view_toggles() {
    let kept = |out: &Output| -> Option<serde_json::Value> {
        out.effects.iter().find_map(|e| match e {
            Effect::Fs { id, op: FsOp::Write { data, .. } } if id == "settings-save" => {
                let at = data.start as usize;
                serde_json::from_slice(&out.bytes[at..at + data.len as usize]).ok()
            }
            _ => None,
        })
    };
    let answer = |json: &str| {
        let data = Blob { start: 0, len: json.len() as u32 };
        let fs = vec![FsResult { id: "settings".into(), reply: FsReply::Bytes { offset: 0, data, eof: true } }];
        Input { data: HostData { fs, ..Default::default() }, bytes: json.as_bytes().to_vec(), ..Default::default() }
    };
    let mut m = DiffMod::default();
    let out = m.update(&Input::default());
    assert!(out.effects.iter().any(|e| matches!(e, Effect::Fs { id, op: FsOp::ReadFile { .. } } if id == "settings")));
    let out = m.update(&answer(r#"{"side_by_side":true,"wrap":true}"#));
    assert!(m.side_by_side && m.wrap && !m.all_files && kept(&out).is_none(), "read, not written back");
    let out = call(&mut m, vec![Event::Action(Action::Key("wrap".into()))], vec![]);
    assert_eq!(kept(&out), Some(serde_json::json!({"all_files": false, "side_by_side": true, "wrap": false})));

    // Toggled before the file was read: the user's choice stays, and is written.
    let mut m = DiffMod::default();
    let _ = m.update(&Input::default());
    let _ = call(&mut m, vec![Event::Action(Action::Key("all-files".into()))], vec![]);
    let out = m.update(&answer(r#"{"side_by_side":true}"#));
    assert!(m.all_files && !m.side_by_side);
    assert_eq!(kept(&out).map(|k| k["all_files"].clone()), Some(serde_json::json!(true)));
}

/// Wrapping cuts long lines after a space when it can, and each piece takes a row.
#[test]
fn wraps_long_lines_into_rows() {
    assert_eq!(rows::wrap("short", 10), [(0, 5)]);
    let text = "alpha beta gamma delta";
    let pieces: Vec<&str> = rows::wrap(text, 10).into_iter().map(|(a, b)| &text[a..b]).collect();
    assert_eq!(pieces, ["alpha ", "beta ", "gamma ", "delta"]);
    let d = parse_diff("@@ -1 +1 @@\n-x\n+alpha beta gamma delta\n");
    let r = rows::rows(0, &d, false, Some(10));
    // The hunk, the removed line, the added one in four pieces.
    assert_eq!(r.len(), 6, "{r:?}");
    assert!(matches!(r[5], Row::Unified { part: 3, .. }));
}

/// A run the host refused (too many at once, the git graph's runs counting too) is
/// asked again, never shown as the file's diff.
#[test]
fn a_refused_run_is_asked_again() {
    let mut m = DiffMod::default();
    let _ = m.update(&Input::default());
    let _ = call(&mut m, vec![], vec![]);
    let _ = call(&mut m, vec![], vec![git_answer("1/head", &format!("{}\n", "a".repeat(40)))]);
    let _ = call(&mut m, vec![], vec![
        git_answer("1/status", "M\0src/list.rs\0"),
        git_answer("1/numstat", "2\t1\tsrc/list.rs\0"),
        git_answer("1/untracked", ""),
    ]);
    let refusal = ToolResult { tool: "git".into(), id: "1/diff/src/list.rs".into(), status: None, stdout: String::new(), stderr: "at most 4 runs at once".into() };
    let out = call(&mut m, vec![], vec![refusal]);
    assert!(!format!("{:?}", out.ui).contains("at most"), "shown as content");
    // Asked again, at once or (the whole range's diff being on its way) once a run is free.
    let again = runs(&out).iter().any(|r| r.0 == "1/diff/src/list.rs") || m.queue.iter().any(|q| q.0 == "1/diff/src/list.rs");
    assert!(again, "asked again");
    let out = call(&mut m, vec![], vec![git_answer("1/diff/src/list.rs", DIFF)]);
    assert!(format!("{:?}", out.ui).contains("count"));
}

/// A big hunk is parsed a block at a time, within the budget; a block goes on from the
/// one before it (a comment opened there is still open).
#[test]
fn highlights_big_hunks_a_block_at_a_time() {
    use highlight::{BLOCK, HunkTokens, Token, cost};
    let n = 10 * BLOCK;
    let mut body = String::from("+/* a comment that goes on\n");
    for i in 1..n {
        body.push_str(&if i == BLOCK + 2 { "+   until here */ const a = 1;\n".to_owned() } else { format!("+   line {i}\n") });
    }
    let d = parse_diff(&format!("@@ -0,0 +1,{n} @@\n{body}"));
    let h = &d.hunks[0];
    let mut t = HunkTokens::new("a.js", h);
    let costs = |lines: std::ops::Range<usize>| -> usize { h.lines[lines].iter().map(|l| cost(&l.text)).sum() };
    // A budget a little over one block's: the block it ran out in is finished, no more.
    assert_eq!(t.parse(h, 0..n, costs(0..BLOCK) + 1), costs(0..2 * BLOCK), "a block started is finished, then it stops");
    assert!(t.has(0..2 * BLOCK) && !t.has(0..n) && !t.done());
    // The second block's lines are still in the comment the first one opened.
    let kind = |t: &HunkTokens, i: usize| t.line(i, git::LineKind::Added).and_then(|ts| ts.first()).map(|x| x.2);
    assert_eq!(kind(&t, BLOCK + 1), Some(Token::Comment));
    // Jumping far ahead parses just that block.
    assert_eq!(t.parse(h, 8 * BLOCK..8 * BLOCK + 3, 1), costs(8 * BLOCK..9 * BLOCK));
    assert!(t.has(8 * BLOCK..9 * BLOCK) && !t.has(6 * BLOCK..7 * BLOCK));
}

/// How long each update takes on a real repo: `DIFF_BENCH=<repo> [DIFF_BENCH_COMMIT=<hash>]
/// [DIFF_BENCH_ALL=1] cargo test --release -p diff bench -- --ignored --nocapture`. git
/// runs here for real; each update gets the answers to what the last one asked (as the
/// host's runs come back), through the same JSON both ways as in the host.
#[test]
#[ignore]
fn bench() {
    let Ok(repo) = std::env::var("DIFF_BENCH") else { return };
    let commit = std::env::var("DIFF_BENCH_COMMIT").ok();
    let all = std::env::var("DIFF_BENCH_ALL").is_ok();
    let mut m = DiffMod::default();
    let mut answers: Vec<ToolResult> = Vec::new();
    let mut events = if all { vec![Event::Action(Action::Key("all-files".into()))] } else { vec![] };
    let (mut worst, mut worst_at, mut total) = (0.0f64, 0, 0.0f64);
    let mut times = Vec::new();
    for call_no in 0..20_000 {
        let window = Some(WindowContext { folder: repo.clone(), ..Default::default() });
        let input = Input {
            events: std::mem::take(&mut events),
            window,
            arg: commit.clone(),
            width: 1200.0,
            height: 800.0,
            focused: false,
            data: HostData { tool_runs: std::mem::take(&mut answers), ..Default::default() },
            ..Default::default()
        };
        let json = serde_json::to_vec(&input).unwrap();
        // DIFF_BENCH_DUMP=<dir>: each input kept, for the host's replay (wasm, its limits).
        if let Ok(dir) = std::env::var("DIFF_BENCH_DUMP") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/input-{call_no:05}.json"), &json).unwrap();
        }
        let t = std::time::Instant::now();
        let out = wasm_mods_sdk::__abi::update(&mut m, &json);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        let out: serde_json::Value = serde_json::from_slice(&out).unwrap();
        times.push(ms);
        total += ms;
        if ms > worst {
            (worst, worst_at) = (ms, call_no);
        }
        let asked: Vec<(String, Vec<String>)> = (out["effects"].as_array().into_iter().flatten())
            .filter_map(|e| serde_json::from_value::<Effect>(e.clone()).ok())
            .filter_map(|e| match e {
                Effect::RunTool { tool, id, args } if tool == "git" => Some((id, args)),
                _ => None,
            })
            .collect();
        if asked.is_empty() && call_no > 2 {
            break;
        }
        for (id, args) in asked {
            let o = std::process::Command::new("git").args(&args).current_dir(&repo).output().unwrap();
            answers.push(ToolResult {
                tool: "git".into(),
                id,
                status: o.status.code(),
                stdout: String::from_utf8_lossy(&o.stdout).into(),
                stderr: String::from_utf8_lossy(&o.stderr).into(),
            });
        }
    }
    let parsed: usize = m.repo.tokens.values().map(|t| t.new.lines.iter().chain(&t.old.lines).filter(|l| l.is_some()).count()).sum();
    println!("BENCH tokens: {} hunks, {parsed} lines; rows {}", m.repo.tokens.len(), m.repo.rows.as_ref().map_or(0, |r| r.1.len()));
    times.sort_by(f64::total_cmp);
    let p = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    println!(
        "BENCH calls {} total {total:.0} ms, worst {worst:.1} ms (call {worst_at}), p50 {:.2} p99 {:.1} ms, files {}",
        times.len(), p(0.5), p(0.99), m.repo.files.len()
    );
}

/// Where a big file's first call goes: grammar loading, parsing git's diff, the rows,
/// a budget of colouring, and the whole hunk coloured at once.
#[test]
#[ignore]
fn bench_highlight() {
    let lines: String = (0..2000).map(|i| format!("+  step{i}(a, b = {i}) {{ const x = a?.b ?? [1, 2].map((v) => v * {i}); return {{ x, s: 'str {i}' }}; }}\n")).collect();
    let text = format!("diff --git a/big.js b/big.js\n--- /dev/null\n+++ b/big.js\n@@ -0,0 +1,2000 @@\n{lines}");
    let t = std::time::Instant::now();
    let ms = |t: &mut std::time::Instant| { let e = t.elapsed().as_secs_f64() * 1000.0; *t = std::time::Instant::now(); e };
    let mut t = t;
    let _ = highlight::syntax_for("a.js", "");
    println!("BENCH grammars {:.1} ms", ms(&mut t));
    let d = parse_diff(&text);
    println!("BENCH parse_diff {:.1} ms", ms(&mut t));
    let r = rows::rows(0, &d, false, None);
    println!("BENCH rows {:.1} ms ({} rows)", ms(&mut t), r.len());
    let mut h = highlight::HunkTokens::new("big.js", &d.hunks[0]);
    let n = h.parse(&d.hunks[0], 0..60, 16_000);
    println!("BENCH 16 kB parse {:.1} ms ({n} bytes)", ms(&mut t));
    let _ = highlight::hunk("big.js", &d.hunks[0]);
    println!("BENCH whole hunk {:.1} ms", ms(&mut t));
}


/// Scrolling a big diff: `DIFF_BENCH=<repo> DIFF_BENCH_COMMIT=<hash> [DIFF_BENCH_ALL=1]
/// [DIFF_BENCH_STEP=<rows>] [DIFF_BENCH_QUIET=<calls>] [DIFF_BENCH_DUMP=<dir>] cargo test
/// --release -p diff bench_scroll -- --ignored --nocapture`. Reads the range as `bench`
/// does, then scrolls down `STEP` rows at a time (the diff's list, 25 rows showing), with
/// `QUIET` calls of its own between (as the host makes while the module is busy). Each
/// step counts the rows showing that the frame before didn't send (blank until this
/// module answers) and those showing without their colours.
#[test]
#[ignore]
fn bench_scroll() {
    let Ok(repo) = std::env::var("DIFF_BENCH") else { return };
    let commit = std::env::var("DIFF_BENCH_COMMIT").ok();
    let all = std::env::var("DIFF_BENCH_ALL").is_ok();
    let step: u64 = std::env::var("DIFF_BENCH_STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(30);
    let quiet: usize = std::env::var("DIFF_BENCH_QUIET").ok().and_then(|s| s.parse().ok()).unwrap_or(2);
    let dump = std::env::var("DIFF_BENCH_DUMP").ok();
    let mut m = DiffMod::default();
    let mut calls = 0usize;
    let mut call = |m: &mut DiffMod, events: Vec<Event>, runs: Vec<ToolResult>| -> (serde_json::Value, f64) {
        let window = Some(WindowContext { folder: repo.clone(), ..Default::default() });
        let input = Input { events, window, arg: commit.clone(), width: 1200.0, height: 800.0, data: HostData { tool_runs: runs, ..Default::default() }, ..Default::default() };
        let json = serde_json::to_vec(&input).unwrap();
        if let Some(dir) = &dump {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(format!("{dir}/input-{calls:05}.json"), &json).unwrap();
        }
        calls += 1;
        let t = std::time::Instant::now();
        let out = wasm_mods_sdk::__abi::update(m, &json);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        (serde_json::from_slice(&out).unwrap(), ms)
    };
    let run_git = |out: &serde_json::Value| -> Vec<ToolResult> {
        (out["effects"].as_array().into_iter().flatten())
            .filter_map(|e| serde_json::from_value::<Effect>(e.clone()).ok())
            .filter_map(|e| match e {
                Effect::RunTool { tool, id, args } if tool == "git" => Some((id, args)),
                _ => None,
            })
            .map(|(id, args)| {
                let o = std::process::Command::new("git").args(&args).current_dir(&repo).output().unwrap();
                ToolResult { tool: "git".into(), id, status: o.status.code(), stdout: String::from_utf8_lossy(&o.stdout).into(), stderr: String::from_utf8_lossy(&o.stderr).into() }
            })
            .collect()
    };
    let t0 = std::time::Instant::now();
    let mut events = if all { vec![Event::Action(Action::Key("all-files".into()))] } else { vec![] };
    let mut answers = Vec::new();
    let (mut out, mut load_ms) = (serde_json::Value::Null, 0.0);
    for n in 0..200 {
        let (o, ms) = call(&mut m, std::mem::take(&mut events), std::mem::take(&mut answers));
        load_ms += ms;
        answers = run_git(&o);
        out = o;
        if answers.is_empty() && n > 2 {
            break;
        }
    }
    let busy = |o: &serde_json::Value| o["wants"]["busy"].as_bool().unwrap_or(false);
    let sent = |o: &serde_json::Value| -> Option<(u64, u64)> {
        fn find(v: &serde_json::Value) -> Option<(u64, u64)> {
            if let Some(l) = v.get("List").filter(|l| l["id"] == LINES) {
                let w = &l["window"];
                let first = w["first"].as_u64()?;
                return Some((first, first + l["items"].as_array()?.len() as u64));
            }
            match v {
                serde_json::Value::Object(o) => o.values().find_map(find),
                serde_json::Value::Array(a) => a.iter().find_map(find),
                _ => None,
            }
        }
        find(&o["ui"])
    };
    let total = m.rows().len() as u64;
    println!("BENCH loaded in {:.0} ms ({load_ms:.0} ms in calls), {} files, {total} rows, frame {} kB", t0.elapsed().as_secs_f64() * 1e3, m.repo.files.len(), out.to_string().len() / 1000);
    let plain = |m: &mut DiffMod, first: u64| -> usize {
        let rows: Vec<Row> = m.rows().iter().skip(first as usize).take(25).cloned().collect();
        rows.iter()
            .filter(|r| {
                let (file, hunk, lines) = match r {
                    Row::Unified { file, hunk, cell, .. } => (*file, *hunk, vec![cell.line]),
                    Row::Split { file, hunk, left, right, .. } => (*file, *hunk, left.iter().chain(right).map(|c| c.line).collect()),
                    // A file not read yet.
                    Row::Notice { text, .. } => return text == "reading…",
                    _ => return false,
                };
                let path = &m.repo.files[file].path;
                !lines.iter().all(|&l| m.repo.tokens.get(&(path.clone(), hunk)).is_some_and(|t| t.has(l..l + 1)))
            })
            .count()
    };
    let (mut blank, mut uncoloured, mut steps, mut worst, mut sum, mut quiet_ms, mut quiet_n) = (0u64, 0usize, 0, 0.0f64, 0.0, 0.0, 0);
    let mut first = 0u64;
    let mut last = sent(&out);
    let mut last_out = out.clone();
    // (The rows grow as files are read.)
    while first + step < m.rows().len() as u64 {
        first += step;
        // git's answers to what the last call asked come with this one.
        let runs = run_git(&last_out);
        let (o, ms) = call(&mut m, vec![Event::VisibleRange { id: LINES.into(), first, count: 25 }], runs);
        // Rows showing that the last frame didn't send: blank until this answer lands.
        if let Some((a, b)) = last {
            blank += (first..first + 25).filter(|r| *r < a || *r >= b).count() as u64;
        }
        uncoloured += plain(&mut m, first);
        last = sent(&o);
        (worst, sum, steps) = (worst.max(ms), sum + ms, steps + 1);
        let mut o = o;
        for _ in 0..quiet {
            if !busy(&o) {
                break;
            }
            let runs = run_git(&o);
            let (q, ms) = call(&mut m, vec![], runs);
            (quiet_ms, quiet_n) = (quiet_ms + ms, quiet_n + 1);
            o = q;
        }
        last_out = o;
    }
    println!(
        "BENCH scroll {steps} steps of {step} rows: blank rows {blank}, rows uncoloured or reading {uncoloured}, event call avg {:.1} ms worst {worst:.1} ms; quiet calls {quiet_n}, avg {:.1} ms",
        sum / steps.max(1) as f64, quiet_ms / quiet_n.max(1) as f64
    );
    // The rest of the background work, to the end.
    let (mut n, mut ms_all, mut worst_q) = (0, 0.0, 0.0f64);
    loop {
        let (o, ms) = call(&mut m, vec![], vec![]);
        (n, ms_all, worst_q) = (n + 1, ms_all + ms, worst_q.max(ms));
        if !busy(&o) || n > 100_000 {
            break;
        }
    }
    println!("BENCH background to the end: {n} calls, {ms_all:.0} ms, worst {worst_q:.1} ms; coloured {} kB, {} hunks", m.repo.coloured / 1000, m.repo.tokens.len());
}
