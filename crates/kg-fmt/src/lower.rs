//! CST → [`Doc`].
//!
//! One function per construct, describing *where a break is allowed* and what
//! the separators look like on each side of that break; `crate::doc::render`
//! decides which breaks to take. Every rule here cites the section of
//! `docs/spec/style.md` that measured it.
//!
//! Two structural commitments run through the whole module:
//!
//! 1. **Nothing is dropped.** A lowering routine walks
//!    [`Children`]'s `elems` *and* `gaps` in lockstep; a gap it never asks
//!    about is a comment it silently deleted, which
//!    [`crate::verify`] then catches. That is why there is no "skip the
//!    punctuation" shortcut anywhere below, even where the punctuation is
//!    re-emitted verbatim.
//! 2. **A line comment ends its line.** Every comment run that finishes with
//!    `//…` is followed by [`Doc::hard_line`], because a token emitted after it
//!    on the same line would be swallowed. That single rule is what makes
//!    comment preservation a layout question rather than a semantic one.
//!
//! Where the corpus is genuinely divided (`docs/spec/style.md` §10) the choice
//! is read back out of the source — whether the author broke a list, collapsed
//! a body, padded an initializer — so "preserve" costs nothing and stays
//! idempotent by construction: the next run reads the decision it just wrote.

use kg_config::Fmt;
use kg_syntax::{SyntaxElement, SyntaxKind, SyntaxNode};

use SyntaxKind::*;

use crate::doc::Doc;
use crate::trivia::{self, Children, Comment, Gap, is_collapsed};

/// What goes between two adjacent pieces of output.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sep {
    /// Nothing at all.
    Tight,
    /// One space.
    Space,
    /// A newline, forcing every enclosing group open.
    Hard,
    /// A newline, plus a blank one when the author wrote a blank line.
    HardBlank,
    /// A newline and a blank one, whatever the author wrote.
    Blank,
    /// Exactly the spaces the author wrote, used only inside a list that is
    /// being preserved verbatim.
    Pad(usize),
}

/// How a delimited list is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// One line, separators inline. Chosen when an item carries its own
    /// multi-line block — a lambda argument — so that breaking the list would
    /// fight the block rather than help it.
    Flat,
    /// Let the width decide.
    Auto,
    /// Broken, because the author broke it or a comment lives inside it.
    Broken,
}

/// The shape of one delimited list.
#[derive(Clone, Copy, Debug)]
struct ListCfg {
    /// Separator before the opening delimiter in the flat layout.
    open_flat: Sep,
    /// Move the opening delimiter to its own line when the list breaks
    /// (`docs/spec/style.md` §3.2/§3.3: 116/121 and 113/125).
    open_breaks: bool,
    /// `{ a, b }` rather than `{a, b}` in the flat layout.
    pad_flat: bool,
    /// Keep the first item on the opener's line when broken
    /// (`docs/spec/style.md` §3.1: 201 : 5).
    hug: bool,
    /// Separators lead their line rather than trailing the previous one
    /// (`docs/spec/style.md` §3.7).
    leading: bool,
    /// Indent the whole list, opener included, relative to the enclosing line.
    /// Zero where the opener's own position already carries it — an `enum`
    /// body, whose brace the Allman header break has already placed.
    indent: bool,
    /// Never break on width alone.
    ///
    /// Template *argument* lists and lambda capture lists are not among the
    /// breakable list kinds of `docs/spec/style.md` §3 — the corpus breaks
    /// template *parameter* lists 121 times and argument lists never — and a
    /// `Foo< T\n    , U\n    >` in the middle of an expression is unreadable.
    never_break: bool,
    /// Break exactly where the author broke, and pad exactly as they padded.
    ///
    /// For the three list kinds §3 normalizes — module exports, template
    /// parameters, function parameters — one item per line is the house style
    /// and the separator moves to the head of the line. Initializer lists and
    /// `enum` bodies are different: 49 of 107 `enum` bodies are written on one
    /// line, three put several constants on a line, and the corpus's numeric
    /// tables (`library/numeric/float32/internal.k:877-888`,
    /// `library/data/datetime.k:41-45`) are column-aligned grids that §10.3
    /// says to re-emit rather than reflow.
    preserve: bool,
}

/// Where in a line a piece of output starts.
///
/// Two independent facts, because two different rules need them. `line_start`
/// suppresses a leading separator — a comment already at the head of its line
/// needs no break in front of it. `item_start` says the piece begins a whole
/// declaration or statement, which is what decides whether a delimited list may
/// move its opening delimiter down a line: doing that when nothing precedes the
/// opener would leave a blank line, and — worse — the next run would read that
/// break back as an author's line break and lay the list out differently.
#[derive(Clone, Copy, Debug)]
struct At {
    line_start: bool,
    item_start: bool,
}

impl At {
    /// The first thing in a declaration or statement.
    const ITEM: At = At { line_start: true, item_start: true };
    /// Somewhere in the middle of one.
    const MID: At = At { line_start: false, item_start: false };

    fn after(sep_broke: bool) -> At {
        At { line_start: sep_broke, item_start: false }
    }
}

pub(crate) struct Lowerer<'a> {
    cfg: &'a Fmt,
    indent: isize,
    /// Source offsets of the trailing comments lifted out of the document, for
    /// [`crate::reattach`] to put back once the layout is fixed.
    lifted: std::cell::RefCell<Vec<u32>>,
    /// Depth of enclosing bodies the author collapsed onto one line.
    ///
    /// Inside one, no list may break on width. A body is kept collapsed
    /// because the author wrote it that way (`docs/spec/style.md` §10.2), and a
    /// width-driven break somewhere inside it would spread it over several
    /// lines — after which the next run would no longer see a collapsed body
    /// and would lay it out Allman instead.
    no_break: std::cell::Cell<usize>,
}

impl std::fmt::Debug for Lowerer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lowerer").field("indent", &self.indent).finish_non_exhaustive()
    }
}

impl<'a> Lowerer<'a> {
    pub(crate) fn new(cfg: &'a Fmt) -> Self {
        Lowerer {
            cfg,
            indent: cfg.indent as isize,
            lifted: std::cell::RefCell::default(),
            no_break: std::cell::Cell::default(),
        }
    }

    /// The trailing comments this lowering held back, in no particular order.
    pub(crate) fn lifted(&self) -> Vec<u32> {
        self.lifted.borrow().clone()
    }

    /// Drops a comment from the document and remembers where it came from.
    ///
    /// Only legal when the separator that follows it ends the line: the comment
    /// is put back at the end of the line its preceding token lands on, which
    /// is the same place only if nothing else was going to follow it there.
    /// That condition is exactly what keeps a comment from crossing a token.
    fn lift(&self, c: &Comment) {
        self.lifted.borrow_mut().push(c.offset);
    }

    /// Forbids width-driven breaks until the guard is dropped.
    fn pin_flat(&self) -> FlatPin<'_> {
        self.no_break.set(self.no_break.get() + 1);
        FlatPin(&self.no_break)
    }

    /// Lowers a whole file.
    pub(crate) fn file(&self, root: &SyntaxNode) -> Doc {
        self.node(root, At::ITEM)
    }

    // ---------------------------------------------------------------- pieces

    fn sep(&self, s: Sep, blank: bool, bol: bool) -> Doc {
        match s {
            Sep::Tight => Doc::nil(),
            Sep::Space => {
                if bol {
                    Doc::nil()
                } else {
                    Doc::text(" ")
                }
            }
            Sep::Hard => {
                if bol {
                    Doc::nil()
                } else {
                    Doc::hard_line()
                }
            }
            Sep::HardBlank => {
                if bol {
                    Doc::nil()
                } else if blank {
                    Doc::blank_line()
                } else {
                    Doc::hard_line()
                }
            }
            Sep::Blank => {
                if bol {
                    Doc::nil()
                } else {
                    Doc::blank_line()
                }
            }
            Sep::Pad(n) => {
                if bol {
                    Doc::nil()
                } else {
                    Doc::text(" ".repeat(n.max(1)))
                }
            }
        }
    }

    /// The trivia of one gap, rendered with `sep` as the separator it replaces.
    ///
    /// `bol` says the output is already at the start of a line, so no leading
    /// break is needed — the state a declaration's own leading doc-comment run
    /// finds itself in, since `kg-parser` pulled it inside the declaration
    /// node and the item list already ended the previous line.
    fn gap(&self, gap: &Gap, sep: Sep, bol: bool) -> Doc {
        // `bol` means the break is already behind us — which is the state a
        // comment `kg-parser` pulled into the *following* declaration arrives
        // in. `// done` written after a statement lands inside the next
        // declaration's leading trivia (`output.rs`'s `n_attached_trivia`
        // stops only at a blank line), and lifting is what puts it back on the
        // line it was written on instead of stranding it above its neighbour.
        let comments = self.keep(gap, breaks(sep) || bol);
        if comments.is_empty() {
            return self.sep(sep, gap.blank, bol);
        }
        let allow_blank = matches!(sep, Sep::Blank | Sep::HardBlank);
        let mut parts = Vec::with_capacity(comments.len() * 2 + 1);
        let mut at_bol = bol;
        let mut prev_line = false;
        // A blank the caller asked for, held back when the first comment trails
        // the previous line: the blank belongs *after* such a comment.
        let mut pending_blank = sep == Sep::Blank;

        for (i, c) in comments.iter().copied().enumerate() {
            let trails_prev_line = i == 0 && !c.own_line && !at_bol;
            let s = if at_bol {
                Sep::Tight
            } else if trails_prev_line {
                Sep::Space
            } else if prev_line || c.own_line {
                if pending_blank || (c.blank_before && allow_blank) {
                    Sep::Blank
                } else {
                    Sep::Hard
                }
            } else {
                Sep::Space
            };
            if !trails_prev_line {
                pending_blank = false;
            }
            parts.push(self.sep(s, true, false));
            parts.push(comment_doc(c));
            prev_line = c.is_line();
            at_bol = false;
        }

        // A line comment must end its line whatever the caller wanted, and a
        // block comment that started one keeps it; only a comment written
        // *between* two tokens on one line hands the line back.
        let ends_line = comments.last().is_some_and(|c| c.ends_line);
        let tail = if prev_line || ends_line {
            if pending_blank || (gap.blank && allow_blank) {
                Sep::Blank
            } else {
                Sep::Hard
            }
        } else if pending_blank {
            Sep::Blank
        } else {
            match sep {
                // A block comment written inline keeps the padding after it:
                // `/*   00   */   LOAD,` in `library/processor/risc_v/isa.k:99`
                // is a column header for the table beneath it.
                Sep::Tight | Sep::Space if gap.width > 1 => Sep::Pad(gap.width),
                Sep::Tight | Sep::Space => Sep::Space,
                Sep::Pad(n) => Sep::Pad(n),
                Sep::Hard => Sep::Hard,
                Sep::HardBlank | Sep::Blank => {
                    if gap.blank {
                        Sep::Blank
                    } else {
                        Sep::Hard
                    }
                }
            }
        };
        parts.push(self.sep(tail, true, false));
        Doc::concat(parts)
    }

    /// Comments that trail the last element of a body, with no separator after
    /// them: the caller closes the line itself.
    fn gap_tail(&self, gap: &Gap, bol: bool) -> Doc {
        let comments = self.keep(gap, true);
        let mut parts = Vec::with_capacity(comments.len() * 2);
        for (i, c) in comments.iter().copied().enumerate() {
            let s = if i == 0 && bol {
                Sep::Tight
            } else if i == 0 && !c.own_line {
                Sep::Space
            } else if c.blank_before {
                Sep::Blank
            } else {
                Sep::Hard
            };
            parts.push(self.sep(s, true, false));
            parts.push(comment_doc(c));
        }
        Doc::concat(parts)
    }

    /// The comments of `gap` that stay in the document; the rest are lifted.
    fn keep<'g>(&self, gap: &'g Gap, may_lift: bool) -> Vec<&'g Comment> {
        gap.comments
            .iter()
            .filter(|c| {
                let lift = may_lift && c.is_trailing();
                if lift {
                    self.lift(c);
                }
                !lift
            })
            .collect()
    }

    fn elem(&self, e: &SyntaxElement, at: At) -> Doc {
        match e {
            SyntaxElement::Node(n) => self.node(n, at),
            SyntaxElement::Token(t) => text_doc(t.text()),
        }
    }

    // -------------------------------------------------------------- dispatch

    fn node(&self, n: &SyntaxNode, at: At) -> Doc {
        match n.kind() {
            SOURCE_FILE => self.source_file(n, at),
            MODULE_DECL => self.module_decl(n, at),
            MODULE_PATH => self.tight(n, at),
            MODULE_EXPORT_LIST => self.delim_node(n, at, module_export_cfg()),

            BLOCK_STMT => self.braced(n, at, false),
            DECL_LIST => match n.parent().map(|p| p.kind()) {
                Some(ENUM_DECL) => self.enum_body(n, at),
                _ => self.braced(n, at, true),
            },

            CLASS_DECL | STRUCT_DECL | UNION_DECL => self.record(n, at),
            ENUM_DECL => self.record(n, at),
            TEMPLATE_DECL => self.template_decl(n, at),
            TEMPLATE_PARAM_LIST => self.template_param_list(n, at),
            PARAM_LIST | ARG_LIST => self.delim_node(n, at, paren_list_cfg()),
            FN_TYPE_PARAM_LIST => self.delim_node(n, at, fn_type_list_cfg()),
            TEMPLATE_ARG_LIST | CAPTURE_LIST => self.delim_node(n, at, tight_list_cfg()),
            INIT_LIST_EXPR => self.init_list(n, at),

            FN_DECL | FN_PROTO | VAR_DECL | EXPORT_DECL | EXTERN_DECL | CALL_EXPR
            | ANNOTATED_STMT | ARRAY_TYPE | FN_TYPE | PARAM | FN_TYPE_PARAM | DO_WHILE_STMT
            | FOR_STMT | STATIC_FOR_STMT => self.attributed(n, at),

            IF_STMT | STATIC_IF_STMT => self.if_stmt(n, at),
            ELSE_BRANCH => self.else_branch(n, at),
            WHILE_STMT => self.while_stmt(n, at),
            SWITCH_STMT => self.switch_stmt(n, at),
            SWITCH_CASE | SWITCH_DEFAULT => self.switch_arm(n, at),
            ATOMIC_STMT | REORDER_STMT => self.prefixed_stmt(n, at),
            ASSIGN_STMT => self.assign_stmt(n, at),

            // `a < b` and `a > b` share their tokens with template brackets,
            // which `inline_sep` keeps tight; as operators they are spaced like
            // every other binary operator (`docs/spec/style.md` §6.1: 95.6-100%).
            BIN_EXPR | TERNARY_EXPR => self.spaced(n, at),
            CONCAT_EXPR | MUX_EXPR | LUT_MUL_EXPR | BIT_OFFSET_EXPR | BYTE_OFFSET_EXPR => {
                self.keyword_call(n, at)
            }
            LAMBDA_EXPR => self.lambda(n, at),
            PREFIX_EXPR => self.prefix_expr(n, at),
            INTERPOLATED_STRING => self.verbatim(n),
            VISIBILITY => self.tight(n, at),

            _ => self.inline(n, at),
        }
    }

    // ------------------------------------------------------------- utilities

    /// Joins the children with separators chosen by `f`, which sees the gap
    /// index and returns the separator and an extra indent for that piece.
    fn join(
        &self,
        ch: &Children,
        at: At,
        f: impl Fn(&Children, usize) -> (Sep, isize),
    ) -> Doc {
        let mut parts = Vec::with_capacity(ch.len() * 2 + 1);
        for i in 0..ch.len() {
            let (sep, nest) = if i == 0 { (Sep::Tight, 0) } else { f(ch, i) };
            let child = if i == 0 { at } else { At::after(breaks(sep)) };
            let piece = Doc::concat([
                self.gap(&ch.gaps[i], sep, i == 0 && at.line_start),
                self.elem(&ch.elems[i], child),
            ]);
            parts.push(if nest == 0 { piece } else { piece.nest(nest) });
        }
        parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
        Doc::concat(parts)
    }

    /// The default layout: everything on one line, spaced per
    /// `docs/spec/style.md` §6.
    fn inline(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| (inline_sep(ch.kind(i - 1), ch.kind(i)), 0))
    }

    /// A binary or ternary expression: one space around every operator, and the
    /// author's own line breaks kept.
    ///
    /// `docs/spec/style.md` §1.4 measured 28 operator-terminated lines in
    /// 119 782 and concluded there is no convention to follow — so the
    /// formatter must not invent one, and equally must not undo the one the
    /// author reached for. Where they wrapped, it wraps; the continuation is
    /// indented one level rather than aligned under the operand, because
    /// alignment would depend on the width of everything to its left.
    fn spaced(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let indent = self.indent;
        self.join(&ch, at, move |ch, i| {
            if ch.gaps[i].newline { (Sep::Hard, indent) } else { (Sep::Space, 0) }
        })
    }

    /// Everything butted together: module paths, `public:`, `a::b`.
    fn tight(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |_, _| (Sep::Tight, 0))
    }

    /// The node's source text, unaltered. Used for interpolated strings, whose
    /// interior whitespace is part of the value (`{expr=}` echoes the
    /// expression's source text) and must never be touched.
    fn verbatim(&self, n: &SyntaxNode) -> Doc {
        text_doc(&n.text().to_string())
    }

    // ------------------------------------------------------------ item lists

    /// A run of declarations or statements, one per line.
    ///
    /// `top` turns on the blank-line rules that only hold at file and
    /// class-member scope (`docs/spec/style.md` §7.3: 99.7% and 97.5%, decaying
    /// to a coin flip deeper in, which is why they are *not* applied inside a
    /// function body).
    fn items(
        &self,
        ch: &Children,
        order: &[usize],
        tail: Option<usize>,
        first: At,
        top: bool,
    ) -> Doc {
        let docs: Vec<Doc> = order.iter().map(|&i| self.elem(&ch.elems[i], At::ITEM)).collect();
        let mut parts = Vec::with_capacity(order.len() * 2 + 1);

        for (k, &i) in order.iter().enumerate() {
            let sep = if k == 0 {
                if first.line_start { Sep::HardBlank } else { Sep::Hard }
            } else if matches!(ch.kind(i), EMPTY_STMT | SEMICOLON)
                && !ch.gaps[i].newline
                && ch.gaps[i].is_empty()
            {
                // A stray `;` the author left on the previous line — `};` after
                // a class body. The grammar makes it a sibling item; keeping it
                // where it was is both minimal and token-preserving.
                Sep::Tight
            } else if self.cfg.group_imports
                && ch.kind(i) == IMPORT_DECL
                && ch.kind(order[k - 1]) == IMPORT_DECL
            {
                Sep::Hard
            } else if self.wants_blank(ch, order[k - 1], i, top) {
                Sep::Blank
            } else {
                Sep::HardBlank
            };
            let piece =
                Doc::concat([self.gap(&ch.gaps[i], sep, k == 0 && first.line_start), docs[k].clone()]);
            // `public:` / `private:` sit one level out from the members they
            // introduce, at the column of the `class` keyword itself
            // (`docs/spec/style.md` §2.5: 2187 : 67).
            parts.push(if ch.kind(i) == VISIBILITY { piece.nest(-self.indent) } else { piece });
        }
        if let Some(tail) = tail {
            parts.push(self.gap_tail(&ch.gaps[tail], order.is_empty() && first.line_start));
        }
        Doc::concat(parts)
    }

    /// Whether a blank line is inserted before an item even though the author
    /// wrote none.
    fn wants_blank(&self, ch: &Children, prev_i: usize, i: usize, top: bool) -> bool {
        if !top {
            return false;
        }
        let (before, here) = (ch.kind(prev_i), ch.kind(i));
        match (before, here) {
            // One contiguous import block, one blank line on each side
            // (`docs/spec/style.md` §9.3: 307/307 and 303/307).
            (IMPORT_DECL, IMPORT_DECL) => false,
            (IMPORT_DECL, _) | (_, IMPORT_DECL) => true,
            // A stray `;` is not a declaration boundary.
            (EMPTY_STMT, _) | (_, EMPTY_STMT) => false,
            // §7.4: 918 : 61 — but never for the first label, which `items`
            // never reaches here because `k == 0` is handled above.
            (_, VISIBILITY) => true,
            (VISIBILITY, _) => false,
            // §7.3: a declaration with a body is followed by a blank line.
            // Runs of one-liners — `static assert(…);` in `library/base.k`,
            // consecutive fields, `using` aliases — are left alone, which is
            // what §12 example 10 requires.
            _ => has_body(before),
        }
    }

    /// `{ … }` around an item list, Allman-style, collapsed body preserved
    /// (`docs/spec/style.md` §2.2, §10.2).
    fn braced(&self, n: &SyntaxNode, at: At, top: bool) -> Doc {
        let ch = trivia::children(n);
        let Some(open) = ch.find(L_CURLY) else { return self.inline(n, at) };
        let close = last_of(&ch, R_CURLY).unwrap_or(ch.len());
        self.brace_body(&ch, open, close, at, top)
    }

    fn brace_body(
        &self,
        ch: &Children,
        open: usize,
        close: usize,
        at: At,
        top: bool,
    ) -> Doc {
        let head = Doc::concat([
            self.gap(&ch.gaps[open], Sep::Tight, at.line_start),
            self.elem(&ch.elems[open], at),
        ]);
        if collapsed_span(ch, open, close) {
            let mut parts = vec![head];
            if close > open + 1 {
                let _pin = self.pin_flat();
                for i in open + 1..close {
                    parts.push(Doc::text(" "));
                    parts.push(self.elem(&ch.elems[i], At::MID));
                }
                parts.push(Doc::text(" "));
            }
            parts.push(self.closer(ch, close));
            return Doc::concat(parts);
        }
        let order: Vec<usize> = (open + 1..close).collect();
        Doc::concat([
            head,
            self.items(ch, &order, Some(close), At::MID, top).nest(self.indent),
            Doc::hard_line(),
            self.closer(ch, close),
        ])
    }

    fn closer(&self, ch: &Children, close: usize) -> Doc {
        if close < ch.len() { self.elem(&ch.elems[close], At::MID) } else { Doc::nil() }
    }

    // ------------------------------------------------------------ list logic

    fn delim_node(&self, n: &SyntaxNode, at: At, cfg: ListCfg) -> Doc {
        let ch = trivia::children(n);
        let close = ch.len().saturating_sub(1);
        self.delim(&ch, 0, close, cfg, at)
    }

    /// One delimited list, from the separator before `open` through `close`.
    fn delim(
        &self,
        ch: &Children,
        open: usize,
        close: usize,
        cfg: ListCfg,
        at: At,
    ) -> Doc {
        if close <= open {
            return self.join(ch, at, |ch, i| (inline_sep(ch.kind(i - 1), ch.kind(i)), 0));
        }
        let mut cfg = cfg;
        if at.item_start {
            cfg.open_breaks = false;
            cfg.indent = false;
        }
        // A list that never breaks on width still breaks for a comment: a
        // `//` inside one would swallow whatever followed it.
        let commented = (open..=close.min(ch.len())).any(|i| !ch.gaps[i].is_empty());
        let pinned = cfg.never_break || self.no_break.get() > 0;
        let mode =
            if pinned && !commented { Mode::Flat } else { list_mode(ch, open, close) };
        match mode {
            // Nothing in the flat layout breaks on the list's account, so it
            // contributes no indent either: a lambda argument's own brace
            // belongs at the indent of the statement that opened it
            // (`docs/spec/style.md` §12, example 5).
            Mode::Flat => self.delim_flat(ch, open, close, cfg, at),
            Mode::Broken => {
                let d = self.delim_broken(ch, open, close, cfg, at);
                if cfg.indent { d.nest(self.indent) } else { d }
            }
            Mode::Auto => {
                let d = self.delim_auto(ch, open, close, cfg, at);
                if cfg.indent { d.nest(self.indent) } else { d }
            }
        }
    }

    fn delim_flat(
        &self,
        ch: &Children,
        open: usize,
        close: usize,
        cfg: ListCfg,
        at: At,
    ) -> Doc {
        let mut parts = vec![
            self.gap(&ch.gaps[open], cfg.open_flat, at.line_start),
            self.elem(&ch.elems[open], at),
        ];
        for i in open + 1..close {
            let sep = if ch.kind(i) == COMMA || (i == open + 1 && !cfg.pad_flat) {
                Sep::Tight
            } else {
                Sep::Space
            };
            parts.push(self.gap(&ch.gaps[i], sep, false));
            parts.push(self.elem(&ch.elems[i], At::MID));
        }
        let sep = if close == open + 1 || !cfg.pad_flat { Sep::Tight } else { Sep::Space };
        parts.push(self.gap(&ch.gaps[close], sep, false));
        parts.push(self.closer(ch, close));
        Doc::concat(parts)
    }

    fn delim_broken(
        &self,
        ch: &Children,
        open: usize,
        close: usize,
        cfg: ListCfg,
        at: At,
    ) -> Doc {
        let open_sep = if cfg.open_breaks { Sep::Hard } else { cfg.open_flat };
        let mut middle = Vec::with_capacity((close - open) * 2);
        for i in open + 1..close {
            let sep = if cfg.preserve {
                preserved_sep(ch, i, i == open + 1, cfg)
            } else if ch.kind(i) == COMMA {
                if cfg.leading { Sep::HardBlank } else { Sep::Tight }
            } else if i == open + 1 {
                if cfg.hug { Sep::Space } else { Sep::HardBlank }
            } else if ch.kind(i - 1) == COMMA {
                if cfg.leading { Sep::Space } else { Sep::HardBlank }
            } else {
                Sep::Space
            };
            // A comment between a separator and its item — the shape of
            // `library/data/array.k:7-16` and of the section headings in
            // `library/processor/risc_v/isa.k:8-16` — sits two columns in,
            // under the item column rather than the separator column
            // (`docs/spec/style.md` §12, example 3).
            let commented = cfg.leading && !ch.gaps[i].is_empty();
            let piece = if commented && ch.kind(i) == COMMA {
                // The comments indent; the separator that follows them does not.
                Doc::concat([
                    self.gap_tail(&ch.gaps[i], false).nest(2),
                    self.sep(sep, ch.gaps[i].blank, false),
                    self.elem(&ch.elems[i], At::after(breaks(sep))),
                ])
            } else {
                let piece = Doc::concat([
                    self.gap(&ch.gaps[i], sep, false),
                    self.elem(&ch.elems[i], At::after(breaks(sep))),
                ]);
                if commented { piece.nest(2) } else { piece }
            };
            middle.push(piece);
        }
        Doc::concat([
            self.gap(&ch.gaps[open], open_sep, at.line_start),
            self.elem(&ch.elems[open], at),
            Doc::concat(middle).nest(if cfg.leading { 0 } else { self.indent }),
            self.gap(&ch.gaps[close], self.close_sep(ch, close, cfg), false),
            self.closer(ch, close),
        ])
    }

    /// The separator before a broken list's closing delimiter.
    fn close_sep(&self, ch: &Children, close: usize, cfg: ListCfg) -> Sep {
        if cfg.preserve && !ch.gaps[close].newline {
            if cfg.pad_flat { Sep::Space } else { Sep::Tight }
        } else {
            Sep::Hard
        }
    }

    fn delim_auto(
        &self,
        ch: &Children,
        open: usize,
        close: usize,
        cfg: ListCfg,
        at: At,
    ) -> Doc {
        debug_assert!(
            (open..=close).all(|i| ch.gaps[i].is_empty()),
            "a comment reached the width-driven list layout"
        );
        let pad = if cfg.pad_flat { Doc::text(" ") } else { Doc::nil() };
        let open_sep = match (cfg.open_breaks, cfg.open_flat) {
            (true, Sep::Space) => Doc::line(),
            (true, _) => Doc::soft_line(),
            (false, s) => self.sep(s, false, at.line_start),
        };
        let after_open = if cfg.hug {
            Doc::if_broken(pad.clone(), Doc::text(" "))
        } else {
            Doc::if_broken(pad.clone(), Doc::nil())
        };

        let mut middle = Vec::with_capacity((close - open) * 2 + 1);
        if !cfg.hug && close > open + 1 {
            // Inside the body's own indent, so a list that breaks puts its
            // first item at the same column as the rest.
            middle.push(Doc::soft_line());
        }
        for i in open + 1..close {
            let sep = if ch.kind(i) == COMMA {
                if cfg.leading { Doc::soft_line() } else { Doc::nil() }
            } else if i == open + 1 {
                Doc::nil()
            } else if ch.kind(i - 1) == COMMA {
                if cfg.leading { Doc::text(" ") } else { Doc::line() }
            } else {
                Doc::text(" ")
            };
            middle.push(sep);
            middle.push(self.elem(&ch.elems[i], At::MID));
        }
        let body = Doc::concat(middle).nest(if cfg.leading { 0 } else { self.indent });
        let before_close = if close == open + 1 {
            Doc::nil()
        } else {
            Doc::concat([Doc::if_broken(pad, Doc::nil()), Doc::soft_line()])
        };
        Doc::concat([
            open_sep,
            self.elem(&ch.elems[open], at),
            if close == open + 1 { Doc::nil() } else { after_open },
            body,
            before_close,
            self.closer(ch, close),
        ])
        .group()
    }

    // ------------------------------------------------------------ constructs

    fn source_file(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let mut order: Vec<usize> = (0..ch.len()).collect();
        if self.cfg.sort_imports {
            sort_import_block(&ch, &mut order);
        }
        self.items(&ch, &order, Some(ch.len()), at, true)
    }

    /// The declarations and statements overlapping `range`, plus the span they
    /// cover. Used by `format_range`; `parent` is the node whose children they
    /// are, so their indentation is the one already in the file.
    pub(crate) fn range_doc(
        &self,
        parent: &SyntaxNode,
        range: kg_base::TextRange,
    ) -> Option<(Doc, kg_base::TextRange)> {
        let ch = trivia::children(parent);
        let order: Vec<usize> = (0..ch.len())
            .filter(|&i| !matches!(ch.kind(i), L_CURLY | R_CURLY))
            .filter(|&i| {
                let r = elem_range(&ch.elems[i]);
                r.end() >= range.start() && r.start() <= range.end()
            })
            .collect();
        let (&first, &last) = (order.first()?, order.last()?);
        let span = kg_base::TextRange::new(
            elem_range(&ch.elems[first]).start(),
            elem_range(&ch.elems[last]).end(),
        );
        let top = matches!(parent.kind(), SOURCE_FILE | DECL_LIST);
        // `tail` points at the last selected item so no trailing trivia — the
        // whitespace before the next declaration — is pulled into the edit.
        Some((self.items(&ch, &order, None, At::ITEM, top), span))
    }

    /// `module a.b { X, Y }`, and the bare `module a.b` / `module a.b \ c.d`
    /// forms that appear as export-list entries.
    fn module_decl(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| (inline_sep(ch.kind(i - 1), ch.kind(i)), 0))
    }

    /// `class`/`struct`/`union`/`enum` — Allman brace, 100% of the corpus
    /// (`docs/spec/style.md` §2.2).
    fn record(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            if ch.kind(i) == DECL_LIST {
                body_sep(&ch.gaps[i], &ch.elems[i])
            } else {
                (inline_sep(ch.kind(i - 1), ch.kind(i)), 0)
            }
        })
    }

    /// The `{ A = 0, B = 1 }` body of an `enum`.
    ///
    /// Separator style and brace hugging are read back from the source: the
    /// corpus is 58 trailing : 1 leading (§3.5), but that one — the aligned
    /// `ABI` register table of `library/processor/risc_v/isa.k:185` — is a
    /// deliberate table that §10.3 says to leave alone.
    fn enum_body(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let close = ch.len().saturating_sub(1);
        let leading = detect_leading(&ch, 0, close);
        // The body's own indent is supplied by the `enum` header's Allman
        // break, so the list adds none of its own.
        let cfg = ListCfg {
            open_flat: Sep::Tight,
            open_breaks: false,
            pad_flat: true,
            hug: leading && detect_hug(&ch, 0),
            leading,
            indent: false,
            never_break: false,
            preserve: true,
        };
        self.delim(&ch, 0, close, cfg, at)
    }

    fn init_list(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let close = ch.len().saturating_sub(1);
        let leading = detect_leading(&ch, 0, close);
        let cfg = ListCfg {
            open_flat: Sep::Tight,
            open_breaks: false,
            // `= { a, b }` 138 : `= {a, b}` 266 — too even to normalize, and
            // the padded cases are hand-aligned numeric tables (§10.7).
            pad_flat: ch.gaps.get(1).is_some_and(|g| g.space),
            hug: leading && detect_hug(&ch, 0),
            leading,
            indent: false,
            never_break: false,
            preserve: true,
        };
        self.delim(&ch, 0, close, cfg, at)
    }

    fn template_decl(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            if ch.kind(i - 1) == TEMPLATE_PARAM_LIST { (Sep::Hard, 0) } else { (Sep::Space, 0) }
        })
    }

    /// `template <T, U>` on one line (1295 cases), or `template` alone with the
    /// list beneath it at +4 (122 cases) — `docs/spec/style.md` §3.2.
    fn template_param_list(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let Some(open) = ch.find(L_ANGLE) else { return self.inline(n, at) };
        let close = last_of(&ch, R_ANGLE).unwrap_or(ch.len().saturating_sub(1));
        let cfg = ListCfg {
            open_flat: Sep::Space,
            open_breaks: true,
            pad_flat: false,
            hug: true,
            leading: true,
            indent: true,
            never_break: false,
            preserve: false,
        };
        let mut parts = Vec::new();
        for i in 0..open {
            let sep = if i == 0 { Sep::Tight } else { Sep::Space };
            let child = if i == 0 { at } else { At::MID };
            parts.push(self.gap(&ch.gaps[i], sep, i == 0 && at.line_start));
            parts.push(self.elem(&ch.elems[i], child));
        }
        parts.push(self.delim(&ch, open, close, cfg, At::MID));
        for i in close + 1..ch.len() {
            parts.push(self.gap(&ch.gaps[i], Sep::Space, false));
            parts.push(self.elem(&ch.elems[i], At::MID));
        }
        parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
        Doc::concat(parts)
    }

    /// Any construct that may carry leading `[[…]]` groups.
    ///
    /// Placement is preserved: the corpus splits 753 same-line : 186 own-line :
    /// 126 mid-line and the split is *per attribute*, not per author
    /// (`docs/spec/style.md` §10.8) — `[[schedule(…)]]` is 53/53 own-line and
    /// `[[pipelined]]` 228/228 same-line.
    fn attributed(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let kind = n.kind();
        self.join(&ch, at, move |ch, i| {
            let (before, here) = (ch.kind(i - 1), ch.kind(i));
            if before == ATTR_LIST {
                return (if ch.gaps[i].newline { Sep::Hard } else { Sep::Space }, 0);
            }
            match kind {
                FN_DECL | LAMBDA_EXPR if here == BLOCK_STMT => {
                    body_sep(&ch.gaps[i], &ch.elems[i])
                }
                VAR_DECL if here == INIT_LIST_EXPR => {
                    // `T x = {` (137) or `T x =` then the brace beneath (50):
                    // both are written, so the author's line break decides —
                    // and, when they broke, whether they indented the brace.
                    // `library/data/datetime.k:41` keeps it at the declaration
                    // column; `library/data/closure.k:70` indents it to carry a
                    // leading-comma list.
                    if !ch.gaps[i].newline {
                        (Sep::Space, 0)
                    } else if indented_in_source(ch, i) {
                        (Sep::Hard, self.indent)
                    } else {
                        (Sep::Hard, 0)
                    }
                }
                FOR_STMT | STATIC_FOR_STMT => self.loop_sep(ch, i),
                DO_WHILE_STMT => self.do_while_sep(ch, i),
                _ => (inline_sep(before, here), 0),
            }
        })
    }

    fn loop_sep(&self, ch: &Children, i: usize) -> (Sep, isize) {
        let (before, here) = (ch.kind(i - 1), ch.kind(i));
        match (before, here) {
            // `for (` is 99.2% spaced; `static for (` is a dead heat, 285 : 215,
            // so it is preserved (`docs/spec/style.md` §10.6).
            (FOR_KW, L_PAREN) => {
                let statik = ch.kind(0) == STATIC_KW || ch.find(STATIC_KW).is_some();
                if statik && !ch.gaps[i].space {
                    (Sep::Tight, 0)
                } else {
                    (Sep::Space, 0)
                }
            }
            (_, R_PAREN) | (L_PAREN, _) => (Sep::Tight, 0),
            (R_PAREN, _) => body_sep(&ch.gaps[i], &ch.elems[i]),
            _ => (inline_sep(before, here), 0),
        }
    }

    fn do_while_sep(&self, ch: &Children, i: usize) -> (Sep, isize) {
        let (before, here) = (ch.kind(i - 1), ch.kind(i));
        match (before, here) {
            // `do;` — the empty-body idiom, 89 : 3 (`docs/spec/style.md` §6.9).
            (DO_KW, EMPTY_STMT) => (Sep::Tight, 0),
            (DO_KW, _) => body_sep(&ch.gaps[i], &ch.elems[i]),
            (_, WHILE_KW) => (if ch.gaps[i].newline { Sep::Hard } else { Sep::Space }, 0),
            // `while (` is 53.8% spaced corpus-wide and 39.1% in `library/`,
            // the one control keyword the library prefers tight (§10.5).
            (WHILE_KW, L_PAREN) => {
                (if ch.gaps[i].space { Sep::Space } else { Sep::Tight }, 0)
            }
            (_, R_PAREN) | (L_PAREN, _) => (Sep::Tight, 0),
            _ => (inline_sep(before, here), 0),
        }
    }

    fn if_stmt(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            let (before, here) = (ch.kind(i - 1), ch.kind(i));
            match (before, here) {
                (IF_KW, L_PAREN) => (Sep::Space, 0),
                (L_PAREN, _) | (_, R_PAREN) => (Sep::Tight, 0),
                (_, ELSE_BRANCH) => (Sep::Hard, 0),
                (R_PAREN, _) => body_sep(&ch.gaps[i], &ch.elems[i]),
                _ => (inline_sep(before, here), 0),
            }
        })
    }

    /// `else` is never written `} else` — 0 occurrences against 329 own-line
    /// `else` (`docs/spec/style.md` §2.2) — but an `else if` chain stays on one
    /// line (129 occurrences).
    fn else_branch(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            if matches!(ch.kind(i), IF_STMT | STATIC_IF_STMT) {
                (Sep::Space, 0)
            } else {
                body_sep(&ch.gaps[i], &ch.elems[i])
            }
        })
    }

    fn while_stmt(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            let (before, here) = (ch.kind(i - 1), ch.kind(i));
            match (before, here) {
                (WHILE_KW, L_PAREN) => {
                    (if ch.gaps[i].space { Sep::Space } else { Sep::Tight }, 0)
                }
                (L_PAREN, _) | (_, R_PAREN) => (Sep::Tight, 0),
                (R_PAREN, _) => body_sep(&ch.gaps[i], &ch.elems[i]),
                _ => (inline_sep(before, here), 0),
            }
        })
    }

    fn switch_stmt(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let Some(open) = ch.find(L_CURLY) else { return self.inline(n, at) };
        let close = last_of(&ch, R_CURLY).unwrap_or(ch.len());
        let mut parts = Vec::new();
        for i in 0..open {
            let sep = if i == 0 {
                Sep::Tight
            } else {
                match (ch.kind(i - 1), ch.kind(i)) {
                    (SWITCH_KW, L_PAREN) => Sep::Space,
                    (L_PAREN, _) | (_, R_PAREN) => Sep::Tight,
                    (b, h) => inline_sep(b, h),
                }
            };
            let child = if i == 0 { at } else { At::MID };
            parts.push(self.gap(&ch.gaps[i], sep, i == 0 && at.line_start));
            parts.push(self.elem(&ch.elems[i], child));
        }
        // `case` sits one level inside the switch brace, its body one further
        // (`docs/spec/style.md` §2.6: 251 : 123).
        parts.push(Doc::hard_line());
        parts.push(self.brace_body(&ch, open, close, At::MID, false));
        for i in close + 1..ch.len() {
            parts.push(self.gap(&ch.gaps[i], Sep::Tight, false));
            parts.push(self.elem(&ch.elems[i], At::MID));
        }
        parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
        Doc::concat(parts)
    }

    fn switch_arm(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let Some(colon) = ch.find(COLON) else { return self.inline(n, at) };
        let mut parts = Vec::new();
        for i in 0..=colon {
            let sep = if i == 0 { Sep::Tight } else { inline_sep(ch.kind(i - 1), ch.kind(i)) };
            let sep = if ch.kind(i) == COLON { Sep::Tight } else { sep };
            let child = if i == 0 { at } else { At::MID };
            parts.push(self.gap(&ch.gaps[i], sep, i == 0 && at.line_start));
            parts.push(self.elem(&ch.elems[i], child));
        }
        if colon + 1 >= ch.len() {
            parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
            return Doc::concat(parts);
        }
        // 197 arms write the body on the label's line and 177 beneath it
        // (`docs/spec/style.md` §10.9); the author decides.
        if collapsed_span(&ch, colon, ch.len()) {
            let _pin = self.pin_flat();
            for i in colon + 1..ch.len() {
                parts.push(Doc::text(" "));
                parts.push(self.elem(&ch.elems[i], At::MID));
            }
            parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
        } else {
            let order: Vec<usize> = (colon + 1..ch.len()).collect();
            parts.push(self.items(&ch, &order, Some(ch.len()), At::MID, false).nest(self.indent));
        }
        Doc::concat(parts)
    }

    /// `concat(a, b)`, `mux(sel, a, b)`, `lutmul(a, b)` and the two
    /// `…offsetof(T, field)` forms: a keyword, then a comma-separated list that
    /// breaks like any other argument list rather than running off the page.
    fn keyword_call(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let Some(open) = ch.find(L_PAREN) else { return self.inline(n, at) };
        let close = last_of(&ch, R_PAREN).unwrap_or(ch.len().saturating_sub(1));
        let mut parts = Vec::new();
        for i in 0..open {
            let child = if i == 0 { at } else { At::MID };
            let sep = if i == 0 { Sep::Tight } else { inline_sep(ch.kind(i - 1), ch.kind(i)) };
            parts.push(self.gap(&ch.gaps[i], sep, i == 0 && at.line_start));
            parts.push(self.elem(&ch.elems[i], child));
        }
        parts.push(self.delim(&ch, open, close, paren_list_cfg(), At::MID));
        for i in close + 1..ch.len() {
            parts.push(self.gap(&ch.gaps[i], Sep::Tight, false));
            parts.push(self.elem(&ch.elems[i], At::MID));
        }
        parts.push(self.gap(&ch.gaps[ch.len()], Sep::Tight, false));
        Doc::concat(parts)
    }

    fn prefixed_stmt(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| body_sep(&ch.gaps[i], &ch.elems[i]))
    }

    /// `x = e;`, `x += e;`, `x++;`.
    ///
    /// The parser leaves a compound assignment as its component tokens — `+`
    /// then `=` — so the operator has to be glued back together here, and
    /// `++`/`--` bind tight to their operand (`docs/spec/style.md` §6.3).
    fn assign_stmt(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        let ops: Vec<SyntaxKind> =
            (0..ch.len()).map(|i| ch.kind(i)).filter(|k| is_op_token(*k)).collect();
        let incdec = matches!(ops.as_slice(), [PLUS, PLUS] | [MINUS, MINUS]);
        self.join(&ch, at, move |ch, i| {
            let (before, here) = (ch.kind(i - 1), ch.kind(i));
            let sep = if here == SEMICOLON || (is_op_token(here) && is_op_token(before)) {
                // A compound assignment reaches the tree as its component
                // tokens — `+` then `=` — and has to be glued back together.
                Sep::Tight
            } else if is_op_token(here) || is_op_token(before) {
                if incdec { Sep::Tight } else { Sep::Space }
            } else {
                inline_sep(before, here)
            };
            (sep, 0)
        })
    }

    fn lambda(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            if ch.kind(i) == BLOCK_STMT {
                body_sep(&ch.gaps[i], &ch.elems[i])
            } else {
                (inline_sep(ch.kind(i - 1), ch.kind(i)), 0)
            }
        })
    }

    /// `!x`, `~x`, `-x` bind tight (638 : 0 and 84 : 0); `bitsizeof`,
    /// `bytesizeof` and `clog2` are keyword operators, written with parentheses
    /// throughout the corpus (`docs/spec/style.md` §6.3, §6.7).
    fn prefix_expr(&self, n: &SyntaxNode, at: At) -> Doc {
        let ch = trivia::children(n);
        self.join(&ch, at, |ch, i| {
            let sep = match ch.kind(i - 1) {
                MINUS | BANG | TILDE => Sep::Tight,
                _ => match &ch.elems[i] {
                    SyntaxElement::Node(node) => {
                        if node.first_token().map(|t| t.kind()) == Some(L_PAREN) {
                            Sep::Tight
                        } else {
                            Sep::Space
                        }
                    }
                    SyntaxElement::Token(_) => Sep::Space,
                },
            };
            (sep, 0)
        })
    }
}

/// Restores the enclosing break budget when a collapsed body is finished.
struct FlatPin<'a>(&'a std::cell::Cell<usize>);

impl Drop for FlatPin<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl std::fmt::Debug for FlatPin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FlatPin").field(&self.0.get()).finish()
    }
}

// ------------------------------------------------------------------- helpers

/// The default spacing table (`docs/spec/style.md` §6).
///
/// `::` is 8014 : 0 tight, member `.` 9680 : 1, `(` after a callee 15 243 : 114,
/// `,` followed by a space 42 784 : 8. Everything the table does not name is
/// spaced, which is the right default for a language whose binary operators are
/// 95-100% spaced.
fn inline_sep(before: SyntaxKind, here: SyntaxKind) -> Sep {
    match (before, here) {
        // The template-template parameter form `template <typename, auto>
        // typename Memory = memory` (`Parser.hs:370-371`), spelled the same way
        // as the outer keyword: 1100 : 236 (§6.7).
        (TEMPLATE_KW, L_ANGLE) => Sep::Space,
        (
            _,
            SEMICOLON
            | COMMA
            | R_PAREN
            | R_BRACK
            | R_ANGLE
            | L_PAREN
            | L_BRACK
            | L_ANGLE
            | COLON2
            | DOT
            | TEMPLATE_ARG_LIST
            | ARG_LIST
            | PARAM_LIST
            | FN_TYPE_PARAM_LIST
            | ATTR_ARGS
            | CAPTURE_LIST
            | MODULE_EXPORT_LIST,
        ) => Sep::Tight,
        (L_PAREN | L_BRACK | L_ANGLE | COLON2 | DOT | BANG | TILDE, _) => Sep::Tight,
        _ => Sep::Space,
    }
}

/// Where a body goes relative to its header.
///
/// Allman for anything braced — 13 375 : 173 across every block construct — and
/// the author's own break for an unbraced one, which the corpus has no
/// convention for.
fn body_sep(gap: &Gap, e: &SyntaxElement) -> (Sep, isize) {
    match e {
        SyntaxElement::Node(n) if matches!(n.kind(), BLOCK_STMT | DECL_LIST) => {
            if is_collapsed(n) && !gap.newline { (Sep::Space, 0) } else { (Sep::Hard, 0) }
        }
        _ => {
            if gap.newline {
                (Sep::Hard, 4)
            } else {
                (Sep::Space, 0)
            }
        }
    }
}

/// Whether a declaration always brings a body with it, and so always spans
/// several lines.
///
/// The blank-line rule is stated over kinds rather than over the rendered
/// height because height depends on the line width: a declaration that the
/// width happened to break in one pass would gain a blank line in the next, and
/// the formatter would not be idempotent.
fn has_body(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        CLASS_DECL | STRUCT_DECL | UNION_DECL | ENUM_DECL | FN_DECL | TEMPLATE_DECL | MODULE_DECL
    )
}

/// Whether element `i` starts further right in the source than its node does.
fn indented_in_source(ch: &Children, i: usize) -> bool {
    let (Some(here), Some(start)) = (first_token(&ch.elems[i]), first_token(&ch.elems[0])) else {
        return false;
    };
    source_column(&here) > source_column(&start)
}

fn first_token(e: &SyntaxElement) -> Option<kg_syntax::SyntaxToken> {
    match e {
        SyntaxElement::Node(n) => n.first_token(),
        SyntaxElement::Token(t) => Some(t.clone()),
    }
}

/// The column `tok` starts at in the source, in characters.
fn source_column(tok: &kg_syntax::SyntaxToken) -> usize {
    let mut col = 0;
    let mut cur = tok.prev_token();
    while let Some(t) = cur {
        let text = t.text();
        if let Some(nl) = text.rfind('\n') {
            return col + text[nl + 1..].chars().count();
        }
        col += text.chars().count();
        cur = t.prev_token();
    }
    col
}

/// The separator the author wrote before element `i`, reproduced.
fn preserved_sep(ch: &Children, i: usize, first: bool, cfg: ListCfg) -> Sep {
    let gap = &ch.gaps[i];
    if gap.newline {
        return Sep::HardBlank;
    }
    if ch.kind(i) == COMMA {
        return Sep::Tight;
    }
    if first {
        return if cfg.pad_flat { Sep::Space } else { Sep::Tight };
    }
    Sep::Pad(gap.width)
}

/// Whether a separator ends the line, so whatever follows starts a new one.
fn breaks(sep: Sep) -> bool {
    matches!(sep, Sep::Hard | Sep::HardBlank | Sep::Blank)
}

fn is_op_token(k: SyntaxKind) -> bool {
    matches!(
        k,
        EQ | PLUS
            | MINUS
            | STAR
            | SLASH
            | PERCENT
            | AMP
            | PIPE
            | CARET
            | AMP2
            | PIPE2
            | CARET2
            | PLUS2
            | MINUS2
            | PLUSEQ
            | MINUSEQ
            | STAREQ
            | SLASHEQ
            | PERCENTEQ
            | AMPEQ
            | PIPEEQ
            | CARETEQ
            | AMP2EQ
            | PIPE2EQ
            | CARET2EQ
    )
}

fn elem_range(e: &SyntaxElement) -> kg_base::TextRange {
    match e {
        SyntaxElement::Node(n) => n.text_range(),
        SyntaxElement::Token(t) => t.text_range(),
    }
}

/// Sorts the leading run of `import` declarations in place.
///
/// Off by default, and rightly: `import` is executed *during* parsing
/// (`Parser.hs:194-206` folds the imported file's symbols into the table before
/// the next line is read), 18 `library/` files have orderings that look
/// deliberate, and the compiler has a `redundant-import` warning whose firing
/// can depend on order (`docs/spec/style.md` §9.2, §10.4). A formatter cannot
/// prove reordering is safe, so it only does it when asked.
fn sort_import_block(ch: &Children, order: &mut [usize]) {
    let start = order.iter().position(|&i| ch.kind(i) == IMPORT_DECL);
    let Some(start) = start else { return };
    let end = order[start..]
        .iter()
        .position(|&i| ch.kind(i) != IMPORT_DECL)
        .map_or(order.len(), |n| start + n);
    order[start..end].sort_by_key(|&i| match &ch.elems[i] {
        SyntaxElement::Node(n) => n
            .children()
            .find(|c| c.kind() == MODULE_PATH)
            .map(|p| p.text().to_string())
            .unwrap_or_default(),
        SyntaxElement::Token(t) => t.text().to_owned(),
    });
}

fn last_of(ch: &Children, kind: SyntaxKind) -> Option<usize> {
    (0..ch.len()).rev().find(|&i| ch.kind(i) == kind)
}

/// Whether `open..=close` was written on one source line with no comments in
/// it, so a "preserve" rule may keep it there.
fn collapsed_span(ch: &Children, open: usize, close: usize) -> bool {
    (open + 1..=close.min(ch.len())).all(|i| !ch.gaps[i].newline && ch.gaps[i].is_empty())
        && (open + 1..close).all(|i| match &ch.elems[i] {
            SyntaxElement::Node(n) => is_collapsed(n),
            SyntaxElement::Token(t) => !t.text().contains('\n'),
        })
}

fn list_mode(ch: &Children, open: usize, close: usize) -> Mode {
    let span = open..=close.min(ch.len());
    let commented = span.clone().any(|i| !ch.gaps[i].is_empty());
    let items = (open + 1..close).filter(|&i| ch.kind(i) != COMMA).collect::<Vec<_>>();
    if !commented && items.iter().any(|&i| trivia::contains_block(&ch.elems[i])) {
        // A lambda argument brings its own Allman brace; breaking the argument
        // list around it would produce a layout the corpus never writes
        // (`docs/spec/style.md` §12, example 5).
        return Mode::Flat;
    }
    if commented || span.skip(1).any(|i| ch.gaps[i].newline) {
        return Mode::Broken;
    }
    // A one-argument call whose argument is itself multi-line: breaking the
    // list would wrap a lone expression in `( … )` across three lines and gain
    // nothing. `static_cast(a ? b : c)` in `library/numeric/float32.k:195` is
    // the shape this protects. Tested after the break check, so that a list the
    // *formatter* broke on a previous run is not then flattened again.
    if items.len() == 1 && multi_line(&ch.elems[items[0]]) {
        return Mode::Flat;
    }
    Mode::Auto
}

fn multi_line(e: &SyntaxElement) -> bool {
    match e {
        SyntaxElement::Node(n) => n.text().to_string().contains('\n'),
        SyntaxElement::Token(t) => t.text().contains('\n'),
    }
}

/// Whether the author put the separators at the head of their lines.
fn detect_leading(ch: &Children, open: usize, close: usize) -> bool {
    (open + 1..close.min(ch.len())).any(|i| ch.kind(i) == COMMA && ch.gaps[i].newline)
}

/// Whether the first item shares the opening delimiter's line.
fn detect_hug(ch: &Children, open: usize) -> bool {
    ch.gaps.get(open + 1).is_some_and(|g| !g.newline)
}

/// `module a.b { X, Y }` — 129 leading-comma lists to 1 trailing, the brace
/// hugging its first export in 201 of 206 (`docs/spec/style.md` §3.1).
fn module_export_cfg() -> ListCfg {
    ListCfg {
        open_flat: Sep::Space,
        open_breaks: true,
        pad_flat: true,
        hug: true,
        leading: true,
        indent: true,
        never_break: false,
        preserve: false,
    }
}

/// Parameter and argument lists: 89 leading : 4 trailing, and when they break
/// the `(` moves to its own line in 113 of 125 (`docs/spec/style.md` §3.3).
fn paren_list_cfg() -> ListCfg {
    ListCfg {
        open_flat: Sep::Tight,
        open_breaks: true,
        pad_flat: false,
        hug: true,
        leading: true,
        indent: true,
        never_break: false,
        preserve: false,
    }
}

/// The `(A, B)` of a function *type*.
///
/// Unlike a declaration's parameter list it keeps its `(` where it is. §3.3's
/// geometry — the opener dropping to its own line at +4 — is about the 125
/// declarations that break, all of which have a name in front of the paren. A
/// function type is often the first thing in its declaration, so moving its
/// opener down would leave the line above holding nothing but an attribute, and
/// the next run would read that break as the author's.
fn fn_type_list_cfg() -> ListCfg {
    ListCfg { open_breaks: false, ..paren_list_cfg() }
}

/// `<…>` and `[…]`, whose opener never leaves the line it is written on.
fn tight_list_cfg() -> ListCfg {
    ListCfg { never_break: true, ..paren_list_cfg() }
}

/// A comment, as a document.
///
/// A line comment is one `Doc::text`. A block comment that spans lines is split
/// so the renderer can put the current indent in front of each continuation,
/// with the comment's own relative structure — a code sample, a numbered list —
/// preserved by subtracting its original common indent first. Interior text is
/// never reflowed: `docs/spec/style.md` §8.7 and §5.4 both forbid it, and
/// example 9 shows a 179-column `//<` that must survive intact.
fn comment_doc(c: &Comment) -> Doc {
    let text = c.text.replace('\r', "");
    if !text.contains('\n') {
        return Doc::text(text.trim_end());
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let base = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut parts = Vec::with_capacity(lines.len() * 2);
    parts.push(Doc::text(lines[0].trim_end()));
    for line in &lines[1..] {
        parts.push(Doc::hard_line());
        let body = if line.len() >= base { &line[base..] } else { line.trim_start() };
        let body = body.trim_end();
        if !body.is_empty() {
            parts.push(Doc::text(body));
        }
    }
    Doc::concat(parts)
}

/// Token text as a document. Tokens never contain a newline; the split is a
/// guard so a malformed tree degrades to bad layout rather than a panic.
fn text_doc(s: &str) -> Doc {
    if !s.contains('\n') {
        return Doc::text(s);
    }
    Doc::join(Doc::hard_line(), s.split('\n').map(|l| Doc::text(l.trim_end())))
}
