//! Frontend pass 7, `trimExternClasses` (`Frontend.hs:315-335`): every
//! function of a class the program declares `extern` keeps its signature
//! and loses its body — `return;`, or `return {};` where it returns a
//! value — so nothing in it is instantiated or checked (the backend links
//! the real one). Runs on types: `extern Foo` names a class only once
//! `Foo`'s type is inferred (`tIsClass $ theType a`).

use kg_base::{FxHashSet, Interner, Symbol};
use kg_hir::rewrite::{desugar, Rewriter};
use kg_hir::{Exp, ExpId};

use crate::infer::Program;

pub fn trim_extern_classes(program: &mut Program<'_>) {
    let interner: &Interner = program.interner();
    // `externClasses = Set.unions $ parallel (setWith extern) program`.
    let mut externs: FxHashSet<Vec<Symbol>> = FxHashSet::default();
    for m in &program.modules {
        for (_, n) in m.ir.arena.iter() {
            if let Exp::Extern { ty, .. } = n.exp {
                let t = m.types.get(ty).the_type();
                if t.is_class()
                    && let Some(q) = t.qualified_name()
                {
                    externs.insert(q.segments().to_vec());
                }
            }
        }
    }
    // Every module's classes are trimmed by what any module declares.
    if let Some(door) = program.door {
        use std::hash::{Hash, Hasher};
        let mut sorted: Vec<&Vec<Symbol>> = externs.iter().collect();
        sorted.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        sorted.hash(&mut h);
        let set = h.finish();
        for reader in 0..program.modules.len() {
            door.fact(reader, "extern classes", set);
        }
    }
    if externs.is_empty() {
        return;
    }
    for m in program.modules.iter_mut() {
        let root = m.ir.root;
        let mut rw = Rewriter::new(&mut m.ir.arena, interner);
        m.ir.root = desugar(&mut rw, root, &mut |rw, id, exp| {
            let Exp::Class { scope, name, body } = exp else { return exp };
            let named = crate::symbols::declared_name(rw.arena, id).map(|(s, n)| {
                let mut q = s.segments().to_vec();
                q.push(n);
                q
            });
            if !named.is_some_and(|q| externs.contains(&q)) {
                return Exp::Class { scope, name, body };
            }
            let body = trim(rw, body);
            Exp::Class { scope, name, body }
        });
    }
}

/// `desugarShallow trim`: a function's body replaced, a nested class left
/// alone, anything else entered.
fn trim(rw: &mut Rewriter<'_>, id: ExpId) -> ExpId {
    let exp = rw.arena[id].exp.clone();
    let out = match exp {
        Exp::Function { scope, kind, attr, modifier, ret, name, params, body } => {
            // `new = flip newNote body`.
            let value = if matches!(rw.arena[ret].exp, Exp::Void) {
                None
            } else {
                let empty = rw.donate(body, Exp::Seq(Vec::new()));
                Some(rw.donate(body, Exp::InitializerList(empty)))
            };
            let ret_stmt = rw.donate(body, Exp::Return(value));
            let body = rw.donate(body, Exp::Seq(vec![ret_stmt]));
            Exp::Function { scope, kind, attr, modifier, ret, name, params, body }
        }
        Exp::Class { .. } => return id,
        mut other => {
            for child in other.children_mut() {
                *child = trim(rw, *child);
            }
            other
        }
    };
    if out == rw.arena[id].exp { id } else { rw.replace(id, out) }
}
