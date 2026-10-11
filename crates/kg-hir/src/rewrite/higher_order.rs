//! `higherOrderFunctions` (`Function.hs:313-361`) — frontend pass 2, the
//! rewrite that makes an inline function with function-typed parameters a
//! template with a capture type and a closure function per such parameter:
//!
//! ```text
//! inline uint32 Go((uint32) -> uint32 Do, uint32 x) { … }
//! ```
//! becomes
//! ```text
//! template <typename Do$capture, (Do$capture, uint32) -> uint32 Do>
//! inline uint32 Go(Closure<(Do$capture, uint32) -> uint32, Do, Do$capture> Do, uint32 x) { … }
//! ```
//!
//! Every lambda the twin desugars is a `cast<Closure<decltype(f), f,
//! capture>>(…)` (`rewrite::lambdas`), and the call `Go(fn, x)` then deduces
//! `Do$capture` from the closure's capture and `Do` from its function
//! (`deduceType` over `Closure<…>` against `TInstance`, `Type.hs:1532-1533`,
//! a `TNameArg` for `Do`); the copy `Go<Capture, main@?lambda0>` has `Do`
//! typed as the closure, which is what `decltype(Do) x = 10` converts from
//! (`closures.k::1`) and what `Do(x)` calls through.
//!
//! # The three clauses
//!
//! * A `Function` whose `resolvedFuncParams` hold a `FunctionType` becomes
//!   `TemplateF (concatMap templateParams functionTypeParams) function` when
//!   inline (`:321`), and `desugarError "Non-inline higher order functions
//!   are not supported"` otherwise (`:322`). The error is pass 2's own report
//!   in `kg-db`; here a non-inline one is left as written, since the twin
//!   would print an error node it cannot reparse.
//! * `TemplateF p1 (Template p2 e)` — a function that was already a template —
//!   is `mergeTemplateParams p1 p2 e` (`:266-280`): `p2`'s parameters after
//!   `p1`'s, their indexes offset by `length p1`, and every `TypeParam` in
//!   the body naming one of `p2` offset with it.
//! * Everything else is itself.
//!
//! `resolvedFuncParams` (`:133-141`) sees a function type through an alias
//! (`using F = (uint32) -> uint32; inline void Go(F f)`,
//! `higher-order-functions.k::22`), and the rewrite then builds from the
//! alias's `FunctionType`, notes and all. Resolving a name needs the
//! program's symbols, which the caller holds ([`apply_with`], an
//! [`AliasTypes`]); without one an alias-typed parameter is left as
//! written, which is what [`apply`] does.
//!
//! # Notes, as the reference donates them
//!
//! `templateParams x = map new […]` with `new fn = newNote (fn x) (snd x)`
//! (`:326-331`): both template parameters wear the function parameter's
//! note. `closureFunctionType` (`:333-339`) rebuilds the written function
//! type with the capture parameter consed in front, each new node noted at
//! the parameter list (`new = flip newNote e` over `b`); the rewritten
//! parameter's `closureInstance` is noted at the written type (`c`).

use kg_base::Symbol;
use kg_ty::{QualifiedName, Ty};

use crate::arena::{ExpArena, ExpId};
use crate::exp::{Exp, Modifier};
use crate::rewrite::{desugar, desugar_shallow, scoped_qualifier, Rewriter, ShallowRewrite};

/// `resolveAlias` (`Function.hs:137-141`) for a parameter's type: the
/// `FunctionType` a `TypeIdentifier` of `arena` names through aliases, and
/// the arena that holds it — `None` for anything else, a name that does not
/// resolve included.
pub type AliasTypes<'s> = dyn Fn(&ExpArena, ExpId) -> Option<(&'s ExpArena, ExpId)> + 's;

/// The pass, with how many functions it made templates of.
#[derive(Default)]
pub struct HigherOrderFunctions<'s> {
    pub fired: u32,
    /// Every function left as written for want of `inline` though a
    /// parameter is function-typed: pass 2's one verdict, `desugarError
    /// "Non-inline higher order functions are not supported"` (`:322`),
    /// which `kg_sem::check` reports where the function is written.
    pub refused: Vec<ExpId>,
    aliases: Option<&'s AliasTypes<'s>>,
}

impl std::fmt::Debug for HigherOrderFunctions<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HigherOrderFunctions")
            .field("fired", &self.fired)
            .field("refused", &self.refused)
            .field("aliases", &self.aliases.is_some())
            .finish()
    }
}

/// `higherOrderFunctions` over the whole tree, a parameter's type read as
/// written.
pub fn apply(rw: &mut Rewriter, root: ExpId) -> ExpId {
    let mut pass = HigherOrderFunctions::default();
    desugar_shallow(rw, root, &mut pass)
}

/// [`apply`], an alias-typed parameter's type resolved by `aliases`
/// (`resolvedFuncParams symbols`, `Function.hs:133-141,319`).
pub fn apply_with<'s>(rw: &mut Rewriter, root: ExpId, aliases: &'s AliasTypes<'s>) -> ExpId {
    apply_refusing(rw, root, Some(aliases)).0
}

/// [`apply`], or [`apply_with`] given `aliases`, and every function it
/// left as written for want of `inline` ([`HigherOrderFunctions::refused`]).
pub fn apply_refusing<'s>(rw: &mut Rewriter, root: ExpId, aliases: Option<&'s AliasTypes<'s>>) -> (ExpId, Vec<ExpId>) {
    let mut pass = HigherOrderFunctions { fired: 0, refused: Vec::new(), aliases };
    let root = desugar_shallow(rw, root, &mut pass);
    (root, pass.refused)
}

impl ShallowRewrite for HigherOrderFunctions<'_> {
    fn reverts(&self, _arena: &ExpArena, _id: ExpId) -> bool {
        false
    }

    fn rewrite(&mut self, rw: &mut Rewriter, id: ExpId, exp: Exp) -> Exp {
        match &exp {
            Exp::Function { modifier, params, .. } => {
                let inline = modifier
                    .is_some_and(|m| matches!(rw.arena[m].exp, Exp::FunctionModifier(Modifier::Inline)));
                // A non-inline one is pass 2's error, left as written here
                // (the module comment) and reported from `refused`.
                if !inline {
                    if self.takes_a_function(rw.arena, *params) {
                        self.refused.push(id);
                    }
                    return exp;
                }
                let Some(function_typed) = self.function_typed_params(rw, *params) else {
                    return exp;
                };
                self.fired += 1;
                self.function_template(rw, id, exp, &function_typed)
            }
            Exp::Template { params: p1, decl } => match rw.arena[*decl].exp.clone() {
                Exp::Template { params: p2, decl: e } => merge_template_params(rw, p1, &p2, e),
                _ => exp,
            },
            _ => exp,
        }
    }
}

impl HigherOrderFunctions<'_> {
    /// Whether `functionTypeParams` is not empty (`:319`): a parameter
    /// typed as written or through an alias as a `FunctionType` — asked of
    /// a function the pass leaves as written, so nothing is minted.
    fn takes_a_function(&self, arena: &ExpArena, params: ExpId) -> bool {
        let Exp::Seq(members) = &arena[params].exp else { return false };
        members.iter().any(|&p| {
            let Exp::FuncParam { ty, .. } = arena[p].exp else { return false };
            matches!(arena[ty].exp, Exp::FunctionType { .. }) || self.aliases.is_some_and(|resolve| resolve(arena, ty).is_some())
        })
    }

    /// `functionTypeParams = zip [0..] $ filter (isFunctionType . paramType)
    /// $ resolvedFuncParams symbols funcParams` (`:319`) — the
    /// function-typed parameters, numbered among themselves: each as
    /// written, and as resolved — the same node for a written
    /// `FunctionType`, a parameter retyped with a copy of the alias's for
    /// an alias. The resolved one is what the rewrite reads (`:333-360`
    /// match on it); the written one is what it replaces.
    fn function_typed_params(&self, rw: &mut Rewriter, params: ExpId) -> Option<Vec<(u32, ExpId, ExpId)>> {
        let Exp::Seq(members) = rw.arena[params].exp.clone() else { return None };
        let mut found = Vec::new();
        for p in members {
            let Exp::FuncParam { scope, attr, ty, name } = rw.arena[p].exp.clone() else { continue };
            let resolved = if matches!(rw.arena[ty].exp, Exp::FunctionType { .. }) {
                p
            } else if let Some((from, at)) = self.aliases.and_then(|resolve| resolve(rw.arena, ty)) {
                let ty = rw.arena.deep_copy_from(from, at);
                rw.donate(p, Exp::FuncParam { scope, attr, ty, name })
            } else {
                continue;
            };
            found.push((found.len() as u32, p, resolved));
        }
        (!found.is_empty()).then_some(found)
    }

    /// `functionTemplate (Just (FunctionModifier Inline))` (`:321`): the
    /// template parameters, then `function` with each function-typed
    /// parameter retyped as the closure instance (`:353-360`).
    fn function_template(
        &self,
        rw: &mut Rewriter,
        id: ExpId,
        exp: Exp,
        function_typed: &[(u32, ExpId, ExpId)],
    ) -> Exp {
        let mut template_params: Vec<ExpId> = Vec::with_capacity(function_typed.len() * 2);
        for &(i, _, p) in function_typed {
            template_params.push(capture_template_param(rw, i, p));
            template_params.push(closure_function_template_param(rw, i, p));
        }

        let Exp::Function { params: params_seq, .. } = &exp else { unreachable!("matched by the caller") };
        let params_seq = *params_seq;
        let Exp::Seq(members) = rw.arena[params_seq].exp.clone() else {
            unreachable!("a parameter list is a Seq (lower.rs)")
        };
        let mut rebuilt: Vec<ExpId> = Vec::with_capacity(members.len());
        for p in members {
            match function_typed.iter().find(|&&(_, written, _)| written == p) {
                Some(&(i, written, p)) => {
                    let Exp::FuncParam { scope, attr, ty, name } = rw.arena[p].exp.clone() else {
                        unreachable!("filtered as a FuncParam")
                    };
                    // `closureInstance a [closureFunctionType p, closureFunctionTypeParam p,
                    // captureTypeParam p] c` (`:357-358`), noted at the type — an alias's own.
                    let fn_ty = closure_function_type(rw, i, p);
                    let fn_tp = closure_function_type_param(rw, i, p);
                    let cap_tp = capture_type_param(rw, i, p);
                    let closure = closure_instance(rw, &scope, [fn_ty, fn_tp, cap_tp], ty);
                    rebuilt.push(rw.replace(written, Exp::FuncParam { scope, attr, ty: closure, name }));
                }
                None => rebuilt.push(p),
            }
        }
        let new_params = rw.replace(params_seq, Exp::Seq(rebuilt));
        let mut function = exp;
        if let Exp::Function { params, .. } = &mut function {
            *params = new_params;
        }
        let f_prime = rw.replace(id, function);
        Exp::Template { params: template_params, decl: f_prime }
    }
}

/// The parameter's type when it is a written `FunctionType`.
fn written_function_type(arena: &ExpArena, param: ExpId) -> Option<ExpId> {
    let Exp::FuncParam { ty, .. } = &arena[param].exp else { return None };
    matches!(arena[*ty].exp, Exp::FunctionType { .. }).then_some(*ty)
}

/// `(scope, name)` of a function parameter: `declScopeName` is the
/// function's — the parameter's key with its own segment dropped — and the
/// name is the parameter's `Identifier`.
fn param_key(rw: &Rewriter, param: ExpId) -> (QualifiedName, Symbol) {
    let Exp::FuncParam { scope, name, .. } = &rw.arena[param].exp else {
        unreachable!("filtered as a FuncParam")
    };
    let Exp::Identifier(sym) = rw.arena[*name].exp else {
        unreachable!("a parameter's name is an Identifier (lower.rs)")
    };
    (scope.init(), sym)
}

/// `captureTemplateParam (i, e) = TemplateParamF declScopeName TypenameF
/// (suffixName "$capture" name) (i * 2) Nothing` (`:341-342`).
fn capture_template_param(rw: &mut Rewriter, i: u32, param: ExpId) -> ExpId {
    let (scope, name) = param_key(rw, param);
    let kind = rw.donate(param, Exp::Typename);
    let name = rw.suffix_name(name, "$capture");
    rw.donate(param, Exp::TemplateParam { scope, kind, name, index: i * 2, default: None })
}

/// `closureFunctionTemplateParam (i, e) = TemplateParamF declScopeName
/// (closureFunctionType x) name (i * 2 + 1) Nothing` (`:345-346`).
fn closure_function_template_param(rw: &mut Rewriter, i: u32, param: ExpId) -> ExpId {
    let (scope, name) = param_key(rw, param);
    let kind = closure_function_type(rw, i, param);
    rw.donate(param, Exp::TemplateParam { scope, kind, name, index: i * 2 + 1, default: None })
}

/// `captureTypeParam = typeParam . captureTemplateParam` (`:349`).
fn capture_type_param(rw: &mut Rewriter, i: u32, param: ExpId) -> ExpId {
    let (scope, name) = param_key(rw, param);
    let kind = rw.donate(param, Exp::Typename);
    let name = rw.suffix_name(name, "$capture");
    rw.donate(param, Exp::TypeParam { scope, kind, name, index: i * 2 })
}

/// `closureFunctionTypeParam = typeParam . closureFunctionTemplateParam` (`:350`).
fn closure_function_type_param(rw: &mut Rewriter, i: u32, param: ExpId) -> ExpId {
    let (scope, name) = param_key(rw, param);
    let kind = closure_function_type(rw, i, param);
    rw.donate(param, Exp::TypeParam { scope, kind, name, index: i * 2 + 1 })
}

/// `closureFunctionType (_, FuncParam _ _ (FunctionType a b c) _) =
/// FunctionTypeF a (cons (captureParam b) b) c` (`:333-339`): the written
/// function type with the capture type parameter consed in front, every
/// new node noted at the parameter list `b`. Built fresh at each use — the
/// reference shares one binding, an arena cannot.
fn closure_function_type(rw: &mut Rewriter, i: u32, param: ExpId) -> ExpId {
    let ty = written_function_type(rw.arena, param).expect("filtered as function-typed");
    let Exp::FunctionType { attr, params, ret } = rw.arena[ty].exp.clone() else {
        unreachable!("checked by written_function_type")
    };
    let attr = rw.deep_copy(attr);
    let ret = rw.deep_copy(ret);
    let Exp::Seq(written) = rw.arena[params].exp.clone() else {
        unreachable!("a function type's parameters are a Seq (lower.rs)")
    };
    let capture_attr = rw.donate(params, Exp::Seq(Vec::new()));
    let capture_ty = capture_type_param(rw, i, param);
    let capture = rw.donate(params, Exp::FunctionTypeParam { attr: capture_attr, ty: capture_ty, name: None });
    let mut list = Vec::with_capacity(written.len() + 1);
    list.push(capture);
    for p in written {
        list.push(rw.deep_copy(p));
    }
    let params = rw.donate(params, Exp::Seq(list));
    rw.donate(ty, Exp::FunctionType { attr, params, ret })
}

/// `closureInstance declScopeName args e` (`Function.hs:180-184`):
/// `Closure<|args|>` from `data.closure.core`, noted at `donor`.
fn closure_instance(rw: &mut Rewriter, scope: &QualifiedName, args: [ExpId; 3], donor: ExpId) -> ExpId {
    let args = rw.donate(donor, Exp::Seq(args.to_vec()));
    let closure_path = Ty::closure_template_path(rw.interner);
    let template = scoped_qualifier(rw, scope, donor, closure_path.segments())
        .expect("the Closure path has two segments");
    let instance = rw.donate(donor, Exp::TemplateInstance { base: template, args });
    rw.donate(donor, Exp::TypeIdentifier(scope.clone(), instance))
}

/// `mergeTemplateParams p1 p2 e` (`Function.hs:264-280`).
fn merge_template_params(rw: &mut Rewriter, p1: &[ExpId], p2: &[ExpId], e: ExpId) -> Exp {
    let offset = p1.len() as u32;
    // `paramMatches`: a `TypeParam` naming one of `p2` by name and index.
    let keys: Vec<(Symbol, u32)> = p2
        .iter()
        .filter_map(|&p| match &rw.arena[p].exp {
            Exp::TemplateParam { name, index, .. } => Some((*name, *index)),
            _ => None,
        })
        .collect();
    let mut reindex = |_rw: &mut Rewriter, _id: ExpId, exp: Exp| -> Exp {
        match exp {
            Exp::TypeParam { scope, kind, name, index } if keys.contains(&(name, index)) => {
                Exp::TypeParam { scope, kind, name, index: index + offset }
            }
            other => other,
        }
    };
    let mut params: Vec<ExpId> = Vec::with_capacity(p1.len() + p2.len());
    for &p in p1 {
        params.push(desugar(rw, p, &mut reindex));
    }
    for &p in p2 {
        let reindexed = desugar(rw, p, &mut reindex);
        let bumped = match rw.arena[reindexed].exp.clone() {
            Exp::TemplateParam { scope, kind, name, index, default } => {
                rw.replace(reindexed, Exp::TemplateParam { scope, kind, name, index: index + offset, default })
            }
            _ => reindexed,
        };
        params.push(bumped);
    }
    let decl = desugar(rw, e, &mut reindex);
    Exp::Template { params, decl }
}

#[cfg(test)]
mod tests {
    use kg_base::{FileId, Interner};
    use kg_syntax::{ast, AstNode};

    use crate::rewrite::higher_order_functions;
    use crate::{lower, pretty, ModuleIr};

    fn run(text: &str) -> (ModuleIr, Interner) {
        let interner = Interner::new();
        let parse = kg_syntax::parse_file(text);
        assert!(parse.errors().is_empty(), "probe must parse clean: {:?}", parse.errors());
        let file = ast::SourceFile::cast(parse.syntax_node()).expect("root");
        let lowered = lower(&file, FileId(0), &interner);
        let out = higher_order_functions(&lowered, &interner);
        (out, interner)
    }

    /// `Function.hs:321,326-360`, spelled as the printer spells it: two
    /// template parameters per function-typed parameter — the capture
    /// typename at `2i`, the closure function type at `2i + 1` with the
    /// capture parameter first — and the parameter retyped as the
    /// `Closure<…>` instance; the other parameter untouched.
    #[test]
    fn an_inline_higher_order_function_becomes_a_template() {
        let (ir, interner) = run("inline uint32 Go((uint32) -> uint32 Do, uint32 x)\n{\nreturn Do(x);\n}\n");
        let printed = pretty(&ir, &interner);
        assert!(
            printed.contains("template <typename Do$capture, (Do$capture, uint32) -> uint32 Do> inline uint32 Go(Closure<(Do$capture, uint32) -> uint32, Do, Do$capture> Do, uint32 x)"),
            "printed:\n{printed}"
        );
    }

    /// `:322`: a non-inline higher-order function is the reference's error;
    /// here it is left as written, pass 2 having reported it.
    #[test]
    fn a_non_inline_higher_order_function_is_left_as_written() {
        let (ir, interner) = run("uint32 Go((uint32) -> uint32 Do, uint32 x)\n{\nreturn Do(x);\n}\n");
        let printed = pretty(&ir, &interner);
        assert!(!printed.contains("template"), "printed:\n{printed}");
    }

    /// `mergeTemplateParams` (`:264-280`): a function that was a template
    /// keeps its own parameters first, the new ones after with their
    /// indexes offset, and the body's `TypeParam`s naming the new ones
    /// offset with them — `T` stays index 0, `Do$capture` is 1, `Do` 2.
    #[test]
    fn a_template_higher_order_function_merges_its_parameters() {
        let (ir, interner) = run("template <typename T>\ninline T Go((T) -> T Do, T x)\n{\nreturn Do(x);\n}\n");
        let printed = pretty(&ir, &interner);
        assert!(
            printed.contains("template <typename T, typename Do$capture, (Do$capture, T) -> T Do> inline T Go(Closure<(Do$capture, T) -> T, Do, Do$capture> Do, T x)"),
            "printed:\n{printed}"
        );
        let mut indexes: Vec<(String, u32)> = Vec::new();
        for (_, node) in ir.arena.iter() {
            if let crate::Exp::TemplateParam { name, index, .. } = &node.exp {
                indexes.push((interner.resolve(*name).to_string(), *index));
            }
        }
        indexes.sort();
        indexes.dedup();
        assert!(indexes.contains(&("Do$capture".to_owned(), 1)) && indexes.contains(&("Do".to_owned(), 2)), "{indexes:?}");
    }
}
