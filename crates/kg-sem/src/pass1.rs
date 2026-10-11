//! What of frontend passes 1 and 3 (`Frontend.hs:72,76-80`) the
//! elaboration's input needs, that the IR's own passes do not do:
//! `redundantNameScope` (`Frontend.hs:160-167`), `interpolatedStringPre`'s
//! echo (`Frontend.hs:233-238`) and `reifyEnums` (`Frontend.hs:195-205`).
//!
//! The parser qualifies every name it resolves with its declaration's scope
//! (`makeQualified`, `Parser.hs:686-688`); pass 1 takes the qualifier off a
//! value name again where it is the declaration's own scope — a
//! same-module function, a local — so `getQualifiedName` of `add` is
//! `add` (the wrapper `addLambdaWrappers` names after it is `$wrapper$add`),
//! and only a name reached through another scope keeps its qualifier.

use kg_base::Interner;
use kg_hir::rewrite::{desugar, get_name, get_qualifier, Rewriter};
use kg_hir::{Exp, ModuleIr};

use crate::reads::{Asked, Door};
use crate::symbols::{declared_name_by, Config, Symbols};

/// `desugar (redundantNameScope symbols)` over every module, `symbols` the
/// program's `getSymbols` — each lookup, and each declaration of another
/// module it reads, shown to `door`.
pub fn redundant_name_scope(modules: &mut [ModuleIr], interner: &Interner, door: Option<&Door>) {
    let symbols = Symbols::with(modules.iter(), interner, Config::AllSymbols);
    let snapshot: Vec<kg_hir::ExpArena> = modules.iter().map(|m| m.arena.fork()).collect();
    for (reader, ir) in modules.iter_mut().enumerate() {
        let root = ir.root;
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = desugar(&mut rw, root, &mut |rw, _id, exp| {
            let Exp::QualifiedIdentifier(s) = exp else { return exp };
            let Exp::ScopedName { scope, prefix: Some(_), ident } = rw.arena[s].exp.clone() else {
                return exp;
            };
            let Some(name) = get_name(rw.arena, ident) else { return exp };
            let found = symbols.in_scope(scope.segments(), &[name]);
            if let Some(door) = door {
                door.lookup(reader, Asked::InScope(scope.segments(), &[name]), found, |d| (&snapshot[d.module], &snapshot[d.module][d.id].exp));
            }
            let Some(x) = found else { return exp };
            let arena = &snapshot[x.module];
            let read = |y: kg_hir::ExpId| {
                if let Some(door) = door {
                    door.node(reader, x.module, arena, y, &arena[y].exp);
                }
                &arena[y].exp
            };
            let declared = declared_name_by(&read, x.id).map(|(scope, _)| scope);
            let written = get_qualifier(rw.arena, s);
            if declared.as_ref().map(|d| d.segments()) != written.as_deref() {
                return exp;
            }
            // `unscopedName = copyNote (ScopedNameF a Nothing c) s`.
            let unscoped = rw.donate(s, Exp::ScopedName { scope, prefix: None, ident });
            Exp::QualifiedIdentifier(unscoped)
        });
    }
}

/// `interpolatedStringPre`'s echo arm (`Frontend.hs:236-237`): `{e=}`
/// becomes the text `showPretty e ++ " = "` before a plain `{e}` — taken
/// here, after `redundantNameScope` as the reference composes the two, and
/// before inference folds `2+2` into `4` or a call into an instance's.
/// What `prettySource` spells is [`show_pretty`]'s subset; any other hole
/// stays an echo, for the walk to decline.
pub fn interpolated_string_echo(modules: &mut [ModuleIr], interner: &Interner) {
    for ir in modules.iter_mut() {
        let root = ir.root;
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = desugar(&mut rw, root, &mut |rw, _id, exp| {
            let Exp::Interpolated(segments) = &exp else { return exp };
            if !segments.iter().any(|s| s.hole.as_ref().is_some_and(|h| h.echo)) {
                return exp;
            }
            let mut out = segments.to_vec();
            for seg in &mut out {
                let Some(hole) = &mut seg.hole else { continue };
                if !hole.echo {
                    continue;
                }
                let Some(shown) = show_pretty(rw.arena, interner, hole.expr) else { continue };
                seg.text = kg_base::SmolStr::new(format!("{}{shown} = ", seg.text));
                hole.echo = false;
            }
            Exp::Interpolated(out.into_boxed_slice())
        });
    }
}

/// `showPretty` — `prettySource` (`PrettyPrint.hs:36-178`) rendered by
/// `show`'s 80-column layout — for the expressions whose rendering is fixed:
/// a name; an untyped integer or a boolean literal; an operator, a binary
/// one parenthesised with a space either side (`:70`) — `{2+2=}` is `(2 +
/// 2) = 4` (`test/logic/string.k`'s own assertion) — a unary one prefixed
/// (`:171`), a size spaced from its operand (`:168-169`), `clog2`
/// parenthesised (`:170`); and a call of a name with no attribute, its
/// arguments comma-separated (`:104,:44`) — `{reduction_nor(x)=}` spelled as
/// written, not as the copy the loop will call (`test/library/data/bits.k`).
/// A `softline` breaks only a rendering too long for the line, which is
/// left an echo.
pub fn show_pretty(arena: &kg_hir::ExpArena, interner: &Interner, id: kg_hir::ExpId) -> Option<String> {
    let text = pretty_exp(arena, interner, id)?;
    (text.len() <= 72).then_some(text)
}

fn pretty_exp(arena: &kg_hir::ExpArena, interner: &Interner, id: kg_hir::ExpId) -> Option<String> {
    use kg_const::UnOp;
    let go = |e| pretty_exp(arena, interner, e);
    match &arena[id].exp {
        Exp::NamedValue(e) | Exp::QualifiedIdentifier(e) => go(*e),
        // `TemplateInstance a b | null $ prev b = pretty a` (`:160-161`).
        Exp::TemplateInstance { base, args } if matches!(&arena[*args].exp, Exp::Seq(a) if a.is_empty()) => go(*base),
        Exp::ScopedName { .. } => pretty_name(arena, interner, id),
        Exp::IntLiteral { ty: None, value } => Some(value.to_string()),
        Exp::BoolLiteral(b) => Some(b.to_string()),
        Exp::Binary(op, a, b) => Some(format!("({} {} {})", go(*a)?, op.text()?, go(*b)?)),
        Exp::Unary(op @ (UnOp::Neg | UnOp::Invert | UnOp::Not), e) => Some(format!("{}{}", op.text(), go(*e)?)),
        Exp::Unary(op @ (UnOp::BitSizeof | UnOp::ByteSizeof), e) => Some(format!("{} {}", op.text(), go(*e)?)),
        Exp::Unary(UnOp::Clog2, e) => Some(format!("clog2({})", go(*e)?)),
        Exp::FunctionCall { attr, function, args } => {
            let (Exp::Seq(attrs), Exp::Seq(args)) = (&arena[*attr].exp, &arena[*args].exp) else { return None };
            let Exp::FunctionSpecifier { receiver: None, function } = &arena[*function].exp else { return None };
            if !attrs.is_empty() || !matches!(arena[*function].exp, Exp::QualifiedIdentifier(_)) {
                return None;
            }
            let args: Option<Vec<String>> = args.iter().map(|&a| go(a)).collect();
            Some(format!("{}({})", go(*function)?, args?.join(", ")))
        }
        _ => None,
    }
}

/// `ScopedName _ (Just a) b` is `pretty b` when `a` is an internal
/// qualifier — a module's `@…` namespace or `$unresolved` — and `pretty a
/// <> "::" <> pretty b` otherwise (`PrettyPrint.hs:145-148,187-192`):
/// `{E::A=}` is `E::A = 0`, as `redundantNameScope` leaves it (`kg-oracle
/// --passes 1`, 2026-09-27). A qualifier no one could have written — a
/// scope's `$` name — is this port's sorting, not the reference's, and
/// declines.
fn pretty_name(arena: &kg_hir::ExpArena, interner: &Interner, id: kg_hir::ExpId) -> Option<String> {
    let Exp::ScopedName { prefix, ident, .. } = &arena[id].exp else { return None };
    let Exp::Identifier(leaf) = &arena[*ident].exp else { return None };
    let leaf = interner.resolve(*leaf);
    if leaf.starts_with('$') || leaf.starts_with('@') {
        return None;
    }
    let Some(prefix) = *prefix else { return Some(leaf.to_string()) };
    let internal = match &arena[prefix].exp {
        Exp::ScopedName { prefix: None, ident, .. } => match &arena[*ident].exp {
            Exp::Identifier(x) => {
                let x = interner.resolve(*x);
                x.starts_with('@') || x == kg_hir::sort::UNRESOLVED_NAME
            }
            _ => return None,
        },
        _ => false,
    };
    if internal {
        return Some(leaf.to_string());
    }
    Some(format!("{}::{leaf}", pretty_name(arena, interner, prefix)?))
}

/// `reifyEnums` (`Frontend.hs:195-205`, pass 3): an enum constant written
/// with no value is given the previous one's plus one — `0` for the first —
/// as the expression the reference writes, `BinaryF Add x (IntLiteralF
/// Nothing 1)`, for `evaluateConstexpr` to fold. Each value wears its
/// constant's note.
pub fn reify_enums(modules: &mut [ModuleIr], interner: &Interner) {
    for ir in modules.iter_mut() {
        let root = ir.root;
        let mut rw = Rewriter::new(&mut ir.arena, interner);
        ir.root = desugar(&mut rw, root, &mut |rw, _id, exp| {
            let Exp::Enum { scope, name, ty, body } = exp else { return exp };
            let Exp::Seq(items) = rw.arena[body].exp.clone() else { return Exp::Enum { scope, name, ty, body } };
            // `reifyValues (IntLiteralF Nothing 0) xs`: the next value, as an
            // expression not yet noted.
            let mut next = Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(0u64) };
            let mut out = Vec::with_capacity(items.len());
            let mut changed = false;
            for x in items {
                let Exp::EnumConstant { scope: cs, name: cn, value } = rw.arena[x].exp.clone() else {
                    out.push(x);
                    continue;
                };
                let v = match value {
                    Some(v) => v,
                    None => {
                        let v = rw.donate(x, next.clone());
                        changed = true;
                        out.push(rw.replace(x, Exp::EnumConstant { scope: cs, name: cn, value: Some(v) }));
                        next = increment(rw, v);
                        continue;
                    }
                };
                out.push(x);
                // `increment $ fromJust $ constValue e'`.
                next = increment(rw, v);
            }
            if !changed {
                return Exp::Enum { scope, name, ty, body };
            }
            let body = rw.replace(body, Exp::Seq(out));
            Exp::Enum { scope, name, ty, body }
        });
    }
}

/// `increment x = BinaryF Add x (copyNote (IntLiteralF Nothing 1) x)` over a
/// *copy* of `x`: the reference's next value holds its own copy of the
/// previous one, which folds a level per inference round. Shared, a level
/// folded for one constant would be met again, a literal already, when the
/// next is visited, and the whole chain would fold in one — typing a use
/// of the last constant as a literal where the reference, rounds earlier,
/// typed it as the enum (`literal-operand.k::11`).
fn increment(rw: &mut Rewriter<'_>, v: kg_hir::ExpId) -> Exp {
    let x = rw.deep_copy(v);
    let one = rw.donate(v, Exp::IntLiteral { ty: None, value: kg_const::ConstInt::from(1u64) });
    Exp::Binary(kg_const::BinOp::Add, x, one)
}

/// Passes 2 and 3 (`higherOrderFunctions`, `captureThis`) over every
/// module, pass 2 seeing a function type through an alias:
/// `resolvedFuncParams symbols` (`Function.hs:133-141`), `symbols` the
/// program's `getSymbols` (`withSymbols`, `Frontend.hs:73`).
///
/// The symbols are built only when some module declares an alias of a
/// function type, or of another name — the only aliases a parameter's
/// type can resolve through to one. With the modules, the range of every
/// function pass 2 refused for want of `inline`
/// (`kg_hir::rewrite::pipeline_head_refusing`).
pub fn heads(modules: &[ModuleIr], interner: &Interner, door: Option<&Door>) -> (Vec<ModuleIr>, Vec<kg_base::FileRange>) {
    let aliased = modules.iter().any(|ir| {
        ir.arena.iter().any(|(_, n)| match &n.exp {
            Exp::Alias { body, .. } => {
                matches!(ir.arena[*body].exp, Exp::FunctionType { .. } | Exp::TypeIdentifier(..))
            }
            _ => false,
        })
    });
    // Whether any module declares such an alias decides whether every
    // module's parameters are resolved through one.
    if let Some(door) = door {
        for reader in 0..modules.len() {
            door.fact(reader, "aliased", u64::from(aliased));
        }
    }
    let mut refused = Vec::new();
    if !aliased {
        let heads = modules
            .iter()
            .map(|ir| {
                let (out, r) = kg_hir::rewrite::pipeline_head_refusing(ir, interner, None);
                refused.extend(r);
                out
            })
            .collect();
        return (heads, refused);
    }
    let symbols = Symbols::with(modules.iter(), interner, Config::AllSymbols);
    let heads = modules
        .iter()
        .enumerate()
        .map(|(reader, ir)| {
            let resolve = |arena: &kg_hir::ExpArena, ty: kg_hir::ExpId| {
                alias_function_type(modules, &symbols, arena, ty, Reader { module: reader, door })
            };
            let (out, r) = kg_hir::rewrite::pipeline_head_refusing(ir, interner, Some(&resolve));
            refused.extend(r);
            out
        })
        .collect();
    (heads, refused)
}

/// The module a pass rewrites, and the door its reads of another are shown.
#[derive(Clone, Copy)]
struct Reader<'d> {
    module: usize,
    door: Option<&'d Door>,
}

/// `resolveAlias` (`Function.hs:137-141`): a `TypeIdentifier` naming an
/// alias is the alias's type, again and again; the `FunctionType` that
/// ends it, and the module arena holding it. Anything else — a name that
/// does not resolve, an alias of any other type — is `None`, the parameter
/// read as written. An alias cycle is the parser's error; the walk is
/// bounded all the same.
///
/// What it reads of another module is shown to `reader`'s door: each
/// lookup, each alias, and each alias's type whole — what it names, and
/// the function type the parameter is retyped with a copy of
/// (`kg_hir::ExpArena::deep_copy_from`), positions and all.
fn alias_function_type<'m>(
    modules: &'m [ModuleIr],
    symbols: &Symbols,
    arena: &kg_hir::ExpArena,
    ty: kg_hir::ExpId,
    reader: Reader<'_>,
) -> Option<(&'m kg_hir::ExpArena, kg_hir::ExpId)> {
    use crate::deduce::Cx;
    let (mut from, mut at): (&kg_hir::ExpArena, kg_hir::ExpId) = (arena, ty);
    let mut resolved: Option<(&'m kg_hir::ExpArena, kg_hir::ExpId)> = None;
    for _ in 0..32 {
        match &from[at].exp {
            Exp::FunctionType { .. } => return resolved,
            Exp::TypeIdentifier(..) => {
                let scope = Cx::scope_of(from, at)?;
                let name = Cx::qualified_name(from, at)?;
                let found = symbols.in_scope(scope.segments(), &name);
                if let Some(door) = reader.door {
                    door.lookup(reader.module, Asked::InScope(scope.segments(), &name), found, |d| {
                        (&modules[d.module].arena, &modules[d.module].arena[d.id].exp)
                    });
                }
                let d = found?;
                let decl = &modules[d.module].arena;
                if let Some(door) = reader.door {
                    door.node(reader.module, d.module, decl, d.id, &decl[d.id].exp);
                }
                let Exp::Alias { body, .. } = &decl[d.id].exp else { return None };
                if let Some(door) = reader.door {
                    door.subtree(reader.module, d.module, decl, None, *body);
                }
                resolved = Some((decl, *body));
                (from, at) = (decl, *body);
            }
            _ => return None,
        }
    }
    None
}
