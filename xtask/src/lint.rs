//! `cargo xtask lint`: spellings the tree is retiring, counted in production
//! code and ratcheted — a count may fall and may not rise
//! (`single-source-of-truth.md` "Enforcement"; sprint W0.4, W1.1, W8.1,
//! W8.2).
//!
//! Each rule names what it counts, why it is retiring, and the owner
//! modules where the spelling is the thing's definition rather than a copy
//! of it. "Production" is `crates/*/src/**/*.rs` less build scripts, files
//! named `tests.rs`, and every item under `#[cfg(test)]`; comments do not
//! count, string literals do (`"<|"` is the spelling). The ledger holds one
//! fail set per rule, an item per occurrence — `path #k`, the `k`-th in that
//! file — so a spelling added in one file is a regression even when another
//! file lost one the same day.
//!
//! A grep, not a type check: a reader renamed out of `mangled-readers`
//! still spells `"<|"`, which is why the two rules run side by side.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// One retiring spelling.
struct Rule {
    name: &'static str,
    /// Any of these, literally.
    needles: &'static [&'static str],
    /// Files (or directories, ending `/`) where the spelling is the owner's.
    owners: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        name: "mangled-text",
        // `<|{` is the bracket written around a mangle by hand —
        // `format!("<|{args}|>")` — which `resolve::instance_suffix` owns
        // (D1.1, 2026-10-10).
        needles: &["\"<|\"", "\"|>\"", "b\"<|\"", "<|{"],
        owners: &["crates/kg-base/src/name.rs", "crates/kg-hir/src/rewrite/resolve.rs"],
        why: "a mangled instance name read as text, or written by hand; identity is structure (W1.1, D1.1-2)",
    },
    Rule {
        name: "mangled-readers",
        needles: &[
            "fn first_instance(",
            "fn last_mangle_start(",
            "fn first_mangle(",
            "fn split_last_hoist(",
            "fn split_joined_names(",
            "fn split_top_level_last(",
            "fn unmangle_text(",
            "fn unmangle_leaf(",
            "fn unmangle_item(",
            "fn mangled_instance(",
            "fn mangled_arg(",
        ],
        owners: &[],
        why: "the parsers of mangled names sprint §1 lists; W1.1's exit is none",
    },
    Rule {
        name: "namespace-replace",
        needles: &["replace('@'"],
        owners: &["crates/kg-base/src/name.rs"],
        why: "a module namespace unmangled by hand; `kg_base::name` owns it (D1.1)",
    },
    Rule {
        name: "rename-layer",
        needles: &["\"__kg_m"],
        owners: &[],
        why: "the rename layer M4 deleted (D1.1)",
    },
    Rule {
        name: "type-key-literal",
        needles: &["TypeKey {"],
        owners: &["crates/kg-ty/src/table.rs"],
        why: "a node's type key built by hand; `TypeKey::at` owns it (D1.4)",
    },
    Rule {
        name: "message-test",
        needles: &["message == \"", "message.starts_with(", "message.contains(", "message.ends_with("],
        owners: &[],
        why: "a diagnostic's text as control flow; a diagnostic carries its kind (W8.1)",
    },
    Rule {
        name: "env-verdict",
        needles: &["env::var(", "env::var_os(", "env_flag!("],
        owners: &[
            // Front doors: an environment read there becomes a setting.
            "crates/kg-cli/",
            "crates/kg-lsp/",
            // Where the backend library is: locating, not deciding.
            "crates/kg-backend-sys/src/locate.rs",
            // Where a board flow's tools are, on PATH: locating likewise.
            "crates/kg-board/src/locate.rs",
            // The verified backend cache's switches: replay only.
            "crates/kg-ptbridge/src/cache.rs",
            // Build-time and recording tools, not the compiler.
            "crates/kg-abi-gen/",
            "crates/kg-ptrace/",
            // `corpus_absent`, test support; `env_flag!`'s definition.
            "crates/kg-base/src/lib.rs",
            "crates/kg-sem/src/lib.rs",
        ],
        why: "an environment variable that changes what the compiler decides, outside every memo key (W8.2)",
    },
];

/// Environment variables that only trace, time or check — allowed
/// anywhere (sprint §2: "an environment variable may only trace").
const TRACES: &[&str] = &[
    "KANAGAWA_ROOT",
    "NO_COLOR",
    "KG_LOOP_TIME",
    "KG_LOOP_LEFT",
    "KG_INFER_TIME",
    "KG_INFER_CHECK",
    "KG_SYMBOLS_CHECK",
    "KG_DEDUCE_TRACE",
    "KG_CENSUS_TRACE",
    "KG_CENSUS_DECIDED",
    "KG_WRAP_TRACE",
    "KG_LOOP_PROFILE",
    "KG_PHASES",
    "KG_ELAB_TRACE",
    "KG_ELAB_TRACE_REFUSED",
    "KG_SEM_DUMP",
    "KG_SEM_CTOR",
    "KG_SEM_COPIES",
    "KG_SEM_TEXT",
];

pub fn run() -> Result<()> {
    let root = crate::fresh::workspace();
    let mut files = Vec::new();
    collect(&root.join("crates"), &mut files);
    files.sort();
    let mut ledger = crate::ratchet::Ledger::new("lint");
    for rule in RULES {
        ledger.set(rule.name);
    }
    let mut graded = 0usize;
    for path in &files {
        let rel = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/");
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        graded += 1;
        let code = production(&text);
        for rule in RULES {
            if rule.owners.iter().any(|o| if o.ends_with('/') { rel.starts_with(o) } else { rel == *o }) {
                continue;
            }
            for (k, _) in occurrences(&code, rule).into_iter().enumerate() {
                ledger.add(rule.name, &format!("{rel} #{k}"));
            }
        }
    }
    ledger.total("graded", graded);
    println!("lint: {graded} production files");
    for rule in RULES {
        let items = &ledger.section.sets[rule.name];
        let mut per_file: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for item in items {
            *per_file.entry(item.rsplit_once(" #").map_or(item.as_str(), |(f, _)| f)).or_default() += 1;
        }
        println!("  {:<18} {:>4}   {}", rule.name, items.len(), rule.why);
        for (f, n) in per_file {
            println!("      {n:>3}  {f}");
        }
    }
    for rule in RULES {
        ledger.total(rule.name, ledger.section.sets[rule.name].len());
    }
    crate::ratchet::enforce(ledger)?;
    Ok(())
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            // A crate's `tests/`, `benches/` and `examples/` are not
            // production; `src/` and nested modules are.
            if !matches!(name.as_str(), "target" | "tests" | "benches" | "examples") && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if name.ends_with(".rs")
            && name != "build.rs"
            && name != "tests.rs"
            && path.components().any(|c| c.as_os_str() == "src")
        {
            out.push(path);
        }
    }
}

/// Where each needle of `rule` occurs in `code`, in order; for
/// `env-verdict`, only reads of a variable not in [`TRACES`].
fn occurrences(code: &str, rule: &Rule) -> Vec<usize> {
    let mut at = Vec::new();
    for needle in rule.needles {
        let mut from = 0;
        while let Some(i) = code[from..].find(needle) {
            let i = from + i;
            from = i + needle.len();
            // A struct literal's fields follow on its line; a function
            // returning the type opens its body instead.
            if rule.name == "type-key-literal" && code[from..].trim_start_matches([' ', '\t']).starts_with(['\n', '\r']) {
                continue;
            }
            if rule.name == "env-verdict" {
                let rest = code[from..].trim_start();
                let name = rest.strip_prefix('"').and_then(|r| r.split_once('"')).map(|(n, _)| n);
                if name.is_some_and(|n| TRACES.contains(&n)) {
                    continue;
                }
            }
            at.push(i);
        }
    }
    at.sort_unstable();
    at
}

/// `text` with comments blanked and every `#[cfg(test)]` item removed —
/// string and character literals kept, since a needle may be one.
fn production(text: &str) -> String {
    let (code, shape) = blank_comments(text);
    let mut out = code.into_bytes();
    let shape = shape.as_bytes();
    let marker = b"#[cfg(test)]";
    let mut i = 0;
    while i + marker.len() <= shape.len() {
        if &shape[i..i + marker.len()] != marker {
            i += 1;
            continue;
        }
        // The item the attribute is on: to the brace matching its first
        // `{`, or to its `;` when one comes first (`mod tests;`, a `use`).
        let start = i;
        let mut j = i + marker.len();
        let mut end = shape.len();
        while j < shape.len() {
            match shape[j] {
                b';' => {
                    end = j + 1;
                    break;
                }
                b'{' => {
                    let mut depth = 0usize;
                    while j < shape.len() {
                        match shape[j] {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                    end = (j + 1).min(shape.len());
                    break;
                }
                _ => j += 1,
            }
        }
        for b in &mut out[start..end] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        i = end;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Two views of `text`, byte for byte the same length: comments blanked,
/// and comments *and* literal contents blanked — the second is what braces
/// are counted in, so a `"{"` cannot unbalance an item.
fn blank_comments(text: &str) -> (String, String) {
    let b = text.as_bytes();
    let mut code = b.to_vec();
    let mut shape = b.to_vec();
    let blank = |v: &mut Vec<u8>, from: usize, to: usize| {
        for x in &mut v[from..to] {
            if *x != b'\n' {
                *x = b' ';
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n);
                blank(&mut code, i, end);
                blank(&mut shape, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let (mut j, mut depth) = (i + 2, 1usize);
                while j < b.len() && depth > 0 {
                    if b[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if b[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut code, i, j);
                blank(&mut shape, i, j);
                i = j;
            }
            b'r' if matches!(b.get(i + 1), Some(b'"' | b'#')) && (i == 0 || !is_ident(b[i - 1])) => {
                let hashes = b[i + 1..].iter().take_while(|&&c| c == b'#').count();
                if b.get(i + 1 + hashes) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let open = i + 2 + hashes;
                let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                let end = b[open..].windows(close.len()).position(|w| w == close.as_slice()).map_or(b.len(), |n| open + n);
                blank(&mut shape, open, end);
                i = (end + close.len()).min(b.len());
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut shape, i + 1, j.min(b.len()));
                i = j + 1;
            }
            // A character literal, not a lifetime: `'x'` or `'\…'`.
            b'\'' => {
                let lit = if b.get(i + 1) == Some(&b'\\') {
                    b[i + 2..].iter().position(|&c| c == b'\'').map(|n| i + 2 + n)
                } else {
                    let ch = text[i + 1..].chars().next().map_or(1, char::len_utf8);
                    (b.get(i + 1 + ch) == Some(&b'\'')).then_some(i + 1 + ch)
                };
                match lit {
                    Some(end) => {
                        blank(&mut shape, i + 1, end);
                        i = end + 1;
                    }
                    None => i += 1,
                }
            }
            _ => i += 1,
        }
    }
    (String::from_utf8_lossy(&code).into_owned(), String::from_utf8_lossy(&shape).into_owned())
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(rule: &str, text: &str) -> usize {
        let rule = RULES.iter().find(|r| r.name == rule).expect("a rule");
        occurrences(&production(text), rule).len()
    }

    /// A needle in production code counts; the same in a comment, a
    /// `#[cfg(test)]` module (braces in its strings notwithstanding) or a
    /// `#[cfg(test)]` function does not. Red with the test items kept, and
    /// red with comments kept.
    #[test]
    fn production_code_counts_and_tests_and_comments_do_not() {
        let text = r#"
fn reads(t: &str) -> bool { t.contains("<|") }
// t.contains("<|") in a comment
/* and "<|" in a block */
#[cfg(test)]
mod tests {
    fn f() { let s = "}"; let c = '{'; assert!(x.contains("<|")); }
}
#[cfg(test)]
fn helper() { "<|"; }
fn after(t: &str) -> bool { t.ends_with("|>") }
"#;
        assert_eq!(count("mangled-text", text), 2);
    }

    /// A literal counts; a function returning the type does not.
    #[test]
    fn a_type_key_literal_is_not_a_return_type() {
        let text = "fn f() -> kg_ty::TypeKey {\n    g()\n}\nfn h() { k(TypeKey { file, offset }) }\n";
        assert_eq!(count("type-key-literal", text), 1);
    }

    /// A read of a tracing variable is allowed; a verdict switch is not.
    #[test]
    fn only_verdict_switches_count() {
        let text = r#"fn f() { std::env::var_os("KG_LOOP_TIME"); std::env::var_os("KG_LEFTOVERS"); crate::env_flag!("KG_WRAP_TRACE"); crate::env_flag!("KG_X"); }"#;
        assert_eq!(count("env-verdict", text), 2);
    }

    /// A lifetime is not a character literal, and a raw string's quote is
    /// not a string's end.
    #[test]
    fn lifetimes_and_raw_strings_do_not_unbalance_the_scan() {
        let text = "fn f<'a>(x: &'a str) -> &'a str { r#\"\"}\"#; x }\n#[cfg(test)]\nmod t { fn g() { \"<|\"; } }\nfn h() { \"<|\"; }\n";
        assert_eq!(count("mangled-text", text), 1);
    }
}
