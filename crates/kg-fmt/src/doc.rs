//! A Wadler-style document algebra.
//!
//! The formatter's job splits cleanly in two: turn a syntax tree into a `Doc`
//! that describes *where breaks are allowed*, then let the renderer decide
//! which of those breaks to take. Only the second half knows about line width,
//! which is why the tree-lowering code reads like a description of the language
//! rather than like a pile of column arithmetic.
//!
//! Based on Wadler's "A prettier printer", with Prettier's practical additions:
//! a `Group` that breaks all-or-nothing, and a hard line that propagates
//! breakage outward through every enclosing group. The second is what makes
//! comments work — a line comment forces its group to break, because putting
//! anything after `// ...` on the same line would swallow it.

use std::rc::Rc;

use smol_str::SmolStr;

/// A formatting document.
///
/// Cheap to clone: the payload is behind an `Rc`. Each document caches whether
/// it contains an unflattenable break, computed once at construction from its
/// children, so the renderer's group decision is O(1) rather than a traversal —
/// without that, rendering a deeply nested expression is quadratic.
#[derive(Debug, Clone)]
pub struct Doc {
    kind: Rc<DocKind>,
    must_break: bool,
}

#[derive(Debug)]
enum DocKind {
    /// Nothing.
    Nil,
    /// Literal text. Must not contain a newline; use [`Doc::hard_line`].
    Text(SmolStr),
    /// A space when flat, a newline when broken.
    Line,
    /// Nothing when flat, a newline when broken.
    SoftLine,
    /// Always a newline, and forces every enclosing group to break.
    HardLine,
    /// A blank line the author wrote, preserved. Collapses runs to one.
    BlankLine,
    Concat(Box<[Doc]>),
    /// Increases the indent of any newline inside by `n`.
    Nest(isize, Doc),
    /// Flat if it fits on the remaining width, broken otherwise.
    Group(Doc),
    /// Renders `flat` when the enclosing group is flat and `broken` otherwise.
    ///
    /// This is how a trailing separator appears only in the broken layout —
    /// or, for Kanagawa, how a *leading* separator does.
    IfBroken { flat: Doc, broken: Doc },
}

impl Doc {
    fn new(kind: DocKind, must_break: bool) -> Doc {
        Doc { kind: Rc::new(kind), must_break }
    }

    pub fn text(s: impl Into<SmolStr>) -> Doc {
        let s = s.into();
        debug_assert!(!s.contains('\n'), "Doc::text must not contain a newline: {s:?}");
        Doc::new(DocKind::Text(s), false)
    }

    pub fn nil() -> Doc {
        Doc::new(DocKind::Nil, false)
    }

    pub fn line() -> Doc {
        Doc::new(DocKind::Line, false)
    }

    pub fn soft_line() -> Doc {
        Doc::new(DocKind::SoftLine, false)
    }

    pub fn hard_line() -> Doc {
        Doc::new(DocKind::HardLine, true)
    }

    pub fn blank_line() -> Doc {
        Doc::new(DocKind::BlankLine, true)
    }

    pub fn concat(docs: impl IntoIterator<Item = Doc>) -> Doc {
        let docs: Vec<Doc> =
            docs.into_iter().filter(|d| !matches!(*d.kind, DocKind::Nil)).collect();
        match docs.len() {
            0 => Doc::nil(),
            1 => docs.into_iter().next().unwrap(),
            _ => {
                let must_break = docs.iter().any(|d| d.must_break);
                Doc::new(DocKind::Concat(docs.into()), must_break)
            }
        }
    }

    pub fn group(self) -> Doc {
        // A group whose contents must break, breaks; the flag propagates so an
        // enclosing group sees it too.
        let must_break = self.must_break;
        Doc::new(DocKind::Group(self), must_break)
    }

    pub fn nest(self, n: isize) -> Doc {
        let must_break = self.must_break;
        Doc::new(DocKind::Nest(n, self), must_break)
    }

    pub fn if_broken(flat: Doc, broken: Doc) -> Doc {
        // Only the branch that would actually render can force a break, and in
        // flat mode that is `flat`.
        let must_break = flat.must_break;
        Doc::new(DocKind::IfBroken { flat, broken }, must_break)
    }

    /// Joins `docs` with `sep`.
    pub fn join(sep: Doc, docs: impl IntoIterator<Item = Doc>) -> Doc {
        let mut out = Vec::new();
        for (i, doc) in docs.into_iter().enumerate() {
            if i > 0 {
                out.push(sep.clone());
            }
            out.push(doc);
        }
        Doc::concat(out)
    }

    /// True when this document contains a break that cannot be flattened, so a
    /// group containing it must render broken however wide the page is.
    pub fn must_break(&self) -> bool {
        self.must_break
    }
}

thread_local! {
    /// One shared `Nil` to hand out while dismantling.
    ///
    /// [`Doc::drop`] has to take each child's `Rc` out of the parent to push it
    /// on its work stack, and `mem::replace` needs something to leave behind.
    /// The obvious `Rc::new(DocKind::Nil)` allocates — once per node — so
    /// tearing a document down cost exactly as many allocations as building it
    /// did. Cloning a shared sentinel is a refcount bump instead.
    ///
    /// Handing out a *shared* `Rc` is safe here precisely because the loop
    /// already handles that case: `Rc::into_inner` returns `None` for anything
    /// with another owner, and a `Nil` has no children to dismantle anyway.
    static NIL: Rc<DocKind> = Rc::new(DocKind::Nil);
}

#[inline]
fn nil() -> Rc<DocKind> {
    NIL.with(Rc::clone)
}

/// Dismantles a document iteratively.
///
/// The default recursive drop would use one stack frame per nesting level,
/// which a pathological input — or a fuzzer — can drive past the stack limit.
/// A formatter that crashes on weird input is worse than one that formats it
/// badly, so the teardown is explicit.
impl Drop for Doc {
    fn drop(&mut self) {
        let mut stack = vec![std::mem::replace(&mut self.kind, nil())];
        while let Some(rc) = stack.pop() {
            // Only the last owner dismantles; a shared subtree is left alone.
            let Some(kind) = Rc::into_inner(rc) else { continue };
            match kind {
                DocKind::Concat(docs) => {
                    for mut d in docs.into_vec() {
                        stack.push(std::mem::replace(&mut d.kind, nil()));
                    }
                }
                DocKind::Nest(_, mut d) | DocKind::Group(mut d) => {
                    stack.push(std::mem::replace(&mut d.kind, nil()));
                }
                DocKind::IfBroken { mut flat, mut broken } => {
                    stack.push(std::mem::replace(&mut flat.kind, nil()));
                    stack.push(std::mem::replace(&mut broken.kind, nil()));
                }
                DocKind::Nil
                | DocKind::Text(_)
                | DocKind::Line
                | DocKind::SoftLine
                | DocKind::HardLine
                | DocKind::BlankLine => {}
            }
        }
    }
}

/// Rendering options.
#[derive(Debug, Clone, Copy)]
pub struct RenderOpts {
    pub width: usize,
    pub indent: usize,
    pub use_tabs: bool,
    pub newline: &'static str,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { width: 100, indent: 4, use_tabs: false, newline: "\n" }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Mode {
    Flat,
    Break,
}

/// Renders `doc` to a string.
pub fn render(doc: &Doc, opts: RenderOpts) -> String {
    let mut out = String::new();
    // Work list of (indent, mode, doc). Explicit rather than recursive so a
    // deeply nested expression cannot blow the stack — corpus files nest more
    // than 60 deep in places.
    let mut work: Vec<(isize, Mode, &Doc)> = vec![(0, Mode::Break, doc)];
    // Column reached so far on the current line.
    let mut col = 0usize;
    // Set when the last thing emitted was a newline, so blank lines collapse.
    let mut pending_blank = false;

    while let Some((indent, mode, doc)) = work.pop() {
        match &*doc.kind {
            DocKind::Nil => {}
            DocKind::Text(s) => {
                out.push_str(s);
                col += s.chars().count();
                pending_blank = false;
            }
            DocKind::Concat(docs) => {
                for d in docs.iter().rev() {
                    work.push((indent, mode, d));
                }
            }
            DocKind::Nest(n, d) => work.push((indent + n, mode, d)),
            DocKind::Group(d) => {
                let fits_flat = !d.must_break && fits(opts.width.saturating_sub(col), d, &work);
                work.push((indent, if fits_flat { Mode::Flat } else { Mode::Break }, d));
            }
            DocKind::IfBroken { flat, broken } => {
                work.push((indent, mode, if mode == Mode::Flat { flat } else { broken }));
            }
            DocKind::Line if mode == Mode::Flat => {
                out.push(' ');
                col += 1;
            }
            DocKind::SoftLine if mode == Mode::Flat => {}
            DocKind::Line | DocKind::SoftLine | DocKind::HardLine => {
                new_line(&mut out, &mut col, indent, opts);
                pending_blank = false;
            }
            DocKind::BlankLine => {
                // Collapse a run of author blank lines to exactly one.
                if !pending_blank {
                    new_line(&mut out, &mut col, indent, opts);
                    // The blank itself: a newline with no indentation, so the
                    // file has no trailing whitespace.
                    out.truncate(out.trim_end_matches([' ', '\t']).len());
                    out.push_str(opts.newline);
                    write_indent(&mut out, indent, opts);
                    col = indent.max(0) as usize;
                    pending_blank = true;
                }
            }
        }
    }
    out
}

fn new_line(out: &mut String, col: &mut usize, indent: isize, opts: RenderOpts) {
    // Never leave trailing whitespace on a line.
    out.truncate(out.trim_end_matches([' ', '\t']).len());
    out.push_str(opts.newline);
    write_indent(out, indent, opts);
    *col = indent.max(0) as usize;
}

fn write_indent(out: &mut String, indent: isize, opts: RenderOpts) {
    let n = indent.max(0) as usize;
    if opts.use_tabs {
        let tabs = n / opts.indent;
        out.extend(std::iter::repeat_n('\t', tabs));
        out.extend(std::iter::repeat_n(' ', n % opts.indent));
    } else {
        out.extend(std::iter::repeat_n(' ', n));
    }
}

/// Whether `doc` — plus whatever already-queued work would land on the same
/// line — fits in `space` columns when rendered flat.
///
/// Looking at the rest of the work list is what stops a group from flattening
/// only to be pushed over the limit by the `);` that follows it.
fn fits(space: usize, doc: &Doc, rest: &[(isize, Mode, &Doc)]) -> bool {
    let mut budget = space as isize;
    let mut stack: Vec<(Mode, &Doc)> = vec![(Mode::Flat, doc)];
    let mut rest_iter = rest.iter().rev();

    loop {
        let Some((mode, doc)) = stack.pop().or_else(|| {
            // Continue into the enclosing work list; those items keep their own
            // mode, and a broken one ends the line, so we fit.
            rest_iter.next().map(|(_, mode, doc)| (*mode, *doc))
        }) else {
            return true;
        };

        match &*doc.kind {
            DocKind::Nil => {}
            DocKind::Text(s) => {
                budget -= s.chars().count() as isize;
                if budget < 0 {
                    return false;
                }
            }
            DocKind::Concat(docs) => {
                for d in docs.iter().rev() {
                    stack.push((mode, d));
                }
            }
            DocKind::Nest(_, d) => stack.push((mode, d)),
            DocKind::Group(d) => stack.push((Mode::Flat, d)),
            DocKind::IfBroken { flat, broken } => {
                stack.push((mode, if mode == Mode::Flat { flat } else { broken }));
            }
            DocKind::Line if mode == Mode::Flat => {
                budget -= 1;
                if budget < 0 {
                    return false;
                }
            }
            DocKind::SoftLine if mode == Mode::Flat => {}
            // A break in `Break` mode ends the line, so everything after it is
            // somebody else's problem.
            DocKind::Line | DocKind::SoftLine => return true,
            DocKind::HardLine | DocKind::BlankLine => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn narrow(width: usize) -> RenderOpts {
        RenderOpts { width, ..RenderOpts::default() }
    }

    #[test]
    fn a_group_that_fits_stays_flat() {
        let doc = Doc::concat([
            Doc::text("f("),
            Doc::concat([Doc::soft_line(), Doc::text("a,"), Doc::line(), Doc::text("b")])
                .nest(4)
                .group(),
            Doc::soft_line(),
            Doc::text(")"),
        ])
        .group();
        assert_eq!(render(&doc, narrow(80)), "f(a, b)");
    }

    #[test]
    fn a_group_that_does_not_fit_breaks() {
        let doc = Doc::concat([
            Doc::text("f("),
            Doc::concat([Doc::soft_line(), Doc::text("aaaa,"), Doc::line(), Doc::text("bbbb")])
                .nest(4),
            Doc::soft_line(),
            Doc::text(")"),
        ])
        .group();
        assert_eq!(render(&doc, narrow(10)), "f(\n    aaaa,\n    bbbb\n)");
    }

    #[test]
    fn a_hard_line_forces_its_enclosing_groups_open() {
        let doc = Doc::concat([Doc::text("a"), Doc::hard_line(), Doc::text("b")]).group();
        assert_eq!(render(&doc, narrow(100)), "a\nb");
    }

    #[test]
    fn the_leading_comma_house_style_renders_both_ways() {
        // Kanagawa's measured house style for a multi-line list: the opening
        // delimiter carries the first item, separators lead each continuation,
        // and the closing delimiter sits on its own line. See
        // `library/data/fifo.k:1-6` and `docs/spec/style-measured.md`.
        fn export_list(items: &[&str]) -> Doc {
            // A separator is `", "` flat and a newline plus `", "` broken, so
            // the comma leads rather than trails.
            let sep = Doc::concat([Doc::soft_line(), Doc::text(", ")]);
            Doc::concat([
                Doc::text("{ "),
                Doc::join(sep, items.iter().map(|i| Doc::text(*i))),
                Doc::line(),
                Doc::text("}"),
            ])
            .nest(4)
            .group()
        }

        assert_eq!(render(&export_list(&["FIFO", "fifo_nb"]), narrow(100)), "{ FIFO, fifo_nb }");
        assert_eq!(
            render(&export_list(&["FIFO", "FIFO_store_and_forward", "fifo_nb"]), narrow(30)),
            "{ FIFO\n    , FIFO_store_and_forward\n    , fifo_nb\n    }"
        );
    }

    #[test]
    fn no_line_is_left_with_trailing_whitespace() {
        let doc = Doc::concat([Doc::text("a"), Doc::line(), Doc::hard_line(), Doc::text("b")])
            .nest(4);
        let out = render(&doc, narrow(1));
        for line in out.lines() {
            assert_eq!(line.trim_end(), line, "trailing whitespace in {line:?}");
        }
    }

    #[test]
    fn blank_line_runs_collapse_to_one() {
        let doc = Doc::concat([
            Doc::text("a"),
            Doc::blank_line(),
            Doc::blank_line(),
            Doc::blank_line(),
            Doc::text("b"),
        ]);
        assert_eq!(render(&doc, narrow(100)), "a\n\nb");
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let mut doc = Doc::text("x");
        for _ in 0..50_000 {
            doc = Doc::concat([Doc::text("("), doc, Doc::text(")")]).group();
        }
        let out = render(&doc, narrow(100));
        assert!(out.ends_with(')'));
    }
}
