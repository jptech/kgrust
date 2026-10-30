# Editions: layering language changes on the frontend, opt-in

*Part of the [plans](README.md). **Plan, 2026-10-29; piloted
2026-10-30 on the `editions-pilot` branch** at the owner's request: steps
1–3 built, with the owner's decisions of that day — versioned editions,
`1.0` the reference; named arguments in any order, reordered — folded in
below and marked *as built*. Whether the pilot merges is still the
roadmap's open question. The user's pages: [`../editions/`](../editions/README.md).
Surveyed at `3562431`; every "today" below was read in the code then.*

The goal: new code and new users can opt into language changes; existing
code never notices. A program without the opt-in — every program the
reference's corpus holds, the standard library, anything `kanagawa`
compiles — checks, diagnoses and builds exactly as it does today, and
every gate in [`../gates.md`](../gates.md) is not merely green but
unchanged.

## 1. The rules

1. **Off is the reference.** With no opt-in, no new branch is taken: the
   parser, the lowering and every pass behave as before, byte for byte. The
   existing gates hold this already; nothing about them changes.
2. **An extension claims only programs the reference rejects.** Its syntax
   is a parse error, or its construct a rejection, in the reference today —
   proven per extension by `kg-oracle` (§6.2) — so turning it on cannot
   change the meaning of any program the reference accepts. Changing what
   existing syntax means (`-` no longer signed, say) is out of bounds; a
   new spelling for the new meaning is the way (`+%`, not a different `+`).
3. **Everything desugars to reference Kanagawa inside the frontend.** The
   ParseTree walk (`kg-ptbridge`) and the C++ backend see only constructs
   the reference itself produces, so the C ABI, the backend and `output`
   are untouched; each extension's result is checked against a
   hand-written reference twin (§6.3).
4. **The opt-in belongs to the source, per package.** A file's language is
   decided by the package that owns it, never by who imports it: the
   standard library is always reference Kanagawa, a dependency keeps the
   edition its own manifest names, and a build profile cannot change it.
5. **Unknown is an error, never ignored.** A `kg` that does not know an
   edition or a feature named in the manifest refuses it, as
   `deny_unknown_fields` already makes every manifest table do
   (`crates/kg-config/src/lib.rs:29-59`). An older tool must not compile a
   newer program as something else.
6. **`kanagawa` never opts in.** The compat binary reads no manifest
   (`crates/kg-cli/src/legacy/options.rs:599-635`) and gains no flag; it
   rejects extension syntax exactly as the reference does.
7. **Diagnostics point at what the user wrote**, and the house rules hold
   inside an edition too: fail toward silence, every rule cited — to the
   edition's own specification, since the reference has none to cite — and
   every new test proven load-bearing with `mutant.sh`.

## 2. What the user writes

Two levels, as Rust has them (its `cargo-features` and editions):

```toml
[package]
edition = "1.1"                  # default "1.0": the language at reference.lock's pin

[language]
features = ["named-arguments"]   # one extension each, opt-in, not yet frozen
```

*As built:* editions are versioned `MAJOR.MINOR` (the owner, 2026-10-30):
`1.0` is the reference and follows the pin, a minor edition only adds, and
only a major one may reject what an earlier edition accepts. `1.1` is a
preview until a release freezes it. Each edition's page lists everything
that differs from the one before ([`../editions/`](../editions/README.md)).

- **A feature** is the unit of change: one extension, specified, tested,
  and free to change between kgrust releases while it is a feature. A
  `kg` release lists the features it knows (`kg --list-features`).
- **An edition** is a frozen bundle of features, named once the first
  bundle exists (the name is the owner's: §8). Frozen means frozen: a
  later edition may differ, a released one never changes meaning.
- **`"reference"`** names what is there today, and follows the pin: when
  `reference.lock` moves, the reference edition moves with it, and §6.2's
  claims check says whether upstream now gives meaning to a syntax a
  feature claims (§7).
- **Without a manifest**, `kg check --feature named-arguments file.k` (and
  `--edition`) give the same to a single file; the language server reads
  manifests only (§8).

`[language]` is a new table rather than more of `[package]`, so that an
older `kg` refuses the whole table loudly (rule 5) and so that a profile —
`Profile` holds only `project`, `warnings`, `fmt` and `hw`
(`lib.rs:197-206`) — cannot reach it (rule 4).

## 3. Which language a file is in

Today there is one `Settings` per `Db` (`crates/kg-db/src/db.rs:84`), and
every file the program reaches — imports, the base library, the bundled
standard library — is parsed under it (`crates/kg-db/src/program.rs:117-174`).
Nothing maps a file to a manifest; only the language server keeps one
`Db` per manifest root (`crates/kg-lsp/src/global_state.rs:604-655`). Rule 4
needs that mapping, as a new piece of `Settings`:

- **The project's own files** — under the root manifest's directory and not
  under a nested `kanagawa.toml` — are in the root manifest's language.
  A nested manifest is another package, as the language server's source
  walk already treats it (`crates/kg-lsp/src/handlers/sources.rs:36-52`).
- **A file reached through an import directory outside the project** is in
  the language of the nearest manifest from the file up to, and including,
  that import directory; none means `"reference"`. *As built: the nearest
  manifest above the file, with no boundary at the import directory* — a
  boundary misses the common layout of a dependency's manifest beside the
  `src/` its users import, and by rule 2 a library read in a later edition
  than its own reads the same (`crates/kg-config/src/language.rs`).
- **The standard library is always `"reference"`**, bundled
  (`Settings::or_bundled_library`, `crates/kg-config/src/resolve.rs:163-170`)
  or named; no manifest sits above the submodule's (checked 2026-10-29).
  A unit test pins it.
- **The synthetic modules** (`.cmdargs`, `.options`; `ModuleSource`,
  `crates/kg-resolve/src/module.rs:39-47`) are `"reference"`.

`Settings` gains `language: LanguageMap` — the root's language and, per
import directory, the language its manifest names — resolved once in
`Settings::from_manifest` (`resolve.rs:126-146`) and `fallback`
(`resolve.rs:174-191`), with a per-directory cache for nested manifests.
`Settings` derives only `Debug, Clone` (`resolve.rs:15`); the map derives
`Eq` and `Hash` so it can key a memo.

## 4. Where each layer learns the language

The surveys of 2026-10-29 found no configuration in the parser and none in
the lowering: `kg_parser::parse_input(&Input)` (`crates/kg-parser/src/lib.rs:41-45`),
`kg_hir::lower_sorted` (`crates/kg-hir/src/lower.rs:70-77`) and
`kg_resolve::lower_file_named` (`crates/kg-resolve/src/lower.rs:333-338`)
take none. The one channel is `kg_syntax::ParseOptions { parse_docs }`
(`crates/kg-syntax/src/parse.rs:77-84`), which reaches the tree's error list
and not the parser.

### 4.1 The parse

- `ParseOptions` gains `features: Features` (a small bit set), carried into
  `Input` so that the grammar asks `p.has(Feature::NamedArguments)` at the
  one place an extension begins. With the set empty, no new branch is
  reachable (rule 1).
- **The `Parse` records the options it was made with.** `Db::parse_module`
  keeps an equal re-parse's old `Arc` (`db.rs:603-607`), and the item tree,
  the sorted lowering and the kept snapshots are keyed on that pointer
  (`db.rs:637-667`, `crates/kg-db/src/sorted.rs:87-146`,
  `crates/kg-db/src/snapshot.rs:160-178`). With the options inside the
  `Parse`, a file whose language changed is never "the same parse", and
  every memo downstream follows without a key of its own. The parse memo
  compares the file's options as it compares `parse_docs` today
  (`db.rs:580-588`), so an unrelated setting still invalidates nothing.
- **Every parser call outside the `Db` takes the file's options**, or an
  extension file is mis-parsed there: the language server's snapshot parse
  (`global_state.rs:533`) — whose errors are published early on the claim
  that they are a subset of the database's (`handlers/mod.rs:224-231`), a
  claim a default-options parse of an extension file breaks — `kg-fmt`
  (`crates/kg-fmt/src/lib.rs:107`, `:144`; `align.rs:80`; `reattach.rs:52`;
  `verify.rs:69`), `kg dump` (`crates/kg-cli/src/modern/dump.rs:47-72`,
  `:146`), `kg-board`'s interface reader (`crates/kg-board/src/interface.rs:398`)
  and rename's identifier check (`crates/kg-lsp/src/handlers/rename.rs:328`).
  Step 1 (§5) threads them all, before any feature exists to need it.

### 4.2 The tree

New syntax gets **new node kinds, never a reference node with a new
shape**. Keywords are contextual (`IDENT` matched by text, as `static
assert` and attribute names are: `crates/kg-parser/src/parser.rs:127-134`),
so the 53 reserved words (`crates/kg-parser/src/syntax_kind.rs:428`) do not
grow and no identifier in existing code is broken. Operators are glued
token pairs, as `+=` is (`crates/kg-parser/src/grammar/stmts.rs:531-559`).
The ungrammar, the generated AST and the TextMate grammar follow from
`cargo xtask codegen`, and the parser's `node_kind_coverage` test
(`crates/kg-parser/src/grammar/tests.rs:1031-1104`) learns each kind.

A new kind is a hazard where it sits *inside* a reference node: the
generated accessors are cast filters (`ArgList::exprs()`,
`crates/kg-syntax/src/ast/generated.rs:3024-3026`), so a consumer not yet
taught skips the new child silently. The rule: **shape the node so that an
untaught consumer sees the reference twin** — a named argument's label is a
sibling of its expression, not its parent (§5, step 3), and `exprs()`
still yields every argument.

### 4.3 The desugaring

Two layers, chosen per extension by what it needs to know:

- **The lowering** (`crates/kg-hir/src/lower.rs`) for a rewrite that is
  local to the text: where the reference's own parser-time desugarings live
  — the ternary, `op=`, `switch` (`lower.rs:1301-1357`, `:1469-1559`,
  `:1596-1612`) — with provenance for free: the replacement is `Own` at the
  extension's range, what it synthesizes `Donated` from it, an operand used
  twice `Reread` as `x op= e`'s is (`lower.rs:1359-1391`). Wrapping and
  saturating operators belong here; so does a tagged enum's declaration,
  mirrored in `kg_resolve::lower_file` so that importers see it — the item
  tree is the cross-module interface.
- **An early HIR rewrite** (`crates/kg-hir/src/rewrite/`, driven from
  `kg-sem`'s loop entry, `crates/kg-sem/src/elaborate.rs:301-310`) for one
  that needs declarations or types, on the pattern of `closure_calls`
  (`crates/kg-sem/src/closures.rs:26-125`): a `match`'s exhaustiveness, a
  named argument reordered. The lowering's symbol table records a function
  as `SymShape::Function` with no parameters
  (`crates/kg-hir/src/sort.rs:65-67`), so nothing that needs a callee's
  parameters can be done earlier.

Either way the walk sees nothing new: an interim IR form an extension
needs is gone before it, as `Exp::Path` and `Exp::Unsupported` are today
(`crates/kg-hir/src/exp.rs:10-23`). A synthesized name carries a character
no identifier can (`$`, as `$quasiN` and `name$capture` do;
`crates/kg-parser/src/lexer.rs:195-204`) with a prefix of kgrust's own —
except a declaration other code must name (a tagged enum's parts), which
gets a documented, reference-spellable name so that a reference-edition
module importing it can use it. Every rewrite ships donor unit tests read
off its twin: `xtask ir`'s provenance clauses see `Own` only, so no gate
notices a swapped donor ([`../gates.md`](../gates.md#src-donors-need-a-unit-test)).

### 4.4 The tools

- **`kg fmt`** formats the new kinds (and keeps a glued operator together:
  `BIN_EXPR` is spaced today, `crates/kg-fmt/src/lower.rs:451-457`, and
  would print `a + % b`); its verifier reparses with the file's options.
- **The language server**: semantic tokens classify a contextual keyword
  by its parent node (`crates/kg-lsp/src/handlers/semantic_tokens.rs:150-250`);
  a `Donated` node answers no hover (`crates/kg-hir/src/position.rs:14-30`),
  so features on extension syntax read the CST, as navigation does.
- **`kg explain`** gets prose for every new diagnostic, under a `kg::`
  name of its own; JSON diagnostics carry it.
- **`kg expand`** (later): the program as the reference would be given it.
  Not first: the sorted printer prints the file's text at a name's head
  (`crates/kg-hir/src/pretty.rs:18-33`), which for a name donated from
  extension syntax is extension text, so the printer needs work first. The
  twin check (§6.3) does not wait for it.

## 5. The steps

Each lands on a branch of its own, gate-neutral except where it says.

**Step 0 — the decision.** The owner decides the roadmap's open question.
Until then this plan is all there is.

**Step 1 — the plumbing, read by nothing.** `kg-config`: the `[language]`
table, `edition` read and validated, the `LanguageMap` (§3), the
`--edition` and `--feature` flags in `crates/kg-cli/src/modern/settings.rs`;
`Features` in `ParseOptions`, recorded in the `Parse` (§4.1); the parse
memo; every outside call site of §4.1. `features` is a closed list with
nothing in it, so the manifest refuses every name. Evidence: every gate
unchanged and `cargo xtask bench` neutral (interleaved A/B); unit tests,
each mutant-proven, that the standard library is `"reference"` under a
project that names a feature, that a nested manifest is its own package,
that an unknown edition or feature is refused, and that changing a file's
features re-parses it and nothing else (a warm `Db` against a fresh one,
as `edit-identity` grades).

**Step 2 — the `editions` gate, empty.** `xtask editions` and its fixtures
(`tests/editions/<feature>/`), §6, with no feature to grade; in
`scripts/fast.sh` and `gates.sh`. A new gate, not an edit to
`conformance.rs` or `corpus.rs` (the scoreboard).

**Step 3 — the pilot: named arguments, in declaration order.** *As built
(2026-10-30), the owner asked for more: labels in any order, reordered —
"if we support named arguments, then we should support reordering them at
call sites correctly." So the "later, its own step" below came first: an
`Exp::Labelled` from the lowering, placed by `crates/kg-sem/src/named.rs`
inside the loop from the callee's inferred type (labels matched by name in
deduction, `crate::deduce::by_parameter`), with the reference's evaluation
order — left to right in the list, `compiler/cpp/ir.cpp:10912-10919` —
kept by refusing any reorder two arguments could tell apart
([`../editions/1.1.md`](../editions/1.1.md)). The plan as first written
follows.*
`f(x: 1, y: 2)` where `f` declares `x` then `y`; the labels are checked,
the order must be the declaration's, and every argument after a labelled
one is labelled. It is the smallest extension that exercises every layer
— the option in the parser, a new node kind, the formatter, the editor, a
diagnostic that needs resolution — while its desugaring is nothing at all:

- *Syntax*: a new `ARG_LABEL` node (`Name ':'`), a **sibling** before its
  expression in `ARG_LIST` (`crates/kg-parser/src/grammar/exprs.rs:293-324`).
  `f(x: 1)` is a parse error in the reference and in `kg` today, at the
  `:` (`kg-oracle --errors` and `kg check`, 2026-10-29).
- *Desugaring*: none. `exprs()` yields the arguments as before, so the
  lowering (`lower.rs:1965-1972`) and every other consumer produce the
  twin's IR unchanged; §6.3 then holds by construction, and is checked.
- *The check*: a pass over the CST reading the loop's callee, as
  navigation does (`Db::definitions_at`): each label names the parameter
  in its position. Where the callee has no declared parameters to check
  against — a closure, a function-typed value — the labels are an error,
  by the feature's specification and not as a guess: the rule is chosen so
  that the check is exact. Where the loop reaches several declarations
  (instances of one template share the declaration's names; different
  declarations may not), the labels must agree with every one.
- *Tools*: `kg fmt` (`x: 1`, no space before the colon); signature help
  highlights the labelled parameter; a label's hover and go-to-definition
  reach the parameter. (The editor's inlay hints are widths alone,
  `crates/kg-lsp/src/handlers/inlay_hints.rs:263`; parameter-name hints,
  should they come, would skip a labelled argument.)
- *Later, its own step*: labels in any order, reordered in an early HIR
  rewrite, once the reference's argument evaluation order is read and
  cited — a reorder that changes which call's side effect happens first is
  a different program, so the rewrite binds the arguments in written order
  first where they are not pure. Named template arguments
  (`memory<T, N, Banks: 4>`, skipping defaults: the programming guide's
  "Template parameter defaults") are a separate feature after it.

**Step 4 — the next feature**, chosen with the pilot's lessons: wrapping
and saturating operators (lowering-local) or tagged enums and `match`
(lowering and an HIR rewrite, with a declaration other modules import).

**Step 5 — the first edition**, once some features have been used long
enough to freeze: the bundle named, `kg fix --edition` to migrate a
package (lints' machine-applicable fixes, the same machinery), and the
user's guide to the language beyond the reference.

## 6. How it is graded

The existing gates grade rule 1 already and change in nothing. `xtask
editions` grades the rest, as pass/fail — exactness is not a ratchet:

1. **Neutral when on.** Every corpus program the reference accepts,
   checked with every known feature on: the same verdicts and diagnostics
   as with none (cheap, in `fast.sh`), and the same walk call for call
   through the trace backend (in `fast.sh walk`). This is rule 2,
   measured: a feature that changes an accepted program fails here.
2. **The claims.** Each feature lists minimal programs of its syntax
   (`claims/*.k`), and the gate asserts `kg-oracle --errors` rejects every
   one — the tripwire for the day upstream gives a claimed syntax a meaning
   of its own (§7).
3. **The twins.** Each accepted fixture `x.k` comes with `x.ref.k`, the
   same program written in reference Kanagawa by hand: `kg-oracle` accepts
   the twin, and the two walks agree call for call, source locations
   aside — so the backend is given exactly what the reference would have
   given it.
4. **The rejections.** Fixtures that must fail carry their expected
   diagnostics inline, in the corpus's `expected:` style, read by the
   gate's own harness.

A feature's fixtures are also its specification: the user's guide quotes
them, and a change to one is a change to the language.

## 7. Risks

- **Upstream moves into the same space.** If upstream Kanagawa later gives
  a syntax a feature claims a meaning, or adds the same feature
  differently, §6.2 fails at the pin move. The feature keeps its meaning in
  the editions that froze it; the reference edition takes upstream's; a
  later edition may adopt upstream's spelling, with `kg fix` to migrate.
  Small features, spelled where upstream is unlikely to go, keep the
  exposure small. Moving the pin's checklist ([`../../AGENTS.md`](../../AGENTS.md))
  gains `xtask editions`.
- **A second language to maintain.** Every feature is a parser branch, a
  node kind, a formatter rule, an editor rule and a gate's fixtures,
  forever. The bar for a feature: it removes a class of bug or a real
  amount of code, and it cannot be a library function instead.
- **The editor's latency.** The lookup is a per-file field and the parse a
  branch never taken in reference files; step 1 measures it before
  anything is built on it. A feature's pass is benchmarked as any new pass
  is ([`performance.md`](performance.md)).
- **Interfaces across editions.** A reference-edition module importing a
  feature's declarations sees their desugared form (§4.3); a feature whose
  public surface cannot be spelled in reference Kanagawa is not admitted.

## 8. Open questions for the owner

| Question | Assumed meanwhile |
|---|---|
| Does kgrust extend the language at all (the roadmap's) | piloted on a branch (2026-10-30), unmerged |
| The first edition's name and the naming scheme | **decided** 2026-10-30: versioned, `1.0` the reference, `1.1` named arguments |
| A file-level opt-in for single files without a manifest (a header comment), beyond `--feature` | flags only, applied under the first input's directory; the language server reads manifests only (the owner left single files to the implementer, 2026-10-30) |
| Whether an edition may also carry *stricter* checks — diagnosing in the frontend what the reference leaves to the backend (the 241 assertions the compat path must not catch early: [`../architecture.md`](../architecture.md), "The scoreboard precedes the work it scores") | allowed, in a major edition only (the owner: "an edition can be stricter, but TBD how much we want to push for that"); none planned |
| Where the user's guide to the language beyond the reference lives | beside each feature's fixtures, gathered into one page at step 5 |
