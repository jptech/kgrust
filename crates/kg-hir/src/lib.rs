//! The lowered IR pass 8 instantiates: the representation, its lowering, the
//! rewrites, materialisation, and a printer. The program-wide elaboration
//! (`kg_sem::elaborate`, which `kg-db` memoizes) runs every program through
//! it, and `kg-ptbridge` walks what it leaves into the backend.
//!
//! The reference materialises an instantiated copy of each template body and
//! re-runs inference over it (`Template.hs:122-139`); five of its sub-passes
//! *add declarations nobody wrote* (`Function.hs`, `Closure.hs`), which a
//! lossless, never-rewritten CST cannot carry. This crate is the second
//! representation those rewrites run on — see `docs/journal.md`,
//! "Provenance is a copy, not a mapping", which fixed the design, and
//! `docs/history/roadmap-history.md` §1d for the decision between this and virtual
//! instantiation.
//!
//! | module | owns |
//! |---|---|
//! | [`exp`] | [`Exp`], mirroring `ExpF` (`Parser/Syntax.hs:282-584`) |
//! | [`arena`] | [`ExpArena`], [`ExpId`], the stamp check |
//! | [`lower`](mod@lower) | the CST walk, `Parser.hs`'s desugarings, scope names |
//! | [`sort`] | the parse-time symbol oracle path sorting consults |
//! | [`rewrite`] | pass 8's rewriting substrate and the sub-passes ported onto it |
//! | [`facts`] | what a subtree holds, so a rewrite skips subtrees it cannot change |
//! | [`constexpr`], [`width`] | `evaluateConstexpr` over the IR and over a width |
//! | [`order`] | the reference's derived `Ord` on expressions |
//! | [`pretty`](mod@pretty) | IR → parseable source, for the round-trip gate |
//! | [`structure`] | the canonical structural dump two IRs are compared by |
//! | [`position`] | the position index: an offset → the innermost `Own` node, for the editor |
//!
//! The human door is `kg dump ir`; the gates are `cargo xtask ir` (the IR
//! against the reference's tree) and `cargo xtask ptdiff` (the walk of it
//! against the reference's).

pub mod arena;
pub mod constexpr;
pub mod exp;
pub mod facts;
pub mod lower;
pub mod order;
pub mod position;
pub mod pretty;
pub mod rewrite;
pub mod sort;
pub mod structure;

use kg_base::{FileId, FileRange, Interner, SmolStr, Symbol};
use kg_syntax::SyntaxKind;

pub use arena::{ExpArena, ExpId, NodeData};

/// `typeOf` (`Type.hs:452`) at an IR node, whoever keeps the notes — the
/// IR inference's node-keyed map (`kg_sem::TypeMap`), which types every
/// node. A rewrite that reads types takes this. The CST inference's
/// position-keyed table, which keyed only a node whose head is its own text
/// (divergence #21), implemented it too until sprint W3.7 deleted it.
pub trait NodeTypes: std::fmt::Debug {
    fn type_at(&self, arena: &ExpArena, id: ExpId) -> kg_ty::Ty;
}

pub mod width;
pub use exp::{plain_string_value, string_token, ErrorNote, Exp, Hole, Modifier, PathSeg, Refused, Segment, UnsortedPath};
pub use lower::{lower, lower_sorted};
pub use pretty::{pretty, pretty_with_source, pretty_with_spellings};
pub use sort::{
    ExternalHit, FnInit, ImportSymbols, NoImports, SortStats, SymShape, TypeName,
    UnsortedReason,
};

/// One file's lowered body: the arena, its root `Seq`, and the imports.
///
/// Imports are metadata, not nodes: the reference's parser consumes an
/// `import` into the symbol table and builds **no** AST node for it
/// (`parseImports`, `Parser.hs:194-207`). They ride here so the printer can
/// re-emit them and the structural comparison can see them.
///
/// The reference also seeds the program `Seq` with one synthetic empty `Seq`
/// carrying any redundant-import warnings (`checkRedundantImports`,
/// `Parser.hs:234-241`). That seed exists only to carry parser warnings, and
/// the warning is decided off the module graph instead (`kg_db`'s `imports`
/// module), so it is deliberately not reproduced; `xtask ir` drops the
/// reference's seed before comparing.
#[derive(Debug)]
pub struct ModuleIr {
    pub file: FileId,
    pub arena: ExpArena,
    /// The program `Seq` (`Parser.hs:111-117`): the declarations, or one
    /// `ModuleDecl` wrapping them when the file has a header.
    pub root: ExpId,
    pub imports: Vec<Import>,
    /// The path sorter's scoreboard — zeros when the lowering ran without a
    /// symbol context (`lower` rather than `lower_sorted`).
    pub sort: sort::SortStats,
    /// The arena length when the lowering finished, and therefore the exact
    /// boundary between "this node is the lowering of some CST node" and "a
    /// rewrite made this".
    ///
    /// There is one place a `ModuleIr` is built (`lower.rs:161`) and nothing
    /// appends to the arena between the lowering and that line — the sorter's
    /// `setQualifier` graft runs *inside* it (`lower.rs:2606`) — so the
    /// boundary is exact rather than approximate. `xtask ir`'s provenance gate
    /// uses it to forbid a rewrite from allocating a fresh `Own` node
    /// indistinguishable from the lowered node it copied.
    pub lowered_len: u32,
}

impl ModuleIr {
    /// The declarations at file scope: the root `Seq`'s items, reaching
    /// through the `ModuleDecl` wrapper a headered file lowers to.
    pub fn top_level_items(&self) -> Vec<ExpId> {
        let Exp::Seq(items) = &self.arena[self.root].exp else { return Vec::new() };
        if let [only] = items.as_slice()
            && let Exp::ModuleDecl { body, .. } = &self.arena[*only].exp
            && let Exp::Seq(inner) = &self.arena[*body].exp
        {
            return inner.clone();
        }
        items.clone()
    }

    /// The module's dotted name (`a.b`), `None` for a file with no header.
    pub fn dotted_module_name(&self) -> Option<String> {
        let Exp::Seq(items) = &self.arena[self.root].exp else { return None };
        let [only] = items.as_slice() else { return None };
        let Exp::ModuleDecl { name, .. } = &self.arena[*only].exp else { return None };
        let Exp::ModuleIdentifier(components) = &self.arena[*name].exp else { return None };
        if components.is_empty() {
            return None;
        }
        Some(components.iter().map(|c| c.as_str()).collect::<Vec<_>>().join("."))
    }
}

/// One `import a.b.c as d` (`Parser.hs:194-197`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Import {
    pub name: Vec<SmolStr>,
    pub alias: Option<Symbol>,
    pub range: FileRange,
}

impl ModuleIr {
    /// A copy to rewrite into — see [`ExpArena::fork`] for why this is not
    /// `Clone`. `lowered_len` travels with it: a fork of a rewritten IR still
    /// knows which nodes the *lowering* made.
    pub fn fork(&self) -> ModuleIr {
        ModuleIr {
            file: self.file,
            arena: self.arena.fork(),
            root: self.root,
            imports: self.imports.clone(),
            sort: self.sort.clone(),
            lowered_len: self.lowered_len,
        }
    }

    /// Every [`Exp::Unsupported`] node, with the CST kind it stood in for —
    /// the totality gate's report.
    pub fn unsupported(&self) -> Vec<(SyntaxKind, kg_base::Src)> {
        self.arena
            .iter()
            .filter(|(_, n)| matches!(n.exp, Exp::Unsupported))
            .map(|(_, n)| (n.kind, n.src))
            .collect()
    }

    /// The canonical structural rendering: everything that defines the IR's
    /// shape — variants, scalar payloads, child order, imports — and nothing
    /// that does not (`Src` ranges, CST kinds, arena indices). Two lowerings
    /// are structurally equal iff these strings are equal; the round-trip gate
    /// compares them. Symbols resolve through `interner`, which must be the
    /// one `lower` ran with.
    pub fn structure_string(&self, interner: &Interner) -> String {
        structure::render(self, interner, structure::Detail::Structure)
    }

    /// [`ModuleIr::structure_string`] plus per-node provenance — kind, range,
    /// `own`/`donated` — the `kg dump ir` rendering.
    pub fn annotated_string(&self, interner: &Interner) -> String {
        structure::render(self, interner, structure::Detail::Annotated)
    }
}
