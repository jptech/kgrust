# Default arguments

*Part of [edition 1.1](../1.1.md). **Preview**: it may still change
before a release freezes it.*

**What it adds:** a parameter may declare a default, and a call may leave
it out. In 1.0, and in the reference, a parameter ends at its name
(`Parser.hs:379`), so `= …` after one is a parse error, and no 1.0
program holds one.

Alone, on any edition: `[language] features = ["default-arguments"]`.

## Syntax

```kanagawa
enum Mode : uint2
{
    Idle,
    Run
}

const uint8 Step = 2;

inline uint8 scale(uint8 value, uint8 shift = Step + 1, bool round = false, Mode mode = Mode::Run)
{
    return round && mode == Mode::Run ? value >> shift : value << shift;
}
```

```text
parameter = attributes type identifier [ "=" expression ]
```

- **A default is a constant**: a literal, a constant, an enum's constant,
  a template's parameter, or an expression of them — whatever the
  compiler folds to an integer, a `bool`, a `float` or an enum value.
  A member, another parameter, a local or a call is not.
- **Any parameter may have one**, not only the last: a call gives the
  ones before it by position, or skips them by naming the ones after
  ([named arguments](named-arguments.md)). Template parameters have the
  same freedom (`Type.hs:1405-1428`), and
  [named template arguments](named-template-arguments.md) skip them the
  same way.
- `kg fmt` spaces the `=` as an initializer's: `uint8 shift = 1`.

## Meaning

A call that leaves out a parameter with a default means the call with the
default written in its place — the program edition 1.0 would have written.
That is what the compiler does: once the callee is typed, before any pass
that reads a call's arguments by position, it writes each default the call
leaves out into the call (`crates/kg-sem/src/named.rs`), so the backend is
given the whole call.

```kanagawa
uint8 a = scale(p);                       // scale(p, Step + 1, false, Mode::Run)
uint8 b = scale(p, 1);                    // scale(p, 1, false, Mode::Run)
uint8 c = scale(p, mode: Mode::Idle);     // scale(p, Step + 1, false, Mode::Idle)
```

- **Positional arguments** fill the parameters from the first; the call
  is given the defaults of every parameter after the last.
- **With named arguments**, a call is given the default of every
  parameter no argument fills, wherever it is.
- **The value** is the default's, folded where the function is declared:
  `Step + 1` is the constant `3`, typed as the reference types the folded
  expression (`uint9`: a `uint8` and a literal), exactly as if the call
  had written `Step + 1` itself. An enum's constant is given by its name.
- **A method of a template class**, `uint8 clip(uint8 value, uint8 width
  = Width)`, has the default of its instance: `Window<5>`'s is `5`.
- **A function template's** defaults are its instance's, once its
  template arguments are known, written or deduced.
- **Across files.** A default is the function's, and so is a call's to
  lean on, in a file of any edition: a 1.0 file may call a function a 1.1
  library declares with defaults, and leave them out.
- **Evaluated as constants.** A default is evaluated where the call
  stands, but is a constant: it has nothing to evaluate in any order, and
  the [order-of-evaluation rule](named-arguments.md#order-of-evaluation)
  of named arguments does not count it.

## Errors

| Error | When |
|---|---|
| `` the default of `x` is not a constant a call can be given: … `` | a default that never folds to a constant integer, `bool`, `float` or enum value — reported once, at the default, and at no call of the function |
| `` `f` is missing an argument for `x` `` | a call that names its arguments leaves out a parameter with no default |
| `Invalid number of arguments, expected N` | a call by position leaves out a parameter with no default: the reference's own error |
| `expected an expression` (a parse error) | `uint8 x =` with nothing after it |

The fixtures in
[`tests/editions/default-arguments/reject/`](../../../tests/editions/default-arguments/reject/)
pin the first and the third.

## The tools

| Tool | 1.1 |
|---|---|
| `kg check`, `kg build` | defaults written into the calls that leave them out; the backend receives the whole call |
| `kg fmt`, editor formatting | `uint8 shift = 1` |
| `kg doc` | the signature shows each default as written, a name in one linked to its declaration |
| language server | signature help shows the defaults; hover on a parameter, or on a label naming it, shows `uint8 shift = 1`; an inlay hint at a call that leaves defaults out shows what it is given, `scale(p⟨, shift: 1, mode: Mode::Run⟩)` |
| `kanagawa` | rejects a default, as the reference does |

The inlay hint reads the default as the declaration spells it, so a
default written `Step + 1` shows as `Step + 1`; it is computed only in a
program with a file of an edition that has default arguments.

## How it is graded

`cargo xtask editions` ([`gates.md`](../../gates.md)):

- with the feature on, every file of the reference's tree parses to the
  same tree as with it off;
- [`accept/defaults.k`](../../../tests/editions/default-arguments/accept/defaults.k)
  leaves out an integer default folded from a constant, a `bool`, an
  enum's constant, a method's and a class template's parameter, and walks
  through the trace backend call for call as its twin, which writes every
  call whole and which the reference accepts;
- [`reject/constants.k`](../../../tests/editions/default-arguments/reject/constants.k)
  and [`reject/missing.k`](../../../tests/editions/default-arguments/reject/missing.k)
  give exactly the errors they list.

Unit tests, each proven with `scripts/mutant.sh`: the parser, the
formatter, the type (`crates/kg-ty/src/ty.rs`: a default is kept as a
parameter's name is), the database (`crates/kg-db/tests/editions.rs`: the
defaults placed beside positional and named arguments, an enum's default
folded after the function is first typed, each error), the language
server's hover and inlay hints, and `kg doc`'s signature. By hand,
2026-10-30: `kg build` of a file using each kind of default through the
real backend, and a 1.0 package calling a 1.1 library's function short of
its defaulted arguments.
