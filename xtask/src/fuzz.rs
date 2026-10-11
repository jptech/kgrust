//! `cargo xtask fuzz` — corpus-mutation fuzzing with shrinking.
//!
//! `kg-parser` already fuzzes with random token soup, and `kg-lexer` with
//! random character strings. Both prove totality, and neither reaches the
//! region where a recursive-descent parser actually breaks: input that is
//! *almost* valid. Random soup hits an error in the first few tokens and spends
//! the rest of its life in recovery; a real corpus file with one brace deleted
//! gets six hundred lines in before anything goes wrong, which is where the
//! interesting state lives.
//!
//! So this mutates real files. Every failure is reported with a **shrunk**
//! reproducer — the seed alone is not enough, because the seed only reproduces
//! against the same corpus and the same mutation table.
//!
//! # The invariants
//!
//! These hold for *every* input, valid or not. That is the point: a tool that
//! is only correct on correct input is not usable in an editor, where most
//! intermediate states are broken.
//!
//! | stage | invariant |
//! |---|---|
//! | lex | total; concatenating every token's text reproduces the input exactly |
//! | parse | total; `parse(t).text() == t`; terminates |
//! | format | on input that parses: total, idempotent, and token-preserving |
//! | check | total: the whole single-file frontend survives an error-recovered tree |
//!
//! Panics are caught rather than allowed to abort, so one bad input yields a
//! reproducer instead of ending the run.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
};

use anyhow::Result;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Lex,
    Parse,
    Format,
    Check,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Lex => "lex",
            Stage::Parse => "parse",
            Stage::Format => "format",
            Stage::Check => "check",
        }
    }
}

/// Why an input was rejected. Carried so shrinking can insist on the *same*
/// failure rather than any failure — otherwise a shrink happily converges on
/// some unrelated bug and reports that instead.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Failure {
    Panic(String),
    LexNotLossless,
    ParseNotLossless,
    FormatNotIdempotent,
    FormatLostTokens,
    /// `kg-fmt` caught itself producing a different program. That is the
    /// formatter's own safety net doing its job, and it is still a bug — the
    /// net exists so the corruption is not written to a file, not so it can be
    /// ignored.
    FormatSelfRejected(String),
}

/// A deterministic LCG — the same one `kg-parser`'s soup test uses, so a
/// failure here reproduces there and vice versa. `rand` is not a dependency
/// worth taking for this.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

pub fn run(
    kanagawa_root: &Path,
    iterations: usize,
    seed: u64,
    stage: Option<&str>,
    verbose: bool,
) -> Result<()> {
    let stages: Vec<Stage> = match stage {
        None => vec![Stage::Lex, Stage::Parse, Stage::Format, Stage::Check],
        Some("lex") => vec![Stage::Lex],
        Some("parse") => vec![Stage::Parse],
        Some("format") => vec![Stage::Format],
        Some("check") => vec![Stage::Check],
        Some(other) => anyhow::bail!("unknown stage {other:?}; use lex, parse, format or check"),
    };

    let mut files = Vec::new();
    for dir in ["library", "test", "doc", "tools"] {
        collect(&kanagawa_root.join(dir), &mut files);
    }
    files.sort();
    let corpus: Vec<String> =
        files.iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
    if corpus.is_empty() {
        anyhow::bail!("no readable .k files under {}", kanagawa_root.display());
    }

    // Panics are expected output here, not a crash; silence the default hook so
    // a run of a few thousand cases does not bury its own report in backtraces.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut rng = Rng(seed);
    let mut failures: Vec<(Stage, Failure, String)> = Vec::new();
    let mut checked = 0usize;

    for i in 0..iterations {
        let base = &corpus[rng.below(corpus.len())];
        let input = mutate(base, &mut rng);
        for &s in &stages {
            checked += 1;
            if let Some(f) = check(s, &input) {
                let small = shrink(s, &input, &f);
                if verbose || failures.len() < 10 {
                    println!(
                        "\n--- {} failure at iteration {i}: {f:?}\n{}\n---",
                        s.name(),
                        preview(&small)
                    );
                }
                failures.push((s, f, small));
            }
        }
    }

    std::panic::set_hook(hook);

    println!(
        "{iterations} mutations from {} corpus files, {checked} stage checks, seed {seed}",
        corpus.len()
    );
    for &s in &stages {
        let n = failures.iter().filter(|(fs, ..)| *fs == s).count();
        println!("  {:<7} {}", s.name(), if n == 0 { "clean".to_owned() } else { format!("{n} FAILURES") });
    }

    if failures.is_empty() {
        return Ok(());
    }
    anyhow::bail!("{} fuzz failure(s)", failures.len())
}

/// Runs one stage and reports the first invariant it breaks.
fn check(stage: Stage, input: &str) -> Option<Failure> {
    let result = catch_unwind(AssertUnwindSafe(|| match stage {
        Stage::Lex => {
            let lexed = kg_parser::LexedStr::new(input);
            let mut rebuilt = String::with_capacity(input.len());
            for i in 0..lexed.len() {
                rebuilt.push_str(lexed.text(i));
            }
            (rebuilt == input).then_some(()).ok_or(Failure::LexNotLossless)
        }
        Stage::Parse => {
            let parse = kg_syntax::parse_file(input);
            (parse.text() == input).then_some(()).ok_or(Failure::ParseNotLossless)
        }
        Stage::Format => {
            let config = kg_config::Fmt::default();
            // Only *one* of the two error variants is an acceptable answer.
            // Declining a file that does not parse is correct. Failing the
            // crate's own output verification is a defect, and treating both
            // the same way is how a fuzzer comes back green while the thing it
            // fuzzes is broken — which is exactly what happened the first time
            // this was tested against an injected bug.
            let once = match kg_fmt::format_file(input, &config) {
                Ok(out) => out,
                Err(kg_fmt::FormatError::Unparseable { .. }) => return Ok(()),
                Err(kg_fmt::FormatError::Verification { detail }) => {
                    return Err(Failure::FormatSelfRejected(detail));
                }
            };
            let twice = match kg_fmt::format_file(&once, &config) {
                Ok(out) => out,
                // Formatted output that no longer parses, or that fails
                // verification on the second pass, is never acceptable.
                Err(e) => return Err(Failure::FormatSelfRejected(e.to_string())),
            };
            if twice != once {
                return Err(Failure::FormatNotIdempotent);
            }
            if tokens(input) != tokens(&once) {
                return Err(Failure::FormatLostTokens);
            }
            Ok(())
        }
        Stage::Check => {
            // The frontend over the mutant as a program alone: one that
            // parses runs every pass, the program-wide loop included, and a
            // panic there is the one answer that is never acceptable. One
            // that does not parse stops at its syntax errors, as the
            // reference does (`Main.hs:73-75`) — until sprint W3.7 this
            // stage ran a second door's passes over the recovered tree; the
            // loop over a broken buffer is the editor's, and `xtask
            // resilience` asks it of every error-recovered corpus file.
            kg_db::check_source(input);
            Ok(())
        }
    }));

    match result {
        Ok(Ok(())) => None,
        Ok(Err(f)) => Some(f),
        Err(payload) => Some(Failure::Panic(panic_message(payload.as_ref()))),
    }
}

fn tokens(text: &str) -> Vec<(kg_parser::SyntaxKind, String)> {
    let lexed = kg_parser::LexedStr::new(text);
    (0..lexed.len())
        .filter(|&i| !lexed.kind(i).is_trivia())
        .map(|i| (lexed.kind(i), lexed.text(i).to_owned()))
        .collect()
}

pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic>".to_owned())
        .lines()
        .next()
        .unwrap_or("")
        .to_owned()
}

/// Cuts the input down while it keeps failing *the same way*.
///
/// Delta debugging with halving granularity: try removing halves, then
/// quarters, and so on. Insisting on the same `Failure` matters — without it a
/// shrink drifts onto whatever other bug the smaller input happens to hit, and
/// reports a reproducer for something else entirely.
fn shrink(stage: Stage, input: &str, want: &Failure) -> String {
    let mut best = input.to_owned();
    let mut chunks = 2usize;
    while chunks <= 64 && best.len() > 1 {
        let len = best.len();
        let size = (len / chunks).max(1);
        let mut cut = false;
        let mut start = 0;
        while start < len {
            let end = char_boundary(&best, (start + size).min(len));
            let s = char_boundary(&best, start);
            if s >= end {
                start += size;
                continue;
            }
            let mut candidate = String::with_capacity(len);
            candidate.push_str(&best[..s]);
            candidate.push_str(&best[end..]);
            if check(stage, &candidate).as_ref() == Some(want) {
                best = candidate;
                cut = true;
                break;
            }
            start += size;
        }
        if !cut {
            chunks *= 2;
        }
    }
    best
}

fn char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// One mutation of a corpus file.
///
/// The table is weighted toward small edits near the middle of a file, because
/// that is what an editor produces and what leaves the parser deepest in a
/// nested state when it goes wrong.
pub(crate) fn mutate(base: &str, rng: &mut Rng) -> String {
    let mut s = base.to_owned();
    // One to four edits: enough to break the file, rarely enough to reduce it
    // to soup.
    for _ in 0..1 + rng.below(4) {
        if s.is_empty() {
            break;
        }
        match rng.below(8) {
            // Delete a byte range — the classic "half a token".
            0 => {
                let a = char_boundary(&s, rng.below(s.len()));
                let b = char_boundary(&s, (a + 1 + rng.below(16)).min(s.len()));
                s.replace_range(a..b, "");
            }
            // Duplicate a range: unbalances brackets without producing garbage.
            1 => {
                let a = char_boundary(&s, rng.below(s.len()));
                let b = char_boundary(&s, (a + 1 + rng.below(64)).min(s.len()));
                let piece = s[a..b].to_owned();
                s.insert_str(a, &piece);
            }
            // Truncate: the state an editor is in for most of a typing session.
            2 => {
                let a = char_boundary(&s, rng.below(s.len()));
                s.truncate(a);
            }
            // Insert a delimiter, unbalancing one nesting level.
            3 => {
                const DELIMS: [&str; 10] =
                    ["{", "}", "(", ")", "[", "]", "<", ">", "\"", "*/"];
                let a = char_boundary(&s, rng.below(s.len()));
                s.insert_str(a, DELIMS[rng.below(DELIMS.len())]);
            }
            // Delete a whole token, so what remains still lexes cleanly.
            4 => {
                let lexed = kg_parser::LexedStr::new(&s);
                if lexed.len() > 1 {
                    let i = rng.below(lexed.len());
                    let r = lexed.range(i);
                    s.replace_range(usize::from(r.start())..usize::from(r.end()), "");
                }
            }
            // Swap two adjacent tokens.
            5 => {
                let lexed = kg_parser::LexedStr::new(&s);
                if lexed.len() > 2 {
                    let i = rng.below(lexed.len() - 1);
                    let (a, b) = (lexed.range(i), lexed.range(i + 1));
                    let (at, bt) = (lexed.text(i).to_owned(), lexed.text(i + 1).to_owned());
                    s.replace_range(
                        usize::from(a.start())..usize::from(b.end()),
                        &format!("{bt}{at}"),
                    );
                }
            }
            // Splice in a fragment of another construct.
            6 => {
                const BITS: [&str; 8] = [
                    "template<typename T>",
                    "[[memory]]",
                    "static assert(",
                    "cast<uint8>(",
                    "obj.template Go<uint32>()",
                    "//| doc\n",
                    "/*| nested /* inner */ */",
                    "\"{x:d 3}\"",
                ];
                let a = char_boundary(&s, rng.below(s.len()));
                s.insert_str(a, BITS[rng.below(BITS.len())]);
            }
            // Replace a byte with a delimiter-ish character.
            _ => {
                const CHARS: [char; 12] =
                    ['<', '>', '{', '}', '(', ')', ';', ':', '"', '\\', '\n', '\u{0}'];
                let a = char_boundary(&s, rng.below(s.len()));
                let b = char_boundary(&s, (a + 1).min(s.len()));
                if a < b {
                    s.replace_range(a..b, &CHARS[rng.below(CHARS.len())].to_string());
                }
            }
        }
    }
    s
}

fn preview(s: &str) -> String {
    const MAX: usize = 600;
    if s.len() <= MAX {
        return s.to_owned();
    }
    let cut = char_boundary(s, MAX);
    format!("{}\n... [{} more bytes]", &s[..cut], s.len() - cut)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}
