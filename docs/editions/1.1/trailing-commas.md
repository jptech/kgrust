# Trailing commas

*Part of [edition 1.1](../1.1.md). **Preview**: it may still change
before a release freezes it.*

**What it adds:** a list may end in a comma, before its closing bracket.
The reference separates every list with `sepBy` or a `comma >>` recursion
(`Parser.hs:289-291,363,377,441,551,697,860,867-871`), so a comma before the
closing bracket is a parse error there, and no 1.0 program holds one. The
comma changes nothing else: a list with one means what the list without it
means.

Alone, on any edition: `[language] features = ["trailing-commas"]`.

## Where

| List | Example |
|---|---|
| call arguments | `add(1, 2,)` |
| an intrinsic's arguments | `mux(sel, a, b,)`, `concat(hi, lo,)` |
| parameters | `uint8 add(uint8 x, uint8 y,)` |
| a function type's parameters | `(uint8, uint8,) -> uint8 f` |
| template parameters | `template <typename T, auto N,>` |
| template arguments | `Scaler<uint8, 3,>` |
| initializer lists, positional and designated | `{1, 2,}`, `{.lo = 1, .hi = 2,}` |
| enum constants | `enum Mode : uint2 { Idle, Run, }` |
| a lambda's captures | `[scale, offset,](uint8 x) -> uint8 { … }` |

The comma earns its keep in a list written one element a line. Adding an
element is then one new line, and a review's diff shows that line alone:

```kanagawa
enum Opcode : uint4
{
    Load,
    Store,
    Add,      // the next opcode is one line, and the diff shows only it
}

uint8 total = sum(
    first,
    second,
);
```

It pairs with [named arguments](named-arguments.md), which tend to be
written one a line:

```kanagawa
uint32 c = acc.add(
    value: a,
    shift: 2,
);
```

## What stays an error

- **Two commas**, `f(1,,)`, or a comma alone, `f(,)`. Nothing may stand
  between commas.
- **A comma where the reference has none to trail**: a statement list, a
  struct's members (each ends in `;`), an attribute list, a module's
  exposure list. Those lists are untouched.
- **In a file without the feature**, each trailing comma is one error, at
  the comma: `` a trailing comma needs edition 1.1: `[package] edition =
  "1.1"` in kanagawa.toml, or the feature `trailing-commas` alone ``.

## The tools

| Tool | 1.1 |
|---|---|
| `kg check`, `kg build` | the comma is read and dropped; the backend receives the list without it |
| `kg fmt`, editor formatting | the comma is kept where written, and a list holding one is laid out as any other |
| language server | diagnostics, highlighting and navigation as for any list |
| `kanagawa` | rejects a trailing comma, as the reference does |

## How it is graded

`cargo xtask editions` ([`gates.md`](../../gates.md)):

- with the feature on, every file of the reference's tree parses to the
  same tree as with it off;
- [`accept/lists.k`](../../../tests/editions/trailing-commas/accept/lists.k)
  holds a trailing comma in every list above, and walks through the trace
  backend call for call as its twin without them, which the reference
  accepts;
- [`reject/commas.k`](../../../tests/editions/trailing-commas/reject/commas.k)
  gives exactly the errors it lists for two commas.

Unit tests: the parser (`crates/kg-parser/src/grammar/tests.rs`, each list
with a trailing comma in 1.1 and rejected in 1.0) and the formatter (a
trailing comma kept, the layout a fixpoint).
