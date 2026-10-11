//! The loop's trees as a debugging aid reads them: every node a module's
//! tree reaches ([`reachable`]), grouped by constructor ([`constructor`]),
//! and `KG_SEM_DUMP`'s dump of one subtree with its types
//! ([`dump_on_request`], which `crate::looped` calls per module).
//!
//! Until sprint W3.7 this module was also `cargo xtask sem`'s shadow gate
//! (`docs/history/ir-elaboration.md` step M1): the IR engine's types against
//! the CST inference engine's, node for node. It went with that engine.

use kg_hir::Exp;

use crate::db::Db;

/// The constructor's name, as a dump filters by it.
pub(crate) fn constructor(exp: &Exp) -> String {
    let text = format!("{exp:?}");
    text.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("?").to_owned()
}

/// Every node `ir`'s tree reaches from its root.
pub(crate) fn reachable(ir: &kg_hir::ModuleIr) -> kg_base::FxHashSet<kg_hir::ExpId> {
    let mut seen = kg_base::FxHashSet::default();
    let mut stack = vec![ir.root];
    let mut kids = Vec::new();
    while let Some(id) = stack.pop() {
        if seen.insert(id) {
            kids.clear();
            ir.arena[id].exp.children(&mut kids);
            stack.extend(kids.iter().copied());
        }
    }
    seen
}

/// `KG_SEM_DUMP=<path suffix>:<offset>` with `KG_SEM_CTOR=<constructor>`:
/// print the engine's subtree at that node, with every type — the corpus
/// half of `kg_sem`'s `inspect` test.
pub(crate) fn dump_on_request(
    db: &Db,
    module: kg_resolve::ModuleSource,
    sem: &kg_sem::Module,
    nodes: &[(kg_hir::ExpId, String)],
    interner: &kg_base::Interner,
) {
    let Ok(spec) = std::env::var("KG_SEM_DUMP") else { return };
    let Some((path, start)) = spec.split_once(':') else { return };
    if !db.path(module.diagnostic_file()).is_some_and(|p| p.as_str().ends_with(path)) {
        return;
    }
    // A number is a node's start; anything else a declared name's prefix
    // (`KG_SEM_DUMP=function.k:compose2<`), whose declaration is dumped
    // shallowly: its own line and its children's.
    let by_name = start.parse::<u32>().is_err().then(|| start.to_owned());
    if let Some(prefix) = &by_name {
        for (id, _) in nodes {
            let Some((_, name)) = kg_sem::symbols::declared_name(&sem.ir.arena, *id) else { continue };
            if !interner.resolve(name).starts_with(prefix.as_str()) {
                continue;
            }
            let n = &sem.ir.arena[*id];
            eprintln!(
                "{} {:?} {:?} :: {}",
                interner.resolve(name),
                id,
                n.src.via,
                format!("{:?}", sem.types.get(*id)).chars().take(300).collect::<String>()
            );
        }
        return;
    }
    let start: u32 = start.parse().unwrap_or(0);
    fn dump(sem: &kg_sem::Module, id: kg_hir::ExpId, depth: usize, interner: &kg_base::Interner) {
        let n = &sem.ir.arena[id];
        let exp = match n.exp {
            Exp::Identifier(sym) => format!("Identifier({:?})", interner.resolve(sym)),
            ref e => format!("{e:?}"),
        };
        eprintln!(
            "{}{:?} {} via {:?} :: {}",
            "  ".repeat(depth),
            n.src.head.range,
            exp,
            n.src.via,
            kg_ty::subtype::show_pretty(&sem.types.get(id), Some(interner))
        );
        let mut kids = Vec::new();
        n.exp.children(&mut kids);
        for k in kids {
            dump(sem, k, depth + 1, interner);
        }
    }
    let want = std::env::var("KG_SEM_CTOR").ok();
    for (id, c) in nodes {
        let copies_only = std::env::var_os("KG_SEM_COPIES").is_some();
        if u32::from(sem.ir.arena[*id].src.head.range.start()) == start
            && want.as_deref().is_none_or(|w| w == c)
            && (!copies_only || sem.ir.arena[*id].src.via != kg_base::Expansion::ROOT)
        {
            dump(sem, *id, 0, interner);
        }
    }
}
