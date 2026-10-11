//! `--help` and `--version` output for the compat binary.
//!
//! The reference's help is `cmdargs` 0.10.22's, rendered from the mode
//! record, and this is a port of that renderer — `System.Console.CmdArgs`'s
//! `Explicit/Help.hs` (which help, laid out how) and `Text.hs` (columns and
//! wrapping) — over [`FLAGS`](super::flags::FLAGS) in each mode's `cmdargs`
//! order ([`mode_flags`]). Byte for byte the reference's but for the version
//! in the first line: `cargo xtask cli` diffs every mode's `--help` against
//! the reference binary's, that line masked.
//!
//! What the binary does and does not implement is said where a run reaches
//! it (`pretty`, the `--dump-*` flags), not here: a help screen that differs
//! from the reference's is a difference every wrapper script would see.

use super::flags::{FlagSpec, Group, Sink, mode_flags};
use super::options::Mode;

/// `makeReleaseName releaseVersion releaseBuildChannel` with the CPP macros
/// absent (`compiler/hs/app/ReleaseVersion.hs:24-51`). The reference's CMake
/// build defines them from `VERSION` (`compiler/hs/CMakeLists.txt:37-48`) —
/// its binary says 1.2.0 — and this build is not that release, so it keeps
/// the undefined-macro reading; the help's first line is the one place the
/// two may differ.
pub const NUMERIC_VERSION: &str = "0.0.0";

/// The program's `summary` (`CmdArgs.hs:171`): `--help`'s first line.
const SUMMARY: &str = "Kanagawa Compiler 0.0.0 (local), (C) Microsoft";

/// Two lines: the reference's summary, so anything grepping for it still works,
/// then an unmistakable statement of what this binary actually is.
pub fn version_text() -> String {
    format!(
        "{SUMMARY}\nkgrust `kanagawa` compatibility front end {}\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Whose help: the program's, when no mode word was given — `kanagawa
/// --help`, and `kanagawa a.k --help`, whose flags the program's own
/// (`zeroMode`'d) mode parses — or one mode's.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Scope {
    Program,
    Mode(Mode),
}

/// Which modes a help shows (`HelpFormat`, `Explicit/Help.hs:57-63`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Which {
    /// All of them if that is 40 entries or fewer, else one.
    Default,
    One,
    All,
}

/// How it is laid out (`TextFormat`, `Text.hs:55-57`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Layout {
    Html,
    Wrap(i64),
}

/// `--help[=FORMAT]`'s value.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Format {
    pub which: Which,
    pub layout: Layout,
}

impl Format {
    /// `def`: the default format, wrapped at 80 (`Text.hs:51-52`).
    pub const DEFAULT: Format = Format { which: Which::Default, layout: Layout::Wrap(80) };

    /// `--help=all`, which a bare `kanagawa` is (`Main.hs:37`).
    pub const ALL: Format = Format { which: Which::All, layout: Layout::Wrap(80) };

    /// `format` (`Explicit.hs:175-188`): words separated by space, `:` or
    /// `,`, case-folded, folded left over the default — so `all,120` is
    /// every mode at width 120 and a bare `--help` the default.
    ///
    /// `bash` and `zsh` print a completion script that calls the program
    /// back under `CMDARGS_COMPLETE`, which this binary does not answer, so
    /// they are refused rather than printed.
    pub fn parse(value: &str) -> Result<Format, String> {
        let folded: String =
            value.chars().map(|c| if c == ':' || c == ',' { ' ' } else { c.to_ascii_lowercase() }).collect();
        let mut format = Format::DEFAULT;
        for word in folded.split_whitespace() {
            match word {
                "all" => format.which = Which::All,
                "one" => format.which = Which::One,
                "def" => format.which = Which::Default,
                "html" => format.layout = Layout::Html,
                "text" => format.layout = Layout::Wrap(80),
                "bash" | "zsh" => {
                    return Err(format!("--help={word}: shell completion is not supported by this build"));
                }
                _ if word.chars().all(|c| c.is_ascii_digit()) => {
                    format.layout = Layout::Wrap(word.parse().unwrap_or(i64::MAX));
                }
                _ => return Err("unrecognised help format, expected one of: all one def html text <NUMBER>".to_owned()),
            }
        }
        Ok(format)
    }
}

/// The help screen (`helpText`, `Explicit/Help.hs:79-92`, with
/// `setHelp`'s naming, `Implicit/Global.hs:189-198`).
pub fn help_text(scope: Scope, format: Format) -> String {
    // `helpPrefix`: the summary and a blank line.
    let mut text = vec![Text::Line(SUMMARY.to_owned()), Text::Line(String::new())];
    let one = || match scope {
        Scope::Program => program_one(),
        // A mode has no modes under it, so its one and its all are the same.
        Scope::Mode(mode) => mode_text(mode, true),
    };
    let all = || match scope {
        Scope::Program => program_all(),
        Scope::Mode(mode) => mode_text(mode, true),
    };
    text.extend(match format.which {
        Which::One => one(),
        Which::All => all(),
        // Counted in entries, not in lines a wrap makes of them.
        Which::Default => {
            let all = all();
            if all.len() > 40 { one() } else { all }
        }
    });
    match format.layout {
        Layout::Wrap(width) => show_wrap(width, &text),
        Layout::Html => show_html(&text),
    }
}

/// One entry of help (`Text`, `Text.hs:70-71`): a paragraph, or a row of
/// columns whose last is wrapped.
#[derive(Clone, Debug)]
enum Text {
    Line(String),
    Cols(Vec<String>),
}

/// The program's mode is its default mode, `compile`, with every flag but
/// the common ones hidden and its argument nameless (`zeroMode`,
/// `Implicit/Global.hs:55-59`), named `kanagawa` and helped "Kanagawa
/// compiler" (`collapse`, `:34-38`).
fn program_head() -> Vec<Text> {
    // The nameless argument still adds a word: an empty one, which the wrap
    // drops and the HTML keeps (`helpArgs`, `Explicit/Help.hs:143-146`).
    vec![Text::Line("kanagawa [COMMAND] ... [OPTIONS] ".to_owned()), Text::Line("  Kanagawa compiler".to_owned())]
}

fn program_flags() -> Vec<Text> {
    space(groups(Mode::Compile, |g| g == Group::Common))
}

/// `helpTextOne` (`Explicit/Help.hs:116-121`): the program, its modes in a
/// table, its common flags.
fn program_one() -> Vec<Text> {
    let mut out = program_head();
    let mut modes = vec![Text::Line("Commands:".to_owned())];
    for mode in Mode::ALL {
        modes.push(cols(vec![first_name(mode).to_owned(), format!(" {}", mode.help())]));
    }
    out.extend(space(modes));
    out.extend(program_flags());
    out
}

/// `helpTextAll` (`Explicit/Help.hs:101-107`): the program, then each mode
/// under its full name, whose common flags `prepare` hid (`groupCommonHide`,
/// `Implicit/Global.hs:198`).
fn program_all() -> Vec<Text> {
    let mut out = program_head();
    out.extend(program_flags());
    for mode in Mode::ALL {
        out.push(Text::Line(String::new()));
        out.extend(mode_text(mode, false));
    }
    out
}

/// `[compile]` for the automatic mode (`toMode`, `Implicit/Global.hs:218`).
fn first_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Compile => "[compile]",
        Mode::Pretty => "pretty",
        Mode::ListDeps => "list-deps",
    }
}

/// `helpTextMode` (`Explicit/Help.hs:124-135`) of one mode, named under the
/// program; `common` keeps its common flags.
fn mode_text(mode: Mode, common: bool) -> Vec<Text> {
    // `FILE` repeated and optional: `[FILE]` (`collapseArgs`,
    // `Implicit/Global.hs:103-104`).
    let mut out = vec![
        Text::Line(format!("kanagawa {} [OPTIONS] [FILE]", first_name(mode))),
        Text::Line(format!("  {}", mode.help())),
    ];
    out.extend(space(groups(mode, |g| common || g != Group::Common)));
    out.extend(space(mode.details().iter().map(|l| Text::Line((*l).to_owned())).collect()));
    out
}

/// A mode's flags under their group headings: each flag's group is its own
/// or, undeclared, the one before it (`assignGroups`), the groups in order
/// of first appearance (`collapseFlags`, `Implicit/Global.hs:75-79`), the
/// flags in the mode's order within each (`helpGroup`,
/// `Explicit/Help.hs:138-140`). `shown` picks the groups not hidden.
fn groups(mode: Mode, shown: impl Fn(Group) -> bool) -> Vec<Text> {
    let mut grouped: Vec<(Group, Vec<&FlagSpec>)> = Vec::new();
    let mut previous: Option<Group> = None;
    for flag in mode_flags(mode) {
        let group = match flag.group {
            Group::Undeclared => previous.expect("an undeclared group follows a declared one"),
            g => g,
        };
        previous = Some(group);
        match grouped.iter_mut().find(|(g, _)| *g == group) {
            Some((_, flags)) => flags.push(flag),
            None => grouped.push((group, vec![flag])),
        }
    }
    let mut out = Vec::new();
    for (group, flags) in grouped.into_iter().filter(|(g, _)| shown(*g)) {
        out.push(Text::Line(format!("{}:", group.name().expect("resolved above"))));
        for flag in flags {
            out.push(flag_row(flag, mode));
        }
    }
    out
}

/// `helpFlag` (`Explicit/Help.hs:149-159`): the short names, the long ones
/// with the value's name on the first, the help.
fn flag_row(flag: &FlagSpec, mode: Mode) -> Text {
    let opt = match flag.sink {
        // `flagBool` and the built-ins take no value, or one only by `=`
        // (`FlagOptRare`), and show none.
        Sink::Bool(_) | Sink::Warnings(_) | Sink::Builtin(_) => String::new(),
        _ if flag.metavar.starts_with('[') => flag.metavar.to_owned(),
        _ => format!("={}", flag.metavar),
    };
    let short: Vec<String> =
        flag.short.filter(|_| flag.short_modes.accepts(mode)).map(|c| format!("-{c}")).into_iter().collect();
    let long: Vec<String> = flag
        .long_names()
        .enumerate()
        .map(|(i, n)| if i == 0 { format!("--{n}{opt}") } else { format!("--{n}") })
        .collect();
    cols(vec![short.join(" "), long.join(" "), format!(" {}", flag.help)])
}

/// `cols` (`Explicit/Help.hs:161`).
fn cols(xs: Vec<String>) -> Text {
    let mut out = Vec::with_capacity(xs.len());
    for (i, x) in xs.into_iter().enumerate() {
        out.push(if i == 0 { format!("  {x}") } else { format!(" {x}") });
    }
    Text::Cols(out)
}

/// `space` (`Explicit/Help.hs:162`): a blank line before anything at all.
fn space(xs: Vec<Text>) -> Vec<Text> {
    if xs.is_empty() {
        return xs;
    }
    let mut out = vec![Text::Line(String::new())];
    out.extend(xs);
    out
}

/// Haskell's `isSpace`: the Latin-1 spaces and controls, and Unicode's
/// space separators — not every `char::is_whitespace`, which takes in
/// U+0085 and the line and paragraph separators too.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{a0}')
        || (c > '\u{ff}' && c.is_whitespace() && !matches!(c, '\u{2028}' | '\u{2029}'))
}

fn len(s: &str) -> i64 {
    s.chars().count() as i64
}

/// The leading spaces of `s` and the rest (`span isSpace`).
fn span_space(s: &str) -> (&str, &str) {
    let at = s.char_indices().find(|&(_, c)| !is_space(c)).map_or(s.len(), |(i, _)| i);
    s.split_at(at)
}

/// `showWrap` (`Text.hs:87-102`).
fn show_wrap(width: i64, xs: &[Text]) -> String {
    // Every row with the same number of columns shares their widths.
    let mut widths: Vec<(usize, Vec<i64>)> = Vec::new();
    for x in xs {
        if let Text::Cols(cs) = x {
            let lens: Vec<i64> = cs[..cs.len() - 1].iter().map(|c| len(c)).collect();
            match widths.iter_mut().find(|(n, _)| *n == cs.len()) {
                Some((_, w)) => w.iter_mut().zip(&lens).for_each(|(w, l)| *w = (*w).max(*l)),
                None => widths.push((cs.len(), lens)),
            }
        }
    }
    let mut out = String::new();
    for x in xs {
        match x {
            Text::Line(line) => {
                let (indent, rest) = span_space(line);
                for l in wrap1(width - len(indent), rest) {
                    out.push_str(indent);
                    out.push_str(&l);
                    out.push('\n');
                }
            }
            Text::Cols(cs) => {
                let ys = &widths.iter().find(|(n, _)| *n == cs.len()).expect("measured above").1;
                let last = &cs[cs.len() - 1];
                let n = ys.iter().sum::<i64>() + len(span_space(last).0);
                let mut lines = wrap1(width - n, last).into_iter();
                for (c, w) in cs.iter().zip(ys) {
                    out.push_str(c);
                    out.extend(std::iter::repeat_n(' ', (w - len(c)).max(0) as usize));
                }
                out.push_str(&lines.next().expect("wrap1 is never empty"));
                out.push('\n');
                for l in lines {
                    out.extend(std::iter::repeat_n(' ', n.max(0) as usize));
                    out.push_str(&l);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// `wrap1` (`Text.hs:105-106`): at least one line.
fn wrap1(width: i64, x: &str) -> Vec<String> {
    let lines = wrap(width, x);
    if lines.is_empty() { vec![String::new()] } else { lines }
}

/// `wrap` (`Text.hs:109-122`): each line's words, each with the spaces after
/// it, joined while the joined length stays under `width` — so a run of
/// spaces inside a line survives, and the leading ones count against the
/// first line.
fn wrap(width: i64, x: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in haskell_lines(x) {
        // `split`: (word, spaces after it).
        let mut chunks: Vec<(String, i64)> = Vec::new();
        let mut rest = line;
        while !rest.is_empty() {
            let word_end = rest.char_indices().find(|&(_, c)| is_space(c)).map_or(rest.len(), |(i, _)| i);
            let (word, after) = rest.split_at(word_end);
            let (spaces, next) = span_space(after);
            chunks.push((word.to_owned(), len(spaces)));
            rest = next;
        }
        // `combine`.
        let mut chunks = chunks.into_iter();
        let Some(mut current) = chunks.next() else { continue };
        for (word, spaces) in chunks {
            if len(&current.0) + current.1 + len(&word) < width {
                current.0.extend(std::iter::repeat_n(' ', current.1.max(0) as usize));
                current.0.push_str(&word);
                current.1 = spaces;
            } else {
                out.push(std::mem::replace(&mut current, (word, spaces)).0);
            }
        }
        out.push(current.0);
    }
    out
}

/// Haskell's `lines`: split at `\n` only, no empty line after a final one.
fn haskell_lines(s: &str) -> Vec<&str> {
    let mut out: Vec<&str> = s.split('\n').collect();
    if out.last() == Some(&"") {
        out.pop();
    }
    out
}

/// `showHTML` (`Text.hs:128-154`), its unclosed `<td>` for an empty cell
/// included.
fn show_html(xs: &[Text]) -> String {
    let max_cols = xs.iter().filter_map(|x| if let Text::Cols(c) = x { Some(c.len()) } else { None }).max().unwrap_or(0);
    let td = |cols: usize, x: &str| {
        let (indent, body) = span_space(x);
        let mut styles = Vec::new();
        if !indent.is_empty() {
            styles.push(format!("padding-left:{}ex;", len(indent)));
        }
        if body.starts_with('-') {
            styles.push("white-space:nowrap;".to_owned());
        }
        let mut out = String::from("<td");
        if cols != 1 {
            out.push_str(&format!(" colspan='{cols}'"));
        }
        if !styles.is_empty() {
            out.push_str(&format!(" style='{}'", styles.join(" ")));
        }
        out.push('>');
        if body.is_empty() {
            out.push_str("&nbsp;");
        } else {
            for c in body.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '>' => out.push_str("&gt;"),
                    '<' => out.push_str("&lt;"),
                    '\n' => out.push_str("<br />"),
                    c => out.push(c),
                }
            }
            out.push_str("</td>");
        }
        out
    };
    let mut out = String::from("<table class='cmdargs'>\n");
    for x in xs {
        out.push_str("<tr>");
        match x {
            Text::Line(l) => out.push_str(&td(max_cols, l)),
            Text::Cols(cs) => {
                for c in &cs[..cs.len() - 1] {
                    out.push_str(&td(1, c));
                }
                out.push_str(&td(max_cols + 1 - cs.len(), &cs[cs.len() - 1]));
            }
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference's `kanagawa list-deps --help` (1.2.0, 2026-10-09),
    /// its version line put back to this build's.
    const LIST_DEPS_HELP: &str = "\
Kanagawa Compiler 0.0.0 (local), (C) Microsoft

kanagawa list-deps [OPTIONS] [FILE]
  List the transitive .k source files needed to compile (parse only; no
  frontend or codegen)

Compiler:
  -b --base-library=FILE     Base library file path
  -d --define=name#val       Define a global constant
     --import-dir=DIR        Directories to search for imported files
  -n --no-implicit-base      Disable implicit import of base library
  -p --parse-docs            Parse documentation comments
  -u --using=name#type       Define a global type alias
Output:
  -f --file-list=FILE        Update specified file whenever list compiled
                             files (including imported files) changes
Code generation:
  -t --target-device=STRING  Target FPGA device name
Common flags:
  -? --help                  Display help message
  -V --version               Print version information
     --numeric-version       Print just the version number
  -v --verbose               Loud verbosity
  -q --quiet                 Quiet verbosity

Usage example:
    Print the dependency list for source.k to stdout:
      kanagawa list-deps source.k
    Write the dependency list to a file:
      kanagawa list-deps --file-list=deps.txt source.k
";

    /// The reference's `kanagawa --help`, likewise.
    const PROGRAM_HELP: &str = "\
Kanagawa Compiler 0.0.0 (local), (C) Microsoft

kanagawa [COMMAND] ... [OPTIONS]
  Kanagawa compiler

Commands:
  [compile]  Compile Kanagawa program
  pretty     Parse and pretty print source file(s)
  list-deps  List the transitive .k source files needed to compile (parse
             only; no frontend or codegen)

Common flags:
  -? --help             Display help message
  -V --version          Print version information
     --numeric-version  Print just the version number
  -v --verbose          Loud verbosity
  -q --quiet            Quiet verbosity
";

    /// The reference's `kanagawa pretty --help`, likewise: `--indent` and
    /// the other three without a `groupname` sit under the group of the
    /// field before them.
    const PRETTY_HELP: &str = "\
Kanagawa Compiler 0.0.0 (local), (C) Microsoft

kanagawa pretty [OPTIONS] [FILE]
  Parse and pretty print source file(s)

Compiler:
  -b --base-library=FILE     Base library file path
  -d --define=name#val       Define a global constant
     --import-dir=DIR        Directories to search for imported files
  -i --indent=INT            Number of spaces per indentation level; default
                             4
  -l --layout=LAYOUT         Layout variant to be used: pretty or smart
  -w --line-width=INT        Width of the page in characters; default 80
  -n --no-implicit-base      Disable implicit import of base library
  -p --parse-docs            Parse documentation comments
  -r --ribbon=NUM            Fraction of line that should be filled before
                             trying to break it up; default 1.0
  -u --using=name#type       Define a global type alias
Diagnostics:
     --dump-parse            Dump abstract syntax tree(s)
Code generation:
  -t --target-device=STRING  Target FPGA device name
Common flags:
  -? --help                  Display help message
  -V --version               Print version information
     --numeric-version       Print just the version number
  -v --verbose               Loud verbosity
  -q --quiet                 Quiet verbosity
";

    #[test]
    fn list_deps_help_is_the_reference_s() {
        assert_eq!(help_text(Scope::Mode(Mode::ListDeps), Format::DEFAULT), LIST_DEPS_HELP);
        // A mode with none under it: its one and its all are one text.
        assert_eq!(help_text(Scope::Mode(Mode::ListDeps), Format::ALL), LIST_DEPS_HELP);
    }

    #[test]
    fn the_program_s_help_lists_its_modes_and_its_common_flags() {
        assert_eq!(help_text(Scope::Program, Format::DEFAULT), PROGRAM_HELP);
    }

    #[test]
    fn pretty_help_puts_ungrouped_flags_under_the_group_before_them() {
        assert_eq!(help_text(Scope::Mode(Mode::Pretty), Format::DEFAULT), PRETTY_HELP);
    }

    /// `--help=all` is every mode under the program, each without the
    /// common flags the program already lists, and every flag row aligned
    /// to the widest of them all (`showWrap`'s `cs`).
    #[test]
    fn help_all_lists_every_mode_aligned_to_one_column() {
        let text = help_text(Scope::Program, Format::ALL);
        assert_eq!(text.matches("Common flags:").count(), 1, "{text}");
        assert!(text.contains("\nkanagawa [compile] [OPTIONS] [FILE]\n"), "{text}");
        assert!(text.contains("\nkanagawa list-deps [OPTIONS] [FILE]\n"), "{text}");
        assert!(text.contains("\n  -? --help                             Display help message\n"), "{text}");
        assert!(text.contains("\n  -b --base-library=FILE                Base library file path\n"), "{text}");
        assert!(text.contains("\n     --backend=BACKEND --target         Code generation backend: sv\n"), "{text}");
        assert!(text.contains("\n     --frontend-passes[=INT]            Subset of frontend passes to run,\n"), "{text}");
    }

    /// The width counts the leading spaces of a column's first line:
    /// "…list compiled" plus " files" is 79 characters, under 80, and still
    /// breaks, because the first chunk is the two spaces before "Update".
    #[test]
    fn wrapping_counts_the_column_s_own_indent() {
        assert_eq!(wrap(51, "  Update specified file whenever list compiled files"), [
            "  Update specified file whenever list compiled",
            "files"
        ]);
        assert_eq!(wrap(40, "a  b"), ["a  b"], "a run of spaces inside a line survives");
        assert_eq!(wrap(3, "abcdef gh"), ["abcdef", "gh"], "a long word is never split");
    }

    #[test]
    fn help_formats_are_cmdargs_words() {
        assert_eq!(Format::parse(""), Ok(Format::DEFAULT));
        assert_eq!(Format::parse("ALL"), Ok(Format::ALL));
        assert_eq!(Format::parse("all,120"), Ok(Format { which: Which::All, layout: Layout::Wrap(120) }));
        assert_eq!(Format::parse("one:html"), Ok(Format { which: Which::One, layout: Layout::Html }));
        assert_eq!(
            Format::parse("wide"),
            Err("unrecognised help format, expected one of: all one def html text <NUMBER>".to_owned())
        );
        assert!(Format::parse("bash").is_err());
    }

    /// `showHTML`, down to its `&nbsp;` cell that is never closed.
    #[test]
    fn html_help_is_cmdargs_table() {
        let html = help_text(Scope::Program, Format { which: Which::One, layout: Layout::Html });
        assert!(html.starts_with("<table class='cmdargs'>\n<tr><td colspan='3'>Kanagawa Compiler"), "{html}");
        assert!(html.contains("<tr><td colspan='3'>&nbsp;</tr>\n"), "{html}");
        assert!(html.contains("<tr><td colspan='3'>kanagawa [COMMAND] ... [OPTIONS] </td></tr>\n"), "{html}");
        assert!(html.contains(
            "<tr><td style='padding-left:2ex;'>&nbsp;<td style='padding-left:1ex; white-space:nowrap;'>--numeric-version</td>"
        ), "{html}");
        assert!(html.ends_with("</table>\n"), "{html}");
    }
}
