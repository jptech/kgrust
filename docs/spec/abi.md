# Kanagawa frontend → backend C ABI — implementation specification

> **Status, 2026-10-11.** Extracted at kanagawa `8e0a3fb` (2026-05-18); the
> reference is pinned at `98a2d8c` (`reference.lock`). Of the files cited
> here only `compiler/hs/app/Main.hs` changed since (the `list-deps` mode,
> #136), and its four citations are re-derived at the pin; `parse_tree.h`,
> `options.h` and the rest of the backend boundary did not change (#121's
> `hw.typealias` work is in `circt_util.cpp` and `verilog.cpp`, which this
> spec does not cite). §11.2 and §14 were written before the code; §14 is
> built now (its "As built" notes), and where the design and the crates
> differ, the crates' `lib.rs` are authoritative.

Reference tree: `third_party/kanagawa` (read-only).
All `path:line` citations below are relative to that root unless already absolute.

This document is the authoritative contract that the Rust crates `kg-ptbridge`
(the walk into the ABI, behind a `Backend` trait), `kg-backend-sys` (raw
bindings and the `Options` marshalling), `kg-ptrace` (the call interposer
that records the reference), `kg-pttrace` (the trace format both sides
write) and `kg-abi-gen` (bindgen over the headers, for the checked-in
bindings and the interposer's build script) must reproduce bit-for-bit.

---

## Contents

| § | topic |
|---|---|
| 0 | Provenance, linkage, scalar type mapping, opaque handle |
| 1 | Function inventory — **exact count: 85** in `parse_tree.h` (+1 `InternalTests`) |
| 2 | Every enum and constant crossing the boundary |
| 3 | `Options` / `CodeGenOptions` / `PlacementOptions` field-by-field with offsets |
| 4 | All 85 functions: signature, Haskell import, callers |
| 5 | The internal-caller graph — what an interposer can and cannot see |
| 6 | The stateful protocol + worked example |
| 7 | Integer and float literal marshalling |
| 8 | Scope array direction |
| 9 | `atomic` → `[[schedule(1)]]` |
| 10 | `data.closure.core::Closure` and `make_captureN` |
| 11 | The 18 `undefined` node kinds |
| 12 | `InitCompiler` / `Codegen` / `Compiler::Reset` and the N+1 passes |
| 13 | `errors.h` code table (JSON) |
| 14 | The `kg-ptrace` interposer: design, and what was built |
| 15 | Open questions |

---

## 0. Provenance and shape of the boundary

The reference compiler is split in two:

* **Front end** — Haskell, `compiler/hs`, produces the executable `kanagawa`
  (`compiler/kanagawa.cabal:25`).
* **Back end** — C++, `compiler/cpp`, built as a *shared library*
  (`compiler/cpp/CMakeLists.txt:67`) whose output name is `kanagawa-backend`
  (`CMakeLists.txt:56`), i.e. `libkanagawa-backend.so` / `libkanagawa-backend.dylib` /
  `kanagawa-backend.dll` (`compiler/cpp/CMakeLists.txt:228`).

The front end links against it directly (`compiler/kanagawa.cabal:55-58`) and
finds it at runtime by rpath: `$ORIGIN` on Linux
(`compiler/kanagawa.cabal:122-123`), `@loader_path` and `@loader_path/../..` on
macOS (`compiler/kanagawa.cabal:125-128`).

There is exactly one header describing the boundary — `compiler/cpp/parse_tree.h`
— plus `compiler/cpp/options.h` which it `#include`s (`compiler/cpp/parse_tree.h:9`).
Both are shipped to the Haskell build as `extra-source-files`
(`compiler/kanagawa.cabal:14`) and consumed by `hsc2hs`
(`compiler/hs/app/ParseTree/Types.hsc:19`, `compiler/hs/app/Options/FFI.hsc:23`).

`extern "C"` blocks exist in exactly three files in `compiler/`:

| file | line |
|---|---|
| `compiler/cpp/options.h` | 7 |
| `compiler/cpp/parse_tree.h` | 12 |
| `compiler/cpp/internal_tests.h` | 10 |

(verified: `grep -rn 'extern "C"' compiler/cpp compiler/hs` returns only these
three plus a comment at `compiler/cpp/internal_tests.cpp:2509`).

### 0.1 The `API` macro

```c
#ifdef EXPORT_API
#define API __declspec(dllexport)
#endif
#ifndef API
#define API
#endif
```
`compiler/cpp/parse_tree.h:26-32`. `EXPORT_API` is defined **only** for MSVC
(`compiler/cpp/CMakeLists.txt:88-91`). No `-fvisibility=hidden` is set anywhere
in the build, so on ELF/Mach-O every symbol is default-visibility and
interposable. This is what makes the interposer of §14 (`crates/kg-ptrace`) possible.

### 0.2 Scalar type mapping (LP64 / x86-64 SysV and AArch64 AAPCS64, verified by compilation)

| C type | bytes | Rust |
|---|---|---|
| `size_t` | 8 | `usize` |
| `BOOL` (`typedef unsigned BOOL`, `compiler/cpp/options.h:18`) | 4 | `c_uint` |
| `bool` (C99 `<stdbool.h>`, `compiler/cpp/parse_tree.h:7`) | 1 | `bool` |
| `int`, `unsigned int` | 4 | `c_int` / `c_uint` |
| `float` | 4 | `f32` |
| `const char*`, `ParseTreeNodePtr` | 8 | raw pointer |
| unscoped C `enum` (all of §2) | 4 | `c_int` (values are all ≥ 0 and < 2³¹) |

Measured on darwin/arm64 with `cc` over the real headers; the numbers are the
same for x86-64 Linux since both are LP64 with the same struct-layout rules.

### 0.3 The opaque node handle

```c
typedef struct _ParseTreeNode* ParseTreeNodePtr;
```
`compiler/cpp/parse_tree.h:34`. `_ParseTreeNode` is a polymorphic C++ class
(`compiler/cpp/parse_tree.h:1719-1849`) — it has a vtable, `std::string`
members in subclasses, etc. **The front end must treat it as strictly opaque.**
It is never freed by the front end; ownership stays with the `Compiler`
singleton via `Compiler::Create<T>` (`compiler/cpp/compiler.h:337-355`), which
records a destructor closure on either `_permanentParseTreeCleanupList` or
`_temporaryParseTreeCleanupList`.

Every node also carries a monotonic `_sequenceNumber` assigned from the global
`g_currParseTreeSequenceNumber` (`compiler/cpp/parse_tree.cpp:1190`,
`compiler/cpp/parse_tree.cpp:1193-1194`), *"a number that uniquely identifies
this node ... Useful for sorting nodes in a way that doesn't change based on
pointers returned from the heap"* (`compiler/cpp/parse_tree.h:1791-1794`). This
is the backend's own logical-id scheme and is the model for §14.2's trace ids.

`NULL` is a legal argument almost everywhere and is used by the Haskell side to
mean "absent". The complete list of `nullPtr` uses in the front-end walk:

| site | meaning |
|---|---|
| `compiler/hs/app/ParseTree.hs:112` | `Function … Nothing …` — no `inline`/`noinline` modifier |
| `compiler/hs/app/ParseTree.hs:116` | `FunctionSpecifier` with no object (free function) |
| `compiler/hs/app/ParseTree.hs:118` | `FunctionTypeParam` with no parameter name |
| `compiler/hs/app/ParseTree.hs:120` | `If` with no `else` branch |
| `compiler/hs/app/ParseTree.hs:129` | `ModuleDiff` — produces no node at all |
| `compiler/hs/app/ParseTree.hs:158` | `SwitchDefault` — no case value |
| `compiler/hs/app/ParseTree.hs:173` | `Variable` with no initialiser |
| `compiler/hs/app/ParseTree.hs:180` | `withUnmangledName` for a non-instance — no unmangled name |
| `compiler/hs/app/ParseTree.hs:210` | `parseList` — empty list base |
| `compiler/hs/app/ParseTree.hs:219` | interpolation segment with no expression |
| `compiler/hs/app/ParseTree.hs:221` | interpolation expression with no alignment |
| `compiler/hs/app/ParseTree.hs:242` | `parseInstanceType` for a non-instance type |
| `compiler/hs/app/ParseTree.hs:253` | template argument of an unhandled kind |

---

## 1. Function inventory — exact count

**`compiler/cpp/parse_tree.h` declares exactly 85 `API`-decorated `extern "C"`
functions** (`grep -c '^\s*API ' compiler/cpp/parse_tree.h` → 85, lines 166–257).

`compiler/cpp/internal_tests.h:12` declares one more, `API int InternalTests();`,
in its own `extern "C"` block, for a **total of 86 exported C entry points** in
the backend library.

> **Discrepancy with the plan.** The plan text says "87 forwarders". The
> verified count is 85 (`parse_tree.h`) or 86 (including `InternalTests`).
> There is no 87th `extern "C"` symbol in the tree. See §14.3 for how
> the interposer (`crates/kg-ptrace`) derives the number from the bindgen AST
> rather than hard-coding it.

The Haskell front end imports **82** of them
(`grep -c '^foreign import ccall' compiler/hs/app/ParseTree/FFI.hs` → 82).

The three declared-but-not-imported symbols are:

| symbol | decl | status |
|---|---|---|
| `ParseKnownConstNamedVariable` | `compiler/cpp/parse_tree.h:200` | defined `compiler/cpp/parse_tree.cpp:6018`; called only from `ParseDeclare` (`compiler/cpp/parse_tree.cpp:5562`) |
| `ParseStages` | `compiler/cpp/parse_tree.h:216` | **has no definition anywhere in the tree** (`grep -rn 'ParseStages' --include='*.cpp' --include='*.h' --include='*.hs'` matches only the declaration). Dead declaration; a Rust `extern` block that declares it will fail to link on some platforms. |
| `SetLocation` | `compiler/cpp/parse_tree.h:252` | defined `compiler/cpp/parse_tree.cpp:6270`; called only from `SetLocation2` (`compiler/cpp/parse_tree.cpp:6276`). Takes `Location` **by value** — an ABI hazard the front end deliberately avoids by using `SetLocation2`. |

---

## 2. Enumerations and constants crossing the boundary

### 2.1 `DECLARE_FLAG_*` bit mask (`compiler/cpp/parse_tree.h:16-25`)

| macro | value | line |
|---|---|---|
| `DECLARE_FLAG_CONST` | `0x00000001` | 16 |
| `DECLARE_FLAG_GLOBAL` | `0x00000002` | 17 |
| `DECLARE_FLAG_INITIALIZE` | `0x00000004` | 18 |
| `DECLARE_FLAG_PARAMETER` | `0x00000008` | 19 |
| `DECLARE_FLAG_PER_THREAD` | `0x00000010` | 20 |
| `DECLARE_FLAG_END_TRANSACTION` | `0x00000020` | 21 |
| `DECLARE_FLAG_STATIC` | `0x00000040` | 22 |
| `DECLARE_FLAG_CLASS` | `0x00000080` | 23 |
| `DECLARE_FLAG_UNINIT_CONST` | `0x00000100` | 24 |

Composite values the front end actually sends (`compiler/hs/app/ParseTree/Types.hsc:45-50`,
selected by `declareFlag` at `compiler/hs/app/ParseTree.hs:182-188`):

| Haskell `DeclareFlag` | pattern | numeric | hsc line |
|---|---|---|---|
| `DeclareGlobal` | `DeclareFlagGlobal` | `GLOBAL\|INITIALIZE` = `0x06` | 45 |
| `DeclareClass` | `DeclareFlagClass` | `GLOBAL\|INITIALIZE\|CLASS` = `0x86` | 46 |
| `DeclareLocal` | `DeclareFlagLocal` | `INITIALIZE` = `0x04` | 47 |
| `DeclareStatic` | `DeclareFlagStatic` | `GLOBAL\|INITIALIZE\|STATIC` = `0x46` | 48 |
| `DeclareMember` | `DeclareFlagMember` | `0` | 49 |
| `DeclareUninitConst` | `DeclareFlagUninitConst` | `UNINIT_CONST` = `0x100` | 50 |

`DECLARE_FLAG_PARAMETER` (`0x08`) is added by the backend inside
`ParseFunctionParam` (`compiler/cpp/parse_tree.cpp:5526`), never by the front end.
`DECLARE_FLAG_CONST` (`0x01`) is added by `ParseDeclare` when the type node is a
`ConstNode` (`compiler/cpp/parse_tree.cpp:5539-5543`).
`DECLARE_FLAG_END_TRANSACTION` (`0x20`) reaches the backend as a
*`FlagAttributeNode` payload*, not as a `flags` bit — see §2.10.
`DECLARE_FLAG_PER_THREAD` (`0x10`) is never produced by the current front end.

### 2.2 `Location` (`compiler/cpp/parse_tree.h:36-44`)

```c
typedef struct {
    size_t _beginLine;
    size_t _beginColumn;
    size_t _endLine;
    size_t _endColumn;
    size_t _fileIndex;
    bool   _valid;
} Location;
```

Verified layout (LP64): `size = 48`, `align = 8`.

| field | offset | size |
|---|---|---|
| `_beginLine` | 0 | 8 |
| `_beginColumn` | 8 | 8 |
| `_endLine` | 16 | 8 |
| `_endColumn` | 24 | 8 |
| `_fileIndex` | 32 | 8 |
| `_valid` | 40 | 1 |
| *(tail padding)* | 41–47 | 7 |

`bool operator<(const Location&, const Location&)` is declared for C++ only
(`compiler/cpp/parse_tree.h:264`).

**Trap.** The Haskell `Storable Location` instance writes only the first five
fields (`compiler/hs/app/ParseTree/Types.hsc:32-37`) — `_valid` and the 7 tail
padding bytes are left as whatever `alloca`
(`compiler/hs/app/ParseTree/Types.hsc:41`) returned. This is benign only because
`SetLocation` unconditionally overwrites `_valid` with `true`
(`compiler/cpp/parse_tree.cpp:6272-6273`). A Rust reimplementation should zero
the struct; it must not *rely* on the caller's `_valid`.

Line and column are 1-based: they come from megaparsec `unPos . sourceLine` /
`unPos . sourceColumn` (`compiler/hs/app/ParseTree/Types.hsc:33-36`).
`_fileIndex` is an index into the `Options._fileNames` array, computed as
`fromJust $ elemIndex (sourceName begin) allFiles`
(`compiler/hs/app/ParseTree.hs:61`), and `allFiles` is exactly the list handed to
`InitCompiler` (`compiler/hs/app/ParseTree.hs:38`, `compiler/hs/app/Options/FFI.hsc:32,39`).

### 2.3 `ParseTreeBinaryOpType` (`compiler/cpp/parse_tree.h:46-67`)

| value | name | Haskell `BinaryOp` ctor (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:75-81`) |
|---|---|---|
| 0 | `ParseTreeBinaryOpTypeAdd` | `Add` |
| 1 | `ParseTreeBinaryOpTypeSub` | `Sub` |
| 2 | `ParseTreeBinaryOpTypeMul` | `Mul` |
| 3 | `ParseTreeBinaryOpTypeLutMul` | `LutMul` |
| 4 | `ParseTreeBinaryOpTypeDiv` | `Div` |
| 5 | `ParseTreeBinaryOpTypeMod` | `Mod` |
| 6 | `ParseTreeBinaryOpTypeAnd` | `BitwiseAnd` |
| 7 | `ParseTreeBinaryOpTypeLogicalAnd` | `And` |
| 8 | `ParseTreeBinaryOpTypeOr` | `BitwiseOr` |
| 9 | `ParseTreeBinaryOpTypeLogicalOr` | `Or` |
| 10 | `ParseTreeBinaryOpTypeXor` | `BitwiseXor` |
| 11 | `ParseTreeBinaryOpTypeLogicalXor` | `Xor` |
| 12 | `ParseTreeBinaryOpTypeShl` | `ShiftLeft` |
| 13 | `ParseTreeBinaryOpTypeShr` | `ShiftRight` |
| 14 | `ParseTreeBinaryOpTypeEQ` | `Equal` |
| 15 | `ParseTreeBinaryOpTypeNE` | `NotEqual` |
| 16 | `ParseTreeBinaryOpTypeGT` | `Greater` |
| 17 | `ParseTreeBinaryOpTypeGE` | `GreaterEqual` |
| 18 | `ParseTreeBinaryOpTypeLT` | `Less` |
| 19 | `ParseTreeBinaryOpTypeLE` | `LessEqual` |

The correspondence is by ordinal — the front end passes `fromEnum op`
(`compiler/hs/app/ParseTree.hs:73`) and the Haskell source says so explicitly:
*"Enum types' constructors must be defined in the same order as values in
coresponding C enums defined in parse_tree.h"*
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:71-73`).

### 2.4 `ParseTreeUnaryOpType` (`compiler/cpp/parse_tree.h:69-74`)

| value | name | Haskell `UnaryOp` (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:106-113`) |
|---|---|---|
| 0 | `ParseTreeUnaryOpTypeInvert` | `Invert` |
| 1 | `ParseTreeUnaryOpTypeNegate` | `Neg` |
| 2 | `ParseTreeUnaryOpTypeLogicalInvert` | `Not` |

Haskell `UnaryOp` has three further constructors — `BitSizeof` (3),
`ByteSizeof` (4), `Clog2` (5) — that are **never** passed to `ParseUnaryOp`.
`BitSizeof`/`ByteSizeof` are routed to `ParseSizeOf`
(`compiler/hs/app/ParseTree.hs:166-167`) and `Clog2` is `undefined`
(`compiler/hs/app/ParseTree.hs:168`, see §11).

### 2.5 `ParseTreeLoopMode` (`compiler/cpp/parse_tree.h:76-81`)

| value | name |
|---|---|
| 0 | `ParseTreeLoopModeOrdered` |
| 1 | `ParseTreeLoopModeUnordered` |
| 2 | `ParseTreeLoopModeReorderByLooping` |

No ABI function takes this type; it is used internally. It is in the header and
therefore in the bindgen output, so `kg-backend-sys` should still emit it.

### 2.6 `ParseTreeSizeofType` (`compiler/cpp/parse_tree.h:83-87`)

| value | name | Haskell pattern (`compiler/hs/app/ParseTree/Types.hsc:52-53`) |
|---|---|---|
| 0 | `ParseTreeSizeofTypeBit` | `SizeofTypeBit` |
| 1 | `ParseTreeSizeofTypeByte` | `SizeofTypeByte` |

### 2.7 `ParseTreeFunctionModifier` (`compiler/cpp/parse_tree.h:89-115`) — a bit mask

| value | name | Haskell pattern |
|---|---|---|
| `0` | `ParseTreeFunctionModifierNone` | — |
| `1 << 0` = 1 | `ParseTreeFunctionModifierInline` | `InlineFunc` (`Types.hsc:58`) |
| `1 << 1` = 2 | `ParseTreeFunctionModifierAsync` | `AsyncFunc` (`Types.hsc:61`) |
| `1 << 2` = 4 | `ParseTreeFunctionModifierExport` | — (marked *"TODO: FOSS - Remove after this has been removed from front-end (PR 39888)"*, `parse_tree.h:92`) |
| `1 << 3` = 8 | `ParseTreeFunctionModifierPipelined` | `PipelinedFunc` (`Types.hsc:62`) |
| `1 << 4` = 16 | `ParseTreeFunctionModifierIntrinsic` | — (backend-internal, `compiler/cpp/compiler.cpp:61-62`) |
| `1 << 5` = 32 | `ParseTreeFunctionModifierUnordered` | `UnorderedFunc` (`Types.hsc:63`) |
| `1 << 6` = 64 | `ParseTreeFunctionModifierExternal` | — (backend adds it, `compiler/cpp/parse_tree.cpp:6144`) |
| `1 << 7` = 128 | `ParseTreeFunctionModifierExternalFixedLatency` | — (backend, `compiler/cpp/ir.cpp:12022`) |
| `1 << 8` = 256 | `ParseTreeFunctionModifierNoBackPressure` | `NoBackPressureFunc` (`Types.hsc:65`) |
| `1 << 9` = 512 | `ParseTreeFunctionModifierReorderByLooping` | `ReorderByLoopingFunc` (`Types.hsc:67`) |
| `1 << 10` = 1024 | `ParseTreeFunctionModifierNoInline` | `NoInlineFunc` (`Types.hsc:59`) |
| `1 << 11` = 2048 | `ParseTreeFunctionModifierExternalExternallyInstantiated` | — (backend, `compiler/cpp/ir.cpp:12023`) |
| `1 << 13` = 8192 | `ParseTreeFunctionModifierReset` | `ResetFunc` (`Types.hsc:64`) |
| `1 << 14` = 16384 | `ParseTreeFunctionModifierExportClassInterface` | — (backend, `compiler/cpp/compiler.cpp:2078-2079`) |
| `1 << 15` = 32768 | `ParseTreeFunctionModifierPure` | `PureFunc` (`Types.hsc:66`) |
| `1 << 16` = 65536 | `ParseTreeFunctionModifierNoSrcTxWarning` | — |

**`1 << 12` (4096) is deliberately skipped** — the enum jumps from bit 11 to
bit 13 (`compiler/cpp/parse_tree.h:100-101`). Do not "fix" this in Rust.

Modifiers are folded by OR in `GetFunctionModifiers`
(`compiler/cpp/parse_tree.cpp`, function body quoted at the `ParseFunctionType`
call site `compiler/cpp/parse_tree.cpp:5845`); specifying the same modifier
twice is `CompileError::InvalidFunctionModifier`.

### 2.8 `ParseTreeMemberProtectionModifier` (`compiler/cpp/parse_tree.h:117-121`)

| value | name | Haskell (`compiler/hs/app/ParseTree/Types.hsc:55-56`, used at `ParseTree.hs:137-138`) |
|---|---|---|
| 0 | `ParseTreeMemberProtectionModifierPublic` | `MemberModifierPublic` |
| 1 | `ParseTreeMemberProtectionModifierPrivate` | `MemberModifierPrivate` |

### 2.9 `ParseTreeArrayType` (`compiler/cpp/parse_tree.h:123-128`)

| value | name |
|---|---|
| 0 | `ParseTreeArrayTypeDefault` |
| 1 | `ParseTreeArrayTypeMemory` |

Never crosses the boundary as a parameter; computed inside `ParseArrayType`
(`compiler/cpp/parse_tree.cpp:5720`, `:5736-5740`).

### 2.10 `ParseTreeMemoryType` (`compiler/cpp/parse_tree.h:130-137`) — **overloaded namespace, read carefully**

| value | name | Haskell pattern (`Types.hsc:80-84`) | how it reaches C++ |
|---|---|---|---|
| 0 | `ParseTreeMemoryTypeDefault` | `MemoryDefault` | `ParseFlagAttribute(0)` for `[[memory]]` (`ParseTree.hs:99`) |
| 1 | `ParseTreeMemoryTypeNoReplication` | `MemoryNorep` | `ParseFlagAttribute(1)` for `[[non_replicated]]` (`ParseTree.hs:102`) |
| 2 | `ParseTreeMemoryTypeQuadPort` | `MemoryQuadPort` | `ParseFlagAttribute(2)` for `[[quad_port]]` (`ParseTree.hs:104`) |
| 3 | `ParseTreeMemoryTypeEcc` | `MemoryEcc` | `ParseAttribute(3, funcName)` for `[[ecc(f)]]` (`ParseTree.hs:190-193`) |
| 4 | `ParseTreeMemoryTypeInitialize` | `MemoryInitialize` | `ParseFlagAttribute(4)` for `[[initialize]]` (`ParseTree.hs:98`) |

The `unsigned _attribute` field of `BaseAttributeNode`
(`compiler/cpp/parse_tree.h:3577`, `:3586`) is a **context-dependent integer**:

* On a `FlagAttributeNode` inside an *array-type* attribute list, it is a
  `ParseTreeMemoryType` (`compiler/cpp/parse_tree.cpp:5734-5755`). Note that
  `attr == ParseTreeMemoryTypeDefault` is `attr == 0` and means *"this array is a
  memory"* (`compiler/cpp/parse_tree.cpp:5736-5740`).
* On a `FlagAttributeNode` inside a *function-parameter* attribute list, it is a
  `DECLARE_FLAG_*` bit that gets OR-ed into the declare flags
  (`compiler/cpp/parse_tree.cpp:5528-5533`), and is separately tested against
  `DECLARE_FLAG_END_TRANSACTION` for `[[last]]`
  (`compiler/cpp/parse_tree.cpp:8023`).
* On an `IntAttributeNode`, it is a `ParseTreeAttribute` (§2.11).
* On an `AttributeNode`, it is either `ParseTreeMemoryTypeEcc` (=3,
  `compiler/cpp/parse_tree.cpp:5759`) or `ParseTreeNameAttr` (=8,
  `compiler/cpp/parse_tree.cpp:6853`, `:6890`).

So the same integer 3 means `Ecc` on an `AttributeNode` and `Latency` on an
`IntAttributeNode`. Disambiguation is *purely by C++ node class*, which the ABI
does not expose. A Rust `LoweredExp` must therefore keep the attribute kind
(flag / int / node-valued) and the attribute code as one tagged unit.

`CheckForMultipleAttributes` (`compiler/cpp/parse_tree.cpp:52-72`) inserts
`baseAttr->_attribute` into a single `std::set<unsigned>` across *all*
`BaseAttributeNode` subclasses in a list — so a list holding
`IntAttributeNode(Latency=3)` and `AttributeNode(Ecc=3)` would spuriously report
"The same attribute cannot be specified multiple times". Unreachable today
because the two never appear in the same list, but it is a latent aliasing bug
to be aware of when designing the Rust representation.

### 2.11 `ParseTreeAttribute` (`compiler/cpp/parse_tree.h:139-151`)

| value | name | Haskell pattern (`Types.hsc:71-78`) | produced by |
|---|---|---|---|
| 0 | `ParseTreeCallRateAttr` | `CallRateAttr` | `intAttr CallRate` (`ParseTree.hs:196`) |
| 1 | `ParseTreeClockAttr` | — | **never produced by the current front end** |
| 2 | `ParseTreeFifoDepthAttr` | `FifoDepthAttr` | `ParseTree.hs:197` |
| 3 | `ParseTreeLatencyAttr` | `LatencyAttr` | `ParseTree.hs:198`; also backend-internal `compiler/cpp/ir.cpp:12028` |
| 4 | `ParseTreeMaxThreadsAttr` | `MaxThreadsAttr` | `ParseTree.hs:199` |
| 5 | `ParseTreeScheduleAttr` | `ScheduleAttr` | `ParseTree.hs:200`; and the `atomic` desugaring `ParseTree.hs:96` (§9) |
| 6 | `ParseTreeThreadRateAttr` | `ThreadRateAttr` | `ParseTree.hs:201` |
| 7 | `ParseTreeTransactionSizeAttr` | `TransactionSizeAttr` | `ParseTree.hs:202` |
| 8 | `ParseTreeNameAttr` | `NameAttr` | `attr Rename` (`ParseTree.hs:192`) — via `ParseAttribute`, not `ParseIntAttribute` |
| 9 | `ParseTreeMarshalAttr` | — | **never produced by the current front end** |

The Haskell `Attribute` enum (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:124-133`)
has 9 constructors: `CallRate`, `Ecc`, `FifoDepth`, `Latency`, `MaxThreads`,
`Rename`, `Schedule`, `ThreadRate`, `TransactionSize`. `Ecc` and `Rename` route
through `ParseAttribute`; the other seven through `ParseIntAttribute`; anything
else is `undefined` (`compiler/hs/app/ParseTree.hs:193`, `:203`).

### 2.12 `ParseTreeFormatSpecifier` (`compiler/cpp/parse_tree.h:153-161`)

| value | name | Haskell pattern (`Types.hsc:86-91`) | source syntax (`Syntax.hs:229-234`) |
|---|---|---|---|
| 0 | `ParseTreeFormatSpecifierBin` | `FormatSpecifierBin` | `:b` |
| 1 | `ParseTreeFormatSpecifierOct` | `FormatSpecifierOct` | `:o` |
| 2 | `ParseTreeFormatSpecifierDec` | `FormatSpecifierDec` | `:d` |
| 3 | `ParseTreeFormatSpecifierHex` | `FormatSpecifierHex` | `:x` |
| 4 | `ParseTreeFormatSpecifierHexUpper` | `FormatSpecifierHexUpper` | `:X` |
| 5 | `ParseTreeFormatSpecifierNone` | `FormatSpecifierNone` | absent (default, `ParseTree.hs:222`) |

### 2.13 Scope pointer typedefs (`compiler/cpp/parse_tree.h:163-164`)

```c
typedef const char* const* ParseNamespaceScopePtr;
typedef const char* const* ParseQualifiedNamePtr;
```
Both are NUL-pointer-terminated arrays of NUL-terminated UTF-8 C strings. See §8
for the ordering contract.

### 2.14 `VerbosityLevel` (`compiler/cpp/options.h:11-16`)

| value | name |
|---|---|
| 0 | `Quiet` |
| 1 | `Normal` |
| 2 | `Verbose` |

Written into `CodeGenOptions._verbosity` as `fromEnum verbosity`
(`compiler/hs/app/Options/FFI.hsc:36`), read back by `Verbosity()`
(`compiler/cpp/config.cpp:241`).

---

## 3. `Options` / `CodeGenOptions` / `PlacementOptions`

All three are declared in `compiler/cpp/options.h` inside its `extern "C"` block
(`compiler/cpp/options.h:7`). Offsets below were obtained by compiling the real
header (LP64, `size_t`=8, `BOOL`=4, `float`=4, pointer=8).

### 3.1 `PlacementOptions` (`compiler/cpp/options.h:20-41`) — size 40, align 8

| # | field | C type | off | size | decl line | Haskell writes | source field |
|---|---|---|---|---|---|---|---|
| 1 | `_numIterations` | `size_t` | 0 | 8 | 23 | yes (`FFI.hsc:120`) | `place_iterations :: Int` |
| 2 | `_seed` | `size_t` | 8 | 8 | 26 | yes (`:121`) | `place_seed :: Int` |
| 3 | `_pullStrength` | `float` | 16 | 4 | 29 | yes (`:122`) | `place_pull :: Float` |
| 4 | `_pushStrength` | `float` | 20 | 4 | 32 | yes (`:123`) | `place_push :: Float` |
| 5 | `_wallStrength` | `float` | 24 | 4 | 35 | yes (`:124`) | `place_wall :: Float` |
| 6 | `_updateRate` | `float` | 28 | 4 | 38 | yes (`:125`) | `place_update_rate :: Float` |
| 7 | `_display` | `BOOL` | 32 | 4 | 41 | yes (`:126`) | `place_display :: Bool` |
| — | *tail padding* | | 36–39 | 4 | | **never written** | |

All 7 fields are written by Haskell. Post-hoc override: if `_optimize < 2`,
`SetupCodeGenConfig` forces `_numIterations = 1`
(`compiler/cpp/config.cpp:189-192`).

### 3.2 `CodeGenOptions` (`compiler/cpp/options.h:43-273`) — size 448, align 8

67 fields, declaration order. "Written" = poked by
`instance Storable Options` (`compiler/hs/app/Options/FFI.hsc:64-137`) or by
`withOptions` (`:36`).

| # | field | C type | off | size | decl | written at | Haskell expression |
|---|---|---|---|---|---|---|---|
| 1 | `_debug` | `BOOL` | 0 | 4 | 48 | `FFI.hsc:127` | `debug` |
| 2 | `_stripDebugSymbols` | `BOOL` | 4 | 4 | 51 | `:128` | `strip_debug_symbols` |
| 3 | `_optimize` | `size_t` | 8 | 8 | 54 | `:101` | `optimize` |
| 4 | `_assertNarrowingConversion` | `BOOL` | 16 | 4 | 57 | `:92` | `check_narrowing_assignment` |
| 5 | `_dumpIR` | `BOOL` | 20 | 4 | 60 | `:93` | `dump_IR` |
| 6 | `_dumpOpt` | `BOOL` | 24 | 4 | 63 | `:94` | `dump_opt` |
| 7 | `_serializeIRPostOpt` | `BOOL` | 28 | 4 | 66 | `:95` | `write_ir_post_opt` |
| 8 | `_serializeCIRCTIR` | `BOOL` | 32 | 4 | 69 | `:96` | `write_circt_ir` |
| 9 | `_skipCirctLowering` | `BOOL` | 36 | 4 | 73 | `:97` | `skip_circt_lowering` |
| 10 | `_detectRaces` | `BOOL` | 40 | 4 | 76 | **never** | forced `false` at `config.cpp:143` |
| 11 | `_controlInspection` | `BOOL` | 44 | 4 | 79 | **never** | forced `false` at `config.cpp:142` |
| 12 | `_inspection` | `BOOL` | 48 | 4 | 82 | **never** | forced `false` at `config.cpp:141` |
| 13 | `_releaseAssert` | `BOOL` | 52 | 4 | 85 | `:112` | `not no_asserts` |
| 14 | `_allowFifoStutter` | `BOOL` | 56 | 4 | 88 | `:108` | `not no_fifo_stutter` |
| 15 | `_stallablePipelines` | `BOOL` | 60 | 4 | 91 | `:109` | `stallable_pipelines` |
| 16 | `_clockGatingLevel` | `size_t` | 64 | 8 | 94 | `:106` | `clock_gating` |
| 17 | `_clockGatingThreshold` | `size_t` | 72 | 8 | 97 | `:107` | `clock_gating_threshold` |
| 18 | `_conditionalMemoryReads` | `BOOL` | 80 | 4 | 101 | `:110` | `conditional_memory_reads` |
| — | *padding* | | 84–87 | 4 | | never | |
| 19 | `_semaphoreDelay` | `size_t` | 88 | 8 | 106 | `:111` | `semaphore_delay` |
| 20 | `_codeCoverage` | `BOOL` | 96 | 4 | 109 | `:114` | `code_coverage` |
| — | *padding* | | 100–103 | 4 | | never | |
| 21 | `_codeCoverageMuxThreshold` | `size_t` | 104 | 8 | 113 | `:115` | `code_coverage_mux_threshold` |
| 22 | `_dumpMemoryMetadata` | `BOOL` | 112 | 4 | 116 | `:116` | `dump_memory_metadata` |
| — | *padding* | | 116–119 | 4 | | never | |
| 23 | `_maxBlockRam` | `size_t` | 120 | 8 | 119 | `:72` | `max_block_ram` |
| 24 | `_maxDeepRam` | `size_t` | 128 | 8 | 120 | `:74` | `max_deep_ram` |
| 25 | `_minBlockRamUtilization` | `size_t` | 136 | 8 | 124 | `:73` | `block_ram_util_threshold` |
| 26 | `_minDeepRamUtilization` | `size_t` | 144 | 8 | 125 | `:75` | `deep_ram_util_threshold` |
| 27 | `_maxSelectInputs` | `size_t` | 152 | 8 | 128 | `:76` | `max_mux_sources` |
| 28 | `_maxMulSrcWidth` | `size_t` | 160 | 8 | 131 | `:77` | `max_mul_src_width` |
| 29 | `_maxThreadsDefault` | `size_t` | 168 | 8 | 134 | `:78` | `max_threads_default` |
| 30 | `_maxThreadsLimit` | `size_t` | 176 | 8 | 138 | `:79` | `max_threads_limit` |
| 31 | `_scheduleLimit` | `size_t` | 184 | 8 | 141 | `:80` | `schedule_limit` |
| 32 | `_additionalLatency` | `size_t` | 192 | 8 | 144 | `:68` | `additional_latency` |
| 33 | `_additionalResetCycles` | `size_t` | 200 | 8 | 148 | `:69` | `reset_duration_cycles` |
| 34 | `_autoPipelineScale` | `size_t` | 208 | 8 | 155 | `:70` | `auto_pipeline_scale` |
| 35 | `_autoPipelineCrossRegion` | `size_t` | 216 | 8 | 162 | `:71` | `auto_pipeline_cross_region` |
| 36 | `_logicRegisterRatio` | `size_t` | 224 | 8 | 166 | `:102` | `register_ratio` |
| 37 | `_maxLogicRegisterRatio` | `size_t` | 232 | 8 | 169 | `:103` | `max_register_ratio` |
| 38 | `_resetCycles` | `size_t` | 240 | 8 | 172 | `:104` | `reset_cycles` |
| 39 | `_resetFanOutCycles` | `size_t` | 248 | 8 | 175 | `:105` | `reset_fan_out_cycles` |
| 40 | `_fifoWriteDelay` | `size_t` | 256 | 8 | 178 | `:98` | `fifo_write_delay` |
| 41 | `_fifoMergeDistance` | `size_t` | 264 | 8 | 181 | `:99` | `fifo_merge_distance` |
| 42 | `_carryChainWidthPerLogicLevel` | `size_t` | 272 | 8 | 184 | `:81` | `carry_chain_width` |
| 43 | `_rmwMemoryWriteDelay` | `size_t` | 280 | 8 | 187 | `:82` | `rmw_memory_write_delay` |
| 44 | `_rmwMemoryReadDelay` | `size_t` | 288 | 8 | 190 | `:83` | `rmw_memory_read_delay` |
| 45 | `_rmwHardenedMemoryWriteDelay` | `size_t` | 296 | 8 | 194 | `:84` | `rmw_hardened_write_delay` |
| 46 | `_rmwHardenedMemoryMaxWidth` | `size_t` | 304 | 8 | 200 | `:85` | `rmw_hardened_max_width` |
| 47 | `_rmwHardenedMemoryMaxHeight` | `size_t` | 312 | 8 | 201 | `:86` | `rmw_hardened_max_height` |
| 48 | `_rmwMemoryFavorArea` | `BOOL` | 320 | 4 | 205 | `:87` | `rmw_memory_favor_area` |
| 49 | `_strictMemoryReadWrite` | `BOOL` | 324 | 4 | 210 | `:88` | `strict_memory_read_write` |
| 50 | `_inputOutputRegisterDelay` | `size_t` | 328 | 8 | 213 | `:89` | `io_register_delay` |
| 51 | `_memToRegThreshold` | `float` | 336 | 4 | 220 | `:90` | `mem_to_reg :: Float` |
| — | *padding* | | 340–343 | 4 | | never | |
| 52 | `_fifoToDspThreshold` | `size_t` | 344 | 8 | 226 | `:91` | `fifo_to_dsp` |
| 53 | `_verbosity` | `size_t` | 352 | 8 | 228 | `FFI.hsc:36` (in `withOptions`, **not** in `poke`) | `fromEnum <$> getVerbosity` |
| 54 | `_maxStringLength` | `size_t` | 360 | 8 | 231 | `:117` | `identifier_length` |
| 55 | `_stall` | `size_t` | 368 | 8 | 234 | `:129` | `stall` |
| 56 | `_forceMemToArray` | `size_t` | 376 | 8 | 237 | `:130` | `force_mem_to_array` |
| 57 | `_suppressDebugView` | `BOOL` | 384 | 4 | 240 | `:113` | `no_debug_view` |
| — | *padding* | | 388–391 | 4 | | never | |
| 58 | `_logWorkListSize` | `size_t` | 392 | 8 | 244 | `:118` | `work_list_size` |
| 59 | `_returnStages` | `size_t` | 400 | 8 | 247 | `:119` | `return_stages` |
| 60 | `_maxStackDepth` | `size_t` | 408 | 8 | 250 | `:133` | `max_stack_depth` |
| 61 | `_resetFanout` | `BOOL` | 416 | 4 | 253 | `:134` | `not no_reset_fanout` |
| 62 | `_sparseRegOpt` | `BOOL` | 420 | 4 | 256 | `:131` | `sparse_reg_opt` |
| 63 | `_carrySelect` | `BOOL` | 424 | 4 | 259 | `:132` | `carry_select` |
| 64 | `_noVerilogHeader` | `BOOL` | 428 | 4 | 262 | `:100` | `no_verilog_header` |
| 65 | `_enableTransactionSizeWarning` | `BOOL` | 432 | 4 | 265 | `:135` | `Set.member TransactionSizeMissing (enabledWarnings warnings_state)` |
| 66 | `_enableDeprecationWarnings` | `BOOL` | 436 | 4 | 268 | `:136` | `Set.member Deprecated (enabledWarnings warnings_state)` |
| 67 | `_warningsAsErrors` | `BOOL` | 440 | 4 | 271 | `:137` | `warnings_as_errors` |
| — | *tail padding* | | 444–447 | 4 | | never | |

**Summary of what Haskell leaves untouched in `CodeGenOptions`:** exactly three
fields — `_detectRaces` (#10), `_controlInspection` (#11), `_inspection` (#12) —
plus all 7 padding runs. The backing memory comes from `alloca`
(`compiler/hs/app/Options/FFI.hsc:29`), which is **not zeroed**, so those bytes
are indeterminate on the wire. It is safe only because `SetupCodeGenConfig`
overwrites all three with `false` immediately
(`compiler/cpp/config.cpp:140-143`). A Rust implementation should
`MaybeUninit::zeroed()` the whole struct.

### 3.3 `Options` (`compiler/cpp/options.h:275-292`) — size 520, align 8

| # | field | C type | off | size | decl | written at |
|---|---|---|---|---|---|---|
| 1 | `_frequency` | `size_t` | 0 | 8 | 278 | `FFI.hsc:67` (`frequency :: Int`) |
| 2 | `_deviceName` | `const char*` | 8 | 8 | 281 | `FFI.hsc:38` (`withCAString target_device`) |
| 3 | `_cmdArgs` | `const char*` | 16 | 8 | 284 | `FFI.hsc:37` (`withCAString cmdArgs`) |
| 4 | `_codegenOptions` | `CodeGenOptions` | 24 | 448 | 286 | §3.2 |
| 5 | `_placementOptions` | `PlacementOptions` | 472 | 40 | 288 | §3.1 |
| 6 | `_fileNames` | `const char**` | 512 | 8 | 290 | `FFI.hsc:39` (`withStringArray allFiles`) |

`_fileNames` is a NULL-terminated array of C strings
(`compiler/hs/app/Options/FFI.hsc:44-58`), consumed by
`for (const char** p = options->_fileNames; *p; ++p)`
(`compiler/cpp/config.cpp:152-153`). It must be non-empty or
`SetupCodeGenConfig` throws `"No input files specified"`
(`compiler/cpp/config.cpp:172-175`).

**Lifetime.** Every pointer in `Options` is valid only for the duration of the
`InitCompiler` call — the Haskell comment says so
(`compiler/hs/app/Options/FFI.hsc:25-26`) and the C++ copies everything it needs
into `std::string`/`std::vector` (`compiler/cpp/config.cpp:145-153`).

### 3.4 hsc2hs poke width — a real hazard

`#{poke T, f} ptr val` expands to `pokeByteOff ptr OFFSET val`: **the number of
bytes written is decided by the Haskell type of `val`, not by the C type of the
field.** The reference build happens to line up because GHC's
`Storable Bool` writes `HsBool` (4 bytes, matching `BOOL`), `Storable Int`
writes 8 bytes (matching `size_t`) and `Storable Float` writes 4. Any Rust
port must pin the widths explicitly; do not assume the Haskell record types are
authoritative.

### 3.5 Validation performed by `SetupCodeGenConfig` (`compiler/cpp/config.cpp:134-208`)

The complete list of rejections, in order:

1. `_resetFanOutCycles > _resetCycles` → `"--reset-fan-out-cycles must be no greater than --reset-cycles"` (`:162-165`)
2. `_logicRegisterRatio < 1` → `"Logic to register ratio must be at least 1"` (`:167-170`)
3. `_fileNames` empty → `"No input files specified"` (`:172-175`)
4. `PipelineClockGatingEnabled()` i.e. `_clockGatingLevel > 0` → `"Invalid -- clock gating level must be equal to 0"` (`:178-181`, predicate at `config.cpp:70`)
5. `_stallablePipelines` → `"Invalid -- stallable pipeline feature must be disabled"` (`:183-186`)
6. `_stall > 4` → `"Invalid --stall value not in [0,4]"` (`:194-195`)
7. `_maxSelectInputs < 2` → `"--max-mux-sources must be at least 2"` (`:197-198`)
8. `!IsPow2(_maxSelectInputs)` → `"--max-mux-sources must be a power of 2"` (`:200-201`)
9. `!IsPow2(_maxThreadsDefault)` → `"--max-threads-default must be a power of 2"` (`:203-204`)
10. `!IsPow2(_maxThreadsLimit)` → `"--max-threads-limit must be a power of 2"` (`:206-207`)

Non-validating mutations: `_isInitialized = true` (`:138`); the three forced
`false`s (`:141-143`); `_moduleName = "KanagawaCore"` (`:148`);
`_bitsPerAlm = 64` (`:150`); `_frequency[0] = options._frequency` (`:146`);
`_placementConfig._numIterations = 1` when `_optimize < 2` (`:189-192`).

All of these throw `std::runtime_error` out of `InitCompiler`, which catches
`const std::exception&`, prints `e.what()` and returns `false`
(`compiler/cpp/parse_tree.cpp:6283-6297`).

---

## 4. The 85 `extern "C"` functions of `parse_tree.h`

Legend for the **Callers** column:

* **HS** — imported and called by the Haskell front end (`compiler/hs/app/ParseTree/FFI.hs`).
* **C++** — also called from inside the backend; every such call site is listed.
* **none** — declared but called by nobody.

All returns are `ParseTreeNodePtr` unless stated. Every function is `API` i.e.
`extern "C"` in `compiler/cpp/parse_tree.h`.

### 4.1 Declaration / statement constructors

| # | decl | signature | Haskell name | Callers |
|---|---|---|---|---|
| 1 | 166 | `ParseTreeNodePtr ParseDeclare(ParseTreeNodePtr attributeList, ParseTreeNodePtr type, ParseTreeNodePtr name, ParseTreeNodePtr val, unsigned int flags, ParseNamespaceScopePtr)` | `parseDeclare` (`FFI.hs:50`) | HS (`ParseTree.hs:173`) + **C++: `ParseFunctionParam` → `parse_tree.cpp:5534`** |
| 2 | 168 | `ParseTreeNodePtr ParseFunctionParam(ParseTreeNodePtr attributes, ParseTreeNodePtr type, ParseTreeNodePtr name, ParseNamespaceScopePtr)` | `parseFunctionParam` (`FFI.hs:67`) | HS (`ParseTree.hs:110`) + **C++: `ClassNode::CreateCallbacksHelper` → `ir.cpp:12011`** |
| 3 | 170 | `ParseTreeNodePtr ParseFlagAttribute(unsigned int attr)` | `parseFlagAttribute` (`FFI.hs:61`) | HS (`ParseTree.hs:97,98,99,102,104`) + **C++: `ir.cpp:12003`** |
| 4 | 171 | `ParseTreeNodePtr ParseIntAttribute(unsigned int attr, ParseTreeNodePtr)` | `parseIntAttribute` (`FFI.hs:74`) | HS (`ParseTree.hs:96,122`) + **C++: `ir.cpp:12028`** |
| 5 | 172 | `ParseTreeNodePtr ParseAssign(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseAssign` (`FFI.hs:38`) | HS (`ParseTree.hs:69`) |
| 6 | 173 | `ParseTreeNodePtr ParseIf(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseIf` (`FFI.hs:72`) | HS (`ParseTree.hs:120`) |
| 7 | 174 | `ParseTreeNodePtr ParseDoWhile(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseDoWhile` (`FFI.hs:53`) | HS (`ParseTree.hs:86`) |
| 8 | 175 | `ParseTreeNodePtr ParseUnrolledFor(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseUnrolledFor` (`FFI.hs:107`) | HS (`ParseTree.hs:151,171`) |
| 9 | 176 | `ParseTreeNodePtr ParseRangeFor(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseRangeFor` (`FFI.hs:86`) | HS (`ParseTree.hs:140`) |
| 10 | 177 | `ParseTreeNodePtr ParseAnnotatedStatement(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseAnnotatedStatement` (`FFI.hs:35`) | HS (`ParseTree.hs:65`) |
| 11 | 178 | `ParseTreeNodePtr ParseMux(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseMux` (`FFI.hs:80`) | HS (`ParseTree.hs:131`) |
| 12 | 179 | `ParseTreeNodePtr ParseBinaryOp(ParseTreeBinaryOpType, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseBinaryOp` (`FFI.hs:42`) | HS (`ParseTree.hs:73`) |
| 13 | 180 | `ParseTreeNodePtr ParseUnaryOp(ParseTreeUnaryOpType, ParseTreeNodePtr)` | `parseUnaryOp` (`FFI.hs:105`) | HS (`ParseTree.hs:169`) |
| 14 | 181 | `ParseTreeNodePtr ParseSizeOf(ParseTreeSizeofType, ParseTreeNodePtr)` | `parseSizeOf` (`FFI.hs:92`) | HS (`ParseTree.hs:166,167`) |
| 15 | 182 | `ParseTreeNodePtr ParseCast(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseCast` (`FFI.hs:45`) | HS (`ParseTree.hs:79`) |
| 16 | 183 | `ParseTreeNodePtr ParseFunctionSpecifier(ParseTreeNodePtr object, ParseTreeNodePtr name)` | `parseFunctionSpecifier` (`FFI.hs:68`) | HS (`ParseTree.hs:116`) + **C++: `enumerate_function_instances.cpp:1328`** |
| 17 | 184 | `ParseTreeNodePtr ParseFunctionCall(ParseTreeNodePtr fun, ParseTreeNodePtr arguments, ParseTreeNodePtr modifiers)` | `parseFunctionCall` (`FFI.hs:65`) | HS (`ParseTree.hs:113`) + **C++: `enumerate_function_instances.cpp:1341`** |
| 18 | 185 | `ParseTreeNodePtr ParseBaseList(ParseTreeNodePtr listBase)` | `parseBaseList` (`FFI.hs:41`) | HS (`ParseTree.hs:147,210`) + **C++ ×14, see §5** |
| 19 | 186 | `ParseTreeNodePtr ParseAppendList(ParseTreeNodePtr list, ParseTreeNodePtr newNode)` | `parseAppendList` (`FFI.hs:36`) | HS (`ParseTree.hs:146,211`) + **C++ ×8, see §5** |
| 20 | 187 | `ParseTreeNodePtr ParseArrayType(ParseTreeNodePtr attributes, ParseTreeNodePtr baseType, ParseTreeNodePtr sizeList)` | `parseArrayType` (`FFI.hs:37`) | HS (`ParseTree.hs:66`) |
| 21 | 188 | `ParseTreeNodePtr ParseFunctionModifier(ParseTreeFunctionModifier)` | `parseFunctionModifier` (`FFI.hs:66`) | HS (`ParseTree.hs:95,100,101,103,105,106,107,115`) + **C++ ×3: `parse_tree.cpp:6144`, `ir.cpp:12021`, `ir.cpp:12032`** |
| 22 | 189 | `ParseTreeNodePtr ParseFunctionTypeParam(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseFunctionTypeParam` (`FFI.hs:70`) | HS (`ParseTree.hs:118`) |
| 23 | 190 | `ParseTreeNodePtr ParseFunctionType(ParseTreeNodePtr modifierList, ParseTreeNodePtr params, ParseTreeNodePtr returnTypeNode)` | `parseFunctionType` (`FFI.hs:69`) | HS (`ParseTree.hs:117`) |
| 24 | 191 | `ParseTreeNodePtr ParseFunction(ParseTreeNodePtr modifierList, ParseTreeNodePtr modifier, ParseTreeNodePtr returnTypeNode, ParseTreeNodePtr name, ParseTreeNodePtr params, ParseTreeNodePtr statements, const char* unmangledName)` | `parseFunction` (`FFI.hs:64`) | HS (`ParseTree.hs:111,112`) |
| 25 | 193 | `ParseTreeNodePtr ParseReturn()` | `parseReturn` (`FFI.hs:89`) | HS (`ParseTree.hs:144`) + **C++: inside `ParseFunction` → `parse_tree.cpp:5913`** |
| 26 | 194 | `ParseTreeNodePtr ParseReturnExpression(ParseTreeNodePtr)` | `parseReturnExpr` (`FFI.hs:90`) | HS (`ParseTree.hs:143`) + **C++: `enumerate_function_instances.cpp:1352`** |
| 27 | 195 | `ParseTreeNodePtr ParseNestedScope(ParseTreeNodePtr body)` | `parseNestedScope` (`FFI.hs:84`) | HS (`ParseTree.hs:133`) + **C++ ×8, all in `parse_tree.cpp`, see §5** |
| 28 | 196 | `ParseTreeNodePtr ParseStruct(ParseTreeNodePtr name, ParseTreeNodePtr members, ParseNamespaceScopePtr, const char* unmangledName)` | `parseStruct` (`FFI.hs:97`) | HS (`ParseTree.hs:155`) |
| 29 | 197 | `ParseTreeNodePtr ParseUnion(ParseTreeNodePtr, ParseTreeNodePtr, ParseNamespaceScopePtr, const char*)` | `parseUnion` (`FFI.hs:106`) | HS (`ParseTree.hs:170`) |
| 30 | 198 | `ParseTreeNodePtr ParseNamedType(ParseTreeNodePtr, ParseNamespaceScopePtr)` | `parseNamedType` (`FFI.hs:81`) | HS (`ParseTree.hs:163`) |
| 31 | 199 | `ParseTreeNodePtr ParseNamedVariable(ParseTreeNodePtr)` | `parseNamedVariable` (`FFI.hs:82`) | HS (`ParseTree.hs:132`) + **C++ ×2: `parse_tree.cpp:5562`, `parse_tree.cpp:6260`** |
| 32 | 200 | `ParseTreeNodePtr ParseKnownConstNamedVariable(ParseTreeNodePtr)` | — **not imported** | **C++ only: `parse_tree.cpp:5562`** |
| 33 | 201 | `ParseTreeNodePtr ParseAccessArray(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseAccessArray` (`FFI.hs:32`) | HS (`ParseTree.hs:67`) |
| 34 | 202 | `ParseTreeNodePtr ParseAccessArrayLValue(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseAccessArrayLValue` (`FFI.hs:33`) | HS (`ParseTree.hs:68`) |
| 35 | 203 | `ParseTreeNodePtr ParseAccessMember(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseAccessMember` (`FFI.hs:34`) | HS (`ParseTree.hs:127`) |
| 36 | 204 | `ParseTreeNodePtr ParseMemberModifier(ParseTreeMemberProtectionModifier)` | `parseMemberModifier` (`FFI.hs:79`) | HS (`ParseTree.hs:137,138`) + **C++ ×4: `ReorderMembers` → `parse_tree.cpp:4788,4791,4824,4836`** |
| 37 | 205 | `ParseTreeNodePtr ParseClass(ParseTreeNodePtr name, ParseTreeNodePtr members, ParseTreeNodePtr templateInstance, ParseNamespaceScopePtr, const char* unmangledName)` | `parseClass` (`FFI.hs:46`) | HS (`ParseTree.hs:80`) |
| 38 | 207 | `ParseTreeNodePtr ParseDesignator(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseDesignator` (`FFI.hs:51`) | HS (`ParseTree.hs:85`) |
| 39 | 208 | `ParseTreeNodePtr ParseInitializerList(ParseTreeNodePtr)` | `parseInitializerList` (`FFI.hs:73`) | HS (`ParseTree.hs:121`) |
| 40 | 209 | `ParseTreeNodePtr ParseSwitch(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseSwitch` (`FFI.hs:98`) | HS (`ParseTree.hs:156`) |
| 41 | 210 | `ParseTreeNodePtr ParseSwitchBlock(ParseTreeNodePtr caseValue, ParseTreeNodePtr statementList)` | `parseSwitchBlock` (`FFI.hs:99`) | HS (`ParseTree.hs:157,158`) |
| 42 | 211 | `ParseTreeNodePtr ParseExternalFunction(ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr, ParseTreeNodePtr)` | `parseExternalFunction` (`FFI.hs:59`) | HS (`ParseTree.hs:114`) |
| 43 | 212 | `ParseTreeNodePtr ParseConst(ParseTreeNodePtr)` | `parseConst` (`FFI.hs:48`) | HS (`ParseTree.hs:82`) |
| 44 | 213 | `ParseTreeNodePtr ParseStatic(ParseTreeNodePtr)` | `parseStatic` (`FFI.hs:93`) | HS (`ParseTree.hs:148`) |
| 45 | 214 | `ParseTreeNodePtr ParseFanOut(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseFanOut` (`FFI.hs:60`) | HS (`ParseTree.hs:94`) |
| 46 | 215 | `ParseTreeNodePtr ParseBarrier()` | `parseBarrier` (`FFI.hs:40`) | HS (`ParseTree.hs:72`) |
| 47 | 216 | `ParseTreeNodePtr ParseStages(ParseTreeNodePtr)` | — **not imported** | **none — no definition exists** |
| 48 | 217 | `ParseTreeNodePtr ParseConcat(ParseTreeNodePtr)` | `parseConcat` (`FFI.hs:47`) | HS (`ParseTree.hs:81`) |
| 49 | 218 | `ParseTreeNodePtr ParseReorder(ParseTreeNodePtr)` | `parseReorder` (`FFI.hs:88`) | HS (`ParseTree.hs:142`) |
| 50 | 219 | `ParseTreeNodePtr ParseAttribute(unsigned int attr, ParseTreeNodePtr)` | `parseAttribute` (`FFI.hs:39`) | HS (`ParseTree.hs:70`) |
| 51 | 220 | `ParseTreeNodePtr ParseTypedef(ParseTreeNodePtr typeNode, ParseTreeNodePtr aliasNode, ParseNamespaceScopePtr, const char*)` | `parseTypedef` (`FFI.hs:103`) | HS (`ParseTree.hs:64`) |
| 52 | 221 | `ParseTreeNodePtr ParseStaticAssert(ParseTreeNodePtr)` | `parseStaticAssert` (`FFI.hs:94`) | HS (`ParseTree.hs:150`) |
| 53 | 222 | `ParseTreeNodePtr ParseExportType(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseExportType` (`FFI.hs:57`) | HS (`ParseTree.hs:92`) |
| 54 | 223 | `ParseTreeNodePtr ParseExtern(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseExtern` (`FFI.hs:58`) | HS (`ParseTree.hs:93`) |
| 55 | 224 | `ParseTreeNodePtr ParseStringType()` | `parseStringType` (`FFI.hs:96`) | HS (`ParseTree.hs:153`) |
| 56 | 225 | `ParseTreeNodePtr ParseStringLiteral(const char*)` | `parseStringLiteral` (`FFI.hs:95`) | HS (`ParseTree.hs:213`) |
| 57 | 226 | `ParseTreeNodePtr ParseInterpolatedString(ParseTreeNodePtr)` | `parseInterpolatedString` (`FFI.hs:76`) | HS (`ParseTree.hs:125`) |
| 58 | 227 | `ParseTreeNodePtr ParseInterpolatedStringSegment(ParseTreeNodePtr, ParseTreeNodePtr)` | `parseInterpolatedStringSegment` (`FFI.hs:77`) | HS (`ParseTree.hs:217`) |
| 59 | 228 | `ParseTreeNodePtr ParseInterpolationExpression(ParseTreeNodePtr exp, ParseTreeNodePtr alignment, ParseTreeFormatSpecifier format, unsigned int precision)` | `parseInterpolationExpression` (`FFI.hs:75`) | HS (`ParseTree.hs:220-223`) |
| 60 | 230 | `ParseTreeNodePtr ParseVoidType()` | `parseVoid` (`FFI.hs:108`) | HS (`ParseTree.hs:174`) |
| 61 | 231 | `ParseTreeNodePtr ParseBoolType()` | `parseBool` (`FFI.hs:44`) | HS (`ParseTree.hs:75`) |
| 62 | 232 | `ParseTreeNodePtr ParseBoolLiteral(BOOL)` | `parseBoolLiteral` (`FFI.hs:43`) | HS (`ParseTree.hs:76`) |
| 63 | 233 | `ParseTreeNodePtr ParseFloatType()` | `parseFloatType` (`FFI.hs:63`) | HS (`ParseTree.hs:108`) |
| 64 | 234 | `ParseTreeNodePtr ParseFloatLiteral(float)` | `parseFloatLiteral` (`FFI.hs:62`) | HS (`ParseTree.hs:109`) |
| 65 | 235 | `ParseTreeNodePtr ParseIntType(int width)` | `parseIntType` (`FFI.hs:78`) | HS (`ParseTree.hs:123,233`) |
| 66 | 236 | `ParseTreeNodePtr ParseDecimalLiteral(const char*)` | `parseDecimalLiteral` (`FFI.hs:49`) | HS (`ParseTree.hs:215`) + **C++: `ir.cpp:12028`** |
| 67 | 237 | `ParseTreeNodePtr ParseUintType(int width)` | `parseUintType` (`FFI.hs:104`) | HS (`ParseTree.hs:172,234`) |
| 68 | 238 | `ParseTreeNodePtr ParseIdentifier(const char*)` | `parseIdentifier` (`FFI.hs:71`) | HS (`ParseTree.hs:119,130`) + **C++ ×6, see §5** |
| 69 | 239 | `ParseTreeNodePtr ParseNamespace(ParseTreeNodePtr target, ParseTreeNodePtr body)` | `parseNamespace` (`FFI.hs:83`) | HS (`ParseTree.hs:128`) + **C++: `compiler.cpp:1887`** |
| 70 | 240 | `ParseTreeNodePtr ParseScopedIdentifier(ParseTreeNodePtr)` | `parseScopedIdentifier` (`FFI.hs:91`) | HS (`ParseTree.hs:139`) + **C++ ×3: `parse_tree.cpp:5557`, `parse_tree.cpp:6255`, `enumerate_function_instances.cpp:1328`** |
| 71 | 241 | `ParseTreeNodePtr ParseEnum(ParseTreeNodePtr name, ParseTreeNodePtr baseType, ParseTreeNodePtr constants, ParseNamespaceScopePtr)` | `parseEnum` (`FFI.hs:54`) | HS (`ParseTree.hs:87`) |
| 72 | 242 | `ParseTreeNodePtr ParseEnumConstant(ParseTreeNodePtr name, ParseTreeNodePtr value, ParseNamespaceScopePtr)` | `parseEnumConstant` (`FFI.hs:55`) | HS (`ParseTree.hs:89`) |
| 73 | 243 | `ParseTreeNodePtr ParseEnumValue(ParseTreeNodePtr identifier, ParseTreeNodePtr value)` | `parseEnumValue` (`FFI.hs:56`) | HS (`ParseTree.hs:90`) |
| 74 | 244 | `ParseTreeNodePtr ParseReference(ParseTreeNodePtr)` | `parseReference` (`FFI.hs:87`) | HS (`ParseTree.hs:141`) |
| 75 | 245 | `ParseTreeNodePtr ParseThis(ParseNamespaceScopePtr)` | `parseThis` (`FFI.hs:102`) | HS (`ParseTree.hs:162`) |
| 76 | 246 | `ParseTreeNodePtr ParseQualifiedName(ParseQualifiedNamePtr name)` | `parseQualifiedName` (`FFI.hs:85`) | HS (`ParseTree.hs:244`) |
| 77 | 247 | `ParseTreeNodePtr ParseTemplateArg(const char* name, ParseTreeNodePtr arg)` | `parseTemplateArg` (`FFI.hs:100`) | HS (`ParseTree.hs:247`) |
| 78 | 248 | `ParseTreeNodePtr ParseTemplateInstance(ParseTreeNodePtr templateName, ParseTreeNodePtr templateArguments)` | `parseTemplateInstance` (`FFI.hs:101`) | HS (`ParseTree.hs:241`) |
| 79 | 249 | `ParseTreeNodePtr ParseDefaultInitialization(ParseTreeNodePtr init)` | `parseDefaultInitialization` (`FFI.hs:52`) | HS (`ParseTree.hs:84`) |

### 4.2 Stateful / lifecycle entry points

| # | decl | signature | Haskell name | Callers |
|---|---|---|---|---|
| 80 | 251 | `void SetNodeType(ParseTreeNodePtr node, ParseTreeNodePtr typeNode)` | `setNodeType` (`FFI.hs:30`) | HS (`ParseTree.hs:53,54`) |
| 81 | 252 | `void SetLocation(Location)` — **by value** | — not imported | **C++ only: `parse_tree.cpp:6276` (from `SetLocation2`)** |
| 82 | 253 | `void SetLocation2(const Location*)` | `setLocation` (`FFI.hs:29`) | HS (`ParseTree.hs:59`) + **C++: `ParseFunction` → `parse_tree.cpp:5910`** |
| 83 | 254 | `void UnknownLocation()` | `unknownLocation` (`FFI.hs:31`) | HS (`ParseTree.hs:57`) |
| 84 | 255 | `BOOL InitCompiler(const Options*)` | `initCompiler` (`FFI.hs:27`) | HS (`ParseTree.hs:38`) + **C++: `internal_tests.cpp:2537`** |
| 85 | 256 | `BOOL Codegen(const char* target, const char* output, const char* res, const char* dmgl, const char* dmglDetailed, ParseTreeNodePtr root)` | `codegen` (`FFI.hs:28`) | HS (`ParseTree.hs:40`) — defined in `kanagawa.cpp:8`, not `parse_tree.cpp` |

Plus, outside `parse_tree.h`:

| decl | signature | Callers |
|---|---|---|
| `compiler/cpp/internal_tests.h:12` | `int InternalTests()` | none in-tree (test harness entry point) |

### 4.3 Haskell-declared parameter types vs. C parameter types (width mismatches)

`compiler/hs/app/ParseTree/FFI.hs` declares every small-integer parameter as
Haskell `Int` — 64 bits — where C declares 32 bits. The complete list:

| function | C parameter | Haskell type | FFI.hs line |
|---|---|---|---|
| `ParseAttribute` | `unsigned int attr` | `Int` | 35 |
| `ParseBinaryOp` | `ParseTreeBinaryOpType` | `Int` | 39 |
| `ParseBoolLiteral` | `BOOL` (`unsigned`) | `Int` | 40 |
| `ParseDeclare` | `unsigned int flags` | `Int` | 47 |
| `ParseFlagAttribute` | `unsigned int attr` | `Int` | 55 |
| `ParseFunctionModifier` | `ParseTreeFunctionModifier` | `Int` | 65 |
| `ParseIntAttribute` | `unsigned int attr` | `Int` | 71 |
| `ParseInterpolationExpression` | `ParseTreeFormatSpecifier`, `unsigned int precision` | `Int`, `Int` | 72 |
| `ParseIntType` | `int` | `Int` | 75 |
| `ParseMemberModifier` | `ParseTreeMemberProtectionModifier` | `Int` | 75 |
| `ParseSizeOf` | `ParseTreeSizeofType` | `Int` | 94 |
| `ParseUintType` | `int` | `Int` | 103 |
| `ParseUnaryOp` | `ParseTreeUnaryOpType` | `Int` | 104 |

This works on x86-64 SysV and AArch64 AAPCS64 because the callee reads only the
low 32 bits of the argument register. **`kg-backend-sys` must declare these as
`c_uint`/`c_int`, matching C, not `isize`.** Similarly `InitCompiler`/`Codegen`
return `BOOL` (`unsigned`, 4 bytes) and are imported as `IO Bool`; GHC's `Bool`
FFI marshalling reads `HsBool` (`int`) and tests `/= 0`, so any non-zero value
means success.

`ParseFloatLiteral(float)` is imported as `Float -> IO NodePtr`
(`FFI.hs:62`) — a genuine 32-bit float in a floating-point register. Do not
promote to `f64`.

---

## 5. The internal-caller graph — what an interposer can and cannot see

This is the single most important fact for the interposer (`crates/kg-ptrace`): **the backend calls
its own `extern "C"` entry points**, so a trace of the shared-library boundary
sees a mixture of front-end-originated calls and backend-internal recursion,
and the two are not distinguishable from the call itself.

The complete set of internal callers (obtained by grepping every `.cpp`/`.h` in
`compiler/cpp` for `\b<Name>(` and subtracting the definitions):

### 5.1 Calls made from inside `parse_tree.cpp` (i.e. during the front-end walk)

| callee | call site | containing ABI function |
|---|---|---|
| `ParseDeclare` | `parse_tree.cpp:5534` | `ParseFunctionParam` |
| `ParseScopedIdentifier` | `parse_tree.cpp:5557` | `ParseDeclare` (only when `val != nullptr`) |
| `ParseBaseList` | `parse_tree.cpp:5557` | `ParseDeclare` (only when `val != nullptr`) |
| `ParseKnownConstNamedVariable` | `parse_tree.cpp:5562` | `ParseDeclare` (only when `val != nullptr` **and** `DECLARE_FLAG_CONST`) |
| `ParseNamedVariable` | `parse_tree.cpp:5562` | `ParseDeclare` (only when `val != nullptr` and **not** const) |
| `ParseNestedScope` | `parse_tree.cpp:5597` | `ParseDoWhile` |
| `ParseNestedScope` | `parse_tree.cpp:5603` | `ParseRangeFor` |
| `ParseNestedScope` | `parse_tree.cpp:5608` | `ParseUnrolledFor` |
| `ParseAppendList` | `parse_tree.cpp:5679` | `ParseBaseList` (only when `listBase != nullptr`) |
| `SetLocation2` | `parse_tree.cpp:5910` | `ParseFunction` (only when synthesising an implicit `return`) |
| `ParseReturn` | `parse_tree.cpp:5913` | `ParseFunction` (same condition) |
| `ParseAppendList` | `parse_tree.cpp:5913` | `ParseFunction` (same condition) |
| `ParseNestedScope` | `parse_tree.cpp:5917` | `ParseFunction` |
| `ParseNestedScope` | `parse_tree.cpp:5952` | `ParseClass` |
| `ParseMemberModifier` | `parse_tree.cpp:4788` | `ReorderMembers` ← `ClassNode::ClassNode` (`parse_tree.cpp:4870`) ← `ParseClass` |
| `ParseMemberModifier` | `parse_tree.cpp:4791` | idem |
| `ParseMemberModifier` | `parse_tree.cpp:4824` | idem |
| `ParseMemberModifier` | `parse_tree.cpp:4836` | idem |
| `ParseNestedScope` | `parse_tree.cpp:6118` | `ParseSwitchBlock` |
| `ParseNestedScope` | `parse_tree.cpp:6132` | `ParseExternalFunction` (`[[latency(n)]]` path) |
| `ParseBaseList` | `parse_tree.cpp:6141` | `ParseExternalFunction` |
| `ParseBaseList` | `parse_tree.cpp:6144` | `ParseExternalFunction` |
| `ParseFunctionModifier` | `parse_tree.cpp:6144` | `ParseExternalFunction` |
| `ParseAppendList` | `parse_tree.cpp:6147` | `ParseExternalFunction` |
| `ParseNestedScope` | `parse_tree.cpp:6149` | `ParseExternalFunction` |
| `SetLocation` | `parse_tree.cpp:6276` | `SetLocation2` |

**Concretely, one front-end `ParseDeclare` with an initialiser emits, at the
library boundary, this sequence:**

```
ParseBaseList(name)                 -- parse_tree.cpp:5557 (inner)
ParseScopedIdentifier(<baselist>)   -- parse_tree.cpp:5557 (outer)
ParseKnownConstNamedVariable(...)   -- parse_tree.cpp:5562  (if const)
  or ParseNamedVariable(...)        -- parse_tree.cpp:5562  (otherwise)
```
before returning. And one front-end `ParseFunctionParam` emits a nested
`ParseDeclare` (`parse_tree.cpp:5534`) — which, because `ParseFunctionParam`
always passes `val = nullptr`, does **not** further recurse.

### 5.2 Calls made from outside `parse_tree.cpp` (i.e. during `Codegen`, after parsing)

These happen inside `Codegen`, long after the front end has stopped calling in.
They synthesise parse-tree nodes for export-class plumbing.

| callee | call site | context |
|---|---|---|
| `ParseBaseList` | `ir.cpp:11990` | `ClassNode::CreateCallbacksHelper` |
| `ParseBaseList` | `ir.cpp:12003` | idem (`[[last]]` param attributes) |
| `ParseFlagAttribute` | `ir.cpp:12003` | idem — argument is `DECLARE_FLAG_END_TRANSACTION` |
| `ParseBaseList` | `ir.cpp:12007` | idem |
| `ParseFunctionParam` | `ir.cpp:12011` | idem |
| `ParseIdentifier` | `ir.cpp:12012` | idem |
| `ParseAppendList` | `ir.cpp:12014` | idem |
| `ParseBaseList` | `ir.cpp:12021` | idem |
| `ParseFunctionModifier` | `ir.cpp:12021` | idem |
| `ParseAppendList` | `ir.cpp:12027` | idem |
| `ParseIntAttribute` | `ir.cpp:12028` | idem — attribute is `ParseTreeLatencyAttr` |
| `ParseDecimalLiteral` | `ir.cpp:12028` | idem — latency rendered via `std::to_string` |
| `ParseBaseList` | `ir.cpp:12032` | idem |
| `ParseFunctionModifier` | `ir.cpp:12032` | idem |
| `ParseIdentifier` | `ir.cpp:12037` | idem |
| `ParseBaseList` | `ir.cpp:12038` | idem |
| `ParseNamedVariableFromString` | `enumerate_function_instances.cpp:1309` | export-callback wiring (C++-only helper, see §5.3) |
| `ParseBaseList` | `enumerate_function_instances.cpp:1314` | idem |
| `ParseAppendList` | `enumerate_function_instances.cpp:1320` | idem |
| `ParseIdentifier` | `enumerate_function_instances.cpp:1320` | idem |
| `ParseAppendList` | `enumerate_function_instances.cpp:1325` | idem |
| `ParseIdentifier` | `enumerate_function_instances.cpp:1325` | idem |
| `ParseFunctionSpecifier` | `enumerate_function_instances.cpp:1328` | idem |
| `ParseScopedIdentifier` | `enumerate_function_instances.cpp:1328` | idem |
| `ParseBaseList` | `enumerate_function_instances.cpp:1330` | idem |
| `ParseNamedVariableFromString` | `enumerate_function_instances.cpp:1334` | idem |
| `ParseAppendList` | `enumerate_function_instances.cpp:1338` | idem |
| `ParseFunctionCall` | `enumerate_function_instances.cpp:1341` | idem |
| `ParseBaseList` | `enumerate_function_instances.cpp:1341` | idem |
| `ParseReturnExpression` | `enumerate_function_instances.cpp:1352` | idem |
| `ParseBaseList` | `parse_tree.cpp:5157` | `ClassNode::PruneForExportOrExternalClassUse` (runs inside `Compiler::Reset`) |
| `ParseNamespace` | `compiler.cpp:1887` | `Compiler::ReorderDeclarations` |
| `ParseIdentifier` | `compiler.cpp:1887` | idem |
| `InitCompiler` | `internal_tests.cpp:2537` | `InternalTests()` only |

### 5.3 C++-only helpers that call ABI functions

Two non-`extern "C"` helpers are declared in `parse_tree.h` for internal use
(`compiler/cpp/parse_tree.h:267`, `:269`):

```cpp
ParseTreeNodePtr ParseScopedIdentifierFromString(const std::string& str);
ParseTreeNodePtr ParseNamedVariableFromString(const std::string& str);
```

* `ParseScopedIdentifierFromString` = `ParseScopedIdentifier(ParseBaseList(ParseIdentifier(str.c_str())))`
  (`compiler/cpp/parse_tree.cpp:6253-6256`).
* `ParseNamedVariableFromString` = `ParseNamedVariable(ParseScopedIdentifierFromString(str))`
  (`compiler/cpp/parse_tree.cpp:6258-6261`).

They are **not** exported with C linkage (they take `std::string`), so an
interposer cannot hook them — but it *will* observe the three/four inner ABI
calls each of them makes.

### 5.4 Consequences for the interposer (`crates/kg-ptrace`)

1. A trace contains strictly more calls than the front end made. Any
   differential test between the Haskell front end and `kg-ptbridge` must
   compare traces produced under identical conditions, or must filter out
   backend-internal recursion.
2. Recursion is not marked. Re-entrancy has to be inferred by the interposer
   itself (a thread-local depth counter incremented on entry to the forwarder,
   decremented on exit; depth > 1 ⇒ backend-internal). This is the recommended
   design; see §14.2.
3. The clean cut point is `Codegen`. Everything before the first `Codegen` call
   is front-end-driven (plus `parse_tree.cpp`-internal recursion); everything
   during `Codegen` is backend-internal. `kg-ptbridge` only ever needs to
   reproduce the pre-`Codegen` window.

---

## 6. The stateful protocol

The boundary is **not** purely functional. Three pieces of hidden state exist.

### 6.1 `g_currentTokenLocation`

```cpp
static Location g_currentTokenLocation = {};
```
`compiler/cpp/parse_tree.cpp:9`.

| function | effect |
|---|---|
| `SetLocation(Location loc)` | `g_currentTokenLocation = loc; g_currentTokenLocation._valid = true;` (`compiler/cpp/parse_tree.cpp:6270-6274`) — note it **forces `_valid = true`** regardless of what the caller passed |
| `SetLocation2(const Location* loc)` | `SetLocation(*loc)` (`compiler/cpp/parse_tree.cpp:6276`) |
| `UnknownLocation()` | `g_currentTokenLocation._valid = false;` (`compiler/cpp/parse_tree.cpp:6281`) — **leaves the line/column/file fields intact**; the documented intent is *"the location of the last node is used ... with the valid flag set to false"* (`compiler/cpp/parse_tree.cpp:6278-6280`) |

**Consumers** (every read of `g_currentTokenLocation` in the tree):

| site | what it does |
|---|---|
| `compiler/cpp/parse_tree.cpp:1193` | `_ParseTreeNode::_ParseTreeNode` copies it into the new node's `const Location _location`. **This is the main consumer: every node created by any `Parse*` call snapshots the current location.** |
| `compiler/cpp/parse_tree.cpp:5776` | `ParseArrayType` → `Compiler::GetArrayType(..., g_currentTokenLocation)` |
| `compiler/cpp/parse_tree.cpp:6231` | `ParseIntType` → `Compiler::GetLeafType(BaseType::Int, width, g_currentTokenLocation)` |
| `compiler/cpp/parse_tree.cpp:6238` | `ParseUintType` → `Compiler::GetLeafType(BaseType::Uint, width, g_currentTokenLocation)` |
| `compiler/cpp/parse_tree.cpp:258` | `ErrorStream(g_currentTokenLocation, CompileError::ExportTypeRestriction)` |
| `compiler/cpp/parse_tree.cpp:1446`, `:1582`, `:1589`, `:1595` | `ErrorStream(g_currentTokenLocation, CompileError::InvalidFunctionModifier)` |
| `compiler/cpp/parse_tree.cpp:1624` | `GetFunctionThreadRate(modifierList, g_currentTokenLocation)` |

There is also an RAII override used *inside* the backend:

```cpp
class SetParseLocation { SetParseLocation(const Location&); ~SetParseLocation(); Location _saved; };
```
`compiler/cpp/parse_tree.h:4312-4321`, implemented `compiler/cpp/parse_tree.cpp:8033-8038`
(save on construct, restore on destruct). Used at `compiler/cpp/ir.cpp:3500` and
`compiler/cpp/ir.cpp:11983`. It writes the location **verbatim**, without forcing
`_valid = true` — unlike `SetLocation`.

**Ordering contract.** `SetLocation2` / `UnknownLocation` must be called
*before* the `Parse*` call whose nodes should carry that location. There is no
"end" call. The state persists until the next `SetLocation2`/`UnknownLocation`.

### 6.2 `_frontEndType` (set by `SetNodeType`)

```cpp
void SetNodeType(ParseTreeNodePtr node, ParseTreeNodePtr typeNode)
{
    const Type* const type = safe_cast<const TypeNode*>(typeNode)->GetType();
    node->SetFrontEndType(type);
}
```
`compiler/cpp/parse_tree.cpp:6263-6268`.

* `typeNode` **must** be a `TypeNode` — `safe_cast` (not `dynamic_cast`) means a
  wrong node kind is a hard failure, not a `nullptr`.
* `_ParseTreeNode::SetFrontEndType` (`compiler/cpp/parse_tree.cpp:1269-1279`)
  asserts `type != nullptr` and that the node's middle-end `_type` is either
  unset, a `DeferredType`, or already equal.
* Consumers:
  * `_ParseTreeNode::SetType` (`compiler/cpp/parse_tree.cpp:1244-1267`) — when
    the middle end computes a *different* type, the **front-end type wins**,
    except for enums: *"This is not done for enums, because the front-end passes
    an integer type rather than the enum type"* (`:1250-1252`). It then asserts
    both types are `LeafType`.
  * `IntegerNode::SetFrontEndType` (`compiler/cpp/parse_tree.cpp:1290-1295`)
    additionally computes `_typedValue = StringToMpInt(_valueString, type)` —
    see §7.
  * `IntegerNode::TypeCheck` (`compiler/cpp/parse_tree.cpp:1308-1313`) —
    *"Simply accept the type provided by the front-end"*, `assert(_frontEndType)`.
    **An `IntegerNode` that never received `SetNodeType` will assert during type
    checking if it is type-checked as an expression.**
  * `compiler/cpp/parse_tree.cpp:3181-3186` — width-overflow recovery: when a
    computed integer type exceeds `c_maxRegisterBitWidth`, fall back to
    `_frontEndType` rather than erroring `TypeToWide`.

**When the front end calls it** (`compiler/hs/app/ParseTree.hs:50-55`):

```haskell
parseWithLoc e@(Note n _) = do
    setSrcLocation $ untype n
    node <- parse e
    when (tIsInt $ typeOf e) $ parseIntegerType (typeOf e) >>= setNodeType node
    when (tIsEnum $ typeOf e) $ parseIntegerType (tEnumBase $ typeOf e) >>= setNodeType node
    return node
```

i.e. **after** the node is created, and only for expressions whose inferred type
is an integer (`tIsInt`, `compiler/hs/lib/Language/Kanagawa/Type.hs:668`) or an
enum (`tIsEnum`, `compiler/hs/lib/Language/Kanagawa/Type.hs:650-651`) — for enums the *base* integer type is sent.
`parseIntegerType` (`compiler/hs/app/ParseTree.hs:232-235`) calls
`ParseIntType` for signed, `ParseUintType` for unsigned, and is `undefined`
otherwise. Because `ParseIntType`/`ParseUintType` themselves create nodes, they
snapshot the *same* `g_currentTokenLocation` that was set for the expression.

Note that a *type* expression (e.g. the AST node `Unsigned 32`) has
`typeOf = TType (TUnsigned 32)`, and `tIsUnsigned (TType …)` is `False` because
`tInner` only unwraps `TConst` and `TInstance`
(`compiler/hs/lib/Language/Kanagawa/Type.hs:598-602`, `:656-657` (`tIsUnsigned`)). So
`SetNodeType` is applied to **values**, never to type nodes.

### 6.3 Compiler-global side effects performed by `Parse*` calls

Several `Parse*` functions are not pure constructors — they mutate the
`Compiler` singleton at parse time:

| function | side effect |
|---|---|
| `ParseTypedef` | `g_compiler->RegisterTypedef(flattenedAlias, type)` (`compiler/cpp/parse_tree.cpp:6179`), which inserts into `_namedTypeMap` and `_typedefs` and reports `CompileError::DuplicateType` on collision (`compiler/cpp/compiler.cpp:1508-1520`). Returns an `UnregisterTypedefNode` (`compiler/cpp/parse_tree.cpp:6182`, class at `compiler/cpp/parse_tree.h:3970-3986`) used later for scoping. |
| `ParseStruct` / `ParseUnion` | `StructUnionNode` ctor (`compiler/cpp/parse_tree.cpp:4359`) registers the type via `Compiler::RegisterStructUnion`, again reporting `DuplicateType` |
| `ParseClass` | `ClassNode` ctor (`compiler/cpp/parse_tree.cpp:4867`) runs `ReorderMembers` (which calls `ParseMemberModifier` ×N) and registers the class type |
| `ParseEnum` | `EnumNode` ctor (`compiler/cpp/parse_tree.cpp:4329`) evaluates every constant and registers the enum type |
| `ParseIntType` / `ParseUintType` | `Compiler::GetLeafType` interns a `LeafType` |
| `ParseArrayType` | `Compiler::GetArrayType` interns an `ArrayTypeBase` |
| `ParseFunctionType` | `Compiler::GetFunctionType` interns a `FunctionType` |
| every `Parse*` | `Compiler::Create<T>` pushes a destructor onto `_permanentParseTreeCleanupList` (pre-`MarkPermanentNodes`) and `AddParseTreeNode` appends to `_parseTreeNodes` (`compiler/cpp/parse_tree.cpp:1196`) |

**Therefore the call order is semantically significant, not just structurally.**
A declaration referring to a typedef must be parsed after the `ParseTypedef`
that registered it.

### 6.4 Worked example — a local `uint32 x = 5;`

Derived from `compiler/hs/app/ParseTree.hs`. The walk is
`cataM parseWithLoc p` (`compiler/hs/app/ParseTree.hs:40`) where
`cataM alg = paraM (const alg)` and
`paraM alg t = alg t =<< traverse (paraM alg) (unfix t)`
(`compiler/hs/lib/Language/Kanagawa/Recursion.hs:79-83`) — i.e. **post-order,
children in derived-`Traversable` order, which is `ExpF` field-declaration
order** (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:282`, the
`deriving (Eq, Ord, Foldable, Functor, Traversable, Show)` at
`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:584` for `ExpF`
and `:591` for the `Note` wrapper).

The AST node is
`VariableF { declScopeName, varAttr, varType, declName, varInit, declFlags }`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:546-554`). `declScopeName`
is a `QualifiedName`, not an `a`, so it is not traversed; the traversed children
are, in order: `varAttr`, `varType`, `declName`, `varInit`.

The handler is
`Variable n a b c d x -> withQualifiedName n $ parseDeclare a b c (fromMaybe nullPtr d) (declareFlag x)`
(`compiler/hs/app/ParseTree.hs:173`).

Boundary call sequence (`Sn` = the node returned by step *n*; `L(e)` = the
source span of AST node *e*):

```
 1  SetLocation2(&L(varAttr))                  ; ParseTree.hs:51,59
 2  ParseBaseList(NULL)                    -> S2 ; Seq [] -> parseList, ParseTree.hs:145,210
 3  SetLocation2(&L(varType))
 4  ParseUintType(32)                      -> S4 ; Unsigned 32, ParseTree.hs:172
 5  SetLocation2(&L(declName))
 6  ParseIdentifier("x")                   -> S6 ; Identifier, ParseTree.hs:119
 7  SetLocation2(&L(varInit))
 8  ParseDecimalLiteral("5")               -> S8 ; IntLiteral -> parseIntLiteral, ParseTree.hs:124,215
 9  ParseUintType(W)                       -> S9 ; parseIntegerType (typeOf lit), ParseTree.hs:53,234
10  SetNodeType(S8, S9)                          ; ParseTree.hs:53
11  SetLocation2(&L(variable))
12  ParseDeclare(S2, S4, S6, S8, 0x04, scope)    ; declareFlag DeclareLocal = 0x04, Types.hsc:47
       ; --- inside the library, all with g_currentTokenLocation = L(variable) ---
       ; ParseBaseList(S6)                  -> A   parse_tree.cpp:5557
       ; ParseScopedIdentifier(A)           -> B   parse_tree.cpp:5557
       ; ParseNamedVariable(B)              -> C   parse_tree.cpp:5562  (not const)
       ; (creates AssignNode, DeclareNode, NodeList)    parse_tree.cpp:5545-5575
    -> S12  (a NodeList containing [DeclareNode, AssignNode])
```

Notes on the example:

* `scope` at step 12 is the NUL-terminated `const char**` produced by
  `withQualifiedName declScopeName`
  (`compiler/hs/app/ParseTree.hs:46`, `compiler/hs/app/Options/FFI.hsc:44-58`).
* `W` at step 9 is whatever width type inference assigned to the literal. For a
  bare `5` with no context that is `TUnsigned (clog2 6)`
  (`compiler/hs/lib/Language/Kanagawa/Type.hs:485-492`); inside a
  `VariableF` the inference pass propagates the declared type down into
  `varInit` (`compiler/hs/lib/Language/Kanagawa/Type.hs:1147-1148`), so in
  practice `W = 32` here.
* `ParseDeclare` returns a **`NodeList`, not a `DeclareNode`**, whenever
  `val != nullptr` (`compiler/cpp/parse_tree.cpp:5545-5575`). The list holds the
  `DeclareNode` first and the `AssignNode` second, *"Ensure the assign is type
  checked first"* — the comment is at `compiler/cpp/parse_tree.cpp:5569` but the append order is
  declare-then-assign (`compiler/cpp/parse_tree.cpp:5570`, `:5572`).
* When `val == nullptr` it returns a bare `DeclareNode`
  (`compiler/cpp/parse_tree.cpp:5577-5579`) and makes **no** inner calls.
* If `type` is a `ConstNode`, `ParseDeclare` unwraps it and ORs
  `DECLARE_FLAG_CONST` into `flags` *before* the branch
  (`compiler/cpp/parse_tree.cpp:5539-5543`) — so `const` declarations take the
  `ParseKnownConstNamedVariable` path.

---

## 7. Literal marshalling

### 7.1 Integers — decimal **string**, never a machine word

```c
API ParseTreeNodePtr ParseDecimalLiteral(const char*);
```
`compiler/cpp/parse_tree.h:236`, imported as
`parseDecimalLiteral :: CString -> IO NodePtr` (`compiler/hs/app/ParseTree/FFI.hs:49`).

Front end:
```haskell
parseIntLiteral x = withCAString (show x) parseDecimalLiteral
```
`compiler/hs/app/ParseTree.hs:215`. `x` is the Haskell `Integer` from
`IntLiteralF { literalType, literalValue :: Integer }`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:347-350`), so `show`
produces an **unbounded decimal string** with no separators, no radix prefix,
and no width suffix. Call sites: `compiler/hs/app/ParseTree.hs:124`
(`IntLiteral _ x`), `:96` (the constant `1` for `atomic`, §9), `:251`
(`TIntArg` template arguments).

Backend:
```cpp
IntegerNode::IntegerNode(const std::string& valueString) : _valueString(valueString)
{
    std::istringstream str(valueString);
    str >> _untypedValue;                    // boost::multiprecision::cpp_int
}
```
`compiler/cpp/parse_tree.cpp:1281-1286`; `mp_int` is
`boost::multiprecision::cpp_int` (`compiler/cpp/pch.h:92`). The raw string is
retained in `const std::string _valueString`
(`compiler/cpp/parse_tree.h:1973`).

Two values are then derived:

* **`_untypedValue`** — parsed immediately, arbitrary precision, exposed by
  `GetUntypedValue()` (`compiler/cpp/parse_tree.cpp:1298`), documented as
  *"Used before type checking, to get integer attributes"*
  (`compiler/cpp/parse_tree.cpp:1297`). Consumers:
  `ParseIntAttribute` → `MpToSizeT(dynamic_cast<const IntegerNode*>(value)->GetUntypedValue())`
  (`compiler/cpp/parse_tree.cpp:5515-5516`). `MpToSizeT` throws
  `"Value too large to convert to integer"` above `SIZE_MAX`
  (`compiler/cpp/pch.h:96-103`).
* **`_typedValue`** — computed *only* when `SetNodeType` fires:
  ```cpp
  void IntegerNode::SetFrontEndType(const Type* const type) {
      ParseTreeNode::SetFrontEndType(type);
      _typedValue = StringToMpInt(_valueString, type);
  }
  ```
  `compiler/cpp/parse_tree.cpp:1290-1295`. `StringToMpInt`
  (`compiler/cpp/parse_tree.cpp:17-35`) re-parses the string and masks:
  ```cpp
  const mp_int typeWidthMask = (mp_int(1) << type->GetBitWidth()) - 1;
  str >> result;                 // sign flag set if the string began with '-'
  result = result & typeWidthMask;   // two's-complement truncation to the width
  ```
  `GetTypedValue()` asserts `_typedValue` is present
  (`compiler/cpp/parse_tree.cpp:1300-1305`).

**Consequences for a Rust port**

1. The literal must be transported as its *source-form decimal text*, not as a
   parsed integer, because the width is not known until `SetNodeType`.
2. Negative literals: the string parser handles a leading `-` (the comment at
   `compiler/cpp/parse_tree.cpp:24-26` says so explicitly), but the front end
   never sends one — `ParseEnumConstant` asserts
   *"IntegerNode doesn't support negative numbers so they are passed as Negate
   unary expression"* (`compiler/cpp/parse_tree.cpp:6058-6060`). Negation is
   modelled as `ParseUnaryOp(ParseTreeUnaryOpTypeNegate, …)`.
3. `ParseArrayType` reads array extents with `MpToSizeT(sizeNode->GetTypedValue())`
   (`compiler/cpp/parse_tree.cpp:5773`) — i.e. array-size literals **must** have
   received `SetNodeType` before `ParseArrayType` is called. The post-order walk
   guarantees this (`compiler/hs/app/ParseTree.hs:50-55`).
4. `IntegerNode::PrettyPrint` is `std::to_string(static_cast<int64_t>(GetTypedValue()))`
   (`compiler/cpp/parse_tree.h:1964`) — lossy above 64 bits; only affects
   diagnostics.

### 7.2 Floats — a real 32-bit `float`

```c
API ParseTreeNodePtr ParseFloatLiteral(float);
API ParseTreeNodePtr ParseFloatType();
```
`compiler/cpp/parse_tree.h:234`, `:233`. Imported as
`parseFloatLiteral :: Float -> IO NodePtr` (`compiler/hs/app/ParseTree/FFI.hs:62`),
called as `FloatLiteral x -> parseFloatLiteral x`
(`compiler/hs/app/ParseTree.hs:109`) where the AST carries `FloatLiteralF Float`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:352`).

Backend: `FloatNode(const float value) : _value(value)`
(`compiler/cpp/parse_tree.h:1981`), stored in `const float _value`
(`compiler/cpp/parse_tree.h:2001`). No string round-trip, no widening.
`ParseFloatType()` always yields `GetFloatType(32)`
(`compiler/cpp/parse_tree.cpp:6225`), and `Compiler::Init` only ever creates the
32-bit float type (`compiler/cpp/compiler.cpp:43`).

**So the float path is lossy-by-design at 32 bits and is *not* symmetric with
the integer path.** A Rust reimplementation must round the parsed decimal to
`f32` at exactly the same point the Haskell parser does, or literal values will
diverge.

### 7.3 Booleans and strings

* `ParseBoolLiteral(BOOL)` (`compiler/cpp/parse_tree.h:232`) →
  `BoolNode(b)` (`compiler/cpp/parse_tree.cpp:6223`). Front end sends
  `fromEnum x` for `x :: Bool` (`compiler/hs/app/ParseTree.hs:76`), i.e. 0 or 1.
* `ParseStringLiteral(const char*)` (`compiler/cpp/parse_tree.h:225`) →
  `LiteralStringNode(str)` (`compiler/cpp/parse_tree.cpp:6201`). Front end:
  `parseString s = withCAString s parseStringLiteral`
  (`compiler/hs/app/ParseTree.hs:213`). `withCAString` is **byte-oriented** — it
  truncates each `Char` to 8 bits; non-Latin-1 source strings are already
  rejected/handled upstream. Escapes are resolved by the Haskell lexer before
  this point; the backend receives the decoded bytes.

---

## 8. Scope arrays — direction contract

Two typedefs, both `const char* const*`, both NUL-pointer-terminated
(`compiler/cpp/parse_tree.h:163-164`):

* `ParseNamespaceScopePtr` — the *enclosing declaration scope* of a declaration
  or reference. Parameter of `ParseDeclare`, `ParseFunctionParam`, `ParseStruct`,
  `ParseUnion`, `ParseNamedType`, `ParseClass`, `ParseTypedef`, `ParseEnum`,
  `ParseEnumConstant`, `ParseThis`.
* `ParseQualifiedNamePtr` — a fully qualified name whose **last** element is the
  name itself. Parameter of `ParseQualifiedName` only.

### 8.1 The array is written outer → inner (front end)

```haskell
withQualifiedName = withStringArray . map toString
```
`compiler/hs/app/ParseTree.hs:46`, and

```haskell
allocaStrings (x:xs) ptr f = withCAString x $ \s -> do
    poke ptr s
    allocaStrings xs (ptr `plusPtr` sizeOf s) f
allocaStrings [] ptr f = do
    poke ptr nullPtr
    f ptr
```
`compiler/hs/app/Options/FFI.hsc:52-58` — element 0 of the Haskell list lands at
array index 0, and a `NULL` terminator is appended.

That the Haskell `QualifiedName = [Name]`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:69`) is **outermost-first**
is established three independent ways:

1. `parseScoped` builds a nested scope by **appending**:
   `let scopeQualifier = parentQualifier ++ concatMap getQualifiedName s`
   (`compiler/hs/lib/Language/Kanagawa/Parser/SymbolTable.hs:187`).
2. `functionScope = (++ ["$"])` appends the function-local marker
   (`compiler/hs/lib/Language/Kanagawa/Desugar.hs:173-174`) and `isLocalScope`
   tests **`last`**: `go xs = '$' == head (toString $ last xs)`
   (`compiler/hs/lib/Language/Kanagawa/Desugar.hs:176-180`).
3. `getQualifiedName e = map fromName $ getQualifier e ++ [getName e]`
   (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:672-673`) — the bare name
   is appended last.

### 8.2 The backend reverses it with `push_front`

```cpp
typedef std::list<std::string> Scope;

inline Scope ToScope(ParseNamespaceScopePtr scope)
{
    Scope _scope;
    for (; scope && *scope; ++scope)
        _scope.push_front(*scope);
    return _scope;
}
```
`compiler/cpp/parse_tree.h:428-438`. A `NULL` array pointer is tolerated
(`scope &&`), yielding an empty `Scope`.

The same `push_front` reversal appears in the two other array→`Scope` readers:

```cpp
Scope ScopeFromNodeList(const ParseTreeNode* const scopeNode)   // parse_tree.cpp:93
{ ... for (node : nodeList->Children()) result.push_front(name); ... }
```
`compiler/cpp/parse_tree.cpp:93-109`, and

```cpp
ParseTreeNodePtr ParseQualifiedName(ParseQualifiedNamePtr nameArray)
{
    Scope qualifier; std::string name;
    for (size_t i = 0; nameArray[i] != nullptr; i++) qualifier.push_front(nameArray[i]);
    assert(!qualifier.empty());
    name = qualifier.front();
    qualifier.pop_front();
    return g_compiler->Create<QualifiedNameNode>(qualifier, name);
}
```
`compiler/cpp/parse_tree.cpp:5956-5972`. Note `ParseQualifiedName` **asserts the
array is non-empty** — passing `{NULL}` is a hard failure.

### 8.3 Net effect

> **The C array is ordered outermost → innermost (name last for
> `ParseQualifiedNamePtr`). The C++ `Scope` list that results is the exact
> reverse: `front()` is the innermost element.**

This is self-consistent because both the registration path and the lookup path
reverse. Registration:
`FlattenScopeAndAppendName(ToScope(namespaceScope), name)`
(`compiler/cpp/parse_tree.cpp:6176-6177` for typedefs;
`compiler/cpp/compiler.cpp:245-248` for structs/unions via
`Compiler::RegisterStructUnion`). Lookup:
`ParseNamedType` builds `scope = ScopeFromNodeList(nameNode)`, pops the front as
the name (`compiler/cpp/parse_tree.cpp:5991-5995`) and hands the remainder to
`DeferredType` → `Compiler::GetNamedType`
(`compiler/cpp/parse_tree.cpp:6006`). Both sides therefore flatten with the
same reversed convention.

`ScopedIdentifierNode` uses the identical trick:
`GetName()` = `ScopeFromNodeList(_identifierNode).front()`
(`compiler/cpp/parse_tree.cpp:7366`); `GetScope()` = the same list with the front
popped (`compiler/cpp/parse_tree.cpp:7368-7374`);
`GetFlatenedName()` = `FlattenScopeAndAppendName(GetScope(), GetName())`
(`compiler/cpp/parse_tree.cpp:7376`).

And the identifier `NodeList` that feeds it is itself built outer→inner by the
front end: `ScopedName _ Nothing a -> parseBaseList a` and
`ScopedName _ (Just a) b -> parseAppendList a b`
(`compiler/hs/app/ParseTree.hs:146-147`), where the *nested* `ScopedNameF`
carries the outer part (`compiler/hs/lib/Language/Kanagawa/Desugar.hs:182-185`,
`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:758`).

`FlattenScope` joins with `"::"` front-to-back
(`compiler/cpp/parse_tree.cpp:7722-7741`), so the flattened internal name of a
symbol declared in a two-or-more-level scope is the **reversed** qualifier
followed by the name. See the Open Questions section for the one place this
appears to matter.

`CombineScopes(lhs, rhs)` splices `rhs` onto the end of `lhs`
(`compiler/cpp/parse_tree.cpp:7773-7781`); `ScopesEqual` is elementwise
(`compiler/cpp/parse_tree.cpp:7783-7805`); `StringToScopeAndName`
(`compiler/cpp/parse_tree.cpp:7808-7838`) splits on `"::"` into vector order and
is **dead code** — it has no callers anywhere in the tree.

---

## 9. `atomic` → `[[schedule(1)]]` desugaring at the boundary

`atomic` is a source-level statement keyword; there is no `atomic` concept in
the ABI at all. The rewrite happens in two steps.

**Step 1 — the parser turns `atomic S` into an annotated statement.**

```haskell
atomic = Annotated <$ keyword "atomic" <*> atomicAttr <*> statement <?> "atomic statement"
  where
    atomicAttr = pure $ Seq [FlagAttr Atomic]
```
`compiler/hs/lib/Language/Kanagawa/Parser.hs:559-561`. `Atomic` is a
`Flag` constructor (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:152`,
`show Atomic = "atomic"` at `:168`).

The only *explicit* statement attribute the grammar accepts is `schedule`:
```haskell
stmtAttr = attributes [ intAttr Schedule ]
annotatedStmt = Annotated <$> try stmtAttr <*> statement
```
`compiler/hs/lib/Language/Kanagawa/Parser.hs:563-567`.

**Step 2 — `ParseTree.hs` lowers `FlagAttr Atomic` into an int attribute.**

```haskell
FlagAttr Atomic -> parseIntLiteral (1 :: Int) >>= parseIntAttribute ScheduleAttr
```
`compiler/hs/app/ParseTree.hs:96`. Concretely at the boundary:

```
ParseDecimalLiteral("1")                 -> T      ; ParseTree.hs:215
ParseIntAttribute(ParseTreeScheduleAttr /*5*/, T)  ; ParseTree.hs:96, Types.hsc:75
```

Note that `T` never receives `SetNodeType` — it is consumed by
`ParseIntAttribute` through `GetUntypedValue()`
(`compiler/cpp/parse_tree.cpp:5513-5517`), which does not require a type.

**Step 3 — the backend reads the schedule attribute back out.**

```cpp
ParseTreeNodePtr ParseAnnotatedStatement(ParseTreeNodePtr attr, ParseTreeNodePtr statement)
{
    size_t maxThreads = 1;
    if (const auto list = dynamic_cast<const NodeList*>(attr))
        for (const auto node : list->Children())
            if (const auto attr = dynamic_cast<const IntAttributeNode*>(node))
                if (attr->_attribute == ParseTreeScheduleAttr)
                    maxThreads = attr->_value;
    return g_compiler->Create<AtomicNode>(statement, maxThreads);
}
```
`compiler/cpp/parse_tree.cpp:5611-5632`. The `AtomicNode` constructor is
`AtomicNode(const ParseTreeNode* body, size_t updateRate)`
(`compiler/cpp/parse_tree.h:3033`).

> **Therefore `atomic { … }` is exactly `[[schedule(1)]] { … }` and both produce
> `AtomicNode(body, 1)`.** The default when no `schedule` attribute is present is
> also `1` (`compiler/cpp/parse_tree.cpp:5613`), so a bare
> `ParseAnnotatedStatement(emptyList, S)` is likewise atomic-with-rate-1.
> The TODO at `compiler/cpp/parse_tree.cpp:5629-5630` — *"generalize AtomicNode
> to a node which can annotate contained statement with various attributes"* —
> confirms this conflation is deliberate and load-bearing.

`kg-ptbridge` must therefore *not* have an `Atomic` case in its lowered AST; it
must emit the `ParseDecimalLiteral("1")` + `ParseIntAttribute(5, …)` pair.

---

## 10. Closure magic names

Lambdas are eliminated in the Haskell front end (they are one of the 18
`undefined` kinds, §11) by rewriting them into ordinary calls to library
functions with hard-coded qualified names.

### 10.1 The names

```haskell
data'closure :: Name
data'closure = moduleNamespace ["data", "closure"]

data'closure'core :: Name
data'closure'core = moduleNamespace ["data", "closure", "core"]

data'closure'core'Closure :: QualifiedName
data'closure'core'Closure = [data'closure'core, "Closure"]
```
`compiler/hs/lib/Language/Kanagawa/Type.hs:115-122`.

`moduleNamespace = fromString . concatMap ('@':)`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:273-274`), so the *wire*
spellings are:

| Haskell binding | string that crosses the ABI |
|---|---|
| `data'closure` | `"@data@closure"` |
| `data'closure'core` | `"@data@closure@core"` |
| `data'closure'core'Closure` | the 2-element array `{"@data@closure@core", "Closure", NULL}` |

A whole module path collapses to **one** `Name` element — this is why namespace
scopes are usually depth 1 at the ABI.

### 10.2 `tIsClosure`

```haskell
tIsClosure (TInstance _ x _) = x == data'closure'core'Closure
tIsClosure x                 = maybeInner tIsClosure x
```
`compiler/hs/lib/Language/Kanagawa/Type.hs:617-618` (declared in the signature
group at `:607`, exported at `:77`).

A type is a closure **iff** it is a template instance whose template's qualified
name is exactly `["@data@closure@core", "Closure"]`. Nothing else about the type
matters. `tIsClosure` participates in `tIsValueType`
(`compiler/hs/lib/Language/Kanagawa/Type.hs:686`), gating whether a closure can
be a struct field, an interpolation operand, etc.

Uses:

| site | purpose |
|---|---|
| `compiler/hs/lib/Language/Kanagawa/Closure.hs:21` | rewrite `f(args)` where `f` is a closure into `fn(capture, args)` |
| `compiler/hs/lib/Language/Kanagawa/Type.hs:1154` | allow a call whose callee is a closure |
| `compiler/hs/lib/Language/Kanagawa/Type.hs:1176` | destructure `TInstance _ _ [(_,TTypeArg a),(_,TNameArg _ b),(_,TTypeArg c)]` into `TClosure b a c` |
| `compiler/hs/lib/Language/Kanagawa/TypeCheck.hs:129` | reduce a global closure initialiser |
| `compiler/hs/lib/Language/Kanagawa/Constexpr.hs:138` | skip constexpr reduction of closure-typed nodes |
| `compiler/hs/lib/Language/Kanagawa/Mangle.hs:31` | `isClosure` — suppresses `$fun…` in the mangled template argument, `compiler/hs/lib/Language/Kanagawa/Mangle.hs:42-44` |

### 10.3 `make_captureN`

```haskell
makeCapture LambdaF{..} = FunctionCallF (newSeq []) specifier $ renote (fmap arg) lambdaCaptures
  where
    ident        = qualifiedIdentifier declScopeName make_capture e
    make_capture = [ if null (unfix lambdaCaptures) then data'closure'core else data'closure
                   , indexedName (length $ unfix lambdaCaptures) "make_capture"
                   ]
```
`compiler/hs/lib/Language/Kanagawa/Function.hs:146-158`, with
`indexedName x = fromString . (++ show x)`
(`compiler/hs/lib/Language/Kanagawa/Desugar.hs:167-168`).

So the callee is:

* `@data@closure@core::make_capture0` when the lambda captures nothing;
* `@data@closure::make_captureN` for `N ≥ 1`.

These functions really exist in the base library:

| name | file |
|---|---|
| `make_capture0` | `library/data/closure/core.k:6`, defined `:13-24` |
| `make_capture1` … `make_capture21` | exported `library/data/closure.k:6-26`, `make_capture1` defined `:37-51` |

`Closure` itself is `template <typename FunctionType, FunctionType fn, typename Capture> using Closure = Capture;`
(`library/data/closure/core.k:9-10`) — a pure type-level alias, so the three
template arguments carry the function type, the function, and the capture struct.
The instance is built by
`closureInstance declScopeName args e = TypeIdentifierF declScopeName (templateInstance declScopeName data'closure'core'Closure args e)`
(`compiler/hs/lib/Language/Kanagawa/Function.hs:180-185`), with
`args = [lambdaType | decltype lambda, lambda, captureType]`
(`compiler/hs/lib/Language/Kanagawa/Function.hs:167`).

**At the ABI these appear as ordinary identifiers.** `ParseIdentifier("@data@closure")`,
`ParseIdentifier("make_capture3")`, assembled by
`ParseBaseList`/`ParseAppendList`/`ParseScopedIdentifier` like any other
qualified name. A Rust front end must produce byte-identical spellings —
including the `@` prefixes and the decimal `N` suffix with no padding.

---

## 11. The `undefined` contract — 18 node kinds that must be eliminated before the walk

`compile` walks the typed AST with `cataM parseWithLoc`
(`compiler/hs/app/ParseTree.hs:40`). The `parse` function
(`compiler/hs/app/ParseTree.hs:63-175`) is a total pattern match over `ExpF`
made total by a `COMPLETE` pragma on the derived pattern synonyms
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax/Pattern.hs:15`,
`compiler/hs/lib/Language/Kanagawa/Parser/Syntax/TH.hs:48-58`).

**Exactly 18 arms are `undefined`.** They are the constructors the front-end
pass pipeline is contractually required to have eliminated by the time
`ParseTree.compile` runs; reaching one is a compiler crash, not a diagnostic.
(Verified: `awk 'NR>=63 && NR<=175 && /undefined/' compiler/hs/app/ParseTree.hs | wc -l` → 18.)

| # | `ParseTree.hs` line | pattern | `ExpF` constructor | declared at | eliminated by |
|---|---|---|---|---|---|
| 1 | 71 | `Auto` | `AutoF` | `Syntax.hs:296` | `deduceAuto` pass (`Frontend.hs:58`), implemented `Type.hs:1595-1615`; `infer AutoF = TUnresolved` (`Type.hs:1174`) |
| 2 | 74 | `BitOffset{}` | `BitOffsetF a a` | `Syntax.hs:343` | `Constexpr.hs:162-164` folds `bitoffset` to a literal; residue becomes a diagnostic in `unresolved` (`Frontend.hs:402`, message via `offsetErr` `Frontend.hs:446-449`) |
| 3 | 77 | `ByteOffset{}` | `ByteOffsetF a a` | `Syntax.hs:345` | `Constexpr.hs:165-167`; residue → `Frontend.hs:401` |
| 4 | 78 | `Capture _ _` | `CaptureF QualifiedName a` | `Syntax.hs:524-528` | lambda desugaring: `makeCapture` turns each `Capture` into a `NamedValueF` argument (`Function.hs:159-160`), driven by `desugarLambdas` (`Function.hs:189`) |
| 5 | 83 | `Decltype{}` | `DecltypeF QualifiedName a` | `Syntax.hs:513-517` | `Constexpr.hs:304-305`; `infer (DecltypeF _ a) = TType (typeOf a)` (`Type.hs:1198`) |
| 6 | 88 | `EnumConstant _ _ Nothing` | `EnumConstantF { constValue = Nothing }` | `Syntax.hs:471-476` | `reifyEnums` fills every missing `constValue` with `0` or `prev+1` (`Frontend.hs:195-208`, scheduled at `Frontend.hs:72`) |
| 7 | 91 | `Error{}` | `ErrorF e` | `Syntax.hs:583` | never reaches codegen: `Main.hs:85-87` exits on `desugarErrors`, and `checkPar` short-circuits the pass pipeline on `hasErrorSymbol` (`Frontend.hs:48-52`) |
| 8 | 126 | `Lambda{}` | `LambdaF{…}` | `Syntax.hs:564-571` | `desugarLambdas` (`Function.hs:189-217`) replaces each lambda with a capture-struct alias + a `LambdaFunction` `FunctionF` + a `Closure` cast (`Function.hs:236-247`, `Function.hs:162-170`); `infer LambdaF{} = TUnresolved` (`Type.hs:1231`) |
| 9 | 134 | `ParamInt{}` | `ParamIntF a` | `Syntax.hs:308` | `Constexpr.hs:298-300` reduces `int<N>` to `IntegerF n`; residue → `Frontend.hs:404` |
| 10 | 135 | `ParamUint{}` | `ParamUintF a` | `Syntax.hs:312` | `Constexpr.hs:301-303`; residue → `Frontend.hs:405` |
| 11 | 149 | `StaticAssert _` | `StaticAssertF a` | `Syntax.hs:577` | `staticAssert` (`Frontend.hs:393-398`): `BoolLiteral True → SeqF []`, `False → desugarError`, anything else → error. Reached via `postDesugar` (`Frontend.hs:179`) |
| 12 | 152 | `StaticIf{}` | `StaticIfF{…}` | `Syntax.hs:440-444` | `Constexpr.hs:44-48` selects a branch on a constant predicate; residue → `Frontend.hs:406` |
| 13 | 159 | `Template{}` | `TemplateF{…}` | `Syntax.hs:508-511` | `instantiateTemplates` (`Frontend.hs:67`, `Template.hs`) then `removeTemplate` (`Frontend.hs:60`, implemented `Frontend.hs:298-302`) rewrites `TemplateF` into `SeqF` of diagnostics only |
| 14 | 160 | `TemplateInstance{}` | `TemplateInstanceF a a` | `Syntax.hs:320` | `instantiateTemplates` (`Frontend.hs:67`); leftovers become errors in `unresolvedTemplates` (`Frontend.hs:409-432`). **Note:** `parseInstanceType` *does* call `ParseTemplateInstance` (`ParseTree.hs:241`) — that is a synthesised instance built from the *type*, not from a surviving `TemplateInstanceF` AST node |
| 15 | 161 | `TemplateParam{}` | `TemplateParamF QualifiedName a Name Int (Maybe a)` | `Syntax.hs:322` | `removeTemplate` (`Frontend.hs:299` — `concatMap diagnostics templateParams`); also converted to `TypeParamF` by `typeParam` (`Function.hs:131`) |
| 16 | 164 | `Typename` | `TypenameF` | `Syntax.hs:326` | only ever constructed as a template-parameter kind marker (`Function.hs:299`, `:343`) and removed with its `TemplateParamF` |
| 17 | 165 | `TypeParam{}` | `TypeParamF QualifiedName a Name Int` | `Syntax.hs:324` | substituted during template instantiation — *"replacing TypeParamF nodes"* (`Template.hs:32`) |
| 18 | 168 | `Unary Clog2 _` | `UnaryF Clog2 a` | `Syntax.hs:107-113` (`Clog2` ctor at `:113`) | `Constexpr.hs:168-170` folds `clog2` of a constant; residue → `Frontend.hs:403` |

### 11.1 Four further `undefined`s outside the node dispatch

These are *not* node kinds and must not be modelled in `LoweredExp`:

| line | context | meaning |
|---|---|---|
| `ParseTree.hs:193` | `attr = \case Ecc → …; Rename → …; _ → undefined` | any `Attribute` other than `Ecc`/`Rename` reaching `AttrF` |
| `ParseTree.hs:203` | `intAttr = \case … ; _ → undefined` | any `Attribute` other than the 7 int attributes of §2.11 reaching `IntAttrF` |
| `ParseTree.hs:235` | `parseIntegerType … \| otherwise = undefined` | a type that is `tIsInt`/`tIsEnum`-true but neither signed nor unsigned |
| `ParseTree.hs:255` | `compile _ _ _ _ = undefined` | `compile` invoked with an `Options` value that is not the `Compile` constructor |

Also note two arms that produce **no ABI call at all** and must be flattened
away in the Rust lowering:

| line | pattern | behaviour |
|---|---|---|
| `ParseTree.hs:129` | `ModuleDiff _ _` | `return nullPtr` |
| `ParseTree.hs:136` | `Positional _ a` | `return a` — passes the child pointer through |
| `ParseTree.hs:175` | `Warning _ _ a` | `return a` — passes the child pointer through |

### 11.2 Implication for the Rust `LoweredExp` type

`LoweredExp` should be the `ExpF` constructor set **minus** the 18 rows above
(and with `ModuleDiffF` / `PositionalF` / `WarningF` collapsed), i.e. it is a
*post-desugar* AST with no templates, no lambdas, no `auto`, no `static if`, no
`decltype`, no constant-fold placeholders and no error nodes. Making those
states unrepresentable is precisely the guarantee the Haskell code buys with
`undefined`.

`EnumConstantF` must additionally carry a **non-optional** `constValue`
(row 6), and `IntLiteralF`'s `literalType :: Maybe LiteralType`
(`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:347-350`) may remain
optional because §7.1 shows the literal is transported as text.

---

## 12. Lifecycle: `InitCompiler` → walk → `Codegen`, and the N+1 passes

### 12.1 `InitCompiler`

```cpp
BOOL InitCompiler(const Options* options)
{
    try {
        SetupCodeGenConfig(*options);
        g_compiler = new Compiler();
        g_compiler->Init();
    } catch (const std::exception& e) {
        std::cout << e.what() << std::endl;
        return false;
    }
    return true;
}
```
`compiler/cpp/parse_tree.cpp:6283-6297`.

Order matters: `SetupCodeGenConfig` (§3.5) runs **before** the `Compiler` object
exists, because it only writes the global `g_codeGenConfig`
(`compiler/cpp/config.cpp:6`, `:134-208`). `Compiler::Compiler()` deliberately
does nothing — *"Initialize is in Init() to ensure that compiler singleton is
available during initialization"* (`compiler/cpp/compiler.cpp:13-18`).

`Compiler::Init()` (`compiler/cpp/compiler.cpp:32-56`), in order:

1. create the MLIR context, threading **disabled** (`:34`) and load dialects (`:36`);
2. `_voidType = Create<VoidType>()` (`:38`);
3. `_stringType = Create<StringType>()` (`:40`);
4. `_boolType = Create<BoolType>()` (`:42`);
5. `_floatTypes[32] = Create<FloatType>(32)` (`:44`) — **only 32-bit floats exist**;
6. `_globalClassType = Create<ClassType>(g_globalClassName, …)` (`:51`);
7. `RegisterIntrinsics()` (`:55`).

`RegisterIntrinsics` (`compiler/cpp/compiler.cpp:60-132`) registers exactly six
global functions, all with modifiers `Inline | Intrinsic`
(`compiler/cpp/compiler.cpp:61-62`) and `Public` protection:

| name | return | params | line |
|---|---|---|---|
| `__print` | `void` | variadic | 77 |
| `__debug_view` | `void` | variadic | 88 |
| `assert` | `void` | `bool` | 99 |
| `__cycles` | `uint64` | — | 109 |
| `__str_cnt` | `uint64` | — | 119 |
| `__assert_str_eq` | `void` | `string, string` | 131 |

**`InitCompiler` is not re-entrant.** It unconditionally leaks any previous
`g_compiler` (`compiler/cpp/parse_tree.cpp:6288`). It is called exactly once per
process by the front end (`compiler/hs/app/ParseTree.hs:38`).

### 12.2 The parse walk

`compile` (`compiler/hs/app/ParseTree.hs:31-41`):

```haskell
compile opt@Compile{..} cmdArgs allFiles p =
    withCAString (show backend)  $ \ptrBackend ->
    withCAString outputBaseName  $ \ptrOutput ->
    withCAString resource_usage  $ \ptrRes ->
    withCAString dgml            $ \ptrDgml ->
    withCAString dgml_detailed   $ \ptrDgmlDetailed -> do
        x <- withOptions opt cmdArgs allFiles initCompiler
        if x
            then cataM parseWithLoc p >>= codegen ptrBackend ptrOutput ptrRes ptrDgml ptrDgmlDetailed
            else return False
```

If `InitCompiler` returns false the walk is skipped entirely and `compile`
returns `False`. `outputBaseName` defaults to the base name of the **last**
input file when `--output` is empty (`compiler/hs/app/ParseTree.hs:48`).

### 12.3 `Codegen`

```c
API BOOL Codegen(const char* target, const char* output, const char* res,
                 const char* dmgl, const char* dmglDetailed, ParseTreeNodePtr root);
```
`compiler/cpp/parse_tree.h:256-257`, defined `compiler/cpp/kanagawa.cpp:8-266`.

Parameter meanings (`compiler/cpp/kanagawa.cpp:28-48`):

| param | meaning |
|---|---|
| `target` (`backend`) | `"sv"` (case-insensitive) selects SystemVerilog emission; anything else means "no codegen backend" and only parses/checks (`:30-33`, `:180-187`) |
| `output` | base file name for every output artefact (`:28`) |
| `res` | resource-usage report path; empty string disables (`:40-43`) |
| `dmgl` | DGML graph path; empty string disables (`:35-38`) |
| `dmglDetailed` | detailed DGML *directory*; empty string disables (`:45-48`) |
| `root` | the single node returned by the front-end walk |

`PD_WAIT_FOR_DEBUG` in the environment makes `Codegen` spin until a debugger
sets `g_continueDebug` (`compiler/cpp/kanagawa.cpp:6`, `:13-20`).

Whole-program sequence, once, before the multi-pass loop
(`compiler/cpp/kanagawa.cpp:52-98`):

1. `compiler.SetRoot(root, compileToVerilog)` (`:56`, impl `compiler.cpp:135-140`)
2. `compiler.FindModifiedParameters()` (`:59`)
3. `compiler.TypeCheck()` (`:64`)
4. `compiler.CheckTypeNames()` (`:69`)
5. `compiler.ReorderDeclarations()` (`:74`) — this is what calls `ParseNamespace`/`ParseIdentifier` at `compiler.cpp:1887`
6. `compiler.ExtractDeviceConfig()` (`:79`)
7. `compiler.DeviceCapabilityCheck()` (`:83`)
8. `compiler.RenameStaticLocals()` (`:86`)
9. `compiler.EnumerateCompiledModules(baseFileName)` (`:89`) — see §12.4
10. `compiler.MarkPermanentNodes()` (`:98`)

`MarkPermanentNodes` (`compiler/cpp/compiler.cpp:142-150`) asserts
`!_doneParsing && _permanentNodeCount == 0`, sets `_doneParsing = true` and
records `_permanentNodeCount = _parseTreeNodes.size()`. From that point
`Compiler::Create<T>` routes non-`Type` allocations onto
`_temporaryParseTreeCleanupList` instead (`compiler/cpp/compiler.h:344-354`).

### 12.4 `EnumerateCompiledModules` — the N+1 rule

```cpp
std::list<CompiledModule> Compiler::EnumerateCompiledModules(const std::string& defaultFileName)
```
`compiler/cpp/compiler.cpp:1937-1986`.

```cpp
struct CompiledModule {
    bool _isDefaultPass;
    ClassNode* _classNodeToCompile;
    std::string _moduleName;
    std::string _baseFileName;
    std::string _placeholderObjectName;
    std::string ModifyFileName(const std::string& inputName) const;
};
```
`compiler/cpp/compiler.h:14-23`.

* **Element 0 is always the default module** (`compiler/cpp/compiler.cpp:1941-1953`):
  `_isDefaultPass = true`, `_moduleName = GetCodeGenConfig()._moduleName`
  (i.e. `"KanagawaCore"`, `compiler/cpp/config.cpp:148`),
  `_baseFileName = defaultFileName`, `_classNodeToCompile = nullptr`.
  It is *"the default module that compiles all extern/export functions"* (`:1941-1942`).
* **Then one module per export class** (`compiler/cpp/compiler.cpp:1955-1984`):
  for every `ClassNode` in `_parseTreeNodes` with `IsExport()`, and with
  `classType->InstanceCount() == 0` **or** `_isCompilingToVerilog`
  (`:1966`), push a module with `_isDefaultPass = false`,
  `_classNodeToCompile = classNode`,
  `_moduleName = classType->GetExportedName()`,
  `_baseFileName = defaultFileName + FixupString(exportedName)`,
  `_placeholderObjectName = g_globalObjectName`.

**So the loop runs `N + 1` times for `N` qualifying export classes.**

The loop body (`compiler/cpp/kanagawa.cpp:101-240`) per module:

1. `compiler.Reset(moduleToCompile)` (`:104`)
2. `compiler.ExternalizeClasses()` (`:107`)
3. `compiler.RegisterObjects(moduleToCompile)` (`:111`)
4. `compiler.EnumerateFunctionInstances(moduleToCompile)` (`:116`) — throws `"Function instance enumeration failed"` on error (`:118-121`)
5. `compiler.GenerateIR(moduleToCompile)` (`:129`) — throws `"IR generation failed"` on error (`:131-134`)
6. **`if (moduleToCompile._isDefaultPass) continue;`** (`:137-141`) — *"Skip output for default module"*. The default pass therefore does full type-checking + instance enumeration + IR generation and then **writes nothing**.
7. otherwise: emit `.cpp`, `.h`, `.sv`, `_types.sv`, `.tcl`, `Symbols.csv`, `DebugSymbols.csv`, `HwConfig.mk`, `ClockGating.pdrpt`, `PathLength.pdrpt`, `RtlMap.json`, `.mlir` (names built `:149-161`), plus DGML/resource/detailed-DGML/memory-metadata as enabled (`:198-229`).

Report/graph file names for non-default passes are uniquified by
`CompiledModule::ModifyFileName`, which splices `_moduleName` before the
extension (`compiler/cpp/compiler.cpp:2130-2149`).

Failure handling: the whole body is inside one `try`; `std::runtime_error`
prints `"Error: " << e.what()` and returns `false`
(`compiler/cpp/kanagawa.cpp:255-260`). On success `std::cout` is explicitly
flushed *"because the process calling this function is not implemented in C++
and will not flush cout before exiting"* (`compiler/cpp/kanagawa.cpp:262-265`).

### 12.5 `Compiler::Reset` — what changes between passes

`compiler/cpp/compiler.cpp:2019-2100`.

1. assert `_doneParsing` and `_parseTreeNodes.size() >= _permanentNodeCount` (`:2021-2022`);
2. pop `_parseTreeNodes` back down to `_permanentNodeCount` (`:2025-2028`) — **this is where nodes synthesised during the previous pass are dropped**;
3. `FreeCleanupList(_temporaryParseTreeCleanupList)` (`:2030`) — actually `delete`s them;
4. `_functionByName.clear()` (`:2035`) — *"must be rebuilt on each pass because some function nodes it contains could be deleted"*;
5. `_currentCompiledModule = moduleToCompile` (`:2037`);
6. `node->Reset()` for every surviving parse-tree node (`:2040-2043`) and `Type::Reset()` for every named type (`:2045-2048`);
7. `_functionEnumerator.reset()` (`:2050`); `_nameToPath.clear()` (`:2052`);
8. **if not the default pass** (`:2054-2083`):
   * strip `Export | ExportClassInterface | External` from every non-external-class `FunctionNode` (`:2060-2074`);
   * `moduleToCompile._classNodeToCompile->MarkInterfaceFunctions(Export | ExportClassInterface)` (`:2077-2079`);
   * `moduleToCompile._classNodeToCompile->CreateExternCallbacks()` (`:2082`) — the path that reaches `ClassNode::CreateCallbacksHelper` and hence the `ir.cpp:11990-12038` ABI calls of §5.2;
9. **always** (`:2085-2098`): for every *other* export `ClassNode`,
   `MarkInterfaceFunctions(External | ExportClassInterface)` and
   `PruneForExportOrExternalClassUse()` — which calls
   `ParseBaseList(nullptr)` to blank every method body
   (`compiler/cpp/parse_tree.cpp:5157`).

Because `Reset` deletes nodes and later passes allocate new ones, **`ParseTreeNodePtr`
values are recycled across passes**. Any pointer→id map in an interposer must be
epoch-scoped (§14.2).

### 12.6 Teardown

`Compiler::~Compiler` (`compiler/cpp/compiler.cpp:20-30`) frees the permanent
list first, then the temporary list, and asserts `_functionNodes.empty()`. The
front end never calls a destructor — the process simply exits
(`compiler/hs/app/Main.hs:102-104`).

---

## 13. Error and warning code table (`compiler/cpp/errors.h`)

`errors.h` defines the numeric codes only; the human-readable text lives at each
call site as a `<<`-chained `ErrStream`. The block below therefore carries, per
code: the symbolic name, the declaring line, the number of raise sites found in
`compiler/cpp/*.{cpp,h}`, and every **distinct first message fragment** with the
`file:line` where it is emitted. This is the feed for `kg explain`.

### 13.1 How a code becomes output

```cpp
ErrStream(const std::string& fileName, const size_t lineNumber, const CompileError error,
          const std::string& callTrace)
  : _stream(std::cout), _changeConsoleColor(ConsoleColor::Red), _callTrace(callTrace)
{
    _stream << "Error " << static_cast<uint32_t>(error) << " at: " << fileName
            << " (" << lineNumber << "): ";
}
```
`compiler/cpp/errors.h:106-111`. Warnings use the same shape with
`"Warning "` and `ConsoleColor::Yellow` (`compiler/cpp/errors.h:113-123`). The
destructor appends `"\n"`, then the call trace, then flushes
(`compiler/cpp/errors.h:125-133`).

Two warning overloads exist: one with a location
(`compiler/cpp/errors.h:113-117`) and one without
(`compiler/cpp/errors.h:119-123`, prefix `"Warning N: "`).

So the wire format of a diagnostic line is:

```
Error <code> at: <sourceFile> (<line>): <message>
Warning <code> at: <sourceFile> (<line>): <message>
Warning <code>: <message>
```

`Compiler::ErrorStream` (`compiler/cpp/compiler.cpp:173-179`) sets
`_compileError = true` and resolves the file name via
`GetSourceFileName(location._fileIndex)`; note it reports
`location._beginLine` only — column is never printed.
`Compiler::WarningStream` (`compiler/cpp/compiler.cpp:181-189`) promotes to an
error when `GetCodeGenConfig()._warningsAsErrors` is set.

Code **1 `SyntaxError`** has no C++ raise site; it belongs to the Haskell front
end, whose only literal use is
`exitError "Error 1: Warnings treated as errors"`
(`compiler/hs/app/Main.hs:99`); parse/desugar diagnostics are rendered by
megaparsec bundles (`compiler/hs/app/Main.hs:87`, `:97`).

Codes with **zero** C++ raise sites (reserved or front-end-only): 1
`SyntaxError`, 4 `UnknownType`, 8 `MultipleMemoryWrites`, 10
`PartialElementWrite`, 28 `InvalidContainer`, 31 `InvalidExtern`, 38
`InvalidWaitForCondition`, 40 `UnmatchedComment`, 41 `InvalidReset`, 45
`TooManyReadPorts`, 47 `InvalidPerThreadUsage`, 49 `ReservedName`, 58
`InvalidClockSpecifier`, 68 `InvalidRAMInitialization`, 72
`InvalidMemoryReadDelay`, 76 `DeprecatedFeature`, 78 `InvalidIntrinsicCall`.

Numbers **15, 23, 39, 42, 46, 48, 55, 80** are absent from the enum entirely —
the sequence has holes. Do not renumber.

`CompileWarning::FreeNonInlineFunction` (13) likewise has no C++ raise site; it
corresponds to the Haskell pass `freeNonInlineFunctions`
(`compiler/hs/lib/Language/Kanagawa/Frontend.hs:227-231`, message
`"Free functions must be declared \`inline\`"`).

The Haskell-side warning kinds that gate `CodeGenOptions` bits are
`Conversion`, `Deprecated`, `InitializerOverrides`, `RedundantImport`, `Shadow`,
`TransactionSizeMissing`
(`compiler/hs/lib/Language/Kanagawa/Warning.hs:20-35`).

### 13.2 Machine-readable table

```json
{
 "errors": [
  {"code": 1, "name": "SyntaxError", "decl": "cpp/errors.h:8", "sites": 0, "messages": []},
  {"code": 2, "name": "DuplicateFunction", "decl": "cpp/errors.h:9", "sites": 1, "messages": [{"text": "Duplicate function: ", "at": "cpp/compiler.cpp:220"}]},
  {"code": 3, "name": "DuplicateType", "decl": "cpp/errors.h:10", "sites": 5, "messages": [{"text": "Duplicate type: ", "at": "cpp/compiler.cpp:235"}, {"text": "Duplicate symbol: ", "at": "cpp/compiler.cpp:1388"}, {"text": "Duplicate typedef: ", "at": "cpp/compiler.cpp:1515"}]},
  {"code": 4, "name": "UnknownType", "decl": "cpp/errors.h:11", "sites": 0, "messages": []},
  {"code": 5, "name": "UnknownFunction", "decl": "cpp/errors.h:12", "sites": 6, "messages": [{"text": "Undefined function: ", "at": "cpp/compiler.cpp:373"}, {"text": "Callback (", "at": "cpp/enumerate_function_instances.cpp:1290"}, {"text": "Function: ", "at": "cpp/ir.cpp:345"}, {"text": "Specify --import-dir for the standard library in order to use DSP functions.", "at": "cpp/ir.cpp:6540"}, {"text": "Specify --import-dir for the standard library in order to use floating point operators.", "at": "cpp/ir.cpp:6636"}]},
  {"code": 6, "name": "ExportTypeRestriction", "decl": "cpp/errors.h:13", "sites": 3, "messages": [{"text": "Fixed-latency extern methods cannot have composite return types", "at": "cpp/ir.cpp:12436"}, {"text": "Fixed-latency extern method parameters cannot have composite types", "at": "cpp/ir.cpp:12451"}, {"text": "Type: ", "at": "cpp/parse_tree.cpp:258"}]},
  {"code": 7, "name": "InvalidGlobalAssignment", "decl": "cpp/errors.h:14", "sites": 3, "messages": [{"text": "Variable cannot be initialized to a value of type: ", "at": "cpp/ir.cpp:734"}, {"text": "Constant shared variables which contain strings cannot be initialized to a value which references ", "at": "cpp/ir.cpp:4406"}, {"text": "Objects can only be initialized with a designated initializer list", "at": "cpp/ir.cpp:4782"}]},
  {"code": 8, "name": "MultipleMemoryWrites", "decl": "cpp/errors.h:15", "sites": 0, "messages": []},
  {"code": 9, "name": "InvalidArrayIndexing", "decl": "cpp/errors.h:16", "sites": 8, "messages": [{"text": "Accessing objects through an array requires the array index to be known at compile time", "at": "cpp/ir.cpp:5271"}, {"text": "Dynamic array indexing requires a value type", "at": "cpp/ir.cpp:5539"}, {"text": "Object arrays must be referenced with compile-time-known indices", "at": "cpp/ir.cpp:5620"}, {"text": "Negative array index", "at": "cpp/ir.cpp:10233"}, {"text": "Invalid array index", "at": "cpp/ir.cpp:10243"}, {"text": "Modifying a member of an array of structs is not supported with memories", "at": "cpp/ir.cpp:11746"}, {"text": "Array index must be an integer", "at": "cpp/parse_tree.cpp:48"}, {"text": "Indexing into a non-array type", "at": "cpp/parse_tree.cpp:4608"}]},
  {"code": 10, "name": "PartialElementWrite", "decl": "cpp/errors.h:17", "sites": 0, "messages": []},
  {"code": 11, "name": "InvalidLiteral", "decl": "cpp/errors.h:18", "sites": 5, "messages": [{"text": "Mod operator requires both sides to be known at compile time, or the right-hand side to be ", "at": "cpp/ir.cpp:7291"}, {"text": "Negative shift amounts are not supported", "at": "cpp/ir.cpp:7403"}, {"text": "Compile time divide by zero", "at": "cpp/ir.cpp:7412"}, {"text": "Right hand size of % operation must be >= 1", "at": "cpp/parse_tree.cpp:3157"}]},
  {"code": 12, "name": "UnorderedOperation", "decl": "cpp/errors.h:19", "sites": 2, "messages": [{"text": "Unordered loops are not allowed in ordered functions", "at": "cpp/ir.cpp:8180"}, {"text": "Ordered functions cannot call unordered functions", "at": "cpp/ir.cpp:10849"}]},
  {"code": 13, "name": "InvalidSwitch", "decl": "cpp/errors.h:20", "sites": 5, "messages": [{"text": "Case value not known at compile time", "at": "cpp/ir.cpp:12133"}, {"text": "Switch statement value must be an unsigned integer", "at": "cpp/parse_tree.cpp:5319"}, {"text": "Case values must be unique", "at": "cpp/parse_tree.cpp:5346"}, {"text": "Case statements values must be integers known at compile time", "at": "cpp/parse_tree.cpp:5351"}, {"text": "Switch statement cannot have 2 default blocks", "at": "cpp/parse_tree.cpp:5363"}]},
  {"code": 14, "name": "InvalidConversion", "decl": "cpp/errors.h:21", "sites": 3, "messages": [{"text": "Can't convert value of type ", "at": "cpp/parse_tree.cpp:220"}, {"text": "Can't implicitly convert value of type ", "at": "cpp/parse_tree.cpp:244"}, {"text": "Assigning void value", "at": "cpp/parse_tree.cpp:2837"}]},
  {"code": 16, "name": "InvalidType", "decl": "cpp/errors.h:22", "sites": 24, "messages": [{"text": "Unknown string value (should you mark it const?)", "at": "cpp/ir.cpp:160"}, {"text": "Integer bit width must be positive", "at": "cpp/parse_tree.cpp:966"}, {"text": "Variables must not have void type", "at": "cpp/parse_tree.cpp:2360"}, {"text": "for loop induction variable must be an unsigned integer", "at": "cpp/parse_tree.cpp:2671"}, {"text": "for loop bound must be unsigned, or be known at compile time and non-negative", "at": "cpp/parse_tree.cpp:2677"}, {"text": "`static for` loop bound must be an integer", "at": "cpp/parse_tree.cpp:2714"}, {"text": "`static for` loop variable must be an unsigned integer", "at": "cpp/parse_tree.cpp:2740"}, {"text": "Unsupported binary operation on boolean types", "at": "cpp/parse_tree.cpp:3037"}, {"text": "Denominator of / % operations must be unsigned", "at": "cpp/parse_tree.cpp:3085"}, {"text": "Operation is not supported on integers", "at": "cpp/parse_tree.cpp:3091"}, {"text": "Operation is not supported on floating point numbers", "at": "cpp/parse_tree.cpp:3208"}, {"text": "Binary operations are only supported on scalar types", "at": "cpp/parse_tree.cpp:3216"}, {"text": "The invert operator applies to integer types only", "at": "cpp/parse_tree.cpp:3330"}, {"text": "The negate operator applies to integer and float types only", "at": "cpp/parse_tree.cpp:3350"}, {"text": "The logical invert operator applies to boolean types only", "at": "cpp/parse_tree.cpp:3359"}, {"text": "If conditional expression must be boolean", "at": "cpp/parse_tree.cpp:3553"}, {"text": "bytesizeof/byteoffsetof require a bit count that is a multiple of 8.  Actual bit count: ", "at": "cpp/parse_tree.cpp:3752"}, {"text": "Sizeof operations are not allowed on void/object types", "at": "cpp/parse_tree.cpp:3774"}, {"text": "Type error, expecting struct, union, or class", "at": "cpp/parse_tree.cpp:4713"}, {"text": "fan_out requires count to be an unsigned integer, or known to be non-negative at compile time", "at": "cpp/parse_tree.cpp:5427"}, {"text": "concat element type must have positive width", "at": "cpp/parse_tree.cpp:5475"}, {"text": "static_assert parameter must have type: bool", "at": "cpp/parse_tree.cpp:6833"}, {"text": "Callback: ", "at": "cpp/parse_tree.cpp:7343"}]},
  {"code": 17, "name": "InvalidArraySize", "decl": "cpp/errors.h:23", "sites": 2, "messages": [{"text": "fan_out requires count to be known at compile time", "at": "cpp/parse_tree.cpp:5436"}, {"text": "fan_out requires count to be positive", "at": "cpp/parse_tree.cpp:5442"}]},
  {"code": 18, "name": "InvalidFunctionModifier", "decl": "cpp/errors.h:24", "sites": 25, "messages": [{"text": "[[pure]] is only allowed on external functions", "at": "cpp/ir.cpp:3293"}, {"text": "[[pipelined]] is not allowed on exported functions", "at": "cpp/ir.cpp:3299"}, {"text": "[[no_backpressure]]/[[latency]] callbacks must also have the [[async]] attribute.", "at": "cpp/ir.cpp:3313"}, {"text": "[[no_backpressure]] cannot be used with control flow (rolled loops, calls to ", "at": "cpp/ir.cpp:3514"}, {"text": "[[no_backpressure]] cannot be used with control calls to non-inline functions", "at": "cpp/ir.cpp:3525"}, {"text": "The same function modifier can be specified more than once", "at": "cpp/parse_tree.cpp:1446"}, {"text": "Function max thread count cannot be specified on an inline function", "at": "cpp/parse_tree.cpp:1523"}, {"text": "Function max thread count can only be specified once", "at": "cpp/parse_tree.cpp:1530"}, {"text": "Function max thread count must be positive", "at": "cpp/parse_tree.cpp:1536"}, {"text": "Function max thread count must be a power of 2", "at": "cpp/parse_tree.cpp:1541"}, {"text": "Function max thread count exceeds the limit set by --max-threads-limit", "at": "cpp/parse_tree.cpp:1552"}, {"text": "Function thread rate count cannot be specified on an inline function", "at": "cpp/parse_tree.cpp:1582"}, {"text": "Function thread rate count can only be specified once", "at": "cpp/parse_tree.cpp:1589"}, {"text": "Function thread rate must be greater than 0", "at": "cpp/parse_tree.cpp:1595"}, {"text": "Async functions cannot be marked unordered", "at": "cpp/parse_tree.cpp:1835"}, {"text": "Extern module functions cannot have the export modifier", "at": "cpp/parse_tree.cpp:1843"}, {"text": "\\'export void startup()\\' is deprecated.  Use \\'[[reset]] void startup()\\' instead.", "at": "cpp/parse_tree.cpp:1850"}, {"text": "Functions with the [[reset]] attribute cannot have other attributes or modifiers.", "at": "cpp/parse_tree.cpp:1858"}, {"text": "Pipelined functions cannot be unordered", "at": "cpp/parse_tree.cpp:1993"}, {"text": "export functions cannot be unordered", "at": "cpp/parse_tree.cpp:1999"}, {"text": "Inline functions cannot have attributes except unordered", "at": "cpp/parse_tree.cpp:2008"}, {"text": "[[no_backpressure]]/[[latency]] cannot be used with max_threads", "at": "cpp/parse_tree.cpp:2044"}, {"text": "[[no_backpressure]]/[[latency]] cannot be used with thread_rate", "at": "cpp/parse_tree.cpp:2050"}, {"text": "[[no_backpressure]]/[[latency]] cannot be used with any attributes/modifiers except ", "at": "cpp/parse_tree.cpp:2060"}]},
  {"code": 19, "name": "InvalidNesting", "decl": "cpp/errors.h:25", "sites": 1, "messages": [{"text": "Nested functions are not allowed", "at": "cpp/parse_tree.cpp:1925"}]},
  {"code": 20, "name": "InvalidFunctionReturnType", "decl": "cpp/errors.h:26", "sites": 2, "messages": [{"text": "Async functions cannot return a value", "at": "cpp/compiler.cpp:203"}, {"text": "Async function return type must be void", "at": "cpp/parse_tree.cpp:1987"}]},
  {"code": 21, "name": "InvalidReturn", "decl": "cpp/errors.h:27", "sites": 5, "messages": [{"text": "Functions can only have 1 return statement", "at": "cpp/parse_tree.cpp:1951"}, {"text": "Non-void functions must have 1 return statement", "at": "cpp/parse_tree.cpp:1960"}, {"text": "Return statement must be the last statement in a function", "at": "cpp/parse_tree.cpp:1976"}, {"text": "return void is not allowed", "at": "cpp/parse_tree.cpp:3445"}, {"text": "non-void function returning void", "at": "cpp/parse_tree.cpp:3456"}]},
  {"code": 22, "name": "InvalidIterationCount", "decl": "cpp/errors.h:28", "sites": 2, "messages": [{"text": "Pipelined function requires unsigned integer count parameter", "at": "cpp/parse_tree.cpp:2018"}, {"text": "Pipelined function requires unsigned integer count argument", "at": "cpp/parse_tree.cpp:2028"}]},
  {"code": 24, "name": "NonValueType", "decl": "cpp/errors.h:29", "sites": 3, "messages": [{"text": "Invalid memory element type", "at": "cpp/parse_tree.cpp:1349"}, {"text": "Class/Memory local variables are not supported", "at": "cpp/parse_tree.cpp:2297"}, {"text": "Struct cannot contain a class or memory; unsupported field type: ", "at": "cpp/parse_tree.cpp:4393"}]},
  {"code": 25, "name": "InvalidLoop", "decl": "cpp/errors.h:30", "sites": 8, "messages": [{"text": "Control flow not allowed inside atomic do-while loop", "at": "cpp/ir.cpp:7860"}, {"text": "`static for` loop bound is not known by the compiler", "at": "cpp/ir.cpp:8583"}, {"text": "Calls to non-inline functions are not allowed inside of atomic do-while loop", "at": "cpp/ir.cpp:10775"}, {"text": "[[unordered]] and [[reorder_by_looping]] loop modifiers cannot be combined", "at": "cpp/parse_tree.cpp:2627"}, {"text": "Loop conditional expression must be boolean (uint1)", "at": "cpp/parse_tree.cpp:2642"}, {"text": "`static for` loop bound is negative", "at": "cpp/parse_tree.cpp:2727"}, {"text": "`static for` loop variable is not wide enough (consider using auto).  Required bit count: ", "at": "cpp/parse_tree.cpp:2746"}]},
  {"code": 26, "name": "InvalidMux", "decl": "cpp/errors.h:31", "sites": 3, "messages": [{"text": "MUX first operand must have bool or uint type", "at": "cpp/parse_tree.cpp:2910"}, {"text": "Incorrect MUX expression list size, expected: ", "at": "cpp/parse_tree.cpp:2920"}, {"text": "MUX is too large", "at": "cpp/parse_tree.cpp:2939"}]},
  {"code": 27, "name": "InvalidCall", "decl": "cpp/errors.h:32", "sites": 26, "messages": [{"text": "Left of . in a method call must be an object/extern module", "at": "cpp/enumerate_function_instances.cpp:430"}, {"text": "Callback: ", "at": "cpp/enumerate_function_instances.cpp:1690"}, {"text": "Exported functions cannot be called", "at": "cpp/ir.cpp:10725"}, {"text": "Non-external functions with the [[latency()]] attribute are not callable from Kanagawa code", "at": "cpp/ir.cpp:10784"}, {"text": "[[latency]] can only be applied to methods of external or exported classes", "at": "cpp/ir.cpp:10808"}, {"text": "Call rate cannot be 0", "at": "cpp/ir.cpp:12827"}, {"text": "Transaction size cannot be 0", "at": "cpp/ir.cpp:12859"}, {"text": "Invalid call attribute", "at": "cpp/ir.cpp:12868"}, {"text": "Functions with the [[reset]] attribute must not call external functions.  ", "at": "cpp/lower.cpp:6159"}, {"text": "function calls must be contained inside of functions", "at": "cpp/parse_tree.cpp:3829"}, {"text": "Left of . in a method call must be a class instance", "at": "cpp/parse_tree.cpp:3869"}, {"text": "Transaction size specified without [[last]] argument", "at": "cpp/parse_tree.cpp:3911"}, {"text": "Calls to inline functions cannot have modifiers", "at": "cpp/parse_tree.cpp:3924"}, {"text": "[[transaction_size]] cannot be used when calling an inline function", "at": "cpp/parse_tree.cpp:3930"}, {"text": "Calls to fixed latency extern function cannot have [[transaction_size]] modifier", "at": "cpp/parse_tree.cpp:3940"}, {"text": "Argument(", "at": "cpp/parse_tree.cpp:3958"}, {"text": "[[last]] argument cannot have constant false value", "at": "cpp/parse_tree.cpp:4007"}, {"text": "Thread count specified: ", "at": "cpp/parse_tree.cpp:4030"}, {"text": "Thread count for inline pipelined call must be positive", "at": "cpp/parse_tree.cpp:4039"}, {"text": "Thread count for inline pipelined call must be known at compile time", "at": "cpp/parse_tree.cpp:4045"}]},
  {"code": 28, "name": "InvalidContainer", "decl": "cpp/errors.h:33", "sites": 0, "messages": []},
  {"code": 29, "name": "ProtectionModifier", "decl": "cpp/errors.h:34", "sites": 2, "messages": [{"text": "Calling private member function", "at": "cpp/parse_tree.cpp:3886"}, {"text": "Accessing private member variable:", "at": "cpp/parse_tree.cpp:4696"}]},
  {"code": 30, "name": "InvalidMember", "decl": "cpp/errors.h:35", "sites": 2, "messages": [{"text": "Object does not contain member variable: ", "at": "cpp/ir.cpp:1375"}, {"text": "Unknown member: ", "at": "cpp/parse_tree.cpp:4705"}]},
  {"code": 31, "name": "InvalidExtern", "decl": "cpp/errors.h:36", "sites": 0, "messages": []},
  {"code": 32, "name": "UnknownIdentifier", "decl": "cpp/errors.h:37", "sites": 1, "messages": [{"text": "Unknown symbol: ", "at": "cpp/parse_tree.h:1372"}]},
  {"code": 33, "name": "DuplicateIdentifier", "decl": "cpp/errors.h:38", "sites": 1, "messages": [{"text": "Duplicate symbol in the same scope: ", "at": "cpp/parse_tree.h:1209"}]},
  {"code": 34, "name": "InvalidAtomic", "decl": "cpp/errors.h:39", "sites": 15, "messages": [{"text": "Unsupported operation inside of atomic do-while loop", "at": "cpp/ir.cpp:7869"}, {"text": "memory read not supported inside of atomic do-while loop", "at": "cpp/ir.cpp:7876"}, {"text": "memory write not supported inside of atomic do-while loop", "at": "cpp/ir.cpp:7882"}, {"text": "Illegal loop within an atomic block", "at": "cpp/ir.cpp:8165"}, {"text": "Illegal control flow within an atomic block", "at": "cpp/ir.cpp:9622"}, {"text": "For atomic blocks that access memories, all memory indices must be computed before the ", "at": "cpp/ir.cpp:9854"}, {"text": "Memory loads and stores in non-leaf atomic block are not supported. Memory name: ", "at": "cpp/ir.cpp:9930"}, {"text": "Nested atomics are not supported", "at": "cpp/ir.cpp:10041"}, {"text": "Control-flow operations within atomic blocks are not supported", "at": "cpp/lower.cpp:6478"}, {"text": "Control-flow, fan_out, DSP, and memory operations within atomic blocks are not ", "at": "cpp/lower.cpp:6488"}, {"text": "Basic block ended with outstanding atomics - function calls are not ", "at": "cpp/lower.cpp:6551"}, {"text": "Atomic do-while loop cannot be nested in another atomic/schedule", "at": "cpp/parse_tree.cpp:3640"}, {"text": "Multiple do-while loops inside atomic", "at": "cpp/parse_tree.cpp:3647"}, {"text": "Do-while loop inside schedule block must have update rate of 1", "at": "cpp/parse_tree.cpp:3678"}, {"text": "Invalid operation in atomic outside of do-while loop", "at": "cpp/parse_tree.cpp:3703"}]},
  {"code": 35, "name": "CallRateNotPredicated", "decl": "cpp/errors.h:40", "sites": 1, "messages": [{"text": "Non-trivial call rate is used, but all threads executing this path will result in a call.  Control ", "at": "cpp/ir.cpp:12832"}]},
  {"code": 36, "name": "ExpressionNotStatic", "decl": "cpp/errors.h:41", "sites": 2, "messages": [{"text": "Expression value is not known at compile time", "at": "cpp/ir.cpp:12291"}, {"text": "static_assert value not known at compile time\\n", "at": "cpp/ir.cpp:12468"}]},
  {"code": 37, "name": "RecursionNotSupported", "decl": "cpp/errors.h:42", "sites": 1, "messages": [{"text": " recursion detected in function: ", "at": "cpp/enumerate_function_instances.cpp:1008"}]},
  {"code": 38, "name": "InvalidWaitForCondition", "decl": "cpp/errors.h:43", "sites": 0, "messages": []},
  {"code": 40, "name": "UnmatchedComment", "decl": "cpp/errors.h:44", "sites": 0, "messages": []},
  {"code": 41, "name": "InvalidReset", "decl": "cpp/errors.h:45", "sites": 0, "messages": []},
  {"code": 43, "name": "StaticAssert", "decl": "cpp/errors.h:46", "sites": 1, "messages": [{"text": "static_assert failed at: ", "at": "cpp/ir.cpp:12477"}]},
  {"code": 44, "name": "TypeToWide", "decl": "cpp/errors.h:47", "sites": 3, "messages": [{"text": "Maximum type width exceeded with type of width: ", "at": "cpp/compiler.cpp:516"}]},
  {"code": 45, "name": "TooManyReadPorts", "decl": "cpp/errors.h:48", "sites": 0, "messages": []},
  {"code": 47, "name": "InvalidPerThreadUsage", "decl": "cpp/errors.h:49", "sites": 0, "messages": []},
  {"code": 49, "name": "ReservedName", "decl": "cpp/errors.h:50", "sites": 0, "messages": []},
  {"code": 50, "name": "InvalidInspectionTarget", "decl": "cpp/errors.h:51", "sites": 1, "messages": [{"text": "Variable: ", "at": "cpp/compiler.cpp:1233"}]},
  {"code": 51, "name": "TooManyInspectableVariables", "decl": "cpp/errors.h:52", "sites": 1, "messages": [{"text": "Limit of inspectable() statements reached", "at": "cpp/compiler.cpp:1212"}]},
  {"code": 52, "name": "InspectableTooDeep", "decl": "cpp/errors.h:53", "sites": 1, "messages": [{"text": "Memory: ", "at": "cpp/compiler.cpp:1244"}]},
  {"code": 53, "name": "InvalidString", "decl": "cpp/errors.h:54", "sites": 7, "messages": [{"text": "memories cannot contain strings", "at": "cpp/parse_tree.cpp:515"}, {"text": "Shared string initialized to non-empty string: ", "at": "cpp/parse_tree.cpp:2402"}, {"text": "Unions cannot contain strings", "at": "cpp/parse_tree.cpp:4401"}, {"text": "String parameters not allowed on public methods of exported classes", "at": "cpp/parse_tree.cpp:5010"}, {"text": "String return type not allowed on public methods of exported classes", "at": "cpp/parse_tree.cpp:5017"}, {"text": "String parameters not allowed for callbacks contained in exported classes", "at": "cpp/parse_tree.cpp:5032"}, {"text": "String return type not allowed for callbacks contained in exported classes", "at": "cpp/parse_tree.cpp:5039"}]},
  {"code": 54, "name": "InvalidConst", "decl": "cpp/errors.h:55", "sites": 1, "messages": [{"text": "const variables must be assigned during declaration", "at": "cpp/parse_tree.cpp:2311"}]},
  {"code": 56, "name": "InvalidEndTransaction", "decl": "cpp/errors.h:56", "sites": 2, "messages": [{"text": "[[last]] attribute can only be used with boolean parameters", "at": "cpp/parse_tree.cpp:1891"}, {"text": "[[last]] attribute can only appear once per function", "at": "cpp/parse_tree.cpp:1901"}]},
  {"code": 57, "name": "InvalidUpdateRate", "decl": "cpp/errors.h:57", "sites": 1, "messages": [{"text": "[[schedule]] attribute cannot be zero", "at": "cpp/parse_tree.cpp:3590"}]},
  {"code": 58, "name": "InvalidClockSpecifier", "decl": "cpp/errors.h:58", "sites": 0, "messages": []},
  {"code": 59, "name": "InvalidDebugView", "decl": "cpp/errors.h:59", "sites": 6, "messages": [{"text": "Duplicate debug view parameter name: ", "at": "cpp/ir.cpp:195"}, {"text": "__debug_view must be passed 1 argument (label string)", "at": "cpp/parse_tree.cpp:4061"}, {"text": "First argument passed to __debug_view must be a string", "at": "cpp/parse_tree.cpp:4066"}, {"text": "__debug_view arguments must have the form (label, name1, value1, name2, value2) - value ", "at": "cpp/parse_tree.cpp:4075"}, {"text": "__debug_view arguments must have the form (label, name1, value1, name2, value2) - expecting ", "at": "cpp/parse_tree.cpp:4083"}]},
  {"code": 60, "name": "DivideByZero", "decl": "cpp/errors.h:60", "sites": 1, "messages": [{"text": "Divide by zero", "at": "cpp/ir.cpp:6944"}]},
  {"code": 61, "name": "AssignmentToConst", "decl": "cpp/errors.h:61", "sites": 2, "messages": [{"text": "Invalid assignment. The variable `", "at": "cpp/ir.cpp:4960"}]},
  {"code": 62, "name": "InvalidInitializerList", "decl": "cpp/errors.h:62", "sites": 9, "messages": [{"text": "Initializer list element not known at compile time", "at": "cpp/ir.cpp:12661"}, {"text": "A class cannot contain multiple default initializers", "at": "cpp/parse_tree.cpp:4907"}, {"text": "Initializer list used in a context where type cannot be inferred", "at": "cpp/parse_tree.cpp:6974"}, {"text": "Initializer list contains too many elements (", "at": "cpp/parse_tree.cpp:6993"}, {"text": "Unions cannot be initialized with a non-empty initializer list", "at": "cpp/parse_tree.cpp:7010"}, {"text": "Non-empty initializer list cannot be assigned to type: ", "at": "cpp/parse_tree.cpp:7035"}, {"text": "Designated initializer list used in a context where type cannot be inferred", "at": "cpp/parse_tree.cpp:7134"}, {"text": "Designated initializer list cannot be assigned to type: ", "at": "cpp/parse_tree.cpp:7187"}]},
  {"code": 63, "name": "InvalidStaticLocal", "decl": "cpp/errors.h:63", "sites": 2, "messages": [{"text": "Static local objects cannot contain exported methods", "at": "cpp/enumerate_function_instances.cpp:1148"}, {"text": "Static local objects cannot be defined within class methods", "at": "cpp/parse_tree.cpp:2303"}]},
  {"code": 64, "name": "BasicBlockNotSchedulable", "decl": "cpp/errors.h:64", "sites": 1, "messages": [{"text": "[[schedule()]] constraints are impossible to satisfy", "at": "cpp/schedule.cpp:331"}]},
  {"code": 65, "name": "InvalidAttribute", "decl": "cpp/errors.h:65", "sites": 3, "messages": [{"text": "Functions connected to external class callbacks can only have noinline, [[async]], ", "at": "cpp/enumerate_function_instances.cpp:170"}, {"text": "The same attribute cannot be specified multiple times", "at": "cpp/parse_tree.cpp:68"}, {"text": "unexpected [[reorder_by_looping]] with for loop", "at": "cpp/parse_tree.cpp:2683"}]},
  {"code": 66, "name": "InvalidExportClass", "decl": "cpp/errors.h:66", "sites": 2, "messages": [{"text": "Global variables cannot be accessed (directly or indirectly) from an exported class", "at": "cpp/ir.cpp:4925"}, {"text": "Export class instances must be contained within classes", "at": "cpp/parse_tree.cpp:2370"}]},
  {"code": 67, "name": "InvalidQuadPortMemory", "decl": "cpp/errors.h:67", "sites": 2, "messages": [{"text": "has more than two read locations", "at": "cpp/lower.cpp:6853"}, {"text": "has more than two write locations", "at": "cpp/lower.cpp:6859"}]},
  {"code": 68, "name": "InvalidRAMInitialization", "decl": "cpp/errors.h:68", "sites": 0, "messages": []},
  {"code": 69, "name": "InvalidEccFunction", "decl": "cpp/errors.h:69", "sites": 5, "messages": [{"text": "ECC function must be inline, and must have any other modifiers or attributes", "at": "cpp/parse_tree.cpp:4566"}, {"text": "ECC function first parameter type must be bool", "at": "cpp/parse_tree.cpp:4574"}, {"text": "ECC function second parameter type must be bool", "at": "cpp/parse_tree.cpp:4580"}, {"text": "ECC function third parameter type must match the type of data stored in the memory", "at": "cpp/parse_tree.cpp:4586"}, {"text": "ECC function must have exactly 3 parameters", "at": "cpp/parse_tree.cpp:4592"}]},
  {"code": 70, "name": "EccUnsupported", "decl": "cpp/errors.h:70", "sites": 2, "messages": [{"text": "Target device does not support ECC memories", "at": "cpp/compiler.cpp:1461"}, {"text": "ECC memories with initial values are not supported", "at": "cpp/parse_tree.cpp:2379"}]},
  {"code": 71, "name": "AmbiguousDebugSymbols", "decl": "cpp/errors.h:71", "sites": 1, "messages": []},
  {"code": 72, "name": "InvalidMemoryReadDelay", "decl": "cpp/errors.h:72", "sites": 0, "messages": []},
  {"code": 73, "name": "MethodReturningReference", "decl": "cpp/errors.h:73", "sites": 1, "messages": [{"text": "Methods returning captured `this` are not supported", "at": "cpp/parse_tree.cpp:4264"}]},
  {"code": 74, "name": "InvalidExternFunction", "decl": "cpp/errors.h:74", "sites": 3, "messages": [{"text": "Methods of non-external classes must have implementations", "at": "cpp/ir.cpp:3321"}, {"text": "Calls to inline pipelined functions cannot be made within flat control flow (if/else/switch).", "at": "cpp/ir.cpp:10794"}]},
  {"code": 75, "name": "InvalidCallback", "decl": "cpp/errors.h:75", "sites": 5, "messages": [{"text": "A single function cannot be connected to multiple external class callbacks: ", "at": "cpp/enumerate_function_instances.cpp:131"}, {"text": "Functions which are connected to external class callbacks cannot have other call sites: ", "at": "cpp/enumerate_function_instances.cpp:1252"}, {"text": "[[latency()]] attribute specified on callback: ", "at": "cpp/parse_tree.cpp:5063"}, {"text": "[[no_backpressure]] attribute specified on callback: ", "at": "cpp/parse_tree.cpp:5070"}, {"text": "Callbacks cannot be connected to methods of module global objects", "at": "cpp/parse_tree.cpp:7412"}]},
  {"code": 76, "name": "DeprecatedFeature", "decl": "cpp/errors.h:76", "sites": 0, "messages": []},
  {"code": 77, "name": "InvalidAssignment", "decl": "cpp/errors.h:77", "sites": 1, "messages": [{"text": "Assignment to string/memory/objects is not supported", "at": "cpp/parse_tree.cpp:2842"}]},
  {"code": 78, "name": "InvalidIntrinsicCall", "decl": "cpp/errors.h:78", "sites": 0, "messages": []},
  {"code": 79, "name": "InvalidTemplateArgument", "decl": "cpp/errors.h:79", "sites": 1, "messages": [{"text": "Unsupported external class template argument.  The only supported types are integers and strings.", "at": "cpp/parse_tree.cpp:7556"}]},
  {"code": 81, "name": "MissingDeviceConfigProperty", "decl": "cpp/errors.h:80", "sites": 1, "messages": [{"text": "Invalid device configuration", "at": "cpp/compiler.cpp:1426"}]},
  {"code": 82, "name": "InvalidDeviceConfigProperty", "decl": "cpp/errors.h:81", "sites": 17, "messages": [{"text": "Device configuration property '", "at": "cpp/device_config_visitor.cpp:122"}, {"text": "Device memory configuration must not exceed 10 elements", "at": "cpp/device_config_visitor.cpp:716"}]},
  {"code": 83, "name": "QuadPortUnsupported", "decl": "cpp/errors.h:82", "sites": 1, "messages": [{"text": "Target device does not support quad port memories", "at": "cpp/compiler.cpp:1476"}]},
  {"code": 84, "name": "MissingDeviceConfigSchema", "decl": "cpp/errors.h:83", "sites": 1, "messages": [{"text": "Device definition schema types are missing. Check that the device configuration modules are reachable and that compiler settings have not set a base module that does not include them.", "at": "cpp/device_config_visitor.cpp:589"}]}
 ],
 "warnings": [
  {"code": 8, "name": "TooManyRams", "decl": "cpp/errors.h:88"},
  {"code": 10, "name": "LastWithoutTransactionSize", "decl": "cpp/errors.h:89"},
  {"code": 12, "name": "NonConstGlobal", "decl": "cpp/errors.h:90"},
  {"code": 13, "name": "FreeNonInlineFunction", "decl": "cpp/errors.h:91"}
 ]
}
```

Paths in the JSON are relative to `compiler/` in the reference tree. `sites` is
the number of `CompileError::<name>` occurrences; `messages` lists the distinct
first string literal `<<`-appended after each raise, which is the stable prefix
of the rendered message (later `<<` operands interpolate names, widths, etc.).

---

## 14. Design: the `kg-ptrace` interposer

Goal: capture the **exact** sequence of C-ABI calls the reference Haskell front
end makes into `libkanagawa-backend`, so that `kg-ptbridge` can be
differentially tested against it on the whole corpus. The interposer is a shim
shared library that exports every backend symbol, logs the call, forwards to the
real implementation, logs the result, and returns it.

This section was written as a design proposal, before the code; only the facts
about the reference tree it relies on are cited.

> **As built (2026-10-11).** The interposer is `crates/kg-ptrace`, installed
> by rename-and-shim (§14.4) in a directory `cargo xtask ptrace` lays out; the
> trace format is `crates/kg-pttrace`, shared with the bridge's trace backend
> so the two writers cannot drift; `cargo xtask ptdiff` compares them. Two
> departures from §14.1: the `Options` argument renders as `<opts>`, not a
> hash of the struct (its padding and three fields are indeterminate, §3.2),
> and `#options` lists the fields by name. Where this section and those
> crates' `lib.rs` differ, the crates are authoritative.

### 14.1 Trace line format

One line per call, ASCII, newline-terminated, written to the file named by
`KG_PTRACE_OUT` (default: stderr). Field separator is a single space; the format
is deliberately fixed-arity-per-function so it can be parsed with `split`.

```
<seq> <depth> <fn> <arg>* -> <ret>
```

* `<seq>` — zero-padded 8-digit decimal, monotonic from 0, per process.
* `<depth>` — call depth *at entry*: `0` for a call from outside the library,
  `1+` for backend-internal recursion (§5). Rendered as a single decimal digit.
* `<fn>` — the exact C symbol name.
* `<ret>` — rendered by the same rules as arguments; `-` for `void`.

Argument rendering is **type-directed**, derived mechanically from the bindgen
AST:

| C type | rendering | example |
|---|---|---|
| `ParseTreeNodePtr` | `n<id>` where `<id>` is the logical id (§14.2); `NULL` → `_` | `n1742`, `_` |
| `const char*` | C-escaped double-quoted string; `NULL` → `_` | `"make_capture3"`, `"@data@closure"` |
| `ParseNamespaceScopePtr` / `ParseQualifiedNamePtr` | `[` + comma-separated escaped strings in **array order** + `]`; `NULL` → `_` | `["@data@closure@core","Closure"]` |
| `unsigned int` / `int` / enum | decimal, signed rendering for `int` | `32`, `5` |
| `BOOL` | decimal (`0`/`1`) | `1` |
| `float` | `%.9g` (round-trippable for `f32`) | `1.5` |
| `const Location*` | `@<file>:<bl>:<bc>-<el>:<ec>:<valid>` ; `NULL` → `_` | `@3:12:5-12:19:1` |
| `Location` (by value, `SetLocation` only) | same as above without the `@`-deref distinction | |
| `const Options*` | `<opts:` + 64-bit FNV-1a of the 520 struct bytes + `>` plus a one-time full dump on a `#options` line | `<opts:9a3f…>` |

Two out-of-band line kinds, both prefixed `#` so they never collide with a
symbol name:

```
#version 1 <libpath> <abi-hash>
#options <json>                       ; emitted once, on the InitCompiler call
#epoch <n>                            ; emitted on entry to Codegen and on each internal Reset boundary we can infer
```

`<abi-hash>` is a hash over the generated bindgen signatures, so a trace can be
rejected if the header changed.

Rationale for including `<depth>`: §5 proves the boundary carries
backend-internal recursion that the front end never issued. A consumer that
wants "what the front end did" filters to `depth == 0`; a consumer validating
the backend contract keeps everything.

### 14.2 Pointers → monotonic logical ids

`ParseTreeNodePtr` values are heap addresses and are unstable across runs
(ASLR, allocator state). They are also **recycled**: `Compiler::Reset` deletes
every node above `_permanentNodeCount` and frees
`_temporaryParseTreeCleanupList` (`compiler/cpp/compiler.cpp:2025-2030`), so the
same address can name a different node in a later pass.

Scheme:

1. A process-global `HashMap<*const c_void, u64>` plus a `u64` counter, both
   behind a `Mutex` (the backend runs single-threaded for parse-tree
   construction — `MLIRContext::Threading::DISABLED`,
   `compiler/cpp/compiler.cpp:34` — but `Codegen` uses a thread pool
   (`compiler/cpp/thread_pool.cpp`), so the lock is not optional).
2. Ids are assigned **on first sighting**, whether the pointer appears as a
   return value or as an argument. Assigning on arguments too is required
   because `ParseFunction`'s implicit-return path (`parse_tree.cpp:5913`) can
   surface a node the trace has not seen returned.
3. Ids are **never reused**. On `#epoch` the map is cleared but the counter is
   not reset, so a recycled address gets a fresh id and cannot be confused with
   the dead node.
4. `NULL` is rendered `_` and never enters the map.

Epoch boundaries the interposer can observe: entry to `Codegen`. `Compiler::Reset`
is *not* observable (it is not an exported symbol), so ids inside `Codegen` are
epoch-1-forever and can in principle alias across the N+1 passes. This is
acceptable because **`kg-ptbridge` only needs to reproduce the pre-`Codegen`
window** (§5.4). The design therefore treats `depth == 0 && seq < seq(Codegen)`
as the sound comparison region and marks everything after as best-effort.

### 14.3 Generating the forwarders from the bindgen AST in `build.rs`

The count is not hard-coded — it is whatever `parse_tree.h` + `internal_tests.h`
contain (85 + 1 today, §1).

```
kgrust/crates/kg-backend-sys/
  build.rs
  wrapper.h                 // #include "parse_tree.h"  #include "internal_tests.h"
kgrust/tools/ptrace/         // built as crates/kg-ptrace
  build.rs                  // reuses the same bindgen invocation
  src/lib.rs                // include!(concat!(env!("OUT_DIR"), "/forwarders.rs"))
```

> **As built, since 2026-10-10 (sprint W8.4).** Only the interposer
> (`crates/kg-ptrace`) generates in `build.rs`. The configuration is
> `kg-abi-gen`'s (the wrapper restates `internal_tests.h`'s one
> declaration, which does not parse as C); `kg-backend-sys`'s bindings and
> `kg-ptbridge`'s `Backend` trait are checked-in sources that `cargo xtask
> codegen` writes through it and `--check` compares, so the tools build
> without the reference or libclang. Step 3's deny-list is
> `kg_abi_gen::UNDEFINED_SYMBOLS`; it is checked against the real library
> by `kg-backend-sys/tests/smoke.rs`, and `Backend::load` refuses a library
> lacking any other declared function.

`build.rs` steps:

1. Run `bindgen` over `wrapper.h` with
   `--allowlist-file '.*/(parse_tree|options|internal_tests)\.h'`,
   `--default-enum-style=moduleconsts`, `--no-layout-tests` off (keep layout
   tests — they are the cheapest guard against §3 drift).
2. Parse the generated Rust with `syn` into a `syn::File`; collect every
   `syn::ForeignItemFn` inside every `syn::ItemForeignMod` whose ABI is `"C"`.
   That is the authoritative forwarder list; emit `pub const N_FORWARDERS: usize`
   from `list.len()` and fail the build if it changes without an explicit bump
   of an `EXPECTED_FORWARDERS` constant (today: 86).
3. **Filter out symbols with no definition.** `ParseStages`
   (`compiler/cpp/parse_tree.h:216`) is declared but never defined (§1); a
   forwarder that resolves it eagerly will fail to load. Maintain an explicit
   `UNDEFINED_SYMBOLS: &[&str] = &["ParseStages"]` deny-list and assert it is
   still accurate by `nm`-ing the real library in `build.rs` when it is present.
4. For each remaining `ForeignItemFn`, emit:

```rust
#[no_mangle]
pub unsafe extern "C" fn ParseDeclare(
    a0: ParseTreeNodePtr, a1: ParseTreeNodePtr, a2: ParseTreeNodePtr,
    a3: ParseTreeNodePtr, a4: ::std::os::raw::c_uint, a5: ParseNamespaceScopePtr,
) -> ParseTreeNodePtr {
    let _g = Depth::enter();
    let real: unsafe extern "C" fn(_,_,_,_,_,_) -> _ =
        ::std::mem::transmute(resolve(b"ParseDeclare\0"));
    let r = real(a0, a1, a2, a3, a4, a5);
    trace(_g.depth(), "ParseDeclare",
          &[Arg::node(a0), Arg::node(a1), Arg::node(a2), Arg::node(a3),
            Arg::uint(a4 as u64), Arg::scope(a5)],
          Ret::node(r));
    r
}
```

  The `Arg::*` constructor per parameter is chosen by matching on the parameter's
  `syn::Type`, using this table (an exhaustive match — an unknown type is a hard
  build error, never a silent `Arg::opaque`):

| bindgen type | constructor |
|---|---|
| `ParseTreeNodePtr` | `Arg::node` |
| `*const ::std::os::raw::c_char` | `Arg::cstr` |
| `*const *const ::std::os::raw::c_char` (i.e. `ParseNamespaceScopePtr`, `ParseQualifiedNamePtr`) | `Arg::scope` |
| `::std::os::raw::c_uint` | `Arg::uint` |
| `::std::os::raw::c_int` | `Arg::int` |
| `BOOL` (alias of `c_uint`) | `Arg::uint` |
| `f32` | `Arg::f32` |
| `*const Location` | `Arg::loc_ptr` |
| `Location` | `Arg::loc` |
| `*const Options` | `Arg::options` |
| enum aliases (`ParseTreeBinaryOpType`, …) | `Arg::int` |

  Because bindgen resolves `typedef`s, `ParseNamespaceScopePtr` and
  `ParseQualifiedNamePtr` collapse to the same Rust type — that is fine, they
  render identically (§14.1).

5. **Trace before or after the real call?** After, so the return value is on the
   same line. But the *sequence number* must be taken **before**, otherwise
   nested internal calls (§5.1) get lower sequence numbers than their caller and
   the trace stops being a valid call-order log. So `Depth::enter()` allocates
   the sequence number, and `trace` uses it.

### 14.4 Platform loading

The interposer must sit between the Haskell executable and the real library.
The reference build links the exe directly against `kanagawa-backend`
(`compiler/kanagawa.cabal:56-58`) and locates it by rpath `$ORIGIN` /
`@loader_path` (`compiler/kanagawa.cabal:122-128`). Two mechanisms:

#### Primary (works on all three platforms): rename-and-shim

1. Rename the real library alongside the executable:
   * Linux: `libkanagawa-backend.so` → `libkanagawa-backend-real.so`
   * macOS: `libkanagawa-backend.dylib` → `libkanagawa-backend-real.dylib`, then
     `install_name_tool -id @rpath/libkanagawa-backend-real.dylib <file>`
     (Mach-O records its own install name; without this the loader still asks
     for the old one)
   * Windows: `kanagawa-backend.dll` → `kanagawa-backend-real.dll`
2. Install the shim under the original name in the same directory. Because the
   rpath is `$ORIGIN`/`@loader_path`, the unmodified executable now loads the
   shim.
3. The shim resolves the real symbols lazily, once, through a `OnceLock`:
   * Linux/macOS: `dlopen("libkanagawa-backend-real.{so,dylib}", RTLD_NOW|RTLD_LOCAL)`
     then `dlsym`.
   * Windows: `LoadLibraryW(L"kanagawa-backend-real.dll")` then `GetProcAddress`.
   Resolution must be relative to the shim's own directory, not the CWD — use
   `dladdr`/`GetModuleFileNameW` on a shim symbol to find it.
4. On Windows the shim additionally needs a `.def` file (or
   `#[no_mangle] extern "system"` plus `/EXPORT:` linker args) listing all 85
   names, since Rust `cdylib` exports are otherwise fine but the *ordinals* are
   not. Generate the `.def` from the same bindgen-derived list in `build.rs`.
   Do **not** use DLL export forwarders (`EXPORTS Foo=real.Foo`) — they bypass
   the shim entirely and log nothing.

#### Secondary (Unix only, no file mutation): preloading

* **Linux**: `LD_PRELOAD=libkgptrace.so`. ELF symbol interposition applies to
  directly-linked shared libraries, so the shim's `ParseDeclare` wins.
  Forwarding uses `dlsym(RTLD_NEXT, "ParseDeclare")`, which reaches the real
  library without needing its path.
* **macOS**: `DYLD_INSERT_LIBRARIES=libkgptrace.dylib` **plus**
  `DYLD_FORCE_FLAT_NAMESPACE=1`. Mach-O's default two-level namespace records
  the defining library in each import, so without flat namespace the insert is
  ignored. Flat namespace can perturb unrelated symbol resolution in a binary
  this large (CIRCT/LLVM are statically linked into the backend), so treat this
  as a debugging convenience, not the CI path. Note also that
  `DYLD_INSERT_LIBRARIES` is stripped by SIP for protected binaries; the
  Kanagawa exe is not protected, but a Homebrew/`/usr/bin` GHC wrapper may be.
* **Windows**: no equivalent. Rename-and-shim is the only option.

#### Determinism requirements for CI

* `PD_WAIT_FOR_DEBUG` must be unset, or `Codegen` spins forever
  (`compiler/cpp/kanagawa.cpp:13-20`).
* Set `KG_PTRACE_OUT` to a file; the backend writes diagnostics to `std::cout`
  and explicitly flushes at the end of `Codegen`
  (`compiler/cpp/kanagawa.cpp:262-265`), so interleaving on a shared stream is
  not reproducible.
* Compare only the `depth == 0`, pre-`Codegen` region for
  `kg-ptbridge` equivalence (§14.2).

### 14.5 What the interposer cannot see

1. `ParseScopedIdentifierFromString` / `ParseNamedVariableFromString` — C++
   linkage, not exported (`compiler/cpp/parse_tree.h:267`, `:269`). Their
   *effects* are visible as the inner `ParseIdentifier`/`ParseBaseList`/
   `ParseScopedIdentifier`/`ParseNamedVariable` calls.
2. `SetParseLocation` RAII scopes (`compiler/cpp/parse_tree.h:4312-4321`) — the
   location changes at `compiler/cpp/ir.cpp:3500` and `:11983` are invisible, so
   node locations inside `Codegen` cannot be predicted from the trace alone.
3. `Compiler::Reset` and all pass boundaries (§12.5).
4. The `_sequenceNumber` the backend itself assigns
   (`compiler/cpp/parse_tree.cpp:1193`) — the interposer's ids are a parallel,
   not identical, numbering. They *do* agree in relative order for nodes created
   by traced calls, because both are allocated at node-construction time.

---

## 15. Open questions

Each item names the probe that would settle it. None of them block the ABI
implementation; all of them affect exact behavioural parity.

### 15.1 `Scope` ordering vs. `GetScopeSearchList`

§8 establishes mechanically that the C array is outer→inner and the C++ `Scope`
list is its reverse (`ToScope` `push_front`,
`compiler/cpp/parse_tree.h:430-438`). Registration and lookup both reverse, so
they agree. But `GetScopeSearchList`
(`compiler/cpp/parse_tree.cpp:7857-7887`) strips with `pop_back()` while its
comment says *"remove entries from implicitScope 1 by 1 ... This searches the
innermost namespace first"* (`compiler/cpp/parse_tree.cpp:7866-7869`). On an inner→outer list, `pop_back`
removes the **outermost** element, so an unqualified lookup from inside
`A::B` would try `A::B`, then `B`, then global — rather than `A::B`, `A`,
global.

This is almost never observable because a whole module path collapses to a
single `Name` (`moduleNamespace`,
`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:273-274`, §10.1), keeping
namespace scopes at depth 1.

> **Probe.** A Kanagawa source with a class nested in a class nested in a
> namespace, declaring a type at the outer-but-one level and referring to it
> unqualified from the innermost level. If it fails to resolve, `Scope` really is
> inner→outer and the search order is a latent bug that `kg-resolve` must
> reproduce. Also check the generated `*Symbols.csv` (produced at
> `compiler/cpp/kanagawa.cpp:154`) for a three-level name: if it reads
> `B::A::c` rather than `A::B::c`, the reversal is confirmed end-to-end.

### 15.2 Does `ParseDeclare`'s inner `ParseBaseList(name)` alias the caller's node?

`compiler/cpp/parse_tree.cpp:5557` builds `ParseScopedIdentifier(ParseBaseList(name))`
where `name` is the *same* `IdentifierNode` the caller also passed as the
`name` argument of the `DeclareNode` (`:5567`). So one `IdentifierNode` is
reachable from two parents.

> **Probe.** Trace a single `uint32 x = 5;` and confirm the id of the argument to
> `ParseBaseList` equals the id of the `name` argument to `ParseDeclare`. If so,
> the Rust parse tree is a DAG at this point, not a tree, and any structural
> hashing must handle it.

### 15.3 Untyped `IntegerNode`s

`IntegerNode::TypeCheck` asserts `_frontEndType`
(`compiler/cpp/parse_tree.cpp:1310`), but literals fed to `ParseIntAttribute`
(§9) never receive `SetNodeType`. They are consumed via `GetUntypedValue` and
apparently never type-checked.

> **Probe.** A debug build with `[[schedule(4)]]` on a statement, plus an
> `[[latency(3)]]` on an extern function. If neither asserts, the invariant is
> "attribute literals are never type-checked" and `kg-ptbridge` may skip
> `SetNodeType` for them; if either asserts, the front end is relying on an
> untraced code path.

### 15.4 `_valid = false` semantics

`UnknownLocation` clears only the flag and leaves the previous line/column
(`compiler/cpp/parse_tree.cpp:6281`, comment `:6278-6280`), while `SetLocation`
force-sets `_valid = true` (`:6273`). Consumers that only look at
`_beginLine`/`_fileIndex` (e.g. `Compiler::ErrorStream`,
`compiler/cpp/compiler.cpp:178`) will therefore report a *stale* location for an
unknown-location node rather than nothing.

> **Probe.** Force an error on a node created immediately after
> `UnknownLocation` (e.g. an implicit `return` in a void function that fails a
> later check) and check whether the reported line is the previous token's.

### 15.5 The plan's "87 forwarders"

Verified counts are 85 (`parse_tree.h`) and 86 (with `InternalTests`). No 87th
`extern "C"` symbol exists.

> **Probe.** `nm -gU libkanagawa-backend.dylib | grep -v '^_ZT\|^__Z'` (or
> `nm -D --defined-only libkanagawa-backend.so`) on a built library, filtered to
> non-mangled names. If it yields 87, two extra C symbols are being introduced by
> the toolchain (e.g. `_init`/`_fini`-adjacent) and should be excluded from the
> forwarder list; if it yields 85, `ParseStages` is confirmed absent from the
> binary and the deny-list of §14.3 step 3 is correct.

### 15.6 Endianness / non-LP64 targets

All layouts in §2–§3 were measured on LP64 little-endian. The reference build
supports only x86-64 and AArch64 (`compiler/cpp/CMakeLists.txt:57-66`, MSVC and
AppleClang branches).

> **Probe.** None required unless a 32-bit or big-endian target is ever added;
> at that point `size_t` shrinks to 4 and every offset in §3 changes.
