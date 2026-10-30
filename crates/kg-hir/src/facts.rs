//! What a subtree holds, kept per node of an arena while the subtree stays
//! as it is — so a rewrite whose algebra changes only a few shapes of node
//! passes over every subtree holding none of them
//! ([`crate::rewrite::desugar_within`]) instead of rebuilding and comparing
//! it node by node.
//!
//! The program-wide loop runs some ten whole-program rewrites a round
//! (`Template.hs:65-81`, and `instantiate . resolve` at `:58`), each a
//! `cata` over every module, while a round changes a sliver of the
//! program: the copies it made and the spines above them. A node never
//! changes once made — the arena is append-only, and a rewrite mints a
//! replacement — except by a write in place ([`ExpArena::node_mut`]), and
//! such a write forgets what was known of the node and of every node known
//! to hold it. So a fact, once known, is true for as long as it is kept.
//!
//! [`ExpArena::node_mut`]: crate::arena::ExpArena::node_mut

use kg_base::FxHashMap;

use crate::arena::NodeData;
use crate::exp::Exp;

/// A set of shapes a subtree may hold. A flag is set when the subtree holds
/// a node of the shape; one not set is a promise that it holds none.
pub type Flags = u16;

/// A `TemplateInstance` — what `resolve` rewrites (`Template.hs:157-170`).
pub const TEMPLATE_INSTANCE: Flags = 1 << 0;
/// A `FunctionCall` — what `closureCalls` and `addLambdaWrappers` rewrite
/// (`Closure.hs:19-26`, `Function.hs:367`).
pub const FUNCTION_CALL: Flags = 1 << 1;
/// A `Template` — what `instantiate` copies (`Template.hs:112-115`).
pub const TEMPLATE: Flags = 1 << 2;
/// A `Seq` with a `Seq` among its items — what `removeNestedSeq` splices
/// (`Desugar.hs:132-137`), every rewrite's catch-all.
pub const NESTED_SEQ: Flags = 1 << 3;
/// A `Seq` with two or more items `deduplicateInstances` keys by name
/// (`Template.hs:144-155`), whatever their names.
pub const DEDUP_SEQ: Flags = 1 << 4;
/// A `Lambda` — what `desugarLambdas` hoists (`Function.hs:190-235`).
pub const LAMBDA: Flags = 1 << 5;
/// A function with a bare `auto` parameter — what
/// `abbreviatedFunctionTemplate` makes a template (`Function.hs:290-311`).
pub const AUTO_PARAMS: Flags = 1 << 6;
/// What `moveFunctions` can move (`Function.hs:88-119`): a function, nested
/// scope or class whose body holds a function, class or template; a loop
/// whose body is a bare `Seq`; an `if` with a bare `Seq` branch.
pub const MOVES: Flags = 1 << 7;
/// What `deduceTemplateArgs` rewrites (`Type.hs:1384-1400`): a member
/// access, a method callee, a designator or call of a function specifier,
/// a named qualified identifier, and a type identifier or scoped name over
/// an instance.
pub const DEDUCES: Flags = 1 << 8;
/// A later edition's named argument — what `kg_sem::named` places; no
/// subtree of a reference file holds one.
pub const LABELLED: Flags = 1 << 9;

/// Every flag: a subtree whose facts are not kept is taken to hold all.
pub const ALL: Flags = (1 << 10) - 1;

const KNOWN: Flags = 1 << 15;
const NO_PARENT: u32 = u32::MAX;

/// The shapes the node at `index` is itself, reading its children's
/// expressions where the shape is theirs.
fn own(nodes: &[NodeData], index: usize) -> Flags {
    let kind = |id: crate::arena::ExpId| &nodes[id.index()].exp;
    match &nodes[index].exp {
        Exp::TemplateInstance { .. } => TEMPLATE_INSTANCE,
        Exp::FunctionCall { function, .. } => {
            FUNCTION_CALL | if matches!(kind(*function), Exp::FunctionSpecifier { .. }) { DEDUCES } else { 0 }
        }
        Exp::MemberAccess { .. } | Exp::FunctionSpecifier { receiver: Some(_), .. } => DEDUCES,
        Exp::Designator { value, .. } if matches!(kind(*value), Exp::FunctionSpecifier { .. }) => DEDUCES,
        Exp::NamedValue(a) if matches!(kind(*a), Exp::QualifiedIdentifier(_)) => DEDUCES,
        Exp::TypeIdentifier(_, t) | Exp::ScopedName { prefix: Some(t), .. } if matches!(kind(*t), Exp::TemplateInstance { .. }) => {
            DEDUCES
        }
        Exp::Template { .. } => TEMPLATE,
        Exp::Lambda { .. } => LAMBDA,
        Exp::Labelled { .. } => LABELLED,
        Exp::Seq(items) => {
            let mut f = 0;
            if items.iter().any(|&i| matches!(kind(i), Exp::Seq(_))) {
                f |= NESTED_SEQ;
            }
            let keyed = items
                .iter()
                .filter(|&&i| {
                    matches!(
                        kind(i),
                        Exp::Class { .. } | Exp::Struct { .. } | Exp::Union { .. } | Exp::Alias { .. } | Exp::Template { .. } | Exp::Function { .. }
                    )
                })
                .count();
            if keyed >= 2 {
                f |= DEDUP_SEQ;
            }
            f
        }
        Exp::Function { params, body, .. } => {
            auto_params(nodes, *params) | if holds_declarations(nodes, *body) { MOVES } else { 0 }
        }
        Exp::FunctionDecl { params, .. } => auto_params(nodes, *params),
        Exp::NestedScope { body, .. } | Exp::Class { body, .. } => {
            if holds_declarations(nodes, *body) {
                MOVES
            } else {
                0
            }
        }
        Exp::DoWhile { body, .. } | Exp::RangeFor { body, .. } => {
            if matches!(kind(*body), Exp::Seq(_)) {
                MOVES
            } else {
                0
            }
        }
        Exp::If { then_branch, else_branch, .. } => {
            if matches!(kind(*then_branch), Exp::Seq(_)) || else_branch.is_some_and(|e| matches!(kind(e), Exp::Seq(_))) {
                MOVES
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// `isAbbreviatedTemplate`'s parameter test, as `abbreviated`'s
/// `is_auto_param` reads it: a `FuncParam` typed bare `auto` and named.
fn auto_params(nodes: &[NodeData], params: crate::arena::ExpId) -> Flags {
    let Exp::Seq(ps) = &nodes[params.index()].exp else { return 0 };
    let auto = ps.iter().any(|p| match &nodes[p.index()].exp {
        Exp::FuncParam { ty, name, .. } => {
            matches!(nodes[ty.index()].exp, Exp::Auto) && matches!(nodes[name.index()].exp, Exp::Identifier(_))
        }
        _ => false,
    });
    if auto { AUTO_PARAMS } else { 0 }
}

/// A body `Seq` holding an item `moveFunctions`' `selects` could pick —
/// a function, class or template, whatever its kind and name.
fn holds_declarations(nodes: &[NodeData], body: crate::arena::ExpId) -> bool {
    let Exp::Seq(items) = &nodes[body.index()].exp else { return false };
    items.iter().any(|i| matches!(nodes[i.index()].exp, Exp::Function { .. } | Exp::Class { .. } | Exp::Template { .. }))
}

/// Per node index, what its subtree holds once known, and the nodes whose
/// facts were derived from it — every one of which a write to it makes
/// unknown again.
#[derive(Debug, Default)]
pub(crate) struct Facts {
    flags: Vec<Flags>,
    first_parent: Vec<u32>,
    more_parents: FxHashMap<u32, Vec<u32>>,
}

impl Facts {
    pub(crate) fn known(&self, index: usize) -> Option<Flags> {
        self.flags.get(index).copied().filter(|f| f & KNOWN != 0).map(|f| f & !KNOWN)
    }

    fn add_parent(&mut self, child: usize, parent: u32) {
        if self.first_parent.len() <= child {
            self.first_parent.resize(child + 1, NO_PARENT);
        }
        match self.first_parent[child] {
            NO_PARENT => self.first_parent[child] = parent,
            p if p == parent => {}
            _ => {
                let more = self.more_parents.entry(child as u32).or_default();
                if !more.contains(&parent) {
                    more.push(parent);
                }
            }
        }
    }

    /// What the subtree at `root` holds, deriving what is not yet known
    /// bottom-up over one stack.
    pub(crate) fn holds(&mut self, nodes: &[NodeData], root: usize) -> Flags {
        if let Some(f) = self.known(root) {
            return f;
        }
        let mut stack = vec![(root, false)];
        let mut kids = Vec::new();
        while let Some((index, below_done)) = stack.pop() {
            if self.known(index).is_some() {
                continue;
            }
            kids.clear();
            nodes[index].exp.children(&mut kids);
            if !below_done {
                stack.push((index, true));
                stack.extend(kids.iter().filter(|k| self.known(k.index()).is_none()).map(|k| (k.index(), false)));
                continue;
            }
            let mut f = own(nodes, index);
            for k in &kids {
                f |= self.known(k.index()).expect("every child is known before its parent");
                self.add_parent(k.index(), index as u32);
            }
            if self.flags.len() <= index {
                self.flags.resize(index + 1, 0);
            }
            self.flags[index] = f | KNOWN;
        }
        self.known(root).expect("derived above")
    }

    /// The node at `index` is about to be written in place: it, and every
    /// node whose facts were derived from it, is unknown again. A node not
    /// known has no known node above it — a fact is derived only from known
    /// children — so the climb stops there.
    pub(crate) fn forget(&mut self, index: usize) {
        let mut stack = vec![index];
        while let Some(i) = stack.pop() {
            match self.flags.get_mut(i) {
                Some(f) if *f & KNOWN != 0 => *f = 0,
                _ => continue,
            }
            if let Some(&p) = self.first_parent.get(i)
                && p != NO_PARENT
            {
                stack.push(p as usize);
                if let Some(more) = self.more_parents.get(&(i as u32)) {
                    stack.extend(more.iter().map(|&p| p as usize));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{AstNode, ast};

    use super::*;
    use crate::arena::ExpId;
    use crate::rewrite::{desugar, desugar_within, Rewriter};
    use crate::sort::NoImports;
    use crate::{lower_sorted, ModuleIr};

    fn lowered(text: &str, interner: &Interner) -> ModuleIr {
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "fixture must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        lower_sorted(&file, FileId(0), interner, &NoImports)
    }

    fn build(text: &str, interner: &Interner) -> ModuleIr {
        let mut ir = lowered(text, interner);
        ir.arena.track_facts();
        ir
    }

    /// The first node, in allocation order, `pick` takes.
    fn find(ir: &ModuleIr, pick: impl Fn(&Exp) -> bool) -> ExpId {
        ir.arena.iter().find_map(|(id, n)| pick(&n.exp).then_some(id)).expect("the fixture has one")
    }

    /// The function named `name`.
    fn function(ir: &ModuleIr, interner: &Interner, name: &str) -> ExpId {
        let name = interner.intern(name);
        let arena = &ir.arena;
        find(ir, |e| matches!(e, Exp::Function { name: n, .. } if matches!(arena[*n].exp, Exp::Identifier(s) if s == name)))
    }

    const CALLS: &str = "\
inline uint8 g(uint8 y)
{
    return y;
}

inline uint8 f(uint8 x)
{
    return g(x);
}

template <typename T>
inline T id(T t)
{
    return t;
}

inline uint8 h(auto a)
{
    const auto k = id<uint8>(1);
    return reduction([](bool p, bool q){ return p && q; }, a);
}
";

    /// Each shape is held by the node that has it and everything above it,
    /// and by nothing beside it. Red when a shape is not derived, or not
    /// carried up from the children.
    #[test]
    fn a_subtree_holds_the_shapes_below_it_and_no_others() {
        let interner = Interner::new();
        let mut ir = build(CALLS, &interner);
        let (f, g, h) = (function(&ir, &interner, "f"), function(&ir, &interner, "g"), function(&ir, &interner, "h"));
        let template = find(&ir, |e| matches!(e, Exp::Template { .. }));
        let root = ir.root;
        let a = &mut ir.arena;
        assert_eq!(a.holds(g) & (FUNCTION_CALL | TEMPLATE_INSTANCE | LAMBDA | AUTO_PARAMS | TEMPLATE), 0);
        assert_ne!(a.holds(f) & FUNCTION_CALL, 0);
        assert_eq!(a.holds(f) & (TEMPLATE_INSTANCE | LAMBDA | AUTO_PARAMS), 0);
        assert_ne!(a.holds(template) & TEMPLATE, 0);
        for shape in [FUNCTION_CALL, TEMPLATE_INSTANCE, LAMBDA, AUTO_PARAMS] {
            assert_ne!(a.holds(h) & shape, 0, "{shape:#x}");
        }
        for shape in [FUNCTION_CALL, TEMPLATE, TEMPLATE_INSTANCE, LAMBDA, AUTO_PARAMS, DEDUP_SEQ] {
            assert_ne!(a.holds(root) & shape, 0, "{shape:#x}");
        }
    }

    /// What `moveFunctions` and `removeNestedSeq` act on, built where no
    /// lowering writes it. Red when either is not derived.
    #[test]
    fn a_nested_seq_and_a_movable_body_are_held() {
        let interner = Interner::new();
        let mut ir = build(CALLS, &interner);
        let (f, g) = (function(&ir, &interner, "f"), function(&ir, &interner, "g"));
        let Exp::Function { scope, kind: fk, attr, modifier, ret, name, params, body } = ir.arena[g].exp.clone() else {
            unreachable!("g is a function")
        };
        let (src, kind) = (ir.arena[body].src, ir.arena[body].kind);
        assert!(ir.arena.lacks(body, NESTED_SEQ | MOVES));
        let nested = ir.arena.alloc(Exp::Seq(vec![body]), src, kind);
        assert!(!ir.arena.lacks(nested, NESTED_SEQ));
        let holding = ir.arena.alloc(Exp::Seq(vec![f]), src, kind);
        let outer = ir.arena.alloc(Exp::Function { scope, kind: fk, attr, modifier, ret, name, params, body: holding }, src, kind);
        assert!(!ir.arena.lacks(outer, MOVES));
        assert!(ir.arena.lacks(g, MOVES));
    }

    /// A write in place forgets the node and every node whose facts came
    /// from it, through both of a shared node's parents. Red when the
    /// climb stops at the node, or follows only its first parent.
    #[test]
    fn a_write_in_place_is_seen_above_it_through_every_parent() {
        let interner = Interner::new();
        let mut ir = build(CALLS, &interner);
        let g = function(&ir, &interner, "g");
        let call = find(&ir, |e| matches!(e, Exp::FunctionCall { .. }));
        let ret = find(&ir, |e| matches!(e, Exp::Return(Some(_))));
        let Exp::Return(Some(value)) = ir.arena[ret].exp else { unreachable!("matched above") };
        let (src, kind) = (ir.arena[ret].src, ir.arena[ret].kind);
        // A second parent of the returned value, beside the return.
        let other = ir.arena.alloc(Exp::Return(Some(value)), src, kind);
        assert!(ir.arena.lacks(g, FUNCTION_CALL));
        assert!(ir.arena.lacks(other, FUNCTION_CALL));
        let as_call = ir.arena[call].exp.clone();
        ir.arena.node_mut(value).exp = as_call;
        assert!(!ir.arena.lacks(g, FUNCTION_CALL));
        assert!(!ir.arena.lacks(other, FUNCTION_CALL));
    }

    /// A pass within the shapes it needs makes the tree the whole cata
    /// makes, calling its algebra on fewer nodes. Red when nothing is
    /// passed over, or when a held shape is missed.
    #[test]
    fn a_pass_within_its_needs_agrees_with_the_whole_cata() {
        let interner = Interner::new();
        let base = build(CALLS, &interner);
        let run = |within: bool| {
            let mut ir = base.fork();
            let mut calls = 0;
            let mut rw = Rewriter::new(&mut ir.arena, &interner);
            let mut f = |rw: &mut Rewriter, _: ExpId, exp: Exp| {
                calls += 1;
                match exp {
                    Exp::FunctionCall { function, args, attr } => {
                        let attr = rw.donate(attr, Exp::Seq(Vec::new()));
                        Exp::FunctionCall { attr, function, args }
                    }
                    e => e,
                }
            };
            let root = ir.root;
            let root = if within { desugar_within(&mut rw, root, FUNCTION_CALL, &mut f) } else { desugar(&mut rw, root, &mut f) };
            ir.root = root;
            (crate::structure::render(&ir, &interner, crate::structure::Detail::Annotated), calls)
        };
        let (whole, all) = run(false);
        let (within, some) = run(true);
        assert_eq!(whole, within);
        assert!(some * 2 < all, "{some} calls of {all}");
    }

    /// Each shape `deduceTemplateArgs` rewrites is held by the node itself,
    /// built over children that hold nothing it rewrites — in a program a
    /// name or call below usually holds it too, which hides a shape left
    /// out. Red with any one of them left out, or with one granted to the
    /// same node over children of the wrong kind.
    #[test]
    fn each_shape_deduction_rewrites_is_its_own() {
        let interner = Interner::new();
        let mut ir = build(CALLS, &interner);
        let ret = find(&ir, |e| matches!(e, Exp::Return(Some(_))));
        let (src, kind) = (ir.arena[ret].src, ir.arena[ret].kind);
        let a = &mut ir.arena;
        let mut node = |exp: Exp| a.alloc(exp, src, kind);
        let scope = kg_base::SymbolPath::empty();
        let name = node(Exp::Identifier(interner.intern("n")));
        let empty = node(Exp::Seq(Vec::new()));
        let qualified = node(Exp::QualifiedIdentifier(name));
        let instance = node(Exp::TemplateInstance { base: name, args: empty });
        let specifier = node(Exp::FunctionSpecifier { receiver: None, function: name });
        let this = node(Exp::This { scope: scope.clone() });
        let deducing = [
            Exp::MemberAccess { base: this, member: name },
            Exp::FunctionSpecifier { receiver: Some(this), function: name },
            Exp::Designator { name, value: specifier },
            Exp::FunctionCall { attr: empty, function: specifier, args: empty },
            Exp::NamedValue(qualified),
            Exp::TypeIdentifier(scope.clone(), instance),
            Exp::ScopedName { scope: scope.clone(), prefix: Some(instance), ident: name },
        ];
        let other = [
            Exp::Designator { name, value: name },
            Exp::FunctionCall { attr: empty, function: name, args: empty },
            Exp::NamedValue(name),
            Exp::TypeIdentifier(scope.clone(), name),
            Exp::ScopedName { scope, prefix: Some(name), ident: name },
        ];
        let deducing: Vec<ExpId> = deducing.into_iter().map(&mut node).collect();
        let other: Vec<ExpId> = other.into_iter().map(&mut node).collect();
        for id in [name, empty, qualified, instance, specifier, this] {
            assert!(ir.arena.lacks(id, DEDUCES), "{:?}", ir.arena[id].exp);
        }
        for id in deducing {
            assert!(!ir.arena.lacks(id, DEDUCES), "{:?}", ir.arena[id].exp);
        }
        for id in other {
            assert!(ir.arena.lacks(id, DEDUCES), "{:?}", ir.arena[id].exp);
        }
    }

    /// An arena that keeps no facts holds every shape: nothing is passed
    /// over.
    #[test]
    fn without_facts_every_shape_is_held() {
        let interner = Interner::new();
        let mut ir = lowered(CALLS, &interner);
        let g = function(&ir, &interner, "g");
        assert_eq!(ir.arena.holds(g), ALL);
    }
}
