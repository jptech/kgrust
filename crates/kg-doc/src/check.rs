//! The Kanagawa code blocks in documentation, checked by this frontend —
//! `kg doc --check`.
//!
//! The reference checks its Markdown documentation with `chkdoc`
//! (`tools/chkdoc/chkdoc.cpp`; `docs/spec/cli.md` §8.9), which compiles every
//! fenced block it takes through the whole compiler. This takes the same
//! blocks by the same conventions, and checks them with the frontend:
//!
//! - **Which blocks.** A fenced block tagged `kanagawa`, in a Markdown file
//!   or a doc comment. An untagged fence may hold anything, shell as readily
//!   as code, so it is passed over and counted, and the command says how many
//!   ([`FileReport::untagged`]). `chkdoc` instead compiles every fence but
//!   those tagged `C++`, `bash` or `md` (`chkdoc.cpp:326-333`), so its docs'
//!   untagged blocks are Kanagawa and so are `sandcastle.md`'s `cpp` ones;
//!   [`Select::Chkdoc`] is that rule, for documents written to it, and in a
//!   doc comment the tag alone still counts: the reference checks no doc
//!   comment, and its library's untagged fences hold other things, a hex key
//!   in `library/data/random/toeplitz.k:18-27`.
//! - **Opting out.** `<!-- invalid -->` on the line before the fence
//!   (`chkdoc.cpp:185-188`).
//! - **Snippets.** A block is checked as a whole file, then as the body of
//!   a function — `chkdoc`'s two contexts, `tools/chkdoc/contexts/
//!   emptyfile.k` and `emptyfunction.k` — each against the project's base
//!   library and then against the library's `mini-base.k`; it passes when
//!   any one compiles (`chkdoc.cpp:204-221`). `<!-- name.k -->` before the
//!   fence names a context of its own instead, a file beside the document
//!   whose one `***` line is where the block goes, checked once against
//!   the base library (`chkdoc.cpp:79-101,190-196`).
//! - **Modules.** A block whose first line is `module a.b` is also the file
//!   `a/b.k` beside the document, for the blocks after it to import
//!   (`chkdoc.cpp:136-173`, which writes it into its working directory).
//! - **Structure.** An empty block and a fence never closed are errors
//!   (`chkdoc.cpp:338-341,386-389`).
//!
//! The blocks are found as `kg doc` renders documentation — CommonMark,
//! `pulldown-cmark` — so a fence indented in a list is a block, which
//! `chkdoc`'s line scanner, matching ```` ``` ```` at a line's start only,
//! does not see. Every offset of a block's text keeps where it came from in
//! the file holding it, and a diagnostic is moved back there: it points at
//! the block's own line and column in the document or the doc comment.

use std::sync::Arc;

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::{FileId, FileRange, LineIndex, TextRange, TextSize};
use kg_config::Settings;
use kg_db::Db;
use kg_diag::Diagnostic;
use kg_vfs::Vfs;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

/// Which fenced blocks of a Markdown file are Kanagawa.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Select {
    /// Tagged `kanagawa`.
    #[default]
    Kanagawa,
    /// Every block but those tagged `C++`, `bash` or `md`, by the info
    /// string's prefix, as `chkdoc` selects them (`chkdoc.cpp:330-333`).
    Chkdoc,
}

impl Select {
    fn takes(self, info: &str) -> bool {
        match self {
            Select::Kanagawa => language(info).eq_ignore_ascii_case("kanagawa"),
            Select::Chkdoc => !["C++", "bash", "md"].iter().any(|other| info.starts_with(other)),
        }
    }
}

/// The info string's first word, its language by CommonMark's convention.
fn language(info: &str) -> &str {
    info.split(|c: char| c.is_whitespace() || c == ',').next().unwrap_or("")
}

/// What the HTML comment on the line before a fence says about the block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive {
    None,
    /// `<!-- invalid -->`: not checked.
    Invalid,
    /// `<!-- name.k -->`: checked in that context.
    Context(String),
}

/// One Kanagawa code block, and where each piece of its text came from.
#[derive(Clone, Debug)]
pub struct Block {
    /// The opening fence's line, from the fence on, in the file holding the
    /// block: where a problem with the block as a whole is reported.
    pub fence: TextRange,
    /// Where the block ends in that file: its closing fence, or, never
    /// closed, the end of its text.
    pub end: TextSize,
    pub closed: bool,
    pub info: String,
    pub directive: Directive,
    /// The directive's comment, where there is one.
    pub directive_range: Option<TextRange>,
    /// The code, its lines each ending in a newline.
    pub text: String,
    pieces: Vec<Piece>,
}

/// A run of a block's text — one line, at most — and where it starts in the
/// file. `exact` when the text is the file's bytes there, so every offset
/// inside maps one for one; otherwise (a tab the parser expanded) the run
/// maps to its start.
#[derive(Copy, Clone, Debug)]
struct Piece {
    at: u32,
    host: u32,
    len: u32,
    exact: bool,
}

impl Block {
    /// Where `at`, an offset into [`Block::text`], is in the file holding
    /// the block. Past the text is the block's end.
    pub fn host(&self, at: TextSize) -> TextSize {
        let at = u32::from(at);
        if at as usize >= self.text.len() {
            return self.end;
        }
        let Some(piece) = self.pieces.partition_point(|p| p.at <= at).checked_sub(1).map(|i| self.pieces[i]) else {
            return self.fence.start();
        };
        let delta = at - piece.at;
        TextSize::from(if piece.exact && delta <= piece.len { piece.host + delta } else { piece.host })
    }

    /// `chkdoc`'s module block: a first line of exactly `module` and a
    /// name (`chkdoc.cpp:140-148`), the file a later block's `import` finds
    /// — `a/b.k` for `a.b`.
    pub fn module_path(&self) -> Option<Utf8PathBuf> {
        let first = self.text.lines().next()?;
        let mut words = first.split_whitespace();
        let (Some("module"), Some(name), None) = (words.next(), words.next(), words.next()) else { return None };
        let segments: Vec<&str> = name.split('.').filter(|s| !s.is_empty()).collect();
        let (last, dirs) = segments.split_last()?;
        let mut path: Utf8PathBuf = dirs.iter().collect();
        path.push(format!("{last}.k"));
        Some(path)
    }
}

/// The Kanagawa blocks of a Markdown file's `text`, in order.
pub fn markdown_blocks(text: &str, select: Select) -> Vec<Block> {
    blocks_in(text, &|info| select.takes(info), &|at| TextSize::from(at as u32))
}

/// How many fenced blocks of a Markdown file's `text` have no tag, and so
/// are Kanagawa only by [`Select::Chkdoc`].
fn untagged_blocks(text: &str) -> usize {
    Parser::new_ext(text, crate::markdown::options())
        .filter(|event| matches!(event, Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if language(info).is_empty()))
        .count()
}

/// The `kanagawa` blocks in the doc comments of the file `root` parses,
/// in order: every documented element's documentation, as `kg doc` renders
/// it ([`crate::markdown::to_html`]).
pub fn doc_comment_blocks(root: &kg_syntax::SyntaxNode) -> Vec<Block> {
    let docs = kg_syntax::file_docs(root);
    let mut out = Vec::new();
    for (element, doc) in docs.documented() {
        let Some(lines) = docs.lines_of(element) else { continue };
        let starts = lines.of_text(doc);
        // `to_commonmark` keeps the text's lines one for one, code and fences
        // verbatim (`kg_syntax::to_commonmark`), so a line of what it makes is
        // the line of the text, and of the source, at the same index.
        let markdown = kg_syntax::to_commonmark(&doc.text(), kg_syntax::Headings::Keep);
        let index = LineIndex::new(&markdown);
        let first = starts.iter().flatten().next().copied().unwrap_or_default();
        let host = |at: usize| {
            let pos = index.line_col(TextSize::from(at as u32));
            match starts.get(pos.line as usize).copied().flatten() {
                Some(start) => start + TextSize::from(pos.col),
                None => starts[..(pos.line as usize).min(starts.len())].iter().rev().flatten().next().copied().unwrap_or(first),
            }
        };
        out.extend(blocks_in(&markdown, &|info| Select::Kanagawa.takes(info), &host));
    }
    out
}

/// The fenced blocks `select` takes in Markdown `md`; `host` maps an offset
/// of `md` to the file holding it.
fn blocks_in(md: &str, select: &dyn Fn(&str) -> bool, host: &dyn Fn(usize) -> TextSize) -> Vec<Block> {
    let mut out = Vec::new();
    let mut open: Option<(std::ops::Range<usize>, String)> = None;
    let mut text = String::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut content_end = 0usize;
    for (event, range) in Parser::new_ext(md, crate::markdown::options()).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) if select(&info) => {
                open = Some((range.clone(), info.to_string()));
                text.clear();
                pieces.clear();
                content_end = md[range.start..].find('\n').map_or(md.len(), |i| range.start + i + 1);
            }
            Event::Text(code) if open.is_some() => {
                if md.get(range.clone()) == Some(&*code) {
                    let mut at = range.start;
                    for line in code.split_inclusive('\n') {
                        pieces.push(Piece {
                            at: text.len() as u32,
                            host: u32::from(host(at)),
                            len: line.len() as u32,
                            exact: true,
                        });
                        text.push_str(line);
                        at += line.len();
                    }
                } else {
                    pieces.push(Piece { at: text.len() as u32, host: u32::from(host(range.start)), len: code.len() as u32, exact: false });
                    text.push_str(&code);
                }
                content_end = content_end.max(range.end);
            }
            Event::End(TagEnd::CodeBlock) => {
                let Some((range, info)) = open.take() else { continue };
                out.push(block(md, range, info, std::mem::take(&mut text), std::mem::take(&mut pieces), content_end, host));
            }
            _ => {}
        }
    }
    out
}

fn block(
    md: &str,
    range: std::ops::Range<usize>,
    info: String,
    text: String,
    pieces: Vec<Piece>,
    content_end: usize,
    host: &dyn Fn(usize) -> TextSize,
) -> Block {
    let fence_line_end = md[range.start..].find('\n').map_or(md.len(), |i| range.start + i);
    let fence_text = md[range.start..fence_line_end].trim_end();
    let fence = TextRange::at(host(range.start), TextSize::of(fence_text));
    let marker: String = {
        let fence_text = fence_text.trim_start();
        let c = fence_text.chars().next().unwrap_or('`');
        fence_text.chars().take_while(|&x| x == c).collect()
    };
    // The closing fence is what the block's extent holds past its code; one
    // never closed runs to its container's end with nothing there.
    let from = content_end.min(range.end);
    let close = md[from..range.end].find(&marker).map(|i| from + i);
    let end = host(close.unwrap_or(from));

    // The line before the fence's: `chkdoc`'s `prevLine` (`chkdoc.cpp:376-383`).
    let line_start = md[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let (directive, directive_range) = match line_start.checked_sub(1) {
        Some(previous_end) => {
            let previous_start = md[..previous_end].rfind('\n').map_or(0, |i| i + 1);
            let line = &md[previous_start..previous_end];
            let comment = line.trim().trim_start_matches(['>', ' ']);
            let at = previous_start + (line.len() - line.trim_start().len());
            let range = TextRange::at(host(at), TextSize::of(line.trim()));
            match directive(comment) {
                Directive::None => (Directive::None, None),
                d => (d, Some(range)),
            }
        }
        None => (Directive::None, None),
    };
    Block { fence, end, closed: close.is_some(), info, directive, directive_range, text, pieces }
}

/// `StripHtmlComment` (`chkdoc.cpp:14-29`) and what `TestCodeBlock` makes of
/// it (`:185-196`). `chkdoc` takes any other comment for a context's file
/// name, and fails to open it; here only a name ending in `.k` is one, and
/// any other comment — a linter's pragma — says nothing.
fn directive(line: &str) -> Directive {
    let Some(inner) = line.strip_prefix("<!--").and_then(|l| l.strip_suffix("-->")) else { return Directive::None };
    let inner = inner.trim();
    if inner == "invalid" {
        Directive::Invalid
    } else if inner.ends_with(".k") && !inner.contains(char::is_whitespace) {
        Directive::Context(inner.to_owned())
    } else {
        Directive::None
    }
}

// ---------------------------------------------------------------------------
// checking
// ---------------------------------------------------------------------------

/// A context: the text before the block and the text after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub name: String,
    pub before: String,
    pub after: String,
}

impl Context {
    /// `tools/chkdoc/contexts/emptyfile.k`: the block as a whole file.
    pub fn file() -> Context {
        Context::parse("emptyfile.k", "// an empty file\n***\n").expect("one `***` line")
    }

    /// `tools/chkdoc/contexts/emptyfunction.k`: the block as a function's
    /// body. Its `;` after the block lets the last statement go without
    /// its own.
    pub fn body() -> Context {
        Context::parse("emptyfunction.k", "// an empty function\ninline void FunctionName()\n{\n***\n;\n}").expect("one `***` line")
    }

    /// A context file's text, split at its one `***` line; `Err` with how
    /// many it has otherwise (`chkdoc.cpp:79-101`). `chkdoc` writes each line
    /// back with a newline after it, the last one too.
    pub fn parse(name: &str, text: &str) -> Result<Context, usize> {
        let lines: Vec<&str> = text.lines().collect();
        let marks: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == "***").collect();
        let [mark] = marks[..] else { return Err(marks.len()) };
        let joined = |part: &[&str]| part.iter().map(|l| format!("{l}\n")).collect::<String>();
        Ok(Context { name: name.to_owned(), before: joined(&lines[..mark]), after: joined(&lines[mark + 1..]) })
    }
}

/// How a block was checked, for a person: in the context named `context`,
/// against `mini-base.k` or not — "as the body of a function".
pub fn how(context: &str, mini_base: bool) -> String {
    let mut how = match context {
        "emptyfile.k" => "as a whole file".to_owned(),
        "emptyfunction.k" => "as the body of a function".to_owned(),
        other => format!("in the context `{other}`"),
    };
    if mini_base {
        how.push_str(", against `mini-base.k`");
    }
    how
}

/// What became of one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It compiled in `context`, against the base library or `mini-base.k`.
    Passed { context: String, mini_base: bool },
    /// `<!-- invalid -->`.
    Skipped,
    Failed,
}

/// One block's verdict and, when it failed, why.
#[derive(Clone, Debug)]
pub struct Checked {
    /// The opening fence's line in the file, 1-based.
    pub line: u32,
    pub verdict: Verdict,
    /// For a failed block, the diagnostics of the try that got furthest into
    /// it, every range that was in the block's text moved to where that is
    /// in the file; or the error about the block itself.
    pub diagnostics: Vec<Diagnostic>,
}

/// The blocks of one file, checked.
#[derive(Clone, Debug)]
pub struct FileReport {
    pub file: FileId,
    pub blocks: Vec<Checked>,
    /// Fenced blocks of a Markdown file passed over for having no tag. A check
    /// that reads none of a document's code must not look like a clean one.
    pub untagged: usize,
}

/// Checks code blocks with the frontend: one database over the project's
/// settings, and one over the same with `mini-base.k` for the base where the
/// import path has it, sharing one file store, so a block's diagnostics
/// name the same files whichever said them.
pub struct Checker {
    vfs: Arc<Vfs>,
    full: Db,
    mini: Option<Db>,
    select: Select,
    warnings_as_errors: bool,
}

/// One try at a block: which database, which context.
struct Try {
    mini: bool,
    context: Context,
}

impl Checker {
    pub fn new(settings: Settings, select: Select) -> Checker {
        let vfs = Arc::new(Vfs::new());
        let warnings_as_errors = settings.warnings_as_errors;
        let mini = mini_base(&settings).map(|path| {
            let mut mini = settings.clone();
            mini.base_library = Some(path);
            Db::with_vfs(vfs.clone(), mini)
        });
        Checker { full: Db::with_vfs(vfs.clone(), settings), mini, vfs, select, warnings_as_errors }
    }

    /// The file store every reported range's file is in.
    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// Checks every Kanagawa block of the file at `path` — a Kanagawa source's
    /// doc comments, any other file as Markdown. `None` when it cannot be read.
    pub fn check_file(&mut self, path: &Utf8Path) -> Option<FileReport> {
        let host = self.vfs.intern(path);
        let text = self.vfs.read(host)?;
        let (blocks, untagged) = if path.extension() == Some("k") {
            (doc_comment_blocks(&self.full.parse(host).syntax_node()), 0)
        } else {
            let untagged = if self.select == Select::Kanagawa { untagged_blocks(&text.text) } else { 0 };
            (markdown_blocks(&text.text, self.select), untagged)
        };
        let dir = path.parent().map(Utf8Path::to_owned).unwrap_or_default();
        let root = self.vfs.intern(&dir.join(format!("{}.block.k", path.file_name().unwrap_or("doc"))));
        let mut modules: Vec<(FileId, Block)> = Vec::new();
        let mut checked = Vec::with_capacity(blocks.len());
        for block in blocks {
            let line = text.line_index.line_col(block.fence.start()).line + 1;
            let fail = |range: TextRange, message: String| Checked {
                line,
                verdict: Verdict::Failed,
                diagnostics: vec![Diagnostic::error(FileRange::new(host, range), message)],
            };
            // `chkdoc` refuses these before it reads the directive.
            if block.text.is_empty() {
                checked.push(fail(block.fence, "this code block is empty".to_owned()));
                continue;
            }
            if !block.closed {
                checked.push(fail(block.fence, "this code block is never closed".to_owned()));
                continue;
            }
            // Written before the block is tried, whatever its directive
            // (`chkdoc.cpp:345-347`) — over a file of that name beside the
            // document, as `chkdoc` writes over one in its directory, but
            // never over the file being checked.
            if let Some(module) = block.module_path() {
                let file = self.vfs.intern(&dir.join(module));
                if file != host {
                    self.write(file, block.text.clone());
                    modules.retain(|(f, _)| *f != file);
                    modules.push((file, block.clone()));
                }
            }
            let tries: Vec<Try> = match &block.directive {
                Directive::Invalid => {
                    checked.push(Checked { line, verdict: Verdict::Skipped, diagnostics: Vec::new() });
                    continue;
                }
                Directive::Context(name) => match self.context(&dir, name) {
                    Ok(context) => vec![Try { mini: false, context }],
                    Err(message) => {
                        checked.push(fail(block.directive_range.unwrap_or(block.fence), message));
                        continue;
                    }
                },
                Directive::None => [false, true]
                    .into_iter()
                    .filter(|&mini| !mini || self.mini.is_some())
                    .flat_map(|mini| [Try { mini, context: Context::file() }, Try { mini, context: Context::body() }])
                    .collect(),
            };
            checked.push(self.try_block(host, root, line, &block, &modules, tries));
        }
        for file in modules.iter().map(|(f, _)| *f).chain([root]) {
            self.forget(file);
        }
        self.full.forget_roots(&[root]);
        if let Some(mini) = &self.mini {
            mini.forget_roots(&[root]);
        }
        Some(FileReport { file: host, blocks: checked, untagged })
    }

    /// Tries `block` in each of `tries` until one compiles; failing all, the
    /// one that got furthest — whose first error is latest in the block —
    /// says why, the first of equals.
    fn try_block(&mut self, host: FileId, root: FileId, line: u32, block: &Block, modules: &[(FileId, Block)], tries: Vec<Try>) -> Checked {
        let mut best: Option<(u32, Try, Vec<Diagnostic>)> = None;
        for attempt in tries {
            let text = format!("{}{}{}", attempt.context.before, block.text, attempt.context.after);
            self.write(root, text);
            let db = if attempt.mini { self.mini.as_ref().unwrap_or(&self.full) } else { &self.full };
            let diagnostics = db.check(&[root]);
            let failing = |d: &Diagnostic| d.is_error() || (self.warnings_as_errors && d.warning.is_some());
            if !diagnostics.iter().any(failing) {
                return Checked {
                    line,
                    verdict: Verdict::Passed { context: attempt.context.name, mini_base: attempt.mini },
                    diagnostics: Vec::new(),
                };
            }
            let before = attempt.context.before.len() as u32;
            let reached = diagnostics
                .iter()
                .filter(|d| failing(d) && d.range.file == root)
                .map(|d| u32::from(d.range.range.start()).saturating_sub(before).min(block.text.len() as u32))
                .min()
                .unwrap_or(block.text.len() as u32);
            if best.as_ref().is_none_or(|(furthest, ..)| reached > *furthest) {
                best = Some((reached, attempt, diagnostics));
            }
        }
        let Some((_, attempt, diagnostics)) = best else {
            return Checked { line, verdict: Verdict::Failed, diagnostics: Vec::new() };
        };
        let before = attempt.context.before.len() as u32;
        let how = how(&attempt.context.name, attempt.mini);
        let place = |range: FileRange| -> FileRange {
            let (offset, block) = if range.file == root {
                (before, block)
            } else if let Some((_, module)) = modules.iter().find(|(f, _)| *f == range.file) {
                (0, module)
            } else {
                return range;
            };
            let start = u32::from(range.range.start());
            if start < offset {
                return FileRange::new(host, block.fence);
            }
            let from = block.host(TextSize::from(start - offset));
            let to = block.host(TextSize::from(u32::from(range.range.end()).saturating_sub(offset))).max(from);
            FileRange::new(host, TextRange::new(from, to))
        };
        let mut diagnostics: Vec<Diagnostic> = diagnostics
            .into_iter()
            .map(|mut d| {
                d.range = place(d.range);
                for label in &mut d.labels {
                    label.range = place(label.range);
                }
                // An edit is to the checked text, which nobody holds; the
                // fix's message still says what to do.
                for fix in &mut d.fixes {
                    fix.edits.clear();
                }
                d
            })
            .collect();
        // Once per block: text that is not Kanagawa at all recovers into
        // hundreds of errors (`BUILDING.md`'s shell, 2026-11-01).
        if let Some(first) = diagnostics.first_mut() {
            first.notes.push(format!("the code block at line {line}, checked {how}"));
            if block.info.is_empty() {
                first.notes.push("`--all-blocks` reads an untagged block as Kanagawa: tag it with its language, or put `<!-- invalid -->` on the line before it".to_owned());
            }
        }
        Checked { line, verdict: Verdict::Failed, diagnostics }
    }

    /// The context `<!-- name -->` names: `chkdoc`'s own two by their names,
    /// any other the file beside the document.
    fn context(&self, dir: &Utf8Path, name: &str) -> Result<Context, String> {
        match name {
            "emptyfile.k" => return Ok(Context::file()),
            "emptyfunction.k" => return Ok(Context::body()),
            _ => {}
        }
        let Some(text) = self.vfs.read_path(&dir.join(name)) else {
            return Err(format!("cannot read `{name}`, the context this code block names"));
        };
        Context::parse(name, &text.text)
            .map_err(|n| format!("the context `{name}` has {n} `***` lines; it needs exactly one, where the block goes"))
    }

    /// Sets `file`'s text in the store, and tells both databases.
    fn write(&mut self, file: FileId, text: String) {
        self.full.set_overlay(file, text);
        if let Some(mini) = &mut self.mini {
            mini.file_changed(file, true);
        }
    }

    /// Drops what [`Checker::write`] set.
    fn forget(&mut self, file: FileId) {
        self.full.remove_overlay(file);
        if let Some(mini) = &mut self.mini {
            mini.file_changed(file, self.vfs.read(file).is_some());
        }
    }
}

/// `--base=<library>/mini-base.k` (`chkdoc.cpp:208`): the first import
/// directory holding one, where a base library is imported at all and is
/// not already it.
fn mini_base(settings: &Settings) -> Option<Utf8PathBuf> {
    if !settings.implicit_base || settings.import_dirs.is_empty() {
        return None;
    }
    if settings.base_library.as_ref().is_some_and(|b| b.file_name() == Some("mini-base.k")) {
        return None;
    }
    settings.import_dirs.iter().map(|d| d.join("mini-base.k")).find(|p| p.is_file())
}

#[cfg(test)]
mod tests;
