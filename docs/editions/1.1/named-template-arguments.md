# Named template arguments

*Part of [edition 1.1](../1.1.md). **Preview**: it may still change
before a release freezes it.*

**What it adds:** a template's arguments may be named by its parameters,
in any order, and a parameter with a default may be skipped wherever it
stands, not only at the end. In 1.0, and in the reference, a template
argument is a bare expression or type (`Parser.hs:697`), and `IDENT ':'`
begins none (`::` is one token; a ternary's `:` follows its `?`), so no 1.0
program holds a label.

Alone, on any edition: `[language] features = ["named-template-arguments"]`.

## Syntax

The standard library's FIFO takes four parameters, the last two with
defaults:

```kanagawa
template <typename T, auto Size, bool EnqueueBlocking = true, bool DequeueBlocking = true>
class FIFO { … }
```

In 1.0, a FIFO that does not block on dequeue must spell out the default
before it, and the reader must count to tell which `bool` is which:

```kanagawa
FIFO<uint8, 4, true, false> _queue;
```

In 1.1:

```kanagawa
FIFO<uint8, Size: 4, DequeueBlocking: false> _queue;
```

```text
template-arguments = "<" [ argument { "," argument } ] ">"
argument           = [ identifier ":" ] expression
```

- A label is an identifier and a colon before an argument, as in a call
  ([named arguments](named-arguments.md)).
- **Positional arguments come first**: once one is named, every argument
  after it is named.
- **A label is given once per list**.
- `kg fmt` writes `Buffer<Size: 8, T: uint16>`.

## Meaning

A template instance whose arguments are named means the instance with
every argument in its parameter's position, and every parameter left out
given its default — the instance edition 1.0 would have written. That is
what the compiler does, where it already fills in defaults
(`resolveTemplateArgs`, `Type.hs:1405-1428`, in `crates/kg-sem/src/deduce.rs`):
the labelled arguments are placed by name, the defaults of the
parameters left out are substituted as the reference substitutes those
past the last argument written, and the positional instance goes on to
be instantiated as any other.

```kanagawa
template <typename T, auto Size = 4, bool Wide = false, auto Shift = Size + 1>
class Buffer { … }

Buffer<uint8, Wide: true> _a;            // Buffer<uint8, 4, true, 5>
Buffer<Size: 8, T: uint16> _b;           // Buffer<uint16, 8, false, 9>
Buffer<uint8, 3, Shift: 1> _c;           // Buffer<uint8, 3, false, 1>
```

- **A default reads the arguments as given.** `Shift = Size + 1` is
  `9` in `_b`, whose `Size` was named, as it is `5` in `_a`, whose `Size`
  was left out.
- **Function templates** take named template arguments too, and deduce
  the rest from the call as before: `scaled<N: 3>(p)` names `N` and
  deduces `T` from `p`.
- **Any order is safe.** A template argument is a constant or a type, so
  placing it in another order than written changes nothing — unlike a
  call's arguments, whose
  [order of evaluation](named-arguments.md#order-of-evaluation) is kept.

## Errors

| Error | When |
|---|---|
| `` `Buffer` has no template parameter named `Depth` `` | a label names no parameter of the template |
| `` the template parameter `T` is given an argument twice `` | a named argument fills a parameter a positional or another named one already filled |
| `` `Buffer` takes 3 template arguments `` | more positional arguments than parameters, in a list that names any |
| `` `Buffer` is missing a template argument for `T` `` | a class template's parameter with no default that no argument fills, reported at the instance |
| `` Could not deduce all arguments for template `f` `` | a function template's parameter no argument fills and the call does not deduce: the reference's own error |
| `` the argument `x` is named twice ``, `a positional argument cannot follow a named one` | parse errors, as in a call |

Each is reported once. The fixture
[`tests/editions/named-template-arguments/reject/names.k`](../../../tests/editions/named-template-arguments/reject/names.k)
pins the first, second, fourth and a function template's.

## The tools

| Tool | 1.1 |
|---|---|
| `kg check`, `kg build` | named template arguments placed and checked; the backend receives the positional instance |
| `kg fmt`, editor formatting | `Buffer<Size: 8>`, the colon against the label |
| language server | a label is highlighted as a parameter; go-to-definition, hover, references and highlight on a label reach the template's parameter; renaming a template's parameter renames its labels, in files nobody has open too; completion in a template's argument list offers the parameters left to name, as `Size:` |
| `kanagawa` | rejects a named template argument, as the reference does |

## How it is graded

`cargo xtask editions` ([`gates.md`](../../gates.md)):

- with the feature on, every file of the reference's tree parses to the
  same tree as with it off;
- [`accept/templates.k`](../../../tests/editions/named-template-arguments/accept/templates.k)
  names a class template's arguments in and out of order, skips defaults
  before and after one given, lets a default read a named argument, and
  names a function template's argument with the rest deduced; it walks
  through the trace backend call for call as its twin, which writes every
  instance positionally and which the reference accepts;
- [`reject/names.k`](../../../tests/editions/named-template-arguments/reject/names.k)
  gives exactly the errors it lists.

Unit tests, each proven with `scripts/mutant.sh`: the parser, the
formatter, the database (`crates/kg-db/tests/editions.rs`: every error,
and a label resolved to its template's parameter), and the language
server's completion and cross-file rename. By hand, 2026-10-30: `kg build`
of `FIFO<uint8, Size: 4, DequeueBlocking: false>` and of
`FIFO<uint8, 4, true, false>` through the real backend give the same
SystemVerilog, comments aside.
