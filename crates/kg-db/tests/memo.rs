//! What an edit recomputes, counted — the memo-miss counters of
//! [`kg_db::stats`] over the four edits a keystroke is priced by (sprint
//! W0.5, W7.1): inside a body, to a signature, to an import, and inside a
//! body in a module the root imports.
//!
//! Each test pins today's answer. Past the parse there is one cutoff per
//! module: the sorted lowering's (sprint §3 W7.3: `sorted module`, one miss
//! per module lowered). Otherwise a body edit makes a new item tree, a new
//! program and a loop from its first round. The interface firewall's lines
//! (W7.4: `module inference` and `file_scope`) were the CST inference
//! engine's, which a check stopped asking in sprint W3.7 and which was
//! deleted the same sprint. Each W7 mechanism changes one of these lines,
//! and the change is its evidence — "a body edit misses `item_tree` once
//! and `program` never" is a line here, not a timing.

use expect_test::{expect, Expect};
use kg_base::{FileId, Utf8PathBuf};
use kg_config::Settings;
use kg_db::Db;

const LIB: &str = "module lib { twice }\n\ninline uint32 twice(uint32 x)\n{\n    return x + x;\n}\n";
const MAIN: &str = "import lib\n\ninline uint32 four(uint32 y)\n{\n    return twice(twice(y));\n}\n\ninline void main()\n{\n    uint32 z = four(1);\n}\n";

fn db() -> Db {
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    Db::new(settings)
}

fn write(db: &mut Db, name: &str, text: &str) -> FileId {
    db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), text.to_owned())
}

/// A warm check of `main.k`, `edit` made, and the check again: which
/// queries missed, as `name misses/calls`.
fn misses_after(edit: impl FnOnce(&mut Db), want: Expect) {
    let mut db = db();
    write(&mut db, "lib.k", LIB);
    let main = write(&mut db, "main.k", MAIN);
    db.check(&[main]);
    let before = db.stats();
    edit(&mut db);
    db.check(&[main]);
    want.assert_eq(&db.stats().since(&before).misses_line());
}

#[test]
fn a_body_edit() {
    misses_after(
        |db| {
            write(db, "main.k", &MAIN.replace("uint32 z = four(1);", "uint32 z = four(2);"));
        },
        expect!["parse 1/79, item_tree 1/58, program 1/14, global_symbols 1/3, looped 1/6, sorted lowering 1/1, sorted module 1/3, CST passes 3/3"],
    );
}

#[test]
fn a_signature_edit() {
    misses_after(
        |db| {
            write(db, "main.k", &MAIN.replace("four(uint32 y)", "four(uint32 y, uint1 w)"));
        },
        expect!["parse 1/79, item_tree 1/58, program 1/14, global_symbols 1/3, looped 1/6, sorted lowering 1/1, sorted module 1/3, CST passes 3/3"],
    );
}

#[test]
fn an_import_edit() {
    misses_after(
        |db| {
            write(db, "main.k", &MAIN.replace("import lib\n", ""));
        },
        expect!["parse 1/43, item_tree 1/31, program 1/10, global_symbols 1/2, looped 1/4, sorted lowering 1/1, sorted module 1/2, CST passes 2/2"],
    );
}

/// A body edit in the imported module: its lowering misses, its
/// importers' are kept (`sorted module 1/3`), and the loop runs again over
/// the new program. The interface firewall (sprint W7.4) kept the
/// importer's CST inference too (`module inference 1/3`), until that
/// engine was deleted (sprint W3.7).
#[test]
fn a_body_edit_in_an_imported_module() {
    misses_after(
        |db| {
            write(db, "lib.k", &LIB.replace("return x + x;", "return x << 1;"));
        },
        expect!["parse 1/78, item_tree 1/57, program 1/14, global_symbols 1/3, looped 1/6, sorted lowering 1/1, sorted module 1/3, CST passes 3/3"],
    );
}

/// The edit `edit-identity` makes (`xtask/src/edits.rs`): a local declared
/// in an imported body. No importer can ask for a local, and none lowers
/// again.
#[test]
fn a_local_declared_in_an_imported_body() {
    misses_after(
        |db| {
            write(db, "lib.k", &LIB.replace("{\n    return", "{ uint32 kg_edit_probe = 0;\n    return"));
        },
        expect!["parse 1/78, item_tree 1/57, program 1/14, global_symbols 1/3, looped 1/6, sorted lowering 1/1, sorted module 1/3, CST passes 3/3"],
    );
}

/// The firewall that exists: re-entering identical text leaves every memo
/// past the parse standing.
#[test]
fn identical_text_misses_only_the_parse() {
    misses_after(
        |db| {
            write(db, "main.k", MAIN);
        },
        expect!["parse 1/57, CST passes 3/3"],
    );
}

/// Two identical declarations are one symbol to `nub`, which compares
/// symbol trees with notes ignored (`SymbolTable.hs:229-237`,
/// `Parser/Syntax.hs:594-599`) — so a body edit in an imported module can
/// change how an importer's names sort though the importer's text did not
/// change. Here `main.k`, itself `module lib`, declares the `twice` it
/// imports: identical, the two are one and the call's qualifier is grafted;
/// once `lib.k`'s body differs the name is ambiguous and stays bare. The
/// sorted lowering's memo (W7.3) must see that through what the lowering
/// asked, by value. Red with the memo keyed on the parse alone, or with the
/// declaration's fingerprint left out of the answers it compares.
#[test]
fn an_imported_body_edit_can_re_sort_an_importer() {
    const OWN: &str = "module lib { main }\n\nimport lib\n\ninline uint32 twice(uint32 x)\n{\n    return x + x;\n}\n\ninline void main()\n{\n    uint32 z = twice(1);\n}\n";
    let edited = LIB.replace("return x + x;", "return x << 1;");
    let sorted_main = |db: &Db, main: FileId| {
        let sorted = kg_db::sorted::sorted_program(db, &[main]);
        let ir = sorted.ir(kg_db::ModuleSource::File(main)).expect("main is lowered");
        kg_hir::structure::render(ir, db.interner(), kg_hir::structure::Detail::Structure)
    };

    let mut warm = db();
    write(&mut warm, "lib.k", LIB);
    let main = write(&mut warm, "main.k", OWN);
    let before = sorted_main(&warm, main);
    write(&mut warm, "lib.k", &edited);
    let after = sorted_main(&warm, main);

    let mut fresh = db();
    write(&mut fresh, "lib.k", &edited);
    let main = write(&mut fresh, "main.k", OWN);
    let want = sorted_main(&fresh, main);
    assert_ne!(before, want, "the edit must change how main.k sorts, or this pins nothing");
    assert_eq!(after, want);
}

/// What an importer's lowering memo keeps of an answer `nub` compared is
/// its fingerprints' values. The harvest defers them over the imported
/// module's IR (`kg_hir::sort::Fingerprint`), and a memo holding a deferred
/// one would keep that IR alive past the module's next lowering — here a
/// trailing space, which re-lowers `lib.k` and leaves `main.k`'s memo
/// standing, since `twice`'s tree is the same. Red with a deferred
/// fingerprint kept, for the answer `nub` compared (`twice`, declared by
/// both) or for one it did not (`once`).
#[test]
fn an_importers_memo_keeps_no_imported_ir_alive() {
    const OWN: &str = "module lib { main }\n\nimport lib\n\ninline uint32 twice(uint32 x)\n{\n    return x + x;\n}\n\ninline void main()\n{\n    uint32 z = twice(once(1));\n}\n";
    let text = format!("{}\ninline uint32 once(uint32 x)\n{{\n    return x;\n}}\n", LIB.replace("{ twice }", "{ twice, once }"));
    let mut db = db();
    let lib = write(&mut db, "lib.k", &text);
    let main = write(&mut db, "main.k", OWN);
    let first = {
        let sorted = kg_db::sorted::sorted_program(&db, &[main]);
        std::sync::Arc::downgrade(sorted.ir(kg_db::ModuleSource::File(lib)).expect("lib is lowered"))
    };
    write(&mut db, "lib.k", &format!("{text} "));
    let before = db.stats();
    let sorted = kg_db::sorted::sorted_program(&db, &[main]);
    let lowered = db.stats().since(&before);
    let module = kg_db::stats::Query::SortedModule;
    assert_eq!((lowered.misses(module), lowered.hits(module)), (1, 2), "lib.k re-lowered, the rest kept");
    drop(sorted);
    assert!(first.upgrade().is_none(), "main.k's memo holds lib.k's first lowering");
}

/// What an importer reads of an answer no `nub` weighed is its shape (and
/// its hits' count and scopes): `twice` turned into a template makes
/// `main.k`'s calls template instances (`Parser.hs:669-672`) though its text
/// did not change. Red with the shape left out of the comparison of an
/// answer `nub` did not read.
#[test]
fn an_imported_shape_edit_re_sorts_an_importer() {
    let edited = LIB.replace("inline uint32 twice", "template <auto N>\ninline uint32 twice");
    let sorted_main = |db: &Db, main: FileId| {
        let sorted = kg_db::sorted::sorted_program(db, &[main]);
        let ir = sorted.ir(kg_db::ModuleSource::File(main)).expect("main is lowered");
        kg_hir::structure::render(ir, db.interner(), kg_hir::structure::Detail::Structure)
    };

    let mut warm = db();
    write(&mut warm, "lib.k", LIB);
    let main = write(&mut warm, "main.k", MAIN);
    let before = sorted_main(&warm, main);
    write(&mut warm, "lib.k", &edited);
    let after = sorted_main(&warm, main);

    let mut fresh = db();
    write(&mut fresh, "lib.k", &edited);
    let main = write(&mut fresh, "main.k", MAIN);
    let want = sorted_main(&fresh, main);
    assert_ne!(before, want, "the edit must change how main.k sorts, or this pins nothing");
    assert_eq!(after, want);
}

/// The check and the loop's fingerprint — every module's final tree, every
/// node's type, the report (`kg_sem::elaborate::fingerprint`) — what an
/// importer decides, with each `Symbol(N)` spelled, since two databases
/// intern apart. File ids agree because both databases are written in one
/// order. Until sprint W3.7 the CST engine's base tables were read here,
/// entry by entry.
fn observed(db: &Db, main: FileId) -> String {
    let roots = [main];
    let mut out: Vec<String> = db.check(&roots).iter().map(|d| format!("{d:?}")).collect();
    out.push(kg_sem::elaborate::fingerprint(&db.looped(&roots).out, db.interner()));
    spell_symbols(&out.join("\n"), db)
}

fn spell_symbols(text: &str, db: &Db) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("Symbol(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "Symbol(".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        match after[..digits].parse::<usize>().ok().and_then(|n| db.interner().resolve_index(n)) {
            Some(name) if after[digits..].starts_with(')') => {
                out.push_str(&name);
                rest = &after[digits + 1..];
            }
            _ => {
                out.push_str("Symbol(");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `lib.k` edited under a warm `main.k`, against a fresh database over the
/// edited text: what a warm importer decides equals what a fresh one does,
/// and the edit changes it, or the test pins nothing.
fn importer_follows(lib: &str, main_text: &str, edited: &str) {
    let mut warm = db();
    write(&mut warm, "lib.k", lib);
    let main = write(&mut warm, "main.k", main_text);
    let before = observed(&warm, main);
    write(&mut warm, "lib.k", edited);
    let after = observed(&warm, main);

    let mut fresh = db();
    write(&mut fresh, "lib.k", edited);
    let main = write(&mut fresh, "main.k", main_text);
    let want = observed(&fresh, main);
    assert_ne!(before, want, "the edit must change what main.k decides, or this pins nothing");
    assert_eq!(after, want);
}

/// An imported declaration whose *type* changed reaches its importer's
/// types though the importer's text and lowering did not change. Written
/// for the interface firewall (sprint W7.4), the CST engine's table kept
/// while every question its fixpoint put to the imports read the same; it
/// pins the loop's memo now, keyed on the program. Red with that memo
/// served whatever the program (`Db::looped`'s `Arc::ptr_eq` taken as
/// true).
#[test]
fn an_imported_signature_re_infers_an_importer() {
    importer_follows(LIB, MAIN, &LIB.replace("inline uint32 twice", "inline uint64 twice"));
}

/// A parameter's name is part of the function type an importer is handed,
/// though `TFuncParam`'s `Eq` ignores it (`Type.hs:126-129`): the CST
/// engine's import door compared its answers by `Ord` for it. The loop's
/// program is new whenever an item tree is, so it re-infers; red with
/// `Db::looped`'s memo served whatever the program.
#[test]
fn an_imported_parameters_name_re_infers_an_importer() {
    importer_follows(LIB, MAIN, &LIB.replace("uint32 x)", "uint32 w)").replace("x + x", "w + w"));
}

/// A constant whose value changed and whose type did not must reach the
/// importer's widths. `uint32`, not `auto`: an `auto` constant's type is
/// its literal's width. Written for the CST engine's fold environment,
/// which held the imported constants by value (`Db::file_scope`, deleted
/// in sprint W3.7); it pins the loop's memo now, keyed on the program. Red
/// with `Db::looped`'s memo served whatever the program.
#[test]
fn an_imported_constants_value_re_folds_an_importer() {
    const LIB_N: &str = "module lib { N }\n\nconst uint32 N = 3;\n";
    const MAIN_N: &str = "import lib\n\ninline void main()\n{\n    uint<N> z = 0;\n}\n";
    importer_follows(LIB_N, MAIN_N, &LIB_N.replace("= 3", "= 4"));
}

/// A constant whose value is a width follows the imported struct it
/// measures: here the struct grows, `main.k`'s own text does not change,
/// and `W` is 16 where it was 8 — the instance the loop makes of `f<W>`
/// with it, a warm `Db`'s as a fresh one's. Until sprint W3.7 this pinned
/// the second census's late fold (`Db::late_constants`) and its table key;
/// the census is gone, and it pins the loop's (`kg_db::instances`).
#[test]
fn an_imported_width_re_folds_a_late_constant() {
    const LIB_S: &str = "module lib { S }\n\nstruct S\n{\n    uint8 a;\n}\n";
    const MAIN_S: &str = "import lib\n\nconst auto W = bitsizeof S;\n\ntemplate <auto N>\ninline uint32 f()\n{\n    return N;\n}\n\ninline void main()\n{\n    uint32 z = f<W>();\n}\n";
    let edited = LIB_S.replace("uint8 a", "uint16 a");
    let instances = |db: &Db, main: FileId| spell_symbols(&format!("{:?}", kg_db::instances::instances(db, &[main]).templates), db);

    let mut warm = db();
    write(&mut warm, "lib.k", LIB_S);
    let main = write(&mut warm, "main.k", MAIN_S);
    let before = instances(&warm, main);
    write(&mut warm, "lib.k", &edited);
    let after = instances(&warm, main);

    let mut fresh = db();
    write(&mut fresh, "lib.k", &edited);
    let main = write(&mut fresh, "main.k", MAIN_S);
    let want = instances(&fresh, main);
    assert_ne!(before, want, "the edit must change the instances main.k decides, or this pins nothing");
    assert_eq!(after, want);
}

/// A loop cancelled mid-run (sprint W7.2) unwinds with `Cancelled` and
/// writes no memo: the next ask misses `looped` again, and its answer is a
/// fresh `Db`'s. Red with the loop's checkpoints removed (it finishes), or
/// with its memo written on the way out.
#[test]
fn a_cancelled_loop_writes_no_memo() {
    use kg_base::cancel::{catch, Cancelled};
    let mut db = db();
    write(&mut db, "lib.k", LIB);
    let main = write(&mut db, "main.k", MAIN);
    let token = db.cancel_token();
    token.raise();
    assert!(matches!(catch(|| db.looped(&[main]).out.report.rounds), Err(Cancelled)));
    token.reset();
    let before = db.stats();
    let warm = db.check(&[main]);
    let after = db.stats().since(&before);
    assert_eq!(after.misses(kg_db::stats::Query::Looped), 1, "{}", after.misses_line());

    let mut fresh = self::db();
    write(&mut fresh, "lib.k", LIB);
    let main = write(&mut fresh, "main.k", MAIN);
    let render = |d: &[kg_diag::Diagnostic]| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>();
    assert_eq!(render(&warm), render(&fresh.check(&[main])));
}

/// An input change lowers a raised token: the edit that superseded a check
/// does not cancel the check that answers for it.
#[test]
fn an_edit_lowers_the_token() {
    let mut db = db();
    write(&mut db, "lib.k", LIB);
    let main = write(&mut db, "main.k", MAIN);
    db.cancel_token().raise();
    write(&mut db, "main.k", &MAIN.replace("four(1)", "four(3)"));
    assert!(kg_base::cancel::catch(|| db.check(&[main])).is_ok());
}
