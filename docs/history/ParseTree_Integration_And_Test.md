# ParseTree integration and testing — an initial rundown

> **Done.** The plan this document laid out landed: `kg build` compiles
> through the C++ backend (2026-09-17), graded by `ptdiff`, `backend` and
> `output` (`docs/status.md`'s headline). Kept as the design record.

*2026-09-07. A plan, not a record. Every fact about the reference cited here
is from [`spec/abi.md`](../spec/abi.md), which is the contract this work
implements; this document says in what order, and what each step proves.*

> **Progress, 2026-09-17.** Steps 1 and 2 are done, with four departures
> from the text below, each argued in [`journal.md`](../journal.md), "The
> boundary is observable": the library is `dlopen`ed, not linked
> (`kg-backend-sys`); the recorder is `crates/kg-ptrace`, not `tools/ptrace`,
> and on macOS it sees depth 0 only; the `#options` line is fields, not a
> hash; and traces are a stamped cache under `target/ptrace/`, recorded and
> replayed by `cargo xtask ptrace`, not fixtures under `test/`. The
> reference's own output is kept beside each trace, which is what confirms
> the corpus's backend-code assertions. Steps 3 and 4 landed the same day:
> `kg-ptbridge` walks the route's rewritten IR (not a new `LoweredExp` — the
> provenance argument won) through a generated `Backend` trait, and
> `cargo xtask ptdiff` is the gate, with the histograms of first
> divergences and declines as the work list; 580 of 2,421 programs agree
> structurally. Step 5 landed too: `kg build` and the compat `kanagawa`
> compile through the real backend, and `cargo xtask backend` grades the
> backend's verdict per program against the recorded runs (912 of 2,422
> agree). The 241 backend-code assertions are graded that way now — by the
> backend itself, on our walk — rather than booked undecidable.

## Why

The frontend's real output is not a tree we print. It is the sequence of
calls the Haskell frontend makes into `libkanagawa-backend` through the 85
`extern "C"` functions of `parse_tree.h` (`spec/abi.md` §1, §4). Types,
mangled instance names, desugared lambdas, materialised copies, the
`[[schedule(1)]]` that `atomic` becomes (§9) — every decision the frontend
makes is observable only as an argument to one of those calls. So a recorded
trace of them is the strongest oracle we can have for a *correct* program,
and it is exactly the harness the backend bridge (roadmap item 6) needs to
be built against.

Today's instruments each see part of this. `conformance` sees verdicts and
error codes; `errors` sees the first diagnostic's position and message;
`xtask ir` compares our lowered IR against `kg-oracle --tree` at ranges,
and agrees on 93% of spans. None of them sees what the backend is told.
The 241 assertions that expect a backend error code are unreachable by all
of them, and reachable by this: identical calls into the real backend must
yield the identical code.

## The boundary, in brief

* One shared library, `libkanagawa-backend.dylib`, found by rpath from the
  `kanagawa` executable (§0). `kanagawa-build/dist/bin/` holds both, built
  on 2026-09-06.
* 85 exported C functions plus `InternalTests` (§1); a stateful protocol
  (§6): `InitCompiler(options)`, a walk that builds nodes bottom-up and
  returns opaque handles, `SetNodeType`/`SetLocation` side channels, then
  `Codegen`, which runs N+1 passes over export classes (§12.4).
* The backend **calls the API itself** from inside `parse_tree.cpp` (§5), so
  a raw trace holds more calls than the frontend issued; the recommended
  filter is a thread-local depth counter (§5.4, §14.2).
* Integers cross as decimal strings, floats as real `float`s, scopes as
  outer→inner arrays the backend reverses (§7, §8). 18 node kinds must never
  reach the walk (§11).

## The plan, in the order it should land

### 1. `tools/ptrace` — the recorder

A shim shared library exporting every backend symbol; each forwarder logs
the call, forwards to the real library, logs the result. §14 already
designs it in detail: the line format (`<seq> <depth> <fn> <arg>* -> <ret>`,
§14.1), pointer-to-logical-id mapping with epochs (§14.2), forwarders
generated from the bindgen AST in `build.rs` so the count is never
hand-maintained (§14.3), platform loading (§14.4), and what it cannot see
(§14.5).

Two decisions §14 leaves open that matter for testing:

* **Locations are a separate channel.** `SetLocation` and every `const
  Location*` argument carry the reference's spans, which differ from ours on
  7% of nodes by construction (divergence #25). The trace should keep them,
  and the gate should compare with and without them, as two numbers.
* **Depth 0 is the comparison region.** Everything the frontend issued is
  `depth == 0 && seq < seq(Codegen)` (§5.4). Deeper calls are the backend's
  own and are kept for the second use of the trace, validating the bridge
  against the backend's contract, never for grading the frontend.

Cost: a day against the spec. Verification: the trace of one `uint32 x =
5;` must match the worked example in §6.4 line for line, and §15's probes
(`ParseBaseList` aliasing, untyped attribute literals) get answered as a
side effect of the first traces.

### 2. Fixtures — every corpus root, recorded once

Record `test/` and `library/` roots under pinned options (`--target-device
mock`, the harness's defaults, §3 for the struct) and check the traces in
under `test/ptrace/`. Two reasons to freeze them rather than record on every
run: the reference build is the owner's and not every machine has it, and a
fixture that changes is a diff worth reading — it means the reference moved,
as it did on 2026-09-02.

The `#options` line (§14.1) is the guard: a trace recorded under different
options is rejected, not compared.

### 3. `kg-ptbridge` with a `Backend` trait, trace writer first

The walker over our final program — the twin route's output, since that is
where copies and deduced types exist — emits calls through a trait with two
implementations:

* `Trace` writes the same line format the recorder does, ids assigned by
  the same first-sighting rule.
* `Real` calls `kg-backend-sys` and is item 6 proper.

Writing `Trace` first means the walker is developed against fixtures with
no C++ in the loop, and every disagreement is a line diff with a function
name in it. It also gives `kg build --backend trace` a use of its own: a
frontend-only artefact that a reviewer can read.

This is where the frontend gaps become visible as *counts*: the walk cannot
emit a node kind from §11's undefined list, so a program the twin route
still declines (depth-two instances, foreign deduced calls with value
arguments, roadmap item 14) produces a trace that stops early, and the gate
says at which call.

### 4. `xtask ptdiff` — the gate

Per root: our trace against the fixture, depth-0 region only, with three
numbers — exact, exact-ignoring-locations, and first divergent call. Summary
like `--strict`: how many roots agree, and the per-file table of what does
not. The rule from `CLAUDE.md` applies from day one: a root that agrees
because both sides emit nothing is not agreement, so the gate reports
empty traces separately.

What the gate must be blind to, stated up front: logical ids are
per-process, so two traces that create nodes in a different *order* but the
same shape will differ; `#epoch` boundaries inside `Codegen` are
unobservable (§14.2); and anything after `Codegen` is best-effort.

### 5. The real bridge

`kg-backend-sys` (bindgen over `parse_tree.h` + `options.h`, layout tests
on, §3.4's poke-width hazard pinned) and the `Real` implementation of the
trait. By then the walker is proven, so the bridge's own tests are the
end-to-end ones: `kg build` on a template-free root produces the same
`.sv`/`.cpp` as `dist/bin/kanagawa`, and the 241 backend-code assertions
are graded by running the same walk through the same library.

## What this tests, and what it does not

It tests the *correct* program's contract completely and exactly. It does
not test diagnostics: a program the frontend rejects never reaches the
walk, so `conformance` and `errors` stay the gates for verdicts, positions
and messages. The two complement each other along the same line the
reference draws (`Main.hs:70-76`): frontend errors stop the run before
`InitCompiler`; backend errors come from `Codegen`.

It also does not test the backend. A trace fixture is what the *Haskell*
frontend said; if the reference frontend is wrong about something, the
fixture is wrong with it, exactly as the conformance corpus is. That is the
right blindness for a port.

## Sequencing and cost

| step | depends on | rough cost | what it proves |
|---|---|---|---|
| 1 recorder | `kanagawa-build`, `spec/abi.md` §14 | a day | the boundary is observable; §15 probes answered |
| 2 fixtures | 1 | an hour, plus disk | the contract is frozen |
| 3 trace writer | 2, the twin route | the bulk of item 6's walker | our program matches the reference's, call by call |
| 4 `ptdiff` | 3 | half a day | a number that must not fall |
| 5 real bridge | 3, 4 | item 6 | RTL, and the 241 |

Steps 1 and 2 need the reference build and can start now, independently of
frontend work. Step 3 is not extra work relative to item 6; it is item 6's
walker written in a testable order.

## Open questions to settle early

* **Recursion filter or shape filter?** §5.4 recommends depth; an
  alternative is to filter by *caller* using the internal-caller graph in
  §5.1. Depth is simpler and the graph is the check on it.
* **What does our side walk?** The twin holds copies for same-module and
  minted foreign instances only. Whether the walker reads the twin's IR
  (the rewritten `ModuleIr`, with `Src` provenance for locations) or a new
  lowered form (`LoweredExp`, §11.2) is the first design decision of step 3.
  The provenance argument favours the IR.
* **Options fidelity.** The `Options` struct is 520 bytes with hsc2hs poke
  hazards (§3.4). The fixtures should be recorded with one options vector,
  and the bridge's first job is to reproduce its FNV hash on the `#options`
  line.
* **Platform.** The recorder is a macOS dylib today. Linux needs `$ORIGIN`
  rpath handling (§0, §14.4); nothing else differs.
