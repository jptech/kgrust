//! The constexpr-fold seam, exercised through the public API only.
//!
//! The reference's inference is
//! `staticControl . unfix . evaluateConstexpr symbols . unfix . inferExpType symbols`
//! (`Type/Inference.hs:50`), and `evaluateConstexpr` is a **tree rewrite**: by
//! the time an enclosing node is inferred, `3 + 4` has been replaced by `7` and
//! a `const auto FOO = 32` reference by `32`. That is why the reference's
//! `fromIntegralConstexpr` (`Type.hs:1097-1100`) is a pure pattern match on a
//! literal — everything foldable already *is* one.
//!
//! The program-wide loop rewrites its tree as the reference does
//! (`kg_sem`), so these sites are the reference's own matches. These tests
//! pin the four sites that fold, and — just as importantly — the cases that
//! must not, because folding wider than the reference invents diagnostics
//! on correct programs. They were written against the CST inference
//! engine, which kept a lossless tree and *asked* where the reference
//! matches (`SymbolEnv::folded_int`); they read the loop since that engine
//! was deleted (sprint W3.7).
//!
//! They go through [`Db::ir_type_of_definition`] — the loop's type at the
//! declaration, what the editor's hover reads — so a regression here is a
//! regression a user would see.

use kg_base::{FileId, TextSize, Utf8PathBuf};
use kg_config::Settings;
use kg_db::Db;
use kg_ty::ty::{Attribute, TAttr, Ty};

fn db() -> Db {
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    Db::new(settings)
}

fn write(db: &mut Db, text: &str) -> FileId {
    db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned())
}

/// The type the editor would show for the *last* occurrence of `name`.
///
/// The last, not the first, so a fixture can declare `const auto N = 4;` and
/// then ask about the `a` in `uint32[N] a;` without the declaration's own
/// occurrence shadowing the query.
fn type_of(db: &Db, file: FileId, text: &str, name: &str) -> Ty {
    let at = text.rfind(name).unwrap_or_else(|| panic!("`{name}` is not in the fixture"));
    let def = db
        .definition_at(&[file], file, TextSize::from(at as u32))
        .unwrap_or_else(|| panic!("nothing is defined at `{name}`"));
    let looped = db.editor_loop(&[file], file).expect("the fixture parses");
    db.ir_type_of_definition(&looped, &def).unwrap_or(Ty::Unresolved)
}

/// Strip the `const` the declarations below all carry; these tests are about
/// the width, not about constness.
fn bare(ty: &Ty) -> Ty {
    ty.clone().un_const()
}

// ---------------------------------------------------------------------------
// The four sites that fold
// ---------------------------------------------------------------------------

/// `infer (ParamUintF x) = try (TType . TUnsigned) $ fromIntegralConstexpr x`
/// (`Type.hs:1246-1247`).
///
/// Without the fold, `uint<FOO>` is `TUnresolved` and every check downstream of
/// it goes quiet — including `Invalid member access on type`, which the
/// reference emits for `uint<FOO>` and `uint<32>` alike.
#[test]
fn a_parameterised_width_written_as_a_constant_resolves_like_a_literal() {
    let src = "const auto FOO = 32;\n\
               inline void main()\n\
               {\n\
                   uint<FOO> folded = 0;\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    assert_eq!(bare(&type_of(&db, file, src, "folded")), Ty::Unsigned(32));
}

/// `infer (ArrayF a b c)` under `mapM`/`try TType` (`Type.hs:1179`).
///
/// Note `fromIntegralConstexpr`, not `fromNaturalConstexpr`: the reference
/// really does give `uint2[-2]` the type `TArray [] uint2 [-2]`. Rejecting a
/// negative bound *here* would invent; positivity is `checkArraySize`'s job
/// (`TypeCheck.hs:187-196`), which pass 19 already ports.
#[test]
fn an_array_bound_written_as_an_expression_resolves_like_a_literal() {
    let src = "const auto N = 4;\n\
               inline void main()\n\
               {\n\
                   uint32[3 + 4] literals;\n\
                   uint32[N] named;\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    for (name, extent) in [("literals", 7i128), ("named", 4)] {
        match bare(&type_of(&db, file, src, name)) {
            Ty::Array { dims, .. } => {
                assert_eq!(dims.len(), 1, "`{name}` should have one dimension");
                assert_eq!(dims[0].to_i128(), Some(extent), "`{name}`");
            }
            other => panic!("`{name}` is {other:?}, not an array"),
        }
    }
}

/// `infer (FanOutF a b) = try (TArray Set.empty (typeOf b) . pure) (fromIntegralConstexpr a)`
/// (`Type.hs:1204`).
#[test]
fn a_fan_out_count_written_as_a_constant_gives_the_array_its_extent() {
    let src = "const auto WAYS = 4;\n\
               inline void main()\n\
               {\n\
                   uint32 v = 0;\n\
                   const auto spread = fan_out<WAYS>(v);\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    match bare(&type_of(&db, file, src, "spread")) {
        Ty::Array { dims, .. } => assert_eq!(dims.first().and_then(kg_ty::ConstInt::to_i128), Some(4)),
        other => panic!("a fan_out is {other:?}, not an array"),
    }
}

/// `attr (IntAttr x y) = TIntAttr x <$> fromIntegralConstexpr y`
/// (`Type.hs:1323-1328`).
///
/// The failure this pins is not a missing diagnostic but a wrong *identity*: an
/// attribute that stays `TAttr::Named` compares unequal to the same attribute
/// written as a literal, so two function types the reference calls equal would
/// not be. Nothing reads a function type's attribute set today, which is exactly
/// why this needs a test rather than a gate — see `docs/journal.md`.
#[test]
fn a_folded_integer_attribute_is_an_int_attribute_not_a_named_one() {
    let src = "const auto LIMIT = 8;\n\
               [[max_threads(LIMIT)]]\n\
               inline void worker()\n\
               {\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    match type_of(&db, file, src, "worker") {
        Ty::Function { attrs, .. } => {
            let folded = attrs.iter().any(|a| match a {
                TAttr::Int(Attribute::MaxThreads, v) => v.to_i128() == Some(8),
                _ => false,
            });
            assert!(folded, "the attribute did not fold: {attrs:?}");
        }
        other => panic!("`worker` is {other:?}, not a function"),
    }
}

// ---------------------------------------------------------------------------
// The cases that must NOT fold
//
// These are the load-bearing half. Folding *wider* than `evaluateConstexpr`
// does not lose a diagnostic — it invents one, on a program the reference
// accepts, which is the failure this port cannot afford.
// ---------------------------------------------------------------------------

/// `evaluateConstexpr`'s `NamedValueF` arm folds only when `tIsConst $ typeOf x`
/// (`Constexpr.hs:285-293`). A mutable `auto` is not a constant, and a width
/// that reads one is an error in the reference (`Frontend.hs:404-405`): "uint
/// width did not evaluate to a positive integer constant" at the `uint`, which
/// the loop gives too (`kg-oracle --errors`, 2026-10-10). A folded `32` would
/// be a wrong type and no error.
#[test]
fn a_mutable_name_does_not_fold_into_a_width() {
    let src = "inline void main()\n\
               {\n\
                   auto WIDTH = 32;\n\
                   uint<WIDTH> unfolded = 0;\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    assert!(
        !bare(&type_of(&db, file, src, "unfolded")).resolved(),
        "a mutable name folded into a width"
    );
    let errors: Vec<String> = db.check(&[file]).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect();
    assert_eq!(errors, ["uint width did not evaluate to a positive integer constant"]);
}

/// A `static for` induction variable is materialised by `staticControl`
/// (`Type/Inference.hs:50`): the loop unrolls the body, one copy per
/// iteration, and folds `i` in each — so the written declaration, which no
/// iteration is, has no width to fold to. Ten `expected:0` assertions across
/// `static-for.k`, `closures.k` and `local-functions.k` are written in this
/// shape, and folding `i` to its first value would turn each into an
/// invented error.
#[test]
fn a_static_for_induction_variable_does_not_fold_into_a_width() {
    let src = "inline void main()\n\
               {\n\
                   static for (const auto i : 2)\n\
                   {\n\
                       uint<i + 1> unrolled = 0;\n\
                   }\n\
               }\n";
    let mut db = db();
    let file = write(&mut db, src);
    assert!(
        !bare(&type_of(&db, file, src, "unrolled")).resolved(),
        "a static-for induction variable folded into a width"
    );
    let at = src.rfind("unrolled").expect("in the fixture");
    let def = db.definition_at(&[file], file, TextSize::from(at as u32)).expect("declared");
    let looped = db.editor_loop(&[file], file).expect("the fixture parses");
    // Each iteration's copy has its own: `uint1`, then `uint2`.
    let answer = db.ir_answer_of_definition(&looped, &def, &|t: &Ty| t.resolved());
    let Some(kg_db::looped::IrAnswer::Copies { types, .. }) = answer else { panic!("{answer:?}") };
    assert_eq!(types.iter().map(|(t, _)| bare(t)).collect::<Vec<_>>(), [Ty::Unsigned(1), Ty::Unsigned(2)]);
    assert!(db.check(&[file]).is_empty());
}

/// A constant whose initializer needs a *width* — `const auto J = bitsizeof
/// S;` — folds, and an importer's template argument folds through it.
///
/// The CST engine's constant environment answered `bit_width_of` with
/// `None`, because a struct's layout was its type table's to know and the
/// table was inferred from the fold. The reference has no such ordering: `inferType` is one catamorphism, so
/// `sizeOf e = tWidth (typeOf e)` reads a type the same pass just wrote
/// (`Type/Inference.hs:50`, `Type.hs:462`), and so does the loop's: the
/// census decides `T3<true>` (`constexpr.k::31`, `T3<(S3_size / 8 ==
/// S3_size >> 3)>`, the recorded walk). Until sprint W3.7 this pinned the
/// second census's late fold (`Db::late_constants`); it is the loop's
/// census now (`kg_db::instances`).
#[test]
fn an_imported_constant_that_needs_a_width_folds_at_a_template_argument() {
    let helper = "module helper.c2 { J, S }\n\n\
                  struct S\n{\nuint32 a;\nuint32 b;\n}\n\n\
                  const auto J = bitsizeof S;\n";
    let t3 = "module helper.t3 { T3 }\n\n\
              template <bool B>\nclass T3\n{\nstatic assert(B);\n}\n";
    let main = "import helper.c2\n\nimport helper.t3\n\n\
                inline void main()\n{\nstatic T3<(J == 64)> foo;\n}\n";
    let mut db = db();
    db.set_file_text(&Utf8PathBuf::from("/proj/helper/c2.k"), helper.to_owned());
    db.set_file_text(&Utf8PathBuf::from("/proj/helper/t3.k"), t3.to_owned());
    let file = write(&mut db, main);
    let set = kg_db::instances::instances(&db, &[file]);
    let spelled: Vec<&str> =
        set.templates.get("helper.t3.T3").map(|m| m.iter().map(|i| i.spelled.as_str()).collect()).unwrap_or_default();
    assert_eq!(spelled, vec!["T3<true>"], "the instances of helper.t3.T3: {:?}", set.templates);
}
