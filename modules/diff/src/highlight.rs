//! Syntax colours, with syntect's bundled grammars (MIT), ported from an earlier viewer.
//! Tokens are kinds, not colours: the view maps them to the theme, so a theme change
//! needs no new parse.
//!
//! Only what's shown is parsed: each side of a hunk (its old lines, context and removed,
//! or its new ones, context and added) in blocks of [`BLOCK`] lines, a block when a row
//! of it is first shown, and kept. A block goes on from where the block before it ended
//! when that one is parsed (scrolling down, it always is), else from a fresh start, as a
//! hunk's first block does. A call parses at most [`BUDGET`] lines: a big hunk never
//! takes long, whatever its size (a grammar parses a few thousand lines a second
//! here). The tab's file isn't read for it (the diff's own lines are enough, on any
//! machine and for uncommitted changes), so a hunk, or a block jumped to, that starts
//! inside a block comment or a long string shows that part as code.

use std::sync::OnceLock;

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::git::{Hunk, LineKind};

/// Lines longer than this aren't highlighted (minified files): plain.
const MAX_LINE: usize = 2000;
/// Lines parsed together, and the most parsed in one call.
pub const BLOCK: usize = 48;
pub const BUDGET: usize = 2 * BLOCK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    Text,
    Comment,
    String,
    Number,
    Constant,
    Keyword,
    Function,
    Type,
    Tag,
    Attribute,
    Punctuation,
}

/// A line's tokens: `(start, end, kind)` byte ranges.
pub type Tokens = Vec<(usize, usize, Token)>;

/// One hunk's tokens, by its line index: the old side's (context and removed lines) and
/// the new side's (context and added), filled in a block at a time ([`HunkTokens::parse`]).
/// No tokens when the language is unknown.
#[derive(Clone, Default)]
pub struct HunkTokens {
    syntax: Option<&'static SyntaxReference>,
    pub old: Side,
    pub new: Side,
}

/// One side's tokens so far, and where each block parsed ended (to go on from).
#[derive(Clone, Default)]
pub struct Side {
    pub lines: Vec<Option<Tokens>>,
    ends: Vec<Option<(ParseState, ScopeStack)>>,
}

impl HunkTokens {
    /// Nothing parsed yet: the language only (cheap).
    pub fn new(path: &str, hunk: &Hunk) -> Self {
        let first = hunk.lines.first().map_or("", |l| l.text.as_str());
        let blocks = hunk.lines.len().div_ceil(BLOCK);
        let side = || Side { lines: vec![None; hunk.lines.len()], ends: vec![None; blocks] };
        HunkTokens { syntax: syntax_for(path, first), old: side(), new: side() }
    }

    /// The tokens line `i` is drawn with: a removed line's old ones, others the new ones.
    pub fn line(&self, i: usize, kind: LineKind) -> Option<&Tokens> {
        let side = if kind == LineKind::Removed { &self.old } else { &self.new };
        side.lines.get(i)?.as_ref()
    }

    /// Parses the blocks holding lines `lines` not parsed yet, both sides, within
    /// `budget` lines; returns the lines parsed.
    pub fn parse(&mut self, hunk: &Hunk, lines: std::ops::Range<usize>, budget: usize) -> usize {
        let Some(syntax) = self.syntax else { return 0 };
        let mut spent = 0;
        let last = lines.end.min(hunk.lines.len());
        for block in lines.start / BLOCK..last.div_ceil(BLOCK) {
            if spent >= budget {
                break;
            }
            for (side, keep) in [(&mut self.old, LineKind::Removed), (&mut self.new, LineKind::Added)] {
                if side.ends[block].is_some() {
                    continue;
                }
                let (mut state, mut stack) = match block.checked_sub(1).and_then(|b| side.ends[b].clone()) {
                    Some(end) => end,
                    None => (ParseState::new(syntax), ScopeStack::new()),
                };
                let span = block * BLOCK..((block + 1) * BLOCK).min(hunk.lines.len());
                for i in span {
                    let l = &hunk.lines[i];
                    if l.kind == LineKind::Context || l.kind == keep {
                        side.lines[i] = line(&mut state, &mut stack, &l.text);
                        spent += 1;
                    }
                }
                side.ends[block] = Some((state, stack));
            }
        }
        spent
    }

    /// Whether every block holding lines `lines` is parsed (or there's nothing to parse).
    pub fn has(&self, lines: std::ops::Range<usize>) -> bool {
        self.syntax.is_none() || (lines.start / BLOCK..lines.end.div_ceil(BLOCK)).all(|b| self.new.ends.get(b).is_none_or(Option::is_some))
    }
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Scope prefixes and the token they make, most specific first.
fn table() -> &'static [(Scope, Token)] {
    static TABLE: OnceLock<Vec<(Scope, Token)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The first two are containers (see `classify`).
        [
            ("comment", Token::Comment),
            ("string", Token::String),
            ("constant.character", Token::String),
            ("markup.inline.raw", Token::String),
            ("constant.numeric", Token::Number),
            ("constant", Token::Constant),
            ("variable.language", Token::Constant),
            ("keyword.operator", Token::Punctuation),
            ("keyword", Token::Keyword),
            ("storage.type.function", Token::Keyword),
            ("storage.type", Token::Type),
            ("storage", Token::Keyword),
            ("markup.heading", Token::Keyword),
            ("entity.name.function", Token::Function),
            ("variable.function", Token::Function),
            ("support.function", Token::Function),
            ("entity.name.type", Token::Type),
            ("entity.name.class", Token::Type),
            ("entity.name.struct", Token::Type),
            ("entity.name.enum", Token::Type),
            ("entity.name.trait", Token::Type),
            ("entity.other.inherited-class", Token::Type),
            ("support.type", Token::Type),
            ("support.class", Token::Type),
            ("entity.name.tag", Token::Tag),
            ("entity.other.attribute-name", Token::Attribute),
            ("punctuation", Token::Punctuation),
        ]
        .into_iter()
        .filter_map(|(s, t)| Some((Scope::new(s).ok()?, t)))
        .collect()
    })
}

fn classify(stack: &ScopeStack) -> Token {
    // Inside a comment or a string, everything is (even its delimiters' punctuation).
    let containers = &table()[..2];
    for scope in stack.as_slice() {
        if let Some((_, token)) = containers.iter().find(|(prefix, _)| prefix.is_prefix_of(*scope)) {
            return *token;
        }
    }
    for scope in stack.as_slice().iter().rev() {
        if let Some((_, token)) = table().iter().find(|(prefix, _)| prefix.is_prefix_of(*scope)) {
            return *token;
        }
    }
    Token::Text
}

/// The grammar for a file: by its extension, its whole name (`Makefile`), else its first
/// line (`#!/bin/sh`).
pub fn syntax_for(path: &str, first_line: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let name = path.rsplit('/').next()?;
    let ext = name.rsplit_once('.').map_or(name, |(_, e)| e);
    set.find_syntax_by_extension(ext)
        .or_else(|| set.find_syntax_by_extension(name))
        .or_else(|| set.find_syntax_by_first_line(first_line))
        .filter(|s| s.name != "Plain Text")
}

/// A whole hunk's tokens in `path`'s language (none if it's unknown), at once.
#[cfg(test)]
pub fn hunk(path: &str, hunk: &Hunk) -> HunkTokens {
    let mut t = HunkTokens::new(path, hunk);
    t.parse(hunk, 0..hunk.lines.len(), usize::MAX);
    t
}

/// One line's tokens, the parse carried on from the line before.
fn line(state: &mut ParseState, stack: &mut ScopeStack, text: &str) -> Option<Tokens> {
    if text.len() > MAX_LINE {
        return None;
    }
    let with_end = format!("{text}\n");
    let ops = state.parse_line(&with_end, syntaxes()).ok()?;
    let end = text.len();
    let mut tokens = Vec::new();
    let mut at = 0;
    for (pos, op) in ops {
        let pos = pos.min(end);
        if pos > at {
            push(&mut tokens, at, pos, classify(stack));
            at = pos;
        }
        let _ = stack.apply(&op);
    }
    if end > at {
        push(&mut tokens, at, end, classify(stack));
    }
    Some(tokens)
}

/// Adds a token, merging it into the previous one if they're the same kind.
fn push(tokens: &mut Tokens, start: usize, end: usize, token: Token) {
    match tokens.last_mut() {
        Some(last) if last.2 == token && last.1 == start => last.1 = end,
        _ => tokens.push((start, end, token)),
    }
}
