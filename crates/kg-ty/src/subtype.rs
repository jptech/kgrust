//! Owner-topic: **subtyping, convertibility and the least upper bound** —
//! `(<:)`, `tIsConvertibleTo`, `tAggregateInitializer`, `tSuper`, `tMux`, plus
//! the type-level half of `decltype` / `bitsizeof` / `bitoffsetof` / `clog2`
//! and the type-comparison operators.
//!
//! Spec: `docs/spec/types.md` §4 (all four), §5 (error types), §7 (the type
//! queries), §15 traps 4-7 and 11.
//! Reference: `Type.hs:306-366` (`tAggregateInitializer`, `<:`,
//! `tIsConvertibleTo`), `Type.hs:899-926` (`tSuper`, `tMux`),
//! `Constexpr.hs:244-250` (the comparison operators),
//! `Type.hs:375-416` (`instance Pretty Type`).
//!
//! # Traps that live in this file
//!
//! * `<:` on integers is **strict** (`x < y`), so `uint8 <: int8` is *false* and
//!   `uint8 <: int9` is true (`Type.hs:321-323`) — trap 4.
//! * `TAuto <: anything` is true, one-directionally (`Type.hs:349`) — trap 5.
//! * Arrays are **invariant** in the element type and require **equal** attribute
//!   sets (`Type.hs:344-348`) — trap 6.
//! * Function subtyping is contravariant in parameters and requires
//!   `attrs(target) ⊆ attrs(source)` (`Type.hs:330-334`) — trap 7.
//! * `tMux []` is `foldr1` on an empty list and **throws** (`Type.hs:926`) —
//!   trap 11.
//!
//! The clause order in `<:` is load-bearing: `Type.hs:317-352` is a first-match
//! cascade, and the two `TError` clauses come *first* so an error type is neither
//! a subtype nor a supertype of anything.
//!
//! # Attribute sets are compared by **subset**, so the container must be ordered
//!
//! `Type.hs:333-334` uses `Set.isSubsetOf` on both the per-parameter attribute
//! sets and the function's own. [`TAttrSet`] is a `BTreeSet<TAttr>` for exactly
//! that reason: a `HashSet` has no `Ord`, and `Ty`'s derived `Ord` — which
//! the template-instance map and `tSuper`'s `x == y` guard both
//! depend on — would stop deriving. `BTreeSet::is_subset` *is* `Set.isSubsetOf`.
//! `test/syntax/type-constexpr.k:92-104` pins the behaviour from both sides:
//! two attribute lists that differ only in **order** are the same type, and two
//! that differ in an argument value are unrelated.
//!
//! # Rendering types inside `TError` messages
//!
//! The reference embeds `showPretty t` in five of the messages produced here
//! (`docs/spec/types.md` §5). Rendering a nominal type needs the string table,
//! which `kg-ty` does not own, so [`show_pretty`] takes the interner as an
//! `Option` and renders a nominal name as `?` when it is absent — the same
//! trade-off `crate::width::expected_integer_error` already makes, and for the
//! same reason (every front-end diagnostic is `Error 1`, so only the *presence*
//! of the error is observable by the conformance corpus). Everything structural —
//! integers, arrays, function types, initializer lists — renders exactly as the
//! reference does with or without an interner.

use kg_base::{Interner, SmolStr};
use kg_const::{BinOp, ConstInt};

use crate::ty::{Name, QualifiedName, TArg, TAttr, TAttrSet, Ty, TyRef};
use crate::width::{t_offset_of, t_width};

// ---------------------------------------------------------------------------
// §4.1 — `(<:)`
// ---------------------------------------------------------------------------

/// `(<:)` — "is a subtype of" (`Type.hs:317-352`).
///
/// Transcribed **in clause order**; the definition is not a set of independent
/// rules. In particular `s == t` is clause 3, which is what makes every
/// non-integer type reflexive without an explicit case, and the two `TInstance`
/// clauses come before `TEnum`, so an instance of an enum is unwrapped as an
/// instance first.
///
/// Declared `infixr 4` (`Type.hs:352`); the relation is neither symmetric nor
/// (on integers) reflexive-by-rule.
pub fn is_subtype(sub: &Ty, sup: &Ty) -> bool {
    // 1-2. `TError` is neither a subtype nor a supertype of anything, *including
    //      itself* — these two clauses precede the `s == t` clause
    //      (`Type.hs:318-319`).
    if matches!(sub, Ty::Error(_)) || matches!(sup, Ty::Error(_)) {
        return false;
    }

    // 3. Structural equality (`Type.hs:320`). The only source of reflexivity.
    if sub == sup {
        return true;
    }

    // 4-6. The integer lattice, **strict** (`Type.hs:321-323`). Note there is no
    //      `TSigned <: TUnsigned` clause at all.
    match (sub, sup) {
        (Ty::Unsigned(x), Ty::Unsigned(y)) => return x < y,
        (Ty::Unsigned(x), Ty::Signed(y)) => return x < y,
        (Ty::Signed(x), Ty::Signed(y)) => return x < y,
        _ => {}
    }

    // 7-8. `const` is transparent on either side (`Type.hs:324-325`).
    if let Ty::Const(s) = sub {
        return is_subtype(s, sup);
    }
    if let Ty::Const(t) = sup {
        return is_subtype(sub, t);
    }

    // 9-10. A template instance compares as its concrete type (`Type.hs:326-327`).
    if sub.is_instance() {
        return is_subtype(sub.concrete(), sup);
    }
    if sup.is_instance() {
        return is_subtype(sub, sup.concrete());
    }

    // 11. An enum is a subtype of whatever its base is (`Type.hs:328`).
    //     One-directional: there is no `s <: TEnum` clause.
    if let Ty::Enum(_, s) = sub {
        return is_subtype(s, sup);
    }

    // 12. `[[pipelined]]` return types are transparent on the right
    //     (`Type.hs:329`).
    if let Ty::Dependent(t) = sup {
        return is_subtype(sub, t);
    }

    // 13. Functions: covariant return, CONTRAvariant parameters, and both
    //     attribute comparisons run target-⊆-source (`Type.hs:330-334`).
    if let (
        Ty::Function { attrs: a1, ret: r1, params: p1, .. },
        Ty::Function { attrs: a2, ret: r2, params: p2, .. },
    ) = (sub, sup)
    {
        // `liftEq` on lists demands equal length as well as pairwise agreement.
        return (matches!(**r2, Ty::Auto) || is_subtype(r1, r2))
            && p2.len() == p1.len()
            && p2.iter().zip(p1.iter()).all(|(x, y)| is_subtype(&x.ty, &y.ty))
            && p2.iter().zip(p1.iter()).all(|(x, y)| x.attrs.is_subset(&y.attrs))
            && a2.is_subset(a1);
    }

    // 14-18. Initializer lists (`Type.hs:335-339`). The empty-list cases come
    //        first, and only then the general aggregate rule.
    if let Ty::Initializer(elements) = sub {
        if elements.is_empty()
            && (matches!(sup, Ty::Boolean | Ty::Float) || sup.is_int() || sup.is_enum())
        {
            return true;
        }
        return aggregate_initializer(&is_subtype, sub, sup);
    }

    // 19. Closures (`Type.hs:340-343`): same lambda, subtype function type,
    //     subtype capture. The closure *alias* name is not compared.
    //
    //     "Same lambda" is compared by the function's own name, the last
    //     segment: the reference's names are the parser's resolved qualified
    //     names, one per declaration, where this port's are the path a twin
    //     writes — `?lambda0` at the cast `desugarLambdas` wrote, `main::$::
    //     ?lambda0` where a copy's parameter was substituted with the hoisted
    //     function's qualified name — two spellings of one declaration
    //     (`static-if.k::44`, 2026-09-19). Two lambdas of one name in two
    //     scopes compare equal here: an accept the reference would refuse,
    //     never an error it would not raise.
    //
    //     And one copy is one type. A closure's copy is named by its function
    //     and its capture alone — the function type mangles to nothing
    //     (`Mangle.hs:42-43`) — and every mention of the copy is typed off
    //     the copy's own note (`infer AliasF`, `Type.hs:1175-1177`), so by
    //     the time a conversion is checked `Closure<(C, auto) -> R, f, C>`
    //     and `Closure<(C, uint32) -> R, f, C>` are one declaration and
    //     `s == t` (clause 3). A generic lambda given to `(uint32) -> R Do`
    //     is the case (`higher-order-functions.k::35`); each twin here types
    //     its own mention from its own instance.
    if let (
        Ty::Closure { func: n1, func_ty: f1, capture: c1, .. },
        Ty::Closure { func: n2, func_ty: f2, capture: c2, .. },
    ) = (sub, sup)
    {
        return n1.last() == n2.last() && (c1 == c2 || is_subtype(f1, f2) && is_subtype(c1, c2));
    }

    // 20. Arrays (`Type.hs:344-348`): EQUAL attribute sets, matching (or absent)
    //     first dimension, and an INVARIANT element type. Note the RHS pattern is
    //     `TArray a2 _ (d2:_)`, so a target with no dimensions never matches.
    if let (Ty::Array { attrs: a1, dims: ds, .. }, Ty::Array { attrs: a2, dims: dt, .. }) =
        (sub, sup)
        && let Some(d2) = dt.first()
    {
        return a1 == a2
            && (ds.is_empty() || &ds[0] == d2)
            // `tElement` on a one-dimensional ECC memory substitutes the ECC
            // function's return type — trap 14 — and that substitution is
            // part of array subtyping.
            && is_subtype(&sub.element(None), &sup.element(None))
            && is_subtype(&sup.element(None), &sub.element(None));
    }

    // 21. `auto` is below everything (`Type.hs:349`). One-directional: nothing is
    //     a subtype of `auto` except by clause 3.
    if matches!(sub, Ty::Auto) {
        return true;
    }

    // 22. `Type.hs:350`.
    false
}

// ---------------------------------------------------------------------------
// §4.2 — `tIsConvertibleTo`
// ---------------------------------------------------------------------------

/// `tIsConvertibleTo` (`Type.hs:354-366`).
///
/// A strict superset of [`is_subtype`]: it starts with `s <: t`, then adds
/// `const` stripping on both sides, `TInstance` unwrapping, aggregate
/// initializers, enum bases, `TDependent`, and finally **any integer to any
/// integer** (`Type.hs:364-365`). That last clause is why narrowing is a
/// *warning*, not an error (`docs/spec/types.md` §6).
pub fn is_convertible_to(from: &Ty, to: &Ty) -> bool {
    // 1. `Type.hs:355-356`.
    if is_subtype(from, to) {
        return true;
    }
    // 2-3. `Type.hs:357-358`.
    if let Ty::Const(s) = from {
        return is_convertible_to(s, to);
    }
    if let Ty::Const(t) = to {
        return is_convertible_to(from, t);
    }
    // 4-5. `Type.hs:359-360`.
    if from.is_instance() {
        return is_convertible_to(from.concrete(), to);
    }
    if to.is_instance() {
        return is_convertible_to(from, to.concrete());
    }
    // 6. `Type.hs:361` — note this comes *before* the enum clause, unlike `<:`
    //    where the initializer clauses come after it.
    if matches!(from, Ty::Initializer(_)) {
        return aggregate_initializer(&is_convertible_to, from, to);
    }
    // 7-8. `Type.hs:362-363`.
    if let Ty::Enum(_, s) = from {
        return is_convertible_to(s, to);
    }
    if let Ty::Dependent(t) = to {
        return is_convertible_to(from, t);
    }
    // 9. The escape hatch (`Type.hs:364-365`).
    if from.is_int() && to.is_int() {
        return true;
    }
    // 10. `Type.hs:366`.
    false
}

// ---------------------------------------------------------------------------
// §4.3 — aggregate initializers
// ---------------------------------------------------------------------------

/// `tAggregateInitializer` (`Type.hs:306-314`), parameterised by the relation to
/// use on each element — `(<:)` from [`is_subtype`], `tIsConvertibleTo` from
/// [`is_convertible_to`].
///
/// Note the first clause: an initializer with **more than one** element can never
/// initialize a union (`Type.hs:307-308`), and that check fires before the
/// element-wise walk.
///
/// The `TError`s that [`initialized_type`] can return here are inert: both
/// relations reject a `TError` operand outright (`Type.hs:318-319`, and clause 9
/// of `tIsConvertibleTo` needs two integers), so only the *presence* of an error
/// changes the answer, never its text. That is what lets this call the
/// interner-free form; the test `interner_free_messages_are_inert` pins it.
pub fn aggregate_initializer(
    rel: &dyn Fn(&Ty, &Ty) -> bool,
    initializer: &Ty,
    target: &Ty,
) -> bool {
    // `tAggregateInitializer _ _ _ = False` (`Type.hs:314`) — anything that is
    // not an initializer list.
    let Ty::Initializer(elements) = initializer else {
        return false;
    };
    // `Type.hs:307-308`.
    if target.is_union() && elements.len() > 1 {
        return false;
    }
    // `Type.hs:309-313`.
    if target.is_union() || target.is_struct() || target.is_class() || target.is_array() {
        return elements
            .iter()
            .all(|e| rel(&initializer_type(e), &initialized_type(target, e)));
    }
    false
}

/// `tInitializerType` (`Type.hs:288-291`) — one element's *value* type.
///
/// The reference is **partial**: `undefined` on anything but a `TDesignator` or a
/// `TPositional` (trap 13). [`Ty::Unresolved`] is returned instead, matching what
/// its sibling `tPositionalType` does for the same shape mismatch
/// (`Type.hs:284-286`). Reachable in practice only while the fixpoint still has
/// unresolved elements, since every parsed element is a `PositionalF` or a
/// `DesignatorF` (`Parser.hs:857-864`, `Type.hs:1201`, `Type.hs:1248`).
pub fn initializer_type(element: &Ty) -> Ty {
    match element {
        Ty::Designator(_, t) | Ty::Positional(_, t) => (**t).clone(),
        _ => Ty::Unresolved,
    }
}

/// `tInitializedType` (`Type.hs:293-304`): given the target aggregate type and
/// one element's `TPositional`/`TDesignator` type, the type that element is
/// initializing.
///
/// The `showPretty` inside two of the messages is rendered without an interner —
/// see the module header. Use [`initialized_type_pretty`] where one is available.
pub fn initialized_type(target: &Ty, element: &Ty) -> Ty {
    initialized_type_pretty(target, element, None)
}

/// [`initialized_type`] with the string table available, so the two
/// ``… type `T` `` messages render their type the way `Type.hs:300` and
/// `Type.hs:303` do.
pub fn initialized_type_pretty(target: &Ty, element: &Ty, names: Option<&Interner>) -> Ty {
    // `tInitializedType (TConst t) x = tInitializedType t x` (`Type.hs:294`).
    // Note it strips only `TConst`; `TInstance` is looked through by the
    // predicates and accessors below instead.
    if let Ty::Const(t) = target {
        return initialized_type_pretty(t, element, names);
    }

    match element {
        // `Type.hs:295-300`.
        Ty::Positional(index, _) => {
            let index = *index;
            let is_array = target.is_array();
            // `x == tDimension t`, guarded by `tIsArray t` because `tDimension`
            // is `undefined` otherwise (`Type.hs:537-541`). An array with *no*
            // dimensions makes `tDimension` throw in the reference; here it is
            // `None`, which simply fails the equality.
            if is_array
                && target.dimension() == Some(&ConstInt::from_usize(index as usize))
            {
                return Ty::Error(SmolStr::new_static("Excess elements in array initializer"));
            }
            if target.is_union() && index != 0 {
                return Ty::Error(SmolStr::new_static("Excess elements in union initializer"));
            }
            if is_array {
                return target.element(names);
            }
            if target.is_union() || target.is_struct() || target.is_class() {
                return target.field_at(index as usize, names);
            }
            // No backticks: `Type.hs:300` concatenates `showPretty t` directly.
            // `docs/spec/types.md` §5 adds them; the Haskell does not.
            Ty::Error(SmolStr::new(format!(
                "Non-empty initializer list cannot initialize type {}",
                show_pretty(target, names)
            )))
        }
        // `Type.hs:301-303`.
        Ty::Designator(field, _) => {
            if target.is_union() || target.is_struct() || target.is_class() {
                return target.field(*field, names);
            }
            // No backticks — `Type.hs:303`, as above.
            Ty::Error(SmolStr::new(format!(
                "Member designator cannot initialize type {}",
                show_pretty(target, names)
            )))
        }
        // `tInitializedType _ _ = undefined` (`Type.hs:304`).
        _ => Ty::Unresolved,
    }
}

// ---------------------------------------------------------------------------
// §4.4 — `tSuper` / `tMux`
// ---------------------------------------------------------------------------

/// `tSuper` — the least upper bound (`Type.hs:899-923`).
///
/// Guards first: an unresolved operand on either side gives [`Ty::Unresolved`],
/// and `x == y` gives `x`. Then the integer lattice (`max` of the widths, with
/// `tSigned` promotion for mixed signedness), enum bases, instances, positional /
/// designator element-wise lifting, initializer convertibility in both
/// directions, and finally element-wise `tSuper` over two initializers with the
/// longer list's tail appended. Anything else is
/// ``TError "Types `x` and `y` have no common supertype"``.
///
/// There is deliberately **no** `TConst` clause: every caller pre-strips it
/// (`tMux` is applied to `tUnConst . typeOf`, `Type.hs:1239`; `inferBinaryOp`
/// goes through `typeOfOperand`, `Type.hs:732-738`), so `tSuper (const uint8)
/// uint8` really does fall through to the error clause.
pub fn t_super(lhs: &Ty, rhs: &Ty) -> Ty {
    t_super_pretty(lhs, rhs, None)
}

/// [`t_super`] with the string table available, so the "no common supertype"
/// message renders its operands the way `Type.hs:923` does.
pub fn t_super_pretty(lhs: &Ty, rhs: &Ty, names: Option<&Interner>) -> Ty {
    // 1-3. `Type.hs:900-902`.
    if !lhs.resolved() || !rhs.resolved() {
        return Ty::Unresolved;
    }
    if lhs == rhs {
        return lhs.clone();
    }

    // 4-7. `Type.hs:903-906`. Mixed signedness promotes the *unsigned* side with
    //      `tSigned` (`uintN` → `int(N+1)`) and recurses.
    match (lhs, rhs) {
        (Ty::Unsigned(x), Ty::Unsigned(y)) => return Ty::Unsigned(*x.max(y)),
        (Ty::Signed(x), Ty::Signed(y)) => return Ty::Signed(*x.max(y)),
        (Ty::Signed(_), Ty::Unsigned(_)) => {
            return t_super_pretty(lhs, &rhs.to_signed(), names);
        }
        (Ty::Unsigned(_), Ty::Signed(_)) => {
            return t_super_pretty(&lhs.to_signed(), rhs, names);
        }
        _ => {}
    }

    // 8-9. `Type.hs:907-908`. The pattern is a bare `TEnum{}`; a `const` enum
    //      does *not* reach here.
    if matches!(lhs, Ty::Enum(..)) {
        return t_super_pretty(&lhs.enum_base(), rhs, names);
    }
    if matches!(rhs, Ty::Enum(..)) {
        return t_super_pretty(lhs, &rhs.enum_base(), names);
    }

    // 10-11. `Type.hs:909-910` — one level of `TInstance`, not `tConcrete`.
    if let Ty::Instance { concrete, .. } = lhs {
        return t_super_pretty(concrete, rhs, names);
    }
    if let Ty::Instance { concrete, .. } = rhs {
        return t_super_pretty(lhs, concrete, names);
    }

    // 12-13. `Type.hs:911-914` — element-wise lifting, only for matching slots.
    if let (Ty::Positional(l, x), Ty::Positional(r, y)) = (lhs, rhs)
        && l == r
    {
        return Ty::Positional(*l, TyRef::new(t_super_pretty(x, y, names)));
    }
    if let (Ty::Designator(l, x), Ty::Designator(r, y)) = (lhs, rhs)
        && l == r
    {
        return Ty::Designator(*l, TyRef::new(t_super_pretty(x, y, names)));
    }

    // 14-15. `Type.hs:915-918` — an initializer that can already initialize the
    //        other side collapses to that side.
    if matches!(lhs, Ty::Initializer(_)) && is_convertible_to(lhs, rhs) {
        return rhs.clone();
    }
    if matches!(rhs, Ty::Initializer(_)) && is_convertible_to(rhs, lhs) {
        return lhs.clone();
    }

    // 16. `Type.hs:919-922` — `zipWith tSuper` plus the tail of whichever list is
    //     longer. The two guarded branches differ only in which tail is taken.
    if let (Ty::Initializer(xs), Ty::Initializer(ys)) = (lhs, rhs) {
        let common = xs.len().min(ys.len());
        let mut out: Vec<TyRef> = xs
            .iter()
            .zip(ys.iter())
            .map(|(a, b)| TyRef::new(t_super_pretty(a, b, names)))
            .collect();
        if xs.len() >= ys.len() {
            out.extend(xs[common..].iter().cloned());
        } else {
            out.extend(ys[common..].iter().cloned());
        }
        return Ty::Initializer(out);
    }

    // 17. `Type.hs:923`.
    Ty::Error(SmolStr::new(format!(
        "Types `{}` and `{}` have no common supertype",
        show_pretty(lhs, names),
        show_pretty(rhs, names)
    )))
}

/// `tMux = foldr1 tSuper` (`Type.hs:925-926`).
///
/// Trap 11: the reference **throws** on an empty list. Whether `mux` with zero or
/// one arm is even reachable is `docs/spec/types.md` open question 3 — the parser
/// uses `sepBy1` (`Parser.hs:870`). `None` here rather than a panic.
pub fn t_mux<'a>(arms: impl IntoIterator<Item = &'a Ty>) -> Option<Ty> {
    t_mux_pretty(arms, None)
}

/// [`t_mux`] with the string table, so a "no common supertype" between two
/// named types spells them (`basics.k::199`: `Bar` and `Foo`, not `?`).
pub fn t_mux_pretty<'a>(arms: impl IntoIterator<Item = &'a Ty>, names: Option<&Interner>) -> Option<Ty> {
    // `foldr1 f [a,b,c] = f a (f b c)`. `tSuper` is not associative (its error
    // messages differ, and the initializer clauses are order-sensitive), so the
    // association has to match.
    let mut arms: Vec<&Ty> = arms.into_iter().collect();
    let last = arms.pop()?.clone();
    Some(arms.into_iter().rev().fold(last, |acc, ty| t_super_pretty(ty, &acc, names)))
}

// ---------------------------------------------------------------------------
// §7 — `decltype`, `bitsizeof`, `bitoffsetof`, `clog2`, type comparison
// ---------------------------------------------------------------------------

/// `infer (DecltypeF _ a) = TType $ typeOf a` (`Type.hs:1198`).
///
/// `decltype` is a *type-forming* expression, so it is resolved by inference, not
/// by folding. The fold that follows replaces the node with the corresponding
/// type expression via `expFromType` (`Constexpr.hs:304-305`); when no such
/// expression exists the reference reports
/// ``"Argument of type `T` is illegal for `decltype`"`` (`Constexpr.hs:345`), and
/// a `DecltypeF` that survives to `validateProgram` becomes
/// `"Couldn't deduce type of the expression"` (`TypeCheck.hs:53`).
pub fn decltype(operand: &Ty) -> Ty {
    Ty::Type(TyRef::new(operand.clone()))
}

/// ``"Argument of type `T` is illegal for `decltype`"`` (`Constexpr.hs:345`).
pub fn illegal_decltype_message(operand: &Ty, names: Option<&Interner>) -> String {
    format!("Argument of type `{}` is illegal for `decltype`", show_pretty(operand, names))
}

/// `sizeOf = tWidth . typeOf` (`Type.hs:467`) — what `bitsizeof e` folds to
/// (`Constexpr.hs:271-272`).
///
/// Inference gives the node [`Ty::Unresolved`] (`Type.hs:749`); `None` here means
/// the fold leaves the node alone and it is reported as unresolved later.
/// The result is an **untyped** integer literal, so it re-derives its width every
/// round (`untypedIntLiteral`, `Constexpr.hs:322`).
pub fn bit_sizeof(operand: &Ty) -> Option<u64> {
    t_width(operand)
}

/// `bytesizeof e` (`Constexpr.hs:274-275` via `bytesFromBits`,
/// `Constexpr.hs:349-351`), inference at `Type.hs:750`.
///
/// * `None` — no width; the node is left alone.
/// * `Some(Ok(n))` — the width is a multiple of 8 and folds to `width >> 3`.
/// * `Some(Err(msg))` — ``"bytesizeof is undefined for a type with bit size of N"``.
pub fn byte_sizeof(operand: &Ty) -> Option<Result<u64, String>> {
    let bits = t_width(operand)?;
    Some(bytes_from_bits("bytesizeof is undefined for a type with bit size of ", bits))
}

/// `offsetOf x = tOffsetOf x . theType` (`Type.hs:470-471`) — what
/// `bitoffsetof(T, f)` folds to (`Constexpr.hs:162-163`), with inference leaving
/// the node [`Ty::Unresolved`] (`Type.hs:1189`).
///
/// The operand is the type of the *first* argument, i.e. a `TType`; `theType`
/// unwraps it and yields ``TError "Expected a type"`` when it is not one
/// (`Type.hs:453-460`), which then has no offset. `None` therefore covers both
/// "not a struct" and "no such field", exactly as `Maybe` does in the reference;
/// the caller renders it as
/// ``"Illegal operands for bitoffsetof: T, f"`` (`Constexpr.hs:347`) or, from the
/// `unresolved` pass, as `"Undefined field"` / `"Field offset is only defined for
/// structs"` (`Frontend.hs:446-449`).
pub fn bit_offset_of(operand: &Ty, field: Name) -> Option<u64> {
    t_offset_of(&operand.the_type(), field)
}

/// `byteoffsetof(T, f)` (`Constexpr.hs:165-166`), inference at `Type.hs:1190`.
/// Same shape as [`byte_sizeof`]; the message is
/// ``"byteoffsetof is undefined for fields with bit offset of N"``.
pub fn byte_offset_of(operand: &Ty, field: Name) -> Option<Result<u64, String>> {
    let bits = bit_offset_of(operand, field)?;
    Some(bytes_from_bits("byteoffsetof is undefined for fields with bit offset of ", bits))
}

/// `bytesFromBits` (`Constexpr.hs:349-351`): `x .&. 7 == 0` folds to `x >> 3`,
/// anything else is `s ++ show x`.
fn bytes_from_bits(message: &str, bits: u64) -> Result<u64, String> {
    if bits & 7 == 0 { Ok(bits >> 3) } else { Err(format!("{message}{bits}")) }
}

/// `clog2(e)` (`Constexpr.hs:168-169`).
///
/// `fromNaturalConstexpr = find (> 0) . fromIntegralConstexpr`
/// (`Type.hs:1102-1103`) — the argument must be a **strictly positive** integer
/// constant, so `0` and negatives are `None` and become
/// `"clog2 argument did not evaluate to a positive integer constant"` in the
/// `unresolved` pass (`Frontend.hs:403`). Inference leaves the node
/// [`Ty::Unresolved`] (`Type.hs:751`), and the fold produces an *untyped*
/// literal.
pub fn clog2(value: &ConstInt) -> Option<u32> {
    if !value.is_positive() {
        return None;
    }
    value.checked_clog2()
}

/// The type-comparison operators — `evalBinaryType` (`Constexpr.hs:244-250`).
///
/// `s` and `t` are the **unwrapped** types (`theType x`, `theType y`); use
/// [`compare_type_operands`] when you have the operands' node types instead.
/// `None` is `evalBinaryType _ _ _ = e` (`Constexpr.hs:250`): the operator is not
/// a comparison and the node is left alone.
///
/// Every entry is defined in terms of [`is_subtype`], so `==` here is
/// *mutual subtyping*, which is strictly weaker than `Eq Type` for
/// `TConst`/`TInstance`/`TDependent` wrappers and, because `<:` is strict on
/// integers, coincides with structural equality for scalars.
///
/// Pinned by `test/syntax/type-constexpr.k:4-62`.
pub fn compare_types(op: BinOp, s: &Ty, t: &Ty) -> Option<bool> {
    Some(match op {
        BinOp::Equal => is_subtype(s, t) && is_subtype(t, s),
        BinOp::NotEqual => !is_subtype(s, t) || !is_subtype(t, s),
        BinOp::LessEqual => is_subtype(s, t),
        BinOp::Less => is_subtype(s, t) && !is_subtype(t, s),
        BinOp::GreaterEqual => is_subtype(t, s),
        BinOp::Greater => is_subtype(t, s) && !is_subtype(s, t),
        _ => return None,
    })
}

/// The guarded entry point: `evalBinary x y | tIsType (typeOf x) && tIsType
/// (typeOf y) = evalBinaryType (theType x) (theType y)` (`Constexpr.hs:236-238`).
///
/// This is the shape `kg_const::ConstEnv::compare_types` needs: hand it the two
/// operands' node types and it applies both the `tIsType` guard and `theType`.
pub fn compare_type_operands(op: BinOp, lhs: &Ty, rhs: &Ty) -> Option<bool> {
    if !(lhs.is_type() && rhs.is_type()) {
        return None;
    }
    compare_types(op, &lhs.the_type(), &rhs.the_type())
}

// ---------------------------------------------------------------------------
// `instance Pretty Type` (`Type.hs:375-416`)
// ---------------------------------------------------------------------------

/// `showPretty = show . pretty` over a [`Ty`] (`Type.hs:376-401`,
/// `Type.hs:427-428`).
///
/// `names` resolves the components of a [`QualifiedName`]. When it is `None`
/// every nominal name renders as `?`; everything else is byte-identical to the
/// reference. See the module header for why the seam exists.
///
/// This is **`showPretty`, not `showUnmangled`**: `showUnmangled` (`TypeCheck.hs:36`,
/// `Type.hs:151-176`) additionally walks the symbol map to turn a mangled
/// template-instance component back into `Name<args>`, which needs the
/// program's symbols: [`show_unmangled_with`] takes them.
pub fn show_pretty(ty: &Ty, names: Option<&Interner>) -> String {
    let mut out = String::new();
    pretty(ty, names, &mut out);
    out
}

/// `showPretty` of a template argument (`instance Pretty TArg`,
/// `Type.hs:217-222`).
pub fn show_pretty_arg(arg: &TArg, names: Option<&Interner>) -> String {
    let mut out = String::new();
    pretty_arg(arg, names, &mut out);
    out
}

/// `showUnmangled = show . unmangle symbols` (`TypeCheck.hs:36`,
/// `Constexpr.hs:131`): a type spelled for a message, every name through
/// the symbol map `declared` — an instance by its arguments
/// (`unmangleInstanceName`), anything else by its qualified name with the
/// module namespace unmangled (`Type.hs:148-205`). Read off the structure,
/// never off a mangled name.
pub fn show_unmangled_with(ty: &Ty, names: Option<&Interner>, declared: Declared<'_>) -> String {
    let mut out = String::new();
    unmangle_with(ty, names, declared, &mut out);
    out
}

/// The symbol map `unmangleQualifiedName` reads (`Type.hs:180-187`), as
/// the type of the declaration a qualified name names — `None` where
/// nothing is declared under it.
pub type Declared<'a> = &'a dyn Fn(&[Name]) -> Option<Ty>;

/// `unmangle s` (`Type.hs:148-164`): a type spelled for the backend, every
/// named type through [`unmangle_qualified_name`], so a copy nested in
/// another's arguments — `make_capture2<Closure<…>, …>::$::Capture` — is
/// spelled from its own instance, function type and all, where its mangled
/// name holds none (`Mangle.hs:42-43`).
pub fn unmangle_with(ty: &Ty, names: Option<&Interner>, declared: Declared<'_>, out: &mut String) {
    unmangle_at(ty, names, declared, out, 0);
}

/// How deep [`unmangle_with`] follows the map before it reads a name back
/// from its mangle instead: a copy's arguments can name, through a path,
/// a copy whose arguments name it again.
const UNMANGLE_DEPTH: u32 = 6;

fn unmangle_at(ty: &Ty, names: Option<&Interner>, declared: Declared<'_>, out: &mut String, depth: u32) {
    match ty {
        Ty::Type(t) => {
            out.push_str("type ");
            unmangle_at(t, names, declared, out, depth);
        }
        Ty::Const(t) => {
            out.push_str("const ");
            unmangle_at(t, names, declared, out, depth);
        }
        Ty::Reference(x) => {
            unmangle_qualified_name(x.segments(), names, declared, out, depth);
            out.push('&');
        }
        Ty::Array { attrs, elem, dims } => {
            format_array_by(attrs, elem, dims, names, out, &|t, out| unmangle_at(t, names, declared, out, depth));
        }
        Ty::Function { attrs, ret, params, .. } => {
            pretty_attrs(attrs, names, out);
            out.push('(');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                pretty_attrs(&p.attrs, names, out);
                unmangle_at(&p.ty, names, declared, out, depth);
            }
            out.push_str(") -> ");
            unmangle_at(ret, names, declared, out, depth);
        }
        Ty::Enum(x, _) | Ty::Struct(x, _) | Ty::Union(x, _) | Ty::Class(x, _) => {
            unmangle_qualified_name(x.segments(), names, declared, out, depth);
        }
        // A closure whose copy is made, without the instance face the
        // reference's type keeps round it (`TInstance (TClosure …)`): spelled
        // as that copy, which the map declares as the instance.
        Ty::Closure { alias, .. }
            if alias.first().is_some_and(|ns| resolve(ns, names).starts_with('@')) =>
        {
            unmangle_qualified_name(alias.segments(), names, declared, out, depth);
        }
        Ty::Instance { concrete, template, .. } => {
            if concrete.is_function() {
                unmangle_at(concrete, names, declared, out, depth);
            } else if concrete.is_error() || !concrete.resolved() {
                pretty(ty, names, out);
            } else {
                let segments = template.segments();
                if segments.len() > 1 {
                    unmangle_qualified_name(&segments[..segments.len() - 1], names, declared, out, depth);
                    out.push_str("::");
                }
                out.push_str(&instance_name_at(ty, names, declared, depth).unwrap_or_default());
            }
        }
        other => pretty(other, names, out),
    }
}

/// `unmangleInstanceName s` (`Type.hs:176-177`) with the symbol map: the
/// template's leaf, then every argument through [`unmangle_with`].
pub fn unmangle_instance_name_with(ty: &Ty, names: Option<&Interner>, declared: Declared<'_>) -> Option<String> {
    instance_name_at(ty, names, declared, 0)
}

fn instance_name_at(ty: &Ty, names: Option<&Interner>, declared: Declared<'_>, depth: u32) -> Option<String> {
    let Ty::Instance { template, args, .. } = ty else { return None };
    let mut out = resolve(*template.segments().last()?, names).to_string();
    out.push('<');
    for (i, (_, a)) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match a {
            TArg::Type(t) => unmangle_at(t, names, declared, &mut out, depth),
            TArg::Name(_, n) => unmangle_qualified_name(n.segments(), names, declared, &mut out, depth),
            other => pretty_arg(other, names, &mut out),
        }
    }
    out.push('>');
    Some(out)
}

/// `unmangleQualifiedName symbols = hcat . punctuate "::" . map unmangleName
/// . tail . inits` (`Type.hs:180-187`): each prefix of the name that the
/// map declares as an instance is spelled as that instance, any other by
/// its last segment, a module namespace unmangled — never read back from
/// a mangled name.
fn unmangle_qualified_name(name: &[Name], names: Option<&Interner>, declared: Declared<'_>, out: &mut String, depth: u32) {
    for i in 0..name.len() {
        if i > 0 {
            out.push_str("::");
        }
        let prefix = &name[..=i];
        let instance = (depth < UNMANGLE_DEPTH).then(|| declared(prefix)).flatten().map(|t| match t {
            Ty::Type(inner) => (*inner).clone(),
            other => other,
        });
        if let Some(t @ Ty::Instance { .. }) = &instance {
            out.push_str(&instance_name_at(t, names, declared, depth + 1).unwrap_or_default());
            continue;
        }
        out.push_str(&unmangle_module_namespace(&resolve(name[i], names)));
    }
}

fn pretty(ty: &Ty, names: Option<&Interner>, out: &mut String) {
    match ty {
        Ty::Auto => out.push_str("auto"),
        Ty::Type(a) => {
            out.push_str("type ");
            pretty(a, names, out);
        }
        Ty::Template => out.push_str("template"),
        // `pretty TUntyped = ""` (`Type.hs:380`).
        Ty::Untyped => {}
        Ty::Undefined => out.push_str("undefined"),
        Ty::Unresolved => out.push_str("unresolved"),
        // `braces (sepBy comma ts)` — `sepBy` puts a space after each comma.
        Ty::Initializer(ts) => {
            out.push('{');
            for (i, t) in ts.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                pretty(t, names, out);
            }
            out.push('}');
        }
        Ty::Void => out.push_str("void"),
        Ty::Boolean => out.push_str("bool"),
        Ty::Designator(x, t) => {
            out.push('.');
            out.push_str(&resolve(*x, names));
            out.push_str(" = ");
            pretty(t, names, out);
        }
        // The index is dropped (`Type.hs:384`).
        Ty::Positional(_, t) => pretty(t, names, out),
        // `float`, not `float32` (`Type.hs:386`).
        Ty::Float => out.push_str("float"),
        Ty::Str => out.push_str("string"),
        Ty::Signed(n) => {
            out.push_str("int");
            out.push_str(&n.to_string());
        }
        Ty::Unsigned(n) => {
            out.push_str("uint");
            out.push_str(&n.to_string());
        }
        // `TDependent` is invisible (`Type.hs:390`).
        Ty::Dependent(t) => pretty(t, names, out),
        Ty::Const(t) => {
            out.push_str("const ");
            pretty(t, names, out);
        }
        Ty::Reference(x) => {
            pretty_qualified_name(x, names, out);
            out.push('&');
        }
        Ty::Array { attrs, elem, dims } => format_array(attrs, elem, dims, names, out),
        Ty::Function { attrs, ret, params, .. } => {
            pretty_attrs(attrs, names, out);
            out.push('(');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                // `instance Pretty TFuncParam` (`Type.hs:189-190`) — attributes
                // then type; the parameter *name* is never printed.
                pretty_attrs(&p.attrs, names, out);
                pretty(&p.ty, names, out);
            }
            out.push_str(") -> ");
            pretty(ret, names, out);
        }
        Ty::Closure { func, capture, .. } => {
            out.push('[');
            pretty(capture, names, out);
            out.push_str("] ");
            pretty_qualified_name(func, names, out);
        }
        Ty::Enum(x, _) | Ty::Struct(x, _) | Ty::Union(x, _) | Ty::Class(x, _) => {
            pretty_qualified_name(x, names, out);
        }
        // `Type.hs:397-401`.
        Ty::Instance { concrete, template, args } => {
            if concrete.is_function() || concrete.is_error() || !concrete.resolved() {
                pretty(concrete, names, out);
            } else {
                pretty_qualified_name(template, names, out);
                out.push('<');
                for (i, (_, a)) in args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    pretty_arg(a, names, out);
                }
                out.push('>');
            }
        }
        Ty::Error(e) => out.push_str(e),
    }
}

/// `prettyAttr` (`Type.hs:371-374`): `[[a,b]] ` with a **trailing space**, or
/// nothing at all when the set is empty. `commaSep = catBy ","` puts no space
/// after the comma, unlike `sepBy comma`.
fn pretty_attrs(attrs: &TAttrSet, names: Option<&Interner>, out: &mut String) {
    if attrs.is_empty() {
        return;
    }
    out.push_str("[[");
    for (i, a) in attrs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        pretty_attr(a, names, out);
    }
    out.push_str("]] ");
}

/// `instance Pretty TAttr` (`Type.hs:200-203`). Note the qualified name in the
/// `TAttr` form is joined with `::` **without** unmangling, unlike
/// `prettyQualifiedName`.
/// `showPretty` of one attribute (`Type.hs:200-215`) — `[[x]]`'s `x`.
pub fn show_attr(attr: &TAttr, names: Option<&Interner>) -> String {
    let mut out = String::new();
    pretty_attr(attr, names, &mut out);
    out
}

fn pretty_attr(attr: &TAttr, names: Option<&Interner>, out: &mut String) {
    match attr {
        TAttr::Flag(f) => out.push_str(f.text()),
        TAttr::Int(a, n) => {
            out.push_str(a.text());
            out.push('(');
            out.push_str(&n.to_str_radix(10));
            out.push(')');
        }
        TAttr::Named(a, name, _) => {
            out.push_str(a.text());
            out.push('(');
            out.push_str(&join_raw(name, names));
            out.push(')');
        }
    }
}

/// `instance Pretty TArg` (`Type.hs:217-222`).
fn pretty_arg(arg: &TArg, names: Option<&Interner>, out: &mut String) {
    match arg {
        TArg::Expr(s) | TArg::Str(s) => out.push_str(s),
        TArg::Type(t) => pretty(t, names, out),
        TArg::Int(n) => out.push_str(&n.to_str_radix(10)),
        TArg::Name(_, n) => out.push_str(&join_raw(n, names)),
    }
}

/// `prettyDimensions` (`Type.hs:403-406`): `[]` when empty, else `[2][3]`.
fn pretty_dimensions(dims: &[ConstInt], out: &mut String) {
    if dims.is_empty() {
        out.push_str("[]");
        return;
    }
    for d in dims {
        out.push('[');
        out.push_str(&d.to_str_radix(10));
        out.push(']');
    }
}

/// `formatArray` (`Type.hs:177-183`).
///
/// The reference's `null (init ns)` guard calls `init` on a possibly-empty list,
/// which throws. It is only reachable with a non-empty attribute set and no
/// dimensions — an unresolved array (`Type.hs:1019`) — and is treated here as
/// "at most one dimension", the same branch a single-dimension array takes.
fn format_array(attrs: &TAttrSet, elem: &Ty, dims: &[ConstInt], names: Option<&Interner>, out: &mut String) {
    format_array_by(attrs, elem, dims, names, out, &|t, out| pretty(t, names, out));
}

/// [`format_array`] with the element printed by `elem_fn` — `formatArray fn`
/// (`Type.hs:166-174`) takes the printer as its first argument, `pretty` or
/// `unmangle s`.
fn format_array_by(
    attrs: &TAttrSet,
    elem: &Ty,
    dims: &[ConstInt],
    names: Option<&Interner>,
    out: &mut String,
    elem_fn: &dyn Fn(&Ty, &mut String),
) {
    // The two nested-array clauses (`Type.hs:178-181`) flatten `T[a][b]` written
    // as an array of arrays back into one dimension list when the outer array
    // carries no attributes of its own.
    if let Ty::Array { attrs: inner_attrs, elem: inner_elem, dims: inner_dims } = elem {
        if attrs.is_empty() {
            let mut merged = dims.to_vec();
            merged.extend(inner_dims.iter().cloned());
            format_array_by(inner_attrs, inner_elem, &merged, names, out, elem_fn);
            return;
        }
        if dims.len() <= 1 {
            pretty_attrs(attrs, names, out);
            out.push('(');
            format_array_by(inner_attrs, inner_elem, inner_dims, names, out, elem_fn);
            out.push(')');
            pretty_dimensions(dims, out);
            return;
        }
        out.push('(');
        pretty_attrs(attrs, names, out);
        out.push('(');
        format_array_by(inner_attrs, inner_elem, inner_dims, names, out, elem_fn);
        out.push(')');
        pretty_dimensions(&dims[dims.len() - 1..], out);
        out.push(')');
        pretty_dimensions(&dims[..dims.len() - 1], out);
        return;
    }

    if attrs.is_empty() {
        elem_fn(elem, out);
        pretty_dimensions(dims, out);
        return;
    }
    if dims.len() <= 1 {
        pretty_attrs(attrs, names, out);
        elem_fn(elem, out);
        pretty_dimensions(dims, out);
        return;
    }
    out.push('(');
    pretty_attrs(attrs, names, out);
    elem_fn(elem, out);
    pretty_dimensions(&dims[dims.len() - 1..], out);
    out.push(')');
    pretty_dimensions(&dims[..dims.len() - 1], out);
}

/// `prettyQualifiedName` (`Type.hs:414-416`): `::`-joined, with the **first**
/// component run through `unmangleModuleNamespace`.
fn pretty_qualified_name(name: &QualifiedName, names: Option<&Interner>, out: &mut String) {
    for (i, seg) in name.segments().iter().enumerate() {
        if i > 0 {
            out.push_str("::");
        }
        let text = resolve(*seg, names);
        if i == 0 {
            out.push_str(&unmangle_module_namespace(&text));
        } else {
            out.push_str(&text);
        }
    }
}

/// `intercalate "::" $ map toString b` — the *raw*, still-mangled join used by
/// `instance Pretty TAttr` and `instance Pretty TArg` (`Type.hs:203`,
/// `Type.hs:222`).
fn join_raw(name: &QualifiedName, names: Option<&Interner>) -> String {
    name.segments()
        .iter()
        .map(|s| resolve(*s, names).to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// `unmangleModuleNamespace` (`Parser/Syntax.hs:276-280`): a leading `@` is
/// dropped and every remaining `@` becomes `.`, so `@data@closure@core` prints as
/// `data.closure.core`. A component with no leading `@` is left alone.
pub fn unmangle_module_namespace(text: &str) -> String {
    kg_base::unmangle_namespace(text)
}

fn resolve(sym: Name, names: Option<&Interner>) -> SmolStr {
    match names {
        Some(i) => i.resolve(sym),
        None => SmolStr::new_static("?"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ty::{Attribute, Flag, FunctionKind, TFuncParam};
    use kg_base::SymbolPath;

    fn r(t: Ty) -> TyRef {
        TyRef::new(t)
    }

    /// `showUnmangled = show . unmangle symbols` (`TypeCheck.hs:36`): a
    /// copy a message names is spelled from the instance the map notes at
    /// its name — `UINT<4>` in module `m` is `m::UINT<4>`, its module
    /// namespace unmangled — and a name the map holds no instance for is
    /// spelled as it stands, however it is shaped. Red with the map
    /// unconsulted (the mangled name comes back), or with the instance's
    /// arguments unmangled wrongly.
    #[test]
    fn a_message_spells_a_copy_from_its_instance_note() {
        let i = Interner::new();
        let copy = SymbolPath::new([i.intern("@m"), i.intern("UINT<|@4|>")]);
        let note = Ty::Instance {
            concrete: r(Ty::Unresolved),
            template: SymbolPath::new([i.intern("@m"), i.intern("UINT")]),
            args: vec![(i.intern("N"), TArg::Int(crate::ConstInt::from(4u64)))],
        };
        let map = |name: &[Name]| (name == copy.segments()).then(|| note.clone());
        let t = Ty::Struct(copy.clone(), Vec::new());
        assert_eq!(show_unmangled_with(&t, Some(&i), &map), "m::UINT<4>");
        let plain = Ty::Struct(SymbolPath::new([i.intern("@m"), i.intern("S")]), Vec::new());
        assert_eq!(show_unmangled_with(&plain, Some(&i), &map), "m::S");
        assert_eq!(show_unmangled_with(&Ty::Unsigned(8), Some(&i), &map), "uint8");
    }

    /// Clause 19: two closures over one function and one capture are one
    /// copy — the function type mangles to nothing (`Mangle.hs:42-43`) and
    /// every mention is typed off the copy (`Type.hs:1175-1177`) — so a
    /// generic lambda's `(C, auto) -> uint32` meets a formal's `(C, uint32)
    /// -> uint32` (`higher-order-functions.k::35`). Another capture is
    /// another copy, compared as the reference compares two closures.
    #[test]
    fn two_closures_over_one_function_and_capture_are_one_copy() {
        let i = Interner::new();
        let capture = |n: &str| Ty::Struct(SymbolPath::new([i.intern("@m"), i.intern(n)]), Vec::new());
        let closure = |f: Ty, c: Ty| Ty::Closure {
            func: SymbolPath::new([i.intern("Run@?lambda0")]),
            func_ty: r(f),
            capture: r(c),
            alias: SymbolPath::new([i.intern("@data@closure@core"), i.intern("__kg_m1")]),
        };
        let generic = closure(func(vec![capture("C"), Ty::Auto], Ty::Unsigned(32)), capture("C"));
        let formal = closure(func(vec![capture("C"), Ty::Unsigned(32)], Ty::Unsigned(32)), capture("C"));
        assert!(is_subtype(&generic, &formal));
        assert!(is_subtype(&formal, &generic));
        let other = closure(func(vec![capture("D"), Ty::Unsigned(32)], Ty::Unsigned(32)), capture("D"));
        assert!(!is_subtype(&generic, &other));
    }

    fn func(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::Function {
            kind: FunctionKind::FreeFunction,
            attrs: TAttrSet::new(),
            ret: r(ret),
            params: params
                .into_iter()
                .map(|t| TFuncParam { attrs: TAttrSet::new(), ty: r(t), name: None })
                .collect(),
        }
    }

    fn attr_func(attrs: TAttrSet, params: Vec<Ty>, ret: Ty) -> Ty {
        let Ty::Function { kind, ret, params, .. } = func(params, ret) else { unreachable!() };
        Ty::Function { kind, attrs, ret, params }
    }

    fn array(elem: Ty, dims: &[i128]) -> Ty {
        Ty::Array {
            attrs: TAttrSet::new(),
            elem: r(elem),
            dims: dims.iter().copied().map(ConstInt::from_i128).collect(),
        }
    }

    // -- §4.1 -------------------------------------------------------------

    #[test]
    fn integer_subtyping_is_strict() {
        // Type.hs:321-323 — `x < y`, not `x <= y`. Reflexivity comes only from
        // clause 3 (Type.hs:320).
        assert!(is_subtype(&Ty::Unsigned(8), &Ty::Unsigned(8)), "clause 3, not clause 4");
        assert!(is_subtype(&Ty::Unsigned(8), &Ty::Unsigned(9)));
        assert!(!is_subtype(&Ty::Unsigned(9), &Ty::Unsigned(8)));
        // Trap 4: equal widths across signedness are NOT related.
        assert!(!is_subtype(&Ty::Unsigned(8), &Ty::Signed(8)));
        assert!(is_subtype(&Ty::Unsigned(8), &Ty::Signed(9)));
        // There is no `TSigned <: TUnsigned` clause at all (Type.hs:317-350).
        assert!(!is_subtype(&Ty::Signed(8), &Ty::Unsigned(9)));
        assert!(is_subtype(&Ty::Signed(8), &Ty::Signed(9)));
    }

    #[test]
    fn corpus_type_comparisons_on_integers() {
        // test/syntax/type-constexpr.k:4-62 — `static assert` battery, with
        // expected:0 meaning the assertion holds.
        let u32t = Ty::Unsigned(32);
        let u30 = Ty::Unsigned(30);
        let i32t = Ty::Signed(32);
        assert_eq!(compare_types(BinOp::Equal, &u32t, &u32t), Some(true)); // :4
        assert_eq!(compare_types(BinOp::Equal, &u32t, &u30), Some(false)); // :28
        assert_eq!(compare_types(BinOp::Equal, &u32t, &i32t), Some(false)); // :32
        assert_eq!(compare_types(BinOp::NotEqual, &u32t, &u30), Some(true)); // :36
        assert_eq!(compare_types(BinOp::Greater, &u32t, &u30), Some(true)); // :44
        assert_eq!(compare_types(BinOp::GreaterEqual, &u32t, &u30), Some(true)); // :52
        assert_eq!(compare_types(BinOp::Less, &u32t, &u30), Some(false)); // :56
        assert_eq!(compare_types(BinOp::LessEqual, &u32t, &u30), Some(false)); // :60
        // `evalBinaryType _ _ _ = e` (Constexpr.hs:250) — not a comparison.
        assert_eq!(compare_types(BinOp::Add, &u32t, &u30), None);
    }

    #[test]
    fn corpus_array_dimension_comparison() {
        // test/syntax/type-constexpr.k:40 — `static assert(uint3[2] != uint3[1])`
        // is expected:0.
        let a2 = array(Ty::Unsigned(3), &[2]);
        let a1 = array(Ty::Unsigned(3), &[1]);
        assert_eq!(compare_types(BinOp::NotEqual, &a2, &a1), Some(true));
        assert!(!is_subtype(&a2, &a1), "first dimensions differ (Type.hs:346)");
    }

    #[test]
    fn corpus_subtype_of_enum_and_functions() {
        // test/syntax/helper/type_constexpr.k declares `enum Enum : uint7`, and
        // test/syntax/type-constexpr.k:134-160 asserts `subtype_of<Enum, uint8>`
        // and `subtype_of<(uint8) -> Struct_t, (Enum_t) -> Struct>` (expected:0).
        let e = Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(7)));
        assert!(is_subtype(&e, &Ty::Unsigned(8)), "Type.hs:328 then Type.hs:321");
        assert!(!is_subtype(&Ty::Unsigned(8), &e), "no `s <: TEnum` clause");
        assert_eq!(compare_types(BinOp::Less, &e, &Ty::Unsigned(8)), Some(true));

        let s = func(vec![Ty::Unsigned(8)], Ty::Void);
        let t = func(vec![e.clone()], Ty::Void);
        // CONTRAvariance: `p2 <: p1` means `Enum <: uint8` (Type.hs:332).
        assert!(is_subtype(&s, &t));
        assert!(!is_subtype(&t, &s));
        assert_eq!(compare_types(BinOp::Less, &s, &t), Some(true));
    }

    #[test]
    fn corpus_subtype_of_wider_parameter_narrower_return() {
        // test/syntax/type-constexpr.k:148-153 —
        // `subtype_of<(uint33) -> uint9, (uint32) -> uint10>` is expected:0.
        let s = func(vec![Ty::Unsigned(33)], Ty::Unsigned(9));
        let t = func(vec![Ty::Unsigned(32)], Ty::Unsigned(10));
        assert!(is_subtype(&s, &t));
        assert!(!is_subtype(&t, &s));
    }

    #[test]
    fn function_parameter_names_are_invisible_to_subtyping() {
        // test/syntax/type-constexpr.k:78-83 —
        // `same_types<(uint3 x) -> void, (uint3) -> void>` is expected:0, because
        // `instance Eq TFuncParam` ignores the name (Type.hs:126-129) and clause 3
        // of `<:` is plain `==`.
        let i = Interner::new();
        let named = Ty::Function {
            kind: FunctionKind::FreeFunction,
            attrs: TAttrSet::new(),
            ret: r(Ty::Void),
            params: vec![TFuncParam {
                attrs: TAttrSet::new(),
                ty: r(Ty::Unsigned(3)),
                name: Some(i.intern("x")),
            }],
        };
        let anon = func(vec![Ty::Unsigned(3)], Ty::Void);
        assert_eq!(compare_types(BinOp::Equal, &named, &anon), Some(true));
    }

    #[test]
    fn function_attribute_sets_compare_by_subset_and_ignore_order() {
        // test/syntax/type-constexpr.k:85-104.
        //  * :92-97  two attribute lists in different order are the SAME type.
        //  * :99-104 `max_threads(1)` vs `max_threads(2)` are unrelated —
        //            `Set.isSubsetOf` fails in both directions (Type.hs:334).
        let mt = |n: i128| TAttr::Int(Attribute::MaxThreads, ConstInt::from_i128(n));
        let tr = TAttr::Int(Attribute::ThreadRate, ConstInt::from_i128(1));

        let a: TAttrSet = [mt(2), tr.clone()].into_iter().collect();
        let b: TAttrSet = [tr.clone(), mt(2)].into_iter().collect();
        let s = attr_func(a, vec![Ty::Unsigned(3)], Ty::Void);
        let t = attr_func(b, vec![Ty::Unsigned(3)], Ty::Void);
        assert_eq!(compare_types(BinOp::Equal, &s, &t), Some(true), "a BTreeSet is order-blind");

        let one: TAttrSet = [mt(1), tr.clone()].into_iter().collect();
        let two: TAttrSet = [mt(2), tr].into_iter().collect();
        let s = attr_func(one, vec![Ty::Unsigned(3)], Ty::Void);
        let t = attr_func(two, vec![Ty::Unsigned(3)], Ty::Void);
        assert!(!is_subtype(&s, &t));
        assert!(!is_subtype(&t, &s));
        assert_eq!(compare_types(BinOp::Equal, &s, &t), Some(false));
    }

    #[test]
    fn a_target_returning_auto_accepts_any_return_type() {
        // `r2 == TAuto ||` short-circuits the return check (Type.hs:331).
        let s = func(vec![], Ty::Unsigned(64));
        let t = func(vec![], Ty::Auto);
        assert!(is_subtype(&s, &t));
        // But a *narrower* declared return is still rejected.
        let t = func(vec![], Ty::Unsigned(4));
        assert!(!is_subtype(&s, &t));
    }

    #[test]
    fn function_subtyping_requires_equal_arity() {
        // `liftEq` on lists is False for different lengths (Type.hs:332).
        let s = func(vec![Ty::Unsigned(8)], Ty::Void);
        let t = func(vec![Ty::Unsigned(8), Ty::Unsigned(8)], Ty::Void);
        assert!(!is_subtype(&s, &t));
        assert!(!is_subtype(&t, &s));
    }

    #[test]
    fn errors_are_neither_subtype_nor_supertype_of_themselves() {
        // Type.hs:318-319 precede the `s == t` clause, so even two *identical*
        // error types are unrelated.
        let e = Ty::Error(SmolStr::new_static("boom"));
        assert!(!is_subtype(&e, &e));
        assert!(!is_subtype(&Ty::Unsigned(8), &e));
        assert!(!is_subtype(&e, &Ty::Unsigned(8)));
        // ...and `tIsConvertibleTo` inherits that: clause 1 fails and no later
        // clause matches a `TError` (Type.hs:354-366).
        assert!(!is_convertible_to(&e, &e));
        assert!(!is_convertible_to(&e, &Ty::Unsigned(8)));
    }

    #[test]
    fn auto_is_below_everything_in_one_direction_only() {
        // Trap 5 — Type.hs:349.
        assert!(is_subtype(&Ty::Auto, &Ty::Void));
        assert!(is_subtype(&Ty::Auto, &Ty::Unsigned(3)));
        assert!(!is_subtype(&Ty::Void, &Ty::Auto));
        // ...except against an error, where clause 2 wins first.
        assert!(!is_subtype(&Ty::Auto, &Ty::Error(SmolStr::new_static("x"))));
    }

    #[test]
    fn arrays_are_invariant_and_attribute_sensitive() {
        // Trap 6 — Type.hs:344-348.
        let narrow = array(Ty::Unsigned(3), &[4]);
        let wide = array(Ty::Unsigned(4), &[4]);
        assert!(!is_subtype(&narrow, &wide), "element type is invariant");
        assert!(!is_subtype(&wide, &narrow));

        let mut attrs = TAttrSet::new();
        attrs.insert(TAttr::Flag(Flag::Memory));
        let mem = Ty::Array { attrs, elem: r(Ty::Unsigned(3)), dims: vec![ConstInt::from_i128(4)] };
        assert!(!is_subtype(&mem, &narrow), "attribute sets must be EQUAL, not subsets");
        assert!(!is_subtype(&narrow, &mem));
    }

    #[test]
    fn an_array_source_with_no_dimensions_matches_any_first_dimension() {
        // `null ds || head ds == d2` (Type.hs:346).
        let dimensionless = array(Ty::Unsigned(3), &[]);
        let sized = array(Ty::Unsigned(3), &[7]);
        assert!(is_subtype(&dimensionless, &sized));
        // ...but the *target* pattern is `TArray a2 _ (d2:_)`, so a dimensionless
        // target never matches clause 20 at all.
        assert!(!is_subtype(&sized, &dimensionless));
    }

    #[test]
    fn const_and_instance_are_transparent_to_subtyping() {
        // Type.hs:324-327.
        let c = Ty::Const(r(Ty::Unsigned(8)));
        assert!(is_subtype(&c, &Ty::Unsigned(9)));
        assert!(is_subtype(&Ty::Unsigned(8), &Ty::Const(r(Ty::Unsigned(9)))));
        let inst = Ty::Instance {
            concrete: r(Ty::Unsigned(8)),
            template: SymbolPath::empty(),
            args: vec![],
        };
        assert!(is_subtype(&inst, &Ty::Unsigned(9)));
        assert!(is_subtype(&Ty::Unsigned(8), &Ty::Instance {
            concrete: r(Ty::Unsigned(9)),
            template: SymbolPath::empty(),
            args: vec![],
        }));
    }

    #[test]
    fn dependent_is_transparent_only_on_the_right() {
        // Type.hs:329 — `s <: TDependent t`, with no left-hand clause.
        let d = Ty::Dependent(r(Ty::Unsigned(9)));
        assert!(is_subtype(&Ty::Unsigned(8), &d));
        assert!(!is_subtype(&Ty::Dependent(r(Ty::Unsigned(8))), &Ty::Unsigned(9)));
        // `tIsConvertibleTo` has the same asymmetry (Type.hs:363).
        assert!(is_convertible_to(&Ty::Unsigned(9), &Ty::Dependent(r(Ty::Unsigned(8)))));
    }

    #[test]
    fn empty_initializer_list_subtyping() {
        // Type.hs:335-338 give the empty initializer four special targets;
        // anything else falls through to clause 18 (Type.hs:339), where an array
        // succeeds vacuously (`all f [] = True`, Type.hs:313) and a function type
        // is not an aggregate at all (Type.hs:314).
        //
        // NOT taken from test/syntax/type-constexpr.k:309-340: those four
        // `static assert(… decltype({}) …)` snippets are all `expected:1`, but
        // for an unrelated reason — `expFromType TInitializer{} = Nothing`
        // (Type.hs:1618), so the `DecltypeF` folds to
        // ``"Argument of type `{}` is illegal for `decltype`"``
        // (Constexpr.hs:304-305, Constexpr.hs:345) before the comparison is ever
        // reached. They cannot distinguish the values below.
        let empty = Ty::Initializer(vec![]);
        assert!(is_subtype(&empty, &Ty::Boolean), "Type.hs:335");
        assert!(is_subtype(&empty, &Ty::Float), "Type.hs:336");
        assert!(is_subtype(&empty, &Ty::Unsigned(1)), "Type.hs:337");
        assert!(
            is_subtype(&empty, &Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(4)))),
            "Type.hs:338"
        );
        assert!(is_subtype(&empty, &array(Ty::Unsigned(1), &[2])), "Type.hs:339 → Type.hs:313");
        assert!(
            !is_subtype(&empty, &func(vec![], Ty::Void)),
            "a function is not an aggregate (Type.hs:314)"
        );
        assert!(!is_subtype(&empty, &Ty::Str), "no clause for TString");
        assert_eq!(compare_types(BinOp::Less, &empty, &Ty::Unsigned(1)), Some(true));
        assert_eq!(compare_types(BinOp::Less, &empty, &func(vec![], Ty::Void)), Some(false));
    }

    #[test]
    fn an_empty_initializer_initializes_an_empty_struct_vacuously() {
        // `all f [] = True` (Type.hs:313), reached through clause 18 because the
        // struct case is not one of Type.hs:335-338.
        let s = Ty::Struct(SymbolPath::empty(), vec![]);
        assert!(is_subtype(&Ty::Initializer(vec![]), &s));
    }

    // -- §4.2 / §4.3 ------------------------------------------------------

    #[test]
    fn any_integer_converts_to_any_integer() {
        // Type.hs:364-365 — the clause the `conversion` warning lives in.
        assert!(is_convertible_to(&Ty::Signed(32), &Ty::Unsigned(32)));
        assert!(!is_subtype(&Ty::Signed(32), &Ty::Unsigned(32)));
        assert!(is_convertible_to(&Ty::Unsigned(64), &Ty::Unsigned(2)));
        // ...but bool is not an integer (Type.hs:663).
        assert!(!is_convertible_to(&Ty::Boolean, &Ty::Unsigned(1)));
        assert!(!is_convertible_to(&Ty::Unsigned(1), &Ty::Boolean));
    }

    #[test]
    fn a_multi_element_initializer_never_initializes_a_union() {
        // Type.hs:307-308.
        let i = Interner::new();
        let u = Ty::Union(
            SymbolPath::empty(),
            vec![(i.intern("a"), r(Ty::Unsigned(8))), (i.intern("b"), r(Ty::Unsigned(8)))],
        );
        let one = Ty::Initializer(vec![r(Ty::Positional(0, r(Ty::Unsigned(4))))]);
        let two = Ty::Initializer(vec![
            r(Ty::Positional(0, r(Ty::Unsigned(4)))),
            r(Ty::Positional(1, r(Ty::Unsigned(4)))),
        ]);
        assert!(is_subtype(&one, &u));
        assert!(!is_subtype(&two, &u));
        assert!(!is_convertible_to(&two, &u));
    }

    #[test]
    fn struct_initializers_check_field_by_field() {
        // The shape test/syntax/conversion-warning.k:28-62 exercises: `{int32,
        // bool}` into `struct S { int32 x; bool y; }` is exact, while `{uint32,
        // bool}` is only *convertible* (and therefore warns, §6.2 step 5).
        let i = Interner::new();
        let s = Ty::Struct(
            SymbolPath::empty(),
            vec![(i.intern("x"), r(Ty::Signed(32))), (i.intern("y"), r(Ty::Boolean))],
        );
        let exact = Ty::Initializer(vec![
            r(Ty::Positional(0, r(Ty::Signed(32)))),
            r(Ty::Positional(1, r(Ty::Boolean))),
        ]);
        let narrowing = Ty::Initializer(vec![
            r(Ty::Positional(0, r(Ty::Unsigned(32)))),
            r(Ty::Positional(1, r(Ty::Boolean))),
        ]);
        assert!(is_subtype(&exact, &s), "conversion-warning.k:46-62 → warning:0");
        assert!(!is_subtype(&narrowing, &s), "conversion-warning.k:28-44 → warning:conversion");
        assert!(is_convertible_to(&narrowing, &s));
    }

    #[test]
    fn designators_select_by_name_and_positionals_by_index() {
        let i = Interner::new();
        let s = Ty::Struct(
            SymbolPath::empty(),
            vec![(i.intern("x"), r(Ty::Signed(32))), (i.intern("y"), r(Ty::Boolean))],
        );
        // Type.hs:299 / Type.hs:302.
        assert_eq!(initialized_type(&s, &Ty::Positional(1, r(Ty::Boolean))), Ty::Boolean);
        assert_eq!(
            initialized_type(&s, &Ty::Designator(i.intern("x"), r(Ty::Signed(32)))),
            Ty::Signed(32)
        );
        // A missing member is `TUndefined` (`tField`, Type.hs:959).
        assert_eq!(
            initialized_type(&s, &Ty::Designator(i.intern("z"), r(Ty::Boolean))),
            Ty::Undefined
        );
        // `tInitializerType` (Type.hs:288-291).
        assert_eq!(initializer_type(&Ty::Positional(3, r(Ty::Boolean))), Ty::Boolean);
        assert_eq!(initializer_type(&Ty::Unsigned(8)), Ty::Unresolved, "reference: undefined");
    }

    #[test]
    fn excess_initializer_elements_are_errors() {
        // Type.hs:296-298.
        let i = Interner::new();
        let a = array(Ty::Unsigned(8), &[2]);
        assert_eq!(
            initialized_type(&a, &Ty::Positional(2, r(Ty::Unsigned(8)))),
            Ty::Error(SmolStr::new_static("Excess elements in array initializer"))
        );
        assert_eq!(initialized_type(&a, &Ty::Positional(1, r(Ty::Unsigned(8)))), Ty::Unsigned(8));

        let u = Ty::Union(SymbolPath::empty(), vec![(i.intern("a"), r(Ty::Unsigned(8)))]);
        assert_eq!(
            initialized_type(&u, &Ty::Positional(1, r(Ty::Unsigned(8)))),
            Ty::Error(SmolStr::new_static("Excess elements in union initializer"))
        );
    }

    #[test]
    fn initialized_type_strips_const_from_the_target() {
        // Type.hs:294 — and only from the target.
        let c = Ty::Const(r(array(Ty::Unsigned(8), &[2])));
        assert_eq!(initialized_type(&c, &Ty::Positional(0, r(Ty::Unsigned(8)))), Ty::Unsigned(8));
    }

    #[test]
    fn a_scalar_target_rejects_a_non_empty_initializer() {
        // Type.hs:300 and Type.hs:303, with the type rendered by `showPretty`
        // and — unlike `docs/spec/types.md` §5's rendering of the same two
        // messages — **no backticks** around it. `Type.hs:923` is the only one of
        // the three that quotes its types, and it does so in the literal itself.
        assert_eq!(
            initialized_type(&Ty::Unsigned(8), &Ty::Positional(0, r(Ty::Unsigned(8)))),
            Ty::Error(SmolStr::new(
                "Non-empty initializer list cannot initialize type uint8"
            ))
        );
        let i = Interner::new();
        assert_eq!(
            initialized_type(&Ty::Float, &Ty::Designator(i.intern("f"), r(Ty::Unsigned(8)))),
            Ty::Error(SmolStr::new("Member designator cannot initialize type float"))
        );
    }

    #[test]
    fn interner_free_messages_are_inert() {
        // `aggregate_initializer` calls `initialized_type` without an interner.
        // That is only sound because both relations reject a `TError` operand
        // whatever it says (Type.hs:318-319), so the answer cannot depend on the
        // rendering. Same target, same verdict, either way.
        let i = Interner::new();
        // A nominal target is the only shape whose message *can* differ: it is
        // neither an aggregate (so `Type.hs:300` fires) nor structural (so
        // `showPretty` needs the string table).
        let target = Ty::Enum(SymbolPath::new([i.intern("@m"), i.intern("E")]), r(Ty::Unsigned(8)));
        let element = Ty::Positional(0, r(Ty::Unsigned(8)));
        let init = Ty::Initializer(vec![r(element.clone())]);

        let anonymous = initialized_type(&target, &element);
        let named = initialized_type_pretty(&target, &element, Some(&i));
        assert_eq!(
            anonymous,
            Ty::Error(SmolStr::new("Non-empty initializer list cannot initialize type ?::?"))
        );
        assert_eq!(
            named,
            Ty::Error(SmolStr::new("Non-empty initializer list cannot initialize type m::E"))
        );
        assert_ne!(anonymous, named, "the two renderings differ in the message");
        // ...and yet both relations reach the same verdict, because a `TError`
        // operand is rejected whatever it says.
        assert!(!aggregate_initializer(&is_subtype, &init, &target));
        assert!(!aggregate_initializer(&is_convertible_to, &init, &target));
        assert!(!is_subtype(&anonymous, &Ty::Unsigned(8)) && !is_subtype(&named, &Ty::Unsigned(8)));
    }

    // -- §4.4 -------------------------------------------------------------

    #[test]
    fn super_of_two_integers_takes_the_max_width() {
        // Type.hs:903-906.
        assert_eq!(t_super(&Ty::Unsigned(8), &Ty::Unsigned(12)), Ty::Unsigned(12));
        assert_eq!(t_super(&Ty::Signed(8), &Ty::Signed(12)), Ty::Signed(12));
        // Mixed signedness promotes the unsigned side with `tSigned`
        // (`uintN` → `int(N+1)`, Type.hs:551-552), so `int8` ⊔ `uint8` is `int9`.
        assert_eq!(t_super(&Ty::Signed(8), &Ty::Unsigned(8)), Ty::Signed(9));
        assert_eq!(t_super(&Ty::Unsigned(8), &Ty::Signed(8)), Ty::Signed(9));
        assert_eq!(t_super(&Ty::Unsigned(2), &Ty::Signed(16)), Ty::Signed(16));
    }

    #[test]
    fn super_is_unresolved_when_either_side_is() {
        // Type.hs:900-901 — the guard runs before `x == y`, so even two identical
        // unresolved types give `TUnresolved` rather than themselves.
        assert_eq!(t_super(&Ty::Unresolved, &Ty::Unresolved), Ty::Unresolved);
        assert_eq!(t_super(&Ty::Unsigned(8), &Ty::Undefined), Ty::Unresolved);
        // `TAuto` and `TError` *are* resolved (Type.hs:1001, trap 26).
        assert_eq!(t_super(&Ty::Auto, &Ty::Auto), Ty::Auto);
    }

    #[test]
    fn super_unwraps_enums_but_not_const() {
        // Type.hs:907-908 vs the deliberate absence of a `TConst` clause.
        let e = Ty::Enum(SymbolPath::empty(), r(Ty::Unsigned(3)));
        assert_eq!(t_super(&e, &Ty::Unsigned(8)), Ty::Unsigned(8));
        assert_eq!(t_super(&Ty::Unsigned(8), &e), Ty::Unsigned(8));
        let c = Ty::Const(r(Ty::Unsigned(8)));
        assert!(
            matches!(t_super(&c, &Ty::Unsigned(8)), Ty::Error(_)),
            "callers pre-strip const (Type.hs:1239)"
        );
    }

    #[test]
    fn super_of_unrelated_types_is_an_error() {
        // Type.hs:923.
        assert_eq!(
            t_super(&Ty::Boolean, &Ty::Unsigned(8)),
            Ty::Error(SmolStr::new("Types `bool` and `uint8` have no common supertype"))
        );
    }

    #[test]
    fn super_lifts_through_matching_slots_only() {
        // Type.hs:911-914.
        let a = Ty::Positional(0, r(Ty::Unsigned(4)));
        let b = Ty::Positional(0, r(Ty::Unsigned(9)));
        assert_eq!(t_super(&a, &b), Ty::Positional(0, r(Ty::Unsigned(9))));
        let c = Ty::Positional(1, r(Ty::Unsigned(9)));
        assert!(matches!(t_super(&a, &c), Ty::Error(_)), "different slots do not unify");
    }

    #[test]
    fn super_of_two_initializers_pads_with_the_longer_tail() {
        // Type.hs:919-922.
        let short = Ty::Initializer(vec![r(Ty::Positional(0, r(Ty::Unsigned(4))))]);
        let long = Ty::Initializer(vec![
            r(Ty::Positional(0, r(Ty::Unsigned(8)))),
            r(Ty::Positional(1, r(Ty::Boolean))),
        ]);
        assert_eq!(
            t_super(&short, &long),
            Ty::Initializer(vec![
                r(Ty::Positional(0, r(Ty::Unsigned(8)))),
                r(Ty::Positional(1, r(Ty::Boolean))),
            ])
        );
    }

    #[test]
    fn mux_folds_right_and_rejects_an_empty_list() {
        // Type.hs:925-926 — `foldr1`, which throws on `[]` (trap 11).
        let arms = [Ty::Unsigned(3), Ty::Unsigned(8), Ty::Signed(4)];
        // tSuper u3 (tSuper u8 i4) = tSuper u3 (tSuper i9 i4) = tSuper u3 i9 = i9.
        assert_eq!(t_mux(arms.iter()), Some(Ty::Signed(9)));
        assert_eq!(t_mux(std::iter::empty()), None);
        assert_eq!(t_mux([Ty::Boolean].iter()), Some(Ty::Boolean));
    }

    // -- §7 ---------------------------------------------------------------

    #[test]
    fn decltype_wraps_in_ttype() {
        // Type.hs:1198, and test/syntax/type-constexpr.k:20-26 where
        // `decltype(x) == uint32` holds for a `uint32 x` while `x == uint32`
        // (:12-18) is a hard error because `x` is not a type.
        assert_eq!(decltype(&Ty::Unsigned(32)), Ty::Type(r(Ty::Unsigned(32))));
        let d = decltype(&Ty::Unsigned(32));
        assert_eq!(compare_type_operands(BinOp::Equal, &d, &Ty::Type(r(Ty::Unsigned(32)))), Some(true));
        // The `tIsType` guard (Constexpr.hs:236-237): a value operand is not a
        // type comparison at all, so the node is left alone.
        assert_eq!(compare_type_operands(BinOp::Equal, &Ty::Unsigned(32), &d), None);
    }

    #[test]
    fn bitsizeof_and_bytesizeof() {
        // Constexpr.hs:271-275 over `tWidth` (Type.hs:695-723).
        let i = Interner::new();
        let s = Ty::Struct(
            SymbolPath::empty(),
            vec![(i.intern("a"), r(Ty::Unsigned(24))), (i.intern("b"), r(Ty::Unsigned(8)))],
        );
        assert_eq!(bit_sizeof(&s), Some(32));
        assert_eq!(byte_sizeof(&s), Some(Ok(4)));
        // Not a multiple of 8 (Constexpr.hs:349-351).
        assert_eq!(
            byte_sizeof(&Ty::Unsigned(3)),
            Some(Err("bytesizeof is undefined for a type with bit size of 3".to_string()))
        );
        // No width at all — the node is left alone (Type.hs:696).
        assert_eq!(bit_sizeof(&Ty::Void), None);
        assert_eq!(byte_sizeof(&Ty::Void), None);
    }

    #[test]
    fn bitoffsetof_packs_lsb_first_with_no_padding() {
        // test/logic/test_cases_26.k:154-168 declares
        // `struct Foo406 { uint40 _x; float32 _y; }` and asserts
        // `0 == bitoffsetof(Foo406,_x)`, `40 == bitoffsetof(Foo406,_y)`,
        // `0 == byteoffsetof(...)`, `5 == byteoffsetof(...)`. Fields are packed
        // LSB-first with no padding (Type.hs:726-730). `offsetOf` goes through
        // `theType` (Type.hs:470-471), so the operand is a `TType`.
        let i = Interner::new();
        let s = Ty::Type(r(Ty::Struct(
            SymbolPath::empty(),
            vec![(i.intern("_x"), r(Ty::Unsigned(40))), (i.intern("_y"), r(Ty::Float))],
        )));
        assert_eq!(bit_offset_of(&s, i.intern("_x")), Some(0));
        assert_eq!(bit_offset_of(&s, i.intern("_y")), Some(40));
        assert_eq!(byte_offset_of(&s, i.intern("_x")), Some(Ok(0)));
        assert_eq!(byte_offset_of(&s, i.intern("_y")), Some(Ok(5)));
        assert_eq!(bit_offset_of(&s, i.intern("nope")), None);
        // A non-`TType` operand becomes `TError "Expected a type"` and has no
        // offset (Type.hs:453-460).
        let bare = Ty::Struct(SymbolPath::empty(), vec![(i.intern("_x"), r(Ty::Unsigned(8)))]);
        assert_eq!(bit_offset_of(&bare, i.intern("_x")), None);
    }

    #[test]
    fn byteoffsetof_rejects_unaligned_fields() {
        // Constexpr.hs:165-166.
        let i = Interner::new();
        let s = Ty::Type(r(Ty::Struct(
            SymbolPath::empty(),
            vec![(i.intern("a"), r(Ty::Unsigned(3))), (i.intern("b"), r(Ty::Unsigned(8)))],
        )));
        assert_eq!(
            byte_offset_of(&s, i.intern("b")),
            Some(Err("byteoffsetof is undefined for fields with bit offset of 3".to_string()))
        );
    }

    #[test]
    fn clog2_requires_a_strictly_positive_argument() {
        // `fromNaturalConstexpr = find (> 0)` (Type.hs:1102-1103) and
        // `clog2` (Internal.hs:53-55).
        let c = ConstInt::from_i128;
        assert_eq!(clog2(&c(1)), Some(0));
        assert_eq!(clog2(&c(2)), Some(1));
        assert_eq!(clog2(&c(3)), Some(2));
        assert_eq!(clog2(&c(1024)), Some(10));
        assert_eq!(clog2(&c(1025)), Some(11));
        assert_eq!(clog2(&c(0)), None, "Frontend.hs:403");
        assert_eq!(clog2(&c(-4)), None);
    }

    // -- pretty printing ---------------------------------------------------

    #[test]
    fn pretty_matches_the_reference_instance() {
        // Type.hs:376-401.
        let p = |t: &Ty| show_pretty(t, None);
        assert_eq!(p(&Ty::Unsigned(32)), "uint32");
        assert_eq!(p(&Ty::Signed(7)), "int7");
        assert_eq!(p(&Ty::Boolean), "bool");
        assert_eq!(p(&Ty::Float), "float", "Type.hs:386 prints `float`, not `float32`");
        assert_eq!(p(&Ty::Str), "string");
        assert_eq!(p(&Ty::Void), "void");
        assert_eq!(p(&Ty::Auto), "auto");
        assert_eq!(p(&Ty::Untyped), "", "Type.hs:380");
        assert_eq!(p(&Ty::Unresolved), "unresolved");
        assert_eq!(p(&Ty::Undefined), "undefined");
        assert_eq!(p(&Ty::Const(r(Ty::Unsigned(8)))), "const uint8");
        assert_eq!(p(&Ty::Type(r(Ty::Unsigned(8)))), "type uint8");
        assert_eq!(p(&Ty::Dependent(r(Ty::Unsigned(8)))), "uint8", "Type.hs:390");
        assert_eq!(p(&Ty::Error(SmolStr::new_static("boom"))), "boom");
        assert_eq!(
            p(&Ty::Initializer(vec![r(Ty::Positional(0, r(Ty::Unsigned(1)))), r(Ty::Boolean)])),
            "{uint1, bool}"
        );
        assert_eq!(p(&array(Ty::Unsigned(4), &[8])), "uint4[8]");
        assert_eq!(p(&array(Ty::Unsigned(4), &[])), "uint4[]");
        assert_eq!(p(&func(vec![Ty::Unsigned(3)], Ty::Void)), "(uint3) -> void");
        assert_eq!(p(&func(vec![], Ty::Boolean)), "() -> bool");
    }

    #[test]
    fn pretty_renders_attributes_as_a_set() {
        // `prettyAttr` (Type.hs:371-374): `[[a,b]] ` with a trailing space, in
        // `Set` order — which is the `Ord` on TAttr, i.e. Flag before Int
        // (Type.hs:188-195).
        let attrs: TAttrSet = [
            TAttr::Int(Attribute::MaxThreads, ConstInt::from_i128(2)),
            TAttr::Flag(Flag::Pipelined),
        ]
        .into_iter()
        .collect();
        let f = attr_func(attrs, vec![], Ty::Void);
        assert_eq!(show_pretty(&f, None), "[[pipelined,max_threads(2)]] () -> void");
    }

    #[test]
    fn pretty_unmangles_only_the_module_namespace_component() {
        // `prettyQualifiedName` (Type.hs:414-416) applied to a mangled module
        // namespace (`Parser/Syntax.hs:273-280`).
        let i = Interner::new();
        let path = SymbolPath::new([i.intern("@data@closure@core"), i.intern("Closure")]);
        let s = Ty::Struct(path, vec![]);
        assert_eq!(show_pretty(&s, Some(&i)), "data.closure.core::Closure");
        assert_eq!(show_pretty(&s, None), "?::?", "no interner: names render as `?`");
    }

    #[test]
    fn pretty_flattens_nested_arrays_without_attributes() {
        // `formatArray` (Type.hs:177-183): the outer array contributes no
        // attributes, so the dimension lists are concatenated.
        let nested = Ty::Array {
            attrs: TAttrSet::new(),
            elem: r(array(Ty::Unsigned(8), &[3])),
            dims: vec![ConstInt::from_i128(2)],
        };
        assert_eq!(show_pretty(&nested, None), "uint8[2][3]");
    }
}
