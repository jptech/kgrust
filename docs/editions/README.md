# Language editions

*Part of the [documentation map](../README.md). Current as built on the
`editions-pilot` branch, 2026-10-30; the design and its reasons are
[`../plans/editions.md`](../plans/editions.md).*

Kanagawa as upstream ships it is **edition 1.0**. A package may opt into a
later edition, or into single features ahead of one; nothing else ever
changes the language a file is read in. A program that opts into nothing
checks, diagnoses and builds exactly as it did before editions existed.

| Edition | Status | Adds | The difference, in full |
|---|---|---|---|
| `1.0` | the default; follows `reference.lock`'s pin | — | [`1.0.md`](1.0.md) |
| `1.1` | **preview**: may still change | named arguments, in any order | [`1.1.md`](1.1.md) |

## Opting in

```toml
# kanagawa.toml
[package]
edition = "1.1"                  # default "1.0"

[language]
features = ["named-arguments"]   # single features, beside the edition
```

- **`edition`** names a frozen bundle. A file is in every feature of its
  edition and of the editions before it.
- **`[language] features`** turns on single features on top of the
  edition: a way to try one before the edition that holds it, or to stay
  on an older edition and take one feature. Known features:
  `named-arguments`.
- **An edition or a feature this `kg` does not know is an error**, never
  ignored: `unknown edition `2.0`: this kg knows 1.0, 1.1`. An older tool
  must not compile a newer program as something else.
- **On the command line**, `--edition <VERSION>` and `--feature <NAME>`
  (repeatable) replace the project's edition and add to its features, for
  one run: `kg check --edition 1.1 file.k`. They change the project's own
  files and nothing else (below).
- **The `kanagawa` binary never opts in.** It reads no manifest and has no
  such flag: it is the reference's interface, and it rejects every
  edition's syntax exactly as the reference does.

## What a version means

Editions are numbered `MAJOR.MINOR`.

- **A minor edition only adds.** Every program the edition before it
  accepts, it accepts and means the same; it gives a meaning only to
  programs the edition before it rejects. `1.1` is `1.0` plus syntax that
  `1.0` refuses as a parse error. So moving a package from `1.0` to `1.1`
  cannot change what any of its code does.
- **A major edition may also take away**: reject what an earlier edition
  accepts — stricter checks — and only a major edition may. None exists
  yet, and none is planned; [`../plans/editions.md`](../plans/editions.md)
  §8 keeps the question open. A program written for an earlier edition
  keeps compiling there, since a file's edition is its own package's.
- **A preview edition may still change** before a release freezes it, as
  `1.1` is now. Once frozen, an edition's meaning never changes.
- **Edition `1.0` follows the reference.** It is upstream Kanagawa at the
  pin, whatever the pin; when the pin moves, `1.0` moves with it, and the
  `editions` gate says whether upstream has given a meaning to syntax a
  later edition claims.

Each edition's page lists everything that differs from the edition before
it: syntax, meaning, diagnostics, and the tools.

## Which language a file is in

A file's language is decided by **its own package**, never by the package
that imports it: walking up from the file's directory, the first
`kanagawa.toml` names it. So:

- the project's own files, its `lib/` and anything else under its manifest
  are in the project's language — with `--edition` and `--feature` applied;
- a dependency with a manifest of its own keeps the language that
  manifest names, whatever the importer's;
- the standard library — the release's bundled copy, or the submodule's —
  has no manifest above it and is edition `1.0`;
- a file with no manifest above it at all is edition `1.0`, and so are the
  synthetic `.cmdargs` and `.options` modules;
- a manifest that does not load names no language: its files read as `1.0`
  (and the manifest's own error is reported where it always was).

Why no boundary at an import directory: a feature never changes the
meaning of a program the language without it accepts (the minor rule
above), so a library read in a later edition than its own reads the same.
The walk can therefore take the nearest manifest — which also finds the
common layout of a dependency's manifest beside the `src/` its users
import.

**A single file, no manifest.** `kg check --edition 1.1 design.k` treats
the directory of the first input as the project's root: files under it
are in the flags' language, the standard library is not. The language
server reads manifests only — an editor buffer gets the language its
`kanagawa.toml` names, or `1.0`.

## The tools

Every command reads each file in its own language: `kg check`, `kg build`
and the language server through the database; `kg fmt` and the editor's
formatting (a file that uses a feature its package has not turned on is
refused as unparseable, as by every other command); `kg dump`; `kg board`'s
interface reader. A change to a manifest's edition reparses the files it
governs, in the editor too.

## How an edition is built

Every extension lives in the frontend and is gone before the backend:
what the walk (`kg-ptbridge`) hands the C++ backend is a program the
reference could have handed it. The rules, in short (in full:
[`../plans/editions.md`](../plans/editions.md) §1):

1. with no opt-in, no new branch is reachable — the existing gates prove
   it on every run;
2. an extension claims only programs the reference rejects — checked
   against `kg-oracle`;
3. everything desugars to reference Kanagawa before the walk — checked
   against a hand-written reference twin, walk for walk;
4. a file's language is its own package's;
5. unknown is an error;
6. `kanagawa` never opts in.
