# Outstanding gaps, by mechanism

> **Superseded 2026-09-27** by [`plans/sprint-parity.md`](sprint-parity.md)
> §4, which partitions the gaps the 2026-09-27 gate run lists. Kept as the
> record of how each row closed; the counts below are history.

*2026-09-10, after queue item 15. A working list: each row is struck through
here as it lands, with the commit, and the counts are re-measured from
`cargo xtask errors --verbose` rather than edited.*

Counts are must-reject snippets the reference rejects and this frontend
compiles cleanly. The numbers here come from bucketing `cargo xtask errors
--verbose`'s `SILENT` lines by message family and reading a sample of each
family's snippets; they are approximate to a few either way, and the strict
gate is the number that must fall.

**The per-row counts below are 2026-09-10's and are read as history, not as a
census.** The two headline numbers are re-measured here, because a total is
the first thing in a partition to rot:

| | 2026-09-10 | 2026-09-20 | 2026-09-22 | 2026-09-23 | 2026-09-24 |
|---|---|---|---|---|---|
| `errors` silent (we compile a must-reject snippet cleanly) | 256 of 834 | 56 | 44 of 742 graded | 81 of 834 — every snippet graded since §C closed (84 at first grading) | **45 of 834** |
| `errors` disagreements | | | 24 | 28 (4 first gradings) | **28** |
| `conformance --strict` | 309 | 102 | 90 | 87 (evening) | **51** |

## A. Frontend diagnostics still silent

| # | Category | ≈ | What is missing | What resolves it | Size |
|---|---|---|---|---|---|
| 1 | **Instances mentioned inside a copy, and cross-module copies the route declines** | ~72 — the second round over a module's own twin, the alias route for a type declared in the importing module, an instance in *prefix* position (`A<uint32>::A1_t`, a census candidate on its own range in a type position) and a written array type travelling to a foreign twin (`A<uint32[2]>`, 3) landed 2026-09-10; the seam (a foreign instance's verdicts in the importing module: twin roots indexed, aliases retried, the minted argument qualified, 14) on 2026-09-11; a member template's instance landing inside its class (in-file, `C::B_t<2>` and `Foo<bool>::A_t<2>`) on 2026-09-12; the cross-module second round (phase 2 iterating until no twin decides a foreign instance: the sorter's door to finished twins, every round's names, one alias per instance across modules; member alias templates' arity, nested copies' members — every `template-instance-identifiers.k` shape) on 2026-09-13, and later that day a round opened by a postponed candidate or a depth-two decision of the module's own text, an import alias or namespace before an instance, a written class qualifier surviving a copy, a nested class's member template in the CST index — the `Expected a function of type` family, ~15; on 2026-09-14 a member template named through a receiver (`c.Foo<bool>(x)`), a function named as an argument reaching the copy, pass 8's rejections dropped before `instantiate` and the home sink seeded with carried verdicts (5); and later that day a deduced call on a receiver (`o.Fn_T_10(x)`, minted through the class, handed to its module; 4); then a partly explicit deduced call and a deduced member call on an instance receiver (4); an alias *to* an instance (`using Bbool = B<bool>;`, 2) on 2026-09-16; a copy's argument mangled as what its alias stands for, so two modules name one instance alike, with nested and qualified mangles read back (3: `local-structs.k::9`, `function-template-param.k::30,34`), and a copy as a function-typed argument — shaped a round later, its typename bound to a named type, spelled by the owner under its namespace (`local-structs.k::10`), a function-local one by its hoisted name (`local-functions.k::5,6`) — on 2026-09-17; left: a designated initializer's narrowing through one (1), a nested foreign instance argument's conversion message (1), a non-spellable type leaf's original as pretty text rather than a mangle; on **2026-09-20 later** six more, each of which had cost its file rather than its copy: an instance in *qualifier* position taking its template's defaults (`C<uint32>::addr`, +22 backend agreements), a template-template parameter's kind mentioning a bound parameter and then a folded one, a template name two modules declare resolving through the mentioning module's imports (`IrTemplates::get_from`), the constants a file scope declined for want of a width (`Db::late_constants`), and the ECC memory of arrays — a type the language cannot spell, which needed divergence #31 *and* the array type nesting to be a gain rather than three rejected programs | `while compiling` (17), most conversions in template-heavy files, template-argument checks on instance types, "Unresolved argument", "template arguments missing" | A second elaboration round over the rewritten IR: census over the twin, member-template candidates, an IR-level fold for arguments like `1 + bitsizeof T`, foreign argument minting, passes 9, 11 and 12 inside copies. Roadmap item 14's last piece and the largest single payoff. | Large, architectural |
| 2 | **Closure typing** | 17 → a lambda typed through its twin — landed 2026-09-17 (`lambdas.k::4`); a `decltype` with no written form, the capture list noted after its `[`, minted arguments exempt from the parser's rule — 2026-09-08 (`lambdas.k::6`); a captured lambda is its closure — 2026-09-08 (`lambdas.k::22`); **Stage B, first half** — `higherOrderFunctions` as a rewrite (`rewrite::higher_order`), `addLambdaWrappers` for a function named in the module's own text (`elaborate::wrappers`), a name-bound `Do` deduced and checked at the binding, the wrapper's closure synthesized for arguments no wrapper reaches — 2026-09-08 (`closures.k::1,9,15`) | Wrappers inside copies (a function argument in a template body, `higher-order-functions.k::116`) and of an instance the round before resolved (`Go(is_valid<uint32>, 10)`, `return-type-polymorphism.k::16`); the `Closure` copies materialised in `data.closure.core` (the top `ptdiff` row, 342 then, **167** now — `tuple2` 25, `Foo` 11, `optional` 10, `pipelined` 8). **Diagnosed 2026-09-20, in two halves, neither of them the pass budget** (six and eight program passes are byte-identical to four): (i) a member alias of a copy has no type at the *mention* inside a template argument list, though its declaration is typed in the map its own module hands on — `atomically<__kg_m20::pointer_t>` declines on the written argument, before any deduction; (ii) `deduceType`'s only usable clause for a template-instance formal takes a `TInstance` actual (`Type.hs:1532-1533`), so the capture is deducible only once the actual `Closure<…>` mention has become an instance type — and the capture alias `moveFunctions` leaves in the body is printed by its leaf, a name nothing at module scope declares. Two nine-line library-free reductions and the way out of each are in the journal, 2026-09-20 "What the closure deduction is actually waiting for" and "The alias that stayed behind"; ~~`closureCalls`~~ (landed 2026-09-19, `elaborate::closure_calls`); the symbolic `S<T>` spelling for an expected function type (`::132,133`); ~~a member function or function-typed member as an argument~~ (landed 2026-09-19, wrapped capturing `this`, `this-capture.k::9`) — except a class template's, which the reference wraps before the lambda leaves the copy (`this-capture.k::18`); a member access on a reference as an argument; attributed function types (`::189`); `bind2nd`/`compose` closures in the library's own twins — **on 2026-09-23** the copy that returns a capturing lambda types in a library module's twin (a closure argument spelled as its `Closure<…>` copy, a copy found in the one module that declares it, the hoisted capture alias told apart under the module's namespace: `data.function`'s `compose2` copy, 114 programs → 38 declines) | `closureCalls` as a round-two rewrite over the twin's types; wrappers and lambda desugaring re-run over copies; symbolic instance types. | Medium |
| 3 | **Value template arguments' type check** | ~4 — the dependent `int<N>` width (2) and pass 12's unresolved-argument arm (7) landed 2026-09-10, a value parameter's written type read off its module (7: an alias, an imported enum, a struct under `T x`) on 2026-09-11, a constant given to a typename parameter as the error type its mentions resolve to (`type-check.k::19`) and deduction from a copy-typed argument through its mangled leaf (`template-argument-deduction-from-instance.k::6,8`) on 2026-09-17; and a member template's list under its class instance's bindings (1) the same day; left: `cast<int32[2]>(1000)`, `T X` under `T = T1<int32>` (the instance argument's type), `Id<T> N` (an instance-typed kind under `T = bool`), and a member call on a foreign instance receiver typed `Unresolved` in the importing module (A1's cross-module read-back) | `NFoo<0, 10>` against an `int0` parameter, `T3<10 != 2>` against `uint5` | `validateArg`'s value clause over the fold result in the template type check. Some are inside copies and fall under A1. | Small |
| 4 | **Parse-time symbol table** | ~4 — on 2026-09-22 a module path became one lexeme (`imports.k::117-119`) and `Foo<…>` a template only where the parser already knows one: a later declaration, a plain function, a template or receiver inside a block, a receiver typed by a `typename` parameter (`function-template.k::8,17`, `dependent-template.k::2,4,7,11,13`); `a.template b<…>` on a receiver with no class or a class without that member template landed 2026-09-15 (2: `dependent-template.k::1,5`) — template-parameter kinds, variable attributes and a bare imported template name landed 2026-09-10 (17 snippets); left: `template` keyword on a dependent member, a template declared under `static if`, `Foo<…>` on a function, `import a1. a1` | The reference's parser rejects using its mid-parse table: `Foo<3> x;` on a non-template, a template parameter used before it is declared, a template name redeclared as a class, `T::template X<…>` shapes | One post-parse rule per shape, the way the template-arguments-missing check was done. Each is a one-off and needs an oracle probe for position. | Small each, ~8 shapes |
| 5 | **Name resolution and scoping** | 21 — `findSymbol`'s two rules (13), the qualified member probe (5), the export list, function-type parameters and the struct member (5), pass 9 at the use site's own scope (6: the item tree's scopes carry their range and the offsets local blocks closed at) landed 2026-09-10, and a template's parameter list judged in order with a function's scope spanning its body (4) on 2026-09-11; pass 9 over the twin — the copies' names, the `static if` holder rule, the printer's dropped qualifiers and the seam left to the base run — on 2026-09-17 (3: `static-if.k::42,61`, `enum.k::58`, plus `scratchpad/sc/leak1.k`'s cross-class `static if` name); a member the class lacks, on a class of this module or an import (`basics.k::184`), a written `this` as a call receiver, and the parser's "`Foo`: undefined template" for a member template it cannot find with pass 12's "Undefined template" on an array receiver (`member-function-template.k::20,22`), on 2026-09-17; left: a nested function's body after `moveFunctions` hoists it (`local-functions.k::1`) | Pass 9 judges "declared anywhere in the file", so out-of-scope names pass; a bogus second segment on a resolved class or enum; `symbol is ambiguous` (7) and import-qualifier conflicts (6); errors inside imported files; duplicate parameter names in a function type; member on a struct that lacks it | Scope-accurate pass 9 through kg-resolve's lookup walk (the main risk is false positives; the resolver already exists); member lookup on the second path segment; `findSymbol`'s two conflict rules; surfacing an imported file's own errors. | Medium |
| 6 | **Const folding and inference clauses** | 18 — a decided top-level `static if` (dead branch gone from the declaration map, the inference and pass 9's hedge; the live branch spliced) and `staticControl` over the copies (a literal predicate, a comparison of two literals) landed 2026-09-15 (5: `static-if.k::3,26,31,33,72`) and `::42,61` on 2026-09-17 (pass 9 over the twin, row 5) — a `static assert` over a runtime value, a division by a non-positive literal, a mux with no common supertype, a union initializer's excess element, the imports' constants in the fold environment and `decltype` of a variable or a call landed 2026-09-10 (14); a designator to a missing field on 2026-09-11 (3) | Mux with an initializer-list operand ("Could not deduce variable type", 7), division by a zero or negative literal as an error type, string equality folds, `decltype` of a call, union initializer conversions, `static if` predicates and array dimensions inside `static for`; a `static for` over a runtime value and a first `return` under an undecided `static if` landed 2026-09-17 (`auto-placeholder.k::7`) | A handful of folder and inference clauses; the `static for` and nested-template cases wait on A1. | Small each |
| 7 | ~~**Interpolated strings**~~ | 0 — landed 2026-09-10 off the token text: a whole-program name → type resolver for pass 18, hole names judged by pass 9, the format grammar's two parse errors | ~~The lexer still keeps a whole `"…"` as one token (divergence #15)~~ — closed 2026-09-22: a hole is an expression node in the CST and a child in the IR, pass 9 judges it as any expression, and the backend walk walks every hole shape but an echo over a non-name (100 declines → 0) | Pass 18 onto the nodes' types; the echo's `showPretty` | Small |
| 8 | **Singles** | 2 — the const receiver and two type-as-expression shapes landed 2026-09-10 | A const lvalue shape, three "type as expression" shapes, a global closure via `bind2nd` | Case by case; the `bind2nd` one is A2. | Small |

## B. Reported, but at a different place or with different words (26)

- **18 parse-recovery positions.** Our recovery reports at the construct,
  megaparsec at the offending token. Closing these means mimicking
  megaparsec's failure point; booked as a design decision, not a gap.
- **5 both-differ, 2 message, 1 position.** A device library's `umul27`, a
  file the reference cannot decode, `decltype` of a call on a non-function, a
  hint list, "Undefined template" versus "not in scope". One-offs.

## C. ~~Snippets the oracle cannot grade (92)~~

Closed 2026-09-23 (`82f7270`): the oracle driver takes `--define`,
`--using`, `--target-device`, the five `.options` fields and `--Werror` with
the warning switches, so all 834 must-reject snippets are graded. The 40
newly visible silent snippets are mostly `constexpr.k` (define-driven
folds), `callbacks.k` (`--Wconversion --Werror`) and `memory.k`
(`--target-device`); the four new disagreements are three "Unresolved
argument for the parameter N of template T1" where we report the fold's own
error, and one `int33` where we say `uint32`.

## D. Undecidable by design until other work lands (320 assertions)

- **241 backend codes.** Need the ParseTree C ABI bridge, queue item 6.
  Independent of everything above; the frontend must keep not diagnosing
  earlier than the reference on these.
- **79 warning assertions.** Booked to "the semantic passes"; shadow,
  conversion and initializer-override warnings exist now, so some fraction may
  already be decidable and is worth a survey.

## G. False alarms the differential fuzzer found

`cargo xtask fuzz --differential` mutates the snippets and grades each
mutant against the oracle; each "we reject, the reference accepts" bucket
is re-checked with the full reference compiler before it counts (a bucket
whose program the reference's *backend* rejects is booked as an earlier
rejection, not a false alarm). Nearly all are one fact: a declaration
enters the parser's table only once parsed whole (`parseSymbol`), and a
name is bound to what `findSymbol` finds at that point.

Re-run 2026-09-24 (`--seed 24301 --iterations 6000 --reduce 600`, after
the probe server was given a memory budget — it had grown to 10 GB on one
mutant): 9 false-alarm buckets. Fixed that day: a local variable shadowing
a template in its own initializer (`Binders`, 4 mutants) and an
interpolated hole naming its own enclosing function (1). Booked, the full
reference rejecting later: `Unresolved argument` over `bitsizeof` of a
function (4, reference Error 24), a duplicate class (1, Error 3), `Go` as
an undefined template (1, Error 24). The one hang — a `static for` of 65535
iterations with a lambda each, which the reference's own frontend does not
finish either — ran `kg check` past 10 GB; the unroll bound
(`static_for::UNROLL_NODES`) leaves such a loop standing, and
`moveFunctions`' selected names are a set now (the list made it quadratic).

| Shape | Ours | Where it is |
|---|---|---|
| `inline void GoVar() { GoVar([]{}); }` beside an imported `GoVar` (18 mutants; the `DoIt` shape) | "Invalid number of arguments, expected 0" | `addLambdaWrappers` (`wrappers.rs`): the parser bound the mention to the import, so the reference's wrapper *calls the import*. Naming it is `item_of` passing over an own item `types.rs`'s `Registered` rule says is not yet in the table (tried 2026-09-23, fixes the name); the call is the hard half — the wrapper's body is printed into the twin as the bare name, which from file scope finds the own function, and no twin can spell a module-qualified callee. Needs the `alias_qualified` route `ArgShape::Function` takes for template arguments, wired to a callee and read back by the walk. |
| a function named like its own parameter, through the wrapper; a hoisted local function whose parameter type is a local struct shadowed by a later parameter | "Undefined function", "Expected a type" | from the 2026-09-23 run; not reproduced by this seed's run (the imports buckets there need their assertion's own `-I` options to judge) |

### ~~`make_capture0` in a module that imports no `data.closure`~~

Landed 2026-09-23 (late), in three pieces: the twin tries a bare name the
imports do not answer under the namespaces the module's tree qualifies
with (`Imports::with_hidden_namespaces`); a *qualified* alias names its
path in a closure's function name (`Imported::qualified_alias`), without
which the closures the first piece typed met themselves as two
(`atomically(constant(true))`, 11 false alarms); and the walk declines a
call through a parameter or local no `closureCalls` rewrote, which the
newly typed `async_then_void` copies exposed. Spelling the namespace in
the twin's text through an alias was tried first and cost 67 agreements.

Left behind it, and closed the same evening: `closureCalls` did not reach
a local class's members in a copy — the class is hoisted to module scope
in the twin, and its `task_t` names its function's local; the scope walk
now looks in the function a hoisted declaration came out of before the
module (+9 agreements). What the walk still declines, each a gap of its
own:

- **"call of a variable"** — a callback parameter whose function type is
  written through an alias (`using Callback = (uint32) -> uint32`,
  `higher-order-functions.k::22`), which `resolvedFuncParams` sees
  through (`Function.hs:134-143`) and our higher-order rewrite does not;
  `test/logic/lambda_test_cases.k:340`.
- ~~**"call of an undeclared function"** for a generic function's
  `$wrapper$` called through its closure from another module's copy~~ —
  closed 2026-09-23: a round that rewrote a closure call asks for the next
  (`lambdas.k::2,15,33`), and each program pass starts from the last
  pass's twins, so `sync.atomic`'s copy deduces against the wrapper the
  calling module declares (+14). Three programs still decline there, for
  reasons not yet reduced: `quad_port_tests.k`'s
  `BypassTest@Test@?lambda0` called from `control.async`'s copy without its
  namespace, and `data/array.k:843`'s `f(x[2*i], x[2*i + 1])` in an
  unrolled `static for` of `reduce`'s copy (`data/bits.k`,
  `data/representation.k`).

### ~~A function named as a default mangles bare~~

Closed 2026-09-24 (journal, "Roadmap 5"): minted as the pool's alias for
the bare name, which serves the deciding module's typing and the owner's
mangle alike.

### ~~A written local type in another module's copy~~

Closed 2026-09-25 (journal, "The mark on a written leaf"): the route puts
the written scope on the outer type identifier only, and the walk honours a
minted leaf's scope exactly where the two levels differ.

### A lambda in an unrolled `static for`

`Go([](uint<i+2> x) { return x; }, 0)` inside `static for (const auto i :
2)`: the loop is unrolled (its index reaches a constant position), the
lambda replicated per iteration (`?lambda0$i0`, `?lambda0$i1`,
`Constexpr.hs:110-119`) and hoisted, and the reference resolves its
parameter type's `i` through the qualified name `fixScopedName` gave it —
the iteration's `const i`. Our twin prints the hoisted function with a bare
`i` at module scope, and the iterations as anonymous blocks, so on reparse
the `i` each lambda needs is keyed under `$quasiN` scopes nothing ties to
`$i0`: no closure is typed and no copy made (`static-for.k::11-18`, eight
programs, first divergence at walk step 8). Needs the twin to spell the
qualified chain (the pool aliasing it, as for a capture alias —
journal 2026-09-20, "The alias that stayed behind"). Eight programs.

### An alias template's instance, decided by the base census

Landed 2026-09-25 for everything a twin types (journal, "An alias copy is the
instance it is"): an alias copy is `TInstance` of what it names, and a copy
over one is displayed through it. Left: an argument the *base* census decides
— `optional<index_t<32>>` written in a library module (13 programs, the
`ParseStruct` row), `A2_t<uint7, 10, index_t<10>>` through a default,
`array<MyInt<12>, 10>` (`using.k::12,13,26,27`) — is typed by the base
table, which resolves an alias template's instance with no copy and no note.
The reference decides such an outer instance a round later, once the inner
copy exists (`infer (TypeIdentifierF _ TemplateInstance{}) = TUnresolved`,
`Type.hs:1274-1275`); the census would build the note from the inner
instance's own decision. Tried 2026-09-25 and reverted — it fixed nothing:
in these programs the argument reaches the census as a twin's alias leaf
or a donated node, never as a written `index_t<4>`; and in
`test/library/control/fsm.k` the copy is first decided, plain, inside a
deduced `just<uint2>` copy a round before the written `optional<index_t<4>>
override` wins it in the reference — a matter of rounds as much as notes.

### A copy's name from its owner's shapes

A foreign instance is minted by its owner from the census's
`ArgShape`s, and a function type's attributes travel as a `TAttrSet` —
sorted. The reference keys and names the copy by the mention's written
tree, attributes in written order, so `same_types<[[max_threads(2),
thread_rate(1)]] (uint3) -> void, [[thread_rate(1), max_threads(2)]]
(uint3) -> void>`'s copy and mention are two names here
(`type-constexpr.k::17`; the walk declines it as an undeclared copy since
2026-09-23). The fix is an ordered attribute list in the shape, or the
mention's own mangling handed to the owner. Small, one snippet.

### A call's template-template default inside a copy

`fill_defaults` qualifies a defaulted template-template argument by the
module that declares the name, through `Templates::visible_through_import`
(journal 2026-09-25, "A default's name, a homonym's defaults, a class
constant's value"). The elaboration's base index carries that map; the
twin rounds' indexes (`elaborate.rs`, built over synthetic file ids) do not,
so a call inside a copy still spells an imported default under its writer
(`@@data@bitarray@memory` for `data.memory`'s `memory`), a name nothing
declares — a missing copy, never a wrong one. The fix is the visibility map
re-keyed through each module's synthetic id. No corpus program is known to
reach it.

## H. Output parity (`cargo xtask output`)

The owner's bar (2026-09-23): every file `kanagawa --backend=sv` writes is
identical to the reference's modulo source locations; `exact` (byte for
byte) is a second ratchet. First reading: 153 of the 274 programs the
reference writes RTL for are equal, 23 exactly; 166 on 2026-09-24. Open:

| Shape | ≈ | Where it is |
|---|---|---|
| An assertion's call-trace frame names its function mangled (`pipelined_for<\|$uint@10@@…\|><uint10>`) where the reference names it for display (`pipelined_for<uint10, data.closure::make_capture2<…>::$::Capture, …>`) — a copy of a copy; and a function-typed parameter printed through its alias there (`type.stdtype::index_t<…>`), resolved here (`uint3`) | 19 | The name the walk gives `ParseFunction`; `ptdiff`'s `ParseFunction / ParseFunction` row is the same fact |
| ~~Static instances numbered differently~~ (`__static_instance_7__…` against `_0`) | 12 → 7 | **Closed for the rounds 2026-09-24** (journal, "Roadmap 3"): a copy's frame records its decided round, and a twin's decision of a mention the base census held is dated by the twin's round (`elaborate::twin_round`) — `test_cases_17/23/31/35/38` agree. Left: `multi_word.k`, whose `tuple2` copies split across two reference rounds for another reason, and `atomic_do`, `context_saver_depth`, `nested_atomic`, `test_cases_7`, whose numbering now agrees and which differ later, in an assertion. |
| Declined by the walk | 79 | The backend gate's decline rows; mostly the closure and template-instance categories |
| Locations: comments agree on 56589 of 149812 lines | — | Divergence #25; ratcheted by `exact`. A minted node now stands at its donor (2026-09-24, 31% → 38%) |

The reference itself fails 8 of these once asked for RTL (`memory::17,18` by
signal, `export-class::1,2,24`, `pipelined::14`, `basics::335` exit 1,
`risc-v.k` past the 900 s budget at 7.5 GB); they are reported, never
graded.

## E. Unwired passes

Passes 11, 12's two remaining arms, 15, 17's rewrite half and 18 are all
waiting on pass 8's rewriting half, which is A1. Not separate work.

## F. The command lines

- **`kg` keys a root by its spelling.** The legacy `kanagawa` knows a file
  by its canonical path, as the reference does (`Parser/IO.hs:104`,
  2026-09-19); the modern CLI interns what the user typed, so a relative
  root that an import also reaches — `kg build system.k` in
  `library/base` — is read twice. Canonical identity with shell-relative
  display is a renderer change. Small.

## Order of attack

1. Cheap and self-contained: A5's member-lookup and `findSymbol` rules, A4's
   parse-time rules, A3, A6's folder clauses, A8, and C. Roughly a hundred
   snippets combined, each probed against the oracle first.
2. Scope-accurate pass 9 (A5's main half). Medium risk, high value, and it
   makes hover and completion honest as well.
3. The second elaboration round (A1). The architectural item with the biggest
   payoff; A2 and A7 follow it.
4. The backend bridge (D). Independent, so it can start any time; the step
   after the frontend gaps close.
