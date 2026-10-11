# Kanagawa for VS Code

A thin client for `kg-lsp`, the Kanagawa language server in
[`crates/kg-lsp`](../../crates/kg-lsp). It spawns the server and registers the
`kanagawa` language for `.k` files. Nothing about the language is implemented
here — the outline, folding, selection ranges, semantic highlighting and
diagnostics are all computed by the server, from the same syntax tree the
compiler uses.

## The two commands

```sh
npm install          # once, to fetch vscode-languageclient and the TypeScript compiler
npm run compile      # tsc -p ./  → out/extension.js
```

Then press <kbd>F5</kbd> in VS Code with this folder open to launch an Extension
Development Host, or `npx vsce package` to build a `.vsix`.

The server is *not* built by either command. Build it from the workspace root:

```sh
cargo build -p kg-lsp        # → target/debug/kg-lsp
```

and point the extension at it:

```jsonc
{
  "kanagawa.server.path": "/path/to/kgrust/target/debug/kg-lsp",
  "kanagawa.server.args": []      // `kg-lsp` needs no subcommand
}
```

The defaults are `kg` and `["lsp"]`, i.e. the shipping `kg lsp` command, found
on `PATH`.

## What the server provides

| Feature | Notes |
|---|---|
| Diagnostics | Published on every keystroke, or on save — `[lsp] diagnostics` in `kanagawa.toml`. Where templates need more than four rounds of the program-wide loop to instantiate, they land twice: first at the fourth round, which may miss an error a later round finds and never shows one it would not, then from the same run at its fixpoint — exactly what `kg check` reports. An edit cancels a check it supersedes, and the first answer stays until the edit's own replaces it |
| Quick fixes | A diagnostic's fix — the cast a narrowing or invalid conversion suggests — applied from the light bulb |
| Go to definition, type definition, references, rename | An import's path opens the module's file; type definition lands on the struct, class or enum a name's type is (an instance's at its template); references and rename across the project's files, each searched under the program it roots (bounded: 512 files, 10 s for references, 60 s for rename); rename refuses what it cannot rewrite everywhere |
| Hover, signature help, inlay hints, completion | Hover and a focused completion show the declaration's documentation — its `//|` and `//<` comments, Markdown — under its type. Types from the compiler's program-wide loop — inside a template's body, each instance's; while the loop runs on past its fourth round, the fourth round's, without waiting (a template's list of instances then says it may be short); on a buffer that does not parse, the last loop's, mapped onto the new text |
| Document symbols | Hierarchical: members nest under their class, struct, union or enum |
| Workspace symbols | Every declaration of every open document's program, imported modules included, found by name |
| Folding ranges | Every braced body, plus comment blocks and the import block |
| Selection ranges | Expand-selection along the syntax tree, working even mid-edit |
| Semantic tokens | Full document and range |
| Document highlight | Other occurrences of the word under the cursor |
| Formatting | `kg-fmt`, whole document and ranges; a file with syntax errors is left alone, and the server says why |

## The TextMate grammar is a fallback

`syntaxes/kanagawa.tmLanguage.json` is **generated** by `cargo xtask codegen`
from the `SyntaxKind` enum in `crates/kg-parser`, so its keyword list is the
compiler's keyword list by construction and cannot drift from it. It exists for
exactly one situation: the server is missing, still starting, or has crashed.
VS Code paints TextMate scopes first and lets semantic tokens override them, so
with the server running you see the server's answer.

That matters, because the reference extension's hand-written grammar
(`../kanagawa/tools/editors/vscode/syntaxes/`) is wrong in three ways that are
visible in an ordinary file. Two of them a regular expression *can* fix, and the
generated fallback does; the third it genuinely cannot, and only the server
fixes it:

1. **`<` and `>` are scoped as comparison operators.** They also delimit template
   argument lists, and no regex can tell `a < b` from `Foo<T>`. *The fallback
   declines to scope bare `<` and `>` at all* — an unscoped token inherits the
   surrounding colour, which is a much smaller lie than "operator". The server
   asks the syntax tree instead: `reverse<uint<Width>>(x)` gets four brackets,
   and `a < b` gets one operator. This is decidable without a symbol table
   because `<` and `>` are declared `no_assoc` in the reference grammar
   (`Parser.hs:915-916`), so `a < b > c` is not a valid expression at all.
2. **`continue` is listed as a control keyword.** The language has no `continue`
   — it is not among the 53 reserved words at `Parser/Lexer.hs:164-219`, and
   `kg-parser` pins that with a test naming it "the TextMate grammar's phantom
   keyword". Because the fallback's keyword list is generated from the enum, the
   word cannot appear in it.
3. **`float` is scoped as a builtin type.** The reserved word is `float32`;
   bare `float` is an ordinary identifier, and colouring it lies about the
   program. The generated fallback lists the type keywords explicitly, so
   `float` is not among them. The server additionally marks `float32`, `int32`,
   `uint8`, `int<N>` and friends with the `defaultLibrary` modifier.

`language-configuration.json` is hand-written rather than copied, for two
reasons the reference file gets wrong: it declares **no** angle-bracket
auto-closing pair (auto-closing `a <` into `a <>` while someone types `a < b` is
worse than not helping), and it continues a `//|` / `//<` doc-comment run with a
plain `//`, which is what the lexer actually does — every comment after the
first in a run is absorbed into the same doc comment
(`Parser/Lexer.hs:82-84`).

The server's legend also carries a non-standard `punctuation` token type, which
is what carries fix #1; `contributes.semanticTokenScopes` in `package.json` maps
it back to a TextMate scope so themes without semantic colouring still look
right.

## Configuration

The server reads `kanagawa.toml` itself, discovered by walking up from each open
file — the same manifest `kg build` uses, so the editor and the build agree on
`import-dirs`, `target-device` and `defines`. There is nothing to configure here
beyond how to launch the server:

| Setting | Default | Meaning |
|---|---|---|
| `kanagawa.server.path` | `kg` | The executable to spawn |
| `kanagawa.server.args` | `["lsp"]` | Its arguments |
| `kanagawa.server.extraEnv` | `{}` | Extra environment variables |
| `kanagawa.trace.server` | `off` | Log the JSON-RPC conversation |

**Kanagawa: Restart Language Server** and **Kanagawa: Show Language Server Log**
are in the command palette.
