# Kanagawa Type System — Implementation Specification

> **Status, 2026-10-11.** Extracted at kanagawa `8e0a3fb` (2026-05-18); the
> reference is pinned at `98a2d8c` (`reference.lock`), and nothing this spec
> cites changed in between (upstream added the `list-deps` mode and
> `hw.typealias` emission, outside the type system). §14 ("Proposed Rust
> representation") is pre-implementation design; as built, the type model is
> `kg-ty` and inference, deduction, the fixpoint and the checks after
> instantiation are `kg-sem` (§14.4's note). Where §14 and the code differ,
> `crates/kg-ty/src/lib.rs` and `crates/kg-sem/src/lib.rs` are authoritative.

Extracted from the reference compiler at `third_party/kanagawa` (read-only).
Every claim below carries a `path:line` citation. Anything not directly citable is marked
**UNVERIFIED**.

## 0. Citation legend

Paths are relative to `third_party/kanagawa`. Short names used throughout:

| Short name | Full path |
|---|---|
| `Type.hs` | `compiler/hs/lib/Language/Kanagawa/Type.hs` |
| `Inference.hs` | `compiler/hs/lib/Language/Kanagawa/Type/Inference.hs` |
| `TypeCheck.hs` | `compiler/hs/lib/Language/Kanagawa/TypeCheck.hs` |
| `Constexpr.hs` | `compiler/hs/lib/Language/Kanagawa/Constexpr.hs` |
| `Syntax.hs` | `compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs` |
| `Parser.hs` | `compiler/hs/lib/Language/Kanagawa/Parser.hs` |
| `Lexer.hs` | `compiler/hs/lib/Language/Kanagawa/Parser/Lexer.hs` |
| `Internal.hs` | `compiler/hs/lib/Language/Kanagawa/Internal.hs` |
| `Desugar.hs` | `compiler/hs/lib/Language/Kanagawa/Desugar.hs` |
| `Symbols.hs` | `compiler/hs/lib/Language/Kanagawa/Symbols.hs` |
| `Frontend.hs` | `compiler/hs/lib/Language/Kanagawa/Frontend.hs` |
| `Function.hs` | `compiler/hs/lib/Language/Kanagawa/Function.hs` |
| `Template.hs` | `compiler/hs/lib/Language/Kanagawa/Template.hs` |
| `Closure.hs` | `compiler/hs/lib/Language/Kanagawa/Closure.hs` |
| `Mangle.hs` | `compiler/hs/lib/Language/Kanagawa/Mangle.hs` |
| `Error.hs` | `compiler/hs/lib/Language/Kanagawa/Error.hs` |
| `Warning.hs` | `compiler/hs/lib/Language/Kanagawa/Warning.hs` |
| `Recursion.hs` | `compiler/hs/lib/Language/Kanagawa/Recursion.hs` |
| `CmdArgs.hs` | `compiler/hs/app/Options/CmdArgs.hs` |
| `cpp/parse_tree.h` | `compiler/cpp/parse_tree.h` |
| `cpp/parse_tree.cpp` | `compiler/cpp/parse_tree.cpp` |
| `cpp/ir.h` | `compiler/cpp/ir.h` |
| `cpp/pch.h` | `compiler/cpp/pch.h` |

There are **two** type checkers. The Haskell front-end computes `Type` values and attaches
them to AST nodes (`Type.hs:1105`). The C++ middle-end re-derives types independently on the
`ParseTreeNode` graph (`cpp/parse_tree.cpp:3004` for binary ops). Where the front-end
computed a narrower integer type, the middle-end *adopts* the front-end type
(`_frontEndType`, `cpp/parse_tree.cpp:1244-1267`). Divergences are called out explicitly in
§2.7.

---

## 1. The type model

### 1.1 The `Type` algebraic data type

`Type` is a plain non-recursive-in-name-only Haskell ADT with **26 constructors**
(`Type.hs:224-277`). It derives `Eq, Ord, Show` (`Type.hs:278`) — structural equality with
no interning, no unification variables, no substitution environment. Type "inference" is
*pure bottom-up synthesis with a whole-program fixpoint*, not Hindley–Milner.

| # | Constructor | Params | Meaning | Source syntax that produces it | Cite |
|---|---|---|---|---|---|
| 1 | `TAuto` | — | the `auto` placeholder, before deduction | `auto` (`Parser.hs:523` `autoType`), `const auto` | `Type.hs:225`, `Type.hs:1223`, `Type.hs:1225` |
| 2 | `TTemplate` | — | a template *name* (not an instance) | `template <...> decl` | `Type.hs:227`, `Type.hs:1267` |
| 3 | `TType Type` | inner | "this expression *is* a type", i.e. a type in expression position | every type production; `theType` unwraps it | `Type.hs:229`, `Type.hs:455-460` |
| 4 | `TUndefined` | — | symbol lookup failed | produced by `typeOfSymbol` when `lookupSymbol` returns `Nothing` | `Type.hs:231`, `Type.hs:1336` |
| 5 | `TUntyped` | — | node has no type by construction (statements, attrs, identifiers) | `SeqF`, `IfF`, `AssignF`, … (full list §1.5) | `Type.hs:233`, `Type.hs:1290-1321` |
| 6 | `TUnresolved` | — | type not yet known this round | initial note for every node (`noteType`); many `infer` clauses | `Type.hs:235`, `Type.hs:970-972` |
| 7 | `TInitializer [Type]` | element types | brace-initializer list type | `{ a, b, c }` and `{ .x = a, .y = b }` (`Parser.hs:857-864`) | `Type.hs:237`, `Type.hs:1227` |
| 8 | `TDesignator Name Type` | field name, value type | one `.f = e` element | `.f = e` (`Parser.hs:864`) | `Type.hs:239`, `Type.hs:1201` |
| 9 | `TPositional Int Type` | index, value type | one positional element of an init list | positional element of `{...}` (`Parser.hs:858-862`) | `Type.hs:241`, `Type.hs:1248` |
| 10 | `TVoid` | — | `void` | `void` (`Parser.hs:523`) | `Type.hs:243`, `Type.hs:1288` |
| 11 | `TBoolean` | — | `bool` | `bool` (`Parser.hs:540`) | `Type.hs:245`, `Type.hs:1192` |
| 12 | `TFloat` | — | 32-bit float | `float32` (`Parser.hs:541`) | `Type.hs:247`, `Type.hs:1206` |
| 13 | `TString` | — | compile-time string | `string` (`Parser.hs:525`) | `Type.hs:249`, `Type.hs:1265` |
| 14 | `TSigned Int` | bit width | `intN` | `int<N>` literal-suffix form `intN` (`Parser.hs:542`); `int<expr>` → `ParamIntF` (`Parser.hs:544`) | `Type.hs:251`, `Type.hs:1228`, `Type.hs:1246` |
| 15 | `TUnsigned Int` | bit width | `uintN` | `uintN` (`Parser.hs:543`); `uint<expr>` (`Parser.hs:545`) | `Type.hs:253`, `Type.hs:1283`, `Type.hs:1247` |
| 16 | `TDependent Type` | return type | return type of a `[[pipelined]]` function, elaborated at the call site | `[[pipelined]]` flag on a function (`Type.hs:1368-1373`) | `Type.hs:255`, `Type.hs:1375-1378` |
| 17 | `TConst Type` | inner | `const T` | `const T` (`Parser.hs:524`) | `Type.hs:257`, `Type.hs:1200` |
| 18 | `TReference QualifiedName` | class name | reference to a class instance (only produced for `this`) | `this` / implicit member access (`Type.hs:1269-1272`); no surface `&` syntax exists | `Type.hs:259`, `Type.hs:1249` |
| 19 | `TArray (Set TAttr) Type [Integer]` | memory attrs, element, dimensions | array / memory | `T[N]`, `[[memory]] T[N]`, `T[N][M]` (`Parser.hs:526,529-538`) | `Type.hs:261`, `Type.hs:1179` |
| 20 | `TFunction FunctionKind (Set TAttr) Type [TFuncParam]` | kind, attrs, return, params | function / function type | `inline T f(...) {}`, `(P) -> R` function type (`Parser.hs:551-557`) | `Type.hs:263`, `Type.hs:1209,1220,1223-1224` |
| 21 | `TClosure QualifiedName Type Type QualifiedName` | lambda-function name, function type, capture type, closure-alias name | a closure value | instantiation of `data.closure.core::Closure<F, fn, Capture>` (`library/data/closure/core.k:9-10`) | `Type.hs:265`, `Type.hs:1175-1177` |
| 22 | `TEnum QualifiedName Type` | enum name, base type | enum | `enum E : uintN { ... }` (`Parser.hs:282-284`) | `Type.hs:267`, `Type.hs:1202` |
| 23 | `TStruct QualifiedName [(Name, Type)]` | name, ordered fields | struct | `struct S { ... }` (`Parser.hs:294`, `Parser.hs:298-299`) | `Type.hs:269`, `Type.hs:1264` |
| 24 | `TUnion QualifiedName [(Name, Type)]` | name, fields | union | `union U { ... }` (`Parser.hs:301`, `Parser.hs:305`) | `Type.hs:271`, `Type.hs:1279` |
| 25 | `TClass QualifiedName [(Name, Type)]` | name, member variables | class | `class C { ... }` (`Parser.hs:331-339`) | `Type.hs:273`, `Type.hs:1195-1197` |
| 26 | `TInstance Type QualifiedName [(Name, TArg)]` | concrete type, template name, bound args | template instance wrapper around the instantiated type | any `Tmpl<args>` after instantiation (`Template.hs:125`, `Type.hs:433-441`) | `Type.hs:275`, `Type.hs:433` |
| 27 | `TError String` | message | a type error, carried as a type | any of the `TError` producers, §5 | `Type.hs:277`, `Frontend.hs:169-174` |

That is 26 data constructors (`TAuto` … `TError`). The task plan's "~25" matches.

### 1.2 Auxiliary types

```haskell
data TFuncParam = TFuncParam
    { attrOfParam :: Set TAttr, typeOfParam :: Type, nameOfParam :: Maybe Name }
```
`Type.hs:117-124`. **`Eq` for `TFuncParam` ignores the name** — it compares only
`typeOfParam` and `attrOfParam` (`Type.hs:126-129`). `Ord` is derived and *does* include the
name (`Type.hs:123`), so `Eq`/`Ord` are inconsistent. A Rust port must reproduce the `Eq`
behaviour (name-insensitive) wherever function-type equality matters.

```haskell
data TAttr = TFlagAttr Flag | TIntAttr Attribute Integer | TAttr Attribute QualifiedName Type
```
`Type.hs:188-195`. Attributes live in a `Set` so they are order-insensitive and deduplicated.

```haskell
data TArg = TArg String | TStrArg String | TTypeArg Type | TIntArg Integer | TNameArg Type QualifiedName
```
`Type.hs:206-215`. `TArg String` is the fallback: the pretty-printed expression
(`Type.hs:437`).

`Attribute` (9 values): `CallRate, Ecc, FifoDepth, Latency, MaxThreads, Rename, Schedule,
ThreadRate, TransactionSize` — spelled `call_rate, ecc, fifo_depth, latency, max_threads,
name, schedule, thread_rate, transaction_size` (`Syntax.hs:124-145`).

`Flag` (13 values): `Async, Atomic, EndTransaction, Initialize, Memory, NoBackPressure,
Pure, NonReplicated, Pipelined, QuadPort, ReorderByLooping, Reset, Unordered` — spelled
`async, atomic, last, memory, initialize, no_backpressure, pure, non_replicated, pipelined,
quad_port, reorder_by_looping, reset, unordered` (`Syntax.hs:150-179`).

`FunctionKind` = `FreeFunction | MemberFunction | LambdaFunction` (`Syntax.hs:205-209`).

`DeclareFlag` = `DeclareGlobal | DeclareClass | DeclareLocal | DeclareStatic |
DeclareMember | DeclareUninitConst` (`Syntax.hs:184-191`).

### 1.3 How a type is attached to a node

```haskell
data Typed a = Typed { typeof :: !Type, untype :: a }
```
`Type.hs:465-471`. Every AST node carries `Typed n` where `n` is the source note (`Src`).
`noteType` initialises every node to `TUnresolved` (`Type.hs:970-972`).

`TypedExp` class (`Type.hs:451-472`):
- `typeOf` — the node's own type.
- `theType` = unwrap `TType`; if the type is not resolved → `TUnresolved`; otherwise
  `TError "Expected a type"` (`Type.hs:453-460`).
- `sizeOf = tWidth . typeOf` (`Type.hs:462`).
- `offsetOf x = tOffsetOf x . theType` (`Type.hs:464-465`).
- `typeResolved = tResolved . typeOf` (`Type.hs:467-468`).

### 1.4 `tResolved` — the termination measure

`Type.hs:1000-1027`. Exhaustive:

| Type | resolved? |
|---|---|
| `TAuto` | **True** |
| `TTemplate` | False |
| `TUndefined` | False |
| `TUnresolved` | False |
| `TUntyped` | True |
| `TInitializer xs` | all elements resolved |
| `TDesignator _ x`, `TPositional _ x` | `tResolved x` |
| `TVoid`, `TBoolean`, `TFloat`, `TString` | True |
| `TSigned _`, `TUnsigned _` | True |
| `TError _` | **True** |
| `TType x`, `TDependent x`, `TConst x` | `tResolved x` |
| `TReference _` | True |
| `TArray as x xs` | `tResolved x && not (null xs) && all attrResolved as` |
| `TFunction _ _ r ps` | `tResolved r && all (tResolved . typeOfParam) ps` |
| `TClosure _ f c _` | `tResolved f && tResolved c` |
| `TEnum _ x` | `tResolved x` |
| `TStruct _ xs`, `TUnion _ xs`, `TClass _ xs` | all field types resolved |
| `TInstance x _ _` | `tResolved x` |

`attrResolved (TAttr _ _ a) = tResolved a`; all other attrs resolved (`Type.hs:1029-1031`).

Two traps: `TAuto` counts as resolved, and `TError` counts as resolved (so an error type
terminates the fixpoint rather than looping).

### 1.5 Which AST nodes are `TUntyped`

Complete list (`Type.hs:1290-1321`): `AssignF, AnnotatedF, BarrierF,
DefaultInitializationF, ErrorF, ExportTypeF, ExternF, FlagAttrF, FunctionModifierF,
IdentifierF, IfF, IntAttrF, DoWhileF, ModuleDeclF, ModuleDiffF, ModuleIdentifierF,
NestedScopeF, PrivateF, PublicF, RangeForF, ReorderF, SeqF, StaticAssertF,
StaticAssertLegacyF, SwitchCaseF, SwitchDefaultF, SwitchF, TemplateParamF, AttrF,
TypenameF, StaticForF, UnrolledForF`.

Additionally `ScopedNameF` yields `TUntyped` when the trailing identifier starts with `'@'`
(module namespace) or `'$'` (compiler-generated) (`Type.hs:1252-1255`), and when the symbol
resolves to something that is not an `Alias` (`Type.hs:1256-1260`).

### 1.6 Bit width (`tWidth`)

`Type.hs:695-723`, exhaustive:

| Type | `tWidth` |
|---|---|
| `TAuto`, `TTemplate`, `TInitializer _`, `TVoid`, `TDependent _`, `TFunction …`, `TClass …`, `TError _`, `TUndefined`, `TUnresolved`, `TUntyped` | `Nothing` |
| `TBoolean` | `Just 1` |
| `TFloat` | `Just 32` |
| `TString` | **`Just 0`** |
| `TSigned n`, `TUnsigned n` | `Just n` |
| `TReference _` | **`Just 1`** |
| `TArray _ _ []` | `Nothing` |
| `TArray _ x xs` | `product xs * width x` |
| `TType x`, `TDesignator _ x`, `TPositional _ x`, `TConst x`, `TInstance x _ _` | width of `x` |
| `TClosure _ _ cap _` | width of the **capture** type |
| `TEnum _ base` | width of base |
| `TStruct _ fs` | `tSumWidths fs` = sum of field widths, `Nothing` if any is `Nothing` (`Type.hs:692-693`) |
| `TUnion _ fs` | `maximum` of field widths (**partial**: `maximum` of the empty list throws) |

C++ agrees: struct = sum, union = max (`cpp/parse_tree.cpp:547-572`); array = element ×
count (`cpp/parse_tree.cpp:445`); bool = 1 (`cpp/parse_tree.h:659`); void = 0
(`cpp/parse_tree.h:608`).

Hard cap on any register width: `c_maxRegisterBitWidth = 1024 * 1024` (`cpp/ir.h:39`),
enforced in `BinaryOpNode::TypeCheck` (`cpp/parse_tree.cpp:3218-3225`,
`cpp/parse_tree.cpp:3219-3224`).

### 1.7 `clog2`

```haskell
clog2 n | isPow2 n  = integerLog2 n
        | otherwise = integerLog2 n + 1
  where isPow2 x = x .&. (x - 1) == 0
```
`Internal.hs:53-57`. So `clog2 1 = 0`, `clog2 2 = 1`, `clog2 3 = 2`, `clog2 4 = 2`,
`clog2 5 = 3`. It is **undefined for `n <= 0`** (`integerLog2` is partial); every call site
guards with `fromNaturalConstexpr` (`> 0`, `Type.hs:1102-1103`) or an explicit sign test.

Rust equivalent for `n > 0`: `if n.is_power_of_two() { n.bits() - 1 } else { n.bits() }`.

C++ uses `Log2RoundUp` and `GetIntegerBitCount(x) = if x == 0 then 1 else Log2RoundUp(x+1)`
(`cpp/parse_tree.cpp:218-228`).

### 1.8 Truncation helpers

```haskell
truncateUInt n x = x .&. ((1 << n) - 1)
truncateInt n x  = let w = n - 1 in
    if 0 == (x .&. (1 << w)) then truncateUInt w x
                             else -(1 << w) + truncateUInt w x
```
`Internal.hs:34-41`. `truncateInt` is a sign-extending two's-complement truncation into
`n` bits.

C++ `StringToMpInt` masks to the type width but does **not** sign-extend
(`cpp/parse_tree.cpp:17-35`) — the C++ side stores the raw masked magnitude.

---

## 2. The bit-width algebra

### 2.1 Dispatch order

`infer (BinaryF op a b)` (`Type.hs:1182-1188`):

1. If **both** operands are int literals or `const` enums →
   `TUnresolved` (the value is left to constant folding, `Constexpr.hs:171-202`).
   Precisely: `(isIntLiteral a || (tIsConst (typeOf a) && tIsEnum (typeOf a))) && (…b…)`.
2. Else if either operand's type is unresolved → `TUnresolved`.
3. Else `inferBinaryOp op a b` (`Type.hs:756`).

`inferBinaryOp` first tries the literal-specialised rules (`Type.hs:759-772`), then falls
through to `tBinaryOp` (`Type.hs:775-798`).

Both generic and literal paths use `typeOfOperand`, which **strips `TEnum`, `TInstance` and
`TConst` recursively** (`Type.hs:732-738`). So enums participate in arithmetic as their base
integer type, and `const`ness is erased.

### 2.2 Generic binary operator table

`x`/`y` below are the *stripped* operand types (`typeOfOperand`).

| Operator (surface) | `BinaryOp` | Operand types | Result type | Cite |
|---|---|---|---|---|
| `+` | `Add` | `uintX`,`uintY` | `uint(1 + max X Y)` | `Type.hs:854` |
| `+` | `Add` | `intX`,`intY` | `int(1 + max X Y)` | `Type.hs:855` |
| `+` | `Add` | `uintX`,`intY` | `int(1 + max (X+1) Y)` (lhs promoted by `tSigned`) | `Type.hs:856`, `Type.hs:551` |
| `+` | `Add` | `intX`,`uintY` | `int(1 + max X (Y+1))` | `Type.hs:857` |
| `+` | `Add` | `float`,`float` | `float` | `Type.hs:858` |
| `+` | `Add` | anything else | ``TError "Invalid operands to binary expression: `X` and `Y`"`` | `Type.hs:859`, `Type.hs:543-544` |
| `-` | `Sub` | `uintX`,`uintY` | **`int(1 + max X Y)` — ALWAYS SIGNED** | `Type.hs:861` |
| `-` | `Sub` | every other case | delegates to `tAdd` → `int(...)` for all integer mixes, `float` for floats | `Type.hs:862` |
| `*`, `lutmul` | `Mul`, `LutMul` | `uintX`,`uintY` | `uint(X + Y)` | `Type.hs:864`, `Type.hs:781-782` |
| `*` | `Mul` | `intX`,`uintY` | `int(X + Y)` — **no sign-promotion widening** | `Type.hs:865` |
| `*` | `Mul` | `uintX`,`intY` | `int(X + Y)` | `Type.hs:866` |
| `*` | `Mul` | `intX`,`intY` | `int(X + Y)` | `Type.hs:867` |
| `*` | `Mul` | `float`,`float` | `float` | `Type.hs:868` |
| `/` | `Div` | both integer | **`x` — the entire LHS type, width and signedness unchanged** | `Type.hs:871` |
| `%` | `Mod` | `uint_`, `y` integer | `uint(width y)` | `Type.hs:874` |
| `%` | `Mod` | `int_`, `y` integer | `int(width y)` | `Type.hs:875` |
| `&` | `BitwiseAnd` | `uintX`,`uintY` | **`uint(min X Y)`** | `Type.hs:882-883` |
| `&` | `BitwiseAnd` | other integer pairs | `tSuper x y` (§4) | `Type.hs:884-885` |
| `\|` | `BitwiseOr` | both integer | `tSuper x y` | `Type.hs:878-879` |
| `^` | `BitwiseXor` | both integer | `tSuper x y` — **`BitwiseXor` is routed to `tBitwiseOr`** | `Type.hs:786-787`, `Type.hs:878` |
| `>>` | `ShiftRight` | both integer | **`x` — LHS type unchanged** | `Type.hs:891` |
| `<<` | `ShiftLeft` | both integer, `width y < 20` | **`tWidenInt x (2^width(y) - 1)`**, i.e. `uint(X + 2^Wy - 1)` / `int(X + 2^Wy - 1)` | `Type.hs:894-897`, `Type.hs:801-802` |
| `<<` | `ShiftLeft` | both integer, `width y >= 20` | `TError "Invalid shift left. Result type exceeds maximum bit width."` | `Type.hs:896` |
| `<<`, `>>` | | non-integer operand | ``TError "Invalid operands to binary expression: …"`` | `Type.hs:892`, `Type.hs:895` |
| `==`,`!=`,`>`,`>=`,`<`,`<=` | `Equal`…`LessEqual` | **any** | **`TBoolean` unconditionally — the front-end performs no operand check at all** | `Type.hs:790-795` |
| `&&`,`\|\|`,`^^` | `And`,`Or`,`Xor` | `bool`,`bool` | `bool` | `Type.hs:796-798`, `Type.hs:888` |
| `&&`,`\|\|`,`^^` | | otherwise | ``TError "Invalid operands to binary expression: …"`` | `Type.hs:889` |

`tWidenInt (TUnsigned x) n = TUnsigned (x+n)`, `tWidenInt (TSigned x) n = TSigned (x+n)`,
otherwise ``TError "Invalid operand, expected an integer: `x`"`` (`Type.hs:801-803`,
`Type.hs:546-547`).

`tNarrowInt (TUnsigned x) n = TUnsigned (max 1 (x-n))`,
`tNarrowInt (TSigned x) n = TSigned (max 2 (x-n))` (`Type.hs:805-807`). Note the asymmetric
floors: unsigned clamps at 1 bit, signed clamps at 2 bits.

`tSigned` (used for promotion): `TSigned x → TSigned x`; `TUnsigned x → TSigned (x+1)`;
unresolved → `TUnresolved`; otherwise `TError "Expected an integer"` (`Type.hs:550-554`).

### 2.3 Literal-specialised binary rules

Guards, in `inferBinaryOp` (`Type.hs:757-773`):

| Op | Guard | Rule applied to |
|---|---|---|
| `BitwiseAnd` | `isUntypedIntLiteral b`, else `isUntypedIntLiteral a` (commutative) | `tAndLiteral` on the **other** operand's stripped type |
| `Mul` | `isUntypedIntLiteral b`, else `isUntypedIntLiteral a` (commutative) | `tMulLiteral` |
| `Div` | `isIntLiteral b` only (typed **or** untyped) | `tDivLiteral` |
| `Mod` | `isIntLiteral b` only | `tModLiteral` |
| `ShiftLeft` | `isIntLiteral b` only | `tShiftLLiteral` |
| `ShiftRight` | `isIntLiteral b` only | `tShiftRLiteral` |

`isIntLiteral` sees through `EnumValue` (`Type.hs:1087-1090`); `isUntypedIntLiteral`
likewise, and requires the literal to have **no** `LiteralType` suffix (`Type.hs:1092-1095`).
`tBinaryOpLiteral fn x y = fn (typeOfOperand x) (fromJust (fromIntegralConstexpr y))`
(`Type.hs:775`) — the `fromJust` is safe only because of the guards above.

#### `tAndLiteral` — `x & n` (`Type.hs:821-829`)

| `x` | `n` | result |
|---|---|---|
| `uintX` | `n == 0` | `uint1` |
| `uintX` | `n < 0` | `uintX` (unchanged) |
| `uintX` | `n > 0` | `uint(min X (clog2 (n+1)))` |
| `intX` | `n == 0` | `int2` |
| `intX` | `n < 0` | `int(max X (1 + clog2 (1-n)))` |
| `intX` | `n > 0` | `int(1 + clog2 (n+1))` — **not** clamped by `X`; can *widen* |
| non-integer | any | ``TError "Invalid operand, expected an integer: `x`"`` |

#### `tMulLiteral` — `x * n` (`Type.hs:831-835`)

| condition | result |
|---|---|
| `x` not integer | ``TError "Invalid operand, expected an integer: `x`"`` |
| `n == 0` | `int2` if `x` signed, else `uint1` |
| `n < 0` | `tMulLiteral (tNeg x) |n|` — `tNeg` gives `int(width x + 1)` (`Type.hs:565-568`) |
| `n > 0` | `tWidenInt x (clog2 n)` — so `x * 1` keeps the width (`clog2 1 = 0`) |

#### `tDivLiteral` — `x / n` (`Type.hs:837-841`)

| condition | result |
|---|---|
| `x` not integer | ``TError "Invalid operand, expected an integer: `x`"`` |
| `n == 0` | `TError "Division by zero is undefined"` |
| `n < 0` | `TError "Division with negative denominator is undefined"` |
| `n > 0` | `tNarrowInt x (clog2 n)` — `uint(max 1 (X - clog2 n))` / `int(max 2 (X - clog2 n))` |

#### `tModLiteral` — `x % n` (`Type.hs:810-819`)

| condition | result |
|---|---|
| `n == 0` | `TError "Remainder by zero is undefined"` |
| `n < 0` | `TError "Remainder with negative denominator is undefined"` |
| `uintX`, `n == 1` | `uint1` |
| `uintX`, `n > 1` | `uint(min X (clog2 n))` |
| `intX`, `n == 1` | `int2` |
| `intX`, `n > 1` | `int(min X (1 + clog2 n))` |
| non-integer | ``TError "Invalid operand, expected an integer: `x`"`` |

#### `tShiftLLiteral` / `tShiftRLiteral` (`Type.hs:843-850`)

`x << n` → `tWidenInt x n` (widen by exactly `n`, not `2^Wy - 1`).
`x >> n` → `tNarrowInt x n`.
Non-integer `x` → ``TError "Invalid operand, expected an integer: `x`"``.
`n` is taken as a Haskell `Int` via `fromIntegralConstexpr`; a **negative** literal shift
count is not rejected here and would narrow/widen in the wrong direction (**UNVERIFIED**
whether any earlier pass rejects `x << -1`; the parser parses `-1` as `Unary Neg`, and
`Constexpr.hs:257-258` folds it into a negative `IntLiteral`, so the case is reachable).

### 2.4 Unary operators

`inferUnaryOp` (`Type.hs:740-754`):

| Operator | Operand | Result | Cite |
|---|---|---|---|
| `~` | untyped int literal, stripped type `t` | `tInvert (tSigned t)` → `int(W+1)` for `uintW`, `intW` for `intW` | `Type.hs:742-743`, `Type.hs:550-551`, `Type.hs:561-563` |
| `~` | otherwise | `t` if `tIsInt t`, else ``TError "Invalid operand, expected an integer: `t`"`` | `Type.hs:744`, `Type.hs:561-563` |
| `-` | int literal (typed or untyped) | `tSuper (typeOfOperand x) (inferIntLiteralType Nothing (negate value))` | `Type.hs:745-746` |
| `-` | `float` | `float` | `Type.hs:747`, `Type.hs:565` |
| `-` | `intW` | `int(W+1)` | `Type.hs:566` |
| `-` | `uintW` | `int(W+1)` | `Type.hs:567` |
| `-` | otherwise | `TError "Expected an integer or a float operand"` | `Type.hs:568` |
| `!` | `bool` | `bool` | `Type.hs:748`, `Type.hs:570` |
| `!` | otherwise | `TError "Expected a boolean operand"` | `Type.hs:571` |
| `bitsizeof` | any | `TUnresolved` (folded, §7) | `Type.hs:749` |
| `bytesizeof` | any | `TUnresolved` | `Type.hs:750` |
| `clog2` | any | `TUnresolved` | `Type.hs:751` |

Gate: `infer (UnaryF op a)` returns `TUnresolved` unless `typeResolved a`
(`Type.hs:1280-1282`).

### 2.5 Non-arithmetic type-forming expressions

| Expression | Source syntax | Result type | Cite |
|---|---|---|---|
| `concat(e1, …, en)` | `Parser.hs:867` | `uint(Σ tWidth(ei))`; `TUnresolved` if any element has no width | `Type.hs:1199` |
| `a[i]` (rvalue) | `Parser.hs:815` | `tElement (typeOf a)` — see §2.6 | `Type.hs:1180` |
| `a[i]` (lvalue) | `Parser.hs:803-805` | `tElementLValue (typeOf a)` | `Type.hs:1181` |
| `a.f` | `Parser.hs:816` | `tField (typeOf a) f`; for a `TReference` it looks the class up in the symbol map | `Type.hs:1232-1236` |
| `c ? t : f` | desugars to `Mux c (Seq [f, t])` — **index 0 is the *else* branch** | `Type.hs:1237-1239`, `Parser.hs:968` |
| `mux(sel, e0, …, en)` | `Parser.hs:870` | selector is `IntLiteral k` → type of element `k` (`tUnConst`); selector is `BoolLiteral b` → element `fromEnum b`; otherwise `foldr1 tSuper` over all element types (`tMux`) | `Type.hs:1237-1239`, `Type.hs:925-926` |
| `cast<T>(e)` | `Parser.hs:872` | `theType T` (unconditionally; the value type is not checked here) | `Type.hs:1193` |
| `fan_out<N>(e)` | `Parser.hs:868` | `TArray {} (typeOf e) [N]` where `N` is the int constexpr | `Type.hs:1205` |
| `static(e)` | `Parser.hs:869` | `typeOf e` | `Type.hs:1262` |
| `bitoffsetof(T, f)` | `Parser.hs:865` | `TUnresolved`, then folded to an untyped literal | `Type.hs:1189`, `Constexpr.hs:162-163` |
| `byteoffsetof(T, f)` | `Parser.hs:866` | `TUnresolved`, then folded | `Type.hs:1190`, `Constexpr.hs:165-166` |
| `decltype(e)` | `Parser.hs:432` | `TType (typeOf e)` | `Type.hs:1198` |
| function call | `Parser.hs:871` | `elaborateType args (tReturn (typeOf callee))` — see §2.8 | `Type.hs:1219` |
| `{ … }` | `Parser.hs:857-864` | `TInitializer [element types]` | `Type.hs:1227` |
| interpolated string `"… {e} …"` | `Parser.hs:883` | `TString` | `Type.hs:1230` |

There is **no bit-slice operator** in the language: the reserved-word list
(`Lexer.hs:164-218`) contains no `slice`, and postfix `[...]` always parses as array access
(`Parser.hs:812-819`). "Slice" in the plan corresponds to `a[i]` (array index) plus
`concat`.

### 2.6 `tElement` and the ECC trap

`tElement` (`Type.hs:928-943`):

| Input | Output |
|---|---|
| `TArray _ a []` | `a` |
| `TArray attrs a [_]` (last dimension) | if `attrs` contains `TAttr Ecc _ t` → **`tReturn t`** (the return type of the named ECC function); otherwise `a` |
| `TArray attrs a (_:bs)` | `TArray attrs a bs` |
| `TConst a` | `TConst (tElement a)` |
| `TInstance a _ _` | `tElement a` |
| `TType _` | ``TError "Invalid element access; did you forget `typename`"`` |
| unresolved | `TUnresolved` |
| non-array | ``TError "Invalid element access on non-array type `x`"`` |
| otherwise | `TError "Invalid element access"` |

`tElementLValue` (`Type.hs:945-955`) is identical **except** the ECC substitution is absent
(`TArray _ a [_] = a`, `Type.hs:947`). Note the bug-shaped asymmetry at `Type.hs:949`:
`tElementLValue (TConst a) = TConst $ tElement a` — it calls `tElement`, not itself, so a
`const` ECC memory *does* get the ECC substitution on the lvalue path.

`tIsEccMem` detects the attribute (`Type.hs:675-680`).

### 2.7 Cross-check against the C++ middle-end

`BinaryOpNode::TypeCheck` (`cpp/parse_tree.cpp:3004-3224`):

- Bool operands: only `&&`, `||`, `^^`, `==`, `!=` allowed → `bool`; anything else is
  `"Unsupported binary operation on boolean types"` (`cpp/parse_tree.cpp:3025-3040`).
- Mixed signedness for `> >= < <= == != + - & | ^`: both operands are converted to signed
  via `ToSigned` which adds one bit to an unsigned type (`cpp/parse_tree.cpp:3050-3062`,
  `cpp/parse_tree.cpp:165-177`). This matches the Haskell `tSigned` promotion in `tAdd`.
- `*`/`lutmul` deliberately do **not** promote: "The math of mul does not require unsigned
  operands to be converted to signed. For example: `uint8 * int8 = int16`"
  (`cpp/parse_tree.cpp:3071-3074`). Matches `Type.hs:864-867`.
- **`/` and `%` require an unsigned denominator**: `"Denominator of / % operations must be
  unsigned"` (`cpp/parse_tree.cpp:3076-3083`). The Haskell front-end has **no such check**
  for non-literal denominators (`Type.hs:871`, `Type.hs:874-875`); it only rejects negative
  *literal* denominators (`Type.hs:812-813`, `Type.hs:839`). A Rust front-end that wants
  parity with the shipped compiler must reproduce this middle-end diagnostic.
- `-` result: `baseType = BaseType::Int` with fall-through into the `+` case, width
  `max(Wl, Wr) + 1`, and a long comment justifying "Sub always produces a signed value"
  (`cpp/parse_tree.cpp:3243-3253`). Confirms §2.2.
- `& | ^`: `bitCount = max(Wl, Wr)` (`cpp/parse_tree.cpp:3256-3260`). **This differs from
  the Haskell front-end's `uint & uint → uint(min X Y)`** (`Type.hs:882-883`). The
  front-end type wins because `SetType` prefers `_frontEndType` for integer types
  (`cpp/parse_tree.cpp:1244-1267`), and the front-end type is a subtype of the middle-end
  type.
- `<<`: `bitCount = lhsWidth + ((mp_int(1) << rhsWidth) - 1)`
  (`cpp/parse_tree.cpp:3262-3264`). Confirms the `2^width(b) - 1` widening exactly.
- `>>`: `bitCount = lhsWidth` (`cpp/parse_tree.cpp:3266-3268`).
- `*`: `bitCount = Wl + Wr`, signed iff either operand is signed
  (`cpp/parse_tree.cpp:3140-3148`).
- `/`: `bitCount = lhsWidth` (`cpp/parse_tree.cpp:3150-3152`).
- `%`: `bitCount = rhsWidth`, sign follows the LHS; `rhsWidth < 1` is
  `"Right hand size of % operation must be >= 1"` (`cpp/parse_tree.cpp:3154-3170`).
- Float: only `+ - *` are legal; result is `float32`
  (`cpp/parse_tree.cpp:3195-3210`). Note the Haskell front-end also accepts `float / float`
  and `float % float`? No — `tDiv`/`tMod` require `tIsInt` on both sides, so floats fall to
  `tInvalidOperandsError` (`Type.hs:871-876`). Consistent.
- Anything else: `"Binary operations are only supported on scalar types"`
  (`cpp/parse_tree.cpp:3212-3216`).

`UnaryOpNode::TypeCheck` (`cpp/parse_tree.cpp:3315-3355`): `~` keeps the leaf type
(integer only), `-` gives `int(width + 1)` for integers and `float` for floats, `!` requires
bool. Matches §2.4 for the non-literal cases.

`CastNode::TypeCheck` (`cpp/parse_tree.cpp:3491-3519`): if `src` and `dst` have equal bit
width the cast is accepted unconditionally; otherwise `TypeCheckConversion` is applied.
Result type is always `dst`.

`ConcatNode::TypeCheck` (`cpp/parse_tree.cpp:5461-5484`): sums element widths, errors with
`"concat element type must have positive width"` on a zero-width element, and produces
`uint(total)`. Note the front-end (`Type.hs:1199`) does **not** reject zero-width elements
(e.g. `string`, width 0 per `Type.hs:705`).

Ternary/`mux` (`cpp/parse_tree.cpp:2920-2988`): every element must be
`TypeCheckConversion`-compatible with the *first* element; the result is the smallest leaf
type covering all of them, promoting to signed if any element is signed; a mux with more
than 32 arms and at least one signed input is `"MUX is too large"`.

Array index: `TypeCheckArrayIndex` requires an integer index type,
`"Array index must be an integer"` (`cpp/parse_tree.cpp:44-52`). The Haskell front-end
ignores the index expression entirely (`Type.hs:1180`).

---

## 3. Literal typing

### 3.1 Integer literals

Grammar: `integer` accepts decimal, `0b`/`0B` binary, `0o`/`0O` octal, `0x`/`0X`
hexadecimal, with `_` digit separators allowed after the prefix and between digits
(`Lexer.hs:133-155`). An optional type suffix follows immediately:
`i<decimal>` or `u<decimal>` (`Parser.hs:905-908`, using `show SignedLiteral = "i"`,
`show UnsignedLiteral = "u"`, `Syntax.hs:223-225`).

```haskell
inferIntLiteralType (Just (LiteralType SignedLiteral w))   _ = TSigned w
inferIntLiteralType (Just (LiteralType UnsignedLiteral w)) _ = TUnsigned w
inferIntLiteralType Nothing x
    | x == 0    = TUnsigned 1
    | x < 0     = TSigned  (1 + clog2 (abs x))
    | otherwise = TUnsigned (clog2 (x + 1))
```
`Type.hs:485-491`.

Worked values (validated against `test/syntax/literal-operand.k:69-79` and
`test/syntax/decltype.k:21-30`): `0 → uint1`, `1 → uint1`, `2 → uint2`, `3 → uint2`,
`4 → uint3`, `10 → uint4`, `-16 → int5`, `-17 → int6`, `-20 → int6`, `-200 → int9`.

Suffixed literals are validated late, in `validateTypedLiterals` (`Frontend.hs:210-217`):
- ``"Invalid literal type specifier `<i|u><w>`"`` when `w == 0`;
- ``"Literal out of range for `<i|u><w>`"`` when `truncate` changes the value.

### 3.2 Other literals

| Literal | Type | Cite |
|---|---|---|
| `true` / `false` | `TBoolean` | `Type.hs:1191`, `Parser.hs:880-881` |
| float literal | `TFloat` (32-bit) | `Type.hs:1207`, `Lexer.hs:158-159` |
| `"…"` with no interpolation | `TString` (`InterpolatedString` collapses to `StringLiteral`) | `Type.hs:1266`, `Frontend.hs:234` |
| `"… {e} …"` | `TString` | `Type.hs:1230` |

### 3.3 Constant folding and literal re-typing

`Constexpr.hs:171-250` folds binary operations on two `IntLiteral`s. The result's literal
type is:

```haskell
intLiteral | isNothing wx && isNothing wy = untypedIntLiteral
           | otherwise                    = typedIntLiteral $ inferBinaryOp op a b
```
`Constexpr.hs:200-202`. **If both operands are untyped literals the result stays untyped**
(so it keeps re-deriving its minimal width); if either had a suffix the result is stamped
with the width/signedness computed by `inferBinaryOp`.

Unary folding: same rule, keyed on `isUntypedIntLiteral a` (`Constexpr.hs:282-284`).

`~` folding truncates into the appropriate width: enum → base type; untyped literal →
`tSigned` of its type; otherwise its own type (`Constexpr.hs:260-269`).

Folded operations on `IntLiteral`s (`Constexpr.hs:176-198`): `Add Sub Mul LutMul Div Mod
BitwiseAnd BitwiseOr BitwiseXor ShiftLeft ShiftRight Equal NotEqual Greater GreaterEqual
Less LessEqual`. `Div`/`Mod` by zero are
`"Division by zero is undefined"` / `"Remainder by zero is undefined"`
(`Constexpr.hs:181-186`). Haskell `quot`/`rem` semantics (truncate toward zero) apply.

On `BoolLiteral`s: `Equal NotEqual Or And Xor` (`Constexpr.hs:204-210`).
On `StringLiteral`s: `Equal NotEqual` (`Constexpr.hs:212-215`).

Partial-evaluation identities (`Constexpr.hs:229-242`):
- `x & 0 → 0` (untyped) when `sizeOf x <= sizeOf 0`-literal;
- `x | y → y` when every bit of `x`'s width is set in `y`;
- `x || true → true`; `x && false → false`.

Constant *variables* are inlined: a `const` variable of integer/float type initialised with
an int literal is replaced by the literal **cast into the declared type**
(`Constexpr.hs:286-293`, `castIntegralConstexpr` at `Constexpr.hs:329-335`); other `const`
variables initialised with a literal/enum/function are inlined verbatim.
This is why ``const uint32 x = 10; uint8 z = x + y;`` produces **no** conversion warning
(`test/syntax/conversion-warning.k:118-127`): the sum folds to a literal whose minimal type
is `uint4`.

`cast<T>(literal)` is folded (`Constexpr.hs:307-309`): integer→int truncates
(`truncateInt`/`truncateUInt`), integer→float **reinterprets the bits**
(`castWord32ToFloat`, `Constexpr.hs:332`), float→integer reinterprets
(`castFloatToWord32`, `Constexpr.hs:338-339`).

---

## 4. Subtyping `<:` and least upper bound

### 4.1 `(<:)` — exact definition

`Type.hs:317-352`, in clause order (first match wins):

```
1.  TError{}    <: _            = False
2.  _           <: TError{}     = False
3.  s <: t                      = True             when s == t   (structural equality)
4.  TUnsigned x <: TUnsigned y  = x <  y           -- STRICT
5.  TUnsigned x <: TSigned y    = x <  y           -- STRICT
6.  TSigned x   <: TSigned y    = x <  y           -- STRICT
7.  TConst s    <: t            = s <: t
8.  s           <: TConst t     = s <: t
9.  TInstance…  <: t            = tConcrete s <: t
10. s           <: TInstance…   = s <: tConcrete t
11. TEnum _ s   <: t            = s <: t
12. s           <: TDependent t = s <: t
13. TFunction _ a1 r1 p1 <: TFunction _ a2 r2 p2 =
        (r2 == TAuto || r1 <: r2)                       -- covariant return
     && liftEq ((<:) `on` typeOfParam) p2 p1            -- CONTRAvariant params
     && liftEq (Set.isSubsetOf `on` attrOfParam) p2 p1  -- param attrs of p2 ⊆ p1
     && (a2 `Set.isSubsetOf` a1)                        -- fn attrs of t ⊆ s
14. TInitializer [] <: TBoolean = True
15. TInitializer [] <: TFloat   = True
16. TInitializer [] <: t        = True   when tIsInt t
17. TInitializer [] <: t        = True   when tIsEnum t
18. TInitializer{}  <: t        = tAggregateInitializer (<:) s t
19. TClosure n1 f1 s _ <: TClosure n2 f2 t _ = n1 == n2 && f1 <: f2 && s <: t
20. TArray a1 _ ds <: TArray a2 _ (d2:_) =
        a1 == a2                       -- attribute sets must be EQUAL
     && (null ds || head ds == d2)     -- first dimension must match (or be unknown)
     && tElement s <: tElement t
     && tElement t <: tElement s       -- element type INVARIANT
21. TAuto <: _                  = True
22. _ <: _                      = False
```

Declared `infixr 4` (`Type.hs:352`).

**The relation on integers is STRICT (`x < y`, not `x <= y`)** — clauses 4, 5, 6
(`Type.hs:321-323`). Reflexivity is supplied only by clause 3 (`s == t`). Consequences:

- `uint8 <: uint8` is **True** (via clause 3), but only because the two types are literally
  equal; `uint8 <: int8` is **False** (`8 < 8` is false) even though every `uint8` value fits
  in an `int8` in the C++ sense (`cpp/parse_tree.cpp:203-207` treats all leaf types as
  mutually convertible).
- `uint8 <: int9` is True.
- `int8 <: uint9` is **False** — clause order means a `TSigned` LHS with a `TUnsigned` RHS
  falls through to clause 22.
- Verified by the `static assert` battery in `test/syntax/type-constexpr.k:34-56`:
  `uint32 > uint30` → true, `uint32 >= uint30` → true, `uint32 < uint30` → false,
  `uint32 == int32` → false.

Other traps:
- **`TAuto <: anything` is True** (clause 21) but `anything <: TAuto` is False (falls to
  clause 22) unless the types are equal.
- **Arrays are invariant in the element type** and require *equal* attribute sets, so a
  `[[memory]] T[N]` is not a subtype of a plain `T[N]`.
- **Function subtyping is contravariant in parameters**: `p2 <: p1` element-wise, i.e. the
  *target*'s parameters must be subtypes of the *source*'s. `liftEq` also requires equal
  arity.
- `r2 == TAuto` short-circuits the return check (a callback declared `-> auto` accepts any
  return type).
- Function attribute sets: the target's attrs must be a subset of the source's.

The C++ side has a much cruder rule: `TypeCheckConversion` accepts *any* leaf/enum → any
leaf/enum, recurses structurally on function types (swapping src/dst for parameters —
`cpp/parse_tree.cpp:229-237`), and otherwise requires pointer identity
(`cpp/parse_tree.cpp:199-249`).

### 4.2 `tIsConvertibleTo`

`Type.hs:354-366`:

```
1. s `conv` t  = True                        when s <: t
2. TConst s    `conv` t         = s `conv` t
3. s           `conv` TConst t  = s `conv` t
4. TInstance…  `conv` t         = tConcrete s `conv` t
5. s           `conv` TInstance…= s `conv` tConcrete t
6. TInitializer{} `conv` t      = tAggregateInitializer tIsConvertibleTo s t
7. TEnum _ s   `conv` t         = s `conv` t
8. s           `conv` TDependent t = s `conv` t
9. s `conv` t  = True                        when tIsInt s && tIsInt t
10. _ `conv` _ = False
```

Clause 9 is the "narrowing is possible" escape hatch: **any integer is convertible to any
other integer**, which is exactly the region in which the `conversion` warning fires (§6).

### 4.3 Aggregate initializer compatibility

`tAggregateInitializer fn s t` (`Type.hs:306-314`):
- `TInitializer ts` against a union with `length ts > 1` → False.
- `TInitializer ts` against union/struct/class/array → `all (\ti -> fn (tInitializerType ti)
  (tInitializedType t ti)) ts`.
- anything else → False.

`tInitializerType` extracts the payload type from `TDesignator`/`TPositional`
(`Type.hs:288-291`; **partial** — `undefined` on anything else).

`tInitializedType t elem` (`Type.hs:293-304`) — what the target type expects at that slot:
- strips `TConst` from `t`;
- `TPositional x _` against an array whose first dimension equals `x` →
  `TError "Excess elements in array initializer"`;
- `TPositional x _` against a union with `x /= 0` →
  `TError "Excess elements in union initializer"`;
- `TPositional _ _` against an array → `tElement t`;
- `TPositional x _` against union/struct/class → `tFieldAt t x` (field by index;
  out of range → ``TError "Excess element in initializer of type `t`"``, `Type.hs:280-282`);
- `TDesignator x _` against union/struct/class → `tField t x`;
- `TPositional` against anything else →
  ``TError "Non-empty initializer list cannot initialize type `t`"``;
- `TDesignator` against anything else →
  ``TError "Member designator cannot initialize type `t`"``.

### 4.4 `tSuper` — least upper bound

`Type.hs:899-923`, in clause order:

| # | Shape | Result |
|---|---|---|
| 1 | `x` not resolved | `TUnresolved` |
| 2 | `y` not resolved | `TUnresolved` |
| 3 | `x == y` | `x` |
| 4 | `TUnsigned x`, `TUnsigned y` | `TUnsigned (max x y)` |
| 5 | `TSigned x`, `TSigned y` | `TSigned (max x y)` |
| 6 | `TSigned`, `TUnsigned y` | `tSuper x (tSigned y)` → `TSigned (max Wx (Wy+1))` |
| 7 | `TUnsigned x`, `TSigned` | `tSuper (tSigned x) y` → `TSigned (max (Wx+1) Wy)` |
| 8 | `TEnum` on the left | `tSuper (tEnumBase x) y` |
| 9 | `TEnum` on the right | `tSuper x (tEnumBase y)` |
| 10 | `TInstance x _ _` left | `tSuper x y` |
| 11 | `TInstance y _ _` right | `tSuper x y` |
| 12 | `TPositional l x`, `TPositional r y`, `l == r` | `TPositional l (tSuper x y)` |
| 13 | `TDesignator l x`, `TDesignator r y`, `l == r` | `TDesignator l (tSuper x y)` |
| 14 | `TInitializer` left, `x tIsConvertibleTo y` | `y` |
| 15 | `TInitializer` right, `y tIsConvertibleTo x` | `x` |
| 16 | both `TInitializer xs`/`ys` | element-wise `tSuper`, padded with the tail of the longer list |
| 17 | otherwise | ``TError "Types `x` and `y` have no common supertype"`` |

Note there is **no** `TConst` clause: `tSuper (TConst a) b` with `a /= b` falls straight to
clause 17. All callers pre-strip `const`: `tMux` is applied to `tUnConst . typeOf`
(`Type.hs:1239`), and `inferBinaryOp` uses `typeOfOperand` (`Type.hs:732-738`).
There is also no `TBoolean`/`TFloat`/`TString`/`TStruct` clause — those only unify with
themselves via clause 3.

`tMux = foldr1 tSuper` (`Type.hs:925-926`) — **partial** on an empty list.

---

## 5. Error types

`TError` values produced by the type layer (complete, in file order):

| Message | Cite |
|---|---|
| ``"Excess element in initializer of type `T`"`` | `Type.hs:280-281` |
| `"Excess elements in array initializer"` | `Type.hs:297` |
| `"Excess elements in union initializer"` | `Type.hs:298` |
| ``"Non-empty initializer list cannot initialize type `T`"`` | `Type.hs:300` |
| ``"Member designator cannot initialize type `T`"`` | `Type.hs:303` |
| ``"Invalid operands to binary expression: `X` and `Y`"`` | `Type.hs:543-544` |
| ``"Invalid operand, expected an integer: `X`"`` | `Type.hs:546-547` |
| `"Expected an integer"` (from `tSigned`) | `Type.hs:554` |
| `"Expected an enum"` | `Type.hs:559` |
| `"Expected an integer or a float operand"` | `Type.hs:568` |
| `"Expected a boolean operand"` | `Type.hs:571` |
| `"Undefined function"` | `Type.hs:582` |
| `"Expected a type"` (from `theType`) | `Type.hs:460` |
| `"Remainder by zero is undefined"` | `Type.hs:811` |
| `"Remainder with negative denominator is undefined"` | `Type.hs:812` |
| `"Division by zero is undefined"` | `Type.hs:839` |
| `"Division with negative denominator is undefined"` | `Type.hs:840` |
| `"Invalid shift left. Result type exceeds maximum bit width."` | `Type.hs:896` |
| ``"Types `X` and `Y` have no common supertype"`` | `Type.hs:923` |
| ``"Invalid element access; did you forget `typename`"`` | `Type.hs:939`, `Type.hs:951` |
| ``"Invalid element access on non-array type `X`"`` | `Type.hs:942`, `Type.hs:954` |
| `"Invalid element access"` | `Type.hs:943`, `Type.hs:955` |
| ``"Invalid member access on type `X`"`` | `Type.hs:967` |
| `"The iteration count of `static for` can't be negative"` | `Type.hs:1116` |
| ``"Conflicting template argument deduction results for parameter `P`. Could be `X` or `Y`."`` | `Type.hs:1500-1502` |

A `TError` on a node is converted into a real diagnostic by the `typeErrors` pass, which
emits `showPretty` of the type as the message unless the node already carries errors
(`Frontend.hs:169-174`).

---

## 6. Implicit conversions and the `conversion` warning

### 6.1 Where conversions are checked

All checks live in `validateProgram` (`TypeCheck.hs:33-216`), which runs as the **last**
front-end pass (`Frontend.hs:55-56`; passes are applied via `foldr`, so the list is executed
bottom-up and this entry runs last).

Three "narrowing continuations" (`TypeCheck.hs:101-103`):

```haskell
warningNarrowingConversions s t = warningMsg Conversion
    ("Narrowing conversion from `" ++ showUnmangled s ++ "` to `" ++ showUnmangled t ++ "`")
allowNarrowingConversions _ _   = id
errorNarrowingConversions s t   = conversionError s t
    ("\nInsert an explicit cast to silence this issue `cast<" ++ showUnmangled t ++ ">()`")
```

and the base error (`TypeCheck.hs:99`):

```haskell
conversionError s t hint = errorMsg
    (("Invalid conversion from `" ++ showUnmangled s ++ "` to `" ++ showUnmangled t ++ "`") ++ hint)
```

`showUnmangled = show . unmangle symbols` (`TypeCheck.hs:36`) — the *unmangled* rendering,
which converts template-instance mangled names back to `Name<args>` form
(`Type.hs:137-176`).

Complete list of call sites:

| Site | Continuation | Cite |
|---|---|---|
| function-call arguments | `warningNarrowingConversions` | `TypeCheck.hs:97` |
| variable initialiser (normal case) | `warningNarrowingConversions` | `TypeCheck.hs:131` |
| variable initialiser that is a **designated** initializer list | `errorNarrowingConversions` | `TypeCheck.hs:130` |
| variable of **function type** whose initialiser is not a subtype | `errorNarrowingConversions` | `TypeCheck.hs:137` |
| the value of a `.field = e` designator, at any depth | `errorNarrowingConversions` | `TypeCheck.hs:157` |
| `return` statements | `allowNarrowingConversions` (**never warns**) | `TypeCheck.hs:84` |

`checkArguments` skips the **first** argument entirely for `[[pipelined]]` functions
(`TypeCheck.hs:92-93`), and errors with
`"Invalid number of arguments, expected N"` on arity mismatch (`TypeCheck.hs:91`).

### 6.2 The conversion algorithm

`checkConversion` (`TypeCheck.hs:152-159`) normalises the *source* type before dispatch:

1. `t` is `TError` → `errorMsg (showPretty t)`.
2. source is an `IntLiteral _ x` → source type is recomputed as
   `inferIntLiteralType Nothing x`, i.e. the **minimal** type for the value, ignoring any
   suffix the literal carries.
3. source is `EnumValue _ x` and target is integer → recurse on the inner literal (the enum
   wrapper is discarded).
4. source is a `Designator x y` → recurse into `y` with `errorNarrowingConversions`.
5. source is a `Positional x y` → recurse into `y` with the same continuation.
6. otherwise → `typeOf a`.

`checkConversion'` (`TypeCheck.hs:161-178`):

1. the node already has errors → unchanged.
2. `s` is `TError` → `errorMsg (showPretty s)`.
3. **`s <: t` → accepted silently.**
4. source is an initializer list → recurse element-wise using
   `tInitializedType t (typeOf elem)`; for a union target, the first element is checked and
   every subsequent element becomes `"Excess elements in union initializer"`
   (`TypeCheck.hs:165-175`).
5. `s tIsConvertibleTo t` → apply the narrowing continuation (warn / error / nothing).
6. otherwise → ``"Invalid conversion from `s` to `t`"`` with no hint.

So the **`conversion` warning fires exactly when**: `not (s <: t)` **and**
`s tIsConvertibleTo t` **and** the site uses `warningNarrowingConversions`. Given §4.2, in
practice that means both types are integers (possibly reached through `const`, enum base,
`TInstance` or `TDependent` unwrapping) and the strict-`<` subtype test failed.

Corollaries confirmed by `test/syntax/conversion-warning.k`:
- `int32 → uint32` warns (`:1-14`), because `TSigned <: TUnsigned` has no clause.
- `uint2 ← enum E : uint3` warns (`:59-72`); `uint2 ← enum E : uint2` does not (`:74-87`)
  because `TEnum _ (TUnsigned 2) <: TUnsigned 2` reduces to equality.
- `uint3 x = Foo::One` where `Foo : uint16` does **not** warn (`:159-169`) — step 3 of
  `checkConversion` unwraps the `EnumValue` to the literal `1`, whose minimal type is `uint1`.
- but `auto x = Foo::One; uint3 y = x;` **does** warn (`:186-198`) — `x` has type
  `const Foo`, not a literal, so `TEnum _ (TUnsigned 16) <: TUnsigned 3` fails.
- `uint8 z = x + y` with `uint32 x, y` warns (`:106-116`); with `const uint32` it does not
  (`:118-127`) because the sum constant-folds.

### 6.3 Rendered diagnostic text

`warningMsg kind msg` builds `warning kind` with offset `-1` (`Error.hs:153-154`), and

```haskell
message = "warning: " ++ msg ++ " [--" ++ disableSwitch kind ++ "]"
```
`Error.hs:142-145`, with `disableSwitch = ("Wno-" ++) . show` (`Warning.hs:43-44`) and
`show Conversion = "conversion"` (`Warning.hs:31`).

**Exact emitted text** for a narrowing at a call site or variable initialiser:

```
warning: Narrowing conversion from `int32` to `uint32` [--Wno-conversion]
```

**Exact emitted text** for the fix-it error form:

```
Invalid conversion from `S` to `T`
Insert an explicit cast to silence this issue `cast<T>()`
```

(the newline is embedded in the message, `TypeCheck.hs:103`).

### 6.4 Warning kinds and defaults

`WarningKind` = `Conversion, Deprecated, InitializerOverrides, RedundantImport, Shadow,
TransactionSizeMissing` (`Warning.hs:20-27`), spelled `conversion, deprecated,
initializer-overrides, redundant-import, shadow, transaction-size` (`Warning.hs:29-35`).
Switches are `--W<name>` / `--Wno-<name>` (`Warning.hs:40-44`).

Defaults (`CmdArgs.hs:111-133`): `RedundantImport, Deprecated, InitializerOverrides,
Shadow` are **on**; `Conversion` and `TransactionSizeMissing` are **off** and are enabled by
`--Wall` or by `--Wconversion` individually. `--Werror` promotes warnings to errors
(`CmdArgs.hs:110`).

Other warnings emitted by the type checker:
- `InitializerOverrides`: ``"Initializer overrides prior initialization of `f`"``
  (`TypeCheck.hs:60-66`).
- `Shadow`: ``"This declaration of `x` shadows the existing declaration"``
  (`TypeCheck.hs:110-121`); only for named-scope declarations that are not class/member
  declarations, and only if the shadowed symbol is a scoped `Variable` or a `FuncParam`.

### 6.5 Other declaration-level checks in `validateProgram`

Complete list of `check` clauses (`TypeCheck.hs:38-58`) and their errors:

| Clause | Diagnostic |
|---|---|
| `UnaryF BitSizeof`, `UnaryF ByteSizeof` | exempt from the "type as expression" check |
| other `UnaryF`, `BinaryF`, `FanOutF`, `MuxF`, `ParamIntF`, `ParamUintF` | `"Invalid use of a type as an expression"` on any operand for which `isType` holds (`TypeCheck.hs:180-185`) |
| `FunctionCallF` | argument conversion + arity (`TypeCheck.hs:87-97`) |
| `ArrayF` | `"Array dimension must be a positive integer"` (`TypeCheck.hs:187-196`) |
| `IntAttrF` | `"Expected a constant expression that evaluates to non-negative integer"` (`TypeCheck.hs:198-199`) |
| `EnumF` | `"Enums must be based on an integral type"`; `"Enum constant value is out of range for the base type"`; `"Enum constant value doesn't evaluate to an integer constant"` (`TypeCheck.hs:201-216`) |
| `VariableF` | shadow warning + `checkVarDecl` (below) |
| `AutoF` | `"Could not deduce type"` (`TypeCheck.hs:51`) |
| `LambdaF` | `"Invalid use of lambda expression in this context"` (`TypeCheck.hs:52`) |
| `DecltypeF` | `"Couldn't deduce type of the expression"` (`TypeCheck.hs:53`) |
| `AssignF` | ``"`e`: illegal use of this constant as an lvalue"`` (`TypeCheck.hs:105-108`) |
| `FunctionF` | `[[reset]]` checks: `"Function with [[reset]] attribute can't have parameters"`, `"Function with [[reset]] must have void return type"` (`TypeCheck.hs:80-83`) |
| `DefaultInitializationF` | `"Expected designated initializer"` (`TypeCheck.hs:69-71`) |
| `InitializerListF` | duplicate-designator warning |

`checkVarDecl` (`TypeCheck.hs:123-144`), in order:
- resolved, non-`const`, `DeclareGlobal` → `"Global state is not allowed"`;
- ECC memory with an initialiser → `"ECC memories with initial values are not supported"`;
- global closure variable → capture must be zero-width, else
  `"Closures that capture value(s) are not allowed in global scope"` (`TypeCheck.hs:146-148`);
- designated-initializer init → error-narrowing conversion check;
- both types resolved, source not an error, target not a function type → warning-narrowing check;
- function-typed, no initialiser, `const` → `"A constant of a function type must be initialized"`;
- function-typed, initialiser not a subtype → error-narrowing check;
- function-typed, non-`const`, not a class member →
  `"Expected 'const'. Mutable variables of a function type are not allowed"`;
- function-typed `const` non-class variable → erased (`SeqF []`);
- function-typed class member declared through a type alias → the alias is expanded into an
  explicit function type ("middle-end doesn't handle function type aliases",
  `TypeCheck.hs:141-142`).

---

## 7. `decltype`, `bitsizeof`, `bitoffsetof`, `clog2`, type comparison

All of these are **inference-stage `TUnresolved`, constant-folded in `evaluateConstexpr`**,
and finally turned into errors by the `unresolved` desugar pass if they never folded.

| Construct | Syntax | Inference | Folding | Failure |
|---|---|---|---|---|
| `decltype(e)` | `Parser.hs:432` | `TType (typeOf e)` (`Type.hs:1198`) | replaced by the type expression via `expFromType` (`Constexpr.hs:304-305`) | ``"Argument of type `T` is illegal for `decltype`"`` (`Constexpr.hs:345`); plus `"Couldn't deduce type of the expression"` if a `DecltypeF` survives to `validateProgram` (`TypeCheck.hs:53`) |
| `bitsizeof e` | prefix keyword (`Parser.hs:948`, `Lexer.hs:170`) | `TUnresolved` (`Type.hs:749`) | untyped int literal of `sizeOf e` (`Constexpr.hs:271-272`) | node stays unresolved |
| `bytesizeof e` | `Parser.hs:948` | `TUnresolved` (`Type.hs:750`) | `sizeOf e / 8`, but ``"bytesizeof is undefined for a type with bit size of N"`` if the width is not a multiple of 8 (`Constexpr.hs:274-275`, `Constexpr.hs:349-351`) | |
| `clog2(e)` | `Parser.hs:948` | `TUnresolved` (`Type.hs:751`) | untyped literal `clog2 n` for `n > 0` (`Constexpr.hs:168-169`) | `"clog2 argument did not evaluate to a positive integer constant"` (`Frontend.hs:403`) |
| `bitoffsetof(T, f)` | `Parser.hs:865` | `TUnresolved` (`Type.hs:1189`) | untyped literal `offsetOf f T` (`Constexpr.hs:162-163`) | ``"Illegal operands for bitoffsetof: T, f"`` (`Constexpr.hs:347`); or `"Undefined field"` / `"Field offset is only defined for structs"` (`Frontend.hs:446-449`) |
| `byteoffsetof(T, f)` | `Parser.hs:866` | `TUnresolved` (`Type.hs:1190`) | offset/8, else `"byteoffsetof is undefined for fields with bit offset of N"` (`Constexpr.hs:165-166`) | as above |
| `int<e>` / `uint<e>` | `Parser.hs:544-545` | `TType (TSigned n)` / `TType (TUnsigned n)` when `e` is an int constexpr (`Type.hs:1246-1247`) | `ParamIntF`→`IntegerF`, `ParamUintF`→`UnsignedF` for `n > 0` (`Constexpr.hs:298-302`) | `"int width did not evaluate to a positive integer constant"` / `"uint width …"` (`Frontend.hs:404-405`) |
| `static if` | `Parser.hs:594` | `TUnresolved` (`Type.hs:1263`) | branch selected by `staticControl` (`Constexpr.hs:44-48`) | `"static if predicate did not evaluate to a boolean constant"` (`Frontend.hs:406`) |
| `static assert(e)` | `Parser.hs:571-574` | `TUntyped` | `SeqF []` on `true`, `"static assertion failed"` on `false` (`Frontend.hs:393-398`) | `"static assertion expression did not evaluate to a boolean constant"` |

### 7.1 Type-comparison operators

When **both** operands of a comparison are types (`tIsType (typeOf x) && tIsType (typeOf y)`)
the comparison is folded into a `BoolLiteral` using `<:` (`Constexpr.hs:223-249`):

| Operator | Folded to | Cite |
|---|---|---|
| `s == t` | `s <: t && t <: s` | `Constexpr.hs:244` |
| `s != t` | `not (s <: t) \|\| not (t <: s)` | `Constexpr.hs:245` |
| `s <= t` | `s <: t` | `Constexpr.hs:246` |
| `s <  t` | `s <: t && not (t <: s)` | `Constexpr.hs:247` |
| `s >= t` | `t <: s` | `Constexpr.hs:248` |
| `s >  t` | `t <: s && not (s <: t)` | `Constexpr.hs:249` |
| any other operator | left alone | `Constexpr.hs:250` |

Because `<:` is strict on integers and `==` is symmetric-`<:`, `uint32 == uint<32>` is true
(`test/syntax/type-constexpr.k:8`), `uint32 == uint30` is false (`:20-22`),
`uint32 > uint30` is true (`:38-40`), and `uint32 < uint30` is false (`:50-52`).

Note the *asymmetry*: `s == t` here means mutual-subtype, which for two distinct integer
types is impossible (strictness), so it coincides with structural equality for scalars, but
for `TInstance`/`TConst` wrappers it is weaker than `Eq Type`.

### 7.2 Bit offsets

`tOffsetOf field t` (`Type.hs:725-730`):
- `TInstance` → recurse on `tConcrete`;
- `TStruct _ fields` → index of the field, then `tSumWidths (take n fields)` — i.e. the sum
  of the widths of all **preceding** fields;
- everything else (including unions, classes, arrays) → `Nothing`.

So the first declared field is at bit offset 0, and fields are packed LSB-first with **no
padding and no alignment**. Confirmed by
`test/logic/test_cases_26.k:163-164` (`0 == bitoffsetof(Foo406,_x)`,
`40 == bitoffsetof(Foo406,_y)`) and by the C++ `GetCppMemberModifier` walk
(`cpp/parse_tree.cpp:413-437`) which accumulates `currOffset` in declaration order.

---

## 8. `auto` deduction and return types

### 8.1 Variables

`infer` for `VariableF` (`Type.hs:1285-1287`):
- `const auto x = e` → `tConst (typeOf e)`;
- `auto x = e` → `tUnConst (typeOf e)`;
- otherwise → `theType varType`.

`tConst` (`Type.hs:584-589`): idempotent on `TConst`; passes `TError` through; distributes
into `TType`; maps unresolved to `TUnresolved`; otherwise wraps.
`tUnConst` (`Type.hs:591-594`): unwraps `TConst`, distributes into `TType` and into the
concrete slot of `TInstance`.

`deduceAuto` (`Type.hs:1595-1609`) then **rewrites the AST**, replacing the `auto` type
expression with a real type expression built by `expFromType`:
- `FunctionF` with `isAuto funcReturnType` → return type expression from
  `tReturn (typeof n)`; failure → `"Could not deduce function return type"`;
- `VariableF` with `isAuto varType || isConstAuto varType` → type expression from
  `typeof n`; failure → `"Could not deduce variable type"`.

It runs as front-end pass index 2 (`Frontend.hs:58`).

`expFromType` (`Type.hs:1612-1660`) — the inverse map from `Type` back to a type expression.
It is **partial**; it returns `Nothing` for `TTemplate`, `TInitializer`, `TDesignator`,
`TPositional`, `TArray _ _ []` (unknown dimensions), `TError`, `TUndefined`, `TUnresolved`,
`TUntyped` (`Type.hs:1615-1637`). Everything else maps to the obvious syntax; `TClosure`,
`TEnum`, `TStruct`, `TUnion`, `TClass` map to a `TypeIdentifier` for the name;
`TInstance t _ _` maps through to `t`; `TDependent x` maps to `x`.

### 8.2 Function return types

`infer FunctionF{..}` (`Type.hs:1209-1218`):

```haskell
returnType Auto = maybe TVoid typeOf $ find returnExp $ unfix body
  where returnExp StaticIf{} = True
        returnExp Return{}   = True
        returnExp _          = False
returnType a = theType a
```

So for `inline auto f() { … }` the return type is the type of the **first top-level
statement in the body that is a `Return` or a `StaticIf`**, and `TVoid` if there is none.
The body is not searched recursively; a `return` nested inside an `if` or a loop will not be
found (the front-end moves the last statement of `if`/loop constructs to the top level in
`moveFunctions`, `Function.hs:104-118`, but a `return` inside a plain `if` body is not
hoisted). A `StaticIf` has type `TUnresolved` (`Type.hs:1263`) until its predicate folds and
`staticControl` replaces it with the chosen branch, so `auto` + `static if` resolves in a
later fixpoint round.

`ReturnF Nothing → TVoid`, `ReturnF (Just a) → tUnConst (typeOf a)` (`Type.hs:1250-1251`).
`returnVoid` rewrites `return e;` where `e : void` into `e; return;`
(`Frontend.hs:293-296`).

`funcType` (`Type.hs:1364-1373`) assembles the `TFunction`, and wraps the return type in
`TDependent` iff the function carries the `[[pipelined]]` flag.

`tReturn` (`Type.hs:573-582`): `TFunction` returning `TAuto` → `TUnresolved`; `TClosure`,
`TInstance`, `TConst` unwrap; unresolved → `TUnresolved`; `TError` passes through;
otherwise `TError "Undefined function"`.

`elaborateType` at the call site (`Type.hs:1375-1378`) turns `TDependent`:
- `TDependent TVoid` → `TVoid`;
- no arguments → `TArray {} y []` (array of unknown extent);
- with arguments → `TArray {} y [n]` where `n` is the integer constexpr value of the **first
  argument**.

This is why `[[pipelined]]` functions return a *batch array* whose length is the first
argument, and why `checkArguments` skips type-checking that first argument
(`TypeCheck.hs:92-93`). `resolveTypeOf` explicitly allows an unknown-extent array to be
installed as a node type (`Type.hs:1038-1041`).

### 8.3 Return-type polymorphism

`setTypeOfTemplateCall t x` (`Type.hs:1120-1129`): given an *expected* type `t` and a
`FunctionCallF`, it descends through `FunctionSpecifierF` and, when the callee is an
unresolved `QualifiedIdentifier (TemplateInstance …)`, stamps that node with

```haskell
typ = TFunction FreeFunction mempty t mempty
```

a synthetic zero-parameter function type whose **return type is the expected type**. Later,
`deduceTemplateArgs` sees `tIsFunction (typeOf ident)` and runs
`deduceType funcReturnType (tReturn (typeOf ident))` (`Type.hs:1483-1484`), deducing the
template arguments *from the context's expected type*.

Applied at four places, all guarded by `typeResolved n`:
- `VariableF` → the initialiser (`Type.hs:1147-1148`);
- `AssignF` → the RHS (`Type.hs:1150-1151`);
- `FunctionCallF` → each argument, paired with the corresponding declared parameter type,
  only when the arities match (`Type.hs:1153-1159`);
- `FunctionF` → each top-level `ReturnF (Just a)` in the body, with the declared return
  type (`Type.hs:1161-1168`).

Behaviour is exercised by `test/syntax/return-type-polymorphism.k` (e.g. `bool x = Fn_T();`
at `:4-9`, `uint2 x = Fn_T_auto(10);` at `:44-49`).

### 8.4 Induction-variable deduction

`RangeForF` / `StaticForF` / `UnrolledForF` with a `const auto` induction variable
(`Type.hs:1131-1145`):
- `static for` / `unrolled for` → `constIndexType limit`;
- range `for` → `constIndexType limit` if the limit is an int literal, else
  `tConst (TUnsigned (sizeOf limit))`.

`constIndexType` (`Type.hs:1113-1118`):
- `x < 0` → `TError "The iteration count of `static for` can't be negative"`;
- `x < 3` → `const uint1`;
- otherwise → `const uint(clog2 x)`.

---

## 9. Template argument deduction

Entry point: `deduceTemplateArgs :: SymbolMap → NotedExp → NotedExp` (`Type.hs:1385-1593`),
run as a front-end pass (`Frontend.hs:71`) and inside the template loop
(`Template.hs:73`).

### 9.1 Rewrite triggers

`rewrite` (`Type.hs:1391-1404`):
1. `MemberAccessF a b` where the node is `TUndefined` and `a` is a class → becomes
   `FunctionSpecifierF (Just a) (qualified name of the method)`.
2. `FunctionSpecifierF (Just a) b` whose qualifier is `$unresolved` and `a` is a class →
   the qualifier is set to the class's qualified name.
3. `DesignatorF a (FunctionSpecifier x y)` → deduce on `y` with **no** argument types.
4. `FunctionCallF a (FunctionSpecifier x y) c` → deduce on `y` with the argument types
   `map typeOf c`.
5. `NamedValueF a@QualifiedIdentifier{}` that is not type-resolved → deduce with no
   argument types.
6. `TypeIdentifierF s t@TemplateInstance{}` → deduce with no argument types.
7. `ScopedNameF s (Just t@TemplateInstance{}) a` → deduce on `t`.

### 9.2 Non-function templates

`deduceInstance'` for `Template params decl` where `decl` is not a function
(`Type.hs:1441-1445`): only fires when `length args < length params`. It fills the missing
trailing arguments purely from **defaults** (`resolveTemplateArgs` with an all-`Nothing`
deduction map). There is no structural deduction for class/struct/alias templates.

### 9.3 Function templates — the deduction monad

`deduceFromFunctionSignature` (`Type.hs:1447-1587`) fires when
`length args <= length params`. State: `Map Int (Maybe (DeduceType, TArg))`, initialised by

```haskell
unresolvedArgs params args = Map.fromList $ map (, Nothing) [length args .. length params - 1]
```
`Type.hs:1431-1432`. **Only the trailing, unspecified parameters are deducible** —
`Map.adjust` on an absent key is a no-op, so an explicitly supplied argument can never be
overwritten by deduction.

Reader environment: `data DeduceType = DeduceSupertype | DeduceSubtype` (`Type.hs:1382`),
initialised to `DeduceSupertype` (`Type.hs:1470`).

Deduction sequence (`Type.hs:1479-1484`), in order:
1. `zipWithM_ deduceFromFuncParam (funcParams) funcArgTypes` — match each declared parameter
   *type expression* against the corresponding **actual argument type**.
2. `zipWithM_ deduceFromTemplateParam params (map typeOf args)` — for template parameters
   whose declared type is a `FunctionType`, match against the supplied argument's type
   (`Type.hs:1511-1512`).
3. If the callee identifier itself carries a function type (return-type polymorphism, §8.3),
   `deduceType funcReturnType (tReturn (typeOf ident))`.

### 9.4 `deduceType` — the directional structural match

`Type.hs:1519-1571`, in clause order. LHS is a **syntax node** (the declared type
expression), RHS is a **`Type`** (the actual):

| # | Pattern (declared, actual) | Action |
|---|---|---|
| 1 | `_`, `TAuto` | nothing |
| 2 | `FunctionType _ a r`, `TFunction _ _ r' a'` | `deduceType r r'` (ambient variance), then **`local (const DeduceSubtype)`** over the parameters, zipped against `map typeOfParam a' ++ repeat TVoid` (missing actual params are treated as `TVoid`) |
| 3 | `TypeIdentifier _ (TemplateInstance a b)`, `TInstance _ a' b'` with `getQualifiedName a == a'` | `zipWithM_ deduceArg b (map snd b')` |
| 4 | `TypeIdentifier _ t@(TemplateInstance _ b)`, `TInitializer x` | look `t` up; if it names a `Template _ (Struct _ _ fields)`, for each field declared as `TypeParam _ _ _ i` deduce `b !! i` against `tPositionalType` of the corresponding initializer element |
| 5 | `TypeIdentifier _ (TemplateInstance _ b)`, unresolved `t` | set every argument of `b` to `TTypeArg TUnresolved` |
| 6 | `a`, `TInstance t _ _` | recurse on `t` |
| 7 | `Const a`, `t` | recurse |
| 8 | `a`, `TConst t` | recurse |
| 9 | `Array _ _ (Seq _)`, `TArray _ _ []` | deduce the element type only |
| 10 | `Array _ _ (Seq (a':_))`, `TArray _ _ (t':_)` | `deduceInt a' t'`; **only if it returns `True`** deduce the element type |
| 11 | `Array _ _ (Seq (a':_))`, `TInitializer ts` | if `ts` non-empty, `deduceInt a' (length ts)`; then deduce the element type against each element's `tPositionalType` |
| 12 | `ParamInt p@TypeParam{}`, `TSigned x` | `setArg p (TIntArg x)` |
| 13 | `ParamInt p@TypeParam{}`, `TUnsigned x` | `setArg p (TIntArg (x + 1))` — **the +1 signed-promotion width** |
| 14 | `ParamUint p@TypeParam{}`, `TUnsigned x` | `setArg p (TIntArg x)` |
| 15 | `TypeParam _ Typename _ _`, `t` | `setArg p (TTypeArg t)` |
| 16 | anything else | nothing |

Note there is **no** `ParamUint` vs `TSigned` clause: `uint<N>` cannot be deduced from a
signed actual.

`deduceArg x` dispatches on the actual `TArg` (`Type.hs:1513-1517`):
`TTypeArg y → deduceType x y`; `TIntArg y → deduceInt x y`;
`TNameArg t y → deduceName x (t, y)`; otherwise nothing.

`deduceInt` (`Type.hs:1580-1582`): `IntLiteral _ x` vs `y` → returns `x == y` (a *match
test*, no binding); `TypeParam` → binds `TIntArg y` and returns `True`; anything else →
`False`.

`deduceName p@(TypeParam _ a _ _) (t, s)` (`Type.hs:1573-1578`): binds `TNameArg t s` **and**
recurses `deduceType a t` (so a non-type template parameter's declared type also
participates). `NamedValue a` unwraps; anything else is ignored.

### 9.5 `setArg` and `reconcile` — the variance-aware merge

`setArg p arg` (`Type.hs:1485-1503`) only fires when the `TypeParam`'s declaring scope equals
the template's scope; it reads the ambient `DeduceType` and does
`Map.adjust (reconcile (Just (how, arg))) index`. **Therefore `reconcile new old`** — the
first argument is the newly deduced value, the second is what is already in the map.

Clauses, in order (`Type.hs:1490-1502`):

| # | Pattern | Result |
|---|---|---|
| 1 | `new = TTypeArg TInitializer{}` | keep `old` (initializer-list types never win) |
| 2 | `old = Nothing` | take `new` |
| 3 | both `DeduceSupertype`, both `TTypeArg`, both int-or-enum | `tSuper tnew told` |
| 4 | both `DeduceSupertype`, both `TTypeArg`, `tnew <: told` | keep `old` (the wider) |
| 5 | both `DeduceSupertype`, both `TTypeArg`, `told <: tnew` | take `new` (the wider) |
| 6 | `new` is `DeduceSubtype`, `told <: tnew` | keep `old` (the narrower) |
| 7 | `old` is `DeduceSubtype`, `tnew <: told` | take `new` (the narrower) |
| 8 | `tnew` not resolved | take `new` |
| 9 | `told` not resolved | keep `old` |
| 10 | `told` is `TError` | keep `old` |
| 11 | `new` and `old` carry the same `TArg` | take `new` |
| 12 | otherwise | ``TError "Conflicting template argument deduction results for parameter `P`. Could be `X` or `Y`."`` tagged `DeduceSupertype` |

In words: in **covariant** (supertype) position the deduced argument is widened to the least
upper bound; in **contravariant** (subtype, i.e. function-parameter) position it is
narrowed. `DeduceSubtype` is entered only through clause 2 of `deduceType`
(`Type.hs:1523-1524`).

### 9.6 Assembling the argument list

`resolveTemplateArgs t template args deduced` (`Type.hs:1406-1428`):

```haskell
fromMaybe args $ sequence $ Map.elems $ foldl substituteDefault (explicitArgs <> deducedArgs) params
```

- `explicitArgs = Map.fromList (zip [0..] (map Just args))`;
- `deducedArgs = Map.map arg deduced`, where `arg` turns
  `TTypeArg x` into `expFromType` (or a `desugarError (showPretty x)` if `x` is a `TError`),
  `TIntArg x` into an untyped `IntLiteralF`, `TNameArg _ x` into a qualified identifier, and
  `Nothing` into `Nothing`;
- `<>` on `Data.Map` is **left-biased**, so explicit arguments always win;
- `substituteDefault` fills in a parameter's declared default when `deduced !? n == Just
  Nothing`, first substituting already-resolved arguments into the default expression via
  the `TypeParam` index (`Type.hs:1422-1427`);
- `sequence` — if **any** slot is still `Nothing` the whole result is discarded and the
  original explicit `args` are returned unchanged.

`deduceInstanceFromTemplateParam` (`Type.hs:1467-1477`) then makes a second pass so that a
deduced `FunctionType` argument can itself trigger deduction on the function value passed
for it.

### 9.7 Template argument *validation*

`templateTypeCheck` (`TypeCheck.hs:218-297`) validates each argument against its parameter
after deduction:

| Parameter kind | Check | Message |
|---|---|---|
| `Typename` + `NamedValue` arg that is a type | reject | ``"Expected a type; did you forget 'typename`"`` |
| `Typename` | `tIsType (typeOf arg)` | `"Expected a type"` |
| `Template params1` | argument must be a template whose parameter kinds match position-wise; extra parameters of the argument must all have defaults | `"Expected a template with parameters:\n\t<…>\nbut the argument has parameters:\n\t<…>"`, `"Expected a template"`, `"Expected a template specifier"` |
| any non-type parameter given a type argument | reject | `"The type can't be used as an argument for non-type template parameter"` |
| `FunctionType` | argument must not be a method; `t2 <: t1` | `"Expected a non-member function"`, `"Expected a function of type:\n\t…\nbut the argument type is:\n\t…"` |
| `Auto` + initializer list | reject | `"Initializer list can't be used as an argument for `auto` parameter"` |
| any parameter + `IntLiteralF` | the literal is **re-stamped** with the parameter's width and truncated (`truncateInt`/`truncateUInt`); with `Auto` it becomes untyped | `TypeCheck.hs:280-285` |
| any other parameter | `t2 <: t1` | `"Expected a constant expression of type:\n\t…\nbut the argument type is:\n\t…"` |

All argument errors are prefixed with
``"Invalid argument for template parameter `P`.\n"`` (`TypeCheck.hs:232`).

`templateArgResolved` (`TypeCheck.hs:314-321`): an argument counts as resolved if it is a
function, a type, a template, or (type-resolved **and** `constexprResolved`).
`constexprResolved` requires a literal or enum value, recursively through initializer lists
and designators (`Constexpr.hs:385-390`).

---

## 10. The whole-program fixpoint

### 10.1 The loop

`Inference.hs:25-42`:

```haskell
inferType = loop []
  where
    loop prevSig p
        | prevSig == sig = inferred
        | otherwise      = loop sig inferred
      where
        symbols  = getSymbols' TemplateDecl p
        inferred = parallel (inferExpTree symbols) p
        -- Count of unresolved types is monotonically decreasing
        sig :: [Integer]
        sig = parallel (para go) inferred
          where
            go e | typeResolved e = sum
                 | otherwise      = succ . sum
```

Precise semantics:

- The program is a **list of top-level trees** (one per source file). `parallel = parMap rpar`
  (`Desugar.hs:54-55`) — parallelism is per-file and does not affect results.
- The symbol map is rebuilt **every round** from the *current* trees, using the
  `TemplateDecl` configuration (template bodies are not walked, `Symbols.hs:46-47`).
- `sig` is a **list of unresolved-node counts, one per file**. `go` is a paramorphism
  algebra: for a node whose type is resolved the count is the sum of the children's counts;
  otherwise `1 +` that sum.
- Termination: the loop stops when the count **vector** is unchanged from the previous
  round, and returns the *current* round's trees. The initial `prevSig = []` never equals a
  non-empty `sig`, so a non-empty program runs **at least two** rounds.
- Monotonicity: once a node's type is resolved it is never re-inferred —
  `inferExpType' _ | typeResolved n = NotedExp n e` (`Type.hs:1170-1171`) — and
  `resolveTypeOf` only overwrites an existing type when the current type is `TUndefined` or
  the new type is one of the "installable-anyway" shapes (`Type.hs:1037-1048`). Hence the
  count is non-increasing and the loop terminates.
- **The loop is not a fixpoint on the tree**, only on the count. A round that changes types
  without changing resolvedness will not be detected, and the final trees are those of the
  first round that produced no change in the count.

### 10.2 Per-round traversal and where laziness matters

`inferExpTree` (`Inference.hs:44-69`):

```haskell
infer = staticControl . unfix . evaluateConstexpr symbols . unfix . inferExpType symbols
```

so at every node, in order: **type inference → constant folding → static control-flow
evaluation**. (Contrast `inferType'`, `Inference.hs:12-22`, used from `TypeCheck.hs` for
resolving template parameter types, where the order is reversed: **constant folding → type
inference**.)

`go` decides which subtrees are visited (`Inference.hs:48-69`) — this is the "laziness" the
Rust port must make explicit:

| Node | Traversal |
|---|---|
| `LambdaF` | **body is not traversed at all**; the node is passed to the algebra untouched. Lambdas are desugared away later (`Function.hs:189-262`). |
| `FunctionF` with **any** parameter of function type | **nothing** is traversed (higher-order function; waits for `higherOrderFunctions` to turn it into a template, `Function.hs:313-363`) |
| `FunctionF` with a not-yet-resolved parameter | only `funcParams` is traversed |
| `FunctionF` in a **local scope** (`isLocalScope`, i.e. the enclosing scope's last name starts with `$`, `Desugar.hs:176-180`) | only `funcParams` and `funcReturnType`; the **body is skipped** |
| `FunctionF` otherwise | full traversal |
| `TemplateF a b` | only the template **parameters** `a`; the declaration `b` is skipped |
| `StaticIfF a b c` | only the **condition** `a`; both branches are skipped |
| `ClassF` in a local scope | children traversed with `staticControl` **only** — no type inference, no constant folding |
| anything else | full traversal |

Consequences:
- A type inside a template body only gets inferred after `instantiateTemplates` has produced
  an instance.
- A type inside the untaken branch of a `static if` is never inferred, so a branch that
  would be ill-typed is silently accepted.
- Local functions have their bodies inferred only via the enclosing function's traversal
  after `moveFunctions` hoists them (`Function.hs:84-128`).

### 10.3 Making it round-based in Rust

A faithful, non-lazy Rust implementation:

```rust
pub struct InferOutcome { pub unresolved_per_file: Vec<u64> }

pub fn infer_program(files: &mut Vec<Tree>) {
    let mut prev: Option<Vec<u64>> = None;
    loop {
        let symbols = build_symbols(files, SymbolsConfig::TemplateDecl); // rebuilt each round
        for f in files.iter_mut() { *f = infer_tree(&symbols, f); }      // parallelisable
        let sig: Vec<u64> = files.iter().map(count_unresolved).collect();
        if prev.as_ref() == Some(&sig) { break; }
        prev = Some(sig);
    }
}
```

with `infer_tree` implementing `go` above as an explicit *visit policy* enum
(`Skip`, `ParamsOnly`, `ParamsAndReturn`, `CondOnly`, `StaticControlOnly`, `Full`) rather
than relying on lazy evaluation. `count_unresolved` must count a node iff
`!t_resolved(node.ty)` and sum children unconditionally — including children of skipped
subtrees, because `para` in `sig` walks the **whole** tree (`Inference.hs:37-41`), not the
pruned one.

Two subtleties to preserve:
1. The `<= 2` round minimum for non-empty programs (`prev` starts as `None`/`[]`).
2. The returned trees are the ones from the *stable* round, not the previous one.

### 10.4 Where the loop sits in the pipeline

`desugarPasses` (`Frontend.hs:37-80`) applies the pass list with `foldr`, so passes execute
**bottom-to-top**. In execution order the type-relevant ones are:

1. `redundantNameScope . localMemberFunctions . freeNonInlineFunctions .
   interpolatedStringPre . validateTypedLiterals` (`Frontend.hs:74-79`)
2. `higherOrderFunctions` (`Frontend.hs:73`)
3. `captureThis . reifyEnums` (`Frontend.hs:72`)
4. `deduceTemplateArgs` (`Frontend.hs:71`)
5. `intrinsics` (`Frontend.hs:70`)
6. **`inferType`** (`Frontend.hs:69`)
7. `trimExternClasses` (`Frontend.hs:68`)
8. `instantiateTemplates` (`Frontend.hs:67`) — which itself contains two more `inferType`
   calls per iteration (`Template.hs:72,75`) and recurses (`Template.hs:58`)
9. `undefinedSymbols`, `typeErrors`, `unresolvedFunctionTemplateArgs`,
   `unresolvedTemplates`, `validateExportableClasses`, `validateExtern`, `removeTemplate`,
   `postDesugar`, `deduceAuto`, `interpolatedStringPost` (`Frontend.hs:57-66`)
10. **`validateProgram`** (`Frontend.hs:55-56`) — the conversion/narrowing checks.

`instantiateTemplates` runs its own pass list (`Template.hs:71-81`), in execution order:
`addLambdaWrappers`, `desugarLambdas`, `abbreviatedFunctionTemplate`, `moveFunctions`,
`inferType`, `closureCalls`, `deduceTemplateArgs`, `inferType`, `templateTypeCheck`; then
recurses until no new instances are decided (`Template.hs:57-58`).

`--frontend-passes`, `--template-passes`, `--template-iterations` truncate these lists for
debugging (`Frontend.hs:38`, `Template.hs:62-64`, `CmdArgs.hs:105-106`).

---

## 11. Struct, union and class layout

### 11.1 Fields

`fields` collects the ordered member list of a struct/union/class
(`Type.hs:1343-1348`): only `Variable` members contribute `(name, type)`; a `StaticIf`
member contributes the placeholder `("", TUnresolved)` (which keeps the aggregate
unresolved until the `static if` folds); everything else is dropped.

Because `StaticIf` members are represented as a single anonymous slot, an aggregate
containing an unfolded `static if` has `tResolved = False` (`Type.hs:1024-1026`) and hence a
`Nothing` width.

### 11.2 Layout rules

- **Field ordering**: declaration order, as parsed (`Parser.hs:294`/`Parser.hs:298-299` for `struct`,
  `Parser.hs:301`/`Parser.hs:305` for `union`, `Parser.hs:331-339` for `class`).
- **Struct width** = Σ field widths (`Type.hs:716`, `Type.hs:692-693`;
  `cpp/parse_tree.cpp:553-559`).
- **Union width** = max field width (`Type.hs:717`; `cpp/parse_tree.cpp:561-567`).
- **Bit offset of field *i*** = Σ widths of fields `0 .. i-1` (`Type.hs:727-729`;
  `cpp/parse_tree.cpp:413-437`).
- **No padding, no alignment**: nothing in either compiler rounds an offset. The only
  alignment-flavoured rule is `byteoffsetof`/`bytesizeof` refusing non-multiples of 8
  (`Constexpr.hs:349-351`).
- **Class width is `Nothing`** (`Type.hs:718`); classes are not values —
  `cpp/parse_tree.h:1023-1030` returns 0 and `IsPod() == false`. Classes cannot be local
  variables, array elements or struct members.
- **Arrays** are laid out element-major with total width `product dims * element width`
  (`Type.hs:712`); `tDimension` returns the **first** dimension (`Type.hs:540-544`) and
  `tElement` peels dimensions from the front (`Type.hs:936`).

### 11.3 Inheritance / base classes

**There is none.** The class production is `class <identifier> { memberDecl* }` with no base
clause (`Parser.hs:331-339`); `struct` and `union` likewise (`Parser.hs:298-299`,
`Parser.hs:305`). Member
declarations are exactly: `private:`, `public:`, `static if`, "common declarations"
(`commonDecl`), `default = …`, a class variable, a member function, and a member function
template (`Parser.hs:341-350`). The C++ `ClassType` has no base pointer either
(`cpp/parse_tree.h:1000-1090`).

Visibility is tracked as a scan over members (`mapClassMembers`, `Desugar.hs:115-121`) and is
used only by the exportable-class validation (`Frontend.hs:337-391`), not by the type system.

### 11.4 Member access typing

`tField t name` = `tLookupField lookup t name`, defaulting to `TUndefined`
(`Type.hs:957-968`):
- struct / union / class → association-list lookup;
- `TConst a` → the result is wrapped in `TConst`;
- `TReference _` → **`undefined`** (a crash) — the `MemberAccessF` inference clause handles
  references separately by looking the class up in the symbol map (`Type.hs:1232-1235`);
- `TInstance a _ _` → recurse;
- unresolved → `TUnresolved`;
- anything else → ``TError "Invalid member access on type `t`"``.

---

## 12. Enums

Syntax: `enum <Ident> : <type> { c0, c1 = e, … }` — the base type is **mandatory**
(`Parser.hs:282-284`).

- Type: `TEnum <qualified name> <base type>` (`Type.hs:1202`).
- Base type must be integral: `"Enums must be based on an integral type"`
  (`TypeCheck.hs:201-203`); the base type expression is normalised through
  `expFromType` (`TypeCheck.hs:202`).
- Width = base width (`Type.hs:715`).
- Constant values: implicit values are reified before type checking — the first constant is
  `0` and each subsequent implicit constant is `previous + 1` as a `BinaryF Add` on untyped
  literals (`Frontend.hs:195-208`).
- Range check (`TypeCheck.hs:205-216`): for base `intN` the legal half-open range is
  `[-(2^(N-1)), 2^(N-1))`; for base `uintN` it is `[0, 2^N)`. Out of range →
  `"Enum constant value is out of range for the base type"`; non-constant →
  `"Enum constant value doesn't evaluate to an integer constant"`.
- An enum **constant** has type `const <EnumType>` (`Type.hs:1203-1204`), obtained by
  looking up the enum symbol by the constant's qualifier.
- In expressions, `typeOfOperand` erases the enum to its base type (`Type.hs:733-737`), and
  `tSuper` does the same (`Type.hs:907-908`).
- In `<:`, `TEnum _ base <: t` iff `base <: t` (`Type.hs:328`); the reverse direction has no
  clause, so `uintN <: E` is false unless the types are equal.
- Enum values are erased for conversion checking when the target is an integer
  (`TypeCheck.hs:155-156`).
- `Constexpr.hs:174-175` strips `EnumValue` wrappers before folding binary operations, and
  `Constexpr.hs:291-292` re-wraps a folded enum constant as `EnumValueF`.
- `ScopedNameF a (Just b) c` where `b` names a class or enum has its qualifier rewritten to
  the type's qualified name (`Constexpr.hs:159-160`), which is how `E::X` resolves.

C++ `EnumType` derives from `LeafType`, carrying the base type's `_baseType` and `_width`
plus the constant list (`cpp/parse_tree.h:911-947`).

---

## 13. Functions, closures and higher-order functions

### 13.1 `TFunction`

`TFunction FunctionKind (Set TAttr) Type [TFuncParam]` (`Type.hs:263`).

Produced by:
- a function definition, `funcType funcKind funcAttr (returnType funcReturnType) (params
  funcParams)` (`Type.hs:1209`);
- a forward declaration, always `FreeFunction` (`Type.hs:1220`);
- a **function type expression** `[[attrs]](P0, P1) -> R`, always `FreeFunction`
  (`Type.hs:1223-1224`, syntax at `Parser.hs:551-557`).

`params` (`Type.hs:1350-1356`) drops parameters whose type is `TVoid` (so `() -> R` and
`(void) -> R` agree), takes the attribute set from the parameter's flag attributes, and
records the parameter name when present. The only parameter-level flag attribute the parser
accepts on a function *type* is `[[last]]` (`EndTransaction`, `Parser.hs:556`).

`tFunctionParams` (`Type.hs:531-536`) unwraps `TInstance`/`TConst`, and for a `TClosure`
returns `tail` of the underlying function type's parameters — **the capture parameter is
dropped**.

`tFunctionAttr` (`Type.hs:526-530`) unwraps `TInstance`/`TConst`.

Free functions must be `inline`: `"Free functions must be declared `inline`"`
(`Frontend.hs:227-231`).

### 13.2 Intrinsics

`intrinsics` (`Type.hs:1069-1084`) assigns types to five reserved names by qualified name:

| Name | Type |
|---|---|
| `__print` | `(string) -> void` |
| `assert` | `(bool) -> void` |
| `__cycles` | `() -> uint64` |
| `__str_cnt` | `() -> uint64` |
| `__assert_str_eq` | `(string, string) -> void` |

### 13.3 Lambdas and closures

A lambda expression parses to `LambdaF declScopeName declName lambdaCaptures funcParams
funcReturnType body` (`Syntax.hs:564-571`, `Parser.hs:444`), with captures listed
explicitly (`Parser.hs:447`).

`infer LambdaF{} = TUnresolved` (`Type.hs:1231`) — lambdas have no type until desugared.
`validateProgram` errors on any surviving lambda:
`"Invalid use of lambda expression in this context"` (`TypeCheck.hs:52`).

`desugarLambdas` (`Function.hs:189-262`) replaces each lambda with:
1. a `using <name>$capture = decltype(make_captureN(<captured values>));` alias
   (`Function.hs:241-243`, `Function.hs:145-159`);
2. an `inline` `LambdaFunction` whose **first parameter** is the capture
   (`Function.hs:246-261`); the captured variables are re-bound at the top of the body as
   `auto` locals reading `capture.x0`, `capture.x1`, … (`Function.hs:255-261`);
3. at the use site, `cast<Closure<F, fn, Capture>>(make_captureN(...))`
   (`Function.hs:161-170`, `Function.hs:180-184`).

`make_captureN` are library functions returning an anonymous `struct Capture { T0 x0; … }`
(`library/data/closure/core.k:13-24` for `N = 0`, `library/data/closure.k:33-47` for `N = 1`,
up to `make_capture21`, `library/data/closure.k:24`).

`Closure` itself is a **type alias template**:
```
template <typename FunctionType, FunctionType fn, typename Capture>
using Closure = Capture;
```
`library/data/closure/core.k:9-10`. Its qualified name is
`["@data@closure@core", "Closure"]` (`Type.hs:112-113`, with
`moduleNamespace ["data","closure","core"]` producing `"@data@closure@core"`,
`Syntax.hs:273-274`).

`tIsClosure t` is *defined* as "`t` is a `TInstance` whose template name is
`data.closure.core::Closure`" (`Type.hs:614-615`), recursing through `TConst`/`TInstance`.

The `AliasF` inference clause turns that instance into a `TClosure`
(`Type.hs:1175-1177`):

```haskell
infer x@(AliasF _ t _)
    | tIsClosure (typeOf n)
    , TInstance _ _ [(_, TTypeArg a), (_, TNameArg _ b), (_, TTypeArg c)] <- typeOf n
        = TType $ TClosure b a c (getQualifiedName x)
```

so the field order is **`TClosure <lambda function qualified name> <function type>
<capture type> <closure alias qualified name>`**. Accessors:
`tClosureFunction` → field 1 (`Type.hs:493-497`), `tClosureCapture` → field 3
(`Type.hs:499-503`), `tQualifiedName` → field 4 (`Type.hs:505-514`).
`tReturn (TClosure _ f _ _) = tReturn f` (`Type.hs:576`);
`tWidth (TClosure _ _ cap _) = tWidth cap` (`Type.hs:714`) — **a closure's runtime width is
its capture's width**.

`closureCalls` (`Closure.hs:19-26`) rewrites `c(args)` where `c` is a closure into
`fn(cast<Capture>(c), args)` — the capture is passed as the first argument.

Closure subtyping requires the *same lambda function name*, plus subtyping of the function
type and the capture type (`Type.hs:340-343`).

A global closure variable must have a zero-width capture, else
`"Closures that capture value(s) are not allowed in global scope"` (`TypeCheck.hs:146-148`).

`tIsValueType` includes closures (`Type.hs:686`) so a closure can be interpolated into a
string (`Frontend.hs:263-265`).

### 13.4 Higher-order functions

`higherOrderFunctions` (`Function.hs:313-363`): a function with one or more parameters whose
declared type is a `FunctionType` is rewritten into a **template** with two parameters per
such function parameter:
- `<name>$capture` (a `typename`, index `2i`) — `Function.hs:342-344`;
- `<name>` with the *closure function type* (index `2i + 1`) — `Function.hs:346-348`, where
  the closure function type is the original function type with a capture parameter prepended
  (`Function.hs:334-340`);

and each such value parameter's declared type becomes
`Closure<closureFunctionType, closureFunctionTypeParam, captureTypeParam>`
(`Function.hs:353-359`).

Non-inline higher-order functions are rejected:
`"Non-inline higher order functions are not supported"` (`Function.hs:325`).

Nested templates produced this way are merged with `mergeTemplateParams`, which re-indexes
the inner template's parameter indices by the outer count (`Function.hs:264-281`,
`Function.hs:361`).

`addLambdaWrappers` (`Function.hs:366-422`) wraps a *named function* passed as an argument
into a synthesised lambda `$wrapper$<qualified name>`, capturing `this` for member
functions, so that every higher-order argument is uniformly a closure.

### 13.5 Abbreviated function templates

A function with an `auto` parameter is an "abbreviated template"
(`Syntax.hs:722-724`). `abbreviatedFunctionTemplate` (`Function.hs:288-311`) expands it into
an explicit `template <typename P$T, …>` whose parameters replace the `auto` parameter types,
emitting both the template and the original declaration (the latter is later deleted by
`removeTemplate`, `Frontend.hs:298-302`).

---

## 14. Proposed Rust representation

### 14.1 Arbitrary-precision constants

The Haskell front-end uses `Integer` (unbounded) for every integer literal
(`Syntax.hs:347-350`, `literalValue :: Integer`); the C++ middle-end uses
`boost::multiprecision::cpp_int` (`cpp/pch.h:92`). Widths can legally reach
`c_maxRegisterBitWidth = 1048576` bits (`cpp/ir.h:39`), so 128-bit arithmetic is **not**
sufficient.

```rust
/// Arbitrary-precision signed integer with a small-value fast path.
/// Semantics must match Haskell `Integer`: unbounded, two's-complement bit ops,
/// `quot`/`rem` truncating toward zero.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ConstInt {
    Small(i128),
    Big(std::sync::Arc<num_bigint::BigInt>),
}

impl ConstInt {
    pub fn clog2(&self) -> u32;            // panics for n <= 0, matching Internal.hs:53
    pub fn truncate_uint(&self, n: u32) -> ConstInt;  // Internal.hs:34-35
    pub fn truncate_int(&self, n: u32) -> ConstInt;   // Internal.hs:37-41
}
```

`clog2` for `n > 0`: `if n.is_power_of_two() { n.bits() - 1 } else { n.bits() }` — matching
`Internal.hs:53-56`.

### 14.2 The `Type` enum

Mirrors `Type.hs:224-277` one-for-one. Widths are `u32` (Haskell uses `Int`); array
dimensions are `ConstInt`-sized in Haskell (`[Integer]`, `Type.hs:261`) so keep them wide.

```rust
pub type Name = kg_base::Symbol;              // interned
pub type QualifiedName = kg_base::SymbolPath; // interned Vec<Symbol>

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ty {
    Auto,                                        // Type.hs:225
    Template,                                    // Type.hs:227
    Type_(TyRef),                                // Type.hs:229  (`TType`)
    Undefined,                                   // Type.hs:231
    Untyped,                                     // Type.hs:233
    Unresolved,                                  // Type.hs:235
    Initializer(Vec<TyRef>),                     // Type.hs:237
    Designator(Name, TyRef),                     // Type.hs:239
    Positional(u32, TyRef),                      // Type.hs:241
    Void,                                        // Type.hs:243
    Boolean,                                     // Type.hs:245
    Float,                                       // Type.hs:247  (always 32-bit)
    Str,                                         // Type.hs:249  (`TString`, width 0)
    Signed(u32),                                 // Type.hs:251
    Unsigned(u32),                               // Type.hs:253
    Dependent(TyRef),                            // Type.hs:255  ([[pipelined]] return)
    Const(TyRef),                                // Type.hs:257
    Reference(QualifiedName),                    // Type.hs:259  (width 1!)
    Array { attrs: TAttrSet, elem: TyRef, dims: Vec<u64> },              // Type.hs:261
    Function { kind: FunctionKind, attrs: TAttrSet, ret: TyRef,
               params: Vec<TFuncParam> },                                // Type.hs:263
    Closure { func: QualifiedName, func_ty: TyRef,
              capture: TyRef, alias: QualifiedName },                    // Type.hs:265
    Enum(QualifiedName, TyRef),                                          // Type.hs:267
    Struct(QualifiedName, Vec<(Name, TyRef)>),                           // Type.hs:269
    Union(QualifiedName, Vec<(Name, TyRef)>),                            // Type.hs:271
    Class(QualifiedName, Vec<(Name, TyRef)>),                            // Type.hs:273
    Instance { concrete: TyRef, template: QualifiedName,
               args: Vec<(Name, TArg)> },                                // Type.hs:275
    Error(kg_base::Str),                                                 // Type.hs:277
}

/// `Eq` must ignore `name` (Type.hs:126-129) while `Ord` includes it (derived, Type.hs:123).
/// Reproduce both faithfully: hand-write `PartialEq`/`Eq`/`Hash`, derive nothing.
#[derive(Clone, PartialOrd, Ord)]
pub struct TFuncParam { pub attrs: TAttrSet, pub ty: TyRef, pub name: Option<Name> }

/// Attribute sets are `Data.Set` in Haskell: ordered, deduplicated, and compared by
/// subset in `<:` (Type.hs:333). A sorted `Vec` or `BTreeSet` is required — a `HashSet`
/// would break the derived `Ord` on `Ty`.
pub type TAttrSet = std::collections::BTreeSet<TAttr>;
```

`TyRef` should be an index into an arena/interner (`Ty` is deeply recursive and is compared
structurally constantly). Interning gives O(1) `Eq`, but **`Ord` must still be the
structural lexicographic order** derived in `Type.hs:278`, because `tSuper`, `<:` and
`Data.Set`/`Data.Map` keys all depend on it. Store a precomputed structural ordering key
alongside the interned id, or keep `Ord` structural and only intern for allocation.

### 14.3 `StructuralKey` — structural equality ignoring spans and types

The Haskell tree gets this for free:

```haskell
-- Equality ignores annotation
instance Eq (f a) => Eq (Note n f a) where (==) a b = unNote a == unNote b
-- Ordering ignores annotation
instance Ord (f a) => Ord (Note n f a) where (<=) a b = unNote a <= unNote b
```
`Syntax.hs:593-599`. Because the note carries **both** the source location (`Src`,
`Syntax.hs:609-621`) and the inferred type (`Typed`, `Type.hs:465-471`), AST equality in the
reference compiler ignores *spans and types simultaneously*.

Consumers that depend on this, all of which a Rust port must reproduce:

| Consumer | What it keys on | Cite |
|---|---|---|
| `decidedInstances` — the set of template instances to emit | `Map <args-Seq node> <note>` | `Template.hs:88-108`, `Template.hs:96` |
| `resolve` — rewriting a `TemplateInstance` to its mangled name | `b \`Map.member\` …` | `Template.hs:159` |
| `instantiate` — enumerating instances | `Map.toList` on the same map | `Template.hs:120` |
| `deduplicateInstances` | `nubBy` over qualified names of user-defined types | `Template.hs:144-155` |
| `overridingInitializerWarning` | designator-name equality | `TypeCheck.hs:60-66` |

Proposed Rust design:

```rust
/// A content hash of an AST subtree that deliberately ignores:
///   * source spans   (`Src`,   Syntax.hs:609-621)
///   * inferred types (`Typed`, Type.hs:465-471)
///   * doc comments   (`Src.docs`, Syntax.hs:615)
/// It includes every `ExpF` payload field: operator tags, names, literal values,
/// literal type suffixes, declare-flags, function kinds, attribute enums.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct StructuralKey(u128);

pub trait Structural {
    fn structural_hash(&self, h: &mut StructuralHasher);
    fn structural_eq(&self, other: &Self) -> bool;
}

/// Newtype wrapper making any node usable as a HashMap/BTreeMap key with
/// Haskell `Note` semantics.
pub struct ByStructure<'a>(pub &'a Node);
impl PartialEq for ByStructure<'_> { /* structural_eq, skipping note */ }
impl Hash      for ByStructure<'_> { /* structural_hash */ }
```

Implementation notes:
- Derive `Structural` with a proc-macro that skips fields annotated `#[structural(skip)]`
  (the span and the type slot), so adding a field cannot silently change key semantics.
- Because `Template.hs` uses an **ordered** `Map`, and iteration order determines instance
  emission order (hence mangled-name suffix order and, ultimately, generated Verilog),
  the Rust map must be a `BTreeMap` keyed by a **structural total order**, not a
  `HashMap`. Reproduce `Ord (Note n f)` (`Syntax.hs:598-599`) as a derived lexicographic
  order over the `ExpF` constructor index and payloads, with the note skipped.
- The 128-bit hash is a cache; `structural_eq` must still be checked on collision, exactly
  as Haskell's `Eq` is authoritative.

### 14.4 Suggested module split

> **As built (2026-10-11).** `kg-ty` holds the type model — `ty`, `width`,
> `subtype`, `convert`, and the node key (`table`). Inference is `kg_sem`, over
> `kg-hir`'s IR: `kg_sem::infer` (the per-node synthesis and the fixpoint),
> `kg_sem::deduce`, `kg_sem::census` and `kg_sem::elaborate` (the round-based
> template loop) and `kg_sem::check` (the checks after instantiation). The CST
> inference engine, `kg-cst-infer`, which the `kg_cst_infer` row below names, was
> deleted in sprint W3.7; there is no `kg_ty::infer`, `kg_ty::fixpoint` or
> `kg_ty::check`. Constant folding is `kg-const` and `kg_hir::constexpr`.

| Rust module | Reference source |
|---|---|
| `kg_ty::ty` — `Ty`, `TAttr`, `TArg`, `TFuncParam`, predicates (`is_int`, `is_closure`, …) | `Type.hs:224-690` |
| `kg_ty::width` — `t_width`, `t_offset_of`, `t_sum_widths`, `clog2` | `Type.hs:692-730`, `Internal.hs:53-57` |
| `kg_ty::algebra` — all `t_add`/`t_sub`/… and the literal-specialised forms | `Type.hs:740-926` |
| `kg_ty::sub` — `<:`, `t_is_convertible_to`, `t_super`, `t_aggregate_initializer` | `Type.hs:306-366`, `Type.hs:899-926` |
| `kg_cst_infer` (was `kg_ty::infer`) — the per-node synthesis algebra | `Type.hs:1105-1380` |
| `kg_ty::deduce` — template argument deduction | `Type.hs:1382-1593` |
| `kg_ty::fixpoint` — the round-based loop | `Inference.hs:25-69` |
| `kg_ty::check` — `validateProgram`, `templateTypeCheck` | `TypeCheck.hs` |
| `kg_const` — constant folding, `static if`/`static for` | `Constexpr.hs` |

---

## 15. Trap list (things a naive port gets wrong)

1. `a - b` on two unsigned operands is **signed**, width `1 + max` (`Type.hs:861`;
   `cpp/parse_tree.cpp:3243-3253`).
2. `a << b` widens by `2^width(b) - 1`, and `width(b) >= 20` is a hard error
   (`Type.hs:894-897`; `cpp/parse_tree.cpp:3262-3264`).
3. `uint & uint` narrows to `min` of the widths in the front-end but `max` in the C++
   middle-end (`Type.hs:882-883` vs `cpp/parse_tree.cpp:3256-3260`).
4. `<:` on integers is **strict** (`x < y`), so `uint8 <: int8` is false (`Type.hs:321-323`).
5. `TAuto <: anything` is true, one-directionally (`Type.hs:349`).
6. Arrays are **invariant** in the element type and require **equal** attribute sets
   (`Type.hs:344-348`).
7. Function subtyping is contravariant in parameters and requires
   `attrs(target) ⊆ attrs(source)` (`Type.hs:330-334`).
8. `TFuncParam`'s `Eq` ignores the name but its `Ord` does not (`Type.hs:123-129`).
9. `tWidth TString = Just 0` and `tWidth TReference = Just 1` (`Type.hs:705`,
   `Type.hs:710`).
10. `tWidth (TUnion _ [])` calls `maximum []` and throws (`Type.hs:717`).
11. `tMux []` calls `foldr1` on an empty list and throws (`Type.hs:926`).
12. `tLookupField (TReference _)` is `undefined` — a crash if reached (`Type.hs:965`).
13. `tInitializerType` is `undefined` on anything but a designator/positional
    (`Type.hs:288-291`).
14. `a[i]` on the **last** dimension of an ECC memory yields the ECC function's *return*
    type, not the element type (`Type.hs:930-935`).
15. `tElementLValue (TConst a)` delegates to `tElement`, not to itself, so the ECC
    substitution leaks onto the lvalue path for `const` memories (`Type.hs:949`).
16. The ternary `c ? t : f` becomes `Mux c (Seq [f, t])` — **the else branch is index 0**
    (`Parser.hs:968`).
17. `[[pipelined]]` return types become arrays whose extent is the **first call argument**,
    and that first argument is exempt from conversion checking
    (`Type.hs:1368-1378`, `TypeCheck.hs:92-93`).
18. `auto` return-type deduction inspects only the **first top-level** `Return`/`StaticIf`
    statement of the body (`Type.hs:1211-1217`).
19. Conversion of an integer **literal** is checked against its *minimal* type, not its
    suffixed type (`TypeCheck.hs:154`) — which is why constant folding silences narrowing
    warnings (`test/syntax/conversion-warning.k:118-127`).
20. `return` statements never emit the `conversion` warning (`TypeCheck.hs:84`,
    `TypeCheck.hs:102`).
21. `conversion` is **off by default**; only `--Wall` or `--Wconversion` enables it
    (`CmdArgs.hs:112-122`).
22. `reconcile` is called as `reconcile new old` (`Map.adjust` semantics,
    `Type.hs:1489`) — getting the argument order backwards inverts the variance rules.
23. Only the **trailing** template parameters (indices `≥ len(explicit args)`) are ever
    deduced (`Type.hs:1431-1432`).
24. Deducing `int<N>` from a `uintW` actual binds `N = W + 1` (`Type.hs:1554-1555`).
25. `sig` in the fixpoint counts nodes over the **whole** tree even though the traversal
    prunes subtrees (`Inference.hs:37-41` vs `Inference.hs:48-69`); the loop returns the
    trees from the *stable* round.
26. `TError` counts as `tResolved` (`Type.hs:1015`), so an error type terminates the
    fixpoint instead of looping forever.
27. `inferType'` folds constants **before** inferring types; `inferExpTree` does the
    opposite (`Inference.hs:21` vs `Inference.hs:46`).
28. `Data.Map`'s `<>` is left-biased, so explicit template arguments always beat deduced
    ones (`Type.hs:1407`).
29. The Haskell front-end never checks that a `/` or `%` denominator is unsigned; the C++
    middle-end does (`Type.hs:871-875` vs `cpp/parse_tree.cpp:3076-3083`).
30. Comparison operators are typed `TBoolean` with **no** operand check in the front-end
    (`Type.hs:790-795`); the operand check happens only in C++
    (`cpp/parse_tree.cpp:3025-3040`, `cpp/parse_tree.cpp:3212-3216`).

---

## Open questions

Each item names a probe program (a `test/syntax`-style snippet with the expected
`expected:N` / `warning:kind` trailer) that would settle it against the reference compiler.

1. **Negative literal shift counts.** `tShiftLLiteral` / `tShiftRLiteral` accept a negative
   `n` and call `tWidenInt`/`tNarrowInt` with it (`Type.hs:843-850`). Does the compiler
   accept `x << -1`?
   *Probe:* `inline void main() { uint8 x; auto y = x << -1; static assert(decltype(y) == uint7); }`
   *Settled for the constant fold (2026-09-24):* the reference **crashes** —
   `static assert((3 << -1) == 1)` exits 1 with no `Error` line (`test/syntax/basics.k::4,5`,
   whose `expected:0` holds only because the harness counts `Error` lines; GHC's
   `shiftL` raises `overflowError` on a negative count). The fold leaves the node
   unfolded and says nothing. The *type* of `x << -1` is still open.

2. **`tWidth` of an empty union.** `maximum []` throws (`Type.hs:717`). Is an empty union
   syntactically reachable, and does it crash the compiler or produce a diagnostic?
   *Probe:* `union U { } inline void main() { U u; }` and
   `union U { } static assert(bitsizeof(U) == 0);`

3. **`tMux` on a one-element `mux`.** `foldr1` on a single-element list returns that element,
   but `mux(sel, e)` with one arm — is it rejected by the parser (`sepBy1`,
   `Parser.hs:870`) or accepted with the arm's type?
   *Probe:* `inline void main() { uint4 a; auto x = mux(0, a); }`

4. **`tField` on a `TReference`.** `tLookupField (TReference _) _ = undefined`
   (`Type.hs:965`). The `MemberAccessF` clause special-cases references
   (`Type.hs:1232-1235`), but `tInitializedType` also calls `tField` (`Type.hs:302`). Can an
   initializer list target a reference type?
   *Probe:* a class member function initialising `this`-typed storage with `{...}` — needs a
   construction that produces a `TReference` on the left of `tInitializedType`.

5. **`auto` return with a nested `return`.** `returnType Auto` searches only top-level body
   statements (`Type.hs:1211-1217`). Does `inline auto f(bool c) { if (c) { return 1u8; }
   return 2u8; }` deduce `uint8` or `void`? (`moveFunctions` hoists the *last* statement of
   an `if` body, `Function.hs:110-117`, which may or may not expose the `return`.)
   *Probe:* `inline auto f(bool c) { if (c) { return 1u8; } return 2u8; } inline void main() { uint8 x = f(true); }`

6. **Whether the `sig` vector can oscillate.** `Inference.hs:38` asserts monotone decrease
   in a comment. `setTypeOf` can rewrite a `TInstance`'s inner type (`Type.hs:1051-1057`) —
   can a node go from resolved back to unresolved, making the loop non-terminating on some
   input?
   *Probe:* a mutually recursive template pair whose instantiation depends on the other's
   deduced return type; compare `--frontend-passes` truncations to observe round counts.

7. **`TString` in `concat`.** The front-end sums widths and `string` has width 0
   (`Type.hs:705`, `Type.hs:1199`) while C++ rejects zero-width concat elements
   (`cpp/parse_tree.cpp:5473-5478`). Which diagnostic does the user see?
   *Probe:* `inline void main() { string s; uint4 a; auto x = concat(s, a); }`

8. **`ParamUint` deduced from a signed actual.** There is no
   `deduceType (ParamUint p) (TSigned x)` clause (`Type.hs:1554-1559`). Is the failure a
   "Could not deduce all arguments" error or a silent fallback to a default?
   *Probe:* `template <uint N> inline void f(uint<N> x) {} inline void main() { int8 y; f(y); }`

9. **Exact `unmangle` rendering inside conversion messages.** `showUnmangled` uses
   `unmangle` (`TypeCheck.hs:36`, `Type.hs:137-176`), which renders template instances as
   `Name<args>`. Confirm the exact spelling of an instance type in a narrowing warning for
   diagnostic-compatibility golden tests.
   *Probe:* `template <typename T> struct S { T x; } inline void f(S<uint4> s) {} inline void main() { S<int8> s; f(s); }` with `--Wconversion`.

10. **Whether `warning:` lines in `test/syntax/*.k` are matched by kind only.**
    `test/syntax/syntax-test.py` drives these fixtures; the exact matching rule (kind name
    vs. full text) determines how much of §6.3 is load-bearing for the test harness.
    *Probe:* read `test/syntax/syntax-test.py` and `test/syntax/CMakeLists.txt`.
