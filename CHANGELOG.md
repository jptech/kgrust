# Changelog

Notable changes to kgrust, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `AGENTS.md`, the working rules for people and coding agents alike, and
  `CONTRIBUTING.md`; `docs/gates.md`, every gate in one place.
- CI on GitHub Actions, sized for the free runners: `scripts/ci.sh`, the
  tests and corpus gates in one job and clippy and the links in another, on
  Linux for every push and pull request and on macOS for `main`. It builds
  neither the backend nor the reference compiler. It covers the workspace
  but `kg-ptrace` — `xtask`'s own tests too — and checks the generated
  files are current (`codegen --check`). Actions are pinned by commit, the
  token is read-only, and pull requests reuse `main`'s cache.
- Completion after a path's `::`: an enum's constants, each with its value
  and documentation (`mode::` → `pass = 0`, `smooth = 1`); what a class,
  struct or union declares; what a `using` alias's type declares; what an
  aliased import exposes. Typing `:` asks for it. Hover shows an enum
  constant's value too.
- The VS Code extension builds to a `.vsix` with `npm run package`, and
  <kbd>F5</kbd> in its folder starts an Extension Development Host
  (`.vscode/launch.json`); its README covers installing, an isolated
  profile, and which `kg` to point it at.
- `scripts/build-backend.sh --fetch-boost` installs Boost 1.88's headers
  where the system's are older (Ubuntu 24.04 ships 1.83).
- Repository metadata on every crate; `publish = false` across the
  workspace. The VS Code extension gains its repository, license, bug
  tracker and a `kanagawa.trace.server` setting.

### Changed

- The documentation reorganised for readers outside the project: a map by
  audience with a note on how to read the pre-publication record, a
  roadmap of where it stands, what is next and the open questions, and the
  finished sprint plan, the IR design and the 2026-10-11 handoff moved to
  `docs/history/`.
- Every spec opens with a status line — the upstream commit it was read at,
  what changed upstream since for the files it cites, and where the code
  supersedes a proposal — and its drifted citations are re-derived at the
  pin. `spec/cli.md` gains "as built" boxes for `kg` and `kanagawa.toml`.
- `kg --help` names `build` and `doc`. The VS Code extension needs VS Code
  1.82 or later (its language client's minimum).

### Fixed

- A `kanagawa.toml` whose `[fmt]` or `[lsp]` table set only some keys was
  refused ("missing field `indent`"); the rest now take their defaults.
- `cargo xtask status-md` refuses a partial gate run; `docs/status.md` had
  been published with most of its headline as `?`.
- `cargo xtask` with `KANAGAWA_ROOT` unset doubled the reference's path
  (2,088 conformance failures); a relative root is now made absolute.
- The VS Code extension's `.vsix` left out its language client
  (`node_modules` was excluded), so a packaged extension would not load.
- A timing test in `kg-db` failed on a loaded machine (a 20 ms sleep took
  56); it now holds however long a sleep takes.
- `scripts/vs-haskell.sh` runs on Linux (GNU `time`, `find`, `sed`).
- On Linux, a gate's child over its memory or time budget lost only its
  leader: procps's `kill` took `kill -KILL -<group>` as two options and
  signalled nothing, so a grandchild (a backend worker) ran on.

## 0.1.0 — first published state, 2026-10-11

Not tagged as a release. What the repository held when it was published:

- **The frontend**, at parity with the reference compiler on everything the
  reference's own corpus grades: 2,681 of 2,681 decidable conformance
  assertions, upstream's syntax test runner 72 of 72, the walk into the
  C++ backend identical call for call on 2,422 programs.
- **The tools:** `kg check`, `build`, `fmt`, `lsp`, `doc`, `dump`,
  `explain`, and `kanagawa`, the reference's command line, argv-compatible.
- **The language server:** diagnostics in two layers, quick fixes,
  definition and type definition, references and rename across a project,
  hover and completion with types — per instance inside a template — and
  with documentation, signature help, inlay hints for widths, symbols,
  folding, selection ranges, semantic tokens, formatting.
- **The backend built from source** (`scripts/build-backend.sh`) from the
  `third_party/kanagawa` submodule, and **release archives**
  (`cargo xtask dist`).
