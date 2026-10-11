//! The query layer, exercised through the public API only.
//!
//! Everything below runs against in-memory overlays, so the tests are hermetic
//! and the same code path an editor uses is the one under test. The last test
//! is different on purpose: it loads the reference's real base library off disk,
//! because a module search that works on invented fixtures and not on
//! `library/base.k` has not been tested at all.

use std::sync::Arc;

use kg_base::{FileId, Utf8Path, Utf8PathBuf};
use kg_config::Settings;
use kg_db::Db;
use kg_resolve::{ModuleSource, SymbolsConfig};

/// A database rooted at `/proj` with `/proj` on the import path, so a bare
/// `import a` finds `/proj/a.k`.
fn db() -> Db {
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    // `implicit_base_file` picks `base.k` as soon as there is an import dir
    // (`Parser/IO.hs:72-75`); these fixtures have no base library, so opt out
    // and take `.cmdargs.k` instead.
    settings.implicit_base = false;
    Db::new(settings)
}

fn write(db: &mut Db, name: &str, text: &str) -> FileId {
    db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), text.to_owned())
}

fn messages(diagnostics: &[kg_diag::Diagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.message.clone()).collect()
}

/// The *error* half of a loading result. Separate from [`messages`] because
/// `Program::diagnostics` also carries the `redundant-import` warning, which is
/// enabled by default (`app/Options/CmdArgs.hs:113-116`) and fires on any
/// fixture that imports a module without using a name from it — which is most of
/// them below, since they are about the *graph* and not about name resolution.
fn errors(diagnostics: &[kg_diag::Diagnostic]) -> Vec<String> {
    diagnostics.iter().filter(|d| d.is_error()).map(|d| d.message.clone()).collect()
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// With no import directories the implicit base is `.cmdargs.k`, which is
/// synthetic and therefore always resolvable (`Parser/IO.hs:71-75`). So even a
/// lone file is a two-module program, and a language server in a bare checkout
/// still gets `--define`d names.
#[test]
fn a_lone_file_is_a_program_of_two_modules() {
    let mut db = Db::new(Settings::fallback("/proj"));
    let main = write(&mut db, "main.k", "inline void main() {}\n");

    let program = db.program(&[main]);
    assert_eq!(program.base, Some(ModuleSource::Cmdargs));
    assert_eq!(program.files, vec![ModuleSource::Cmdargs, ModuleSource::File(main)]);
    assert!(program.diagnostics.is_empty(), "{:?}", messages(&program.diagnostics));
}

/// The thing `check_source` structurally cannot do: `import a` finds a file.
#[test]
fn an_import_resolves_through_the_search_path() {
    let mut db = db();
    let a = write(&mut db, "a.k", "module a { x }\nconst auto x = 7;\n");
    let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

    let program = db.program(&[main]);
    assert!(program.contains(ModuleSource::File(a)));
    assert!(errors(&program.diagnostics).is_empty(), "{:?}", errors(&program.diagnostics));
}

/// **Post-order DFS, dependencies first** (`Parser/IO.hs:96`) — the order
/// duplicate-key precedence in `getSymbols` reads
/// (`docs/spec/resolve.md` §3.1). Getting it backwards silently changes which
/// declaration a name resolves to.
#[test]
fn the_file_order_is_post_order_dfs() {
    let mut db = db();
    let leaf = write(&mut db, "leaf.k", "module leaf { }\n");
    let mid = write(&mut db, "mid.k", "module mid { }\nimport leaf\n");
    let main = write(&mut db, "main.k", "import mid\n");

    let program = db.program(&[main]);
    assert_eq!(
        program.files,
        vec![
            ModuleSource::Cmdargs,
            ModuleSource::File(leaf),
            ModuleSource::File(mid),
            ModuleSource::File(main),
        ]
    );
}

/// A diamond parses each module once, however many routes reach it — the cache
/// probe of `Parser/IO.hs:91-95`. Here that is the `item_tree` memo, so the
/// shared module appears exactly once in the file list.
#[test]
fn a_diamond_visits_the_shared_module_once() {
    let mut db = db();
    let shared = write(&mut db, "shared.k", "module shared { }\n");
    write(&mut db, "left.k", "module left { }\nimport shared\n");
    write(&mut db, "right.k", "module right { }\nimport shared\n");
    let main = write(&mut db, "main.k", "import left\nimport right\n");

    let program = db.program(&[main]);
    assert_eq!(program.files.iter().filter(|&&m| m == ModuleSource::File(shared)).count(), 1);
    assert!(errors(&program.diagnostics).is_empty(), "{:?}", errors(&program.diagnostics));
}

/// `docs/spec/resolve.md` §8: a cycle terminates and is reported at the
/// offending `import` as the bare string `"Circular dependency"`
/// (`Parser/IO.hs:90`). Oracle: the 3-cycle of `test/syntax/imports.k:525-535`.
#[test]
fn an_import_cycle_terminates_and_is_reported() {
    let mut db = db();
    write(&mut db, "a.k", "module a { }\nimport b\n");
    write(&mut db, "b.k", "module b { }\nimport a\n");
    let main = write(&mut db, "main.k", "import a\n");

    let program = db.program(&[main]);
    assert!(
        messages(&program.diagnostics).iter().any(|m| m == "Circular dependency"),
        "{:?}",
        messages(&program.diagnostics)
    );
}

/// A self-import is the degenerate cycle (`test/syntax/imports/a1/a12.k:6`,
/// `imports.k:537-539`, `expected:1`).
#[test]
fn a_self_import_is_a_cycle() {
    let mut db = db();
    let a = write(&mut db, "a.k", "module a { }\nimport a\n");

    let program = db.program(&[a]);
    assert!(
        messages(&program.diagnostics).iter().any(|m| m == "Circular dependency"),
        "{:?}",
        messages(&program.diagnostics)
    );
}

/// `Parser/IO.hs:108-109` — the message names the file and lists the import
/// directories, and pointedly **not** the importing file's own directory even
/// though that was searched.
#[test]
fn a_missing_import_reports_the_reference_message() {
    let mut db = db();
    let main = write(&mut db, "main.k", "import nowhere.at.all\n");

    let program = db.program(&[main]);
    let text = messages(&program.diagnostics).join("");
    assert!(text.starts_with("Can't find file: nowhere"), "{text:?}");
    assert!(text.contains("in any of the import directories"), "{text:?}");
    assert!(text.contains("/proj"), "{text:?}");
}

/// `parseFileFailure offset err` (`Parser.hs:201`) takes the offset captured
/// *before* `keyword "import"` (`:195-201`), so a missing import is reported at
/// its own `import` keyword — `docs/spec/diagnostics.md` example 4 has the
/// reference pointing at `2:1` for the second line of a file. A failed import
/// is never added to the module graph, so the keyword has to come from the item
/// tree; read off the graph, every such error landed at offset 0, which is the
/// *first* import's keyword or a copyright comment.
#[test]
fn a_missing_import_is_reported_at_its_own_keyword() {
    let mut db = db();
    write(&mut db, "a.k", "module a { }\n");
    let text = "import a\nimport nowhere.at.all\n";
    let main = write(&mut db, "main.k", text);

    let program = db.program(&[main]);
    let missing: Vec<_> = program
        .diagnostics
        .iter()
        .filter(|d| d.message.starts_with("Can't find file: nowhere"))
        .collect();
    assert_eq!(missing.len(), 1, "{:?}", messages(&program.diagnostics));
    let second_import = text.find("import nowhere").unwrap();
    assert_eq!(missing[0].range.file, main);
    assert_eq!(u32::from(missing[0].range.range.start()), second_import as u32);
}

/// `Parser.hs:200-206` / `docs/spec/resolve.md` §1.4: the found file must
/// *declare* the module that was imported, and on a mismatch the import
/// contributes nothing. Oracle: `test/syntax/imports/a1/a2.k` declares
/// `module a1.a3` (`imports.k:270-272`, `expected:1`).
#[test]
fn an_import_whose_file_declares_another_module_is_an_error() {
    let mut db = db();
    write(&mut db, "a.k", "module elsewhere { }\n");
    let main = write(&mut db, "main.k", "import a\n");

    let program = db.program(&[main]);
    assert_eq!(
        messages(&program.diagnostics),
        vec!["Imported module name does not match, expected a"]
    );
    // The import is not in the graph at all — `parseFileFailure` runs *instead
    // of* `updateImports`.
    assert!(program.graph.imports(ModuleSource::File(main)).iter().all(|i| i.name != vec!["a"]));
}

/// `addImport Needed 0 Nothing emptySymbolTable` (`Parser/IO.hs:77`): the base
/// library is import #0 of every command-line file, and the base itself gets
/// none. Without this, every base-library name is invisible even once the file
/// is loaded — which was the whole point of the exercise.
#[test]
fn the_base_library_is_an_implicit_import_of_every_root_but_not_of_itself() {
    let mut db = Db::new(Settings::fallback("/proj"));
    let main = write(&mut db, "main.k", "inline void main() {}\n");

    let program = db.program(&[main]);
    let root_imports = program.graph.imports(ModuleSource::File(main));
    assert_eq!(root_imports.len(), 1);
    assert_eq!(root_imports[0].id, kg_db::program::IMPLICIT_BASE_IMPORT);
    assert!(root_imports[0].needed.is_needed(), "the base import must never warn as redundant");
    assert!(program.graph.imports(ModuleSource::Cmdargs).is_empty());
}

// ---------------------------------------------------------------------------
// The whole-program symbol map — the seam the unwired passes need
// ---------------------------------------------------------------------------

/// `getSymbols` spans the program, not the file: a declaration in an imported
/// module is in the map. This is the fact `check_source` cannot supply and
/// every one of the seven unwired passes is blocked on.
#[test]
fn the_symbol_map_spans_every_loaded_module() {
    let mut db = db();
    write(&mut db, "a.k", "module a { helper }\ninline void helper() {}\n");
    let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

    let symbols = db.global_symbols(&[main], SymbolsConfig::AllSymbols);
    let names: Vec<String> = symbols
        .iter()
        .map(|(key, _)| {
            key.segments()
                .iter()
                .map(|&s| db.interner().resolve(s).to_string())
                .collect::<Vec<_>>()
                .join("::")
        })
        .collect();
    assert!(names.iter().any(|n| n == "@a::helper"), "{names:?}");
    assert!(names.iter().any(|n| n == "main"), "{names:?}");
}

/// `hasErrorSymbol` (`Symbols.hs:31-35`) — the flag `pass`/`checkPar` stops the
/// pipeline on (`Frontend.hs:44-52`). Every `ErrorF` collapses to the one key
/// `["$errors"]`.
#[test]
fn the_symbol_map_reports_whether_anything_failed_to_parse() {
    let mut db = db();
    let ok = write(&mut db, "ok.k", "inline void main() {}\n");
    assert!(!db.global_symbols(&[ok], SymbolsConfig::AllSymbols).has_error_symbol());

    let bad = write(&mut db, "bad.k", "inline void main(\n");
    assert!(db.global_symbols(&[bad], SymbolsConfig::AllSymbols).has_error_symbol());
}

/// The three configs are independent queries, not one map filtered
/// (`Symbols.hs:28`, `docs/spec/resolve.md` §11.5). `NoTemplate` makes a
/// template invisible; `AllSymbols` does not.
#[test]
fn the_three_symbol_configs_disagree_about_templates() {
    let mut db = db();
    let main = write(&mut db, "main.k", "template <auto N>\ninline void f() {}\n");

    let all = db.global_symbols(&[main], SymbolsConfig::AllSymbols);
    let none = db.global_symbols(&[main], SymbolsConfig::NoTemplate);
    assert!(all.len() > none.len(), "{} vs {}", all.len(), none.len());
    assert_eq!(all.config(), SymbolsConfig::AllSymbols);
}

// ---------------------------------------------------------------------------
// Incrementality
// ---------------------------------------------------------------------------

/// The identity firewall. A `didChange` carrying identical text reparses, sees
/// the same green tree, and hands back the *previous* `Arc` — so the item tree,
/// the program and the symbol map are all reused untouched. An editor that
/// re-sends the buffer on every keystroke pays nothing.
#[test]
fn identical_text_keeps_every_downstream_memo() {
    let mut db = db();
    let main = write(&mut db, "main.k", "inline void main() {}\n");

    let before = (
        db.parse(main),
        db.item_tree(ModuleSource::File(main)),
        db.program(&[main]),
        db.global_symbols(&[main], SymbolsConfig::AllSymbols),
    );
    write(&mut db, "main.k", "inline void main() {}\n");
    let after = (
        db.parse(main),
        db.item_tree(ModuleSource::File(main)),
        db.program(&[main]),
        db.global_symbols(&[main], SymbolsConfig::AllSymbols),
    );

    assert!(Arc::ptr_eq(&before.0, &after.0), "the parse was rebuilt");
    assert!(Arc::ptr_eq(&before.1, &after.1), "the item tree was rebuilt");
    assert!(Arc::ptr_eq(&before.2, &after.2), "the program was rebuilt");
    assert!(Arc::ptr_eq(&before.3, &after.3), "the symbol map was rebuilt");
}

/// Editing one buffer must not reparse the other four hundred files. Per-file
/// revisions are what make a keystroke O(one file) instead of O(program).
#[test]
fn editing_one_file_leaves_the_others_parsed() {
    let mut db = db();
    let a = write(&mut db, "a.k", "module a { }\n");
    let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

    let a_before = db.parse(a);
    let main_before = db.parse(main);
    write(&mut db, "main.k", "import a\ninline void main() { uint32 x = 1; }\n");

    assert!(Arc::ptr_eq(&a_before, &db.parse(a)), "an untouched file was reparsed");
    assert!(!Arc::ptr_eq(&main_before, &db.parse(main)), "the edited file was not reparsed");
}

/// A *new* file changes what an import resolves to, so the world moves and
/// programs rebuild — even the ones that never mentioned it. A negative probe
/// is a dependency (`Parser/IO.hs:104-108`).
#[test]
fn creating_a_file_invalidates_resolution() {
    let mut db = db();
    let main = write(&mut db, "main.k", "import late\n");
    assert!(!errors(&db.program(&[main]).diagnostics).is_empty());

    write(&mut db, "late.k", "module late { }\n");
    assert!(
        errors(&db.program(&[main]).diagnostics).is_empty(),
        "{:?}",
        errors(&db.program(&[main]).diagnostics)
    );
}

/// A keystroke in a file that exists before and after cannot change what any
/// import search finds, so it must not move the world — or every keystroke
/// rebuilds every whole-program memo in the database (AUDIT D4). Driven the
/// way the language server drives it: the shared `Vfs` is written first, and
/// the database is told afterwards.
#[test]
fn an_edit_to_an_existing_file_leaves_the_world_alone() {
    let mut db = db();
    let a = write(&mut db, "a.k", "module a { }\n");
    let main = write(&mut db, "main.k", "import a\ninline void main() {}\n");

    let world = db.world_revision();
    let program = db.program(&[main]);

    // Identical text: the parse firewall keeps the item tree, so the program
    // survives exactly when the world did not move.
    db.vfs().set_overlay(a, "module a { }\n".to_owned());
    db.file_changed(a, true);
    assert_eq!(db.world_revision(), world, "an edit to an existing file moved the world");
    assert!(Arc::ptr_eq(&program, &db.program(&[main])), "the program was reloaded");

    // The watcher's door probes for itself, and must reach the same verdict.
    db.invalidate(a);
    assert_eq!(db.world_revision(), world, "a watcher event on an existing file moved the world");
    assert!(Arc::ptr_eq(&program, &db.program(&[main])), "the program was reloaded");

    // A real edit is still seen: new text, new item tree, new program.
    db.vfs().set_overlay(a, "module a { x }\nconst auto x = 1;\n".to_owned());
    db.file_changed(a, true);
    assert_eq!(db.world_revision(), world);
    let edited = db.program(&[main]);
    assert!(!Arc::ptr_eq(&program, &edited), "an edit that changed the item tree was not seen");
    assert!(errors(&edited.diagnostics).is_empty(), "{:?}", errors(&edited.diagnostics));
}

/// The other half of D4: a file the database has never been told about may have
/// been probed absent by an import search, so the first news of it moves the
/// world — and so does every later appearance or disappearance.
#[test]
fn a_file_created_behind_the_databases_back_reaches_its_importer() {
    let mut db = db();
    let main = write(&mut db, "main.k", "import late\ninline void main() {}\n");
    assert!(!errors(&db.program(&[main]).diagnostics).is_empty(), "`late` should be missing");

    let late = db.file_id(Utf8Path::new("/proj/late.k"));
    db.vfs().set_overlay(late, "module late { }\n".to_owned());
    db.file_changed(late, true);
    let found = db.program(&[main]);
    assert!(found.contains(ModuleSource::File(late)), "the new file was not found");
    assert!(errors(&found.diagnostics).is_empty(), "{:?}", errors(&found.diagnostics));

    db.vfs().remove_overlay(late);
    db.file_changed(late, false);
    assert!(!errors(&db.program(&[main]).diagnostics).is_empty(), "a deleted import resolved");

    db.vfs().set_overlay(late, "module late { }\n".to_owned());
    db.file_changed(late, true);
    assert!(errors(&db.program(&[main]).diagnostics).is_empty(), "a recreated import is missing");
}

/// Why [`Db::file_changed`] takes the writer's `exists` instead of probing.
///
/// The language server applies its notifications on a worker, after the main
/// thread has already moved the `Vfs` on. A file that vanishes and comes back
/// between two applications probes "present" both times; a query that ran in
/// the gap memoized the failed import at the current world revision. Only the
/// sequence of after-states the writer recorded shows the gap.
#[test]
fn a_file_that_vanished_and_came_back_unobserved_still_moves_the_world() {
    let mut db = db();
    let late = write(&mut db, "late.k", "module late { }\n");
    let main = write(&mut db, "main.k", "import late\ninline void main() {}\n");

    // The main thread closes the buffer (no file on disk behind it)...
    db.vfs().remove_overlay(late);
    // ...a query runs in the gap, probes, and memoizes the failed import...
    assert!(!errors(&db.program(&[main]).diagnostics).is_empty());
    // ...and the buffer is reopened before either notification is applied.
    db.vfs().set_overlay(late, "module late { }\n".to_owned());

    db.file_changed(late, false);
    db.file_changed(late, true);
    let program = db.program(&[main]);
    assert!(errors(&program.diagnostics).is_empty(), "{:?}", errors(&program.diagnostics));
}

// ---------------------------------------------------------------------------
// The rooted entry point
// ---------------------------------------------------------------------------

/// `check_source` is the rooted door over its text as the program's one
/// file, with no base library and no import directory (sprint W3.7) — so it
/// says what a `Db` under those settings says of the same text, every pass
/// the loop judges included: an enum constant out of its base's range is
/// pass 19's, a `static assert` pass 16's. Until W3.7 it was a second door
/// with its own passes, and this test pinned where the two still agreed and
/// where the text door, running no loop, was silent.
#[test]
fn check_source_is_the_rooted_door_over_its_text_alone() {
    let mut settings = Settings::fallback("/proj");
    settings.implicit_base = false;
    let mut db = Db::new(settings);
    let cases = [
        ("inline void main() {}\n", vec![]),
        ("void main() {}\n", vec!["Free functions must be declared `inline`"]),
        ("enum Foo : uint8\n{\nE1 = 255,\nE2\n}\n", vec!["Enum constant value is out of range for the base type"]),
        ("inline void main() { static assert(1 == 2); }\n", vec!["static assertion failed"]),
        ("const auto Foo = 3 + 4;\nstatic assert(Foo == 8);\n", vec!["static assertion failed"]),
    ];
    for (source, expected) in cases {
        let rooted = messages(&db.check_snippet(Utf8Path::new("/proj/snippet.k"), source));
        assert_eq!(rooted, expected, "in {source:?}");
        assert_eq!(messages(&kg_db::check_source(source)), expected, "in {source:?}");
    }
}

/// `checkSymbols` over a module that exports a name an import also
/// exposes — here under the same namespace, a second `h.dsp` compiled as the root
/// while `b` re-exposes the one the search path finds: `findSymbol` sees
/// two definitions, "symbol is ambiguous" and so "undefined symbol", at the
/// export (`SymbolTable.hs:228-237`, `Parser.hs:219-223`;
/// `library/device/agilex-7-f-023/hardware/dsp.k`). `nub` compares the
/// trees with notes ignored (`Parser/Syntax.hs:594-595`): declared the same
/// way, the two are one, and the program loads (`arria-10-gx-1150`'s).
#[test]
fn an_export_an_import_also_declares_differently_is_ambiguous() {
    let run = |body: &str| {
        let mut db = db();
        write(&mut db, "h/dsp.k", "module h.dsp\n    { f\n    }\n\ninline uint32 f(uint32 x)\n{\n    return x;\n}\n");
        write(&mut db, "b.k", "module b\n    { module h.dsp\n    }\n\nimport h.dsp\n");
        let root = write(
            &mut db,
            "other/h/dsp.k",
            &format!("module h.dsp\n    {{ f\n    }}\n\nimport b\n\ninline uint32 f(uint32 x)\n{{\n    return {body};\n}}\n"),
        );
        errors(&db.check(&[root]))
    };
    assert_eq!(
        run("x + 1"),
        [
            "`f`: undefined symbol",
            "`f`: symbol is ambiguous. It could refer to the definition at\n\t   /proj/other/h/dsp.k:7:1\n\tor /proj/h/dsp.k:5:1",
        ]
    );
    assert_eq!(run("x"), Vec::<String>::new());
}

/// `--Werror` is the driver's (`Main.hs:93-99`): a program the frontend
/// accepts fails on its warnings, which the snippet protocol returns as its
/// errors, kinds kept (`all-warnings-as-errors.k::2`); without the flag, and
/// through `Db::check` with it, they stay warnings.
#[test]
fn werror_makes_a_snippets_warnings_its_errors() {
    let shadow = "inline void main()\n{\n    bool x;\n    {\n        uint32 x;\n    }\n}\n";
    let path = Utf8Path::new("/proj/snippet.k");
    let mut settings = Settings::fallback("/proj");
    let mut db = Db::new(settings.clone());
    let plain = db.check_snippet(path, shadow);
    assert_eq!(plain.iter().filter(|d| d.warning.is_some()).count(), 1, "{plain:?}");
    assert!(!plain.iter().any(kg_diag::Diagnostic::is_error), "{plain:?}");

    settings.warnings_as_errors = true;
    db.set_settings(settings);
    let werror = db.check_snippet(path, shadow);
    assert_eq!(werror.len(), 1, "{werror:?}");
    assert!(werror[0].is_error() && werror[0].warning == Some(kg_diag::WarningKind::Shadow), "{werror:?}");
    let file = db.set_file_text(path, shadow.to_owned());
    assert!(!db.check(&[file]).iter().any(kg_diag::Diagnostic::is_error));
}

/// `Main.hs:73-75` — a parse error aborts before `frontend` is entered, so none
/// of the nineteen passes runs. Reproducing that keeps the passes off a tree the
/// reference never desugars, which is the false-positive direction.
#[test]
fn a_parse_error_anywhere_stops_the_frontend_passes() {
    let mut db = db();
    write(&mut db, "a.k", "module a { }\nvoid broken(\n");
    let main = write(&mut db, "main.k", "import a\nvoid needs_inline() {}\n");

    let out = messages(&db.check(&[main]));
    assert!(
        !out.iter().any(|m| m.contains("must be declared `inline`")),
        "a frontend pass ran despite a parse error: {out:?}"
    );
    assert!(!out.is_empty());
}

/// The language-server answer is per file and does *not* inherit the
/// program-wide gate: a user editing `main.k` keeps seeing its diagnostics while
/// another file is halfway through being typed.
#[test]
fn per_file_diagnostics_survive_a_broken_neighbour() {
    let mut db = db();
    write(&mut db, "a.k", "module a { }\nvoid broken(\n");
    let main = write(&mut db, "main.k", "void needs_inline() {}\n");

    let out = messages(&db.diagnostics(&[main], main));
    assert_eq!(out, vec!["Free functions must be declared `inline`"]);
}

/// A language server owns one `Db` and answers requests from a worker pool, so
/// the database and everything a query hands out have to cross threads.
///
/// This is a compile-time guard with a specific failure it is guarding against:
/// rowan's `SyntaxNode` is a **red** node and is neither `Send` nor `Sync`.
/// Caching one — an `ast::EnumDecl` in a memo entry, say — compiles fine
/// locally and breaks every caller. `Db` stores green trees only, and this is
/// what keeps it that way.
#[test]
fn the_database_and_its_query_results_cross_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Db>();
    assert_send_sync::<kg_db::Program>();
    assert_send_sync::<kg_db::SymbolMap>();
}

// ---------------------------------------------------------------------------
// The reference corpus
// ---------------------------------------------------------------------------

fn corpus_root() -> Utf8PathBuf {
    Utf8PathBuf::from(kg_base::reference_root().to_string_lossy().into_owned())
}

/// The real thing: load `library/mini-base.k` exactly as
/// `test/syntax/CMakeLists.txt:41-45` configures every syntax test, and require
/// that every import in its transitive closure resolves.
///
/// This is the test that would have caught a search order built out of
/// plausible reasoning rather than out of `Parser/IO.hs:110`: `mini-base.k`
/// pulls in `compiler/config.k` and `compiler/device/schema.k`, and the device
/// tree under `library/device/<target>` is reachable only through the
/// device-first half of the path.
#[test]
fn the_reference_base_library_loads_with_no_import_errors() {
    let root = corpus_root();
    let library = root.join("library");
    if !library.is_dir() {
        kg_base::corpus_absent(&format!("no corpus at {root}; set KANAGAWA_ROOT"));
        return;
    }

    let mut settings = Settings::fallback(root.join("test/syntax"));
    settings.import_dirs = vec![library.clone(), root.join("test/syntax")];
    settings.base_library = Some(library.join("mini-base.k"));
    settings.target_device = "mock".to_owned();

    let mut db = Db::new(settings);
    // A snippet in the shape the harness produces: the temp file lives beside
    // the helper directory and imports one helper module.
    let main = db.set_file_text(
        &root.join("test/syntax/syntax_test_temp_file.k"),
        "import helper.string\ninline void main() {}\n".to_owned(),
    );

    let program = db.program(&[main]);
    let failures: Vec<String> =
        program.failures.iter().map(|f| f.error.to_string()).collect();
    assert!(failures.is_empty(), "unresolved imports: {failures:?}");
    assert_eq!(program.base, Some(ModuleSource::File(db.file_id(&library.join("mini-base.k")))));
    // `mini-base.k` reaches `.cmdargs`, `data/closure.k`, `debug/print.k`,
    // `compiler/config.k` and `compiler/device/schema.k`, each of which pulls in
    // more; a program of a handful of files means the search silently failed.
    assert!(program.files.len() > 5, "only loaded {:?}", program.files.len());

    // And the whole-program symbol map is non-trivial, which is the fact the
    // seven unwired passes are waiting for.
    let symbols = db.global_symbols(&[main], SymbolsConfig::AllSymbols);
    assert!(symbols.len() > 100, "only {} symbols", symbols.len());
}

/// **The false-positive gate.** Compile every file the reference ships in
/// `library/` through the rooted door and require *zero* errors.
///
/// This is the test that decides whether the query layer may be wired to the
/// conformance harness at all. 1,768 corpus assertions expect a clean compile
/// and 241 more expect the *C++ backend* to be the one that complains; a single
/// frontend diagnostic invented on library code fails all of them at once
/// (`docs/known-divergences.md`). `check_source` was safe from this by being
/// blind — it could not see `library/` — and the rooted door gives that
/// blindness up, so the safety has to be demonstrated instead of assumed.
///
/// It is also the only place the *loader* is exercised at full depth: the base
/// library, the `.cmdargs` module, the device tree under
/// `library/device/mock/`, the `module A \ B` difference that
/// `compiler/device/config.k` uses to let a device override a default
/// (`docs/spec/resolve.md` §9.1), and the diamond where a dozen modules all
/// import `data/array.k`.
#[test]
fn the_rooted_door_accepts_every_file_of_the_reference_library() {
    let root = corpus_root();
    let library = root.join("library");
    if !library.is_dir() {
        kg_base::corpus_absent(&format!("no corpus at {root}; set KANAGAWA_ROOT"));
        return;
    }

    let mut settings = Settings::fallback(library.clone());
    settings.import_dirs = vec![library.clone()];
    // **`base.k`, not `mini-base.k`.** `base.k` is what `importBase` picks by
    // default (`Parser/IO.hs:72-75`) and what `library/` is written against;
    // `mini-base.k:13-16` re-exposes only `.cmdargs`, `data.closure` and
    // `debug.print`, so under it most of `library/` genuinely has `memory`,
    // `optional` and `static_cast` **out of scope** — `data/datetime.k:42` uses
    // `memory<uint9,32>`, `memory` is declared only in `data/memory.k`, and no
    // import of `data/datetime.k` reaches that file at all. The reference
    // rejects those the same way, so asserting a clean compile under
    // `mini-base.k` was asserting something untrue; only `--base-library` was
    // wrong, and the gate is unchanged in strength.
    settings.base_library = Some(library.join("base.k"));
    // `defaultOptions` sets `targetDevice = "mock"` (`Parser/Options.hs:47`);
    // without it `import hardware.config` has nowhere to resolve
    // (`docs/spec/resolve.md` §9.1).
    settings.target_device = "mock".to_owned();
    let mut db = Db::new(settings);

    let mut files = Vec::new();
    collect_k_files(library.as_std_path(), &mut files);
    assert!(files.len() > 100, "only found {} library files", files.len());

    // `test/runner.k` is a **fragment**, not a program: its `main::run` calls
    // `test_main` (`library/test/runner.k:27`), which every consumer defines for
    // itself (`test/interface/external_class.k:6,247`). Compiled alone it has an
    // undefined symbol, in the reference too — so it is compiled the way it is
    // meant to be, through an importer that supplies the missing definition,
    // rather than dropped from the gate.
    let importer = db.set_file_text(
        &library.join("_test_runner_consumer.k"),
        "import test.runner\n\ninline void test_main()\n{\n}\n".to_owned(),
    );

    // And no `shadow` warning: the false-positive canary of pass 19's
    // `checkShadowDecl` (`TypeCheck.hs:110-121`), which caught a hoisting
    // boundary one component too generous — `data/buffer/gearbox.k:133`'s
    // `bool last` in a lambda beside a `bool last` parameter — when the CST
    // pass reconstructed `moveFunctions` from the text; the loop's tree has
    // it done.
    let shadow = |d: &kg_diag::Diagnostic| d.is_error() || d.warning == Some(kg_diag::WarningKind::Shadow);
    let reported = reported_over(&db, &files, &library, shadow, |path, file| {
        if path.ends_with("test/runner.k") { importer } else { file }
    });
    // Two device files are rejected by the reference itself, compiled as
    // roots under `mock`: each declares `hardware.dsp`, which `base.k` also
    // imports from `device/mock/`, and exports an `fmul32` declared
    // differently from `mock`'s — "symbol is ambiguous", so "undefined
    // symbol", at the export (`kg-oracle --errors` and the recorded backend
    // runs, 2026-09-27; `Db::exposure_ambiguities`). The other device
    // files' declarations are `mock`'s own, which `nub` makes one.
    let rejected = ["device/agilex-7-f-023/hardware/dsp.k", "device/stratix-10-gx-2800/hardware/dsp.k"];
    let (expected, invented): (Vec<&String>, Vec<&String>) =
        reported.iter().partition(|r| rejected.iter().any(|f| r.starts_with(f)));
    assert!(invented.is_empty(), "{} diagnostics, e.g. {:?}", invented.len(), &invented[..1]);
    for f in rejected {
        assert!(expected.iter().any(|r| **r == format!("{f}: `fmul32`: undefined symbol")), "{f}: {expected:?}");
    }
}

/// **The false-positive gate's other half**: every program under
/// `test/library/` — the reference's own tests of its library, whole
/// programs with templates, lambdas and classes the snippets in
/// `test/syntax/` never combine — through the rooted door, against what
/// `kg-oracle --errors` says of each (2026-09-17): every one compiles
/// clean except `processor/risc-v/mmio/mmio.k`, a fragment whose `HARTS`
/// is defined by its importer, rejected four times over (lines 15, 32,
/// 141, 245). The two `static if (HARTS == 1)` predicates of `mmio_load`
/// and `mmio_store` (lines 65, 93) are no verdict: those functions'
/// parameters are typed by `HARTS` too, and inference never types a body
/// whose parameters it could not (`Type/Inference.hs:58`). The CST pass
/// judged them by syntax, six reports, until pass 9 moved to the loop
/// (sprint W3 step 1, 2026-10-10), which gives the reference's four.
///
/// And `processor/risc-v/testbench.k`, which this test once called clean:
/// its `Config::HARTS`, over `import .cmdargs as Config` with no
/// `--define`, is "Symbol name not in scope: `HARTS`" in the reference
/// (`scripts/probe.sh`, the recorded run, 2026-09-27), which also reports
/// the bare `ENABLE_M` beside it — in the selector of a `?:` no fold
/// decides, which the CST pass never judged and the loop's pass 9 does.
///
/// This is where pass 9 over the twin is held to the seam: the copies of
/// `data/fifo/wide_to_narrow.k`'s lambdas hoist their closure aliases
/// above the block declaring what they capture, and `processor/risc-v.k`
/// hoists a lambda body naming its class's `Opcode` to module scope —
/// both are the rewrites' spelling, judged by nobody, and both were
/// reported before the twin's `Whole` learned its `copies` and the closure
/// alias (`docs/journal.md`, "Pass 9 over the twin").
#[test]
fn the_rooted_door_agrees_with_the_reference_on_every_test_library_program() {
    let root = corpus_root();
    let library = root.join("library");
    let programs = root.join("test").join("library");
    if !library.is_dir() || !programs.is_dir() {
        kg_base::corpus_absent(&format!("no corpus at {root}; set KANAGAWA_ROOT"));
        return;
    }

    let mut settings = Settings::fallback(library.clone());
    settings.import_dirs = vec![library.clone(), programs.clone()];
    settings.base_library = Some(library.join("base.k"));
    settings.target_device = "mock".to_owned();
    let db = Db::new(settings);

    let mut files = Vec::new();
    collect_k_files(programs.as_std_path(), &mut files);
    assert!(files.len() > 50, "only found {} programs", files.len());

    let reported = errors_over(&db, &files, &programs, |_, file| file);
    let mut expected = vec!["processor/risc-v/mmio/mmio.k: Symbol name not in scope: `HARTS`"; 4];
    expected.push("processor/risc-v/testbench.k: Symbol name not in scope: `HARTS`");
    expected.push("processor/risc-v/testbench.k: Symbol name not in scope: `ENABLE_M`");
    assert_eq!(reported, expected);
}

/// `--Wno-<name>` is applied by the query layer, not by whoever calls it.
///
/// Worth pinning explicitly: for a while `WarningSet::filter` had **no callers
/// anywhere in the workspace**, so every `--W` flag parsed correctly, was stored
/// correctly, and did nothing. Nothing failed — a disabled warning that still
/// fires looks exactly like a warning.
///
/// The error arm is the other half of the contract: `filter` keeps every
/// diagnostic whose `warning` is `None`, so disabling a warning can never
/// silence an error.
#[test]
fn the_warning_set_reaches_the_diagnostics() {
    const SHADOWED: &str = "inline void main()\n{\n    uint32 a;\n    {\n        uint32 a;\n    }\n}\n";

    let mut db = db();
    let file = write(&mut db, "a.k", SHADOWED);
    let warnings = db.check(&[file]);
    assert_eq!(
        warnings.iter().filter(|d| d.warning == Some(kg_diag::WarningKind::Shadow)).count(),
        1,
        "{:?}",
        messages(&warnings)
    );

    let mut settings = db.settings().clone();
    settings.warnings.disable(kg_diag::WarningKind::Shadow);
    db.set_settings(settings);
    assert!(db.check(&[file]).is_empty(), "{:?}", messages(&db.check(&[file])));

    // Disabling every warning must not touch a hard error.
    let bad = write(&mut db, "b.k", "inline void main() { uint32 x; uint32 y; auto z = x << y; }");
    let mut settings = db.settings().clone();
    settings.warnings = kg_diag::WarningSet::none();
    db.set_settings(settings);
    assert!(db.check(&[bad]).iter().any(kg_diag::Diagnostic::is_error));
}

/// The whole-program memos are keyed by *root set*, so a long-lived language
/// server accumulates one entry per file the user has ever opened. Each pins an
/// `Arc<Program>`. The cap is what stops that being a leak.
///
/// The second half is the part worth pinning: crossing the cap must not break
/// memoization for the file being worked on, only for the ones abandoned.
#[test]
fn the_program_memo_does_not_grow_without_bound() {
    let mut db = db();
    let mut files = Vec::new();
    for i in 0..200 {
        let file = write(&mut db, &format!("m{i}.k"), "inline void main() {}\n");
        db.check(&[file]);
        files.push(file);
    }

    // A cheap proxy for "the map was bounded": if every root set were retained,
    // the first file's program would still be memoized and identical.
    let current = *files.last().unwrap();
    let a = db.program(&[current]);
    let b = db.program(&[current]);
    assert!(Arc::ptr_eq(&a, &b), "the file in hand must still hit its memo");

    // And an edit still invalidates, cap or no cap.
    db.set_overlay(current, "inline void other() {}\n".to_owned());
    assert!(!Arc::ptr_eq(&a, &db.program(&[current])));
}

/// Two roots that share a module namespace can see each other's names.
///
/// `Main.hs:70,80` merges every command-line file into one program, and a
/// declaration inside `module m` is filed under `[@m, Ten]`; a use of `Ten` from
/// another file that also says `module m` resolves by walking outward from that
/// scope (`Symbols.hs:79-87`). Pass 9 probed two *exact* keys instead, so
/// `kg check a.k b.k` rejected a valid program and exited 1.
///
/// Not reachable from the language server, which roots each program at a single
/// file — which is why this survived: the door with the tests is not the door
/// with the bug.
#[test]
fn roots_sharing_a_module_namespace_see_each_others_names() {
    let mut db = db();
    let a = write(&mut db, "a.k", "module m\n{\n}\n\nconst auto Ten = 10;\n");
    let b = write(&mut db, "b.k", "module m\n{\n}\n\nstatic assert(Ten == 10);\n");
    let out = db.check(&[a, b]);
    assert!(out.is_empty(), "{:?}", messages(&out));

    // The other direction still has to work, or the fix is just a mute button.
    let c = write(&mut db, "c.k", "module m\n{\n}\n\nstatic assert(NeverDeclared == 10);\n");
    let out = db.check(&[c]);
    assert!(
        messages(&out).iter().any(|m| m.contains("NeverDeclared")),
        "an undefined name must still be reported: {:?}",
        messages(&out)
    );
}

/// **Navigation, swept over real code.** Ask [`Db::definition_at`] about every
/// path segment of the reference library and require two things of every answer:
/// that it exists, and that the declaration it names is spelled the way the
/// reference was.
///
/// The spelling check is what makes this more than a crash test. A resolver can
/// be wrong in exactly one visible way — landing on some *other* declaration —
/// and a jump whose target reads `foo` when the caret was on `bar` is that bug
/// caught without an oracle for what the right target would have been. It runs
/// over class hierarchies, template members, `static if` bodies, aliased imports
/// and eight hundred lines of `data/array.k`, none of which a hand-written
/// fixture would have covered.
///
/// The floor on the resolution rate is the other half: a query that quietly
/// started declining everything would satisfy the spelling check vacuously.
#[test]
fn every_name_the_reference_library_writes_resolves_or_is_a_known_decline() {
    let root = corpus_root();
    let library = root.join("library");
    if !library.is_dir() {
        kg_base::corpus_absent(&format!("no corpus at {root}; set KANAGAWA_ROOT"));
        return;
    }

    let mut settings = Settings::fallback(library.clone());
    settings.import_dirs = vec![library.clone()];
    settings.base_library = Some(library.join("base.k"));
    settings.target_device = "mock".to_owned();
    let db = Db::new(settings);

    let mut files = Vec::new();
    collect_k_files(library.as_std_path(), &mut files);
    assert!(files.len() > 100, "only found {} library files", files.len());

    let (mut asked, mut resolved) = (0usize, 0usize);
    for path in files.iter().take(60) {
        let path = Utf8PathBuf::from_path_buf(path.clone()).expect("utf-8 corpus path");
        let file = db.file_id(&path);
        for element in db.parse(file).syntax_node().descendants_with_tokens() {
            let Some(token) = element.into_token() else { continue };
            if token.kind() != kg_syntax::SyntaxKind::IDENT {
                continue;
            }
            // Path segments only. A module path, an attribute name and a `.`
            // field access are documented declines and are not the subject.
            let in_segment = token.parent().map(|n| n.kind())
                == Some(kg_syntax::SyntaxKind::NAME_REF)
                && token.parent().and_then(|n| n.parent()).map(|n| n.kind())
                    == Some(kg_syntax::SyntaxKind::PATH_SEGMENT);
            if !in_segment {
                continue;
            }
            asked += 1;

            let Some(found) = db.definition_at(&[file], file, token.text_range().start())
            else {
                continue;
            };
            resolved += 1;
            // A declaration in `.cmdargs` or `.options` has no on-disk text to
            // read; the answer is still well-formed, and its `file` says so.
            if found.range.file == FileId::SYNTHETIC {
                continue;
            }
            let target = db.parse(found.range.file).text();
            assert_eq!(
                &target[found.range.range],
                token.text(),
                "`{}` at {:?} in {path} jumped to a declaration named `{}` in {:?}",
                token.text(),
                token.text_range(),
                &target[found.range.range],
                db.path(found.range.file),
            );
        }
    }

    assert!(asked > 5_000, "only {asked} names were asked about");
    // Measured at 96%. The floor is well below that so that a *new* documented
    // decline does not fail the build, while a resolver that stopped answering
    // does.
    assert!(
        resolved * 10 >= asked * 9,
        "only {resolved} of {asked} names resolved; the query stopped answering"
    );
}

/// An enum constant reaches `inductionVarType` folded, like any other constant.
///
/// `isIntLiteral` and `fromIntegralConstexpr` each unwrap an `EnumValue` before
/// matching (`Type.hs:1088`, `:1099`), and `EnumValueF` is built in exactly one
/// place in the compiler — `evaluateConstexpr` (`Constexpr.hs:292`). Those
/// unwrapping equations exist only because the tree is folded by the time the
/// match runs.
///
/// Pinned through the narrowing warning because it names the width it computed:
/// `clog2 256 = 8`, so a `uint7` parameter narrows *from `const uint8`*. Before
/// folding accepted enums the induction variable took the enum's base type and
/// the same line read `from `const uint16``, which is why the assertion is on
/// the spelling and not merely on a warning being present.
#[test]
fn an_enum_constant_limit_folds_like_an_integer_one() {
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    settings.warnings.enable(kg_diag::WarningKind::Conversion);
    let mut db = Db::new(settings);

    let f = write(
        &mut db,
        "e.k",
        "enum E : uint16\n{\n    Small,\n    Big = 256\n}\n\n\
         inline void V7(uint7 x) { }\n\
         inline void main() { for (const auto i : E::Big) { V7(i); } }\n",
    );
    let msgs = messages(&db.check(&[f]));
    assert!(
        msgs.iter().any(|m| m.contains("from `const uint8` to `uint7`")),
        "the limit must fold to 256 and size the induction variable `clog2 256 = 8`: {msgs:?}"
    );
}

/// Every error `db.check` reports for each of `files`, as `<path relative to
/// base>: <first line>`, in file order — the shape both corpus gates assert on.
///
/// Spread over threads sharing the one `Db`: the query layer is `Sync` (pinned
/// above), so eight threads walk the corpus through one memo and the base
/// library is inferred once. In file order regardless of which thread got
/// there first, so a failure reads the same on every run. This walk was 40 of
/// the workspace suite's 68 seconds when it ran one file at a time.
fn errors_over(
    db: &Db,
    files: &[std::path::PathBuf],
    base: &Utf8Path,
    roots_for: impl Fn(&Utf8Path, FileId) -> FileId + Sync,
) -> Vec<String> {
    reported_over(db, files, base, kg_diag::Diagnostic::is_error, roots_for)
}

/// [`errors_over`], keeping what `keep` keeps.
fn reported_over(
    db: &Db,
    files: &[std::path::PathBuf],
    base: &Utf8Path,
    keep: impl Fn(&kg_diag::Diagnostic) -> bool + Sync,
    roots_for: impl Fn(&Utf8Path, FileId) -> FileId + Sync,
) -> Vec<String> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    let workers = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    let next = AtomicUsize::new(0);
    let out: Vec<Mutex<Vec<String>>> = files.iter().map(|_| Mutex::new(Vec::new())).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(path) = files.get(i) else { break };
                let path = Utf8PathBuf::from_path_buf(path.clone()).expect("utf-8 corpus path");
                let file = db.file_id(&path);
                let roots = roots_for(&path, file);
                let name = path.strip_prefix(base).unwrap_or(&path);
                let errors: Vec<String> = db
                    .check(&[roots])
                    .iter()
                    .filter(|d| keep(d))
                    .map(|d| format!("{name}: {}", d.message.lines().next().unwrap_or("")))
                    .collect();
                *out[i].lock().expect("no panics hold this lock") = errors;
            });
        }
    });
    out.into_iter().flat_map(|m| m.into_inner().expect("threads joined")).collect()
}

fn collect_k_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_k_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

// ---------------------------------------------------------------------------
// The cross-module type environment
// ---------------------------------------------------------------------------

/// The type of the declaration `name` binds, in `file`: the program-wide
/// loop's, at the declaration (`Db::ir_type_of_definition`), `Unresolved`
/// where the loop has none.
///
/// Goes through the public queries a hover goes through, so what is asserted
/// is what an editor is told. These were the CST inference engine's tests —
/// its per-module environment and the imports it read — until sprint W3.7
/// deleted it; the visibility they pin is the loop's to keep.
fn type_of(db: &Db, roots: &[FileId], file: FileId, name: &str) -> kg_ty::Ty {
    let text = db.vfs().read(file).expect("the fixture was written").text;
    let at = text.find(name).expect("the fixture spells the name");
    let offset = kg_base::TextSize::new(at as u32);
    let def = db.definition_at(roots, file, offset).expect("the name resolves to a declaration");
    let looped = db.editor_loop(roots, file).expect("the fixture parses");
    db.ir_type_of_definition(&looped, &def).unwrap_or(kg_ty::Ty::Unresolved)
}

/// Every error message `kg check` gives the program rooted at `roots`.
fn errors_of(db: &Db, roots: &[FileId]) -> Vec<String> {
    db.check(roots).into_iter().filter(|d| d.is_error()).map(|d| d.message).collect()
}

/// `Struct x;` where `Struct` is another module's is that struct's type: the
/// environment is the program's (`getSymbols' TemplateDecl` over every
/// module's tree, `Inference.hs:37`), reached through the import.
#[test]
fn a_type_declared_in_an_imported_module_resolves() {
    let mut db = db();
    write(&mut db, "a.k", "module a { Aa }\nstruct Aa\n{\n    uint32 f;\n};\n");
    let main = write(
        &mut db,
        "main.k",
        "import a\ninline void main()\n{\n    Aa imported;\n}\n",
    );

    let ty = type_of(&db, &[main], main, "imported");
    assert!(matches!(ty, kg_ty::Ty::Struct(..)), "expected the struct's own type, got {ty:?}");
    assert_eq!(errors_of(&db, &[main]), Vec::<String>::new());
}

/// **And no further.** A module sees what it imports and what those re-export
/// (`findInImports`, `SymbolTable.hs:263-270`) — not every declaration in the
/// program.
///
/// `b` is *in* the program: `a` imports it. It is not in `main`'s scope,
/// because `a`'s export list is `{ Aa }` and `updateVisible`'s `reExposed`
/// keeps only what the importer re-exposes (`SymbolTable.hs:164-166`). An
/// environment that simply merged every module's map would resolve `Bb` here
/// and start inventing types on programs the reference rejects, and silence
/// on the rejection — `Bb` is undefined (`kg-oracle --errors`, 2026-10-10).
#[test]
fn a_type_from_a_module_this_one_does_not_import_stays_unresolved() {
    let mut db = db();
    write(&mut db, "b.k", "module b { Bb }\nstruct Bb\n{\n    uint32 g;\n};\n");
    write(&mut db, "a.k", "module a { Aa }\nimport b\nstruct Aa\n{\n    uint32 f;\n};\n");
    let main = write(
        &mut db,
        "main.k",
        "import a\ninline void main()\n{\n    Aa visible;\n    Bb hidden;\n}\n",
    );

    // The differential is the point: one file, one import, two names, and only
    // the exported one has a type.
    let visible = type_of(&db, &[main], main, "visible");
    assert!(matches!(visible, kg_ty::Ty::Struct(..)), "{visible:?}");
    let hidden = type_of(&db, &[main], main, "hidden");
    assert!(!matches!(hidden, kg_ty::Ty::Struct(..)), "`Bb` is not exported by `a`, so it must not be visible in `main`: {hidden:?}");
    assert_eq!(errors_of(&db, &[main]), ["Type name not in scope: `Bb`\nPerhaps you meant: `Bb` defined in b"]);
}

/// **The fold and the type must agree about scope.** `inferExpTree` hands one
/// map to `evaluateConstexpr` and to `inferExpType` (`Inference.hs:37,47`), so
/// `inductionVarType`'s `isIntLiteral` guard (`Type.hs:1133-1137`) is true of an
/// imported `const` exactly as it is of a local one — `Constexpr.hs:286-293`
/// folds any `const` integer with a literal initializer through the whole-program
/// map, and `test/syntax/auto-placeholder.k:39-49` pins the local case at
/// `bitsizeof(x) == 3`.
///
/// The CST engine's two maps disagreed — its type half reached the imports,
/// its constant half one file — and without a guard the induction variable
/// came out `const uint32`, a *narrowing conversion* warning on a program
/// the reference accepts. The loop holds one map, and `x` is `const uint3`
/// — `static assert(decltype(x) == const uint3)` holds in the reference
/// (`kg-oracle --errors`, 2026-10-10). Divergence #20, closed with the CST
/// engine.
#[test]
fn an_imported_constant_limit_does_not_type_the_induction_variable_by_its_width() {
    let mut db = db();
    write(&mut db, "k.k", "module k { Limit }\nconst uint32 Limit = 8;\n");
    let main = write(
        &mut db,
        "main.k",
        "import k\ninline void main()\n{\n    for (const auto x : Limit)\n    {\n    }\n}\n",
    );

    let ty = type_of(&db, &[main], main, "x : Limit");
    assert_eq!(ty, kg_ty::Ty::Const(kg_ty::Ty::Unsigned(3).into()));
}

/// **Two roots sharing a namespace share types.** The reference merges every
/// command-line file into one symbol map (`Main.hs:70,80`,
/// `Symbols.hs:37-41`), so a declaration in `r1.k` is in `r2.k`'s scope with
/// no import at all: `bool b = sibling` is "Invalid conversion from
/// `same.ns::Local1` to `bool`" there and here (`kg-oracle --errors`,
/// 2026-10-10). The loop's program is the roots' together, as the
/// reference's is.
///
/// Until sprint W3.7 this pinned the opposite — the CST inference engine's
/// per-module environment resolved only through the import graph, and a
/// sibling root's type stayed unresolved (divergence #19, the price of its
/// memo). That engine is deleted, and the divergence with it.
#[test]
fn two_roots_in_one_namespace_see_each_others_types() {
    let mut db = db();
    let r1 = write(
        &mut db,
        "r1.k",
        "module same.ns { Local1 }\nstruct Local1\n{\n    uint32 f;\n};\n",
    );
    let r2 = write(
        &mut db,
        "r2.k",
        "module same.ns { Use }\ninline void Use()\n{\n    Local1 sibling;\n    bool b = sibling;\n}\n",
    );

    let roots = [r1, r2];
    let ty = type_of(&db, &roots, r2, "sibling");
    assert!(matches!(ty, kg_ty::Ty::Struct(..)), "a sibling root's struct, as the reference merges the roots: {ty:?}");
    assert_eq!(errors_of(&db, &roots), ["Invalid conversion from `same.ns::Local1` to `bool`"]);
}

// ---------------------------------------------------------------------------
// The loop's instances (`kg_db::instances`)
// ---------------------------------------------------------------------------

/// `decidedInstances` reaches across the import graph: the template lives in
/// `a.k`, the instance in `main.k`, and the instance the loop made is listed
/// under the declaring module's qualified name, spelled as a message spells
/// it, decided at the `Vec<uint8, 4>` the user wrote. Until sprint W3.7 the
/// second census answered this; it is the loop's census now.
#[test]
fn an_instance_of_an_imported_template_is_decided() {
    let mut db = db();
    write(
        &mut db,
        "a.k",
        "module a { Vec }\ntemplate <typename T, auto N>\nstruct Vec\n{\nT[N] x;\n}\n",
    );
    let main = write(
        &mut db,
        "main.k",
        "import a\ninline void main()\n{\nstatic Vec<uint8, 4> v;\n}\n",
    );

    let set = kg_db::instances::instances(&db, &[main]);
    assert!(!set.erred);
    let vec = set.templates.get("a.Vec").unwrap_or_else(|| {
        panic!("expected a.Vec decided, got {:?}", set.templates.keys().collect::<Vec<_>>())
    });
    assert_eq!(vec.iter().map(|i| (i.spelled.as_str(), i.round)).collect::<Vec<_>>(), [("Vec<uint8, 4>", 1)]);
    // The deciding site is in `main.k` — where the user wrote `Vec<uint8, 4>`.
    let site = vec[0].site;
    assert_eq!(site.file, main);
    assert_eq!(&db.parse(main).text()[site.range], "Vec<uint8, 4>");
}

/// A class template's member template is filed under its declaration,
/// `a.Box.get`: its copy's `TInstance` names the member inside the class's
/// instance (`Box<|$uint@32|>.get`), and that instance's own note says it
/// is `Box`'s (`lookupSymbolByName`, `Type.hs:198-205`). Decided a round
/// after the class (`Template.hs:58-90`), and spelled through the copy it
/// was made in.
#[test]
fn a_member_template_of_a_class_instance_is_filed_under_its_declaration() {
    let mut db = db();
    write(
        &mut db,
        "a.k",
        "module a { Box }\ntemplate <typename T>\nclass Box\n{\npublic:\n    template <auto N>\n    T get()\n    {\n        return N;\n    }\n}\n",
    );
    let main = write(
        &mut db,
        "main.k",
        "import a\ninline uint32 main()\n{\nstatic Box<uint32> b;\nreturn b.get<3>();\n}\n",
    );

    let set = kg_db::instances::instances(&db, &[main]);
    assert!(!set.erred);
    let listed: Vec<(&str, &str, u32)> = set
        .templates
        .iter()
        .flat_map(|(t, is)| is.iter().map(move |i| (t.as_str(), i.spelled.as_str(), i.round)))
        .collect();
    assert_eq!(listed, [("a.Box", "Box<uint32>", 1), ("a.Box.get", "Box<uint32>::get<3>", 2)]);
}

/// **A settings flip that changes the parse must invalidate the parse memo.**
///
/// `Db::stamp` is the *text* revision for a `ModuleSource::File`, so before
/// `parse_docs` reached the parser nothing about a `set_settings` call could
/// invalidate a file's parse. The flag changes the error list — a bare `//|`
/// carries no declaration and is a parse error under `--parse-docs`
/// (`Parser/Lexer.hs:76-88`, `Parser.hs:92-97`) and plain whitespace without
/// it — so serving the cached parse would report the wrong diagnostics for the
/// rest of the session.
///
/// Load-bearing, and the plant lands earlier than it looks: delete the
/// `memo.parse_docs == parse_docs` conjunct in `Db::parse_module` and the
/// **second** assertion fails (`left: 0, right: 1`), not the third — the stale
/// parse is served the moment the flag turns *on*, so the round trip back to
/// off never gets a chance to be wrong. Run it; the failing line is not the
/// one a reading of this test predicts.
#[test]
fn flipping_parse_docs_invalidates_the_parse_memo() {
    // A doc comment with nothing after it to carry it.
    const SRC: &str = "inline void main()\n{\n}\n\n//|\n";
    let mut db = db();
    let f = write(&mut db, "main.k", SRC);
    let module = ModuleSource::File(f);

    assert_eq!(
        db.parse_module(module).errors().len(),
        0,
        "with the flag clear every comment is whitespace"
    );

    let mut on = Settings::fallback("/proj");
    on.import_dirs = vec![Utf8PathBuf::from("/proj")];
    on.implicit_base = false;
    on.parse_docs = true;
    db.set_settings(on);

    assert_eq!(
        db.parse_module(module).errors().len(),
        1,
        "the dangling doc comment is a parse error under --parse-docs"
    );

    let mut off = Settings::fallback("/proj");
    off.import_dirs = vec![Utf8PathBuf::from("/proj")];
    off.implicit_base = false;
    db.set_settings(off);

    assert_eq!(
        db.parse_module(module).errors().len(),
        0,
        "and flipping back must re-parse rather than serve the flag-on memo"
    );
}

/// The editor's first answer, a loop stopped short of its fixpoint, leaves
/// diagnostics out, never puts one in: `test/library/control/loop.k`
/// compiles clean to the fixpoint, and at the editor's four rounds it once
/// showed 25 "Failed to instantiate template" errors — pass 12 judging
/// instances the next round would have made (2026-10-10). The reference
/// has no budget (`Template.hs:55-64`), so a leftover verdict over a
/// truncated loop is never its own. Red with `kg_sem::check`'s
/// `truncated` ignored, or the first answer built as a last one.
#[test]
fn the_editor_s_first_answer_invents_no_error() {
    let root = corpus_root();
    let library = root.join("library");
    let file = root.join("test/library/control/loop.k");
    if !file.is_file() {
        kg_base::corpus_absent(&format!("no corpus at {root}; set KANAGAWA_ROOT"));
        return;
    }
    let mut settings = Settings::fallback(root.join("test/library/control"));
    settings.import_dirs = vec![library.clone()];
    settings.base_library = Some(library.join("base.k"));
    settings.target_device = "mock".to_owned();
    let mut db = Db::new(settings);
    db.set_pass_budget(kg_db::PassBudget::EDITOR);
    let text = std::fs::read_to_string(file.as_std_path()).expect("the corpus file reads");
    let main = db.set_file_text(&file, text);
    let mut first = None;
    let last = db.diagnostics_layered(&[main], main, |d| first = Some(d));
    let first = first.expect("the loop must run past the first bound, or the test grades nothing");
    assert_eq!(errors(&first), Vec::<String>::new());
    assert_eq!(errors(&last), Vec::<String>::new());
    assert!(db.budget_exhausted(&[main]).is_none(), "the last answer is the fixpoint's");
}
