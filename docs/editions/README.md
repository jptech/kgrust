# Language editions

*Part of the [documentation map](../README.md). Current as built,
2026-10-31; the design and its reasons are
[`../plans/editions.md`](../plans/editions.md).*

Kanagawa as upstream ships it is **edition 1.0**. A package may opt into a
later edition, or into single features ahead of one; nothing else ever
changes the language a file is read in. A program that opts into nothing
checks, diagnoses and builds exactly as it did before editions existed.

| Edition | Status | Adds | The difference, in full |
|---|---|---|---|
| `1.0` | the default; follows `reference.lock`'s pin | — | [`1.0.md`](1.0.md) |
| `1.1` | **preview**: may still change | call sites that say what they mean: named arguments, trailing commas, default arguments, named template arguments | [`1.1.md`](1.1.md) |

## Opting in

```toml
# kanagawa.toml
[package]
edition = "1.1"                  # default "1.0"

[language]
features = ["named-arguments"]   # single features, beside the edition
```

- **`edition`** names a bundle of features, frozen once a release ships
  it (`1.1` is still a preview). A file is in every feature of its edition
  and of the editions before it. It is a string: `edition = 1.1`, a
  number, is refused with the spelling to write.
- **`[language] features`** turns on single features on top of the
  edition: a way to try one before the edition that holds it, or to stay
  on an older edition and take one feature. Known features:
  `named-arguments`, `trailing-commas`, `default-arguments`,
  `named-template-arguments`.
- **An edition or a feature this `kg` does not know is an error**, never
  ignored: `` unknown edition `2.0`: this kg knows 1.0, 1.1 ``. An older
  tool must not compile a newer program as something else. A dependency's
  manifest is held to the same: `kg check`, `kg build` and `kg doc` report
  it as they report the project's own, and exit 2. In the editor either
  is marked in `kanagawa.toml` itself, at the value refused, and said once
  in a message: the files under a refused manifest are read as if it were
  not there, in edition 1.0.
- **A feature used without it is one error naming the edition**: `f(x: 1)`
  in a 1.0 file is `` named arguments need edition 1.1, which `kg` reads
  from kanagawa.toml: `[package] edition = "1.1"`, or the feature
  `named-arguments` alone ``, and not the parse errors of a language
  without labels. The program is rejected wherever the reference rejects
  it — the error may stand a token earlier than the reference's — and only
  what it is told changes ([`known-divergences.md`](../known-divergences.md)
  #42).
- **On the command line**, `--edition <VERSION>` and `--feature <NAME>`
  (repeatable) replace the project's edition and add to its features, for
  one run: `kg check --edition 1.1 file.k`. `check`, `build`, `doc`, `fmt`
  and `dump` take them; they change the project's own files and nothing
  else (below). `kg lsp` reads the manifests only.
- **The `kanagawa` binary never opts in.** It reads no manifest — not even
  one above a file it is given from another directory — and has no such
  flag: it is the reference's interface, and it rejects every edition's
  syntax wherever the reference does, with the error above. So a program
  that leans on a later edition, a 1.0 file calling a 1.1 library's
  defaults included, builds with `kg` only.

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
- a manifest that does not load names no language: its files read as `1.0`,
  and the manifest is reported — the project's where it always was, a
  dependency's by every command that reaches its files (above);
- with `--no-manifest`, no manifest is read for the language either: the
  files under the first input's directory are in the flags' language,
  every other in `1.0`.

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

**Parameter names are API in 1.1.** A call that names an argument, or a
template argument, names the callee's parameter, the standard library's
included (`FIFO<uint8, Size: 4>`). Upstream renaming one breaks such a
call when the pin moves, and no gate sees it: the corpus is 1.0 code.

## The tools

Every command that reads a project reads each file in its own language:
`kg check`, `kg build`, `kg doc` and the language server through the
database; `kg fmt` and the editor's formatting (a file that uses a
feature its package has not turned on is refused as unparseable, as by
every other command); `kg board`'s interface reader. `kg dump` reads one
file and no manifest: its language is the flags' alone. A change to a
manifest's edition reparses the files it governs, in the editor too.

### `kg expand`

`kg expand` shows the reference program a later edition's program stands
for: your own text, with only the edition's constructs rewritten. Trailing
commas are dropped, labels are removed and arguments put in their parameters'
order, defaults are written into the calls that leave them out, and named
template arguments are put in their places. Comments and layout elsewhere stay
as they are (a comment between two arguments stays where it is when they swap),
and a file that uses nothing of a later edition comes back byte for byte.

```kanagawa
// edition 1.1                              // kg expand
const uint8 Step = 2;                       const uint8 Step = 2;

uint32 scale(                               uint32 scale(
    uint32 value,                               uint32 value,
    uint32 factor = 1,                          uint32 factor,
    uint32 offset = Step + 1,                   uint32 offset
)                                           )
{ … }                                       { … }

scale(offset: 5, value: a);                 scale(a, 1, 5);
scale(a, 3);                                scale(a, 3, 3u9);
Buffer<Fast: true> b;                       Buffer<8, 16, true> b;
```

A default is written as the value the compiler folded it to, its type kept:
`3u9` is what `Step + 1` is, a `uint9`. The declared text may not mean the same
where the call stands.

One file goes to stdout, the program and nothing else, with anything else
said on stderr. `--out DIR` (`-o`) writes every file of the project the program
reaches under the directory, at its path in the project, and never over the
sources; two roots need it. A file of the project is one under its root and
under no other package's manifest: a dependency with a manifest of its own and
the standard library keep their own language and are not written, so a project
whose dependency is in a later edition expands to a program that still needs
that dependency's expansion.

**What cannot be written exactly is refused**, naming the construct, and no
file is written; the command exits 1:

| Refused | Why |
|---|---|
| a list the template's instances place differently | a default naming the template's parameter is another value in each instance, and one text cannot say both |
| a labelled list, or a call by position of a function with a default, that nothing compiles | a template no one instantiates, a `static if` branch not kept, a list folded into a constant: nothing says where the arguments go |
| a skipped template argument that is not an integer, a `bool` or one of the language's types | edition 1.0 must write it to reach the ones after, and a class's name may not mean the same there |
| a default that means another thing where it lands | an enum's constant whose name a nearer declaration shadows |
| a default whose expression makes an instance no other text makes | without the default, the program has no reason for it |

Every expansion is proved before it is given: read in edition 1.0 it must
check clean and walk into the backend call for call as the program does.
A program with an error gives `kg check`'s errors, and no text, and so does an
evaluation order only the labels could keep
([named arguments](1.1/named-arguments.md#order-of-evaluation)).

## How an edition is built

Every extension lives in the frontend and is gone before the backend:
what the walk (`kg-ptbridge`) hands the C++ backend is a program the
reference could have handed it. The rules, in short (in full:
[`../plans/editions.md`](../plans/editions.md) §1):

1. with no opt-in, no program the reference accepts reaches a new branch —
   a later edition's syntax in a 1.0 file is parsed only to be refused by
   name — and the existing gates prove it on every run;
2. an extension claims only programs the reference rejects — checked
   against `kg-oracle`;
3. everything desugars to reference Kanagawa before the walk — checked
   against a hand-written reference twin, walk for walk;
4. a file's language is its own package's;
5. unknown is an error;
6. `kanagawa` never opts in.
