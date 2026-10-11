//! Where a documentation comment may appear once `parseDocs` is set.
//!
//! With the flag clear every comment is whitespace and this module is never
//! consulted. With it set, the reference's `whitespace` stops skipping
//! doc-marked comments (`Parser/Lexer.hs:96-114`) and they are consumed only
//! where the grammar runs `docCommentPre` / `docCommentPost` — inside
//! `notedWithDocs` (`Parser.hs:92-97`), which wraps exactly the elements of the
//! doc-carrying list combinators: the local `many` / `sepBy` / `sepEndBy`
//! (`Parser.hs:114-117`), `symbolWithDocs` (`:161`), the module specifier and
//! the exposed names (`:187-192`), the template `paramList` (`:361-363`) and
//! the enum constants (`:285-292`). Anywhere else a doc comment is a stray `/`
//! to the parser and the file is rejected.
//!
//! This port keeps doc comments as trivia — the tree shape, the formatter and
//! the losslessness gate are untouched by the flag — and *validates* their
//! placement after parsing instead: [`errors`] reproduces the reference's
//! accept/reject decision from the finished tree. That works because with the
//! flag set the reference's parser sees the same non-comment token stream ours
//! does; the only new failures it can produce are "a doc comment sat where no
//! `docComment` parser runs", and those are decidable from the tree.
//!
//! # The model
//!
//! Every gap between two neighbouring non-trivia tokens (or file edge) is
//! classified by two questions, answered from the tree:
//!
//! * does the token on the left **end** a doc-carrying element? Then
//!   `docCommentPost` runs here and one `<`-marked run is consumed.
//! * does the token on the right **begin** one? Then `docCommentPre` runs here
//!   (after the post run, if both) and one `|`-marked run is consumed.
//!
//! A *run* is what one `docComment'` call swallows (`Parser/Lexer.hs:81-88`):
//! it starts at a doc-marked comment and then `many (some lineComment <|>
//! blockComment)` takes every comment — doc-marked or not — that chains onto
//! it: a line comment consumes its own newline plus horizontal space, so the
//! next comment must start right there on the next line; a block comment
//! consumes only trailing horizontal space, so the next comment must follow on
//! the same line. A blank line, or a newline after a block comment, ends the
//! run. Comments outside any run are ordinary whitespace when they carry no
//! marker, and errors when they do.
//!
//! # Known deliberate inexactness, all in the accepting direction
//!
//! Where this model and the reference disagree, this model accepts — per the
//! project rule that an invented error is worse than a missed one:
//!
//! * a doc **line** comment terminated by end-of-file instead of a newline
//!   fails `manyTill anySingle (char '\n')` in the reference even at a legal
//!   position; here it is accepted.
//! * the single memory-attributed array dimension (`Parser.hs:529`) is not
//!   `notedWithDocs`, unlike the plain dimensions of `type''` (`:535`); the
//!   tree does not distinguish the two spellings of `ARRAY_TYPE`, so both
//!   accept docs.
//! * around a stray `;`, wherever this parser accepts one the reference does
//!   not: the semicolon itself is the pre-existing divergence, and doc runs
//!   next to it follow the `hidden (skipMany semi)` extents (`Parser.hs:251`) —
//!   so `struct Foo{} /*< d */;` is accepted here, where the comment stops the
//!   reference's `skipMany semi` from eating the `;` it then chokes on.
//! * a `/*<` after a **bare-path** template argument is accepted: whether the
//!   reference accepts it turns on whether the symbol table knows the name as
//!   a template (`templateSpecifier`, `Parser.hs:702-708`) — a template name
//!   completes as a specifier and takes the doc, anything else falls to
//!   `expressionTemplate` and divides. This parser has no symbol table.

use kg_base::{FxHashMap, TextRange, TextSize};
use kg_parser::SyntaxKind::{self, *};
use rowan::NodeOrToken;

use crate::parse::SyntaxError;
use crate::{SyntaxNode, SyntaxToken};

/// Every misplaced-documentation error in the file, in source order.
///
/// Empty exactly when the reference, run with `parseDocs = True`, would accept
/// the file's doc comments. The caller is [`crate::parse_file_with`]; with
/// `parse_docs` clear this is never invoked.
pub(crate) fn errors(root: &SyntaxNode) -> Vec<SyntaxError> {
    let bounds = Boundaries::collect(root);
    let mut out = Vec::new();

    // One linear walk over the leaves. `trivia` accumulates the comment and
    // whitespace tokens of the current gap; a non-trivia token closes the gap.
    let mut trivia: Vec<SyntaxToken> = Vec::new();
    let mut left: Option<TextSize> = None;
    for element in root.descendants_with_tokens() {
        let NodeOrToken::Token(token) = element else { continue };
        if token.kind().is_trivia() {
            trivia.push(token);
            continue;
        }
        let right = token.text_range().start();
        check_gap(&trivia, left, Some(right), &bounds, &mut out);
        trivia.clear();
        left = Some(token.text_range().end());
    }
    check_gap(&trivia, left, None, &bounds, &mut out);
    out
}

/// The doc-position offsets, read off the tree once, each with the element
/// it belongs to.
struct Boundaries {
    /// Offsets at which a doc-carrying element begins: `docCommentPre` runs
    /// immediately before each. Where two begin at one offset the outer one
    /// is kept: its `notedWithDocs` runs first and consumes the run.
    pre: FxHashMap<TextSize, TextRange>,
    /// Offsets at which one ends — `docCommentPost` runs immediately after —
    /// and whether a `/*<` **block** run is reachable there.
    ///
    /// It is not always: when the element ends in an *expression*, the
    /// expression parser gets the `/` first. `operator "/"` is `string "/" <*
    /// notFollowedBy (oneOf "/=")` (`Parser/Lexer.hs:261-262`), so `//<` is
    /// saved by its own second slash while `/*<` is consumed as a division
    /// whose right operand then fails — `1 /*< d */` is a parse error in the
    /// reference and `1 //< d` is not. Confirmed against `kg-oracle
    /// --parse-docs`, which rejects the block spelling with
    /// `unexpected "*< d */"` at the `*`. Where two end at one offset the
    /// inner one is kept: it finishes parsing first and takes the run.
    post: FxHashMap<TextSize, (BlockPost, TextRange)>,
    /// The module header's element, `module a.b`, where there is one.
    module: Option<TextRange>,
}

/// Whether a `/*< … */` run may open at a post boundary; `//<` runs may at any.
#[derive(Copy, Clone, PartialEq, Eq)]
enum BlockPost {
    Allowed,
    /// The element ends in an expression, whose parser eats the `/` first.
    LineOnly,
}

/// Which declaration list encloses a statement block, because the reference's
/// three lists treat a bare `;` differently; see [`mark_list`].
#[derive(Copy, Clone, PartialEq, Eq)]
enum Ctx {
    /// Top level: `decl` (`Parser.hs:245-251`).
    File,
    /// Class body: `memberDecl` (`:341-350`).
    Member,
    /// Function or lambda body: `statementOrDecl` (`:471-479`), where a bare
    /// `;` is itself a statement (`:482`) and therefore an element.
    Body,
}

impl Boundaries {
    fn collect(root: &SyntaxNode) -> Boundaries {
        let mut b = Boundaries { pre: FxHashMap::default(), post: FxHashMap::default(), module: None };
        b.node(root, Ctx::File);
        b
    }

    fn node(&mut self, node: &SyntaxNode, ctx: Ctx) {
        match node.kind() {
            // `declarations = parseImports >> many decl` (`Parser.hs:243`).
            // Imports are parsed by `skipMany`, not `notedWithDocs`, so they
            // are not elements; neither is the module header, which carries
            // its own docs below.
            SOURCE_FILE => self.mark_list(
                node.children().filter(|n| !matches!(n.kind(), MODULE_DECL | IMPORT_DECL)),
                Ctx::File,
            ),
            // `notedWithDocs (ModuleIdentifier <$ keyword "module" <*> moduleName)`
            // (`Parser.hs:191`): the element is `module a.b` alone — pre docs
            // before the keyword, post docs between the name and the `{`.
            MODULE_DECL => {
                let first = first_meaningful_token(node);
                let last = node.children().find(|n| n.kind() == MODULE_PATH).and_then(|p| last_meaningful_token(&p));
                if let (Some(first), Some(last)) = (first, last) {
                    let element = TextRange::new(first.text_range().start(), last.text_range().end());
                    self.pre_at(element.start(), element);
                    self.post_at(element.end(), BlockPost::Allowed, element);
                    self.module = Some(element);
                }
                if let Some(list) = node.children().find(|n| n.kind() == MODULE_EXPORT_LIST) {
                    self.exposed_segments(&list);
                }
            }
            CLASS_DECL => {
                if let Some(body) = node.children().find(|n| n.kind() == DECL_LIST) {
                    self.mark_list(body.children(), Ctx::Member);
                }
            }
            // `enumConsts'` wraps each constant in `symbolWithDocs`
            // (`Parser.hs:285-292`); the separating commas stay outside. A
            // constant with an `= expression` initializer ends in expression
            // position, where only a `//<` run survives the division ambiguity.
            ENUM_DECL => {
                if let Some(body) = node.children().find(|n| n.kind() == DECL_LIST) {
                    for variant in body.children().filter(|n| n.kind() == ENUM_VARIANT) {
                        self.mark_element_with(&variant, block_post_after_eq(&variant));
                    }
                }
            }
            // `memberVariable sepEndBy semi` (`Parser.hs:299,305`): the `;` is
            // a separator, so post docs come *before* it, never after.
            STRUCT_DECL | UNION_DECL => {
                if let Some(body) = node.children().find(|n| n.kind() == DECL_LIST) {
                    for field in body.children().filter(|n| n.kind() == VAR_DECL) {
                        self.mark_element(&field);
                    }
                }
            }
            // Function and lambda bodies, nested `{ }` scopes and `static if`
            // branches are all `braces (many p)` in the reference
            // (`Parser.hs:424,444,467,469`), so every child is an element.
            BLOCK_STMT => self.mark_list(node.children(), ctx),
            // `braces (many block)` (`Parser.hs:599`): the cases themselves.
            SWITCH_STMT => {
                let mut in_body = false;
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) if t.kind() == L_CURLY => in_body = true,
                        NodeOrToken::Node(n) if in_body => {
                            self.mark_element(&n);
                        }
                        _ => {}
                    }
                }
            }
            // `many statementOrDecl <* keyword "break" <* semi`
            // (`Parser.hs:602-603`): the body statements are elements, the
            // mandatory `break;` is part of the case, not of its list.
            SWITCH_CASE | SWITCH_DEFAULT => {
                let mut in_body = false;
                let mut body = Vec::new();
                for child in node.children_with_tokens() {
                    match child {
                        NodeOrToken::Token(t) if t.kind() == COLON => in_body = true,
                        NodeOrToken::Node(n) if in_body && n.kind() != BREAK_STMT => body.push(n),
                        _ => {}
                    }
                }
                self.mark_list(body.into_iter(), Ctx::Body);
            }
            // `paramList` wraps each parameter in `notedWithDocs`
            // (`Parser.hs:361-363`). The inner `paramKind MP.sepBy comma` of a
            // template-template signature (`:370`) does not, and those nodes
            // are not direct children here, so they are correctly excluded.
            TEMPLATE_PARAM_LIST => {
                for param in node.children().filter(|n| matches!(n.kind(), TYPE_PARAM | VALUE_PARAM))
                {
                    // A `typename` parameter's default is a *type* and safe; a
                    // non-type parameter's default is `expressionTemplate`
                    // (`Parser.hs:367`), expression position.
                    let block = if param.kind() == VALUE_PARAM {
                        block_post_after_eq(&param)
                    } else {
                        BlockPost::Allowed
                    };
                    self.mark_element_with(&param, block);
                }
            }
            // `funcParams = parens (symbol param `sepBy` comma)` (`Parser.hs:377`)
            // and the function-type list (`:551`) both use the doc `sepBy`.
            PARAM_LIST => {
                for param in node.children().filter(|n| n.kind() == PARAM) {
                    self.mark_element(&param);
                }
            }
            FN_TYPE_PARAM_LIST => {
                for param in node.children().filter(|n| n.kind() == FN_TYPE_PARAM) {
                    self.mark_element(&param);
                }
            }
            // `functionCall`'s `expression `sepBy` comma` (`Parser.hs:871`).
            // `mux` and `concat` use the doc-free `sepBy1` (`:867,870`) and
            // build their own nodes, not an `ARG_LIST`, so they stay strict.
            // Every call argument ends in expression position: line runs only.
            ARG_LIST => {
                for arg in node.children() {
                    self.mark_element_with(&arg, BlockPost::LineOnly);
                }
            }
            // `templateArgs` (`Parser.hs:697`) reads each argument as
            // `try templateSpecifier <|> expressionTemplate` — and
            // `templateSpecifier` (`:702-708`) is a symbol-table lookup that
            // succeeds only for a bare name the table knows as a *template*.
            // Everything else — type keywords included; `kg-oracle` rejects
            // `C<uint32 /*< d */>` — is parsed by `expressionTemplate` and
            // ends in expression position. A bare path is the one shape that
            // may be the template-template case, undecidable without the
            // symbol table, so it keeps the accepting answer.
            TEMPLATE_ARG_LIST => {
                for arg in node.children() {
                    let block = if arg.kind() == PATH {
                        BlockPost::Allowed
                    } else {
                        BlockPost::LineOnly
                    };
                    self.mark_element_with(&arg, block);
                }
            }
            // Only the designated form is doc-carrying: `designator `sepBy`
            // comma` (`Parser.hs:862`) against `positionalList`'s bare `noted`
            // (`:856-861`). A designator always ends in its `= expression`.
            INIT_LIST_EXPR => {
                for designator in node.children().filter(|n| n.kind() == DESIGNATOR) {
                    self.mark_element_with(&designator, BlockPost::LineOnly);
                }
            }
            // `brackets (noted capture `sepBy` comma)` (`Parser.hs:441`).
            CAPTURE_LIST => {
                for capture in node.children().filter(|n| n.kind() == CAPTURE) {
                    self.mark_element(&capture);
                }
            }
            // `many (brackets expression)` (`Parser.hs:535`): each `[dim]`
            // group is an element of the type.
            ARRAY_TYPE => {
                let element = node.text_range();
                for child in node.children_with_tokens() {
                    if let NodeOrToken::Token(t) = child {
                        match t.kind() {
                            L_BRACK => self.pre_at(t.text_range().start(), element),
                            R_BRACK => self.post_at(t.text_range().end(), BlockPost::Allowed, element),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }

        for child in node.children() {
            self.node(&child, child_ctx(node.kind(), ctx));
        }
    }

    /// Marks the elements of one `many`-shaped declaration list.
    ///
    /// A bare `;` needs care. In `Body` position it is itself a statement
    /// (`Parser.hs:482`) and therefore a full element. In `File` and `Member`
    /// position this parser accepts one where the reference's `hidden (skipMany
    /// semi)` (`:251`) ate it as part of the *preceding* declaration — so it
    /// extends that declaration's post-doc boundary and is otherwise
    /// transparent: not an element, and docs may not precede it.
    fn mark_list(&mut self, children: impl Iterator<Item = SyntaxNode>, ctx: Ctx) {
        let mut prev: Option<TextRange> = None;
        for child in children {
            if child.kind() == ERROR {
                prev = None;
                continue;
            }
            if child.kind() == EMPTY_STMT && ctx != Ctx::Body {
                if let Some(element) = prev
                    && let Some(last) = last_meaningful_token(&child)
                {
                    self.post_at(last.text_range().end(), BlockPost::Allowed, element);
                }
                continue;
            }
            prev = self.mark_element(&child);
        }
    }

    /// The exposed names of a module: `(moduleDiff <|> moduleIdentifier <|>
    /// notedWithDocs identifier) MP.sepBy comma` (`Parser.hs:187`), all three
    /// alternatives doc-carrying. `MODULE_DIFF` has no node kind — a diff is
    /// written flat — so an element is simply everything between two commas.
    fn exposed_segments(&mut self, list: &SyntaxNode) {
        let mut segment: Option<(TextSize, TextSize)> = None;
        for child in list.children_with_tokens() {
            let (kind, range) = (child.kind(), child.text_range());
            match kind {
                L_CURLY => {}
                k if k.is_trivia() => {}
                COMMA | R_CURLY => {
                    if let Some((start, end)) = segment.take() {
                        let element = TextRange::new(start, end);
                        self.pre_at(start, element);
                        self.post_at(end, BlockPost::Allowed, element);
                    }
                }
                _ => {
                    segment = match segment {
                        None => Some((range.start(), range.end())),
                        Some((start, _)) => Some((start, range.end())),
                    };
                }
            }
        }
    }

    fn mark_element(&mut self, node: &SyntaxNode) -> Option<TextRange> {
        self.mark_element_with(node, BlockPost::Allowed)
    }

    /// Marks `node` as an element, and returns its extent: first to last
    /// meaningful token, which is what the two runs sit outside of.
    fn mark_element_with(&mut self, node: &SyntaxNode, block: BlockPost) -> Option<TextRange> {
        let first = first_meaningful_token(node)?;
        let last = last_meaningful_token(node)?;
        let element = TextRange::new(first.text_range().start(), last.text_range().end());
        self.pre_at(element.start(), element);
        self.post_at(element.end(), block, element);
        Some(element)
    }

    fn pre_at(&mut self, offset: TextSize, element: TextRange) {
        self.pre.entry(offset).or_insert(element);
    }

    fn post_at(&mut self, offset: TextSize, block: BlockPost, element: TextRange) {
        self.post.insert(offset, (block, element));
    }
}

/// `LineOnly` when the element carries an `= …` default or initializer, whose
/// expression is the last thing parsed before any post doc.
fn block_post_after_eq(node: &SyntaxNode) -> BlockPost {
    let has_eq = node
        .children_with_tokens()
        .any(|c| matches!(c, NodeOrToken::Token(ref t) if t.kind() == EQ));
    if has_eq { BlockPost::LineOnly } else { BlockPost::Allowed }
}

/// The declaration-list flavour that `parent`'s children live in.
fn child_ctx(parent: SyntaxKind, ctx: Ctx) -> Ctx {
    match parent {
        SOURCE_FILE => Ctx::File,
        CLASS_DECL => Ctx::Member,
        FN_DECL | LAMBDA_EXPR => Ctx::Body,
        _ => ctx,
    }
}

/// A node's first non-trivia token. Trivia may be attached inside the node, so
/// `first_token` alone is not enough.
fn first_meaningful_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    let end = node.text_range().end();
    let mut token = node.first_token()?;
    while token.kind().is_trivia() {
        token = token.next_token()?;
        if token.text_range().start() >= end {
            return None;
        }
    }
    Some(token)
}

fn last_meaningful_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    let start = node.text_range().start();
    let mut token = node.last_token()?;
    while token.kind().is_trivia() {
        token = token.prev_token()?;
        if token.text_range().end() <= start {
            return None;
        }
    }
    Some(token)
}

// ---------------------------------------------------------------------------
// gap checking
// ---------------------------------------------------------------------------

/// Validates one gap's trivia against the up-to-two runs the reference's
/// grammar consumes there: `notedWithDocs` sequences `docCommentPost` (for the
/// element ending at `left`) before `docCommentPre` (for the one starting at
/// `right`), so a legal gap is `[<-run] [|-run]` and nothing more.
fn check_gap(
    trivia: &[SyntaxToken],
    left: Option<TextSize>,
    right: Option<TextSize>,
    bounds: &Boundaries,
    out: &mut Vec<SyntaxError>,
) {
    let post_legal = left.and_then(|off| bounds.post.get(&off).map(|&(block, _)| block));
    let pre_legal = right.is_some_and(|off| bounds.pre.contains_key(&off));

    let mut i = 0;
    let mut seen_pre = false;
    let mut seen_post = false;
    while i < trivia.len() {
        let token = &trivia[i];
        if !token.kind().is_doc_comment() {
            i += 1;
            continue;
        }
        let trailing = token.kind().is_trailing_doc();
        let error = |msg: &str| SyntaxError { message: msg.to_owned(), range: token.text_range() };
        if seen_pre {
            // After a consumed `|` run the element itself must begin; the
            // reference's declaration parser fails on the `/`.
            out.push(error(
                "expected a declaration after this documentation comment's run — \
                 only one `//|` run may precede a declaration",
            ));
            return;
        }
        if trailing {
            if seen_post || post_legal.is_none() {
                out.push(error(
                    "misplaced documentation comment: `//<` and `/*<` document \
                     the declaration they follow, and none precedes this one",
                ));
                return;
            }
            if post_legal == Some(BlockPost::LineOnly) && token.kind() == DOC_BLOCK_POST {
                // The reference reads the `/` of `/*<` after an expression as
                // a division operator (`Parser/Lexer.hs:261-262`) and then
                // fails on the `*`; only `//<` survives, saved by its own
                // second `/` tripping `notFollowedBy (oneOf "/=")`.
                out.push(error(
                    "a `/*< … */` documentation comment cannot follow an expression — \
                     the reference reads its `/` as a division operator; use `//<`",
                ));
                return;
            }
            seen_post = true;
        } else {
            if !pre_legal {
                out.push(error(
                    "misplaced documentation comment: `//|` and `/*|` document \
                     the declaration they precede, and none follows this one",
                ));
                return;
            }
            seen_pre = true;
        }
        i = skip_run(trivia, i);
    }
}

/// The extent of the run starting at doc comment `i`: the index just past the
/// last comment it swallows. Reproduces `many ((LineComment <$> some
/// lineComment) <|> (BlockComment . lines <$> blockComment))`
/// (`Parser/Lexer.hs:85-88`) — after the marker test, *every* comment that
/// chains is consumed, doc-marked or not.
fn skip_run(trivia: &[SyntaxToken], i: usize) -> usize {
    let mut at = i;
    loop {
        // The next comment token, and the whitespace between, if any.
        let mut next = at + 1;
        let mut gap = "";
        if next < trivia.len() && trivia[next].kind() == WHITESPACE {
            gap = trivia[next].text();
            next += 1;
        }
        if next >= trivia.len() || trivia[next].kind() == WHITESPACE {
            return at + 1;
        }
        let chains = if line_comment(trivia[at].kind()) {
            // `manyTill anySingle (char '\n') <* hspace` (`Parser/Lexer.hs:87`):
            // the comment ate its own newline, so the next may start on the
            // next line, indented — but a blank line ends the run.
            let mut chars = gap.chars();
            chars.next() == Some('\n') && chars.all(is_hspace)
        } else {
            // `manyTill anySingle (string "*/") <* hspace` (`:88`): only
            // horizontal space, so the next comment must share the line.
            gap.chars().all(is_hspace)
        };
        if !chains {
            return at + 1;
        }
        at = next;
    }
}

fn line_comment(kind: SyntaxKind) -> bool {
    matches!(kind, LINE_COMMENT | DOC_LINE_PRE | DOC_LINE_POST)
}

/// megaparsec's `hspace` character class: `isSpace` minus the two line
/// terminators. Matches the lexer's `is_hspace_hs`, reproduced here because
/// that one is private to the lexer.
fn is_hspace(c: char) -> bool {
    let uc = c as u32;
    let is_space =
        if uc <= 0x377 { uc == 0x20 || uc.wrapping_sub(0x9) <= 4 || uc == 0xa0 } else { c.is_whitespace() };
    is_space && c != '\n' && c != '\r'
}

// ---------------------------------------------------------------------------
// reading the documentation
// ---------------------------------------------------------------------------

/// The documentation of one doc-carrying element, as the reference's
/// documentation generator reads it (`third_party/kanagawa/doc/sandcastle.md`): the
/// `//|` or `/*|` run before the element and the `//<` or `/*<` run after,
/// each with its markers and, on a line comment, the one space of margin
/// taken off. The text is Markdown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Doc {
    pub before: Option<String>,
    pub after: Option<String>,
}

impl Doc {
    /// Both runs as one text, the one before first.
    pub fn text(&self) -> String {
        match (&self.before, &self.after) {
            (Some(b), Some(a)) => format!("{b}\n\n{a}"),
            (Some(t), None) | (None, Some(t)) => t.clone(),
            (None, None) => String::new(),
        }
    }
}

/// Every doc-carrying element of a file, and the documentation of those that
/// have any.
#[derive(Clone, Debug, Default)]
pub struct FileDocs {
    /// Every element's extent, first to last meaningful token.
    elements: Vec<TextRange>,
    docs: FxHashMap<TextRange, Doc>,
    /// The module header's element.
    module: Option<TextRange>,
}

impl FileDocs {
    /// The documentation of the innermost doc-carrying element around
    /// `range` — for a declaration's name, the declaration's own; `None`
    /// where that element has none (an enclosing element's documentation is
    /// not its).
    pub fn around(&self, range: TextRange) -> Option<&Doc> {
        let element = self.elements.iter().filter(|e| e.contains_range(range)).min_by_key(|e| e.len())?;
        self.docs.get(element)
    }

    /// The module's own documentation: its header's, `/*| … */ module a.b`.
    pub fn module(&self) -> Option<&Doc> {
        self.docs.get(&self.module?)
    }

    /// The module header's extent, `module a.b`.
    pub fn module_header(&self) -> Option<TextRange> {
        self.module
    }

    /// The documentation of the element exactly `element`.
    pub fn of(&self, element: TextRange) -> Option<&Doc> {
        self.docs.get(&element)
    }

    /// The documented elements, in source order.
    pub fn documented(&self) -> Vec<(TextRange, &Doc)> {
        let mut out: Vec<_> = self.docs.iter().map(|(r, d)| (*r, d)).collect();
        out.sort_by_key(|(r, _)| (r.start(), std::cmp::Reverse(r.end())));
        out
    }
}

/// The documentation in `root`, wherever the reference's grammar would read
/// it with `--parse-docs` — whether or not this parse had the flag, since the
/// comments are trivia either way. A run where none may stand is ignored
/// here; [`errors`] reports it when the flag is set.
pub fn file_docs(root: &SyntaxNode) -> FileDocs {
    let bounds = Boundaries::collect(root);
    let mut docs: FxHashMap<TextRange, Doc> = FxHashMap::default();
    let mut gap = |trivia: &[SyntaxToken], left: Option<TextSize>, right: Option<TextSize>| {
        let post = left.and_then(|off| bounds.post.get(&off).copied());
        let pre = right.and_then(|off| bounds.pre.get(&off).copied());
        let (mut seen_pre, mut seen_post) = (false, false);
        let mut i = 0;
        while i < trivia.len() {
            let token = &trivia[i];
            if !token.kind().is_doc_comment() {
                i += 1;
                continue;
            }
            let end = skip_run(trivia, i);
            let text = run_text(&trivia[i..end]);
            if token.kind().is_trailing_doc() {
                let Some((block, element)) = post.filter(|_| !seen_post && !seen_pre) else { return };
                if block == BlockPost::LineOnly && token.kind() == DOC_BLOCK_POST {
                    return;
                }
                docs.entry(element).or_default().after = Some(text);
                seen_post = true;
            } else {
                let Some(element) = pre.filter(|_| !seen_pre) else { return };
                docs.entry(element).or_default().before = Some(text);
                seen_pre = true;
            }
            i = end;
        }
    };
    let mut trivia: Vec<SyntaxToken> = Vec::new();
    let mut left: Option<TextSize> = None;
    for element in root.descendants_with_tokens() {
        let NodeOrToken::Token(token) = element else { continue };
        if token.kind().is_trivia() {
            trivia.push(token);
            continue;
        }
        gap(&trivia, left, Some(token.text_range().start()));
        trivia.clear();
        left = Some(token.text_range().end());
    }
    gap(&trivia, left, None);

    let mut elements: Vec<TextRange> =
        bounds.pre.values().copied().chain(bounds.post.values().map(|&(_, e)| e)).collect();
    elements.sort_by_key(|r| (r.start(), r.end()));
    elements.dedup();
    docs.retain(|_, d| d.before.as_deref().is_some_and(|t| !t.is_empty()) || d.after.as_deref().is_some_and(|t| !t.is_empty()));
    FileDocs { elements, docs, module: bounds.module }
}

/// How [`to_commonmark`] treats a heading.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Headings {
    /// As written, `{#id}` attributes included, for a renderer that reads
    /// them (`kg doc`).
    Keep,
    /// As a bold line, attributes dropped: a heading inside an editor's
    /// tooltip is louder than the declaration it documents.
    Bold,
}

/// Documentation in the reference's dialect — Pandoc's Markdown, with
/// svgbob diagrams between `@@` lines (`third_party/kanagawa/doc/sandcastle.md`) — as
/// CommonMark: a diagram becomes a ```` ```svgbob ```` block, a bracketed
/// span `[text]{#id .anchor}` an HTML `<span id="id">text</span>`, and a
/// heading what `headings` says. Code, fenced or indented, is left alone.
pub fn to_commonmark(text: &str, headings: Headings) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut fence: Option<&str> = None;
    let mut diagram = false;
    let mut previous_blank = true;
    // Inside an indented code block: opened by an indented line after a
    // blank one, kept by indented or blank lines, closed by any other.
    let mut indented = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if diagram {
            if trimmed == "@@" {
                out.push_str("```\n");
                diagram = false;
            } else {
                out.push_str(line);
                out.push('\n');
            }
            continue;
        }
        if let Some(marker) = fence {
            out.push_str(line);
            out.push('\n');
            if trimmed.starts_with(marker) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            out.push_str(line);
            out.push('\n');
            previous_blank = false;
            continue;
        }
        if trimmed == "@@" {
            out.push_str("```svgbob\n");
            diagram = true;
            continue;
        }
        let is_indented = line.starts_with("    ") || line.starts_with('\t');
        indented = if indented { is_indented || trimmed.is_empty() } else { previous_blank && is_indented };
        if indented {
            out.push_str(line);
        } else if let Some(heading) = trimmed.strip_prefix('#').filter(|_| trimmed.trim_start_matches('#').starts_with(' ')) {
            let title = heading.trim_start_matches('#').trim();
            match headings {
                Headings::Keep => out.push_str(&spans(line)),
                Headings::Bold => {
                    let title = strip_heading_attributes(title);
                    let title = title.trim_matches(|c| c == '_' || c == '*');
                    out.push_str("**");
                    out.push_str(&spans(title));
                    out.push_str("**");
                }
            }
        } else {
            out.push_str(&spans(line));
        }
        out.push('\n');
        previous_blank = trimmed.is_empty();
    }
    if diagram || fence.is_some() {
        out.push_str("```\n");
    }
    out.truncate(out.trim_end().len());
    out
}

/// A heading's text without its trailing `{#id .class}`.
fn strip_heading_attributes(title: &str) -> &str {
    match title.rfind('{') {
        Some(i) if title.ends_with('}') && title[i..].starts_with("{#") => title[..i].trim_end(),
        _ => title,
    }
}

/// Pandoc's bracketed spans in one line, `[text]{#id .class}`, as HTML
/// spans; a span without an id keeps only its text. Links, `[text](url)`,
/// are untouched.
fn spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find("]{").map(|i| open + i) else { break };
        let Some(end) = rest[close..].find('}').map(|i| close + i) else { break };
        let text = &rest[open + 1..close];
        if text.contains('[') {
            out.push_str(&rest[..open + 1]);
            rest = &rest[open + 1..];
            continue;
        }
        let attributes = &rest[close + 2..end];
        out.push_str(&rest[..open]);
        match attributes.split_whitespace().find_map(|a| a.strip_prefix('#')) {
            Some(id) => {
                out.push_str("<span id=\"");
                out.push_str(id);
                out.push_str("\">");
                out.push_str(text);
                out.push_str("</span>");
            }
            None => out.push_str(text),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// One run's text: each comment's markers off — `//|`, `//<` and a plain
/// `//` continuing the run lose one space of margin as well (`sandcastle.md`:
/// "The line comment style `//` requires a single space of margin after the
/// comment prefix") — a block comment's lines dedented, and the blank lines
/// at either end dropped.
fn run_text(run: &[SyntaxToken]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for token in run {
        let text = token.text();
        let marker = match token.kind() {
            DOC_LINE_PRE | DOC_LINE_POST | DOC_BLOCK_PRE | DOC_BLOCK_POST => 3,
            LINE_COMMENT | BLOCK_COMMENT => 2,
            _ => continue,
        };
        if line_comment(token.kind()) {
            let body = text.get(marker..).unwrap_or("").trim_end_matches(['\r', '\n']);
            lines.push(body.strip_prefix(' ').unwrap_or(body).trim_end().to_owned());
        } else {
            let body = text.get(marker..text.len().saturating_sub(2)).unwrap_or("");
            let body: Vec<&str> = body.lines().map(str::trim_end).collect();
            let indent = body
                .iter()
                .skip(1)
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start().len())
                .min()
                .unwrap_or(0);
            for (n, line) in body.iter().enumerate() {
                let line = if n == 0 { line.trim_start() } else { line.get(indent..).unwrap_or(line.trim_start()) };
                lines.push(line.to_owned());
            }
        }
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use crate::{ParseOptions, parse_file, parse_file_with};

    const DOCS: ParseOptions = ParseOptions { parse_docs: true };

    fn doc_errors(text: &str) -> usize {
        let with = parse_file_with(text, DOCS).errors().len();
        let without = parse_file(text).errors().len();
        assert!(with >= without, "the flag removed an error, which it never can");
        with - without
    }

    #[track_caller]
    fn accepts(text: &str) {
        let parse = parse_file_with(text, DOCS);
        assert_eq!(parse.errors(), &[], "expected no errors for:\n{text}");
    }

    #[track_caller]
    fn rejects(text: &str) {
        assert!(doc_errors(text) > 0, "expected a doc placement error for:\n{text}");
        assert_eq!(
            parse_file(text).errors(),
            &[],
            "this case must be an error only under the flag:\n{text}"
        );
    }

    // ------------------------------------------------------------ rejections
    //
    // Each of these parses cleanly with the flag clear — `rejects` asserts
    // that — so every one is an error that exists only under `parseDocs`,
    // matching the `expected:1` half of `test/syntax/doc-comments.k`.

    #[test]
    fn a_doc_comment_with_nothing_to_attach_to_is_an_error() {
        rejects("//|\n");
        rejects("//<\n");
        rejects("/*| */\n");
        rejects("/*< */\n");
    }

    #[test]
    fn a_trailing_doc_with_no_preceding_declaration_is_an_error() {
        rejects("//< invalid post\nstruct Foo{}\n");
    }

    #[test]
    fn a_leading_doc_with_no_following_declaration_is_an_error() {
        rejects("struct Foo{}\n//| invalid pre\n");
    }

    #[test]
    fn a_blank_line_splits_runs_and_the_second_is_an_error() {
        rejects("//| invalid\n\n//| multiple doc comments\nstruct Foo{}\n");
        rejects("//| invalid\n\n//< multiple doc comments\nstruct Foo{}\n");
        rejects("struct Foo{}\n//< invalid\n\n//< multiple doc comments\n");
    }

    #[test]
    fn a_doc_between_template_clause_and_declaration_is_an_error() {
        // `docCommentPre` runs before the `template` keyword, never between
        // the parameter list and the wrapped declaration (`Parser.hs:373-375`).
        rejects("template <typename T>\n//| invalid pre\nclass Foo\n{\n}\n");
    }

    #[test]
    fn a_doc_before_an_import_is_an_error() {
        // `parseImports` is `skipMany`, not `notedWithDocs` (`Parser.hs:194`).
        rejects("//| d\nimport foo.bar\n");
    }

    #[test]
    fn a_doc_inside_an_expression_is_an_error() {
        rejects("inline uint32 f()\n{\nreturn /*| d */ 1;\n}\n");
    }

    #[test]
    fn concat_and_mux_arguments_carry_no_docs() {
        // `expression sepBy1 comma` (`Parser.hs:867,870`), the doc-free list,
        // unlike `functionCall`'s `sepBy` (`:871`) which accepts them below.
        rejects("inline uint32 f(uint32 y)\n{\nreturn concat(/*| d */ y);\n}\n");
        rejects("inline uint32 f(uint32 y)\n{\nreturn mux(y, /*| d */ y, y);\n}\n");
    }

    #[test]
    fn positional_initializers_carry_no_docs() {
        // `positionalList` is bare `noted` (`Parser.hs:856-861`).
        rejects("const uint32[2] X = {/*| d */ 1, 2};\n");
    }

    // ------------------------------------------------------------ acceptances

    #[test]
    fn plain_comments_stay_whitespace_under_the_flag() {
        accepts("//\n");
        accepts("/* */\n");
        accepts("// plain\n/* nested /* inner */ outer */\nstruct Foo{}\n");
    }

    #[test]
    fn docs_attach_around_a_declaration() {
        accepts("//| pre\nstruct Foo{} //< post\n");
        accepts("/*|\npre\n*/\nstruct Foo{}\n/*<\npost\n*/\n");
    }

    #[test]
    fn a_run_swallows_chained_comments_regardless_of_marker() {
        // `//< line` and the block chain onto the `//|` run and are consumed
        // with it (`Parser/Lexer.hs:85-88`); doc-comments.k asserts this shape.
        accepts("//| multi\n//< line\n/*| multi-comment\ndocs\n*/\nstruct Foo{}\n");
        // A blank line ends the run; the non-doc block after it is whitespace.
        accepts("//| multi\n// line doc comment\n\n/* not a doc comment\n\n*/\nstruct Foo{}\n");
    }

    #[test]
    fn a_post_run_and_a_pre_run_share_one_gap() {
        accepts("struct A{} //< post of A\n\n//| pre of B\nstruct B{}\n");
    }

    #[test]
    fn docs_attach_inside_parameter_and_template_lists() {
        accepts("template\n< /*| pre N */ auto N //< post N\n, /*| pre T */ typename T //< post T\n>\nclass Foo\n{\n}\n");
        accepts("inline void f(/*| pre x */ uint32 x /*< post x */, /*| pre b */ bool b /*< post b */)\n{\n}\n");
        accepts("template <(uint32 /*< post fn param */) -> bool Fn /*< post Fn */>\nclass Foo\n{\n}\n");
    }

    #[test]
    fn docs_attach_to_members_fields_and_enum_constants() {
        accepts("class C\n{\npublic:\n//| pre m\ninline void m()\n{\n} //< post m\n\n//| pre state\nbool state;\n}\n");
        // `memberVariable sepEndBy semi` (`Parser.hs:299`): the `;` is a
        // separator, so a post doc binds *before* it and is an error after it —
        // `docCommentPost` runs at the end of `memberVariable`, and past the
        // `;` the next `notedWithDocs` only consumes `|`-marked runs.
        accepts("struct S\n{\n//| pre x\nuint32 x //< post x\n;\n//| pre b\nbool b;\n}\n");
        rejects("struct S\n{\nuint32 x; //< too late, the separator is behind us\n}\n");
        accepts("enum E : uint4\n{ //| pre One\nOne //< post One\n, //| pre Two\nTwo = One + 1 //< post Two\n}\n");
    }

    #[test]
    fn docs_attach_to_statements_and_call_arguments() {
        accepts("inline void f(uint32 y)\n{\n//| pre stmt\nuint32 x = y; //< post stmt\ng(/*| d */ y);\n}\n");
    }

    #[test]
    fn docs_attach_to_the_module_header_and_exposed_names() {
        // The shape of `test/syntax/helper/doc_comment_module.k`.
        accepts(
            "/*|\nModule with doc comments\n*/\nmodule helper.doc_comment_module\n\
             { //| Foo and friends\nFoo //< Foo struct\n, make_foo\n}\nstruct Foo{};\ninline Foo make_foo()\n{\nreturn {};\n}\n",
        );
    }

    #[test]
    fn docs_attach_to_designated_initializers() {
        // The post doc must be the line spelling: after `= 1` the expression
        // parser reads a `/*<`'s slash as division (`Parser/Lexer.hs:261-262`)
        // — `kg-oracle --parse-docs` rejects the block spelling at the `*`.
        accepts("const auto X = {/*| d */ .a = 1 //< d\n, .b = 2};\n");
        rejects("const auto X = {.a = 1 /*< d */, .b = 2};\n");
    }

    #[test]
    fn a_block_post_doc_after_an_expression_is_division() {
        // Same mechanism at every expression-final doc position; each line
        // spelling is accepted, each block spelling divides. Oracle-checked.
        rejects("enum E : uint4\n{ One = 1 /*< d */\n}\n");
        accepts("enum E : uint4\n{ One = 1 //< d\n}\n");
        rejects("inline uint32 g(uint32 a)\n{\nreturn a;\n}\ninline uint32 h(uint32 y)\n{\nreturn g(y /*< d */);\n}\n");
        rejects("template <auto N = 3 /*< d */>\nclass C\n{\n}\n");
        accepts("template <auto N = 3 //< d\n>\nclass C\n{\n}\n");
        // A template *argument* is `expressionTemplate` even when it spells a
        // type — `templateSpecifier` (`Parser.hs:702-708`) admits only a name
        // the symbol table knows as a template — so type keywords divide too.
        rejects("template <typename T>\nclass C\n{\n}\nconst C<uint32 /*< d */> x = 0;\n");
        // A `typename` *parameter's* default really is `typ` (`Parser.hs:366`)
        // and completes before the slash is looked at.
        accepts("template <typename T = uint32 /*< d */>\nclass C\n{\n}\n");
    }

    // ------------------------------------------------- documented divergences

    #[test]
    fn an_eof_terminated_doc_line_comment_is_accepted_here() {
        // The reference's `lineComment` requires the closing newline
        // (`Parser/Lexer.hs:87`) and so rejects this; accepting is the
        // documented fail-toward-silence divergence in the module docs.
        accepts("struct Foo{} //< post");
    }

    #[test]
    fn the_flag_clear_is_inert() {
        // Every rejection above parses cleanly without the flag — `rejects`
        // asserts that one by one. The whole-corpus equality version of this
        // check is `the_corpus_gains_no_errors_under_the_flag` below.
        assert_eq!(parse_file("//|\n").errors(), &[]);
        assert_eq!(
            parse_file_with("//|\n", ParseOptions { parse_docs: false }),
            parse_file("//|\n"),
        );
    }

    // ------------------------------------------------------------ the corpus

    /// The documentation [`super::file_docs`] attaches around the first
    /// occurrence of `name` in `text`.
    fn doc(text: &str, name: &str) -> Option<String> {
        use kg_base::{TextRange, TextSize};
        let at = text.find(name).expect("the name is in the text");
        let range = TextRange::at(TextSize::from(at as u32), TextSize::from(name.len() as u32));
        super::file_docs(&parse_file(text).syntax_node()).around(range).map(super::Doc::text)
    }

    #[test]
    fn a_declaration_reads_the_run_before_it_and_the_run_after_it() {
        let text = "//| Return xor of inputs.\ninline uint8 xor_of(uint8 a, uint8 b)\n{\n    return a ^ b;\n}\n//< And more.\n";
        assert_eq!(doc(text, "xor_of").as_deref(), Some("Return xor of inputs.\n\nAnd more."));
    }

    #[test]
    fn a_run_continues_through_plain_comments_and_keeps_markdown_indentation() {
        let text = "//| Adds two values.\n// Returns the result.\n//\n//     >>> add_two(1, 2)\ninline uint8 add_two(uint8 a, uint8 b) { return a + b; }\n";
        assert_eq!(
            doc(text, "add_two(uint8").as_deref(),
            Some("Adds two values.\nReturns the result.\n\n    >>> add_two(1, 2)")
        );
    }

    #[test]
    fn a_block_run_is_dedented_and_needs_no_margin() {
        let text = "/*|\n    Adds two values.\n      Indented.\n*/\ninline uint8 add_blk(uint8 a) { return a; }\n";
        assert_eq!(doc(text, "add_blk").as_deref(), Some("Adds two values.\n  Indented."));
    }

    #[test]
    fn parameters_fields_and_constants_carry_their_own() {
        let text = "template <typename Elem //< Element type\n>\ninline Elem twice(Elem input_value //< Input number\n) { return input_value; }\n";
        assert_eq!(doc(text, "Elem").as_deref(), Some("Element type"));
        assert_eq!(doc(text, "input_value").as_deref(), Some("Input number"));
        assert_eq!(doc(text, "twice"), None, "a parameter's documentation is not its function's");
        let text = "struct opt\n{\n    //| Pre: valid flag\n    bool is_valid /*< Post: valid flag*/;\n    uint8 value;\n}\n";
        assert_eq!(doc(text, "is_valid").as_deref(), Some("Pre: valid flag\n\nPost: valid flag"));
        assert_eq!(doc(text, "value"), None);
        let text = "enum Color : uint2\n{\n    //| The red one\n    RED,\n    GREEN\n}\n";
        assert_eq!(doc(text, "RED").as_deref(), Some("The red one"));
    }

    #[test]
    fn a_member_does_not_inherit_its_class_documentation() {
        let text = "//| A counter.\nclass counter\n{\npublic:\n    //| The count.\n    uint8 count() { return 0; }\n    void reset() {}\n}\n";
        assert_eq!(doc(text, "counter\n{").as_deref(), Some("A counter."));
        assert_eq!(doc(text, "count()").as_deref(), Some("The count."));
        assert_eq!(doc(text, "reset"), None);
    }

    #[test]
    fn plain_and_misplaced_comments_are_not_documentation() {
        assert_eq!(doc("// Just a comment.\ninline void plain() {}\n", "plain"), None);
        // A blank line ends a run; the second run documents nothing it can.
        assert_eq!(doc("//| Detached.\n\nimport foo.bar\ninline void after() {}\n", "after"), None);
    }

    #[test]
    fn the_module_header_and_its_exposed_names_carry_their_own() {
        let text = "/*|\nOperations on arrays.\n*/\nmodule data.arrays\n    { //| # Constructing arrays\n      make\n    , copy\n    }\n";
        let root = parse_file(text).syntax_node();
        let docs = super::file_docs(&root);
        let texts: Vec<String> = docs.documented().into_iter().map(|(_, d)| d.text()).collect();
        assert_eq!(texts, ["Operations on arrays.", "# Constructing arrays"]);
    }

    #[test]
    fn pandoc_markup_becomes_commonmark() {
        use super::{Headings, to_commonmark};
        let doc = "Intro with an [anchor]{#here .anchor} and a [link](#here).\n\n@@\n+--+\n|  |\n@@\n\n#### __Examples__\n\n    >>> f([x]{#not-a-span})\n    1";
        assert_eq!(
            to_commonmark(doc, Headings::Bold),
            "Intro with an <span id=\"here\">anchor</span> and a [link](#here).\n\n```svgbob\n+--+\n|  |\n```\n\n**Examples**\n\n    >>> f([x]{#not-a-span})\n    1"
        );
        assert!(to_commonmark("# Title {#t}", Headings::Keep).starts_with("# Title {#t}"));
        assert_eq!(to_commonmark("# Title {#t}", Headings::Bold), "**Title**");
        assert_eq!(to_commonmark("```\n[a]{#b}\n```", Headings::Bold), "```\n[a]{#b}\n```");
    }

    fn corpus_root() -> std::path::PathBuf {
        kg_base::reference_root()
    }

    /// The library is the documentation's real reader: every file's runs
    /// attach somewhere, and a known one reads as written.
    #[test]
    fn the_library_documentation_reads() {
        let path = corpus_root().join("library/data/array.k");
        let Ok(text) = std::fs::read_to_string(&path) else {
            kg_base::corpus_absent(&format!("no corpus at {}", path.display()));
            return;
        };
        let name = "inline auto map(";
        assert!(
            doc(&text, name).is_some_and(|d| d.starts_with("Map array of input values to result values.")),
            "{:?}",
            doc(&text, name)
        );
        let mut files = Vec::new();
        collect(&corpus_root().join("library"), &mut files);
        let documented: usize = files
            .iter()
            .filter_map(|f| std::fs::read_to_string(f).ok())
            .map(|t| super::file_docs(&parse_file(&t).syntax_node()).documented().len())
            .sum();
        assert!(documented > 500, "only {documented} documented elements in the library");
    }

    /// `test/syntax/doc-comments.k`, replayed section by section the way
    /// `syntax-test.py:60-140` (and `xtask conformance`) split it: lines
    /// accumulate *trimmed*, a line containing `expected:<n>` is a marker that
    /// closes a section. All 28 of its assertions are parse-decidable under
    /// `--parse-docs`, so this is the whole file's conformance answer measured
    /// without the harness.
    #[test]
    fn doc_comments_corpus_sections_match_the_reference() {
        let path = corpus_root().join("test/syntax/doc-comments.k");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "the reference corpus is not at {}. Set KANAGAWA_ROOT to the kanagawa checkout.",
                path.display()
            )
        });

        let mut acc = String::new();
        let mut index = 0usize;
        let mut failures = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if let Some(rest) = line.find("expected:").map(|i| &line[i + "expected:".len()..]) {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                let expected: u32 = digits.parse().unwrap_or(0);
                let source = std::mem::take(&mut acc);
                let got = u32::from(!parse_file_with(&source, DOCS).errors().is_empty());
                if got != expected.min(1) {
                    failures.push(format!("section {index} (expected:{expected}, got {got})"));
                }
                index += 1;
                continue;
            }
            acc.push_str(line);
            acc.push('\n');
        }
        assert_eq!(index, 28, "doc-comments.k grew or shrank; re-read it");
        assert_eq!(failures, Vec::<String>::new());
    }

    /// Every real program in the corpus must carry exactly the same errors
    /// with the flag set — the reference's own doc-comments ctest compiles the
    /// whole base library under `--parse-docs`, so a single invented placement
    /// error here is a lie about a correct program. This is the
    /// fail-toward-silence half, and it covers every doc comment the corpus
    /// contains (~600 runs in `library/` alone).
    ///
    /// `test/syntax/*.k` (minus `helper/`) are marker-accumulation files, not
    /// programs, and doc-comments.k among them *should* gain errors; they are
    /// covered by the section replay above instead.
    #[test]
    fn the_corpus_gains_no_errors_under_the_flag() {
        let root = corpus_root();
        assert!(root.exists(), "no corpus at {}; set KANAGAWA_ROOT", root.display());
        let mut files = Vec::new();
        for dir in ["library", "test", "doc", "tools", "examples"] {
            collect(&root.join(dir), &mut files);
        }
        files.sort();
        assert!(!files.is_empty(), "no .k files under {}", root.display());

        let mut checked = 0usize;
        let mut offenders = Vec::new();
        for path in &files {
            let rel = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/");
            if rel.starts_with("test/syntax/") && !rel.starts_with("test/syntax/helper/") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let with = parse_file_with(&text, DOCS).errors().len();
            let without = parse_file(&text).errors().len();
            checked += 1;
            if with != without {
                offenders.push(format!("{rel}: {without} errors -> {with} under --parse-docs"));
            }
        }
        assert!(checked > 300, "only {checked} corpus files checked; the walk is broken");
        assert_eq!(offenders, Vec::<String>::new());
    }

    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "k") {
                out.push(path);
            }
        }
    }
}
