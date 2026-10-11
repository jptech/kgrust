# One source of truth per language construct

Status: **done as sprint W4 defines it** (2026-10-10): every "copies
today" cell below is empty or names a pair in the register, with its
reason and the gate that compares the two. D1.1, D1.3 and D1.4 landed
2026-09-25, D1.2 with sprint W1.1 (2026-09-27), D1.8 by deletion with the
CST inference engine (W3.7). After W3.7 a survey (2026-10-10) rewrote each
open row's copies to what stood; the rest landed the same day
([`sprint-parity.md`](sprint-parity.md) W4). Open, a D2 change of
verdicts: the parser's template checks onto the sorted lowering's facts
(the register's CST parse-time row). Proposed 2026-09-25 as the companion
to [`ir-elaboration.md`](ir-elaboration.md).

## Principle

Every core construct has **one owner**: a name lookup, a constant's value, a
type's spelling, a template's identity, a `static if`'s verdict, an instance's
name.

A second implementation is allowed only when there is a *stated reason* and a
*gate that compares the two*. Both go in the register at the end of this
document. A copy without both is a bug waiting for its drift.

The evidence that this matters is in the code. The 2026-09-25 inventory found
at least six template indexes, five constant folders, four readers of mangled
text, about seven "written type → `Ty`" conversions and four encodings of
template arguments. **Nine drift bugs are already dated in comments.** Four of
them are:

- `own_templates` counting member templates (`elaborate.rs:3399`);
- `top_level_by_leaf` picking the wrong member (`:3900`);
- the IR `written_builtin` accepting hex widths that the CST one rejects
  (`instances.rs:1767`);
- `Shadow` lacking `Binders`' parse-order rule — confirmed, not yet fixed.

Three more were found and fixed in this session alone:

- the `static if (true)` rule missing from three readers;
- a template index without the import-visibility map;
- `Templates` unable to answer a name two modules declare.

## How it relates to the elaboration plan

Most CST/IR pairs exist *only* because inference runs on the CST and pass 8 on
the IR. [`ir-elaboration.md`](ir-elaboration.md) deletes the CST side of those
at M4–M6.

So the rule for this plan is:

- **deduplicate now what survives the migration;**
- **do not invest in** a pair whose CST half the migration deletes.

For those, freeze the CST half and make the IR half the owner.

## D1 — Pure consolidation (gate-neutral, do first)

Each item is a refactor that must leave every gate byte-identical. Each lands
as its own commit.

**Surveyed 2026-10-10**, after sprint W3.7 deleted the CST inference engine
(`26700d6`) and the second census (`b56fd7b`), and with them most of what
the rows were written against. The last column is what stands *today*, read
in the code, by `file:line`; what went is named with the commit that took
it.

| # | Construct | Owner | Copies today (2026-10-10) |
|---|---|---|---|
| D1.1 | **Names and mangling** | `kg_base::name` — **landed 2026-09-25**: `mangle_namespace`/`unmangle_namespace` (exactly `moduleNamespace`/`unmangleModuleNamespace`, `Parser/Syntax.hs:273-279`; `kg-resolve`'s own had replaced `@` in a segment with no leading one), `is_instance_name`/`mentions_instance`; `ModuleIr::top_level_items`/`dotted_module_name`. `mangle_instance` stays in `rewrite/resolve.rs` until the `InstanceRegistry` (M5) owns naming, and the `__kg_m` tests stay with the rename layer M4 deletes | none of the 14 inlined namespace conversions, the 32 `contains("<|")`/`ends_with("|>")` tests or the two `top_level_items`. Found by the survey and closed 2026-10-10: the instance bracket written by hand beside `rewrite/resolve.rs`'s mangler — `rewrite/instantiate.rs` (three) and `kg-sem/src/census.rs` — which `xtask lint`'s bare `"<|"` needle did not see; `resolve::instance_suffix` writes it now, and `mangled-text` counts `<|{` too |
| D1.2 | **Reading instances back** | the loop's instance note: `Ty::Instance` and its `TArg`s, displayed by `unmangleInstanceName` over the note (`kg_ty::subtype::unmangle_instance_name_with`, `subtype.rs:754`); the census keys an instance by the structural encoding of its argument tree (`kg_hir::structure`, `kg-sem/src/census.rs:75`) — **landed 2026-09-27 (sprint W1.1 steps 1–3)**: no production code parses a mangled name, `xtask lint`'s `mangled-readers` and `mangled-text` at 0 | none. walk.rs's six splitters went in `40d96cb` (W1.1 step 2); `subtype.rs`'s `unmangle_text`/`unmangle_leaf`/`unmangle_item` and the CST engine's `mangled_instance`/`mangled_arg` in `012a260` (step 3). What still looks at a name's text takes no structure out of it: D1.1's predicate `mentions_instance` (`kg-ptbridge/src/walk.rs:232,2631`; `kg-hir`'s `pretty.rs:257`, `rewrite/resolve.rs:685`, `rewrite/instantiate.rs:1199,1302`) |
| D1.3 | **Constant evaluation** | `kg_hir::constexpr` — **landed 2026-09-25**: `evaluateConstexpr`'s rules as functions of values, `value()` bottom-up with a `Leaves` hook, all arithmetic via `fold_binary_values`. The CST `ConstEval` was frozen, then deleted with the CST engine (W3.7) | none: `rewrite/mod.rs exp_value`/`static_value`, `instances.rs Dependent::value`, `kg-hir width::value` (own `i128` arithmetic) and `walk.rs literal_arithmetic` replaced. `static_value`, which survived as a call of `constexpr::value` on the twin's dead path, went with it (D2.4, 2026-10-10) |
| D1.4 | **Node → type key** | `TypeKey::at(file, range, kind, via)` and `NodeData::type_key()` — **landed 2026-09-25**, 19 production literals replaced (the rest are test fixtures). The partner-then-base `NodeTypes` service was dropped from this step: ir-elaboration M3 replaces all three lookups with the `TypeMap`, so it would be written to be deleted | none (`xtask lint`'s `type-key-literal` at 0) |
| D1.5 | **Written type → `Ty`** | `kg_hir::constexpr::written_type` (`constexpr.rs:210`) for a type spelled without a table; the loop's `infer` arms (`kg-sem/src/infer.rs:1381-1577`, `Type.hs:1174-1340`) for every node it types — **2026-10-10: no copy outside the register** | Gone: `rewrite/mod.rs written_builtin_ty` and `width.rs width_of` (`3b055d7`, D1.3), `bound_argument`'s head (`81452c6`, M4). **Kept (the register), each measured by an instrumented `ptdiff` over its 2422 programs:** `rewrite/instantiate.rs written_builtin`, `t_arg`'s fallback for a written builtin the loop's map leaves untyped — the reference's `tArg` has no such arm (`Type.hs:431-437`), yet it fires twice: `template.k`'s `Bar<uint6, 101>` over `Foo<T, 100> N`, whose indirect instance's argument the census mints untyped where the reference substitutes the typed written node; `kg-sem/src/census.rs kind_type`, `resolveTemplateParamType` with the template's parameters substituted and no tree built, whose seven scalar arms are `written_type`'s; `kg-ptbridge/src/walk.rs written_integer_type` with the rest of `set_node_type`'s fallbacks, which fired 0 times |
| D1.6 | **Literal typing** | `inferIntLiteralType` (`Type.hs:485-491`) in the value domain, `kg_const::LiteralType::minimal` and `ConstValue::literal_type`, which `kg-ty` depends on (the crate order, `kg-ty` → `kg-const`, forbids the plan's other direction); `kg_ty::width::infer_int_literal_type`/`minimal_int_type` its image in `Ty` — **landed 2026-10-10** | none. Gone: `infer.rs:2086` (the CST engine, `26700d6`), `deduce_template_args.rs`'s (W3.1); then `minimal_int_type`/`infer_int_literal_type`'s own arithmetic (`kg-ty/src/width.rs`), which computed `LiteralType::minimal`/`literal_type` (`kg-const/src/value.rs:84,403`) a second time, and `walk.rs`'s `folded_initializer_type` by hand, both now calls |
| D1.7 | **Small shared rules** | one each: `Dependency` and `stands_unrolled` (`rewrite/static_for.rs`); `constIndexType`, the loop's (`kg-sem/src/infer.rs:1322`); `kg_hir::rewrite::is_local_scope` (`Desugar.hs:176-180`); `kg_hir::rewrite::is_abbreviated_template` (`Parser/Syntax.hs:722-723`); `kg_const::LiteralType::truncate` (`Internal.hs:34-41`); `kg_ty::subtype::t_super_pretty` (`Type.hs:899-923`) — **landed 2026-10-10** | Gone: `kg_const::Dependency` with `induction_dependency`, `constIndexType`'s `infer.rs:2460` and `isLocalScope`'s `infer.rs:3477` (the CST engine, `26700d6`); `is_abbreviated_template`'s `kg-db` `facts.rs:100` (`cea5957`, W3.1); then `isLocalScope`'s two inline copies in `rewrite/move_functions.rs`, `isAbbreviatedTemplate`'s `has_auto_param` there and `is_abbreviated_template` in `walk.rs`, the stamp's `truncateInt`/`truncateUInt` by sign written six times (`rewrite/instantiate.rs` ×3, `rewrite/resolve.rs` ×2, `kg-db/src/passes/early_normalization.rs`), and `kg_ty::width::t_super`, clauses 1–7 transcribed beside `subtype`'s (the register's row); `rewrite/static_for.rs`'s `const_index_width`, `constIndexType` on the twin's dead path (with D2.4). The one drift: `kg-sem/src/check.rs`'s `abbreviated` also took `const auto`, so every check that skips an abbreviated template was silent on `F(const auto x)`, where the reference says "Could not deduce type" (`kg-oracle --errors`; no corpus program writes one) — it asks the owner now, its own commit. Kept, the register: the firing guard `rewrite/abbreviated.rs is_auto_param` and its per-node summary `kg-hir/src/facts.rs:128 auto_params` |
| D1.8 | **Scope derivation** | `kg_resolve::ScopeStack` — **landed by deletion (W3.7, `26700d6`)** | none. `infer.rs scope_for_children` went with the CST engine, `types.rs Walk::node`/`child_scope` (and the duplicate-key poisoning) with `kg_db::passes::types`. Both lowerings walk with `ScopeStack` (`kg_resolve::lower_file`; `kg-hir/src/lower.rs:43,137,277-281`), and pass 9's use scope replays the item tree's scopes (`undefined_symbols.rs:902`). The CST walks left read the *parser's* table at a position (D2.3) or serve the editor (`navigate.rs`, the register) |

**Enforcement.** `xtask lint` greps for the retired spellings — `"<|"` and `<|{`,
`"__kg_m"`, `replace('@'`, `TypeKey {` — outside their owner modules and
fails on a new one. That is the ratchet that keeps D1 from regrowing.

## D2 — Consolidation that fixes drift (behaviour changes, ratcheted)

Each item unifies copies that *disagree today*. Expect gate movement, read
every diff, and land each one separately with its evidence.

The same survey (2026-10-10). "Known drift" is what each row was written
against; the last column is what stands now.

| # | Construct | Owner | Known drift it fixes | Copies today (2026-10-10) |
|---|---|---|---|---|
| D2.1 | **Template identity and lookup** | after the parse: the loop's `kg_sem::symbols::Symbols` (`getSymbols`, `lookupInScope`; `symbols.rs:79,85`), program-wide, by qualified name — every template lookup of deduction, the census, closures, inference, pass 1 and the checks asks it. The planned `TemplateIndex` was not needed: the sorted lowering resolves names and the loop asks one map | the three dated bugs above; homonyms answered differently by each copy; `Templates::of` rebuilt per module (O(modules²), AUDIT §4.2) | Gone: `IrTemplates` (`b56fd7b`); `own_templates`/`own_top_level_templates`/`top_level_by_leaf`, `defaults::homonym`, `with_tables` (`81452c6`, M4; `defaults.rs` in `b56fd7b`) — with them the dated bugs and the homonym drift. **Standing, kept (the register):** the CST `Templates` (`kg-db/src/passes/templates.rs:55`) — top-level templates by leaf, a name two modules declare dropped — read now by two parser checks only (`early_normalization.rs:101,746`) and still built per module (`db.rs:1417`, through `with_cst_inputs`, `:1441`); walk.rs `template_functions`/`declared_copies` (`walk.rs:193,217`), leaf-name sets that only decline. Deleted 2026-10-10: `kg-db/src/sorted.rs`'s `named_template`, over a `templates` map its one constructor left empty (dead since `81452c6`) |
| D2.2 | **Import visibility** | `kg_resolve::ModuleGraph::find_in_imports` (`module.rs:1045`, `findInImports`), which every other door asks (`sorted.rs:232`, `navigate.rs:655`, `imports.rs:100`, `kg-resolve/src/scope.rs:549,570`, `undefined_symbols.rs:312,430,560,603,675`) | templates reached through a re-export | Gone: `IrTemplates::get_from` (`b56fd7b`), and its re-export drift; `import_spellings`/`leaf_spellable`/`foreign_function_namespace` (`81452c6`). **Standing, kept (the register):** `db.rs:1578 visible_through_imports` — each unaliased import's two-segment interface keys, one file per name (where two imports expose one name the last written wins; `find_in_imports` returns both) — feeding `Templates::visible` (`templates.rs:57-96`) for one parser check (`early_normalization.rs:102`). Deleted 2026-10-10, the twin's import half, dead since `81452c6`: `sorted.rs`'s `find_in_twins` and `twin_token`, over a `twins` map left empty, and `SymbolMap::base` (`kg-db/src/symbols.rs`), always `None` |
| D2.3 | **Shadowing** | the parser's table at a position (`parseSymbol`, `findSymbol`; `SymbolTable.hs:194-261`), which every check made before the passes asks | `Shadow` binds a variable before its declaration ends | Gone: `Shadow`, the IR reading, with the second census (`b56fd7b`), and its drift. **Standing, kept (the register):** four CST readings of the parser's table, for the parse-time checks and the editor — `Binders` (`kg-db/src/passes/templates.rs:182`; the parse-order rule for a variable only) behind `shadowed_before_module_scope` (`:141`; one check, `early_normalization.rs:78`); `innermost_declaring_scope` (`undefined_symbols.rs:729`; the parse-order rule applied at module scope by its caller, `:291-301`, and in a block by `nested_non_template_in_table`, `:504`); `kg_db::facts::innermost_declaration` (`kg-db/src/facts.rs:58`; parameters and variables, no parse order; `early_normalization.rs:434,770,793`); `navigate::binder`/`bound_in` (`navigate.rs:782,817`; the editor's, which `Binders` borrows) |
| D2.4 | **`static if` / `static for` decisions** | the loop: `evaluate_constexpr`, then `static_control_node`, at each node its traversal reaches (`kg-sem/src/infer.rs:1172-1178,1970,2360`; `kg_hir::rewrite::static_control_node`, `rewrite/mod.rs:547`) — `staticControl . evaluateConstexpr . inferExpType` (`Inference.hs:47`), typed predicates included. The planned per-module `StaticDecisions` query has nothing left to unify — **landed by deletion (W3.7, then 2026-10-10)** | the 2026-09-25 bug class: the census and the splice disagreed on a table-folded predicate, so the census's rule was literals-only | Gone: `facts::decided_static_ifs_within`, inference's live/dead lists, `kg_const`'s `induction_dependency` (`26700d6`); the census's `static_if_taken` and its literals-only compromise (`b56fd7b`); then the twin's static control, with no production caller since `81452c6` — `static_control`/`static_control_with`/`static_control_before_templates`, `infer_static`, `postpones_body`, `static_value`, `static_eval`'s `fold` and `for_twin` arms, `unroll`'s `for_twin` half and `const_index_width` (D1.7's `constIndexType` copy) — and walk.rs `extern_classes`, a fold of its own over the program that `trimmed_already` emptied before every walk since pass 7 moved onto the loop; and walk.rs `seq_items`' `static if` arm, which decided by a closed fold (`Walker::folded`) a `static if` the loop left standing, with the `removeNestedScope` it carried for the splice (`spliced`, `chain_names`) — none fired on the corpus (an instrumented `ptdiff`, 2422 programs), and a `static if` still standing is pass 16's error (`Frontend.hs:406`), which the walk now declines as it declines one anywhere else |
| D2.5 | **Template arguments** | `kg_ty::TArg` (`kg-ty/src/ty.rs:151`) in the instance note; IR → `TArg` is `rewrite/instantiate.rs:737 t_arg` (`tArg`, `Type.hs:431-437`), `TArg` → IR is `Mint::exp_from_type` (`kg-sem/src/from_type.rs:79`, `expFromType`); `instance Pretty TArg` (`Type.hs:217-222`) is `kg_ty::subtype::show_pretty_arg` — **landed 2026-10-10** | argument spellings the owner could not mint ("cannot mint") where another representation could | none. Gone: `ArgShape`/`arg_key`, `SubstKind`, `defaults::Bound` (`b56fd7b`); `known_targ`/`shape_of_targ`, `mint_type`/`mint_ty` (`81452c6`); then `InstanceSpec::known`, which only the deleted shapes filled (every caller passed `Vec::new()`, so its reads never answered), and `kg-sem/src/deduce.rs`'s own `instance Pretty TArg`. Not argument values, so not copies: the census's key (`census.rs:75`) and `resolve::Fingerprint` (`resolve.rs:194`), identity over the argument tree; `census::Minted` (`census.rs:50`), the values a kind evaluates to |
| D2.6 | **Conversion semantics** | pass 19 on the loop: `kg_sem::check`'s `check`/`check_prime`/`check_init_list` (`check.rs:1809,1833,1905`; `checkConversion`/`checkConversion'`, `TypeCheck.hs:152-178`) over `kg_ty::convert`'s helpers (`normalize_source`, `narrowing_warning`, `invalid_conversion_error`, `cast_fix`) | five ptdiff fixes this session were the walk re-deriving `checkConversion`; the next one should not have to | Gone: the CST checks, `passes/conversion.rs` (`7def769`) and `passes/validate_program.rs` (`1f4cb92`); then, 2026-10-10, `kg_ty::convert`'s `classify`/`check_conversion`/`Conversion`/`ConversionSite`/`InitElement`/`initializer_element_plan`/`literal_source_type` — the owner this row planned, which nothing ever called: pass 19 decides under guards of its own (`poisoned`, `certainly_unresolved`, the written-type skip), so it was a second `checkConversion'` beside the live one. **Standing, kept (the register):** walk.rs `conversions`/`literal_narrows`/`narrows` and the `Conversions { ints, warned }` sets (`walk.rs:136`), which spell two of pass 19's tree rewrites — an enum value converted to its literal (`TypeCheck.hs:155-156`), the narrowing `WarningF` wrap (`:101,176-177`) — that the loop decides but does not write into its tree |

## Not deduplicated, on purpose (the register)

A pair stays only with a reason and a comparing gate. Today's entries:

| Pair | Reason | Gate | Ends |
|---|---|---|---|
| ~~`template_type_check::Cx::instance` (CST) vs `instances::resolve_guard` (IR)~~ | the independent second reading that caught real drift (journal:1403) | `xtask ir` `differential` | **ended 2026-10-10 (sprint W3.7, M6)**: both sides deleted; the loop's census is the one, graded end to end by `ptdiff`, `backend`, `output` and `loop-left` |
| `kg_resolve::find_symbol` vs `kg_hir::sort::SymTab::find` | `SymTab` answers *during* lowering, in declaration order | `xtask ir` sorted trip / oracle diff | stays (it is the lowering's own) |
| `navigate.rs`'s CST scope walk | `ScopeData` has no source extents; IDE on broken buffers | IDE tests | stays past M6 (2026-10-10): it resolves names, which the loop does not do for a position; the types it reaches are the loop's |
| ~~`kg_ty::width::t_super` vs `subtype::t_super`~~ | transcribed from the same seven lines | unit test | **ended 2026-10-10 (D1.7)**: the width algebra calls `subtype::t_super_pretty` |
| pass 19's conversion verdicts (`kg_sem::check`) vs the walk's `conversions`/`literal_narrows`/`narrows` (`kg-ptbridge/src/walk.rs`) | `checkConversion` *rewrites* the tree the backend reads — an enum value into its literal, a narrowed operand into a `WarningF` (`TypeCheck.hs:101,155-156,176-177`) — and pass 19 on the loop reports its verdicts as diagnostics without writing them into the loop's tree, so the walk spells the two rewrites itself; making the walk read them needs pass 19 to record them per node (ir-elaboration M4's `checkConversion` IR rewrite) | `ptdiff`, `backend`, `output` | when pass 19 writes its rewrites into the loop's tree |
| `rewrite/abbreviated.rs is_auto_param` vs `kg-hir/src/facts.rs auto_params` | the rewrite's firing guard (`isAbbreviatedTemplate` plus a written name, the documented divergence that declines where the reference would crash) and its summary per node over `NodeData`, kept while the arena is built so `desugar_within` can skip a subtree holding no such function; a summary must imply the guard | `facts.rs`'s `a_pass_within_its_needs_agrees_with_the_whole_cata`; `infer-identity` | stays (the summary is the speed-up) |
| the loop's types vs `rewrite/instantiate.rs written_builtin` (D1.5) | `tArg` reads an argument's note (`Type.hs:431-437`); an indirect instance's argument the census mints from a substituted kind (`Mint::exp_from_type` under `newNote`) carries none, where the reference substitutes the written argument and its `TType` note (`resolveTemplateParam`, `TypeCheck.hs:334-339`). Fires twice on the corpus (`template.k`'s `Bar<uint6, 101>`; an instrumented `ptdiff`, 2026-10-10) | `ptdiff`, `output` (the copy's display, read off its note) | when the census mints its arguments typed, or substitutes the written node |
| `kg_hir::constexpr::written_type` vs `kg-sem/src/census.rs kind_type` (D1.5) | `kind_type` is `resolveTemplateParamType` (`TypeCheck.hs:349-362`): a parameter's kind with the template's arguments substituted, computed without building the substituted tree — parameters, widths, attributes and instances read through the census's binding; only its seven scalar arms coincide with `written_type`'s | `loop-left`, `ptdiff`, `backend` (what the census decides) | stays |
| the loop's types vs the walk's `set_node_type` fallbacks (`written_integer_type`, `local_type`, `operand_type`, the literal arms) and `deduced_type`'s folded literal (D1.5, D1.6) | insurance: a node the loop leaves untyped aborts the backend on a program the reference compiles. Each fallback was written against a CST-table gap that the loop has since closed; none fired over `ptdiff`'s 2422 programs (instrumented, 2026-10-10) | `ptdiff` (every `SetNodeType` against the recording), `backend` | when a count of their firings in the gates stands at zero and they go |
| the CST parse-time readings — `Templates` (`kg-db/src/passes/templates.rs`) with `visible_through_imports` (`db.rs`), `Binders`, `undefined_symbols.rs innermost_declaring_scope`, `kg_db::facts::innermost_declaration` — vs the loop's `Symbols` and `ModuleGraph::find_in_imports` (D2.1–D2.3) | they answer the parser's question, `findSymbol` at a position in parse order (`SymbolTable.hs:194-261`), for the checks the reference makes while parsing (each a `PARSE-ERROR`, so no pass runs after one); no post-parse structure holds the parser's table at a position. They do not agree with each other on the parse-order rule (a variable only in `Binders`; module scope and blocks in `innermost_declaring_scope`; none in `innermost_declaration`) | `conformance`, `errors` (against `kg-oracle --errors`), `passdiff`'s parse probes | when the sorted lowering records the parser's template verdicts (`kg-hir/src/lower.rs` already decides `typeSpecifier`'s shape) and the checks read them through `Parsed` — a D2 change of verdicts, not yet made (sprint W4) |
| walk.rs `template_functions`/`declared_copies` vs the loop's `Symbols` (D2.1) | leaf-name sets the walk declines by — a call of a template no deduction rewrote, a type named as a copy no round declared — never a verdict or a name; homonyms only widen a decline | `ptdiff`'s and `backend`'s declined counts (0, and 2 — the reference's own crash) | stays (a decline is silence) |
| `kg_db::SymbolMap` vs `kg_sem::symbols::Symbols` | `getSymbols` twice: over the modules' item trees, for the parse-time checks, pass 9's hints and the editor's navigation, which read the text; over the loop's rewritten IR for everything after the parse | `errors`, `conformance`; the IDE tests | stays |
| ~~base / twin / read-back type tables~~ | forced by the text round trip | `copy-types` (M2) | **ended**: the twin and the read-back with M4 (`81452c6`, 2026-09-26), the base `TypeTable` with the CST engine (`26700d6`, W3.7) |
| ~~CST `ConstEval` implementation~~ | IDE and migration | ir-elaboration shadow gates | **deleted 2026-10-10 (sprint W3.7, M6)**; `kg_const` keeps the binary table and `CastTarget`, which `kg_hir::constexpr` reads |

Any new pair is added here in the same commit that creates it, or it is a
review failure.

## Sequencing with the elaboration plan

```
D1.1–D1.8 (gate-neutral)  ──►  M0 foundations ──► M1 ──► M2 ──► M3 ──► M4 ──► M5 ──► M6
D2.1–D2.6 (ratcheted)     ──►  interleaved with M1–M3; D2.1 and D2.4 before M3,
                               because the program loop (M5) is built on them
```

D1 is mechanical — about fifteen commits, each byte-identical on every gate —
and pays for itself immediately: fewer places to fix each rule, and the
`xtask lint` ratchet.
D2.1 and D2.4 are the ones that change what the compiler decides. They should
land before the loop that depends on them, each with its own gate diff read
line by line.
