//! One position index: a source offset → the innermost `Own` IR node whose
//! text holds it, then outward through the nodes that enclose it
//! (`docs/plans/ir-elaboration.md` §2.6; sprint W3.5).
//!
//! The loop keys its types by node (`kg_sem::TypeMap`), so the editor's
//! type-reading features — hover, completion, inlay hints, signature help,
//! navigation, the loop alone since sprint W3.7 — need a bridge from where
//! the caret is to which node the loop typed. This is that bridge, and the
//! only one: a second copy in the language server would be free to disagree
//! with the resilience gate that records what it answers (`xtask
//! resilience`). The CST inference engine keyed its types by position and
//! needed none; it is deleted (sprint W3.7).
//!
//! # Which nodes
//!
//! Only a node that may answer a position question: `Written::Own`, at
//! `Expansion::ROOT`, in this module's own file, and reachable from the root.
//!
//! * `Donated` wears a donor's range (`Src::donated`, `newNote`,
//!   `Type.hs:983-984`) — answering from it answers about the donor;
//! * `Reread` is the second reading of text an `Own` node already holds
//!   (`kg_base::Written::Reread`), at the same range and kind — the `Own`
//!   one answers;
//! * a node inside an instantiated copy (`via != ROOT`) is the template's
//!   body seen through one instance; the text at that offset is the written
//!   body, which the ROOT tree holds — and which no engine types, since
//!   `inferType` visits a template for its condition only. What the copies
//!   say about that text is [`CopyIndex`]'s to answer, one node per copy;
//! * an unreachable node is one a rewrite replaced (an append-only arena
//!   keeps it, `crate::arena`), and its type is the one the engine stopped
//!   updating.
//!
//! One `Donated` node is indexed after all: a **rebuild**. A rewrite that
//! changes what a declaration holds mints the declaration again round its
//! new children, donating it the old one's `Src` — `moveFunctions` lifting
//! a class out of `main`'s body (`Function.hs:88-119`), `desugarLambdas`
//! rebuilding a method round its body (`Function.hs:206-214`) — and the
//! `Own` original is then unreachable. Where a reachable `Donated` node has
//! the range, the kind and the constructor of an `Own` node the arena
//! holds, and no reachable `Own` node claims that range and kind, it is
//! that declaration rebuilt, its text the same, and it answers for it.
//! Without this, hover on a function holding a local class answered
//! nothing (`test/syntax/export-class.k`'s `main`, sprint W3.7).
//!
//! # Why ranges alone order the nodes
//!
//! Every `Own` head is a CST node's text range (`lower.rs`'s `own_src`, the
//! one place `Src::own` is called outside tests), and CST ranges nest or are
//! disjoint — so the indexed ranges form a laminar family whatever the IR's
//! own shape, which the rewrites change (a moved function, a desugared
//! lambda) without changing a node's `Src`. "Innermost" is therefore the
//! smallest range holding the offset; between equal ranges — a `NamedValue`
//! over the `NAME_REF` it was lowered with, both at `x` — the node deeper in
//! the tree. Tree depth only breaks that tie.

use kg_base::{Expansion, TextRange, TextSize, Written};
use kg_syntax::SyntaxKind;

use crate::{ExpId, ModuleIr};

/// See the module docs.
#[derive(Debug, Default)]
pub struct PositionIndex {
    /// Sorted by start ascending, then end descending, then depth ascending:
    /// an entry comes after every entry that encloses it.
    entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    range: TextRange,
    kind: SyntaxKind,
    depth: u32,
    id: ExpId,
    /// The nearest entry enclosing this one's range (an index into
    /// `entries`), `None` at the outermost.
    parent: Option<u32>,
}

impl PositionIndex {
    /// The index of `ir`'s tree as it stands — the lowering, or the loop's
    /// final tree, whichever the caller holds.
    pub fn build(ir: &ModuleIr) -> PositionIndex {
        let mut entries = Vec::new();
        let mut donated = Vec::new();
        let mut stack = vec![(ir.root, 0u32)];
        let mut seen = kg_base::FxHashSet::default();
        let mut kids = Vec::new();
        while let Some((id, depth)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &ir.arena[id];
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().map(|&k| (k, depth + 1)));
            let src = node.src;
            if src.via != Expansion::ROOT || src.head.file != ir.file {
                continue;
            }
            let entry = Entry { range: src.head.range, kind: node.kind, depth, id, parent: None };
            match src.written {
                Written::Own => entries.push(entry),
                Written::Donated if declares(&node.exp) => donated.push(entry),
                _ => {}
            }
        }
        // A rebuilt declaration (the module docs): a reachable `Donated`
        // node standing where an `Own` node of its constructor was lowered,
        // whose range and kind no reachable `Own` node claims.
        if !donated.is_empty() {
            let claimed: kg_base::FxHashSet<(TextRange, SyntaxKind)> = entries.iter().map(|e| (e.range, e.kind)).collect();
            let lowered: kg_base::FxHashSet<(TextRange, SyntaxKind, std::mem::Discriminant<crate::Exp>)> = ir
                .arena
                .iter()
                .filter(|(_, n)| n.src.written == Written::Own && n.src.via == Expansion::ROOT && n.src.head.file == ir.file)
                .map(|(_, n)| (n.src.head.range, n.kind, std::mem::discriminant(&n.exp)))
                .collect();
            for e in donated {
                let shape = std::mem::discriminant(&ir.arena[e.id].exp);
                if !claimed.contains(&(e.range, e.kind)) && lowered.contains(&(e.range, e.kind, shape)) {
                    entries.push(e);
                }
            }
        }
        entries.sort_by(|a, b| {
            (a.range.start(), std::cmp::Reverse(a.range.end()), a.depth, a.id.index())
                .cmp(&(b.range.start(), std::cmp::Reverse(b.range.end()), b.depth, b.id.index()))
        });
        let mut open: Vec<u32> = Vec::new();
        for i in 0..entries.len() {
            let range = entries[i].range;
            while let Some(&top) = open.last() {
                if entries[top as usize].range.contains_range(range) {
                    break;
                }
                open.pop();
            }
            entries[i].parent = open.last().copied();
            open.push(i as u32);
        }
        PositionIndex { entries }
    }

    /// How many nodes the index holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The innermost indexed node whose range holds `offset` — half-open,
    /// `start <= offset < end`, as `TextRange::contains` reads it, so a caret
    /// just past a word is not on it — or `None` when no node's text does.
    pub fn at(&self, offset: TextSize) -> Option<ExpId> {
        self.innermost(offset).map(|i| self.entries[i].id)
    }

    /// Every indexed node whose range holds `offset`, innermost first.
    pub fn enclosing(&self, offset: TextSize) -> impl Iterator<Item = ExpId> + '_ {
        let mut next = self.innermost(offset);
        std::iter::from_fn(move || {
            let i = next?;
            next = self.entries[i].parent.map(|p| p as usize);
            Some(self.entries[i].id)
        })
    }

    /// The indexed node lowered from the CST node at exactly `range` with
    /// `kind` — a `TypeKey`'s `(range, kind)` at ROOT — innermost first
    /// where two share both.
    pub fn node(&self, range: TextRange, kind: SyntaxKind) -> Option<ExpId> {
        let from = self.entries.partition_point(|e| e.range.start() < range.start());
        self.entries[from..]
            .iter()
            .take_while(|e| e.range.start() == range.start())
            .filter(|e| e.range == range && e.kind == kind)
            .last()
            .map(|e| e.id)
    }

    /// The last entry starting at or before `offset`, then outward to the
    /// first that holds it. Every entry that starts inside the innermost
    /// holder and at or before `offset` nests inside it (the family is
    /// laminar), so the holder is an ancestor of that last entry and the
    /// first holder on the way up is the innermost.
    fn innermost(&self, offset: TextSize) -> Option<usize> {
        let last = self.entries.partition_point(|e| e.range.start() <= offset).checked_sub(1)?;
        let mut i = Some(last);
        while let Some(at) = i {
            if self.entries[at].range.contains(offset) {
                return Some(at);
            }
            i = self.entries[at].parent.map(|p| p as usize);
        }
        None
    }
}

/// Whether `exp` is a declaration a rewrite may rebuild — what the module
/// docs index a `Donated` node for; anything else a rebuild leaves `Donated`
/// at a declaration's range is a part of it (an attribute list's `Seq`
/// wears the function's), which no question asks for.
fn declares(exp: &crate::Exp) -> bool {
    use crate::Exp;
    matches!(
        exp,
        Exp::Function { .. }
            | Exp::FunctionDecl { .. }
            | Exp::Class { .. }
            | Exp::Struct { .. }
            | Exp::Union { .. }
            | Exp::Enum { .. }
            | Exp::Alias { .. }
            | Exp::Variable { .. }
            | Exp::FuncParam { .. }
            | Exp::Template { .. }
    )
}

/// The copies' side of the position question: for one CST node of the
/// module's text, the node each instantiated copy made of it.
///
/// A copy's node keeps the body's head and its `Own`ness and carries the
/// instantiation in its `via` (`Src::expanded`, `desugarWithNote n`,
/// `Template.hs:127`), and copies are made in the module that declares the
/// template (`kg_sem::elaborate`), so a template body's text in this module
/// is answered by this module's arena. Indexed: `Own`, `via != ROOT`, in
/// this module's file, reachable — the same filters as [`PositionIndex`]
/// but the one on `via`, for the same reasons.
#[derive(Debug, Default)]
pub struct CopyIndex {
    /// Sorted by range, kind, `via`, then depth descending.
    entries: Vec<CopyEntry>,
}

#[derive(Clone, Copy, Debug)]
struct CopyEntry {
    range: TextRange,
    kind: SyntaxKind,
    via: Expansion,
    depth: u32,
    id: ExpId,
}

impl CopyIndex {
    pub fn build(ir: &ModuleIr) -> CopyIndex {
        let mut entries = Vec::new();
        let mut stack = vec![(ir.root, 0u32)];
        let mut seen = kg_base::FxHashSet::default();
        let mut kids = Vec::new();
        while let Some((id, depth)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let node = &ir.arena[id];
            kids.clear();
            node.exp.children(&mut kids);
            stack.extend(kids.iter().map(|&k| (k, depth + 1)));
            let src = node.src;
            if src.written != Written::Own || src.via == Expansion::ROOT || src.head.file != ir.file {
                continue;
            }
            entries.push(CopyEntry { range: src.head.range, kind: node.kind, via: src.via, depth, id });
        }
        entries.sort_by(|a, b| {
            (a.range.start(), a.range.end(), a.kind, a.via, std::cmp::Reverse(a.depth), a.id.index())
                .cmp(&(b.range.start(), b.range.end(), b.kind, b.via, std::cmp::Reverse(b.depth), b.id.index()))
        });
        CopyIndex { entries }
    }

    /// How many copied nodes the index holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Each copy's node lowered from the CST node at exactly `range` with
    /// `kind`, in the order the copies' expansions were interned — the
    /// order the loop decided them — and, where one copy holds two such
    /// nodes, the deeper, as [`PositionIndex::node`] takes it.
    pub fn nodes(&self, range: TextRange, kind: SyntaxKind) -> Vec<(Expansion, ExpId)> {
        let from = self.entries.partition_point(|e| (e.range.start(), e.range.end()) < (range.start(), range.end()));
        let mut out: Vec<(Expansion, ExpId)> = Vec::new();
        for e in self.entries[from..].iter().take_while(|e| e.range == range) {
            if e.kind == kind && out.last().is_none_or(|&(via, _)| via != e.via) {
                out.push((e.via, e.id));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Exp;
    use kg_base::{FileId, Interner};

    fn lowered(text: &str) -> (ModuleIr, Interner) {
        let interner = Interner::default();
        let parse = kg_syntax::parse_file(text);
        // Sorted, as `kg_db` lowers a program: the unsorted lowering keeps a
        // path as one `Path` node, with nothing donated inside it.
        let ir = crate::lower_sorted(&parse.source_file(), FileId(0), &interner, &crate::NoImports);
        (ir, interner)
    }

    fn offset_of(text: &str, needle: &str) -> TextSize {
        TextSize::from(text.find(needle).unwrap_or_else(|| panic!("`{needle}` in the text")) as u32)
    }

    /// What a node is, for assertions: its CST kind and its text.
    fn shown(ir: &ModuleIr, text: &str, id: ExpId) -> (SyntaxKind, String) {
        let node = &ir.arena[id];
        (node.kind, text[node.src.head.range].to_owned())
    }

    const SRC: &str = "inline uint32 f(uint32 a)\n{\n    uint32 y = a + 1;\n    return y;\n}\n";

    /// The name `a` in `a + 1` is held by the `BIN_EXPR`, the `PATH` it
    /// lowers to a `NamedValue` from and the `NAME_REF` under that — the
    /// last two at the same range. The smallest range wins, and between the
    /// two equal ones the deeper. Red with the sort's end or depth order
    /// flipped: the `BIN_EXPR` or the `PATH` answers.
    #[test]
    fn the_innermost_node_wins_and_the_deeper_of_two_equal_ranges() {
        let (ir, _) = lowered(SRC);
        let index = PositionIndex::build(&ir);
        let at = index.at(offset_of(SRC, "a + 1")).expect("a node holds `a`");
        assert_eq!(shown(&ir, SRC, at), (SyntaxKind::NAME_REF, "a".to_owned()));
        let outward: Vec<(SyntaxKind, String)> =
            index.enclosing(offset_of(SRC, "a + 1")).map(|id| shown(&ir, SRC, id)).collect();
        assert_eq!(
            outward.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            [
                SyntaxKind::NAME_REF,
                SyntaxKind::PATH,
                SyntaxKind::BIN_EXPR,
                SyntaxKind::VAR_DECL,
                SyntaxKind::BLOCK_STMT,
                SyntaxKind::FN_DECL,
                SyntaxKind::SOURCE_FILE,
            ]
        );
    }

    /// `a` lowers to `NamedValue(QualifiedIdentifier(ScopedName(ScopedName
    /// (…, f), a)))`: the scope prefix `f` is a chain of nodes *donated* the
    /// `a`'s range, deeper in the tree than the `NAME_REF` that owns it.
    /// Indexing them answers `Identifier f` at `a` — the donor's text read
    /// as the donee's. Red with the `Written::Own` filter dropped.
    #[test]
    fn a_donated_node_is_never_the_answer() {
        let (ir, interner) = lowered(SRC);
        let index = PositionIndex::build(&ir);
        let at = index.at(offset_of(SRC, "a + 1")).expect("a node holds `a`");
        let Exp::Identifier(name) = &ir.arena[at].exp else {
            panic!("expected the identifier, got {:?}", ir.arena[at].exp)
        };
        assert_eq!(&*interner.resolve(*name), "a");
        // The deeper donated `f` is there to be found.
        assert!(
            ir.arena.iter().any(|(_, n)| n.src.written == Written::Donated
                && n.src.head.range == ir.arena[at].src.head.range
                && matches!(n.exp, Exp::Identifier(s) if &*interner.resolve(s) == "f")),
            "the lowering no longer donates `a`'s range to the scope prefix — this test needs a new donated node"
        );
    }

    /// The spaces before `return` belong to no statement: the block holds
    /// them. Red with the parent walk dropped (the last entry starting
    /// before them is the first statement's `1`, which does not).
    #[test]
    fn an_offset_between_nodes_finds_the_node_enclosing_both() {
        let (ir, _) = lowered(SRC);
        let index = PositionIndex::build(&ir);
        let gap = offset_of(SRC, "    return") + TextSize::from(1);
        let at = index.at(gap).expect("the block holds the gap");
        assert_eq!(shown(&ir, SRC, at).0, SyntaxKind::BLOCK_STMT);
        // And past the last byte of the file, nothing.
        assert_eq!(index.at(TextSize::from(SRC.len() as u32)), None);
    }

    /// A caret just past a word is not on it: ranges are half-open, and
    /// the offset after `y` is the `;`'s, which the statement holds.
    #[test]
    fn a_range_is_half_open() {
        let (ir, _) = lowered(SRC);
        let index = PositionIndex::build(&ir);
        let after = offset_of(SRC, "y;") + TextSize::from(1);
        let at = index.at(after).expect("the return statement holds its `;`");
        assert_eq!(shown(&ir, SRC, at), (SyntaxKind::RETURN_STMT, "return y;".to_owned()));
    }

    /// A node the tree no longer reaches is not indexed: a rewrite's
    /// replacement appends a fresh node with the same `Src` and the old one
    /// stays in the arena. Red with the walk replaced by `arena.iter()`:
    /// both are found and the orphan, appended last, answers.
    #[test]
    fn an_unreachable_node_is_not_indexed() {
        let (mut ir, _) = lowered(SRC);
        let before = PositionIndex::build(&ir);
        let y = before.at(offset_of(SRC, "y;")).expect("`y` is indexed");
        let data = ir.arena[y].clone();
        let orphan = ir.arena.alloc(Exp::Barrier, data.src, data.kind);
        let after = PositionIndex::build(&ir);
        assert_eq!(after.len(), before.len());
        assert_eq!(after.at(offset_of(SRC, "y;")), Some(y));
        assert_ne!(after.at(offset_of(SRC, "y;")), Some(orphan));
    }

    /// A node of an instantiated copy keeps its own head and `Own`
    /// (`Src::expanded`) but is the body seen through one instance; the
    /// written text answers from the ROOT tree. Here the `y` of `return y`
    /// is stamped as a copy's, so its `PATH` answers. Red with the `via`
    /// filter dropped.
    #[test]
    fn a_node_inside_an_instantiated_copy_is_not_indexed() {
        let (mut ir, _) = lowered(SRC);
        let y = PositionIndex::build(&ir).at(offset_of(SRC, "y;")).expect("`y` is indexed");
        let expansions = kg_base::ExpansionInterner::new();
        let site = ir.arena[y].src.head;
        let copied = kg_base::Src::expanded(ir.arena[y].src, site, kg_base::InstanceId(7), 1, &expansions);
        assert_eq!(copied.written, Written::Own);
        ir.arena.node_mut(y).src = copied;
        let at = PositionIndex::build(&ir).at(offset_of(SRC, "y;")).expect("the path holds `y`");
        assert_eq!(shown(&ir, SRC, at), (SyntaxKind::PATH, "y".to_owned()));
    }

    /// Two copies of the declaration of `y`: each answers for the text it
    /// was copied from, in the order their expansions were interned (not
    /// the order the walk meets them), the deeper of two nodes one copy
    /// holds there, and never the written declaration. Red with the `via`
    /// filter dropped (the ROOT declaration answers), with the per-copy
    /// dedupe dropped (the wrapper answers too), or with the depth order
    /// flipped (the wrapper answers instead).
    #[test]
    fn each_copy_answers_for_the_text_it_was_copied_from() {
        let (mut ir, _) = lowered(SRC);
        let range = TextRange::new(offset_of(SRC, "uint32 y"), offset_of(SRC, "\n    return"));
        let decl = PositionIndex::build(&ir).node(range, SyntaxKind::VAR_DECL).expect("the declaration");
        assert!(CopyIndex::build(&ir).is_empty(), "a lowering has no copies");

        let data = ir.arena[decl].clone();
        let expansions = kg_base::ExpansionInterner::new();
        let first = kg_base::Src::expanded(data.src, data.src.head, kg_base::InstanceId(9), 1, &expansions);
        let second = kg_base::Src::expanded(data.src, data.src.head, kg_base::InstanceId(3), 1, &expansions);
        assert!(first.via < second.via);
        let in_second = ir.arena.alloc(data.exp.clone(), second, data.kind);
        let deep = ir.arena.alloc(data.exp.clone(), first, data.kind);
        let wrapper = ir.arena.alloc(Exp::Seq(vec![deep]), first, data.kind);
        let root = ir.arena.alloc(Exp::Seq(vec![ir.root, in_second, wrapper]), ir.arena[ir.root].src, SyntaxKind::SOURCE_FILE);
        ir.root = root;

        let copies = CopyIndex::build(&ir);
        assert_eq!(copies.nodes(range, SyntaxKind::VAR_DECL), [(first.via, deep), (second.via, in_second)]);
        assert_eq!(copies.nodes(range, SyntaxKind::BLOCK_STMT), []);
        // The written index is unmoved by them.
        assert_eq!(PositionIndex::build(&ir).node(range, SyntaxKind::VAR_DECL), Some(decl));
    }

    /// `node` is the `TypeKey` lookup: the declaration of `y` by its range
    /// and kind, which `at` would never return — its name is smaller.
    #[test]
    fn a_node_is_found_by_its_range_and_kind() {
        let (ir, _) = lowered(SRC);
        let index = PositionIndex::build(&ir);
        let start = offset_of(SRC, "uint32 y");
        let end = offset_of(SRC, "\n    return");
        let decl = index.node(TextRange::new(start, end), SyntaxKind::VAR_DECL).expect("the declaration");
        assert_eq!(shown(&ir, SRC, decl), (SyntaxKind::VAR_DECL, "uint32 y = a + 1;".to_owned()));
        assert_eq!(index.node(TextRange::new(start, end), SyntaxKind::BLOCK_STMT), None);
        assert!(index.enclosing(offset_of(SRC, "y =")).any(|id| id == decl));
    }
}
