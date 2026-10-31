# Named arguments

*Part of [edition 1.1](../1.1.md). **Preview**: it may still change
before a release freezes it.*

**What it adds:** a call may name its arguments by their parameters, in
any order. A label is a parse error in 1.0 — and in the reference
(`Parser.hs:871`: a call's arguments are bare expressions) — so no 1.0
program contains one.

Alone, on any edition: `[language] features = ["named-arguments"]`.

## Syntax

```kanagawa
uint16 a = sub(y: q, x: p);       // every argument named, any order
uint16 b = sub(p, y: q);          // positional first, then named
uint32 c = acc.add(shift: 2, value: a);
```

```text
arguments = "(" [ argument { "," argument } ] ")"
argument  = [ identifier ":" ] expression
```

- A label is an identifier and a colon before an argument.
- **Positional arguments come first.** Once one argument is named, every
  argument after it is named: `f(x: 1, 2)` is an error, *a positional
  argument cannot follow a named one*.
- **A label is given once per call**: `f(x: 1, x: 2)` is an error, *the
  argument `x` is named twice*.
- The label's colon hugs the label: `kg fmt` writes `f(x: 1)`.

## Meaning

A call that names its arguments means the call with every argument moved
to its parameter's position — the program edition 1.0 would have written.
That is what the compiler does: before any pass that reads a call's
arguments by position, it puts them in their parameters' order and drops
the labels, so the backend is given the 1.0 call (`crates/kg-sem/src/named.rs`).

- **Positional arguments** fill the parameters from the first, as in 1.0.
- **A named argument** fills the parameter of that name.
- **The names are the callee's type's.** A function's and a method's are
  its declaration's; a closure's are its lambda's; a value of function
  type has the names its type spells — `(uint8 a, uint8 b) -> uint8 fn`
  takes `fn(b: x, a: 1)`. A callee whose parameters have no names takes
  no labels.
- **Templates.** A method of a template class takes the names of its
  instance's method. A function template's arguments may be named before
  its template arguments are known: deduction matches each named argument
  to its parameter by name (`deduceFromFuncParam`'s pairing, `Type.hs:1477`,
  made by name instead of position), and so `pick(first: true, b: q, a: p)`
  deduces `pick`'s `auto` parameters from `p` and `q`. Its template
  arguments may be named too
  ([named template arguments](named-template-arguments.md)):
  `pick<T: uint8>(…)`.
- **Pipelined functions.** A `[[pipelined]]` function's first parameter
  is the thread's index, and a call passes the thread count in its place
  (`Type.hs:1364-1373`; `compiler/cpp/ir.cpp:10932`). The count is written
  first, by position; the parameters after it may be named:
  `Work(4, v: 3, flag: f)`. The index is no argument's to name.
- **Every parameter gets exactly one argument** — written, or its default
  where it has one ([default arguments](default-arguments.md)): naming
  the arguments after a parameter with a default skips it.

### Order of evaluation

The arguments are evaluated **in the order written**, as everywhere else
in Kanagawa. Placing them in their parameters' order would change that,
since the backend evaluates a call's arguments in list order
(`CallNode::GenerateIR`, `compiler/cpp/ir.cpp:10912-10919`), so the
compiler places them only where the change cannot be seen. Each argument
is classed by what evaluating it can do:

| Class | Holds | Examples |
|---|---|---|
| pure | literals, and the enclosing function's own locals and parameters — nothing a call could change | `3`, `p`, `p + 1` |
| reads | anything else that is read: a member, a global, a memory, a constant — what a call might change | `_m`, `_mem[i]` |
| calls | any argument that calls a function | `g()`, `p + g()` |

Two arguments may swap when neither calls a function, or when one calls
and the other is pure. If the parameters' order would swap any other pair,
the call is an error, at the argument that would move ahead:

```kanagawa
uint32 d = sub(y: g(), x: h());   // error: in `sub`'s parameter order `x` would be
                                  // evaluated before `y`, which is written first,
                                  // and one of them calls a function: …
uint16 h = sub(y: g(), x: p);     // fine: `p` is a parameter, which `g` cannot change
```

Write such a call in the parameters' order, or bind one argument to a
local first. The rule is conservative by design: it never runs side
effects in another order than written, at the cost of refusing some
calls that would have been harmless. (A global constant is classed
*reads*, though no call can change it.)

## Errors

Every error is reported once, at the argument it is about; the call is
otherwise left alone. The fixtures in
[`tests/editions/named-arguments/reject/`](../../../tests/editions/named-arguments/reject/)
pin each message.

| Error | When |
|---|---|
| `` `f` has no parameter named `z` `` | a label names no parameter of the callee |
| `` the parameter `x` is given an argument twice `` | a named argument fills a parameter a positional one or another named one already filled |
| `` `f` takes N arguments `` | more positional arguments than parameters |
| `` `f` is missing an argument for `x` `` | a parameter no argument fills and that has no default (`the thread count`, for a pipelined call) |
| `` in `f`'s parameter order `x` would be evaluated before `y`, which is written first, and one of them calls a function: … `` | the order-of-evaluation rule above |
| `` `tid` is the thread's index in pipelined `f`, which no call passes: … `` | a label names a pipelined function's first parameter |
| `` `p` is not a function whose parameters have names `` | a labelled call whose callee is not a function |
| `` `f` takes no parameters to name `` | a labelled call to a function of no parameters |
| `` cannot tell which of `f`'s parameters these arguments name `` | the callee's parameters never became known (the backstop: no fixture reaches it) |
| `the argument `x` is named twice`, `a positional argument cannot follow a named one` | parse errors, above |

A call inside a template is checked where the template is instantiated,
as the reference checks every template; an undefined callee is reported
as undefined, with nothing added.

## The tools

| Tool | 1.1 |
|---|---|
| `kg check`, `kg build` | named arguments resolved and checked; the backend receives the positional call |
| `kg fmt`, editor formatting | `f(x: 1)`, the colon against the label |
| language server | diagnostics; a label is highlighted as a parameter; go-to-definition, hover, references and highlight on a label reach its parameter; renaming a parameter renames its labels, in files nobody has open too; signature help lights the parameter a label names; completion offers the parameters an argument could name, as `x:` |
| `kg dump ir` | the lowering shows the label (`Labelled`), before the loop resolves it |
| `kanagawa` | rejects named arguments, as the reference does |

The language server finds a label's parameter through the callee's
declaration, as signature help finds the signature: a callee that is a
value — a closure, a parameter of function type — has its parameters' names
only in a type, so a label of a call through one reaches nothing, and a
rename of a lambda's parameter leaves such labels as they are.

## How it is graded

`cargo xtask editions` ([`gates.md`](../../gates.md)):

- with the feature on, every file of the reference's tree parses to the
  same tree as with it off;
- every fixture is rejected by `kg-oracle --errors` and by edition 1.0;
- each accepted fixture under
  [`tests/editions/named-arguments/accept/`](../../../tests/editions/named-arguments/accept/)
  walks through the trace backend call for call as its hand-written
  edition-1.0 twin (`x.ref.k`), which the reference accepts;
- each rejected fixture gives exactly the errors it lists.

Unit tests, each proven with `scripts/mutant.sh`: the parser
(`crates/kg-parser/src/grammar/tests.rs`), the formatter, the semantic
tokens, the database (`crates/kg-db/tests/editions.rs`: a file read in its
package's language, a change of language reparsing it, every error), and
the walk against its twin (`crates/kg-ptbridge/tests/editions.rs`). By
hand, 2026-10-30: `kg build` of `reorder.k` and of its twin through the
real backend give the same SystemVerilog, location comments aside.
