//! The fuzzy matcher's ranking: what comes first for a query.

use wasm_mods_protocol::ui::fuzzy_rank;

#[test]
fn fuzzy_ranking() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        // An empty query keeps every entry, in order.
        ("", &["shell", "git graph", "diff"], &["shell", "git graph", "diff"]),
        // Word starts beat letters in the middle of a word.
        ("tn", &["catppuccin mocha", "tokyo night"], &["tokyo night", "catppuccin mocha"]),
        // A contiguous run beats scattered letters.
        ("dark", &["dawn rock", "one dark"], &["one dark", "dawn rock"]),
        // A prefix beats the same letters inside a word.
        ("git", &["digit", "git graph"], &["git graph", "digit"]),
        // Initials of words.
        ("gg", &["diff", "git graph", "shell"], &["git graph"]),
        // Matching case wins a tie.
        ("Git", &["git", "Git"], &["Git", "git"]),
        // Ties go to the shorter entry.
        ("sh", &["shell session", "shell"], &["shell", "shell session"]),
        // Every query letter must appear, in order.
        ("xyz", &["shell", "zyx"], &[]),
        ("ba", &["abc"], &[]),
    ];
    for (query, texts, want) in cases {
        let got: Vec<&str> = fuzzy_rank(query, texts.iter().copied()).into_iter().map(|(i, _)| texts[i]).collect();
        assert_eq!(&got, want, "query {query:?}");
    }
}
