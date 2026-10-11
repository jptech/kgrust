# Kanagawa — Complete Syntactic Specification

Extracted from the reference Haskell compiler for the purpose of a Rust rewrite.

**Primary sources** (all paths relative to `third_party/kanagawa/`):

| File | Lines | Role |
|---|---|---|
| `compiler/hs/lib/Language/Kanagawa/Parser.hs` | 968 | the entire parser (one giant `where`-block inside `program`) |
| `compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs` | 805 | AST node definitions (`ExpF`) and auxiliary enums |
| `compiler/hs/lib/Language/Kanagawa/Parser/Lexer.hs` | 288 | tokens, keywords, literals, comments |
| `compiler/hs/lib/Language/Kanagawa/Parser/SymbolTable.hs` | 279 | parser state: scopes, modules, `findSymbol` |
| `compiler/hs/lib/Language/Kanagawa/Parser/Syntax/TH.hs` | 99 | TH deriving of pattern synonyms (`AliasF` → `Alias`) |
| `compiler/hs/lib/Language/Kanagawa/Parser/Pattern.hs` | 16 | `$(deriveExplBidirPatternSyn ''ExpF)` |
| `compiler/hs/lib/Language/Kanagawa/Parser/Syntax/Pattern.hs` | 16 | `$(deriveUnidirPatternSynAndCompletePragma 'Note ''ExpF)` |
| `compiler/hs/lib/Language/Kanagawa/Parser/Syntax/FixPattern.hs` | 16 | `$(deriveUnidirPatternSyn 'NotedExp ''ExpF)` |
| `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs` | 119 | file/import resolution, the two synthetic modules |
| `compiler/hs/lib/Language/Kanagawa/Parser/Options.hs` | 51 | `ParseOptions` |
| `compiler/hs/lib/Language/Kanagawa/PrettyPrint.hs` | 232 | canonical **source** rendering of every node — used here as a cross-check |

Every claim below carries a `path:line` citation. Claims that could not be settled
from the source are marked **UNVERIFIED** or moved to *Open questions*.

## 0. Reading notes

### 0.1 Pattern synonyms

`Parser.hs` never mentions the `…F` constructors directly. `Parser/Pattern.hs:16` runs
`$(deriveExplBidirPatternSyn ''ExpF)`, which (per `Parser/Syntax/TH.hs:78-99`) generates for
every constructor `XyzF` a bidirectional pattern synonym `Xyz` whose *construction*
direction wraps the node with `Src` annotation `SrcUnknown`:

```haskell
pattern Alias n a b <- NotedExp _ (AliasF n a b)
  where Alias n a b =  NotedExp SrcUnknown (AliasF n a b)
```
(`Parser/Syntax/TH.hs:72-75`, the doc example; the mechanism is `TH.hs:83-99`.)

So in the parser, `Struct <$ keyword "struct" …` builds `StructF …` and *the note is
`SrcUnknown` unless the whole thing is wrapped in `noted`*. The three wrappers are
`noted` (`Parser.hs:58-64`), `unaryNoted` (`:66-77`), `binaryNoted` (`:78-89`), and
`notedWithDocs` (`:91-96`). Only the **outermost** node produced by the wrapped parser
gets a real `Src`; inner nodes built by pattern-synonym application keep `SrcUnknown`.
This is why, e.g., attribute nodes carry no source location.

### 0.2 Shadowed combinators

Inside `program`'s `where` block, four list combinators are **redefined** and shadow
`Text.Megaparsec`'s (which are hidden at `Parser.hs:36`):

| local | definition | site |
|---|---|---|
| `many p` | `noted $ Seq <$> MP.many (notedWithDocs p)` | `Parser.hs:114` |
| `sepBy p s` | `noted $ Seq <$> (notedWithDocs p \`MP.sepBy\` s)` | `Parser.hs:115` |
| `sepBy1 p s` | `noted $ Seq <$> (noted p \`MP.sepBy1\` s)` | `Parser.hs:116` |
| `sepEndBy p s` | `noted $ Seq <$> (notedWithDocs p \`MP.sepEndBy\` s)` | `Parser.hs:117` |

They all wrap the result in a **`SeqF`** node. Note `sepBy1` uses `noted`, not
`notedWithDocs` — so elements of a `sepBy1` list never carry doc comments.

### 0.3 Parser monad and backtracking

`type Parser' s e a = ReaderT Config (StateT SymbolTable (ParsecT e s Cache))` (`Parser.hs:47`).
`Cache = StateT [(FilePath, ParseResult)] IO` (`Parser.hs:55`) — the import cache lives in `IO`.

Consequences that matter for a Rust port:

* `try` rolls back **both** the megaparsec state (input position *and* the
  `registerParseError` accumulator) **and** the `SymbolTable`, because
  `MonadParsec`'s `StateT` instance threads the state through the parser's result.
* `lookAhead` discards `SymbolTable` mutations entirely. This is what makes the
  multi-pass class/function-template pre-scan (`Parser.hs:307-309, 316-330, 428-430`)
  non-polluting.
* `registerFancyError` / `registerSymbolError` (`Error.hs:130-137`) record a
  **non-fatal** error and let the parse continue. `unexpectedSymbol` (`Error.hs:115-116`)
  and `parseError` **fail** the parser.

### 0.4 `Src` annotation

```haskell
data Src = Src { offset :: !Int, begin :: !SourcePos, end :: !SourcePos
               , docs :: ([DocComment], [DocComment]) }
         | SrcStack Src Src
         | SrcUnknown
```
(`Parser/Syntax.hs:609-621`.) `docs` is `(preceding-doc-comments, following-doc-comments)`,
filled only by `notedWithDocs` (`Parser.hs:91-96`).

---

## 1. Lexical structure

### 1.1 Whitespace and comments — `Lexer.hs:94-119`

Two modes selected by `Config.parseDocs` (`Lexer.hs:67-73`, set from
`ParseOptions.parseDocs` at `Parser/IO.hs:94`):

* `parseDocs = False` → `whitespaceAll` (`Lexer.hs:100-103`): `space1`,
  line comment `//…`, block comment `/* … */` **nested** (`L.skipBlockCommentNested`).
* `parseDocs = True` → `whitespaceNonDoc` (`Lexer.hs:105-119`): identical, except a
  comment whose opener is followed (after horizontal space) by `|` or `<` is **not**
  skipped; it is left in the stream for `docCommentPre`/`docCommentPost`.

Doc-comment prefixes (`Lexer.hs:63-65`):
* `docCommentPrefixPre  = '|'` → `//| …`, `/*| … */` documents the item that **follows**.
* `docCommentPrefixPost = '<'` → `//< …`, `/*< … */` documents the item that **precedes**.

`docComment` (`Lexer.hs:75-88`) collects a run of `LineComment [String]` /
`BlockComment [String]` (`Parser/Syntax.hs:601-603`). A line doc comment consumes
`//` then everything up to `\n`; a block doc comment `/*` … `*/` (non-nested here,
`Lexer.hs:88`).

`skipBraces = lexeme $ L.skipBlockCommentNested "{" "}"` (`Lexer.hs:121-122`) — a *raw
brace counter* used to skip bodies during look-ahead passes. See §9 trap 3.

### 1.2 Reserved words — `Lexer.hs:164-219` (complete, 53 entries)

```
as        atomic    auto      barrier    bitsizeof  bitoffsetof
bool      break     bytesizeof byteoffsetof cast     case
class     clog2     concat    const      decltype   default
do        else      enum      export     extern     false
fan_out   float32   for       if         import     inline
int       lutmul    module    mux        noinline   private
public    reorder   return    static     static_assert string
struct    switch    template  true       typename   uint
union     unrolled_for using   void      while
```

(53 entries, one per source line `Lexer.hs:166-218`.) `unrolled_for` is reserved but **never referenced by the parser** —
it is dead (removed per `tools/editors/vscode/CHANGELOG.md:26`). `while`, `break`,
`case`, `default`, `as` are used only in specific productions.

`keyword k` (`Lexer.hs:248-253`) *errors at compile time* if `k` is not in the reserved
set; `keyword'` (`Lexer.hs:245-246`) does not check. Both require the keyword not be
followed by `alphaNumChar` or `_`.
`keyword2 k1 k2 = try (keyword k1 >> keyword' k2)` (`Lexer.hs:255-256`) — used only for
`static assert` (`Parser.hs:573`), which is why `assert` is **not** reserved.

Keywords actually consumed by `Parser.hs` (via `keyword`, `keyword2`, `open`, or
`keyword (show x)`):
`as, atomic, auto, barrier, bitsizeof, bitoffsetof, bool, break, bytesizeof,
byteoffsetof, case, cast, class, clog2, concat, const, decltype, default, do, else,
enum, export, extern, false, fan_out, float32, for, if, import, inline, int, lutmul,
module, mux, noinline, private, public, reorder, return, static, static_assert, string,
struct, switch, template, true, typename, uint, union, using, void, while`
plus the two-word form `static assert`.
Not consumed: `unrolled_for`.

### 1.3 Identifiers — `Lexer.hs:221-243`

```
ident  ::= (letter | "_") { letter | digit | "_" }
```
`name` (`Lexer.hs:235-243`) rejects an `ident` if it is in `reserved` **or** matches
`int<decimal>` / `uint<decimal>` where `<decimal>` parses as a `Word`
(`Lexer.hs:241-243`). So `int7`, `uint1024` can never be identifiers, but `int_`,
`intx`, `int07`… — `int07` **is** rejected (`readMaybe "07" :: Maybe Word` = `Just 7`).

`dotSepName` (`Lexer.hs:230-233`) — used for module names only — allows `-` in each
component: `ident'(alnum | '_' | '-')` separated by `.`, at least one component.

### 1.4 Literals

**Integer** — `Lexer.hs:133-156`, wrapped at `Parser.hs:904-908`:
```
integer   ::= hex | binary | octal | decimal          -- tried in that order, Lexer.hs:134
hex       ::= "0" ("x"|"X") "_"? hexdigit+ [hexdigit|"_"]*
binary    ::= "0" ("b"|"B") "_"? bindigit+ [bindigit|"_"]*
octal     ::= "0" ("o"|"O") "_"? octdigit+ [octdigit|"_"]*
decimal   ::= digit+ [digit|"_"]*
intLiteral::= integer ( ("i"|"u") decimal )?           -- Parser.hs:904-908
```
The radix prefix letter is matched case-insensitively (`char'` at `Lexer.hs:136`).
At least one digit must precede any `_` (`takeWhile1P` then `takeWhileP`,
`Lexer.hs:148-149`). Suffix `i`/`u` comes from `show SignedLiteral = "i"` /
`show UnsignedLiteral = "u"` (`Parser/Syntax.hs:223-225`); the width is `L.decimal`
(`Parser.hs:908`). Result: `IntLiteralF (Maybe LiteralType) Integer`
(`Parser/Syntax.hs:347-350`), `LiteralType { literalSignedness, literalWidth }`
(`Parser/Syntax.hs:216-220`).

**Float** — `float = lexeme L.float` (`Lexer.hs:158-159`), used at `Parser.hs:879`
inside `try`. Megaparsec's `L.float` requires `digit+ "." digit+` with optional
exponent. Stored as Haskell `Float` (32-bit) — `FloatLiteralF Float`
(`Parser/Syntax.hs:352`).

**Plain string** — `stringLiteral` (`Lexer.hs:161-162`) = `"` `charLiteral*` `"` with
Haskell escape rules (`L.charLiteral`). Used **only** by `strLiteral`
(`Parser.hs:821`), which is used **only** by `strAttr` (`Parser.hs:177`), i.e. only in
`[[name("…")]]`. Produces `StringLiteralF String`.

**Interpolated string** — every other `"…"` in the language, `Parser.hs:883-899`.
See §3.13.

### 1.5 Operator and punctuation tokens

`operator o` (`Lexer.hs:261-262`):
```haskell
operator o = (lexeme . try) (string o *> notFollowedBy (oneOf (o ++ "=")))
```
i.e. the token `o` must not be followed by `=` **nor by any character occurring in `o`**.
Consequences (each is a real disambiguation rule a Rust lexer must reproduce):

| token | forbidden follower set | so these are *not* that token |
|---|---|---|
| `+` | `+=` | `++`, `+=` |
| `-` | `-=` | `--`, `-=` |
| `*` | `*=` | `**`, `*=` |
| `/` | `/=` | `//`, `/=` |
| `%` | `%=` | `%%`, `%=` |
| `!` | `!=` | `!!`, `!=` |
| `~` | `~=` | `~~`, `~=` |
| `&` | `&=` | `&&`, `&=` |
| `\|` | `\|=` | `\|\|`, `\|=` |
| `^` | `^=` | `^^`, `^=` |
| `&&` | `&=` | `&&&`, `&&=` |
| `\|\|` | `\|=` | `\|\|\|`, `\|\|=` |
| `^^` | `^=` | `^^^`, `^^=` |
| `<` | `<=` | `<<`, `<=` |
| `>` | `>=` | `>>`, `>=` |
| `<<` | `<=` | `<<<`, `<<=` |
| `>>` | `>=` | `>>>`, `>>=` |
| `==` | `=` | `===` |
| `!=` | `!=` | `!==`, `!=!` |
| `<=` | `<=` | `<==`, `<=<` |
| `>=` | `>=` | `>==`, `>=>` |
| `?` | `?=` | `??`, `?=` |
| `:` | `:=` | `::`, `:=` |
| `--` | `-=` | `---`, `--=` |
| `++` | `+=` | `+++`, `++=` |

Note in particular: **`x - -y` requires whitespace** (`x--y` lexes `--` and then fails
as an operator); and the ternary `:` cannot be immediately followed by `:`
(so `c ? X::A : X::B` is fine but `c?a::b` is not).

Angle brackets are a *separate* token pair — `Lexer.hs:268-273`:
```haskell
langle = _angle "<";  rangle = _angle ">"
_angle o = lexeme $ try $ string o *> notFollowedBy (char '=')
```
`_angle` only forbids a following `=`. **There is no `>>` token for closing template
argument lists** — `Foo<Bar<int32>>` closes with two independent `rangle` tokens, which
is why nested templates work with no special maximal-munch hack.

Fixed punctuation (`Lexer.hs:264-288`), all via `symbol` (= `L.symbol whitespace`, so
each consumes trailing whitespace/comments):
`(` `)` `{` `}` `[` `]` `;` `,` `:` `.` `=` `#`.
`doubleBrackets = brackets . brackets` (`Lexer.hs:279`) → `[[ … ]]`.
`hash` (`Lexer.hs:288`) is exported but **never used** by `Parser.hs` (verified: no `hash`
occurrence in `Parser.hs`).

Multi-character symbols matched with `L.symbol` (no `notFollowedBy` guard), in
`Parser.hs`:
`->` (`:447 (lambda return), :551 (function type)`), `::` (`:682`), `[[`/`]]`
(`:165`, and via `doubleBrackets`), `{` (`:888`, interpolation), `\` (`:192`, module
diff), `"` (`:883`), `template` (`:646`).

---

## 2. AST node catalogue — `Parser/Syntax.hs:282-584`

`ExpF e a` has **93** constructors. `e` is the diagnostic payload type
(`ParseError String (ExtendedError String)`), `a` is the recursive child position.
The parser's concrete type is
`ExpSrc = NotedExp Src (ParseError String (ExtendedError String))` (`SymbolTable.hs:46`),
i.e. `Fix (Note Src (ExpF …))` (`Parser/Syntax.hs:628-637`).

### 2.1 Auxiliary enumerations

**`BinaryOp`** (`Parser/Syntax.hs:75-104`) — 20 constructors, order fixed to match the
C enum `ParseTreeBinaryOpType` (`Parser/Syntax.hs:71-74`):
`Add(+) Sub(-) Mul(*) LutMul(⟂) Div(/) Mod(%) BitwiseAnd(&) And(&&) BitwiseOr(|)
Or(||) BitwiseXor(^) Xor(^^) ShiftLeft(<<) ShiftRight(>>) Equal(==) NotEqual(!=)
Greater(>) GreaterEqual(>=) Less(<) LessEqual(<=)`.
`LutMul` has `pretty = undefined` (`:88`) — it is spelled `lutmul(a, b)` in source
(`Parser.hs:866`) and rendered so by `PrettyPrint.hs:79`.

**`UnaryOp`** (`Parser/Syntax.hs:107-122`) — 6:
`Invert(~) Neg(-) Not(!) BitSizeof(bitsizeof) ByteSizeof(bytesizeof) Clog2(clog2)`.

**`Attribute`** (`Parser/Syntax.hs:124-148`) — 9 argument-taking attributes, with their
source spellings from the `Show` instance (`:136-145`):
`CallRate="call_rate" Ecc="ecc" FifoDepth="fifo_depth" Latency="latency"
MaxThreads="max_threads" Rename="name" Schedule="schedule" ThreadRate="thread_rate"
TransactionSize="transaction_size"`.

**`Flag`** (`Parser/Syntax.hs:150-179`) — 13 nullary attributes:
`Async="async" Atomic="atomic" EndTransaction="last" Initialize="initialize"
Memory="memory" NoBackPressure="no_backpressure" Pure="pure"
NonReplicated="non_replicated" Pipelined="pipelined" QuadPort="quad_port"
ReorderByLooping="reorder_by_looping" Reset="reset" Unordered="unordered"`.
(The declaration order at `:151-163` and the `Show` order at `:167-179` differ — the
`Show` instance is authoritative for spelling.)

**`DeclareFlag`** (`Parser/Syntax.hs:184-191`) — 6:
`DeclareGlobal DeclareClass DeclareLocal DeclareStatic DeclareMember DeclareUninitConst`.

**`Modifier`** (`Parser/Syntax.hs:193-203`) — `Inline="inline"`, `NoInline="noinline"`.

**`FunctionKind`** (`Parser/Syntax.hs:205-209`) — `FreeFunction MemberFunction LambdaFunction`.
(`LambdaFunction` is never used by `Parser.hs`; lambdas get their own `LambdaF` node.)

**`LiteralTypeSignedness`** (`:211-214`) `SignedLiteral="i"`, `UnsignedLiteral="u"`;
**`LiteralType`** (`:216-221`) `{ literalSignedness, literalWidth :: Int }`.

**`FormatSpecifier`** (`:233-246`) `Bin="b" Oct="o" Dec="d" Hex="x" HexUpper="X"`;
**`Format`** (`:251-256`) `{ formatSpecifier, formatPrecision :: Maybe Int }`.

**`WarningKind`** (`Warning.hs:20-35`) — `Conversion="conversion" Deprecated="deprecated"
InitializerOverrides="initializer-overrides" RedundantImport="redundant-import"
Shadow="shadow" TransactionSizeMissing="transaction-size"`. Only `RedundantImport`
is produced by the parser (`Parser.hs:239`).

**`Name`** (`Parser/Syntax.hs:51`) is a hashed `String`; **`QualifiedName = [Name]`**
(`:69`). Reserved internal names: `thisName = "$this"` (`:267-268`),
`unresolvedName = "$unresolved"` (`:270-271`),
`moduleNamespace ns = concatMap ('@':) ns` (`:273-274`).

### 2.2 Node table

`✔` in the *Parsed* column = the parser constructs this node; `✘` = it exists only
downstream (constexpr evaluation, type inference, desugaring) and a Rust **parser** need
not produce it.

#### List / structural

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `SeqF` | `[a]` | ✔ | `Parser.hs:114-117,163,167,189,237,288,561` and everywhere `many/sepBy/sepBy1/sepEndBy` are used |

#### Attributes

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `FlagAttrF` | `Flag` | ✔ | `Parser.hs:171` (`flagAttr`), `:561` (synthetic `Atomic`) |
| `IntAttrF` | `Attribute`, `a` | ✔ | `Parser.hs:173` (`intAttr`) |
| `AttrF` | `Attribute`, `a` | ✔ | `Parser.hs:175` (`identAttr`), `:177` (`strAttr`) |
| `FunctionModifierF` | `Modifier` | ✔ | `Parser.hs:404` |

#### Types

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `AutoF` | — | ✔ | `Parser.hs:526` (`autoType`), `:439` (lambda default return) |
| `VoidF` | — | ✔ | `Parser.hs:523` |
| `BooleanF` | — | ✔ | `Parser.hs:540` |
| `FloatF` | — | ✔ | `Parser.hs:541` (`float32`) |
| `StringF` | — | ✔ | `Parser.hs:525` |
| `IntegerF` | `Int` | ✔ | `Parser.hs:542` (`int<N>` suffix form) |
| `ParamIntF` | `a` | ✔ | `Parser.hs:544` (`int<expr>`) |
| `UnsignedF` | `Int` | ✔ | `Parser.hs:543` (`uint<N>` suffix form) |
| `ParamUintF` | `a` | ✔ | `Parser.hs:545` (`uint<expr>`) |
| `ConstF` | `a` | ✔ | `Parser.hs:524` (`constType`), `:451`, `:554` |
| `ReferenceF` | `a` | ✘ | only `Type.hs:1629`; **no source syntax** |
| `ArrayF` | attr `a`, elem `a`, dims `a` | ✔ | `Parser.hs:529` (with `[[…]]`), `:538` (plain) |
| `TemplateInstanceF` | name `a`, args `a` | ✔ | `Parser.hs:693`, `:694` |
| `TemplateParamF` | `QualifiedName`, kind `a`, `Name`, index `Int`, default `Maybe a` | ✔ | `Parser.hs:366,367,368` |
| `TypeParamF` | `QualifiedName`, kind `a`, `Name`, index `Int` | ✔ | `Parser.hs:660,776,789,791` |
| `TypenameF` | — | ✔ | `Parser.hs:513` |
| `FunctionTypeParamF` | attr `a`, type `a`, name `Maybe a` | ✔ | `Parser.hs:553` |
| `FunctionTypeF` | attr `a`, params `a`, ret `a` | ✔ | `Parser.hs:551` |

#### Expressions

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `ThisF` | `declScopeName :: QualifiedName` | ✘ | `Function.hs:408`, `Frontend.hs:190`; **no `this` keyword** |
| `DesignatorF` | name `a`, value `a` | ✔ | `Parser.hs:863` |
| `PositionalF` | `Int`, value `a` | ✔ | `Parser.hs:859` |
| `InitializerListF` | `a` | ✔ | `Parser.hs:856` (positional), `:862` (designated) |
| `BitOffsetF` | type `a`, member `a` | ✔ | `Parser.hs:864` |
| `ByteOffsetF` | type `a`, member `a` | ✔ | `Parser.hs:865` |
| `IntLiteralF` | `literalType :: Maybe LiteralType`, `literalValue :: Integer` | ✔ | `Parser.hs:906`; also synthesized `1` at `:614` |
| `FloatLiteralF` | `Float` | ✔ | `Parser.hs:879` |
| `BoolLiteralF` | `Bool` | ✔ | `Parser.hs:881,882` |
| `StringLiteralF` | `String` | ✔ | `Parser.hs:821` (only inside `[[name("…")]]`) |
| `InterpolatedStringF` | `[(String, Maybe (a, Bool, Maybe a, Maybe Format))]` | ✔ | `Parser.hs:883` |
| `IdentifierF` | `Name` | ✔ | `Parser.hs:268,434`; also `:133` (`$quasi<i>`), `:127` (`$`), `:764` (`$unresolved`) |
| `ScopedNameF` | scope `QualifiedName`, prefix `Maybe a`, name `a` | ✔ | `Parser.hs:655,656,219,742,764` |
| `QualifiedIdentifierF` | `a` | ✔ | `Parser.hs:644,734` |
| `TypeIdentifierF` | scope `QualifiedName`, name `a` | ✔ | `Parser.hs:707,722,725` |
| `FunctionSpecifierF` | receiver `Maybe a`, name `a` | ✔ | `Parser.hs:728,734` |
| `EnumValueF` | name `a`, value `a` | ✘ | `Constexpr.hs:292` |
| `NamedValueF` | `a` | ✔ | `Parser.hs:789,792,801` |
| `BinaryF` | `BinaryOp`, `a`, `a` | ✔ | `Parser.hs:962,963` (operator table), `:614` (`++`/`--`), `:636` (compound assign), `:866` (`lutmul`) |
| `UnaryF` | `UnaryOp`, `a` | ✔ | `Parser.hs:964` (symbolic), `:965` (`bitsizeof`/`bytesizeof`/`clog2`) |
| `FunctionCallF` | `callAttr`, `callFunction`, `callArgs` | ✔ | `Parser.hs:871` |
| `ArrayAccessF` | `a`, `a` | ✔ | `Parser.hs:809`→`:819`, `:637` |
| `ArrayAccessLValueF` | `a`, `a` | ✔ | `Parser.hs:805`→`:819` |
| `MemberAccessF` | `a`, `a` | ✔ | `Parser.hs:818` |
| `CastF` | type `a`, value `a` | ✔ | `Parser.hs:872` |
| `MuxF` | selector `a`, cases `a` | ✔ | `Parser.hs:870` (`mux(…)`), `:968` (`?:` desugar) |
| `ConcatF` | `a` | ✔ | `Parser.hs:867` |
| `FanOutF` | count `a`, value `a` | ✔ | `Parser.hs:868` |
| `StaticF` | `a` | ✔ | `Parser.hs:869` |

#### Statements

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `AnnotatedF` | attr `a`, stmt `a` | ✔ | `Parser.hs:559` (`atomic`), `:567` (`[[schedule(n)]]`) |
| `BarrierF` | — | ✔ | `Parser.hs:495` |
| `ReorderF` | `a` | ✔ | `Parser.hs:496` |
| `ReturnF` | `Maybe a` | ✔ | `Parser.hs:569` |
| `DoWhileF` | `loopAttr`, `loopCond`, `body` | ✔ | `Parser.hs:589` |
| `UnrolledForF` | `forVar`, `forLimit`, `body` | ✘ | `Constexpr.hs:54` only |
| `StaticForF` | `forVar`, `forLimit`, `body` | ✔ | `Parser.hs:591` |
| `RangeForF` | `loopAttr`, `forVar`, `forLimit`, `body` | ✔ | `Parser.hs:584` |
| `IfF` | `ifCond`, `ifThen`, `ifElse :: Maybe a` | ✔ | `Parser.hs:597` |
| `StaticIfF` | `staticIfCond`, `staticIfThen`, `staticIfElse :: Maybe a` | ✔ | `Parser.hs:593` |
| `SwitchF` | scrutinee `a`, blocks `a` | ✔ | `Parser.hs:599` |
| `SwitchCaseF` | label `a`, body `a` | ✔ | `Parser.hs:602` |
| `SwitchDefaultF` | body `a` | ✔ | `Parser.hs:603` |
| `AssignF` | lhs `a`, rhs `a` | ✔ | `Parser.hs:614,631,636` |

#### Declarations

| Node | Fields | Parsed | Built at |
|---|---|---|---|
| `NestedScopeF` | `declScopeName`, `declName`, `body` | ✔ | `Parser.hs:467` (`nestedScope`), `:469` (`localNestedScope`) |
| `ModuleIdentifierF` | `[String]` | ✔ | `Parser.hs:191` |
| `ModuleDiffF` | `[String]`, `[String]` | ✔ | `Parser.hs:192` |
| `ModuleDeclF` | `declName`, `exposed :: [a]`, `body` | ✔ | `Parser.hs:189` |
| `EnumConstantF` | `declScopeName`, `declName`, `constValue :: Maybe a` | ✔ | `Parser.hs:290` |
| `EnumF` | `declScopeName`, `declName`, `enumType`, `body` | ✔ | `Parser.hs:284` |
| `StructF` | `declScopeName`, `declName`, `body` | ✔ | `Parser.hs:299` |
| `UnionF` | `declScopeName`, `declName`, `body` | ✔ | `Parser.hs:305` |
| `ClassF` | `declScopeName`, `declName`, `body` | ✔ | `Parser.hs:338` |
| `DefaultInitializationF` | `a` | ✔ | `Parser.hs:314` |
| `PrivateF` | — | ✔ | `Parser.hs:342` |
| `PublicF` | — | ✔ | `Parser.hs:343` |
| `TemplateF` | `templateParams :: [a]`, `templateDecl` | ✔ | `Parser.hs:375`, `:370` (template-template param kind) |
| `DecltypeF` | `declScopeName`, `declExp` | ✔ | `Parser.hs:432` |
| `AliasF` | `declScopeName`, `body` (= the type), `declName` | ✔ | `Parser.hs:276` |
| `CaptureF` | `declScopeName`, `declName` | ✔ | `Parser.hs:447` |
| `VariableF` | `declScopeName`, `varAttr`, `varType`, `declName`, `varInit :: Maybe a`, `declFlags :: DeclareFlag` | ✔ | `Parser.hs:449` (member), `:451` (loop induction const), `:455` (all others) |
| `FuncParamF` | `declScopeName`, `paramAttr`, `paramType`, `declName` | ✔ | `Parser.hs:379` |
| `FunctionF` | `declScopeName`, `funcKind`, `funcAttr`, `funcModifier :: Maybe a`, `funcReturnType`, `declName`, `funcParams`, `body` | ✔ | `Parser.hs:422` |
| `FunctionDeclF` | `declScopeName`, `funcAttr`, `funcReturnType`, `declName`, `funcParams` | ✔ | `Parser.hs:421` |
| `LambdaF` | `declScopeName`, `declName`, `lambdaCaptures`, `funcParams`, `funcReturnType`, `body` | ✔ | `Parser.hs:444` |
| `ExportTypeF` | attr `a`, type `a` | ✔ | `Parser.hs:278` |
| `ExternF` | attr `a`, type `a` | ✔ | `Parser.hs:280` |
| `StaticAssertF` | `a` | ✔ | `Parser.hs:573` (`static assert`) |
| `StaticAssertLegacyF` | `a` | ✔ | `Parser.hs:572` (`static_assert`) |
| `WarningF` | `WarningKind`, `e`, `a` | ✔ (indirect) | `Parser.hs:239` via `Error.hs:142-145` |
| `ErrorF` | `e` | ✘ | `Error.hs:139-140` (`desugarError`), used post-parse |

**Nodes with no source syntax at all: `ReferenceF`, `ThisF`, `EnumValueF`,
`UnrolledForF`, `ErrorF`** (verified by grepping each identifier in `Parser.hs`:
zero occurrences).

### 2.3 Classification predicates — `Parser/Syntax.hs:675-780`

The parser branches on these; a Rust port must replicate them for any symbol-table-driven
decision.

* `isType` (`:676-699`) — true for `AutoF VoidF BooleanF FloatF StringF ConstF ReferenceF
  IntegerF UnsignedF ParamIntF ParamUintF EnumF StructF ClassF UnionF AliasF
  TypeIdentifierF ArrayF FunctionTypeF`; recursive through `TemplateF` and
  `TemplateInstanceF`; **inverted** through `TypeParamF`/`TemplateParamF`
  (`isType (TypeParamF _ a _ _) = not (isType a)` — a `typename T` parameter has
  kind `TypenameF`, which is not a type, hence the parameter *is* a type).
* `isTemplate` (`:701-704`) — `TemplateF`, or a `TemplateParamF`/`TypeParamF` whose kind
  is a template (template-template parameters).
* `isAuto` (`:706-707`), `isConst` (`:709-710`), `isDiagnostic` (`:712-714`).
* `isFunction` (`:716-720`) — `FunctionF`, `LambdaF`, `FunctionDeclF`, or a `TemplateF`
  wrapping one.
* `isAbbreviatedTemplate` (`:722-724`) — any `FuncParamF` with `AutoF` type.
* `getName` / `getQualifier` / `getScope` (`:726-780`) — name extraction used by
  `parseSymbol` and `findSymbol`.

---

## 3. Grammar — EBNF

Notation: `"lit"` terminal; `X?` optional; `X*` zero-or-more; `X+` one-or-more;
`(…)` grouping; `|` ordered choice (**PEG-style: first match wins**, since the reference
is a backtracking parser-combinator);
`try(X)` marks a point where the reference wraps `X` in `try` — i.e. unbounded
backtracking is required there;
`⟦…⟧` marks a **symbol-table-dependent** decision (see §6).

`SEQ<X>` = the list wrapper `many/sepBy/sepBy1/sepEndBy` that produces a `SeqF` node
(§0.2).

### 3.1 Entry point

```ebnf
(* Parser.hs:106-108 *)
file          = whitespace program EOF ;

(* Parser.hs:110-111 *)
program       = ( moduleDecl | declarations ) ;
```
`program` additionally runs two post-passes on the result:
`checkExposed` (`Parser.hs:208-231`) and `checkRedundantImports` (`Parser.hs:234-241`).
`checkRedundantImports` prepends a `SeqF` of `WarningF RedundantImport` nodes to the
top-level `SeqF` (`Parser.hs:237-239`).

### 3.2 Modules and imports

```ebnf
(* Parser.hs:185-192 *)
moduleDecl        = moduleIdentifier
                    "{" ( exposedItem ( "," exposedItem )* )? "}"
                    declarations ;                        (* body parsed in scope named by moduleIdentifier *)

exposedItem       = moduleDiff | moduleIdentifier | identifier ;

(* Parser.hs:191 — wrapped in try *)
moduleIdentifier  = try( "module" moduleName ) ;

(* Parser.hs:192 — wrapped in try *)
moduleDiff        = try( "module" moduleName "\\" moduleName ) ;

(* Parser.hs:179-183 *)
moduleName        = ".cmdargs" | ".options" | dotSepName ;

(* Parser.hs:194-202 *)
declarations      = importDecl* SEQ<decl> ;

importDecl        = "import" moduleName ( "as" name )? ;
```

Notes:
* `moduleName`'s first two alternatives are the two synthetic modules
  (`Parser.hs:41-45`, generated by `Parser/IO.hs:49-65`): `.cmdargs` carries `-D` defines
  and `-u` usings, `.options` carries backend/stall/optimize/max_threads settings.
* Imports may appear **only** at the start of a declaration list
  (`declarations = parseImports >> many decl`, `Parser.hs:243`) — including the start of
  a module body — never interleaved with declarations.
* An import is executed eagerly: the imported file is parsed through the `Cache`
  (`Parser.hs:198-202`), and mismatched module names are a hard error (`:205-206`).
* No terminating `;` on `import` or `module`.
* `moduleDecl`'s result is `SeqF [ ModuleDeclF … ]` (`Parser.hs:189`).

### 3.3 Top-level and common declarations

```ebnf
(* Parser.hs:245-251 *)
decl        = ( try( globalVariable ";" )
              | staticIf<decl>
              | commonDecl
              | function<FreeFunction>
              | functionTemplate<FreeFunction>
              ) ";"* ;

(* Parser.hs:253-266 *)
commonDecl  = ( try( exportType )
              | try( extern )
              | alias
              | enum
              | struct
              | union
              | staticAssert
              | try( structTemplate )
              | try( unionTemplate )
              | try( aliasTemplate )
              | try( classTemplate )
              | class
              ) ";"* ;
```
The trailing `";"*` is `hidden (skipMany semi)` in both — stray semicolons after any
declaration are silently accepted.

```ebnf
(* Parser.hs:278 *)
exportType   = attributeSeq<[ name(str) ]> "export" ( try(templateSpecifier) | type ) ";" ;

(* Parser.hs:280 *)
extern       = attributeSeq<[ name(str) ]> "extern" ( try(templateSpecifier) | typeSpecifier ) ";" ;

(* Parser.hs:270-276 *)
alias        = "using" identifier "=" type ";" ;
aliasTemplate= templateHeader alias ;                 (* Parser.hs:272, via `template` *)

(* Parser.hs:282-292 *)
enum         = "enum" identifier ":" type "{" enumConstList "}" ;
enumConstList= enumConst ( "," enumConst )* ;         (* at least one; Parser.hs:289-292 *)
enumConst    = identifier ( "=" expression )? ;

(* Parser.hs:294-299 *)
struct        = "struct" identifier "{" ( memberVariable ( ";" memberVariable )* ";"? )? "}" ;
structTemplate= templateHeader struct ;

(* Parser.hs:301-305 *)
union         = "union" identifier "{" ( memberVariable ( ";" memberVariable )* ";"? )? "}" ;
unionTemplate = templateHeader union ;

(* Parser.hs:571-574 *)
staticAssert = "static_assert" "(" expression ")" ";"
             | "static" "assert" "(" expression ")" ";" ;   (* keyword2, Parser.hs:573 *)
```

Notes:
* `exportType` is registered in the symbol table under a generated name
  `$export<N>` (`SymbolTable.hs:213`); `extern` is **not** registered (no `symbol`
  wrapper at `Parser.hs:280`).
* `enum` requires the `":" type` underlying-type clause (`Parser.hs:284`); it is not
  optional. Body braces are mandatory and must contain ≥1 constant (`Parser.hs:289-292`).
* `struct`/`union` bodies use `sepEndBy semi` (`Parser.hs:299,305`) so a trailing `;`
  is allowed and an empty body `{}` is allowed. Members are `memberVariable` **only** —
  no functions, no nested types, no attributes (see `uninitVar`, `Parser.hs:449,465`).
* Each enum constant is visible to the following ones
  (`unscopedWith [x] (comma >> …)`, `Parser.hs:291`).

### 3.4 Classes

```ebnf
(* Parser.hs:331-339 *)
class        = "class" identifier "{" SEQ<memberDecl> "}" ;

(* Parser.hs:341-350 *)
memberDecl   = "private" ":"
             | "public" ":"
             | staticIf<memberDecl>
             | commonDecl
             | defaultInitialization
             | try( classVariable ";" )
             | function<MemberFunction>
             | functionTemplate<MemberFunction> ;

(* Parser.hs:314 *)
defaultInitialization = "default" "=" expression ";" ;

(* Parser.hs:462 *)
classVariable = attributeSeq<[ name(str) ]> type identifier ( "=" expression )? ;

(* Parser.hs:323-329 *)
classTemplate = templateHeader class ;
```

The **three-pass** class parse (this is grammar-relevant because it changes which names
resolve):

1. `class_` (`Parser.hs:316-321`): `lookAheadSkippingMemberBody (class' [] [])` — a
   speculative parse with `Config.skipMemberBody = True` (`Parser.hs:307`). In that mode
   member-function bodies become `Seq [] <$ L.skipBraces` (`Parser.hs:422-424`) and
   variable initializers are skipped textually (`Parser.hs:455-459`). From the resulting
   `ClassF` the parser keeps the class itself plus every member satisfying
   `isTemplate || isType || isVariable` (`Parser.hs:319`), and re-parses the class for
   real with those pre-declared (`unscopedWith symbols`, `Parser.hs:335`).
2. `classTemplate` (`Parser.hs:323-329`) inserts one more pass first:
   `lookAheadSkippingClassBody (template (class' []))` with `Config.skipClassBody = True`
   (`Parser.hs:309`), which skips the *entire* class body (`Parser.hs:337-338`) and
   yields just the `template<…> class X` header, so that the class template can refer to
   itself inside its own body.
3. Real parse.

Effect: **members are visible throughout the class body regardless of declaration order**
(C++ "complete-class context"), for types, templates and variables — but **not** for
member functions (they are not in the `isTemplate || isType || isVariable` filter, so a
non-template member function used before its declaration resolves to `Nothing`).

### 3.5 Templates

```ebnf
(* Parser.hs:373-375 *)
templateHeader = templateParameters ;                       (* then the wrapped decl *)

(* Parser.hs:352-359 *)
templateParameters = "template" "<" templateParam ( "," templateParam )* ">" ;

(* Parser.hs:362-368 — ordered choice *)
templateParam  = typenameParam | templateTemplateParam | nonTypeParam ;

typenameParam        = "typename" name ( "=" type )? ;                   (* :366 *)
templateTemplateParam= "template" "<" ( paramKind ( "," paramKind )* )? ">" "typename"
                       name ( "=" templateSpecifier )? ;                 (* :368,370 *)
paramKind            = "typename" | type ;                               (* :371 *)
nonTypeParam         = type name ( "=" expressionTemplate )? ;           (* :367 *)
```

* `templateParameters` requires **at least one** parameter (`paramList`, `Parser.hs:361-364`);
  `template<>` is a syntax error.
* Duplicate parameter names are rejected with a hard `fail` (`Parser.hs:356-359`).
* Each parameter is in scope for the following ones (`Parser.hs:363`), enabling
  `template<typename T, T Value>`.
* `templateTemplateParam`'s inner list uses `MP.sepBy` (`Parser.hs:370`) so
  `template<> typename TT` is accepted there (unlike a real `templateParameters`).
* Index `i` is recorded in `TemplateParamF … Int …` (`Parser.hs:361-368`).

### 3.6 Functions

```ebnf
(* Parser.hs:406-424 *)
functionDef  = funcAttr funcModifier? type identifier funcParams
               ( ";"                          (* → FunctionDeclF; MemberFunction & no modifier only *)
               | "{" SEQ<statementOrDecl> "}"  (* → FunctionF *)
               ) ;

(* Parser.hs:399-404 *)
funcModifier = "inline" | "noinline" ;

(* Parser.hs:394-397 *)
funcAttr     = attributeSeq<[ async, no_backpressure, pipelined, unordered,
                              latency(e), max_threads(e), thread_rate(e),
                              reset, pure ]>? ;

(* Parser.hs:377-382 *)
funcParams   = "(" ( funcParam ( "," funcParam )* )? ")" ;
funcParam    = attributeSeq<[ last ]>? type identifier ;

(* Parser.hs:426 *)
function         = functionDef ;
(* Parser.hs:428-430 *)
functionTemplate = templateHeader functionDef ;
```

* The `";"` (prototype) alternative is taken **only** when
  `isNothing modifier && kind == MemberFunction` (`Parser.hs:418-419`). A free function
  or an `inline`/`noinline` member function must have a body.
* `functionTemplate` does a look-ahead pass first (`Parser.hs:429`) so a function
  template can call itself recursively; plain `function` does not (`Parser.hs:426`),
  so a recursive non-template function's own name is unresolved at parse time — harmless,
  since unresolved names simply stay `ScopedNameF` (see §6).
* The body is parsed in a scope named by the function identifier and then a nested
  `"$"` scope (`scoped ident`, `functionScoped`, `Parser.hs:415,424,127-129`).

### 3.7 Variables

```ebnf
(* Parser.hs:453-459 *)
variable        = attr type identifier ( "=" expression )? ;

(* Parser.hs:461-465 *)
globalVariable  = variable          with attr = ε,               flags = DeclareGlobal
classVariable   = variable          with attr = [[ name("…") ]]?, flags = DeclareClass
localVariable   = variable          with attr = ε,               flags = DeclareLocal
staticVariable  = "static" variable with attr = ε,               flags = DeclareStatic
memberVariable  = type identifier                                 flags = DeclareMember   (* Parser.hs:449 *)

(* Parser.hs:451 *)
uninitConst     = "const" type identifier                          flags = DeclareUninitConst
```

* `memberVariable` (`uninitVar`, `Parser.hs:449`) has **no attribute slot and no
  initializer** — struct/union fields cannot be initialized in place and cannot carry
  `[[…]]`.
* `uninitConst` is the induction variable of `for` and `static for` only
  (`Parser.hs:584,591`); the `const` keyword is mandatory there and the whole thing wraps
  the parsed `type` in a `ConstF` node (`Parser.hs:451`).
* When `Config.skipMemberBody` is set, the initializer is parsed only if it is
  immediately followed by `;`; otherwise everything up to the next `;` is skipped raw
  (`skipInit = equal <* takeWhileP Nothing (/= ';')`, `Parser.hs:456-459`).

### 3.8 Types

```ebnf
(* Parser.hs:515-557.  `typ` = typ' True, `typ' False` used only inside expressions *)
type          = "void"
              | "const" type1
              | type1 ;                                            (* Parser.hs:517-521 *)

type1         = "auto"                                             (* :528 *)
              | try( memoryAttr ) type3 "[" expression "]"         (* :529 — exactly ONE dim *)
              | functionType                                       (* :530 *)
              | type2 ;                                            (* :531 *)

type2         = type3 ( "[" expression "]" )* ;                    (* :533-538 *)

type3         = "bool"                                             (* :540 *)
              | "float32"                                          (* :541 *)
              | try( "int"  intSuffix )                            (* :542 — no space allowed *)
              | try( "uint" intSuffix )                            (* :543 *)
              | "int"  "<" expressionTemplate ">"                  (* :544 *)
              | "uint" "<" expressionTemplate ">"                  (* :545 *)
              | ⟦typeSpecifier⟧                                    (* :546 *)
              | "typename" ⟦typeSpecifier(allowUnresolved=True)⟧   (* :547 *)
              | decltype                                           (* :548 *)
              | "string" ;                                         (* :549 *)

(* Parser.hs:551-557 *)
functionType  = attributeSeq<funcTypeAttr>? "(" ( ftParam ( "," ftParam )* )? ")"
                "->" ftReturn ;
ftParam       = attributeSeq<[ last ]>? ftParamType identifier? ;
ftParamType   = "const" type2 | "auto" | "string" | type2 ;         (* :554 *)
ftReturn      = "void" | "auto" | type2 ;                           (* :557 *)

(* Parser.hs:432 *)
decltype      = "decltype" "(" expression ")" ;

(* Parser.hs:513 *)
typename      = "typename" ;

(* Parser.hs:505-511 *)
memoryAttr    = attributeSeq<[ initialize, memory, non_replicated, quad_port, ecc(id) ]> ;
```

Type-syntax facts:

1. **`int5` vs `int<5>`** — `try (Integer <$ string "int" <*> intSuffix)` uses raw
   `string "int"` followed immediately by `intSuffix = lexeme (L.decimal <* notFollowedBy (alphaNumChar|'_'))`
   (`Lexer.hs:258-259`). So `int 5` (with a space) is **not** `int5`; and `int5x` is not a
   type (nor an identifier — `name` rejects it only when the suffix parses as a `Word`,
   `Lexer.hs:241-243`, so `int5x` is a legal *identifier*).
2. **Array dimension order.** `type2` collects dimensions left-to-right into a single
   `SeqF` (`Parser.hs:533-538`) producing `ArrayF (Seq []) elem (Seq dims)`. Aliases
   compose by appending: with `using t = uint32[10];`, `t[4][2]` is `uint32[4][2][10]`
   (verified in `test/syntax/memory.k:325-341`).
3. **A `[[…]]`-attributed array accepts exactly one dimension** (`Parser.hs:529`
   uses `Seq . pure <$> brackets expression`, singular). Confirmed against the
   oracle 2026-09-20 — see *Open questions* Q1, which also records the one
   place this port parses more than the reference does: pass 8's twin, whose
   substitutions can build the shape the grammar cannot spell
   (`known-divergences.md` #31).
4. `const` may only be applied at the outermost level (`Parser.hs:519`) and inside
   `ftParamType` (`:554`) and `uninitConst` (`:451`).
5. `functionType` cannot appear as a function-type parameter type or return type
   (`ftParamType`/`ftReturn` route through `type2`, which excludes `functionType`), so
   `((uint32) -> void) -> void` does not parse — matching `test/syntax/function-type.k:4-16`
   (`expected:1`).
6. Function-type parameter *names* are optional and, when present, are registered as
   symbols in a fresh quasi-scope (`quasiScoped`, `symbol identifier`, `Parser.hs:551,553`),
   which is how duplicate parameter names are diagnosed
   (`test/syntax/function-type.k:56-62`).
7. `typename X` (`Parser.hs:547`) forces `X` to be treated as a type **even when
   unresolved** — the C++ `typename` disambiguator.
8. `⟦typeSpecifier⟧` is where the symbol table decides whether an identifier is a type
   (§6, site S2).

### 3.9 Statements

```ebnf
(* Parser.hs:471-479 *)
statementOrDecl = ( commonDecl
                  | try( functionTemplate<FreeFunction> )
                  | statement
                  | try( "static" defaultInitialization )
                  | staticVariable ";"
                  | try( localVariable ";" )
                  | function<FreeFunction>
                  ) ";"* ;

(* Parser.hs:481-493 *)
statement       = ";"                              (* → SeqF []  *)
                | "barrier" ";"
                | "reorder" statement
                | atomic
                | annotatedStmt
                | returnStmt
                | incDecStmt ";"
                | assignStmt ";"
                | nestedBlock
                | try( expression ";" )
                | controlFlow ;

(* Parser.hs:559-561 *)
atomic          = "atomic" statement ;             (* → AnnotatedF (Seq [FlagAttr Atomic]) stmt *)

(* Parser.hs:563-567 *)
annotatedStmt   = try( attributeSeq<[ schedule(e) ]> ) statement ;

(* Parser.hs:569 *)
returnStmt      = "return" expression? ";" ;

(* Parser.hs:469 *)
nestedBlock     = "{" SEQ<statementOrDecl> "}" ;   (* → NestedScopeF, localScoped *)

(* Parser.hs:497-503 *)
controlFlow     = loop | staticFor | staticIf<statementOrDecl> | ifStmt | switch ;

(* Parser.hs:580-589 *)
loop            = loopAttr ( rangeFor | doWhile ) ;
loopAttr        = attributeSeq<[ unordered, reorder_by_looping, fifo_depth(e) ]>? ;
rangeFor        = "for" "(" uninitConst ":" expression ")" statement ;
doWhile         = "do" statement "while" "(" expression ")" ;

(* Parser.hs:591 *)
staticFor       = try( "static" "for" ) "(" uninitConst ":" expression ")" statement ;

(* Parser.hs:593-595 *)
staticIf<P>     = try( "static" "if" ) "(" expression ")" body<P> ( "else" body<P> )? ;
body<P>         = "{" SEQ<P> "}"        (* → NestedScopeF, Parser.hs:467 *)
                | P ;

(* Parser.hs:597 *)
ifStmt          = "if" "(" expression ")" statement ( "else" statement )? ;

(* Parser.hs:599-605 *)
switch          = "switch" "(" expression ")" "{" SEQ<switchBlock> "}" ;
switchBlock     = "case" expression ":" SEQ<statementOrDecl> "break" ";"
                | "default" ":" SEQ<statementOrDecl> "break" ";"
                | staticIf<switchBlock> ;

(* Parser.hs:607-614 *)
incDecStmt      = ( "--" lvalue | try( lvalue "--" ) )     (* → Assign l (Binary Sub l 1) *)
                | ( "++" lvalue | try( lvalue "++" ) ) ;   (* → Assign l (Binary Add l 1) *)

(* Parser.hs:616-638 *)
assignStmt      = try( lvalue "=" expression )
                | try( lvalue "+="  expression )
                | try( lvalue "-="  expression )
                | try( lvalue "*="  expression )
                | try( lvalue "/="  expression )
                | try( lvalue "%="  expression )
                | try( lvalue "&="  expression )
                | try( lvalue "|="  expression )
                | try( lvalue "^="  expression )
                | try( lvalue "&&=" expression )
                | try( lvalue "||=" expression )
                | try( lvalue "^^=" expression ) ;
```

Statement facts:

* **There is no `<<=` or `>>=`.** The list at `Parser.hs:617-629` is complete.
  (`tools/editors/vscode/GRAMMAR.md:524-525` claims otherwise — that document is a
  hand-written prompt, not ground truth.)
* **There is no `continue`** and `break` exists **only** as the mandatory terminator of a
  `case`/`default` block (`Parser.hs:602-603`). `switch` blocks *must* end with
  `break;`. (`GRAMMAR.md:635` is wrong about `continue`.)
* `if` / `else` branches are `statement`, not `statementOrDecl` — so a declaration in an
  unbraced branch is a syntax error (`test/syntax/statement.k:11-17`, `expected:1`).
  `static if` branches, by contrast, take the caller-supplied `P` (declarations, member
  declarations, statements-or-declarations, or switch blocks).
* `do … while (…)` has **no** required trailing `;` in the production
  (`Parser.hs:587-589`); a following `;` is swallowed by `statementOrDecl`'s `";"*`.
* Compound assignment desugars to `Assign lval (Binary op rval rhs)` where `rval` is
  `lval` with every `ArrayAccessLValueF` rewritten to `ArrayAccessF`
  (`Parser.hs:632-638`).
* `atomic` synthesizes the attribute list `Seq [FlagAttr Atomic]` (`Parser.hs:561`) —
  `[[atomic]]` is **not** accepted as written source anywhere.
* `reorder` takes a `statement`, so `reorder bool x;` is an error
  (`test/syntax/statement.k:4-9`).
* `statement`'s `try(expression ";")` sits **before** `localVariable` in
  `statementOrDecl` — see §8.1.

### 3.10 Expressions

```ebnf
(* Parser.hs:823-828 — checks isType and registers "Invalid use of a type as an
   expression" (non-fatal) but still returns the node *)
expression         = expr(full) ;

(* Parser.hs:830 *)
expressionTemplate = expr(template) ;

(* Parser.hs:832-835 *)
expr(mode)         = makeExprParser term ops(mode) ;

(* Parser.hs:836-854 — ordered choice *)
term = mux | lutmul | concat | fanOut | staticExpr | bitOffsetOf | byteOffsetOf
     | try( ⟦type' False⟧ )
     | "(" expr(full) ")"                        (* NB: always FULL mode, Parser.hs:845 *)
     | literal
     | cast
     | functionCall
     | methodInstanceSpecifier
     | value
     | try( designatedInitializerList )
     | initializerList
     | lambda ;

(* Parser.hs:864-872 *)
bitOffsetOf   = "bitoffsetof" "(" type "," identifier ")" ;
byteOffsetOf  = "byteoffsetof" "(" type "," identifier ")" ;
lutmul        = "lutmul" "(" expression "," expression ")" ;
concat        = "concat" "(" expression ( "," expression )* ")" ;    (* sepBy1: ≥1 arg *)
fanOut        = "fan_out" "<" expressionTemplate ">" "(" expression ")" ;
staticExpr    = "static" "(" expression ")" ;
mux           = "mux" "(" expression "," expression ( "," expression )* ")" ;
cast          = "cast" "<" type ">" "(" expression ")" ;

(* Parser.hs:871,873-877 *)
functionCall  = callModifiers try( functionSpecifier "(" ) ( expression ( "," expression )* )? ")" ;
callModifiers = attributeSeq<[ call_rate(e), fifo_depth(e), transaction_size(e) ]>? ;

(* Parser.hs:856-863 *)
initializerList            = "{" expression ( "," expression )* "}" ;   (* → PositionalF i e *)
designatedInitializerList  = "{" ( designator ( "," designator )* )? "}" ;
designator                 = "." identifier "=" expression ;

(* Parser.hs:436-447 *)
lambda        = "[" ( identifier ( "," identifier )* )? "]"
                funcParams?
                ( "->" type )?
                "{" SEQ<statementOrDecl> "}" ;
```

Notes:

* `concat` uses the local `sepBy1` (`Parser.hs:867`), so at least one argument.
* `mux(sel, e0, e1, …)` requires ≥2 arguments (`Parser.hs:870`); the cases become
  `MuxF sel (Seq [e0, e1, …])`, indexed by the selector value.
* The ternary `a ? b : c` desugars to `MuxF a (Seq [c, b])` (`Parser.hs:968`) — note the
  **reversed order**: index 0 is the *false* branch.
* `{}` is parsed by `designatedInitializerList` (which allows zero elements,
  `Parser.hs:862`), never by `initializerList` (which requires ≥1, `Parser.hs:856-861`).
  Trailing commas are **not** allowed in either (`try (comma >> …)` backtracks the comma,
  leaving it for `braces`' `}` which then fails — `Parser.hs:860`).
* `designator` matches `.` with raw `char '.'` (`Parser.hs:863`), not the whitespace-eating
  `dot`.
* Lambda captures are bare identifiers only — no `&`, no `=`, no `this`
  (`capture = Capture <$> … <*> identifier`, `Parser.hs:447`). Parameter list and
  `-> type` are both optional; the default return type is `AutoF` (`Parser.hs:439,443`).
* `"(" expr(full) ")"` at `Parser.hs:845` deliberately re-enters **full** operator mode,
  which is the escape hatch for using `<`/`>`/`<<`/`>>` inside template arguments
  (comment at `Parser.hs:910-912`).

### 3.11 Names, paths and specifiers

```ebnf
(* Parser.hs:268 *)
identifier            = name ;                                  (* → IdentifierF *)

(* Parser.hs:646 *)
templateQualifier     = try( "template" ¬"<" ) ;

(* Parser.hs:648-695 *)
scopedName            = pathSegment ( "::" pathSegment )* ;
pathSegment           = templateQualifier? identifier ⟦templateArgs?⟧ ;

(* Parser.hs:697 *)
templateArgs          = "<" ( templateArg ( "," templateArg )* )? ">" ;
templateArg           = try( templateSpecifier ) | expressionTemplate ;

(* Parser.hs:640-644 *)
qualifiedIdentifier   = scopedName ;                            (* → QualifiedIdentifierF *)

(* Parser.hs:702-708 *)
templateSpecifier     = ⟦scopedName that resolves to a template and is not instantiated⟧ ;

(* Parser.hs:710-726 *)
typeSpecifier         = ⟦scopedName that resolves to a type⟧ ;

(* Parser.hs:728-734 *)
functionSpecifier     = methodSpecifier | freeFunctionSpecifier ;
methodSpecifier       = try( lvalue "." ) ⟦scopedName resolved against lvalue's type⟧ ;
freeFunctionSpecifier = ⟦qualifiedIdentifier, with const-alias substitution⟧ ;

(* Parser.hs:766-770 *)
methodInstanceSpecifier = methodSpecifier ;   (* diagnoses if the name is not a TemplateInstance *)

(* Parser.hs:783-819 *)
value      = ⟦namedValue⟧   accessChain<ArrayAccessF> ;
lvalue     = ⟦namedLValue⟧  accessChain<ArrayAccessLValueF> ;
accessChain<K> = ( "[" expression "]"                                     (* → K *)
                 | try( "." identifier ¬"(" ¬templateArgs ) )* ;          (* → MemberAccessF *)
```

* `accessChain`'s member-access branch is guarded by `notFollowedBy lparen` and
  `notFollowedBy templateArgs` (`Parser.hs:814`) so that `obj.f(…)` and `obj.f<T>…`
  fall through to `functionCall` / `methodInstanceSpecifier` instead.
* `templateQualifier` uses `L.symbol "template"` — a bare prefix match — plus
  `notFollowedBy langle` to distinguish `obj.template Go<T>` from a `template<…>` header
  (`Parser.hs:646`; usage `test/syntax/dependent-template.k:121`).

### 3.12 Operator precedence — `Parser.hs:913-968`

`makeExprParser` takes the table **tightest first**.

| Lvl | Operators | Fixity | Table entry | Present in `expressionTemplate`? |
|---|---|---|---|---|
| 1 | `-` (Neg) | prefix | `:922` | yes |
| 2 | `!` (Not), `~` (Invert) | prefix | `:924-925` | yes |
| 3 | `bitsizeof`, `bytesizeof`, `clog2` | prefix (keyword) | `:927-929` | yes |
| 4 | `*` `/` `%` | infixL | `:931-933` | yes |
| 5 | `+` `-` | infixL | `:935-936` | yes |
| 6 | `>>` `<<` | infixL | `:918-919` via `shiftOps` | **no** (`opsTemplate = ops' [] []`, `:913`) |
| 7 | `<=` `>=` | infixN | `:939-940` | yes |
| 7 | `>` `<` | infixN | `:915-916` via `relOps` | **no** |
| 8 | `==` `!=` | infixN | `:944-945` | yes |
| 9 | `&` | infixL | `:947` | yes |
| 10 | `^` | infixL | `:949` | yes |
| 11 | `\|` | infixL | `:951` | yes |
| 12 | `&&` | infixL | `:953` | yes |
| 13 | `^^` | infixL | `:955` | yes |
| 14 | `\|\|` | infixL | `:957` | yes |
| 15 | `? :` | ternary, right-assoc (`TernR`) | `:959`, `:967-968` | yes |

Prefix-operator quirks that follow from the level assignment (`Control.Monad.Combinators.Expr`
builds level *n* from level *n−1*):

* `-` binds **tighter** than `!`/`~`, which bind tighter than `bitsizeof`. Hence
  `bitsizeof -x` parses, but `-bitsizeof x` and `!bitsizeof x` **do not** — parens are
  required. **UNVERIFIED** by execution; derived from the table order at
  `Parser.hs:921-930` and `makeExprParser` semantics.
* Two prefix operators at the same level cannot stack. `!` and `~` are **both** level 2
  (`Parser.hs:923-926`), so `!~x` and `~!x` fail along with `!!x`, `~~x` and `--x` — but
  for two *different* reasons, which is worth keeping straight when writing a lexer:
  `!!x` and `~~x` die in the **lexer**, because `operator o` is
  `try (string o *> notFollowedBy (oneOf (o ++ "=")))` (`Parser/Lexer.hs:262`) and so `!`
  refuses a following `!`; whereas `!~x` lexes fine and dies in the **precedence table**,
  because `makeExprParser` applies at most one `Prefix` per level.
  **VERIFIED** against `kg-parser`, which rejects `!~a`, `~!a` and `-~a` while accepting
  `!-a` and `~-a` — exactly the "looser level onto a tighter one, once" rule.
  (An earlier revision of this section asserted "`!~x` works", contradicting its own
  opening sentence; `kg-parser`'s `prefix_operators` test pins the corrected behaviour.)
* `clog2` is a prefix operator, not a call: `clog2(x)` is `clog2` applied to the
  parenthesized term `(x)`; `clog2 x + 1` is `(clog2 x) + 1`.

### 3.13 Interpolated strings — `Parser.hs:883-903`

```ebnf
interpolatedString = '"' segment* '"' ;
segment            = rawText interpolation? ;
interpolation      = "{" expression "="? ( "," expression )? ( ":" format )? "}" ;
format             = ( "b" | "o" | "d" | "x" | "X" ) decimal? ;
```

* `rawText` is `manyTill charLiteral (lookAhead ('"' | try singleBrace))`
  (`Parser.hs:887`) where `charLiteral = char '{' *> MP.charLiteral <|> MP.charLiteral`
  (`:892`) — i.e. `{{` produces a literal `{`, and every other character goes through
  Haskell's `charLiteral` escape decoding.
* `singleBrace = char '{' <* notFollowedBy (char '{')` (`:893`) is the interpolation
  opener.
* The closing `}` is a raw `char '}'` (`:888`) — no whitespace is skipped before it, but
  `format`/`expression` are lexemes so trailing whitespace inside the braces is fine.
* `"="` after the expression sets the `Bool` in the tuple, causing the printed form to be
  prefixed with the expression text (`Parser.hs:896,899`; semantics documented at
  `tools/editors/vscode/GRAMMAR.md:327-331`).
* `","` introduces the alignment expression; `":"` the format specifier
  (`Parser.hs:897-898`).
* `format`'s specifier letters come from `Show FormatSpecifier`
  (`Parser/Syntax.hs:241-246`) via `choiceViaShow [Bin,Oct,Dec,Hex,HexUpper]`
  (`Parser.hs:900`); precision is `L.decimal` (`:900`).
* Ordering matters: `Hex="x"` is tried before `HexUpper="X"`, so `X` is unambiguous.

---

## 4. Attributes — complete enumeration

```ebnf
(* Parser.hs:163-165 *)
attributeSeq<S> = "[[" S ( sep S )* "]]" ;
sep             = "," | try( "]]" "[[" ) ;
```
`optionalAttributes S = try(attributeSeq<S>) | ε` (`Parser.hs:169`) — an attribute list
that fails to match *any* of the permitted attribute names simply yields `Seq []`, so a
misspelled or misplaced attribute produces a **downstream** error, not "unknown
attribute". (E.g. `[[assume_ordered]] do {…}` → `test/syntax/loops.k:171-181`, `expected:1`.)

Attribute names are matched with `L.symbol` (`Parser.hs:171,173,175,177`), i.e. plain
prefix matching with **no identifier-boundary check** — they are not keywords.

Argument shapes (`Parser.hs:171-177`):

| builder | shape | node |
|---|---|---|
| `flagAttr x` | `name` | `FlagAttrF Flag` |
| `intAttr x` | `name "(" expression ")"` | `IntAttrF Attribute Exp` |
| `identAttr x` | `name "(" qualifiedIdentifier ")"` | `AttrF Attribute Exp` |
| `strAttr x` | `name "(" stringLiteral ")"` | `AttrF Attribute Exp` |

### 4.1 Where each attribute is accepted

| Attribute | Shape | Permitted positions (source line of the permit-list) |
|---|---|---|
| `async` | flag | function attr (`:385` in `funcTypeAttr`), function-type attr (`:555`) |
| `no_backpressure` | flag | function attr (`:386`), function-type attr (`:555`) |
| `pipelined` | flag | function attr (`:387`), function-type attr (`:555`) |
| `unordered` | flag | function attr (`:388`), function-type attr (`:555`), loop attr (`:581`) |
| `latency(expr)` | int | function attr (`:389`), function-type attr (`:555`) |
| `max_threads(expr)` | int | function attr (`:390`), function-type attr (`:555`) |
| `thread_rate(expr)` | int | function attr (`:391`), function-type attr (`:555`) |
| `reset` | flag | function attr only (`:395`) |
| `pure` | flag | function attr only (`:396`) |
| `last` | flag | function parameter (`:381`), function-type parameter (`:556`) |
| `initialize` | flag | array type, i.e. `memoryAttr` (`:506`) |
| `memory` | flag | array type (`:507`) |
| `non_replicated` | flag | array type (`:508`) |
| `quad_port` | flag | array type (`:509`) |
| `ecc(qualifiedIdentifier)` | ident | array type (`:510`) |
| `schedule(expr)` | int | statement (`:563-565`) |
| `reorder_by_looping` | flag | loop (`:581`) |
| `fifo_depth(expr)` | int | loop (`:581`), function call (`:875`) |
| `call_rate(expr)` | int | function call (`:874`) |
| `transaction_size(expr)` | int | function call (`:876`) |
| `name("string")` | str | `export` (`:278`), `extern` (`:280`), class variable (`:462`) |
| `atomic` | flag | **never written in source** — synthesized by the `atomic` statement (`:561`) |

That is all 22 source-writable attributes plus the synthetic `atomic`; the corpus
(`library/`, `test/syntax/`, `test/logic/`) contains exactly these names plus two that
appear only in negative tests (`assume_ordered` at `test/syntax/loops.k:175`,
`always` at `test/syntax/callbacks.k:1050`, both `expected:1`).

`memoryAttr` (`Parser.hs:505`) is **not** wrapped in `optionalAttributes` — it is
`attributes` directly, hence `Parser.hs:529` guards it with `try`.

---

## 5. Symbol table — parser state

`SymbolTable { scopes :: [Scope], module_ :: Module }` (`SymbolTable.hs:51-55`).

```haskell
data Scope = Scope { scopeQualifier :: QualifiedName
                   , symbolsInScope :: HashMap Name ExpSrc
                   , lambdaIndex, scopeIndex, exportIndex :: Int }
```
(`SymbolTable.hs:58-66`.) Initial state: one scope with an empty qualifier
(`SymbolTable.hs:94`).

### 5.1 Scope-introducing combinators (`Parser.hs:119-156`)

| combinator | pushes a scope named | pops with | used by |
|---|---|---|---|
| `unscopedWith syms` (`:119`) | *nothing* (qualifier unchanged), only pre-seeds `syms` | `tail` | template params (`:363,408`), class pre-scan symbols (`:335`), alias body (`:276`), enum-constant chaining (`:291`), function self-symbol (`:414`) |
| `scopedWith syms ident` (`:123`) | `ident` | `tail` | class body (`:336`) |
| `scoped ident` (`:125`) | `ident` | `tail` | module body (`:189`), enum body (`:284`), function body (`:415`), lambda (`:440`) |
| `functionScopedWith`/`functionScoped` (`:127-129`) | `"$"` | `tail` | function bodies (`:424`), lambda bodies (`:444`) |
| `quasiScopedWith syms` / `quasiScoped` (`:136-145`) | `$quasi<i>` | keeps `lambdaIndex`, bumps parent `scopeIndex` | struct/union body (`:299,305`), function-type param list (`:551`), `static if` braced body (`:467`) |
| `localScoped` (`:147-156`) | `$quasi<i>` **and appends it to the parent's qualifier** | resets child `scopeIndex` to 0 | `{}` blocks (`:469`), `for` (`:584`), `static for` (`:591`), `switch` case/default bodies (`:602-603`) |

`getQuasiScopeIdentifier` (`Parser.hs:131-133`) reads `scopeIndex` of the innermost scope
and yields `Identifier ("$quasi" ++ show i)`.

### 5.2 Symbol registration

`symbol = parseSymbol . noted` (`Parser.hs:159`), `symbolWithDocs = parseSymbol . notedWithDocs`
(`:161`). `parseSymbol` (`SymbolTable.hs:194-214`):
1. computes the name — `getName`, except `ExportTypeF` which gets a generated
   `$export<N>` (`SymbolTable.hs:213`);
2. **rejects redefinition in the same scope** with `symbolError … "symbol redefinition"`
   (a registered, non-fatal error, `SymbolTable.hs:200-201`);
3. inserts into `symbolsInScope` **and** into the module-level `symbols` map keyed by
   `scopeQualifier ++ [name]` (`SymbolTable.hs:203-211`).

Productions wrapped in `symbol`: `alias` (`:270`), `exportType` (`:278`), `enum` (`:284`),
`struct` (`:294`), `union` (`:301`), `class` (`:321`), `template` (`:373`),
`funcParams`' parameters (`:377`), `function` (`:426`), `uninitConst` (`:451`),
`variable` (`:453`), function-type parameter names (`:553`).
`symbolWithDocs`: enum constants (`:290`).
**Not** registered: `extern` (`:280`), lambdas' own name is generated via
`makeSymbolName "?lambda"` (`Parser.hs:434`) but the `LambdaF` node is not passed through
`symbol`.

---

## 6. Where the parser consults the symbol table

### 6.1 The five `findSymbol` call sites

`findSymbol :: ExpSrc -> m (Maybe ExpSrc)` (`SymbolTable.hs:227-270`). Lookup algorithm:
* if `getQualifier e` is empty → walk `scopes` innermost-first by `getName`
  (`SymbolTable.hs:256-259`), then fall back to the module's own `symbols`
  (`lookupSymbol`, `Symbols.hs:79-87`, which itself walks *up* the scope qualifier), then
  every import's `visibleSymbols` (`SymbolTable.hs:261-273`);
* otherwise → skip scopes, search module symbols + imports directly
  (`SymbolTable.hs:243-244`);
* **more than one match ⇒ registered "symbol is ambiguous" error and `Nothing`**
  (`SymbolTable.hs:234-237`);
* the lookup also **mutates** state: it marks each import that supplied a match as
  `Needed` (`SymbolTable.hs:263-270`), which drives the redundant-import warning.

| # | Site | Purpose | What a context-free parser must do instead |
|---|---|---|---|
| **S1** | `Parser.hs:219` — `checkExposed.checkSymbols` | After the whole file is parsed, verify each name in the `module X { … }` exposed list exists and is not mutable global state | Not a parse decision. Move to a post-parse name-resolution pass; the parser just records the exposed identifiers. |
| **S2** | `Parser.hs:656` — `prefixedScopedNameAndSymbol` | **The** hot site. Called once per path segment. Decides (a) whether `<…>` following the name is a template-argument list or a `<` operator, (b) whether the name denotes a type, (c) whether it is a template parameter to be replaced by `TypeParamF`, (d) whether it is an alias whose target must be followed for the next `::` step, (e) whether an unqualified name needs its declaring scope prepended (`makeQualified`) | Parse a syntactic `Path` with an *optional, speculatively parsed* `GenericArgList` (see §7). Do **not** try to classify. Emit `Path { qualifier?, NameRef, GenericArgList? }` and resolve in a later pass. |
| **S3** | `Parser.hs:744` — `methodSpecifier.resolveTypeName` on a `ScopedNameF` | Look up an object expression (`a`, `a.b`, `a[i].b`) to find its declaring `Variable` so its *type name* can be recovered | Emit `MethodCallExpr { receiver, NameRef, GenericArgList? }` and let type inference bind the method. The `GenericArgList?` must be decided syntactically. |
| **S4** | `Parser.hs:747` — same function, fallback | If the qualified form missed, retry with the bare last component | same as S3 |
| **S5** | `Parser.hs:757` — `methodSpecifier.typeName` on a `TypeIdentifierF` | Follow `using`-alias chains to the ultimate class name so `obj.m<T>` can find `m` | same as S3 |

There are no other `findSymbol` occurrences in `Parser.hs` (verified by grep).

### 6.2 Symbol-table-dependent *decisions* built on S2

Every one of these is a syntactic branch, i.e. it changes the shape of the tree, not just
a diagnostic:

| Decision | Code | Consequence if the answer flips |
|---|---|---|
| Consume `<…>` as template args | `templateInstance`, `Parser.hs:690-695` | `a < b` becomes `a<b>` or vice versa |
| A function-template name with no `<…>` still becomes `TemplateInstanceF s (Seq [])` | `Parser.hs:694` | affects `templateSpecifier`'s `isInstance` check (`:706`) |
| `typeSpecifier'` accepts / rejects the name as a type | `Parser.hs:712-726` | `try (typ' False)` in `term` (`:843`) succeeds or fails ⇒ a statement is a type expression or a value expression |
| unresolved name is/isn't a type | `allowUnresolved` flag, `Parser.hs:546` (`False` in expressions), `:547` and `:710` (`True`) | forward-referenced types work in declarations but not in expressions |
| `templateSpecifier` requires `isTemplate && not isInstance` | `Parser.hs:702-708` | `export Foo;` vs `export Foo<int32>;` |
| TemplateParam ⇒ `TypeParamF` substitution | `Parser.hs:658-665` | `T` inside a template body becomes a distinct node kind |
| Alias transparency for the next `::` step | `Parser.hs:667-668` | `MyAlias::Nested` resolves or not |
| `makeQualified` when `not isQualified` | `Parser.hs:686-688` | the emitted `ScopedNameF` gains the symbol's declaring qualifier |
| `namedValue`: type used as an expression ⇒ error | `Parser.hs:786-787` | diagnostic only |
| `namedValue`: function-typed template parameter ⇒ `NamedValueF (TypeParamF …)`; other template parameter ⇒ `TypeParamF` | `Parser.hs:788-791` | node kind changes |
| `namedLValue`: `const` variable ⇒ "illegal use of this constant as an lvalue" | `Parser.hs:799-800` | diagnostic only |
| `freeFunctionSpecifier`: `const auto f = g;` then `f()` ⇒ **the callee node is replaced by `g`'s `NamedValueF`** | `Parser.hs:779-780` | the call target is rewritten at parse time |
| `freeFunctionSpecifier`: non-function-typed template parameter as callee ⇒ error | `Parser.hs:777-778` | diagnostic only |
| `methodInstanceSpecifier` requires the resolved name to be a `TemplateInstance` | `Parser.hs:766-770` | diagnostic only (the node is returned anyway) |

### 6.3 Other parser-state reads

`getScopeQualifier` (`SymbolTable.hs:96-97`) is read at `Parser.hs:276, 284, 287, 299,
305, 332, 353, 379, 407, 432, 437, 447, 449, 451, 455, 467, 469, 649, 703, 713, 731, 740`
and stored in the `declScopeName` / `scope` field of the resulting node. A Rust CST
parser should **not** store scope qualifiers in the tree at all — compute them in
lowering.

`gets (scopeIndex . head . scopes)` (`Parser.hs:132`) generates `$quasi<i>` names.
`gets module_` (`Parser.hs:210, 236`) is used only in the two post-passes.

### 6.4 Recommended Rust strategy

Make the parser **fully context-free** and defer every S2 decision:

1. Lex `<` and `>` as independent tokens; never produce `<<`/`>>` in a path context.
2. Parse `Path = PathSegment ('::' PathSegment)*`, `PathSegment = 'template'? NameRef GenericArgList?`,
   where `GenericArgList` is attempted with a **bounded speculative scan**: after `<`,
   scan for a balanced `<…>` whose closing `>` is followed by one of
   `( ) { } [ ] , ; :: > < . = -> identifier literal EOF` — the *follow set* observed in
   the reference for a template instantiation. On failure, treat `<` as the operator.
3. Represent "is this a type or a value" as a single syntactic node
   (`PathType` ≡ `PathExpr`) and let HIR lowering pick, exactly as rust-analyzer does.
4. Keep the `try (typ' False)`-vs-`value` ambiguity out of the grammar entirely: parse a
   *declaration-or-expression* statement uniformly (§8.1) and disambiguate by a syntactic
   predicate.
5. Reproduce the reference's diagnostics (types-as-expressions, const-as-lvalue,
   redefinition, ambiguity, template-args-missing) in a name-resolution pass, keyed to
   the same source offsets.

---

## 7. Template-argument-list disambiguation

The reference is explicit (`Parser.hs:910-912`):

```haskell
-- Disallow '>', '<', `>>` and '<<' operators in the top level expression
-- used for template parameters. They can be used within sub-expressions
-- wrapped in parenthesis (achieved via the `parens expression` term).
opsTemplate = ops' [] []                       -- Parser.hs:913
ops = ops'
    [ no_assoc Greater                         -- Parser.hs:915
    , no_assoc Less                            -- Parser.hs:916
    ]
    [ binary ShiftRight                        -- Parser.hs:918
    , binary ShiftLeft                         -- Parser.hs:919
    ]
```

`ops' relOps shiftOps` (`Parser.hs:921-961`) splices `shiftOps` in at precedence level 6
and `relOps` at level 7. Passing `[] []` removes `<`, `>`, `<<`, `>>` from the operator
table used inside `<…>`.

### 7.1 Exactly what can appear inside `<…>`

`templateArgs = angles ((try templateSpecifier <|> expressionTemplate) `sepBy` comma)`
(`Parser.hs:697`). Therefore each comma-separated argument is:

**(a)** a `templateSpecifier` — a `::`-path that resolves to an *uninstantiated* template
(`Parser.hs:702-708`); this is the template-template-argument case; **or**

**(b)** an `expressionTemplate` = `makeExprParser term opsTemplate` (`Parser.hs:835`),
whose token repertoire is:

* every `term` alternative (`Parser.hs:836-854`) — including a **type**
  (`try (typ' False)`, `:843`), which is how `Foo<uint32>` works: `uint32` is a type
  term, not an expression;
* the prefix operators `-`, `!`, `~`, `bitsizeof`, `bytesizeof`, `clog2`;
* the infix operators `*` `/` `%` `+` `-` `<=` `>=` `==` `!=` `&` `^` `|` `&&` `^^` `||`
  and the ternary `? :`;
* **not** `<` `>` `<<` `>>` at the top level of the argument.

Because `_angle` (`Lexer.hs:272-273`) only rejects a following `=`, `<=` and `>=` remain
unambiguous inside `<…>`: `Foo<a >= b>` closes at the final `>` because `>=` is lexed as
one token and `>` as another.

Nesting is unrestricted **through terms**, so all of the following legally contain
`<`/`>` inside a template argument list:

| construct | why | line |
|---|---|---|
| `Foo<(a < b)>` | `"(" expr(full) ")"` re-enables the full table | `Parser.hs:845` |
| `Foo<Bar<T>>` | inner `templateArgs` via `pathSegment` | `Parser.hs:690-697` |
| `Foo<int<8>>` | `ParamInt … angles expressionTemplate` | `Parser.hs:544` |
| `Foo<uint<N>>` | `ParamUint` | `Parser.hs:545` |
| `Foo<cast<uint8>(x)>` | `cast … angles typ …` | `Parser.hs:872` |
| `Foo<fan_out<4>(x)>` | `fanOut … angles expressionTemplate …` | `Parser.hs:868` |
| `Foo<[](){ return a < b; }>` | lambda body uses `statement` → full `expression` | `Parser.hs:444` |

### 7.2 Closing `>` is per-character

There is **no `>>` token** in the closing position: `angles` calls `rangle`
(`Lexer.hs:280, 269-273`) which matches a single `>` not followed by `=`. So `Foo<Bar<T>>`
terminates with two `rangle`s and needs no maximal-munch fixup. Conversely, `a >> b`
*is* lexed as `>>` by `operator ">>"` (`Parser.hs:962` with `pretty ShiftRight = ">>"`,
`Parser/Syntax.hs:98`) — the two are distinguished purely by which parser is running.

### 7.3 The gate: are we even in a template-argument list?

`templateArgs` is only ever attempted:

1. inside `templateInstance` (`Parser.hs:690-695`), and **only** when `findSymbol`
   said the preceding name is a template (or `templateQualifier` forced it) —
   `Parser.hs:668-680`;
2. inside `access`' negative lookahead `notFollowedBy templateArgs` (`Parser.hs:814`).

So a context-free Rust parser cannot copy the reference literally; it must use the
speculative-scan strategy of §6.4 step 2.

### 7.4 `templateQualifier`

`try (L.symbol "template" <* notFollowedBy langle)` (`Parser.hs:646`), consumed at
`Parser.hs:653`. It forces `templateInstance` even when the symbol is unresolved
(`Parser.hs:676-678`). Real usage: `obj.template Go<uint32>`
(`test/syntax/dependent-template.k:121, 144, 166`; `test/syntax/helper/dependent_template.k:27, 42`).
Note this also permits `X::template Y<…>` — **UNVERIFIED** that any corpus file uses the
`::` form.

---

## 8. Declaration-vs-expression ambiguity

Four ambiguity sites exist. In each, the reference resolves by **ordered choice +
unbounded backtracking + symbol-table classification**.

### 8.1 Statement position: `T x;` vs `expr;`

`statementOrDecl` (`Parser.hs:471-479`) orders:
```
commonDecl > try(functionTemplate) > statement > try("static" defaultInit)
           > staticVariable ";" > try(localVariable ";") > function
```
and `statement` (`:481-493`) itself ends with `try (expression <* semi)` before
`controlFlow`.

So for input `Foo x;`:
1. `statement` reaches `try (expression <* semi)`;
2. `term`'s `try (typ' False)` (`:843`) consumes `Foo` **iff `findSymbol Foo` says it is a
   type** (`typeSpecifier' False`, `:712-726`);
3. the operator parser stops at `x`;
4. `expression` (`:823-828`) notices `isType e` and **registers** "Invalid use of a type
   as an expression" — but does not fail;
5. `<* semi` fails at `x`, so `try` backtracks — *including* the registered error
   (megaparsec's `pTry` restores the whole state);
6. `controlFlow` fails; `statement` fails;
7. `staticVariable` fails (no `static`), and `try (localVariable ";")` succeeds.

For input `Foo;` (a bare type name) step 5 succeeds and the error stands.

**Rust equivalent:** parse a single `ExprOrDeclStmt`. Predicate: after parsing a
`Type`-shaped prefix (path + optional generic args + optional `[expr]` dims), if the next
token is an identifier, it is a declaration; otherwise re-interpret the prefix as an
expression. That predicate is decidable with bounded lookahead and matches the reference
on every corpus program (**UNVERIFIED** at corpus scale — see Open questions Q4).

### 8.2 Top-level: `T x;` vs everything else

`decl` (`Parser.hs:245-251`) puts `try (globalVariable <* semi)` **first**, before
`staticIf`, `commonDecl`, and functions. Because `globalVariable` = `type identifier
("=" expression)?` and functions are `funcAttr modifier? type identifier "(" …`, the
`try` backtracks on the `(` of a function's parameter list.

### 8.3 Member position

`memberDecl` (`Parser.hs:341-350`) puts `try (classVariable <* semi)` after
`commonDecl` and `defaultInitialization`, before `function MemberFunction`. Same
mechanism: a member function is distinguished from a member variable only by the `(`
after the identifier.

### 8.4 `functionCall` vs `value`

`functionCall = FunctionCall <$> callModifiers <*> try (functionSpecifier <* lparen) <*> …`
(`Parser.hs:871`) — the whole `functionSpecifier` plus the opening paren is inside `try`,
so a name followed by anything other than `(` falls through to
`methodInstanceSpecifier` (`:849`) and then `value` (`:850`).

### 8.5 `try` sites — complete list in `Parser.hs`

`:165` (`]] [[` separator), `:169` (`optionalAttributes`), `:191`, `:192` (module
identifier/diff), `:246` (global variable), `:254`, `:255`, `:261`, `:262`, `:263`,
`:264` (`commonDecl`), `:307`, `:309` (via `optional . lookAhead . try`), `:456`
(skip-body initializer), `:473`, `:475`, `:477` (`statementOrDecl`), `:492` (expression
statement), `:529` (memory attr), `:542`, `:543` (`int`/`uint` suffix forms), `:567`
(statement attr), `:591` (`static for`), `:593` (`static if`), `:617`, `:632`
(assignments), `:646` (template qualifier), `:697` (`templateArgs`' first alternative),
`:732` (`lvalue <* dot`), `:814` (member access), `:843` (type-in-expression), `:851`
(designated initializer), `:860` (initializer-list comma), `:879` (float), `:887`, `:893`
(interpolation), plus `incDecStmt`'s `:613`.

---

## 9. Traps a Rust implementer would otherwise miss

1. **`>` is never `>>` when closing a template list** — `Lexer.hs:269-273`. Do not lex a
   `>>` token in path context. Conversely `operator ">>"` at `Parser.hs:962` *does*
   require a real `>>`.
2. **`x--y` does not parse.** `operator "-"` forbids a following `-`
   (`Lexer.hs:262`); `x - -y` needs whitespace.
3. **Body skipping is a dumb brace counter.** `skipBraces = L.skipBlockCommentNested "{" "}"`
   (`Lexer.hs:121-122`) counts braces without understanding string literals or comments,
   and `skipInit = equal <* takeWhileP Nothing (/= ';')` (`Parser.hs:459`) scans to the
   first `;` regardless of nesting or strings. A `}` inside a string literal in a skipped
   member body makes the look-ahead pass fail; `optional` (`Parser.hs:307-309`) then
   yields `Nothing` and the class is parsed with **no pre-declared member symbols**,
   silently changing name resolution.
4. **Attribute lists never report "unknown attribute".** `optionalAttributes`
   (`Parser.hs:169`) swallows the failure and returns `Seq []`; the following construct
   then fails to parse at the `[`, producing a confusing error.
5. **`[[a]] [[b]]` is the same as `[[a, b]]`** — the separator is `comma <|> try ("]]" "[[")`
   (`Parser.hs:165`).
6. **Attribute names are matched by prefix**, not as keywords (`L.symbol`,
   `Parser.hs:171-177`). `[[memoryfoo]]` matches `memory` then fails on the separator.
7. **`{}` is a *designated* initializer list**, not a positional one
   (`Parser.hs:851, 856, 862`). `InitializerListF (Seq [])`.
8. **`a ? b : c` becomes `Mux a (Seq [c, b])`** — arms reversed (`Parser.hs:968`).
9. **`switch` cases must end in `break;`** and `break` exists nowhere else
   (`Parser.hs:602-603`). There is no `continue`.
10. **No `<<=` / `>>=`** (`Parser.hs:616-629`).
11. **`enum` requires `: underlying-type`** (`Parser.hs:284`).
12. **Struct/union members are bare `type name`** — no initializer, no attributes
    (`Parser.hs:449, 299, 305`).
13. **`extern` is not registered as a symbol; `export` is, under `$export<N>`**
    (`Parser.hs:278, 280`; `SymbolTable.hs:213`).
14. **`const auto f = someFn; f();` rewrites the callee at parse time** to `someFn`
    (`Parser.hs:779-780`).
15. **`bitsizeof`/`bytesizeof`/`clog2` are prefix operators at a *looser* level than
    `!`/`~`/`-`** (`Parser.hs:921-930`) — `-bitsizeof x` does not parse.
16. **Doc comments change the lexer.** With `parseDocs = True`, `//|` and `//<` comments
    are *not* whitespace (`Lexer.hs:105-119`) and must be consumed by `docCommentPre` /
    `docCommentPost` at exactly the positions `notedWithDocs` is used
    (`Parser.hs:91-96, 114, 115, 117, 161, 187, 191, 192, 290, 362`).
17. **Only `notedWithDocs` positions can carry docs.** `sepBy1` (`Parser.hs:116`) uses
    plain `noted`, so e.g. `mux` arguments and `concat` arguments can never be documented.
18. **Block comments nest** (`L.skipBlockCommentNested`, `Lexer.hs:103, 108`).
19. **Integer literals allow `_` only after the first digit run**
    (`Lexer.hs:147-149`), but the radix prefix may be followed by one `_`
    (`Lexer.hs:136`): `0x_1234_abcd` is legal, `_1` is not a literal.
20. **`int07` is not an identifier** but `int5x` is (`Lexer.hs:241-243`).
21. **`float` is 32-bit** (`FloatLiteralF Float`, `Parser/Syntax.hs:352`) and the
    keyword is `float32` (`Parser.hs:541`); `float` alone is not a keyword.
22. **`sepEndBy` in struct/union bodies means an empty body and a trailing `;` are both
    legal** (`Parser.hs:299, 305`).
23. **Template argument lists may be empty**: `templateArgs` uses `sepBy`
    (`Parser.hs:697`), so `Foo<>` parses; but `templateParameters` uses `paramList`
    (`Parser.hs:361`) which requires ≥1, so `template<>` does not.
24. **`Config` is threaded through `ReaderT` and is part of the grammar**: `skipMemberBody`
    and `skipClassBody` change what `functionDef` and `class'` accept
    (`Parser.hs:413, 334, 422-424, 337-338`).

---

## 10. Open questions

Each entry names the probe program that settles it. `kanagawa` here means the reference
compiler binary built from `compiler/`.

**Q1 — Multi-dimensional attributed arrays.** `Parser.hs:529` builds the dimension list
with `Seq . pure <$> brackets expression` — exactly one dimension — while the unattributed
form (`Parser.hs:533-538`) allows any number. Does `[[memory]] uint32[512][4] m;` parse?
Probe:
```
class Main { [[memory]] uint32[512][4] _mem;
public: void main() { println(_mem[0][0]); } }
export Main;
```
Expected from reading the source: a syntax error at the second `[`. No corpus file
exercises it (grep for `\]\]\s*\w+\[[^]]*\]\[` in `library/` and `test/` finds none).

**Answered 2026-09-20, against the oracle rather than by reading.** `kg-oracle --errors`
on `using C = [[memory]] uint32[16][10];` reports `unexpected '['` at column 32 — the
second `[`, exactly as the source predicted, and the same token this parser rejects it at.

**And pass 8's twin needs the shape anyway.** `template <typename T, auto N> using
memory_ecc = [[memory, ecc(make_ecc<T>)]] T[N];` with `T = uint32[10]` *is* an attributed
array of an array, and the reference reaches it without spelling anything —
`instantiateTemplates` substitutes into a tree. This port prints the twin and parses it
back, so `kg_syntax::parse_twin` takes any number of dimensions there and `parse_file`,
which every other reader goes through, still takes one. The type it builds **nests**
(`TArray attrs (TArray [] uint32 [10]) [16]`), which is what the substitution leaves and
what `tElement`'s ECC clause needs (`Type.hs:930-935`). See
[`known-divergences.md`](../known-divergences.md) #31.

**Q2 — `::template` disambiguator.** `templateQualifier` (`Parser.hs:646`) is consumed at
the head of *every* path segment (`:653`), so `Outer::template Inner<T>` should be legal,
but only the `.template` form appears in the corpus
(`test/syntax/dependent-template.k:121`). Probe:
```
template <typename T> class Outer { public: template <typename U> struct Inner { U x; } }
template <typename T> inline void f() { typename Outer<T>::template Inner<uint8> v; }
```

**Q3 — Prefix-operator stacking and `-bitsizeof`.** Derived from the level assignment at
`Parser.hs:921-930` but not executed. Probe:
```
inline void main() {
    static assert(bitsizeof -1i8 == 8);   // expected to parse
    static assert(-bitsizeof 1i8 == -8);  // expected to FAIL to parse
    static assert(!!true);                // expected to FAIL (same-level stacking)
    static assert(!~0u1 == 0);            // expected to FAIL (`!` and `~` are one level)
}
```

**Q4 — Is the declaration/expression predicate of §8.1 exactly equivalent?** The
reference's answer depends on `findSymbol`; the proposed context-free predicate does not.
A case where they could differ: an *unresolved* name in expression position.
`typeSpecifier' False` (`Parser.hs:546`) rejects unknown names, so `Unknown x;` in a
function body is a *declaration* in the reference (via `try (localVariable ";")`, which
uses `typ' True`). The proposed predicate agrees. But `Unknown;` alone is an expression
in the reference (unresolved name → `NamedValueF`). Probe: compile
```
inline void main() { Unknown x; Unknown; }
```
and diff the two error sets.

**Q5 — `try` rollback of `registerParseError`.** The claim in §0.3 that megaparsec's
`try` restores the registered-error list is derived from `pTry`'s definition (it re-passes
the original `State`), not from the vendored source. Probe: a program where a
speculative `try (expression <* semi)` registers "Invalid use of a type as an expression"
and then backtracks, e.g. `inline void main() { uint32 x; }` — if rollback did not
happen the reference would emit a spurious error, and it does not
(`test/syntax/basics.k` compiles clean).

**Q6 — Look-ahead brace-counter fragility (trap 3).** Probe:
```
class C { public: void f() { println("}"); } uint32 g; void h() { g = 1; } }
export C;
```
If `skipBraces` mis-counts, `h` will fail to resolve `g` before its declaration point.

**Q7 — Does `[[name(…)]]` accept anything but a plain string literal?** `strAttr`
(`Parser.hs:177`) takes `strLiteral` (`:821`), a `StringLiteralF`, not an interpolated
string. `test/syntax/name-attr.k:37-41` shows `[[name(10)]]` → `expected:1`. Probe
whether `[[name("a{b}")]]` is a parse error or an accepted literal `a{b}` — `stringLiteral`
(`Lexer.hs:162`) does not treat `{` specially, so it should be accepted verbatim.
**UNVERIFIED**.

**Q8 — Empty template argument list.** `Foo<>` should parse (`sepBy`, `Parser.hs:697`)
but its meaning is unclear. Probe: `template <typename T> struct S { T x; }  S<> v;`

---

## 11. `kanagawa.ungram` — proposed codegen input

Ungrammar syntax: `Node = Rule`, `'tok'` terminals, juxtaposition = sequence,
`|` alternation, `?` optional, `*` repetition, `()` grouping, `label:` field names.

This grammar is deliberately **context-free**: every place the reference consults
`findSymbol` is replaced by a single syntactic node that a later resolution pass
reinterprets (see §6.4). The AST-constructor → ungram-node mapping follows the grammar.

```ungrammar
//============================================================================
// kanagawa.ungram — syntactic (CST) grammar for the Kanagawa HLS language.
// Derived from compiler/hs/lib/Language/Kanagawa/Parser.hs (968 lines) and
//                compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs (805 lines).
//============================================================================

//----------------------------------------------------------------------------
// Files and modules
//----------------------------------------------------------------------------

SourceFile =
  ModuleDecl?
  DeclList

ModuleDecl =
  ModuleIdentifier
  '{' (ExposedItem (',' ExposedItem)*)? '}'
  DeclList

ModuleIdentifier =
  'module' ModulePath

ModuleDiff =
  'module' lhs:ModulePath '\' rhs:ModulePath

ExposedItem =
  ModuleDiff
| ModuleIdentifier
| Name

ModulePath =
  '@module_path'                      // '.cmdargs' | '.options' | dot-separated, '-' allowed

DeclList =
  Import*
  Item*

Import =
  'import' ModulePath ('as' alias:Name)?

//----------------------------------------------------------------------------
// Items (top level, class members, and function-body-local declarations)
//----------------------------------------------------------------------------

Item =
  Variable
| StaticIfItem
| ExportType
| Extern
| Alias
| Enum
| Struct
| Union
| StaticAssert
| StaticAssertLegacy
| Class
| Function
| FunctionDecl
| Template
| DefaultInitialization
| Visibility
| EmptyItem

EmptyItem =
  ';'

Visibility =
  ('private' | 'public') ':'

ExportType =
  AttrList? 'export' (TemplateRef | Type) ';'

Extern =
  AttrList? 'extern' (TemplateRef | PathType) ';'

Alias =
  'using' Name '=' Type ';'

Enum =
  'enum' Name ':' Type
  '{' EnumConstant (',' EnumConstant)* '}'

EnumConstant =
  Name ('=' Expr)?

Struct =
  'struct' Name '{' (FieldDecl (';' FieldDecl)* ';'?)? '}'

Union =
  'union' Name '{' (FieldDecl (';' FieldDecl)* ';'?)? '}'

FieldDecl =
  Type Name

Class =
  'class' Name '{' Item* '}'

DefaultInitialization =
  'static'? 'default' '=' Expr ';'

StaticAssert =
  'static' 'assert' '(' Expr ')' ';'

StaticAssertLegacy =
  'static_assert' '(' Expr ')' ';'

StaticIfItem =
  'static' 'if' '(' cond:Expr ')'
  then_branch:ItemOrBlock
  ('else' else_branch:ItemOrBlock)?

ItemOrBlock =
  ItemBlock
| Item

ItemBlock =
  '{' Item* '}'

//----------------------------------------------------------------------------
// Templates
//----------------------------------------------------------------------------

Template =
  TemplateParamList
  decl:Item

TemplateParamList =
  'template' '<' TemplateParam (',' TemplateParam)* '>'

TemplateParam =
  TypenameParam
| TemplateTemplateParam
| NonTypeParam

TypenameParam =
  'typename' Name ('=' default_type:Type)?

TemplateTemplateParam =
  'template' '<' (TemplateParamKind (',' TemplateParamKind)*)? '>'
  'typename' Name ('=' default_template:TemplateRef)?

TemplateParamKind =
  TypenameKind
| Type

TypenameKind =
  'typename'

NonTypeParam =
  Type Name ('=' default_value:TemplateExpr)?

//----------------------------------------------------------------------------
// Variables and functions
//----------------------------------------------------------------------------

Variable =
  AttrList? 'static'? Type Name ('=' Expr)? ';'

UninitConst =
  'const' Type Name

Function =
  AttrList? FunctionModifier? ret:Type Name ParamList Block

FunctionDecl =
  AttrList? ret:Type Name ParamList ';'

FunctionModifier =
  'inline' | 'noinline'

ParamList =
  '(' (Param (',' Param)*)? ')'

Param =
  AttrList? Type Name

Block =
  '{' Stmt* '}'

//----------------------------------------------------------------------------
// Types
//----------------------------------------------------------------------------

Type =
  VoidType
| ConstType
| AutoType
| StringType
| BoolType
| Float32Type
| IntType
| UintType
| ParamIntType
| ParamUintType
| DecltypeType
| TypenameType
| PathType
| ArrayType
| FunctionType

VoidType     = 'void'
ConstType    = 'const' Type
AutoType     = 'auto'
StringType   = 'string'
BoolType     = 'bool'
Float32Type  = 'float32'
IntType      = '@int_width'                     // 'int'  immediately followed by digits
UintType     = '@uint_width'                    // 'uint' immediately followed by digits
ParamIntType = 'int'  '<' TemplateExpr '>'
ParamUintType= 'uint' '<' TemplateExpr '>'
DecltypeType = 'decltype' '(' Expr ')'
TypenameType = 'typename' PathType

PathType =
  Path

ArrayType =
  AttrList? elem:Type ArrayDim*

ArrayDim =
  '[' Expr ']'

FunctionType =
  AttrList? '(' (FunctionTypeParam (',' FunctionTypeParam)*)? ')' '->' ret:Type

FunctionTypeParam =
  AttrList? Type Name?

//----------------------------------------------------------------------------
// Attributes
//----------------------------------------------------------------------------

AttrList =
  '[[' Attr (AttrSep Attr)* ']]'

AttrSep =
  ','
| ']]' '[['

Attr =
  FlagAttr
| IntAttr
| IdentAttr
| StrAttr

FlagAttr =
  'async' | 'last' | 'initialize' | 'memory' | 'no_backpressure'
| 'pure' | 'non_replicated' | 'pipelined' | 'quad_port'
| 'reorder_by_looping' | 'reset' | 'unordered'
// 'atomic' exists as a Flag but is synthesized by the `atomic` statement,
// never written in source (Parser.hs:561).

IntAttr =
  ('call_rate' | 'fifo_depth' | 'latency' | 'max_threads'
  | 'schedule' | 'thread_rate' | 'transaction_size')
  '(' Expr ')'

IdentAttr =
  'ecc' '(' Path ')'

StrAttr =
  'name' '(' StringLiteral ')'

//----------------------------------------------------------------------------
// Statements
//----------------------------------------------------------------------------

Stmt =
  EmptyStmt
| BarrierStmt
| ReorderStmt
| AtomicStmt
| AnnotatedStmt
| ReturnStmt
| IncDecStmt
| AssignStmt
| NestedScope
| ExprStmt
| ForStmt
| DoWhileStmt
| StaticForStmt
| StaticIfStmt
| IfStmt
| SwitchStmt
| Item                              // declarations are legal wherever Stmt* appears

EmptyStmt      = ';'
BarrierStmt    = 'barrier' ';'
ReorderStmt    = 'reorder' Stmt
AtomicStmt     = 'atomic' Stmt
AnnotatedStmt  = AttrList Stmt
ReturnStmt     = 'return' Expr? ';'
NestedScope    = '{' Stmt* '}'
ExprStmt       = Expr ';'

IncDecStmt =
  (IncDecOp LValue | LValue IncDecOp) ';'

IncDecOp =
  '++' | '--'

AssignStmt =
  LValue AssignOp Expr ';'

AssignOp =
  '=' | '+=' | '-=' | '*=' | '/=' | '%='
| '&=' | '|=' | '^=' | '&&=' | '||=' | '^^='

ForStmt =
  AttrList? 'for' '(' UninitConst ':' limit:Expr ')' body:Stmt

DoWhileStmt =
  AttrList? 'do' body:Stmt 'while' '(' cond:Expr ')'

StaticForStmt =
  'static' 'for' '(' UninitConst ':' limit:Expr ')' body:Stmt

StaticIfStmt =
  'static' 'if' '(' cond:Expr ')'
  then_branch:StmtOrBlock
  ('else' else_branch:StmtOrBlock)?

StmtOrBlock =
  StmtBlock
| Stmt

StmtBlock =
  '{' Stmt* '}'

IfStmt =
  'if' '(' cond:Expr ')' then_branch:Stmt ('else' else_branch:Stmt)?

SwitchStmt =
  'switch' '(' Expr ')' '{' SwitchBlock* '}'

SwitchBlock =
  SwitchCase
| SwitchDefault
| StaticIfSwitchBlock

SwitchCase =
  'case' Expr ':' Stmt* 'break' ';'

SwitchDefault =
  'default' ':' Stmt* 'break' ';'

StaticIfSwitchBlock =
  'static' 'if' '(' cond:Expr ')'
  then_branch:SwitchBlockOrBlock
  ('else' else_branch:SwitchBlockOrBlock)?

SwitchBlockOrBlock =
  SwitchBlockGroup
| SwitchBlock

SwitchBlockGroup =
  '{' SwitchBlock* '}'

//----------------------------------------------------------------------------
// Expressions
//----------------------------------------------------------------------------

Expr =
  Literal
| PathExpr
| ParenExpr
| UnaryExpr
| BinExpr
| TernaryExpr
| CallExpr
| MethodRefExpr
| IndexExpr
| FieldExpr
| CastExpr
| MuxExpr
| LutMulExpr
| ConcatExpr
| FanOutExpr
| StaticExpr
| BitOffsetOfExpr
| ByteOffsetOfExpr
| InitializerList
| DesignatedInitializerList
| LambdaExpr
| TypeExpr

// A type appearing where an expression is expected (Parser.hs:843).
// The reference gates this on findSymbol; here it is purely syntactic and the
// resolver decides.
TypeExpr =
  Type

// TemplateExpr is Expr with '<', '>', '<<', '>>' removed from the top-level
// operator table (Parser.hs:913).  Parenthesised sub-expressions restore them.
TemplateExpr =
  Expr

ParenExpr =
  '(' Expr ')'

UnaryExpr =
  op:('-' | '!' | '~' | 'bitsizeof' | 'bytesizeof' | 'clog2') Expr

BinExpr =
  lhs:Expr
  op:( '*' | '/' | '%'
     | '+' | '-'
     | '<<' | '>>'
     | '<=' | '>=' | '<' | '>'
     | '==' | '!='
     | '&' | '^' | '|'
     | '&&' | '^^' | '||' )
  rhs:Expr

TernaryExpr =
  cond:Expr '?' then_branch:Expr ':' else_branch:Expr

CallExpr =
  AttrList? callee:(PathExpr | MethodRefExpr) ArgList

ArgList =
  '(' (Expr (',' Expr)*)? ')'

MethodRefExpr =
  receiver:Expr '.' PathSegment

IndexExpr =
  base:Expr '[' index:Expr ']'

FieldExpr =
  base:Expr '.' NameRef

CastExpr =
  'cast' '<' Type '>' '(' Expr ')'

MuxExpr =
  'mux' '(' selector:Expr ',' Expr (',' Expr)* ')'

LutMulExpr =
  'lutmul' '(' lhs:Expr ',' rhs:Expr ')'

ConcatExpr =
  'concat' '(' Expr (',' Expr)* ')'

FanOutExpr =
  'fan_out' '<' TemplateExpr '>' '(' Expr ')'

StaticExpr =
  'static' '(' Expr ')'

BitOffsetOfExpr =
  'bitoffsetof' '(' Type ',' NameRef ')'

ByteOffsetOfExpr =
  'byteoffsetof' '(' Type ',' NameRef ')'

InitializerList =
  '{' PositionalInit (',' PositionalInit)* '}'

PositionalInit =
  Expr

DesignatedInitializerList =
  '{' (Designator (',' Designator)*)? '}'

Designator =
  '.' NameRef '=' Expr

LambdaExpr =
  CaptureList
  ParamList?
  ('->' ret:Type)?
  Block

CaptureList =
  '[' (Capture (',' Capture)*)? ']'

Capture =
  NameRef

LValue =
  PathExpr
| IndexExpr
| FieldExpr

//----------------------------------------------------------------------------
// Paths and names
//----------------------------------------------------------------------------

PathExpr =
  Path

Path =
  qualifier:Path? '::'? PathSegment

PathSegment =
  'template'? NameRef GenericArgList?

GenericArgList =
  '<' (GenericArg (',' GenericArg)*)? '>'

GenericArg =
  TemplateRef
| Type
| TemplateExpr

// A path known (after resolution) to name an uninstantiated template.
TemplateRef =
  Path

Name    = 'ident'
NameRef = 'ident'

//----------------------------------------------------------------------------
// Literals
//----------------------------------------------------------------------------

Literal =
  IntLiteral
| FloatLiteral
| BoolLiteral
| StringLiteral
| InterpolatedString

IntLiteral    = '@int'      // radix prefix, '_' separators, optional 'i'N / 'u'N suffix
FloatLiteral  = '@float'
BoolLiteral   = 'true' | 'false'
StringLiteral = '@string'   // only inside [[name("…")]]

InterpolatedString =
  '"' InterpolationSegment* '"'

InterpolationSegment =
  '@text'
| Interpolation

Interpolation =
  '{' Expr '='? (',' alignment:Expr)? (':' Format)? '}'

Format =
  spec:('b' | 'o' | 'd' | 'x' | 'X') precision:'@decimal'?
```

### 11.1 AST constructor → ungram node mapping

| `ExpF` constructor | ungram node(s) |
|---|---|
| `SeqF` | implicit (repetition in `DeclList`, `Block`, `ArgList`, `AttrList`, …) |
| `FlagAttrF` | `FlagAttr` |
| `IntAttrF` | `IntAttr` |
| `AttrF` | `IdentAttr`, `StrAttr` |
| `FunctionModifierF` | `FunctionModifier` |
| `AutoF` | `AutoType` |
| `VoidF` | `VoidType` |
| `BooleanF` | `BoolType` |
| `FloatF` | `Float32Type` |
| `StringF` | `StringType` |
| `IntegerF` | `IntType` |
| `ParamIntF` | `ParamIntType` |
| `UnsignedF` | `UintType` |
| `ParamUintF` | `ParamUintType` |
| `ConstF` | `ConstType` |
| `ReferenceF` | *(no source form)* |
| `ArrayF` | `ArrayType` + `ArrayDim` |
| `TemplateInstanceF` | `PathSegment` with `GenericArgList` |
| `TemplateParamF` | `TypenameParam`, `TemplateTemplateParam`, `NonTypeParam` |
| `TypeParamF` | resolution-time projection of `TemplateParam`; no CST node |
| `TypenameF` | `TypenameKind` (in `TemplateParamKind`), `TypenameType` |
| `FunctionTypeParamF` | `FunctionTypeParam` |
| `FunctionTypeF` | `FunctionType` |
| `ThisF` | *(no source form)* |
| `DesignatorF` | `Designator` |
| `PositionalF` | `PositionalInit` |
| `InitializerListF` | `InitializerList`, `DesignatedInitializerList` |
| `BitOffsetF` | `BitOffsetOfExpr` |
| `ByteOffsetF` | `ByteOffsetOfExpr` |
| `IntLiteralF` | `IntLiteral` |
| `FloatLiteralF` | `FloatLiteral` |
| `BoolLiteralF` | `BoolLiteral` |
| `StringLiteralF` | `StringLiteral` |
| `InterpolatedStringF` | `InterpolatedString`, `InterpolationSegment`, `Interpolation`, `Format` |
| `IdentifierF` | `Name`, `NameRef` |
| `ScopedNameF` | `Path` |
| `QualifiedIdentifierF` | `Path` (wrapper elided) |
| `TypeIdentifierF` | `PathType` |
| `FunctionSpecifierF` | `CallExpr.callee` (`PathExpr` or `MethodRefExpr`) |
| `EnumValueF` | *(no source form)* |
| `NamedValueF` | `PathExpr` |
| `BinaryF` | `BinExpr`, `LutMulExpr` (for `LutMul`), and the `IncDecStmt`/`AssignStmt` desugarings |
| `UnaryF` | `UnaryExpr` |
| `FunctionCallF` | `CallExpr` |
| `ArrayAccessF` | `IndexExpr` |
| `ArrayAccessLValueF` | `IndexExpr` in `LValue` position |
| `MemberAccessF` | `FieldExpr` |
| `CastF` | `CastExpr` |
| `MuxF` | `MuxExpr`, `TernaryExpr` |
| `ConcatF` | `ConcatExpr` |
| `FanOutF` | `FanOutExpr` |
| `StaticF` | `StaticExpr` |
| `AnnotatedF` | `AnnotatedStmt`, `AtomicStmt` |
| `BarrierF` | `BarrierStmt` |
| `ReorderF` | `ReorderStmt` |
| `ReturnF` | `ReturnStmt` |
| `DoWhileF` | `DoWhileStmt` |
| `UnrolledForF` | *(no source form)* |
| `StaticForF` | `StaticForStmt` |
| `RangeForF` | `ForStmt` |
| `IfF` | `IfStmt` |
| `StaticIfF` | `StaticIfStmt`, `StaticIfItem`, `StaticIfSwitchBlock` |
| `SwitchF` | `SwitchStmt` |
| `SwitchCaseF` | `SwitchCase` |
| `SwitchDefaultF` | `SwitchDefault` |
| `AssignF` | `AssignStmt`, `IncDecStmt` |
| `NestedScopeF` | `NestedScope`, `ItemBlock`, `StmtBlock` |
| `ModuleIdentifierF` | `ModuleIdentifier` |
| `ModuleDiffF` | `ModuleDiff` |
| `ModuleDeclF` | `ModuleDecl` |
| `EnumConstantF` | `EnumConstant` |
| `EnumF` | `Enum` |
| `StructF` | `Struct` |
| `UnionF` | `Union` |
| `ClassF` | `Class` |
| `DefaultInitializationF` | `DefaultInitialization` |
| `PrivateF` / `PublicF` | `Visibility` |
| `TemplateF` | `Template`, `TemplateParamList` |
| `DecltypeF` | `DecltypeType` |
| `AliasF` | `Alias` |
| `CaptureF` | `Capture` |
| `VariableF` | `Variable`, `FieldDecl`, `UninitConst` |
| `FuncParamF` | `Param` |
| `FunctionF` | `Function` |
| `FunctionDeclF` | `FunctionDecl` |
| `LambdaF` | `LambdaExpr` |
| `ExportTypeF` | `ExportType` |
| `ExternF` | `Extern` |
| `StaticAssertF` | `StaticAssert` |
| `StaticAssertLegacyF` | `StaticAssertLegacy` |
| `WarningF` | *(synthesized; `Parser.hs:239`)* |
| `ErrorF` | *(synthesized post-parse; `Error.hs:139-140`)* |

### 11.2 Deliberate divergences from the reference tree

1. `SeqF` is not a CST node — repetition is expressed structurally.
2. `TypeIdentifierF` / `ScopedNameF` / `QualifiedIdentifierF` / `NamedValueF` /
   `IdentifierF` collapse into `Path` + `NameRef`; the qualifier and scope fields
   (`declScopeName`, `getScope`) are recomputed during lowering, not stored.
3. `declScopeName` on every declaration node is dropped from the CST.
4. `PositionalF`'s index is the child's position in the list.
5. `ArrayAccessF` vs `ArrayAccessLValueF` is a lowering distinction, not a syntactic one.
6. `MuxF` retains the reversed arm order **only in HIR**; `TernaryExpr` in the CST keeps
   source order.
7. `TemplateInstanceF` with an empty `Seq` (the "function template with no explicit args"
   case, `Parser.hs:694`) is a resolution artefact and has no CST node.

---

## 12. Cross-reference: `PrettyPrint.hs` as an oracle

`prettySource` (`PrettyPrint.hs:37-232`) renders every node back to source and is a useful
round-trip oracle. Known divergences from what the parser accepts (i.e. the printer is
*not* a valid grammar source in these cases):

| Node | printed as | parser accepts |
|---|---|---|
| `BarrierF` | `"barier"` (`:78`) | `barrier` |
| `BitOffsetF` | `bitoffset(…)` (`:81`) | `bitoffsetof(…)` |
| `ByteOffsetF` | `byteoffset(…)` (`:85`) | `byteoffsetof(…)` |
| `ThisF` | `this` (`:165`) | *(no such keyword)* |
| `ReferenceF` | `T&` (`:143`) | *(no such syntax)* |
| `UnrolledForF` | `static for(…)` (`:176`) | *(never parsed)* |
| `AliasF` | `using b = a` (`:69`) — note field order `(_ a b)` = `(scope, type, name)` | `using name = type;` |
