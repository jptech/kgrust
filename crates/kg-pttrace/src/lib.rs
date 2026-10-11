//! The trace format of `docs/spec/abi.md` §14.1, in one place so that the
//! two writers — `kg-ptrace`, which records what the *reference* front end
//! tells the backend, and the bridge's trace backend, which writes what *we*
//! would tell it — can never drift apart, and `xtask ptdiff` reads one
//! grammar.
//!
//! One line per call:
//!
//! ```text
//! <seq> <depth> <fn> <arg>* -> <ret>
//! ```
//!
//! `seq` is eight zero-padded decimal digits, allocated at *entry* so nested
//! calls sort after their caller; `depth` is the nesting at entry, `0` for
//! a call from outside the library. Arguments are rendered by type
//! ([`Arg`]); a node pointer is its logical id, `n<id>`, assigned on first
//! sighting whether as a result or an argument ([`Ids`]), and `NULL` is `_`.
//! Lines starting with `#` are out of band: `#version`, `#options` (once,
//! on `InitCompiler`), `#epoch` (written right after the `Codegen` line,
//! after which the id map is cleared and the counter is not — a recycled
//! address is a new id, §14.2).
//!
//! Two departures from §14.1, both because the reference's own bytes are not
//! reproducible: the `Options` argument renders as `<opts>` rather than a
//! hash of the struct — its padding and three fields are indeterminate
//! (§3.2) — and the `#options` line is the struct's fields by name, which
//! the differ compares field by field.

use std::fmt::Write as _;
use std::io::Write;

use rustc_hash::FxHashMap;

/// A source span as `SetLocation2` carries it (§2.2): 1-based, `file` an
/// index into `_fileNames`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub begin_line: usize,
    pub begin_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub file: usize,
    pub valid: bool,
}

impl Location {
    /// `@<file>:<bl>:<bc>-<el>:<ec>:<valid>`.
    pub fn render(&self) -> String {
        format!(
            "@{}:{}:{}-{}:{}:{}",
            self.file,
            self.begin_line,
            self.begin_column,
            self.end_line,
            self.end_column,
            u8::from(self.valid)
        )
    }
}

/// One argument or result, already lifted out of C.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    /// A `ParseTreeNodePtr`: its logical id, or `None` for `NULL`.
    Node(Option<u64>),
    /// A `const char*`: the bytes, or `None` for `NULL`.
    Str(Option<Vec<u8>>),
    /// A NUL-terminated `const char**` in array order, or `None` for `NULL`.
    Scope(Option<Vec<Vec<u8>>>),
    UInt(u64),
    Int(i64),
    F32(f32),
    /// A `const Location*` or a `Location` by value; `None` for a null pointer.
    Loc(Option<Location>),
    /// A `const Options*`; rendered `<opts>`, the fields go on `#options`.
    Options,
    /// A `void` result.
    Void,
}

impl Arg {
    /// The line rendering of §14.1.
    pub fn render(&self, out: &mut String) {
        match self {
            Arg::Node(None) | Arg::Str(None) | Arg::Scope(None) | Arg::Loc(None) => out.push('_'),
            Arg::Node(Some(id)) => {
                let _ = write!(out, "n{id}");
            }
            Arg::Str(Some(bytes)) => render_string(bytes, out),
            Arg::Scope(Some(items)) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    render_string(item, out);
                }
                out.push(']');
            }
            Arg::UInt(n) => {
                let _ = write!(out, "{n}");
            }
            Arg::Int(n) => {
                let _ = write!(out, "{n}");
            }
            // `{:?}` of an f32 is the shortest round-tripping decimal.
            Arg::F32(x) => {
                let _ = write!(out, "{x:?}");
            }
            Arg::Loc(Some(loc)) => out.push_str(&loc.render()),
            Arg::Options => out.push_str("<opts>"),
            Arg::Void => out.push('-'),
        }
    }
}

/// C-escaped, double-quoted: printable ASCII as is (`"` and `\` escaped),
/// everything else as `\xNN`, so a line never contains a raw newline or a
/// space inside a string field.
pub fn render_string(bytes: &[u8], out: &mut String) {
    out.push('"');
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            0x21..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\x{b:02x}");
            }
        }
    }
    out.push('"');
}

/// Pointer → logical id, first sighting wins, never reused (§14.2).
#[derive(Debug, Default)]
pub struct Ids {
    map: FxHashMap<usize, u64>,
    next: u64,
}

impl Ids {
    /// The id for `ptr`, minting one if this is its first sighting. `0` is
    /// `NULL` and is never mapped.
    pub fn node(&mut self, ptr: usize) -> Option<u64> {
        if ptr == 0 {
            return None;
        }
        Some(*self.map.entry(ptr).or_insert_with(|| {
            self.next += 1;
            self.next
        }))
    }

    /// Forget every address; keep counting. Called at an epoch boundary.
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// Renders and writes lines; owns the sequence counter and the ids.
pub struct Writer {
    out: Box<dyn Write + Send>,
    seq: u64,
    epoch: u64,
    ids: Ids,
    line: String,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer").field("seq", &self.seq).field("epoch", &self.epoch).finish_non_exhaustive()
    }
}

impl Writer {
    pub fn new(out: Box<dyn Write + Send>) -> Writer {
        Writer { out, seq: 0, epoch: 0, ids: Ids::default(), line: String::new() }
    }

    /// Allocate the next sequence number — at entry, before the real call.
    pub fn next_seq(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    pub fn ids(&mut self) -> &mut Ids {
        &mut self.ids
    }

    /// `#version 1 <libpath> <abi-hash>`.
    pub fn version(&mut self, lib: &str, abi_hash: &str) -> std::io::Result<()> {
        writeln!(self.out, "#version 1 {lib} {abi_hash}")
    }

    /// `#options <fields>` — `fields` is the writer's own rendering of the
    /// struct, one token per field, no newlines.
    pub fn options(&mut self, fields: &str) -> std::io::Result<()> {
        writeln!(self.out, "#options {fields}")
    }

    /// `#epoch <n>`: the id map is cleared, the counter runs on.
    pub fn epoch(&mut self) -> std::io::Result<()> {
        self.epoch += 1;
        self.ids.clear();
        writeln!(self.out, "#epoch {}", self.epoch)
    }

    /// One call line.
    pub fn call(&mut self, seq: u64, depth: u32, name: &str, args: &[Arg], ret: &Arg) -> std::io::Result<()> {
        self.line.clear();
        let _ = write!(self.line, "{seq:08} {depth} {name}");
        for a in args {
            self.line.push(' ');
            a.render(&mut self.line);
        }
        self.line.push_str(" -> ");
        ret.render(&mut self.line);
        self.line.push('\n');
        self.out.write_all(self.line.as_bytes())
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// A parsed line, for the differ. Arguments are kept as their rendered
/// tokens: the differ compares text, and a token is self-describing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    Call { seq: u64, depth: u32, name: String, args: Vec<String>, ret: String },
    /// `#version`, `#options`, `#epoch`: the keyword and the rest of the line.
    Meta { kind: String, rest: String },
}

/// Split one line. `None` for a blank line; an error names what was wrong.
pub fn parse_line(line: &str) -> Result<Option<Line>, String> {
    let line = line.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return Ok(None);
    }
    if let Some(rest) = line.strip_prefix('#') {
        let (kind, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        return Ok(Some(Line::Meta { kind: kind.to_owned(), rest: rest.to_owned() }));
    }
    let mut tokens = split_tokens(line)?;
    let arrow = tokens.iter().rposition(|t| t == "->").ok_or_else(|| format!("no `->` in {line:?}"))?;
    let ret = tokens.split_off(arrow + 1).pop().ok_or_else(|| format!("no result after `->` in {line:?}"))?;
    tokens.pop();
    if tokens.len() < 3 {
        return Err(format!("too few fields in {line:?}"));
    }
    let seq = tokens[0].parse().map_err(|_| format!("bad seq in {line:?}"))?;
    let depth = tokens[1].parse().map_err(|_| format!("bad depth in {line:?}"))?;
    let name = std::mem::take(&mut tokens[2]);
    let args = tokens.drain(3..).collect();
    Ok(Some(Line::Call { seq, depth, name, args, ret }))
}

/// Space-separated tokens, where a `"…"` string (possibly inside `[…]`)
/// may contain escaped quotes but never a raw space — `render_string`
/// guarantees that — so splitting on spaces outside quotes is exact.
fn split_tokens(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut escape = false;
    for c in line.chars() {
        if in_str {
            cur.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            ' ' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            '"' => {
                in_str = true;
                cur.push(c);
            }
            _ => cur.push(c),
        }
    }
    if in_str {
        return Err(format!("unterminated string in {line:?}"));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Every line of a trace, in order.
pub fn parse(text: &str) -> Result<Vec<Line>, String> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if let Some(line) = parse_line(l).map_err(|e| format!("line {}: {e}", i + 1))? {
            out.push(line);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_round_trips() {
        let mut w = Writer::new(Box::new(Vec::new()));
        let n1 = w.ids().node(0x1000);
        let seq = w.next_seq();
        let mut buf = String::new();
        let args = [
            Arg::Node(n1),
            Arg::Node(None),
            Arg::Str(Some(b"a \"b\"\n".to_vec())),
            Arg::Scope(Some(vec![b"@x".to_vec(), b"y".to_vec()])),
            Arg::UInt(4),
            Arg::Int(-3),
            Arg::F32(0.001),
            Arg::Loc(Some(Location {
                begin_line: 1,
                begin_column: 2,
                end_line: 3,
                end_column: 4,
                file: 0,
                valid: true,
            })),
        ];
        for a in &args {
            buf.push(' ');
            a.render(&mut buf);
        }
        assert_eq!(buf, r#" n1 _ "a\x20\"b\"\x0a" ["@x","y"] 4 -3 0.001 @0:1:2-3:4:1"#);
        let ret = Arg::Node(w.ids().node(0x2000));
        w.call(seq, 0, "ParseDeclare", &args, &ret).unwrap();
        let line = format!("00000000 0 ParseDeclare{buf} -> n2");
        match parse_line(&line).unwrap().unwrap() {
            Line::Call { seq, depth, name, args, ret } => {
                assert_eq!((seq, depth, name.as_str(), ret.as_str()), (0, 0, "ParseDeclare", "n2"));
                assert_eq!(args.len(), 8);
                assert_eq!(args[2], r#""a\x20\"b\"\x0a""#);
                assert_eq!(args[3], r#"["@x","y"]"#);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn ids_are_first_sighting_and_never_reused() {
        let mut ids = Ids::default();
        assert_eq!(ids.node(0), None);
        assert_eq!(ids.node(10), Some(1));
        assert_eq!(ids.node(20), Some(2));
        assert_eq!(ids.node(10), Some(1));
        ids.clear();
        assert_eq!(ids.node(10), Some(3));
    }

    #[test]
    fn meta_lines_parse() {
        assert_eq!(
            parse_line("#epoch 1").unwrap().unwrap(),
            Line::Meta { kind: "epoch".into(), rest: "1".into() }
        );
        assert!(parse_line("").unwrap().is_none());
    }
}
