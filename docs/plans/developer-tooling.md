# Developer tooling: what exists, its state, and what to consider

*Part of the [plans](README.md). **Survey, 2026-10-11 — nothing decided.**
**Progress, later 2026-10-11:** the release archive (`cargo xtask dist`,
from the submodule's backend), doc comments in hover and completion, and
`kg doc` are built — §2.1 and §2.2 say what is left of each.
Written at the owner's request so that, as the frontend and tooling work
finishes, the shortcomings can be taken up as a whole. Read alongside
[`backend-survey.md`](backend-survey.md) (a whole build is the backend's)
and [`loop-across-edits.md`](loop-across-edits.md) §8 (the editor's
keystroke). Every "today" below was checked in the code or by running the
release binaries at `72e6a69`.*

## 1. What exists

The toolchain is one Cargo workspace whose tools all call one core
(`kg-db`'s queries over `kg-sem`'s program-wide loop), so what the
compiler knows the editor knows.

| Tool | State today | Held by |
|---|---|---|
| `kg check` | the whole frontend, every pass of the reference's; `--error-format human\|short\|json`; warnings `-W` and `[warnings]`; exit codes 0/1/2/3 documented | `conformance` 2681/2681, `errors`, `repoint` 72/72 |
| `kg build` | the frontend, then the reference's C++ backend through its C ABI in a worker process (a backend crash is exit 3, not a dead `kg`); refuses with exit 2 what it cannot lower, never compiles wrong | `ptdiff` 2422/2422, `backend` 3267/3270, `output` 0 differ |
| `kanagawa` | the reference's command line, argv-compatible, every mode and flag | `cli` 25/25 |
| `kg fmt` | comment-preserving, over the lossless tree; refuses a file that does not parse; proves each result the same program (tokens, comments, order); `--check` for CI, `--stdout`, `[fmt] line-width`; its style measured from the corpus (`spec/style.md`) | `fmt-check` 405/405 idempotent and unchanged |
| `kg lsp` | diagnostics in two layers (the fourth round's, then the fixpoint's, cancellable), quick fixes, definition, type definition, references and rename across the project, hover, signature help, inlay hints (widths), completion, document and workspace symbols, folding, selection ranges, semantic tokens (full and range), document highlight, formatting (document and range), file watching; on a broken buffer the last loop's answers mapped onto the new text | `edit-identity` 348/348, `resilience`, `shadow`, the keystroke bench, `xtask mem` |
| VS Code extension (`editors/vscode`) | a thin client: spawns `kg lsp`, a TextMate grammar generated from the parser's keyword list as the fallback, restart and log commands, three settings | built by hand (`npm run compile`, `vsce package`); **not published** |
| `kg dump` | tokens, CST, AST, the lowered IR, the template instances the loop made, round by round | — |
| `kg explain` | every code named: `Error 1` (the frontend) and every warning with prose; `Error 2`–`84` (the backend's) by name and source line only | — |
| `kg doc` | *(2026-10-11)* a static site from the doc comments: a page per module in the reference generator's structure (description, export-list index with its sections, exposed declarations), linked signatures, parameters and members, folded examples, search; Markdown by `pulldown-cmark`, no pandoc. The standard library: 124 modules, 1,215 declarations, 730 documented, in 2 s | `kg-doc`'s tests; `kg-syntax`'s over the library |
| Release archive | *(2026-10-11)* `cargo xtask dist`: `kg`, `kanagawa`, the backend library beside them, the standard library (`kg`'s import directory of last resort), `LICENSE`, generated `THIRD-PARTY-NOTICES.md`; 14 MiB on macOS. The backend from the submodule, `scripts/build-backend.sh` | proven by hand from a fresh unpack: check, build, doc |
| `kanagawa.toml` | `[package]`, `[project]` (import dirs, defines, base library, device), `[warnings]`, `[fmt]`, `[lsp]`, `[hw]` (the backend's knobs), `[profile.<name>]` overlays; discovered from the nearest ancestor | — |

For the compiler's own developers the instruments are deep and gated:
per-item ratchets over every gate (`baseline.json`), the differential
fuzzer with reduction, `infer-identity`/`identity.sh`/`edit-identity`/
`shadow` for exactness, `mutant.sh` for tests, `bench` and `mem`, the
three-column comparison (`vs-haskell.sh`), and trace flags
(`KG_LOOP_PROFILE` by pass and by round, `KG_PHASES`, …). What they lack
is a CI: there is no remote yet, and the full gates take ~6 minutes and
peak near 17 GB.

## 2. Gaps worth considering

Grouped by whom they serve. Each says what it would take and what it
depends on; none is scheduled.

### 2.1 Getting the tools to users

- **No published release.** *Built 2026-10-11:* the archive
  (`cargo xtask dist`) and the backend from the submodule
  (`scripts/build-backend.sh`), proven on macOS. Left: publishing — the
  remote, a CI building the archive per platform (Linux untested), and
  signing/notarisation on macOS.
- **The extension is not published.** A Marketplace / Open VSX release
  that bundles or downloads `kg` per platform is what puts the editor in
  front of anyone. The reference ships its own VS Code package
  (`kanagawa/tools/editors/vscode`, TextMate only); ours supersedes it.
- *Effort*: small to medium; *depends on*: owner decisions about naming,
  publishing and the backend library's distribution.

### 2.2 Documentation, in the editor and out of it

- **Doc comments in the editor** — *done 2026-10-11*: hover shows a
  declaration's `//|` and `//<` runs under its type, an import the module's
  own description; completion resolves the focused item's
  (`completionItem/resolve`). Read whether or not `--parse-docs` is set
  (`kg_syntax::file_docs`), Pandoc's Markdown as CommonMark.
- **`kg doc`** — *built 2026-10-11* (`kg-doc`), replacing the reference's
  `sandcastle` (a separate Haskell tool whose site build wants pandoc and
  node). Left: svgbob diagrams rendered as pictures (shown as text today),
  sandcastle's `--svgbob`; re-exposed modules' declarations listed on the
  re-exposing page (linked only); a Markdown output beside the HTML.
- **Doc tests**: the reference's `chkdoc` compiles every code block of its
  markdown documentation (`kanagawa/tools/chkdoc`, `spec/cli.md` §8.9); a
  `kg check` over a markdown file's code blocks would give the same to any
  project's docs.
- *Effort*: hover docs small; `kg doc` medium; doc tests small.

### 2.3 Testing and simulation

- **No `kg test`.** The reference's tests are CMake targets that compile
  with the backend and simulate with Verilator, using the library's own
  fixture framework (`library/test/unit.k`; `spec/cli.md` §8). A `kg test`
  that builds, simulates and reports, with "run test" code lenses in the
  editor, is the largest workflow gap for a hardware designer.
- *Effort*: large; *depends on*: the backend library, Verilator, the
  reference's SV runtime, and probably the whole-run cache
  (`backend-survey.md` §3) so that unchanged designs are not rebuilt.

### 2.4 Project workflow

- **`kg init`**, with `--from-cmake` importing an existing CMake project's
  settings from the `*.kanagawa.args` files the reference's CMake already
  writes, is specified (`spec/cli.md` §10.4), not built.
- **No watch mode** (`kg check --watch`, `kg build --watch`).
- **`-j/--jobs` is accepted and does nothing**: the frontend is single
  threaded (`performance.md` §13). Either it gains a meaning with parallel
  rounds, or its help should say so.
- **Timings**: `KG_PHASES` and `KG_LOOP_PROFILE` are trace variables; a
  user-facing `--timings` (frontend phases, the backend's stage table) would
  let users see where their build goes — usually the backend.

### 2.5 Diagnostics and explanations

- **`kg explain` has prose for the frontend's code and the warnings only.**
  The backend's 83 codes are named, not explained — and they are the codes
  a user meets after a clean `kg check`.
- **Backend errors appear only under `kg build`.** Running the backend in
  the background for the editor (`backend-survey.md` §3) would bring its
  verdicts into the editor; latency is a full backend run.
- **CI formats**: JSON diagnostics exist; SARIF (code scanning) does not.
- **Lints beyond the reference's six warnings** — the width surprises the
  README names (`a - b` always signed, `a << b` widening by `2^width(b) −
  1`) as opt-in lints. Additive, but every one needs a decision that it is
  not a divergence: the reference has no such lint.

### 2.6 Editor features not yet offered

Call hierarchy (who calls this function, through which instances); code
lenses (a template's instance count; "run test"); a "show instance"
command opening an instance's elaborated body (the data is `kg dump
instances`'); completion and inlay-hint resolve; snippet completions;
semantic-token deltas; pull diagnostics. Rename's follow-ups are in the
handoff (member renames through lambdas and abbreviated templates; the
no-manifest policy, the owner's).

### 2.7 What only a hardware language's tools can show

The distinctive opportunity. The backend already computes, per build,
what a designer most wants to see beside the source: pipeline stages,
path lengths (`<base>PathLength.pdrpt`), resource usage, and a source-to-
RTL map (`<base>RtlMap.json`). Surfaced in the editor — hover "stage 3 of
7", "go to generated RTL", a path's critical length, a width explained
("why `uint17`") — they would be features no HLS toolchain in this
language has. They depend on the backend running in the background and on
the cache (2.5, `backend-survey.md` §3), and on the backend's reports being
read rather than reproduced.

### 2.8 Other editors and the wider ecosystem

The server is editor-agnostic; documented configurations for Neovim,
Helix, Zed and Emacs cost little. A **tree-sitter grammar** — generated
from `grammar/kanagawa.ungram` or the parser — is what Helix, Zed and
Neovim highlight with and what GitHub's linguist could use. A **web
playground** (the frontend built to WebAssembly: check, hover, format, no
backend) would serve documentation and onboarding.

## 3. A suggested order (the lead's; for the owner)

1. **Release and publish** (2.1): the archive is built; the remote, CI per
   platform and the extension's publishing remain. Every other item reaches
   nobody without it.
2. **Doc comments in hover and completion** (2.2): *done*, and `kg doc`;
   doc tests remain, on the same plumbing.
3. **The whole-run cache and the background backend** (2.5,
   `backend-survey.md` §3): faster rebuilds, backend errors in the editor,
   and the base for 2.3 and 2.7.
4. **`kg test`** (2.3), scoped once 3 exists.
5. **Hardware insight in the editor** (2.7), after 3.
6. The small items as they fit: call hierarchy, code lenses, show-instance,
   watch, `--timings`, `kg init --from-cmake`, explain prose for the
   backend's codes, SARIF, the tree-sitter grammar; and `-j`'s meaning.

Where the owner's open decisions bear: publishing and naming (1); the
backend's scope and distribution (1, 3, 5); lints that the reference does
not have (2.5); the no-manifest rename policy (2.6).
