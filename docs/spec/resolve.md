# Kanagawa — Complete Specification of Modules, Imports, Scoping and Symbol Tables

> **Status, 2026-10-11.** Extracted at kanagawa `8e0a3fb` (2026-05-18); the
> reference is pinned at `98a2d8c` (`reference.lock`). The import machinery
> (`Parser/IO.hs`, `SymbolTable.hs`, `Symbols.hs`) did not change since; of the
> files cited here `compiler/hs/app/Main.hs` and `CmdArgs.hs` did — upstream
> added the `list-deps` mode (#136), which runs exactly this spec's parse and
> import resolution and prints the program's files — and their citations are
> re-derived at the pin. §11 ("Proposed Rust design") is pre-implementation
> design; where it and the code differ, `crates/kg-resolve/src/lib.rs` and
> `crates/kg-db/src/lib.rs` are authoritative.

Extracted from the reference Kanagawa HLS compiler for the purpose of a Rust rewrite.

**All paths below are relative to `third_party/kanagawa/`** unless written
absolute. Every claim carries a `path:line` citation; anything that could not be settled
from the source is marked **UNVERIFIED** or moved to *§12 Open questions*.

| File | Lines | Role |
|---|---|---|
| `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs` | 119 | file lookup, import search order, the two synthetic modules, the parse cache, cycle detection |
| `compiler/hs/lib/Language/Kanagawa/Parser/SymbolTable.hs` | 279 | mutable parser state: scope stack, module record, imports, `findSymbol` |
| `compiler/hs/lib/Language/Kanagawa/Symbols.hs` | 88 | the *post-parse* global symbol map (`getSymbols`) and its lookup rules |
| `compiler/hs/lib/Language/Kanagawa/Parser/Options.hs` | 51 | `ParseOptions` record + defaults |
| `compiler/hs/lib/Language/Kanagawa/Parser.hs` | 968 | module/import grammar, scope combinators, `::` resolution |
| `compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs` | 805 | `Name`, `QualifiedName`, `getName`/`getQualifier`/`getScope` |
| `compiler/hs/lib/Language/Kanagawa/Internal.hs` | 57 | `readFileUtf8` (UTF-8-BOM decoding, strict read) |
| `compiler/hs/app/Main.hs` | 183 | driver: `parseProgram` → `frontend` → `codegen`, `--file-list`, `list-deps` (152 lines at `8e0a3fb`) |
| `compiler/hs/lib/Language/Kanagawa/Frontend.hs` | 457 | frontend pass list, `undefinedSymbols`, `redundantNameScope` |
| `compiler/hs/lib/Language/Kanagawa/Template.hs` | 164 | template pass list, `instantiateTemplates` |
| `compiler/hs/lib/Language/Kanagawa/TypeCheck.hs` | 378 | `checkShadowDecl` (the `shadow` warning) |
| `compiler/hs/app/Options/CmdArgs.hs` | 174 | CLI flag names + defaults (163 at `8e0a3fb`) |
| `compiler/cpp/parse_tree.h` | 4322 | C++ `TypeContext` scope stack, `LookupSymbol`, `TypeCheckPass` |
| `compiler/cpp/device_config_visitor.h` / `.cpp` | 55 / 780 | device-config extraction from `@compiler@config` |
| `library/` | 124 `.k` files | ground-truth usage |
| `test/syntax/imports.k` + `test/syntax/imports/**` | 1072 + 50 files | the behavioural oracle for everything in §2 |

---

## 1. Module naming and the import search order

### 1.1 Module names

A module name is produced by `moduleName` (`Parser.hs:179-183`):

```haskell
    moduleName = choice
        [ pure <$> L.symbol cmdargsSpecialModule
        , pure <$> L.symbol optionsSpecialModule
        , dotSepName
        ] <?> "module name"
```

so exactly three forms, in this priority order:

1. the literal token `.cmdargs` (`Parser.hs:41-42`: `cmdargsSpecialModule = ".cmdargs"`),
2. the literal token `.options` (`Parser.hs:44-45`: `optionsSpecialModule = ".options"`),
3. a `dotSepName`.

`dotSepName` (`Parser/Lexer.hs:230-233`):

```haskell
dotSepName = lexeme $ try (ident' isAlphaNumOrDashOrUnderscore `sepBy1` char '.')
  where
    isAlphaNumOrDashOrUnderscore c = isAlphaNumOrUnderscore c || c == '-'
```

with `ident' f = (:) <$> (letterChar <|> char '_') <*> takeWhileP … f` (`Parser/Lexer.hs:224-225`).
Consequences, all load-bearing:

* Each dotted component must **start** with a letter or `_`, and may then contain
  ASCII alphanumerics, `_` and **`-`** (`Parser/Lexer.hs:221-233`). Dashes are legal only
  in module-name position — `name` (`Parser/Lexer.hs:235-243`) uses `ident`, which excludes `-`.
  Verified by `test/syntax/imports/with-dash.k:4` (`module with-dash`) and
  `test/syntax/imports.k:1050-1060`.
* `dotSepName` is a single `lexeme`, and `sepBy1 … (char '.')` consumes **no whitespace**
  around the dots. `import a1 .a1`, `import a1. a1` and `import a1 . a1` are all errors
  (`test/syntax/imports.k:1062-1071`, all `expected:1`).
* Module names are **not** checked against the reserved-word list (`name`'s `check` at
  `Parser/Lexer.hs:238-240` is not applied by `dotSepName`).
* `library/mini-base.k:12` is a real module whose name contains a dash: `module mini-base`.

### 1.2 Dotted name → file path

`parseImports` (`Parser.hs:194-199`):

```haskell
        let parseFileWithExt = lift . lift . lift . parseFile . addExtension (joinPath ns)
        k <- parseFileWithExt "k"
```

so the requested file name is `joinPath ns <.> "k"`:

| module name | `ns` | requested file |
|---|---|---|
| `a1.b1.c1` | `["a1","b1","c1"]` | `a1/b1/c1.k` |
| `with-dash` | `["with-dash"]` | `with-dash.k` |
| `.cmdargs` | `[".cmdargs"]` | `.cmdargs.k` |
| `.options` | `[".options"]` | `.options.k` |

`joinPath` uses the host path separator; the extension is *always* `k`, hardcoded at
`Parser.hs:199`. `addExtension ".cmdargs" "k" == ".cmdargs.k"` (filepath's `addExtension`
prepends `.` because the argument does not already start with an extension separator).

### 1.3 The EXACT search order — verified

`Parser/IO.hs:100-110`:

```haskell
    readFileContent dir file
        | file == cmdargsSpecialModule <.> "k" = return $ Right (file, dir, buildCmdargsSpecialModule opt)
        | file == optionsSpecialModule <.> "k" = return $ Right (file, dir, buildOptionsSpecialModule opt)
    readFileContent dir file = findFile (dir:importDirs) file >>= \case
        Just path' -> do
            canonicalPath <- canonicalizePath path'
            let displaySomeException = Ex.displayException :: Ex.SomeException -> String
            bimap displaySomeException (canonicalPath, takeDirectory path',) <$> Ex.try (readFileUtf8 path')
        Nothing -> return $ Left ("Can't find file: " ++ file ++ " in any of the import directories\n" ++ unlines importDirs)

    importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++ importDir opt
```

**The plan's claim is CONFIRMED, with one refinement that matters.** The candidate
directory list, in order, is:

```
[ dir_of_importing_file
, D1/device/<target-device>
, D2/device/<target-device>
, …
, Dn/device/<target-device>
, D1
, D2
, …
, Dn
]
```

where `D1..Dn` are the `--import-dir` values in command-line order. It is **not**
interleaved: *every* `-I` directory's `device/<target>` subdirectory is tried before *any*
plain `-I` directory. `System.Directory.findFile` returns the first `d </> file` that
exists, so the first hit wins.

Additional exact behaviours of this function:

* Two short-circuits precede the search entirely: the literal names `.cmdargs.k` and
  `.options.k` never touch the filesystem (`IO.hs:101-102`). Their "path" is the literal
  string and their "directory" is the *importing* file's directory.
* `dir` for a top-level file is `getCurrentDirectory` (`IO.hs:81-82`); for an imported file
  it is `takeDirectory path'`, i.e. the directory of the **found** path as joined, *not*
  canonicalised (`IO.hs:107`). If `-I ../library` was given, nested imports of that module
  will search `../library` — a relative path — first.
* If `file` is absolute, `d </> file == file` for every `d` (filepath's `</>` returns the
  second argument when it is absolute), so an absolute `--base-library` resolves on the
  first probe. Used in practice: `test/syntax/CMakeLists.txt:43`
  (`--base-library=${CMAKE_SOURCE_DIR}/library/mini-base.k`).
* `--base-library` may also be a *relative* multi-segment path resolved through the same
  search list: `test/syntax/CMakeLists.txt:91` uses `--base-library helper/custom_base.k`
  with `-I <test/syntax>` (`CMakeLists.txt:59-60`).
* The read is `readFileUtf8` (`Internal.hs:24-32`): `hSetEncoding h utf8_bom`, then the
  whole string is forced with `evaluate (length s)` so I/O errors surface here.
  **A UTF-8 BOM is stripped; the file must be valid UTF-8** — a binary file yields a decode
  exception rendered by `Ex.displayException` (`test/syntax/imports/binary.k`,
  `test/syntax/imports.k:15-17`, `expected:1`).
* The "not found" message is exactly
  `"Can't find file: " ++ file ++ " in any of the import directories\n" ++ unlines importDirs`
  — note it lists `importDirs`, i.e. it **omits** the importing file's own directory.
* `--target-device` defaults to `"mock"` (`Parser/Options.hs:47`, `app/Options/CmdArgs.hs:87`).
* The real device tree is `library/device/<dev>/…`; the seven devices present are
  `agilex-7-f-023`, `agilex-7-i-041`, `arria-10-gx-1150`, `mock`, `stratix-10-gx-2800`,
  `stratix-10-nx-2100`, `ultrascaleplus-xcu250` (directory listing of `library/device/`).
  Each provides exactly `hardware/config.k`, `hardware/dsp.k`, `numeric/float32/operator.k`.
* Documentation agreement: `doc/programming-guide.md:4298-4339` describes the same rule
  ("When the compiler is unable to locate a module in a directory specific to a device, it
  will then search for it in the designated import directories").

### 1.4 Module-name / path agreement is enforced

`Parser.hs:200-206`:

```haskell
        case k of
            Left err -> parseFileFailure offset err
            Right result -> either (importFailure offset) (updateSymbolTable offset ns as) result
      where
        updateSymbolTable offset ns as (_, t)
            | moduleNamespace ns == namespace (module_ t) = updateImports offset as t
            | otherwise = parseFileFailure offset $ "Imported module name does not match, expected " ++ intercalate "." ns
```

So after a successful parse the imported file's declared module namespace must equal
`moduleNamespace ns`. Oracle: `test/syntax/imports/a1/a2.k:4` declares `module a1.a3` while
living at `a1/a2.k`; `import a1.a2` is `expected:1` (`test/syntax/imports.k:270-272`).

A file with **no** `module` declaration has `namespace = ""` (`SymbolTable.hs:94`), which
never equals `moduleNamespace ns` for a non-empty `ns`, so **every importable file must
carry a `module` declaration**. Top-level files named on the command line are parsed by
`parseFile` directly (`IO.hs:70`) and are *not* subject to this check — they may omit the
module declaration (which is how every `test/syntax/*.k` case works).

### 1.5 Namespace mangling

`Parser/Syntax.hs:273-274`:

```haskell
moduleNamespace :: [String] -> Name
moduleNamespace = fromString . concatMap ('@':)
```

so `["a1","b1"] → "@a1@b1"`, `["mini-base"] → "@mini-base"`, `[".cmdargs"] → "@.cmdargs"`.
The inverse used for diagnostics is `unmangleModuleNamespace` (`Parser/Syntax.hs:276-279`):
strip the leading `@`, then map every remaining `@` to `.`.

`getName (ModuleIdentifierF n) = fromName $ moduleNamespace n` (`Parser/Syntax.hs:743`) and
`getQualifier ModuleIdentifierF{} = []` (`Parser/Syntax.hs:763`), so a module identifier's
`getQualifiedName` is the single-element list `["@a1@b1"]`.

The mangled namespace is what reaches the C++ side: `ModuleDecl a _ b -> parseNamespace a b`
(`compiler/hs/app/ParseTree.hs:128`) and `ModuleIdentifier a -> withName (moduleNamespace a)
parseIdentifier` (`app/ParseTree.hs:130`). That is why C++ hardcodes
`Scope _configScope = {"@compiler@config"}` (`compiler/cpp/device_config_visitor.h:46`) and
`Scope _schemaScope = {"@compiler@device@schema"}` (`device_config_visitor.cpp:78`, `:583`).

---

## 2. `module`, `import`, exports, re-exports

### 2.1 Grammar

Whole-file production (`Parser.hs:108`):

```haskell
program parseFile = (moduleDecl <|> declarations) >>= checkExposed >>= checkRedundantImports
```

Module declaration (`Parser.hs:185-192`):

```haskell
    moduleDecl = do
        ident <- moduleIdentifier
        exposed <- braces ((moduleDiff <|> moduleIdentifier <|> notedWithDocs identifier) `MP.sepBy` comma)
        updateModule ident exposed
        noted ((Seq . pure) . ModuleDecl ident exposed <$> scoped ident declarations)
      where
        moduleIdentifier = try $ notedWithDocs (ModuleIdentifier <$ keyword "module" <*> moduleName <?> "module specifier")
        moduleDiff = try $ notedWithDocs (ModuleDiff <$ keyword "module" <*> moduleName <* L.symbol "\\" <*> moduleName <?> "module difference")
```

Imports (`Parser.hs:194-206`), and the fact that they must all precede every declaration
(`Parser.hs:243`, `declarations = parseImports >> many decl`).

**Complete concrete syntax** (no other forms exist):

```
module-decl   ::= 'module' module-name '{' [ export-item { ',' export-item } ] '}'
export-item   ::= identifier                                   -- expose one symbol
                | 'module' module-name                         -- re-expose a whole module
                | 'module' module-name '\' module-name         -- re-expose a module difference
import-decl   ::= 'import' module-name [ 'as' identifier ]
file          ::= [ module-decl-header ] import-decl* declaration*
```

* `sepBy` (not `sepBy1`) at `Parser.hs:187` ⇒ `module foo {}` is legal
  (`test/syntax/imports/a1/a9.k:4`, `library/mini-base.k` uses a non-empty list;
  `test/syntax/imports/export.k:4` uses `module export {}`).
* Trailing comma is **not** allowed (`sepBy` with `comma` as separator).
* The three export-item alternatives are tried in the order shown, each of the first two
  wrapped in `try`. `module a1.a21 {module}` (a bare `module` with no name) is a parse error
  (`test/syntax/imports/a1/a21.k:4`, `test/syntax/imports.k:668-670`, `expected:1`).
* Export items may be in any order and may mix kinds:
  `module a1.a25 {D, module a1.a1, C}` (`test/syntax/imports/a1/a25.k:4`).
* `as` takes a plain `name` (`Parser.hs:197`): a single, non-reserved, non-dashed identifier.
* `import` is only reachable from `declarations` (`Parser.hs:243`), which occurs at the top
  of a file and at the top of a module body. **There is no import inside a class, function,
  namespace or `static if`.**
* Doc comments attach to `moduleIdentifier`, `moduleDiff` and plain export identifiers via
  `notedWithDocs` (`Parser.hs:187,191,192`).

### 2.2 What `updateModule` records

`SymbolTable.hs:114-133`:

```haskell
updateModule (ModuleIdentifier n) exposed = modify $ \x -> x
    { module_ = (module_ x)
                    { namespace = moduleNamespace n
                    , exposedModules = Set.fromList $ mapMaybe exposedModule exposed
                    , exposedSymbols = Map.fromList $ mapMaybe exposedSymbol exposed
                    , exposedDiffs = mapMaybe exposedDiff exposed
                    }
    }
```

Three disjoint buckets: `exposedModules :: Set Name` (mangled namespaces),
`exposedSymbols :: Map Name ExpSrc` (plain identifiers), `exposedDiffs :: [(Name,Name)]`
(pairs of mangled namespaces). Note `updateModule` runs **before** the body is parsed
(`Parser.hs:188` precedes `Parser.hs:189`), so `exposedModules` is already populated when
imports are processed — which `updateImports` relies on (§2.6).

### 2.3 Validation of the export list — `checkExposed`

`Parser.hs:208-231`, run after the whole file body has been parsed:

```haskell
    checkExposed a = do
        let namespaceImported = namespace . importedModule
        Module{..} <- gets module_
        mapM_ (checkSymbols namespace) exposedSymbols
        let moduleNamespaces = namespace : map namespaceImported imports
        mapM_ (checkModules moduleNamespaces) exposedModules
        mapM_ (checkDiffs moduleNamespaces) exposedDiffs
        modify updateVisible
        return a
      where
        checkSymbols ns x = do
            s <- findSymbol $ copyNote (ScopedNameF [ns] Nothing x) x
            case s of
               Just (Variable _ _ t _ _ _) | not (isConst t)
                       -> registerSymbolError x "global, mutable state can't be exposed outside a module"
               Nothing -> registerSymbolError x "undefined symbol"
               _       -> return ()
        checkModules ns x
            | x `elem` ns = return ()
            | otherwise = registerFancyError 0 $ "Missing import for re-exposed module " ++ toString (unmangleModuleNamespace x)

        checkDiffs ns (x, y)
            | y `elem` ns = checkModules ns x
            | otherwise = registerFancyError 0 $ "Missing import for module used in re-exposed module difference " ++ toString (unmangleModuleNamespace y)
```

Rules, each with an oracle:

| rule | error text | oracle |
|---|---|---|
| a named export must resolve (own module or an import) | `` `X`: undefined symbol `` | `test/syntax/imports/undefined.k` + `imports.k:413-415`; `imports.k:580-585` (`module foo { x }` with no `x` at all); `imports.k:587-597` (`x` declared only *inside a function* — module-scope lookup does not descend, `expected:1`); `imports.k:599-606` (`const uint32 x = 10;` at module scope ⇒ `expected:0`) |
| a named export may not be a mutable global | `` `X`: global, mutable state can't be exposed outside a module `` (`Parser.hs:221-222`) | **no test in the tree exercises this branch** — see *Open questions* §12.8 |
| `module M` must be the module itself or a **directly** imported module | `Missing import for re-exposed module M` | `test/syntax/imports/a1/a20.k` (exposes `module a1.a1`, imports only `a1.a19`) + `imports.k:664-666` |
| `module A \ B` requires `B` present, then applies the `module A` rule to `A` | `Missing import for module used in re-exposed module difference B` | `test/syntax/imports/diff/self_missing.k` + `imports.k:908-910` |

`checkSymbols` resolves against the *fully parsed* module, so an export list may name a
symbol declared **later in the file** — this is the entire content of the
`declaration-order` test (`test/syntax/helper/declaration_order.k:4-8` exposes `Foo` at
line 5, defines it at line 8; `test/syntax/declaration-order.k:4-5` uses it,
`expected:0`; registered at `test/syntax/CMakeLists.txt:95`).

All three checks use `registerParseError`-style *deferred* errors (`Error.hs:130-137`), so
parsing continues and `updateVisible` still runs.

### 2.4 `updateVisible` — the complete re-export algebra

`SymbolTable.hs:135-181` (quoted in full because every clause matters):

```haskell
updateVisible t@SymbolTable{..} = t { module_ = update module_ }
  where
    symbolName = head . tail

    visibleImported = visibleSymbols . importedModule

    namespaceImported = namespace . importedModule

    difference a b = Map.map (HashMap.filterWithKey
        (\k _ -> not $ getAny $ Map.foldMapWithKey
            (\namespace symbols -> Any ([namespace, symbolName k] `HashMap.member` symbols)) b)) a

    update m@Module{..} = m { visibleSymbols = Map.unionsWith HashMap.union [exposed, reExposed, diffs]
                            , moduleNeeded = if any exported symbols then Needed else Redundant
                            }
      where
        diffs = Map.unionsWith HashMap.union $ map diff exposedDiffs
        diff (n1, n2) = difference (moduleSymbols n1) (moduleSymbols n2)
        moduleSymbols n
            | namespace == n = Map.singleton namespace $ HashMap.filter exposable symbols
            | otherwise      = visibleImported $ fromJust $ find ((n ==) . namespaceImported) imports

        filterImported i@Import{..}
            | namespaceImported i `Set.member` exposedModules = visibleImported i
            | isJust importedAs                               = mempty
            | otherwise                                       = Map.map filterExposed $ visibleImported i

        filterExposed = HashMap.filterWithKey $
            \k _ -> symbolName k `Map.member` exposedSymbols

        reExposed = Map.filter (not . HashMap.null) $ Map.unionsWith HashMap.union $ map filterImported imports

        exposable (Variable _ _ typ _ _ _) = isConst typ
        exposable _ = True

        exposed
            | namespace `Set.member` exposedModules = Map.singleton namespace $ HashMap.filter exposable symbols
            | otherwise                             = Map.singleton namespace $ filterExposed symbols

        exported ExportType{} = True
        exported (Function _ _ attr _ _ _ _ _)
            | any reset $ unfix attr  = True
        exported _ = False

        reset (FlagAttr Reset) = True
        reset _                = False
```

Key types: `visibleSymbols :: Map Name Symbols` — a map from *namespace* to a
`HashMap QualifiedName ExpSrc` (`SymbolTable.hs:74`, `Symbols.hs:26`). A module-scoped
symbol's key is `[namespace, name]` because the module body is parsed inside
`scoped ident` (`Parser.hs:189`) whose qualifier is `["@a@b"]`. Hence
`symbolName = head . tail` (`SymbolTable.hs:138`) extracts the *top-level* name of the
key, even for nested keys like `["@a1@a5","N","Aa"]` (→ `"N"`).

Derived semantics — enumerated exhaustively:

1. **Self-export by name** (`module M { x, y }`): only own symbols whose key's second
   component is in `exposedSymbols`. Because the filter is on the *second* component,
   exposing a class name `C` also makes every `["@M","C",…]` member key visible
   (`filterExposed`, `SymbolTable.hs:163-164`). Oracle:
   `test/syntax/imports/a1/a5.k` exposes `N`, and `imports.k:828-841` uses `A5::N` and
   calls `foo.Go<10>()` (a member of `N`) with `expected:0`.
2. **Self-export as a whole module** (`module M { module M }`): every own symbol passing
   `exposable`, i.e. everything except non-`const` `Variable`s
   (`SymbolTable.hs:168-169, 171-173`). Documented at `doc/programming-guide.md:4432-4448`.
3. **Re-export of an import by name** (`module M { x }` + `import B` where `B` exposes `x`):
   `filterImported` third clause — `B`'s `visibleSymbols`, filtered by *M*'s
   `exposedSymbols` names. Oracle: `test/syntax/imports/a1/a3.k` (`module a1.a3 { a1 }` +
   `import a1.a1`) used at `imports.k:274-281`.
4. **Re-export of a whole import** (`module M { module B }` + `import B`): `B`'s entire
   `visibleSymbols`, unfiltered (`SymbolTable.hs:159`). Oracle:
   `test/syntax/imports/a1/a16.k` + `imports.k:608-616`.
5. **Aliased imports are never implicitly re-exported.** `isJust importedAs ⇒ mempty`
   (`SymbolTable.hs:160`) — *unless* the module is explicitly listed in `exposedModules`,
   because that clause is tested first. Oracle: `test/syntax/imports/a1/a6.k` imports
   `a1.a5 as A5` and exposes only its own `a1`; `imports.k:334-342` shows `a1<bool>` is not
   reachable, `imports.k:344-353` shows it is reachable through `A5::`.
   `test/syntax/imports/a1/a19.k` imports `a1.a16` both plain and `as A` and exposes
   `module a1.a16`, so both `Import`s contribute (`imports.k:618-626`, `expected:0`).
6. **Transitivity**: re-exports chain, because `filterImported` reads the *imported
   module's already-computed* `visibleSymbols`. `a1.a4 { a1 }` ← `a1.a3 { a1 }` ← `a1.a1`
   works (`imports.k:283-289`). A **whole-module** re-export of a module that itself
   whole-module-re-exports is likewise transitive: `a1.a18 { module a1.a17 }` sees
   `a1.a17 { Aa }` which sees `a1.a16 { module a1.a1 }`, so `Aa` is visible but `a1` is not
   (`imports.k:646-662`).
7. **`Map.unionsWith HashMap.union [exposed, reExposed, diffs]`** — the three sources are
   *merged*, keyed by namespace; nothing shadows anything. A symbol re-exported from `B`
   stays under `B`'s namespace key, which is what makes `findInImport`'s namespace
   reconstruction work (§2.7).
8. **`moduleNeeded`** is `Needed` iff the module contains an `ExportType` declaration
   (`export T;`) or a `Function` carrying the `[[reset]]` flag attribute
   (`SymbolTable.hs:175-181`). This makes importing such a module never "redundant"
   (§2.9). Oracles: `test/syntax/imports/export.k` and `test/syntax/imports/reset.k`,
   both asserted `warning:0` at `imports.k:912-918`.

### 2.5 Module difference `module A \ B`

`difference a b` (`SymbolTable.hs:144-146`) removes from `a` every entry whose
`symbolName` (second key component) occurs as a **top-level** name in *any* namespace of
`b` — the membership probe builds the two-element key `[namespace, symbolName k]`.

`moduleSymbols n` (`SymbolTable.hs:154-156`) resolves a namespace to a symbol map: the
module itself (own `exposable` symbols) or the `visibleSymbols` of the directly imported
module with that namespace. **`fromJust`** here is partial; it is protected only by
`checkDiffs`, and only because Haskell laziness means `visibleSymbols` is never forced
when the module has already errored (see `test/syntax/imports/diff/self_missing.k`).

Ground truth from `test/syntax/imports/diff/` and `imports.k:861-906`:

| module | export list | asserted from importer |
|---|---|---|
| `diff.foo` | `{module diff.foo}` | `x==10, y==false, t==uint32, foo_t==uint20, foo==3` |
| `diff.bar` | `{module diff.bar}` | `x==11, y==true, t==uint20, bar_t==uint32, bar==2` |
| `diff.foo_self` | `{module diff.foo \ diff.foo_self, module diff.foo_self}` | `self==99, foo==false, x==10, y==false, bitsizeof(t)==32` |
| `diff.self_foo` | `{module diff.self_foo \ diff.foo, module diff.foo}` | `self==99, foo==3, x==10, y==false, bitsizeof(t)==32` |
| `diff.foo_bar` | `{module diff.foo \ diff.bar, module diff.bar}` | `foo==3, bar==2, x==11, y==true, bitsizeof(t)==20` |
| `diff.foo_minus_bar` | `{module diff.foo \ diff.bar}` | `foo==3`, `foo_t` usable; `x` **not** visible (`imports.k:902-906`, `expected:1`) |

Note `diff.foo_minus_bar` still exposes `foo` and `foo_t` because `diff.bar` defines
`bar`/`bar_t` (different names) but not `foo`/`foo_t`; `x`, `y`, `t` are removed because
`diff.bar` defines those names.

The intended idiom is documented at `doc/programming-guide.md:4450-4479`: a `params`
module that exposes `module .cmdargs` plus `module params \ .cmdargs`, so `--define`d
values override in-module defaults. The library uses the same trick for device config:
`library/compiler/device/config.k:7-11`

```
module compiler.device.config
    { module compiler.device.config \ hardware.config
    , module hardware.config
    }
import hardware.config
```

i.e. *device-specific values in `hardware.config` win over the defaults in
`compiler.device.config`*. This is the only module difference in the whole `library/` tree.

### 2.6 `updateImports` / `addImport`

`SymbolTable.hs:99-112`:

```haskell
updateImports offset qualifier t = modify addImport'
  where
    addImport' x@(SymbolTable _ m) = addImport needed offset qualifier x t
      where
        needed | namespace (module_ t) `Set.member` exposedModules m = Needed
               | any (namespace (module_ t) `in2`) $ exposedDiffs m  = Needed
               | otherwise                                           = moduleNeeded $ module_ t
        in2 a (b, c) = a == b || a == c

addImport needed offset qualifier x t = x
    { module_ = (module_ x) { imports = Import (module_ t) qualifier offset needed : imports (module_ x) }
    }
```

* `Import` fields (`SymbolTable.hs:81-88`): the imported `Module` record (by value — a
  snapshot), the optional alias, the byte offset of the `import` keyword (for the warning
  location), and the `Needed`/`Redundant` flag.
* `imports` is built by **cons**, so the list is in **reverse source order**, with the
  implicit base import last.
* An import is pre-marked `Needed` when the importing module re-exposes it by name, uses it
  in either side of a difference, or when the imported module is itself `Needed`.
* The same module may be imported twice (plain + aliased) producing two `Import` entries
  (`test/syntax/imports/a1/a19.k:8-9`). `doc/programming-guide.md:4278-4296` recommends this.

### 2.7 Unqualified and qualified lookup at parse time — `findSymbol`

`SymbolTable.hs:227-278`:

```haskell
findSymbol e = do
    t <- get
    sym <- findInSymbolTable e t
    case nub sym of
        [x] -> return $ Just x
        []  -> return Nothing
        xs  -> do
            registerSymbolError e $ "symbol is ambiguous. It could refer to the definition at\n\t   "
                                    ++ intercalate "\n\tor " (map symbolLoc xs)
            return Nothing
  where
    findInSymbolTable s SymbolTable{..}
        | null $ getQualifier s = findInScopes scopes >>= checkImportQualifiers
        | otherwise             = findInSymbols
      where
        checkImportQualifiers [x] = do
            case find ((Just (getName s) == ) . importedAs) $ imports module_ of
                Just Import{..} -> registerFancyError importOffset $ "The import qualifier '" ++ (getName s) ++
                                                                     "' conflicts with the name of symbol defined at "
                                                                     ++ symbolLoc x
                _ -> return ()

            return [x]
        checkImportQualifiers xs = return xs

        findInScopes (Scope{..}:xs) = case maybeToList $ HashMap.lookup (getName s) symbolsInScope of
            [x] -> (x :) <$> findInImports x
            _   -> findInScopes xs
        findInScopes _ = findInSymbols

        findInSymbols = ((maybeToList $ lookupSymbol s (symbols module_)) <>) <$> findInImports s

        findInImports x = do
            let foundImported = map (findInImport x) $ imports module_
                foundUnique = zipWith (foldl' (\\)) foundImported (inits foundImported)
            modify (\t -> t { module_ = module_ { imports = zipWith updateNeeded foundUnique (imports module_) } })
            return $ concat foundUnique
          where
            updateNeeded [] m = m
            updateNeeded _  m = m { importNeeded = Needed }

findInImport x Import{..} = mapMaybe (findExposed $ getQualifiedName x) $ Map.toList $ visibleSymbols importedModule
  where
    findExposed name (namespace, symbols) = flip lookupSymbolByName symbols =<< withNamespace name
      where
        withNamespace (n:ns)
            | n == namespace = Just $ n:ns
        withNamespace ns     = (namespace:) <$> stripPrefix (maybeToList importedAs) ns
```

**Resolution order for an unqualified name** (`getQualifier s == []`):

1. Walk the scope stack innermost→outermost. The **first** scope containing the name wins
   (`findInScopes`, `SymbolTable.hs:256-258`).
2. If found in a scope, *also* run `findInImports` on the **found declaration**, and
   concatenate. This means a locally declared symbol whose fully-qualified name collides
   with an imported one produces an **ambiguity error**, not shadowing. Oracle:
   `imports.k:75-86` — `import a1.b2.c1.c1` plus a file-scope `struct c1 {…}` ⇒ `expected:1`.
   A *nested* declaration (`class Foo { struct c1 … }`) does not collide because its
   qualified name is `["Foo","c1"]` (`imports.k:128-138`, `expected:0`).
3. If no scope contains the name, `findInSymbols`: the current module's own symbol map via
   `lookupSymbol` (scope-walking, §3.3), then all imports.
4. `checkImportQualifiers` (`SymbolTable.hs:246-254`): if resolution produced exactly one
   symbol and some import alias equals that symbol's name, register
   `The import qualifier 'X' conflicts with the name of symbol defined at <loc>` at the
   *import's* offset. Oracles: `imports.k:929-940`, `:961-974`, `:976-989`, `:991-1008`,
   `:1010-1018`, `:1020-1028` — all `expected:1`.

**Resolution order for a qualified name** (`A::B`, `getQualifier s /= []`): skip the scope
stack entirely; go straight to `findInSymbols` (`SymbolTable.hs:244`).

**`findInImport` name matching** — two ways a name can hit an import:

* `withNamespace (n:ns) | n == namespace` — the leading component already *is* the
  mangled namespace (this happens after `makeQualified` has rewritten an earlier prefix).
* otherwise strip the import alias (`stripPrefix (maybeToList importedAs) ns`) and prepend
  the namespace. With **no** alias, `stripPrefix [] ns == Just ns`, so the plain name is
  looked up under the namespace. With an alias `A5`, only names starting with `A5` match,
  and the alias is consumed. **This is exactly why an aliased import hides its symbols from
  unqualified use** (`imports.k:334-342` vs `:344-353`).

**Ambiguity dedup and `Needed` marking**: `foundUnique !! i = f_i \\ f_0 \\ … \\ f_{i-1}`
(`SymbolTable.hs:265`). Because `imports` is reverse source order, the **last** `import`
statement in the file wins the "who provided this symbol" attribution, and earlier ones
become redundant. Oracle pair:

* `import a1.a1` then `import a1.a8` (a1.a8 re-exposes a1's `a1`) using both `a1` and `a8`
  ⇒ `warning:redundant-import` (`imports.k:503-512`);
* the reverse order ⇒ no warning (`imports.k:514-523`).

`nub` then collapses *identical* `ExpSrc` values (the same declaration reached through two
re-export chains is not ambiguous); genuinely distinct declarations produce
`` `X`: symbol is ambiguous. It could refer to the definition at … or … ``. Oracles:
importing two different `c1`s without using them ⇒ `expected:0` (`imports.k:140-143`);
using one ⇒ `expected:1` (`imports.k:154-162`).

### 2.8 `::` qualified lookup during parsing

`Parser.hs:645-681`:

```haskell
    prefixedScopedNameAndSymbol scope prefix resolvedPrefix isQualified = do
        forceTemplate <- isJust <$> optional templateQualifier
        ident <- noted identifier
        let s = copyNote (ScopedNameF scope prefix ident) ident
        sym <- findSymbol $ copyNote (ScopedNameF scope resolvedPrefix ident) ident

        (prefix', resolvedPrefix', isQualified') <- case sym of
            Just e@(TemplateParam n a b c _) -> …
            Just e@(Alias _ (TypeIdentifier _ t) _) ->
                return (qualifyScope e s, t, True)
            Just e
                | forceTemplate || isTemplate e -> do
                    i <- templateInstance s e
                    return (qualifyScope e i, i, True)
                | otherwise ->
                    return (qualifyScope e s, s, True)
            Nothing
                | forceTemplate -> …
                | otherwise ->
                    return (s, s, isQualified)

        sep <- isJust <$> optional (L.symbol "::")
        if sep then prefixedScopedNameAndSymbol scope (Just prefix') (Just resolvedPrefix') isQualified'
               else return (prefix', sym)
      where
        qualifyScope e
            | not isQualified = makeQualified e
        qualifyScope _        = id
```

Semantics:

* `ScopedNameF scope prefix ident` (`Parser/Syntax.hs:362`) carries **both** the lexical
  scope in which the name appeared (`scope`, from `getScopeQualifier`) *and* the explicit
  `::` prefix chain. `getScope (ScopedNameF n _ _) = n` and
  `getQualifier (ScopedNameF _ (Just a) _) = getQualifier a ++ [getName a]`
  (`Parser/Syntax.hs:767`, `:758`).
* Each `::` step re-enters with the previous component as prefix; resolution is left-to-right.
* Two parallel prefixes are threaded: `prefix'` (what ends up in the AST) and
  `resolvedPrefix'` (what is used for the next `findSymbol`). They differ for type aliases
  (`Alias _ (TypeIdentifier _ t) _` resolves *through* the alias — `Parser.hs:663-664`) so
  `A::B` where `A` is an alias to a class resolves `B` in the class.
* `makeQualified` (`Desugar.hs:190-200`) rewrites the AST node so its prefix chain becomes
  the *declaration's* fully-qualified path — but only when the user did not already write a
  qualifier (`isQualified == False`). This is the mechanism that turns a bare `foo` into
  `@some@module::foo` in the emitted tree.
* `templateQualifier` is the `template` keyword used as a disambiguator, matched only when
  not followed by `<` (`Parser.hs:646`).
* `templateInstance` (`Parser.hs:676-681`) attaches `<args>` if present; a *function*
  template with no explicit args still becomes `TemplateInstanceF s (Seq [])`.

Nested-class access is thus `Outer::Inner::member`, and library code uses the alias form
`A5::N`, `opt::Backend::Sv` (`test/syntax/options.k:14`).

### 2.9 The `redundant-import` warning — exactly what it fires on

Generation (`Parser.hs:234-241`):

```haskell
    checkRedundantImports (ModuleDecl _ _ a) = checkRedundantImports a
    checkRedundantImports x@(Seq a) = do
        Module{..} <- gets module_
        return $ copyNote (SeqF $ foldr redundantWarning (copyNote (SeqF []) x) imports : a) x
      where
        redundantWarning (Import _ _ offset Redundant) = warning offset RedundantImport "The import is redundant"
        redundantWarning _ = id
```

Message text (`Error.hs:142-145`):
`warning: The import is redundant [--Wno-redundant-import]`.

An `import` warns **iff** its `Import.importNeeded` is still `Redundant` at end of file,
i.e. **all** of the following hold:

1. the importing module does not list it in `exposedModules` (`SymbolTable.hs:104`),
2. it does not appear on either side of an `exposedDiffs` pair (`SymbolTable.hs:105`),
3. the imported module contains no `export T;` and no `[[reset]]` function
   (`SymbolTable.hs:106` via `moduleNeeded`, computed at `SymbolTable.hs:149,175-181`),
4. no name resolved through it — *and* was not already attributed to a later-in-source
   import (`SymbolTable.hs:263-270`, §2.7).

Note: the implicit base import is added with `Needed` (`IO.hs:77`), so `base`/`.cmdargs`
never warn. The warning is enabled by default (`app/Options/CmdArgs.hs:113-116`) and
controlled by `--Wredundant-import` / `--Wno-redundant-import`
(`app/Options/CmdArgs.hs:127`, `:134`; switch names from `Warning.hs:34,40-44`).

Complete oracle set in `test/syntax/imports.k`:
`:426-428` (unused ⇒ warn), `:430-437` (used ⇒ no warn), `:439-442` (re-exposed ⇒ no warn),
`:444-446` (transitive module unused ⇒ warn), `:457-464` (`a1.a6` — aliased inner import ⇒
warn), `:466-473` (`a1.a7` imports two modules that both provide `a1` ⇒ warn),
`:475-483`/`:485-501` (`a1.a3`+`a1.a4` — only one attribution ⇒ warn),
`:503-512`/`:514-523` (order-dependence), `:697-699`, `:726-734`, `:823-826`.

`test/syntax/CMakeLists.txt:42` disables it globally for all other syntax tests
(`--Wno-redundant-import`), and re-enables it for `options` and `imports`
(`CMakeLists.txt:93`, `:123`).

---

## 3. `getSymbols` — the post-parse global symbol map

### 3.1 What it builds

`Symbols.hs:26,37-71`:

```haskell
type SymbolMap n e = HashMap QualifiedName (NotedExp n e)

getSymbols = getSymbols' AllSymbols
getSymbols' config = HashMap.unions . parallelChunksOf 6 HashMap.unions . parallel (getSymbols'' config)

getSymbols'' config = HashMap.fromList . reverse . para update
  where
    update e@Template{}
        | TemplateDecl == config  = symbol e . const mempty
    update e@Struct{}             = symbol e . const mempty
    update e@Union{}              = symbol e . const mempty
    update e                      = symbol e . concat

    symbol e@Alias{}              = insert e
    symbol e@Capture{}            = insert e
    symbol e@Struct{}             = insert e
    symbol e@Union{}              = insert e
    symbol  (Template _ Typename) = id
    symbol e@Template{}
        | NoTemplate == config    = tail
        | otherwise               = insert e
    symbol e@Function{}           = insert e
    symbol e@FunctionDecl{}       = insert e
    symbol e@Enum{}               = insert e
    symbol e@EnumConstant{}       = insert e
    symbol e@Variable{}           = insert e
    symbol e@FuncParam{}          = insert e
    symbol e@Class{}              = insert e . insert' e [getName e, thisName]
    symbol e@Error{}              = insert' e errorsName
    symbol _                      = id

    insert' e n = ((getScope e ++ n, e) :)
    insert e    = insert' e [getName e]
```

* **Key type**: `QualifiedName = [Name]` (`Parser/Syntax.hs:69`), where `Name` is a
  `Hashed String` newtype (`Parser/Syntax.hs:51-52`). The key is
  `getScope e ++ [getName e]` — the declaration's *own* `declScopeName` field plus its name.
  `getScope` for every declaration form falls through to `declScopeName`
  (`Parser/Syntax.hs:780`).
* **Complete list of node kinds that produce a symbol**: `Alias`, `Capture`, `Struct`,
  `Union`, `Template` (unless it is `Template _ Typename`, and unless config is
  `NoTemplate`), `Function`, `FunctionDecl`, `Enum`, `EnumConstant`, `Variable`,
  `FuncParam`, `Class`, `Error`. Nothing else (`Symbols.hs:52-68`).
* **`Class` inserts two keys**: `scope ++ [name]` and `scope ++ [name, "$this"]`
  (`Symbols.hs:66`; `thisName = "$this"` at `Parser/Syntax.hs:267-268`). The second key
  backs `infer x@ThisF{}` (`Type.hs:1269-1272`).
* **`Error` is keyed `getScope e ++ ["$errors"]`**, and `getScope ErrorF{} = []`
  (`Parser/Syntax.hs:778`), so *all* error nodes collapse to the single key `["$errors"]`.
  `hasErrorSymbol = HashMap.member ["$errors"]` (`Symbols.hs:31-35`) is the pipeline's
  "did anything fail" flag.
* **Struct and union bodies are NOT descended into** (`Symbols.hs:48-49`,
  `update e@Struct{} = symbol e . const mempty`). Struct/union *fields* never appear in the
  symbol map; field access is resolved through the *type* (`tField`, `Type.hs:1236`).
  Class bodies **are** descended into.
* **`SymbolsConfig`** (`Symbols.hs:28`) has exactly three values:
  * `AllSymbols` — everything;
  * `NoTemplate` — a `Template` node contributes `tail`, i.e. it *removes* the first entry
    its child produced (the template's inner declaration's own key) and does not add its
    own — templates become invisible;
  * `TemplateDecl` — a `Template` node contributes only its own key and its body is not
    traversed (used by `inferType`, `Type/Inference.hs:37`).
* **Duplicate-key precedence**: `HashMap.fromList` is last-wins, and the list is
  `reverse`d, so **the first entry in `para` order wins**. `symbol e` conses the parent's
  own entry *before* the concatenated children (`Symbols.hs:44,50`), so a parent beats its
  descendants. `HashMap.unions` is left-biased, and `parallelChunksOf 6` preserves order,
  so **the earlier AST in the merged program list wins** (`Symbols.hs:41`).
* The merged program order is the order in which files finished parsing
  (`IO.hs:96`, `modify (++ [(canonicalPath, result)])`), i.e. a **post-order DFS of the
  import graph** with the base library first, then `Main.hs:70,80`
  (`exprs = map fst $ rights results`, `foldr1 append`).

### 3.2 It is rebuilt from scratch many times — VERIFIED

`withSymbols`/`withSymbols'` is defined twice and called **12 times**:

| # | site | pass |
|---|---|---|
| def | `Frontend.hs:40` | `withSymbols fn p = fn (getSymbols $ fst p) p` |
| 1 | `Frontend.hs:55` | `validateProgram` |
| 2 | `Frontend.hs:59` | `postDesugar` |
| 3 | `Frontend.hs:61` | `validateExtern` |
| 4 | `Frontend.hs:62` | `validateExportableClasses` |
| 5 | `Frontend.hs:63` | `unresolvedTemplates` |
| 6 | `Frontend.hs:66` | `undefinedSymbols` |
| 7 | `Frontend.hs:71` | `deduceTemplateArgs` |
| 8 | `Frontend.hs:73` | `higherOrderFunctions` |
| 9 | `Frontend.hs:74` | `validateTypedLiterals ∘ … ∘ redundantNameScope` |
| def | `Template.hs:66` | `withSymbols fn p = fn (getSymbols p) p` |
| def | `Template.hs:68` | `withSymbols' config fn p = fn (getSymbols' config p) p` |
| 10 | `Template.hs:71` | `templateTypeCheck` |
| 11 | `Template.hs:73` | `deduceTemplateArgs` |
| 12 | `Template.hs:79` | `addLambdaWrappers` (`withSymbols' NoTemplate`) |

**The plan's "12+ times" claim is exact for `withSymbols` and is a floor for the real
count.** Additional whole-program `getSymbols*` builds:

| site | when |
|---|---|
| `Frontend.hs:50` | `checkPar` — one extra build to test `hasErrorSymbol`, used by pass #9 |
| `Frontend.hs:158` | `removeRedundantNameScope`, used only by `--pretty` mode (`Main.hs:52`) |
| `Template.hs:83` | `symbols = getSymbols program'` for `decidedInstances`/`hasErrors'` |
| `Type/Inference.hs:37` | `getSymbols' TemplateDecl` **inside `inferType`'s fixpoint loop** — at least two builds per `inferType` call; `inferType` is invoked at `Frontend.hs:69`, `Template.hs:72`, `Template.hs:75`, and `Template.hs:58` (the recursion) |
| `app/ParseTree.hs:43` | `getSymbols'' AllSymbols p` in the FFI bridge |
| `Function.hs:33` | `getSymbols'' AllSymbols $ body parent` — **per function / nested scope / class node** during `moveFunctions` |

So a minimal single-iteration compile does ≥ 12 + 1 + 1 + 1 (ParseTree) = 15 whole-program
rebuilds plus ≥ 8 `TemplateDecl` builds plus one local build per function.
`instantiateTemplates` recurses (`Template.hs:58`), re-running its 3 `withSymbols` passes,
`getSymbols` at `:83` and two `inferType` fixpoints **per template-instantiation
iteration**, with iteration count bounded only by `--template-iterations`
(default `maxBound`, `app/Options/CmdArgs.hs:105`).

**Pass order caution for a reimplementation**: `desugarPasses` uses
`foldr ($) (program, False) passes` (`Frontend.hs:38`), so the list is in **reverse
execution order** — element 18 (`redundantNameScope`/`validateTypedLiterals`) runs first
and element 0 (`validateProgram`) runs **last**. Same for `Template.hs:60`
(`program' = foldr ($) program passes'`). `--frontend-passes=N` keeps the last `N` list
elements = the first `N` executed (`Frontend.hs:38`).

### 3.3 Lookup rules

`Symbols.hs:73-87`:

```haskell
lookupSymbolByName = HashMap.lookup

lookupSymbol e = lookupInScope (getScope e) e

lookupInScope s e symbols = case lookupSymbolByName (s ++ getQualifiedName e) symbols of
    Just x           -> Just x
    Nothing
      | null s       -> Nothing
      | otherwise    -> lookupInScope (init s) e symbols
```

`lookupSymbol` walks **outward** by dropping the last component of the scope, trying
`scope ++ qualifier ++ [name]` at each level, and finally `qualifier ++ [name]` with an
empty scope. This is the *post-parse* analogue of the parser's scope stack, and it is what
makes forward references inside a module work (§7).

### 3.4 Class member and "base class" lookup

**Kanagawa has no inheritance.** `class'` (`Parser.hs:331-341`) parses
`class <ident> { member* }` with no base-clause; grepping the whole Haskell tree and
`compiler/cpp/` for `baseclass|base class|inherit` returns nothing. There is therefore no
base-class lookup order to reproduce.

Member lookup is by exact qualified key (`Type.hs:1358-1362`):

```haskell
    method :: Type -> Name -> Type
    method (TClass x _) y                   = try typeOf $ lookupSymbolByName (x ++ [y]) symbols
    method (TReference x) y                 = method (try theType $ lookupSymbolByName x symbols) y
    method (TInstance x _ _) y              = method x y
    method _ _                              = TUnresolved
```

i.e. class-type qualified name `++ [member]`, one level, no walk; `TReference` (a `this`
reference) is dereferenced through the symbol map first, and a template instance delegates
to its underlying type. Data-member access goes through the struct/class field list
(`Type.hs:1232-1236`, `fields` at `Type.hs:1343-1348`), not the symbol map.

At parse time, methods on an object are resolved by `methodSpecifier`
(`Parser.hs:730-763`): resolve the object expression's type name, then continue
`prefixedScopedNameAndSymbol` with that type as the resolved prefix.

---

## 4. Scope kinds, the scope stack, and shadowing

### 4.1 The scope record

`SymbolTable.hs:51-66`:

```haskell
data SymbolTable =
    SymbolTable
        { scopes :: [Scope]                         -- symbols defined in currently visible scopes
        , module_ :: Module                         -- current module description
        }

data Scope =
    Scope
        { scopeQualifier :: QualifiedName
        , symbolsInScope :: HashMap Name ExpSrc     -- symbols defined in the scope
        , lambdaIndex :: Int
        , scopeIndex :: Int
        , exportIndex :: Int
        }
```

`emptySymbolTable = SymbolTable [Scope mempty mempty 0 0 0] (Module "" Redundant mempty mempty mempty mempty mempty mempty)`
(`SymbolTable.hs:94`) — one global scope with an empty qualifier.

Pushing/popping (`SymbolTable.hs:183-192`):

```haskell
parseScoped startLambdaIndex startScopeIndex updateScopes symbols s p = do
    parentQualifier <- getScopeQualifier
    let scopeQualifier = parentQualifier ++ concatMap getQualifiedName s
        symbolsInScope = HashMap.fromList $ map (\x -> (getName x, x)) symbols
    modify (\x -> x { scopes = Scope scopeQualifier symbolsInScope (startLambdaIndex x) (startScopeIndex x) 0 : scopes x })
    result <- p
    modify (\x -> x { scopes = updateScopes $ scopes x })
    return result
```

### 4.2 The complete set of scope kinds

Four combinators are built on `parseScoped` (`Parser.hs:119-152`):

| combinator | qualifier effect | index seeding | pop action | definition |
|---|---|---|---|---|
| `unscopedWith syms` | **none** (same qualifier as parent) | `lambdaIndex=0`, `scopeIndex=0` | `tail` | `Parser.hs:119` |
| `scopedWith syms ident` | `parent ++ getQualifiedName ident` | `0`, `0` | `tail` | `Parser.hs:123` |
| `quasiScopedWith syms p` | `parent ++ ["$quasi<parent.scopeIndex>"]` | `lambdaIndex = parent.lambdaIndex`, `scopeIndex = parent.scopeIndex + 1` | `y { lambdaIndex = x.lambdaIndex, scopeIndex = y.scopeIndex + 1 }` | `Parser.hs:136-143` |
| `localScoped p` | same as quasi | same as quasi | **also rewrites the parent's qualifier**: `y { scopeQualifier = y.scopeQualifier ++ ["$quasi<y.scopeIndex+1>"], lambdaIndex = x.lambdaIndex, scopeIndex = 0 }` | `Parser.hs:147-153` |

Derived: `scoped = scopedWith []` (`:125`), `functionScopedWith syms = scopedWith syms (Identifier "$")` (`:127`),
`functionScoped = functionScopedWith []` (`:129`), `quasiScoped = quasiScopedWith []` (`:145`).

**Every scope-introducing construct, exhaustively** (from `Parser.hs`):

| construct | scope(s) pushed | line |
|---|---|---|
| `module A.B { … } …` | `scoped ident` → qualifier `["@A@B"]` | 189 |
| `using T = …;` | `unscopedWith params` around the RHS type | 276 |
| `enum E : t { … }` | `scoped ident` → `…++["E"]` | 284 |
| each enum constant | `unscopedWith [x]` (so a later constant can reference an earlier one) | 291 |
| `struct S { … }` | `quasiScopedWith params` | 299 |
| `union U { … }` | `quasiScopedWith params` | 305 |
| `class C { … }` | `unscopedWith selfSymbols` then `scopedWith params ident` | 335-336 |
| template parameter list | `unscopedWith [x]` per parameter (left-to-right defaults) | 363 |
| function definition | `unscopedWith templateParams`, then `unscopedWith (maybeToList self)`, then `scoped ident`, then `functionScoped` for the body | 408, 414, 415, 424 |
| lambda | `scoped ident` (`?lambdaN`), then `functionScopedWith captures` | 440, 444 |
| function **type** `(a,b)->c` | `quasiScoped` around the parameter list | 551 |
| `{ … }` statement block (`localNestedScope`) | `localScoped` | 469 |
| `static if`'s branch blocks (`nestedScope`) | `quasiScoped` | 467 |
| `for (const auto x : e) …` | `localScoped` | 584 |
| `static for (…)` | `localScoped` | 591 |
| `case E:` / `default:` bodies | `localScoped` | 602-603 |

The two synthetic name generators: `makeSymbolName` (`SymbolTable.hs:216-225`) produces
`?lambda<lambdaIndex>` (`Parser.hs:434`) and `$export<exportIndex>`
(`SymbolTable.hs:213`); `getQuasiScopeIdentifier` produces `$quasi<scopeIndex>`
(`Parser.hs:131-134`). `functionScope = (++ ["$"])` (`Desugar.hs:173-174`) and
`isLocalScope = ('$' ==) . head . toString . last . getScope` (`Desugar.hs:176-180`).

**`localScoped`'s parent-qualifier rewrite is the key non-obvious rule**: after a block,
loop or switch-case, *subsequent declarations in the enclosing scope live under a deeper
qualifier*. That is what makes

```
inline void main() { for (const auto x : 10) {} bool x; }
```

warning-free (`test/syntax/shadow-warning.k:77-86`, `warning:0`): the loop's `x` is keyed
`[main,$,$quasi0,x]` and the later `x` is keyed `[main,$,$quasi1,x]`, and
`lookupInScope [main,$,$quasi1] x` never probes the `$quasi0` sibling.

### 4.3 Redefinition in the same scope

`parseSymbol` (`SymbolTable.hs:194-214`) is a hard, immediate error:

```haskell
parseSymbol p = do
    result <- p
    name <- symbolName result
    s <- gets (HashMap.lookup name . symbolsInScope . head . scopes)
    case s of
        Just e ->
            symbolError result $ "symbol redefinition. See previous declaration at " ++ symbolLoc e
        _ -> do
            modify $ \x -> x { scopes = addToScope name result (scopes x)
                             , module_ = addToModule name result (module_ x) (scopeQualifier $ head $ scopes x)
                             }
            return result
```

Only the **innermost** scope is consulted; the symbol is simultaneously inserted into the
scope's `symbolsInScope` and into the module's flat `symbols` map keyed
`scopeQualifier ++ [name]` (`SymbolTable.hs:211`). `ExportType` nodes get the synthetic
name `$export<n>` so multiple `export T;` never collide (`SymbolTable.hs:213`).

Template parameter lists get their own duplicate check with a different message
(`Parser.hs:352-359`): `Redefinition of template parameter(s) […]`.

### 4.4 The `shadow` warning

`WarningKind` (`Warning.hs:20-27`) has exactly six members: `Conversion`, `Deprecated`,
`InitializerOverrides`, `RedundantImport`, `Shadow`, `TransactionSizeMissing`, rendered as
`conversion`, `deprecated`, `initializer-overrides`, `redundant-import`, `shadow`,
`transaction-size` (`Warning.hs:29-35`). `Shadow` is **on by default**
(`app/Options/CmdArgs.hs:112-117`).

Implementation (`TypeCheck.hs:110-121`, invoked from `validateProgram` at `TypeCheck.hs:50`):

```haskell
    checkShadowDecl a@VariableF{..}
        | null declScopeName = a
        | declFlags == DeclareMember || declFlags == DeclareClass = a
        | otherwise = case lookupInScope (init declScopeName) declName symbols of
              Just x | shadowable x
                -> a { declName = warningMsg Shadow ("This declaration of `" ++ getName a ++ "` shadows the existing declaration") $ noteContext x declName }
              _ -> a
      where
        shadowable x@Variable{} = not $ null $ getScope x
        shadowable FuncParam{} = True
        shadowable _ = False
    checkShadowDecl a = a
```

Exact firing conditions — **all** must hold:

1. the node is a `Variable` (never a function, type, alias, enum constant, …);
2. its `declScopeName` is non-empty — a declaration at the unqualified global scope never
   warns (`test/syntax/shadow-warning.k:44-53`, `warning:0`);
3. its `declFlags` is neither `DeclareMember` (struct/union field) nor `DeclareClass`
   (class field). The remaining flags are `DeclareGlobal`, `DeclareLocal`, `DeclareStatic`,
   `DeclareUninitConst` (`Parser/Syntax.hs:184-191`);
4. `lookupInScope (init declScopeName) declName symbols` finds something — i.e. dropping
   *one* level of scope and then walking outward finds a declaration of the same simple
   name;
5. the found declaration is a `Variable` with a non-empty scope, or a `FuncParam`.

Message: `warning: This declaration of `x` shadows the existing declaration [--Wno-shadow]`
(`Error.hs:142-145`), with the shadowed declaration's `Src` appended via `noteContext`
(`Desugar.hs:100-101`), which `makeErrorBundles` renders as a `note:` sub-bundle
(`Error.hs:186-195`).

**Critical timing detail**: `validateProgram` is the *last* frontend pass to execute
(§3.2), so shadow checking sees the AST **after** `moveFunctions` has hoisted nested
functions to the parent scope and renamed them `<parent>@<name>`
(`Function.hs:39,47-50,97`). That is why

```
inline void main() { uint32 x; inline void Bar() { uint32 x; x = 0; } x = 0; }
```

produces no warning (`test/syntax/shadow-warning.k:133-147`): `Bar`'s `x` ends up keyed
`["main@Bar","$","x"]`, whose `init` is `["main@Bar"]`, which never reaches `main`'s `x`.

Complete oracle table (`test/syntax/shadow-warning.k`, registered at
`test/syntax/CMakeLists.txt:77`):

| lines | situation | expected |
|---|---|---|
| 4-12 | local `bool x`, nested block `uint32 x` | `shadow` |
| 14-22 | outer `const bool x`, inner `uint32 x` | `shadow` |
| 24-32 | outer `bool x`, inner `const uint32 x` | `shadow` |
| 34-42 | outer `bool x`, inner `auto x` | `shadow` |
| 44-53 | file-scope `const bool x`, local `uint32 x` | **none** |
| 55-62 | parameter `bool x`, block-local `uint32 x` | `shadow` |
| 64-66 | `import helper.shadow_warning` (module-scoped global + local in the *imported* file) | `shadow` |
| 68-75 | parameter `x`, `for (const auto x : …)` | `shadow` |
| 77-86 | `for (const auto x …) {}` then `bool x` | **none** |
| 88-98 | nested `for` with the same induction name | `shadow` |
| 100-112 | `if/else` branches each declaring `y` | **none** |
| 114-131 | nested `if` inside a branch that already declares `y` | `shadow` |
| 133-147 | nested *function* re-declaring `x` | **none** |
| 149-159 | class field `x`, member-function local `x` | `shadow` |

Row 64-66 also shows that warnings from *imported* files are reported: every parsed file's
AST is merged into one program (`Main.hs:80`) and `warnings` scans the whole tree
(`Error.hs:94-99`).

### 4.5 The C++ side's scope stack (second, independent resolver)

The C++ compiler re-does name resolution over the `ParseTree`. `TypeContext`
(`compiler/cpp/parse_tree.h:1132-1381`) keeps `std::list<TableType> _scopes` plus a
separate `Scope _namespaceScope` (`Scope = std::list<std::string>`, `parse_tree.h:428`).

`LookupNoException` (`parse_tree.h:1313-1360`) walks scopes back-to-front; at each scope it
tries every candidate from `GetScopeSearchList(currImplicitScope, explicitScope)`
(`parse_tree.cpp:7857-7887`):

```cpp
std::list<Scope> GetScopeSearchList(const Scope& implicitScope, const Scope& explicitScope)
{
    if (!explicitScope.empty())
    {
        return {explicitScope};
    }
    …
    Scope currOuterScope = implicitScope;
    while (true)
    {
        const Scope combinedScope = CombineScopes(currOuterScope, explicitScope);
        result.push_back(combinedScope);
        if (currOuterScope.empty()) break;
        currOuterScope.pop_back();
    }
    return result;
}
```

So: an **explicit** scope is used verbatim with no walk; otherwise the accumulated implicit
namespace scope is truncated from the back, innermost-first. Symbol keys are flattened to
`"a::b::name"` (`FlattenScopeAndAppendName`, `parse_tree.cpp:7743-7757`). `AddSymbol`
(`parse_tree.h:1190-1217`) stores into the innermost **non-namespace** scope and errors with
`Duplicate symbol in the same scope: <name>` unless `AddSymbolBehavior::AllowDuplicates`.
`_hideAbove`/`hideWhenRequested` (`parse_tree.h:1140-1146`, `:1336`) hides caller locals
from inlined callees.

---

## 5. The synthetic `.cmdargs` and `.options` modules

### 5.1 The generator

`Parser/IO.hs:37-65` (quoted in full — this is the exact generated text):

```haskell
buildModule :: String -> [String] -> [(String, String)] -> [(String, String)] -> String
buildModule self modules consts aliases =
    "module " ++ self ++
    "{" ++ expose (self:modules) ++ "}" ++
    imports modules ++ "\n" ++
    defs "const auto " consts ++
    defs "using " aliases
  where
    defs x = intercalate "\n" . map (\(n, v) -> x ++ n ++ " = " ++ v ++ ";")
    imports = intercalate "\n" . map ("import " ++)
    expose = intercalate ",\n" . map ("module " ++)

buildCmdargsSpecialModule :: ParseOptions -> String
buildCmdargsSpecialModule opt = buildModule cmdargsSpecialModule [] (nameValue <$> define opt) (nameValue <$> using opt)
  where
    nameValue [] = ([], [])
    nameValue (x:xs')
       | x == '#' || x == '=' = ([], xs')
       | otherwise            = let (ys, zs) = nameValue xs' in (x:ys, zs)

buildOptionsSpecialModule :: ParseOptions -> String
buildOptionsSpecialModule ParseOptions{..} = buildModule optionsSpecialModule ["compiler.options"] options []
  where
    options = [ ("backend",             "Backend::" ++ show backend)
              , ("stall",               show stall)
              , ("optimize",            show optimize)
              , ("max_threads_default", show maxThreadsDefault)
              , ("max_threads_limit",   show maxThreadsLimit)
              ]
```

### 5.2 `.cmdargs` — exact generated source

`--define name#value` / `--define name=value` → `const auto name = value;`
`--using name#type`  / `--using name=type`   → `using name = type;`

`nameValue` splits at the **first** occurrence of `#` **or** `=`, whichever comes first;
the delimiter is dropped and *everything after it* is the value (including further `#`/`=`
characters). A `--define` with no delimiter yields `("foo","")` → `const auto foo = ;`
→ parse error. Both spellings are used in the tree: `--define=ECC_SUPPORT#true`
(`test/logic/CMakeLists.txt:132`) and `--define=MemoryLength=32`
(`tools/chkdoc/chkdoc.cpp:41`).

For `--define A#1 --define B#2 --using X#uint32` the produced text is **byte-exactly**:

```
module .cmdargs{module .cmdargs}
const auto A = 1;
const auto B = 2;using X = uint32;
```

**Trap**: `defs "const auto " consts ++ defs "using " aliases` (`IO.hs:42-43`) has **no
separator between the two groups** — the last `const` line and the first `using` line are
concatenated on one line. This is harmless only because `;` terminates the declaration.
With an empty `define` list and a non-empty `using` list the file is
`module .cmdargs{module .cmdargs}\nusing X = uint32;`.

The export list is `{module .cmdargs}` — a self whole-module export — so every generated
constant and alias is visible to importers (§2.4 rule 2).
`doc/programming-guide.md:4505-4517` documents the same shape.

### 5.3 `.options` — exact generated source

Fixed content, five constants, one import:

```
module .options{module .options,
module compiler.options}import compiler.options
const auto backend = Backend::Sv;
const auto stall = 0;
const auto optimize = 2;
const auto max_threads_default = 512;
const auto max_threads_limit = 512;
```

Notes:
* `expose` joins with `",\n"` (`IO.hs:47`) hence the line break inside the braces, and
  `imports` is appended directly after `}` with no separator (`IO.hs:41`).
* `show backend` uses the derived `Show` of `data Backend = Sv | None`
  (`Parser/Options.hs:16-17`), producing `Backend::Sv` or `Backend::None`. The enum itself
  lives in `library/compiler/options.k:4-12`, which is why `.options` imports
  `compiler.options`; **`.options` therefore cannot be used without `-I` pointing at a tree
  containing `compiler/options.k`.**
* `stall`, `optimize`, `max_threads_default`, `max_threads_limit` are `show`n `Int`s.
  In `--pretty` mode `optimize` is never set, so it defaults to `-1`
  (`Parser/Options.hs:44`, `Main.hs:163-172`) and the generated line is
  `const auto optimize = -1;`.
* **Discrepancy with the documentation**: `doc/programming-guide.md:4541-4549` lists
  `target_device` and `target_device_name` as `.options` constants. The code at
  `IO.hs:60-65` generates **neither**. The doc is stale; reproduce the code.
* Oracle: `test/syntax/options.k`, run with
  `--backend=sv --stall=3 --target-device=ultrascaleplus-xcu250`
  (`test/syntax/CMakeLists.txt:93`); it asserts `stall == 3`,
  `backend == Backend::Sv`, and that `.options` can be imported plain or `as opt`
  (`options.k:4-40`), that `Backend` values are usable in array initialisers
  (`options.k:18-24`) and `static if` (`options.k:26-40`), that neither constant exists
  without the import (`options.k:42-48`), and that an unused `import .options` is
  `redundant-import` (`options.k:50-52`).

### 5.4 How they are injected

`IO.hs:100-102`, inside `readFileContent`, **before** any filesystem access:

```haskell
    readFileContent dir file
        | file == cmdargsSpecialModule <.> "k" = return $ Right (file, dir, buildCmdargsSpecialModule opt)
        | file == optionsSpecialModule <.> "k" = return $ Right (file, dir, buildOptionsSpecialModule opt)
```

The triple is `(canonicalPath, fileDir, content)`. Consequences:

* the cache key (`canonicalPath`) is the literal string `".cmdargs.k"` / `".options.k"`,
  so each is built and parsed at most once per compiler run (`IO.hs:91-98`);
* `fileDir` is the *importing* file's directory, so `.options`'s own
  `import compiler.options` searches from there first;
* the `Src` `sourceName` of everything in these modules is `.cmdargs.k` / `.options.k`,
  which are not real files — `makeErrorBundles readFileUtf8` (`Main.hs:87,96`) would fail
  trying to re-read them if a *desugar-stage* error or warning ever pointed inside them
  (**UNVERIFIED** whether any input triggers this in practice);
* `renderDeps` (`Main.hs:132-133`) filters them out of `--file-list` and `list-deps`:
  `unlines . filter (('.' /=) . head) . sort`.

`.cmdargs` is additionally the fallback implicit base (§6).

---

## 6. The base library

`Parser/IO.hs:67-77`:

```haskell
parseProgram :: ParseOptions -> Cache [ParseResult]
parseProgram opt = importBase >>= \case
    Left e -> return [Left e]
    Right symbolTable -> forM (files opt) (parseFile opt symbolTable)
  where
    importBase = do
        let baseFile
              | noImplicitBase opt || null (importDir opt) = cmdargsSpecialModule <.> "k"
              | otherwise                                  = baseLibrary opt
        b <- parseFile opt emptySymbolTable baseFile
        return $ addImport Needed 0 Nothing emptySymbolTable . snd <$> b
```

Exact rules:

1. The implicit base file is `--base-library` (default `"base.k"`,
   `Parser/Options.hs:38`, `app/Options/CmdArgs.hs:66`) **unless**
   `--no-implicit-base` is given **or** no `--import-dir` was given at all — in either of
   those cases it is `.cmdargs.k`.
   **So `--no-implicit-base` does not remove `.cmdargs`; `--define`/`--using` symbols are
   always in scope.**
2. The base file itself is parsed with `emptySymbolTable` — it gets **no** implicit base of
   its own. That is why `library/base.k:30` and `library/mini-base.k:18` explicitly
   `import .cmdargs`.
3. The resulting symbol table handed to each top-level file is
   `emptySymbolTable` + exactly one `Import` of the base module, flagged `Needed` (so it
   never warns) with `importOffset = 0` and no alias.
4. `forM (files opt) (parseFile opt symbolTable)` — **each command-line file gets a fresh
   copy of the same base table**. Top-level files do *not* see each other's symbols at
   parse time; cross-file references between two command-line files can only be resolved
   later by the global symbol map (§7).

### 6.1 `library/base.k` vs `library/mini-base.k`

`library/base.k:4-58` — the full base:

* exposes **13** individual symbols at lines 5-17 (`optional`, `make_optional`, `pair`,
  `make_pair`, `first`, `second`, `array`, `index_t`, `count_t`, `bitindex_t`,
  `bitcount_t`, `float`, `size_t`) plus **10** whole modules at lines 18-27
  (`.cmdargs`, `base.system`, `control.async`, `control.loop`, `control.wait`,
  `data.closure`, `data.memory`, `debug.print`, `hardware.dsp`, `type.coerce`);
* issues **18** imports (lines 30-47), including `hardware.dsp` and
  `numeric.float32.operator as F` which resolve **only** under
  `device/<target>/` (§1.3);
* `static assert`s that `F::mul`, `F::add`, `F::sub`, `F::mad` have the exact
  `(float32,…)->float32` types the compiler expects for floating-point operators
  (lines 49-55);
* `static assert (version <= device_schema::schema_version);` (line 58) — the device
  configuration schema version gate.

`library/mini-base.k:12-24` — the minimal base used by the syntax tests and by the
markdown code-checker (its doc comment, lines 4-10, says so):

```
module mini-base
    { module .cmdargs
    , module data.closure
    , module debug.print
    }

import .cmdargs
import data.closure
import debug.print
import compiler.config
import compiler.device.schema as device_schema

static assert (version <= device_schema::schema_version);
```

Differences that matter to a reimplementation:

* mini-base exposes only 3 whole modules and **no** individual symbols;
* mini-base does **not** import `hardware.dsp` or `numeric.float32.operator`, so it does
  not force the device tree to contain float32 operators — but it **does** import
  `compiler.config`, which transitively imports `compiler.device.config` →
  `hardware.config`, so the `device/<target>/hardware/config.k` file is still required
  (`library/compiler/config.k:21-22`, `library/compiler/device/config.k:11`);
* `--no-implicit-base` documented consequence: "compilation errors will occur if the source
  code uses floating-point operators or device-specific functions"
  (`doc/programming-guide.md:4493-4495`).

A *custom* base is exercised by `test/syntax/helper/custom_base.k:4-14`, which itself
does `module helper.custom_base { Foo, module mini-base }` + `import mini-base`, invoked
with `--no-implicit-base=false --base-library helper/custom_base.k`
(`test/syntax/CMakeLists.txt:91`).

CLI surface (`app/Options/CmdArgs.hs`): `--import-dir` (`:62`, repeatable),
`--define`/`-d` (`:63`), `--using`/`-u` (`:64`), `--no-implicit-base` (`:65`),
`--base-library` (`:66`), `--parse-docs` (`:67`), `--target-device` (`:87`),
`--file-list` (`:72`). The `ParseOptions` record carrying them is
`Parser/Options.hs:19-33`; the mapping is `getParseOptions`, `Main.hs:146-183`.

---

## 7. Declaration order — does the Rust resolver need two passes?

**Answer: yes, two (really three) phases — but the *parser* is single-pass with two
specific lookahead hacks.**

### 7.1 What must be declared before use (parse time)

The parser resolves names *while parsing* (`findSymbol` at `Parser.hs:656`), and the parse
tree it produces **depends on the result**. Constructs that genuinely require a prior
declaration:

1. **Templates.** `Foo<10>` is parsed as a template instance only if `Foo` is already known
   to be a template, or the `template` disambiguator was written
   (`Parser.hs:646,667-671,676-681`). Otherwise `<` is parsed as the comparison operator.
2. **Type-vs-expression disambiguation.** In expression position the parser first tries
   `try $ typ' False` (`Parser.hs:844`), i.e. `typeSpecifier' False`, which *fails* for an
   unknown identifier (`Parser.hs:712-726`; the failing branch is `Nothing | otherwise ->
   unexpectedSymbol` at `Parser.hs:724,726`). An identifier that is not yet a known type therefore becomes a value
   (`namedValue`, `Parser.hs:783-792`); if it later turns out to be a type,
   `validateProgram`'s "illegal use of this type as an expression" path is what fires.
3. **Template specifiers** (`Parser.hs:702-709`): the symbol must already be a template, or
   `unexpectedSymbol s "template specifier"`.
4. **`typeSpecifier' False` sites** — only the one at `Parser.hs:844`.
5. **Alias transparency in `::` chains** (`Parser.hs:663-664`) requires the alias to be
   already declared.
6. **Redefinition detection** (`SymbolTable.hs:198-201`) is inherently textual-order.

### 7.2 What may be used before declaration

1. **Type names in type position.** `typ = typ' True` (`Parser.hs:515`), and
   `typeSpecifier' True` with `sym == Nothing` returns `TypeIdentifier scope s` with **no
   error** (`Parser.hs:724-725`). The name is left *relative* (not `makeQualified`) and is
   resolved later by `lookupSymbol`/`lookupInScope` against the whole-program symbol map.
2. **Value names in expression position.** `namedValue`/`namedLValue`/`freeFunctionSpecifier`
   all fall through to `return $ NamedValue s` / `return s` when `sym == Nothing`
   (`Parser.hs:781,792,801`).
3. **Names in a module's export list** — `checkExposed` runs after the body
   (`Parser.hs:108,208`). This is the sole content of the `declaration-order` test
   (`test/syntax/helper/declaration_order.k`, `test/syntax/declaration-order.k`).
4. **Class self-reference and forward references among class members**, via an explicit
   *lookahead pre-pass*: `class_` (`Parser.hs:316-322`) and `classTemplate`
   (`Parser.hs:323-330`) first parse the class with
   `lookAheadSkippingMemberBody`/`lookAheadSkippingClassBody`
   (`Parser.hs:307-309`, using `Config{skipMemberBody, skipClassBody}` from
   `Parser/Lexer.hs:67-72` and `L.skipBraces` at `Parser/Lexer.hs:121-122`), then re-parse
   for real with the class itself plus its templates/types/variables pre-seeded into an
   `unscopedWith` scope. Same trick for `functionTemplate` (`Parser.hs:428-430`).
5. **Anything unresolved at parse time** is reported only much later, by `undefinedSymbols`
   (`Frontend.hs:66`; definition at `Frontend.hs:82-155`), which runs after `inferType` has built the global symbol map;
   its messages are `Type name not in scope: `X`` / `Symbol name not in scope: `X`` with
   edit-distance hints (`Frontend.hs:122-153`).

### 7.3 The C++ stage adds a third pass structure

`compiler/cpp/parse_tree.h:1383-1396`:

```cpp
// To enable variables to be referenced before declaration
// and functions to be called before definition
// Type checking is divided into 3 passes.
enum class TypeCheckPass { Functions, Globals, Default };
```

and `Compiler::TypeCheck` (`compiler/cpp/compiler.cpp:1342-1375`) runs
`{Functions, Globals, Default}` over the same `TypeCheckContext`, aborting between passes
on error. Documented at `doc/compiler-design-reference.md:161-173`.

### 7.4 Consequence for the Rust design

* Phase A (per file, ordered): lexing + parsing + *scope-local* symbol collection, with the
  template/type oracle available from already-parsed imports and earlier declarations in
  the same file. This phase is inherently order-sensitive and must be reproduced faithfully
  or `a < b` vs `a<b>` will diverge.
* Phase B (whole program): flat `QualifiedName → Decl` map + `lookupInScope` outward walk;
  resolves everything left relative by phase A, and reports undefined names.
* Phase C: type-check-like passes that assume all functions and globals are registered.

---

## 8. Import cycles

`Parser/IO.hs:79-98`:

```haskell
parseFile opt symbolTable path = do
    cwd <- liftIO getCurrentDirectory
    r <- parseFile' [] cwd path
    either (Ex.throw . FileException) return r
  where
    parseFile' ancestorFiles dir file = do
        x <- liftIO $ readFileContent dir file
        either (return . Left) parseContent x
      where
        parseContent (canonicalPath, fileDir, content)
            | canonicalPath `elem` ancestorFiles = return $ Left "Circular dependency"
            | otherwise = gets (lookup canonicalPath) >>= \case
                Nothing -> do
                    let parseFileCallback = parseFile' (canonicalPath : ancestorFiles) fileDir
                        cfg = Config (parseDocs opt) False False
                    result <- parseKanagawa parseFileCallback cfg symbolTable canonicalPath content
                    modify (++ [(canonicalPath, result)])
                    return $ Right result
                Just result -> return $ Right result
```

Precise semantics:

* `ancestorFiles` is the **DFS path** of canonical paths, not a global visited set. It is
  reset to `[]` for every `parseFile` entry point (the base library and each command-line
  file).
* The cycle check happens **after** reading the file and **before** the cache probe, so a
  cycle beats a cache hit.
* The error value is the bare string `"Circular dependency"` (no file names), surfaced at
  the offending `import` via `parseFileFailure offset err` (`Parser.hs:201`), which wraps
  it in `ExtendedError.FileError` (`Error.hs:57,127-128`).
* Oracles: a 3-cycle `a1.a9 → a1.a10 → a1.a11 → a1.a9`
  (`test/syntax/imports/a1/a9.k`, `a10.k`, `a11.k`; `imports.k:525-527`, `expected:1`) and
  a self-import `module a1.a12 { } import a1.a12` (`a1/a12.k:6`; `imports.k:537-539`).
* The **cache** (`Cache = StateT [(FilePath, ParseResult)] IO`, `Parser.hs:55`) is an
  association list keyed by canonical path, appended in completion order (`IO.hs:96`).
  A diamond import parses each module once; the cached `ParseResult` carries the module's
  final `SymbolTable`, which is all `updateImports` needs (`Parser.hs:205`).
* Because the list is appended *after* the parse completes, the resulting file order is a
  **post-order DFS**: dependencies precede dependents, base library first. That order is
  the merge order for the whole-program AST (`Main.hs:70,80`) and therefore decides
  duplicate-key precedence in `getSymbols` (§3.1) and the `--file-list` content
  (`Main.hs:76-79, 132-144`; and `list-deps`, `Main.hs:111-122`).
* A top-level file that cannot be found is *not* a parse error: `either (Ex.throw .
  FileException) return r` (`IO.hs:83`) throws, aborting the compiler with the message from
  `IO.hs:108`.

---

## 9. The device-configuration path

### 9.1 How `library/device/<dev>/hardware/config.k` is reached

The chain, each link cited:

1. Base (`library/base.k:42` or `library/mini-base.k:21`): `import compiler.config`.
2. `library/compiler/config.k:17-22`:
   ```
   module compiler.config { module compiler.config }
   import compiler.device.schema
   import compiler.device.config as device
   ```
3. `library/compiler/device/config.k:7-11`:
   ```
   module compiler.device.config
       { module compiler.device.config \ hardware.config
       , module hardware.config
       }
   import hardware.config
   ```
4. `import hardware.config` requests the path `hardware/config.k`, which the search order
   of §1.3 finds at `<import-dir>/device/<target-device>/hardware/config.k` **before** any
   plain `<import-dir>/hardware/config.k` (there is no such file in `library/`).
5. The module difference at step 3 means: everything `compiler.device.config` defines
   itself (`fifo_depth_pow2`, `supports_auto_pipelining`, `min_auto_pipeline_depth`,
   `supports_luts`, `trade_area_for_speed` — `library/compiler/device/config.k:14-28`)
   is exposed **only if the device file does not define a symbol of the same name**, and
   `hardware.config`'s symbols are exposed unconditionally. That is the defaults-plus-override
   mechanism (§2.5).
6. `library/compiler/config.k:25-176` then re-declares every property as
   `const … name = device::name;`, all inside the namespace `@compiler@config`.

The device file declares `module hardware.config { module hardware.config }` and imports
`compiler.device.schema` (`library/device/mock/hardware/config.k:10-14`).

Schema/version handshake: `library/compiler/device/schema.k:11`
(`const auto schema_version = 1;`), `library/device/mock/hardware/config.k:17`
(`const auto version = 1;`), and `static assert (version <= device_schema::schema_version);`
in both bases (`library/base.k:58`, `library/mini-base.k:24`).

### 9.2 What the C++ `DeviceConfigVisitor` consumes

`compiler/cpp/device_config_visitor.cpp:642-673`:

```cpp
void DeviceConfigVisitor::VisitNode(const ParseTreeNode* const node, VisitContext& context)
{
    const auto declareNode = dynamic_cast<const DeclareNode*>(node);
    if (nullptr != declareNode)
    {
        auto scope = declareNode->GetScope();
        auto assignNode = declareNode->GetAssignNode();

        if (nullptr != assignNode && ScopesEqual(_configScope, scope))
        {
            auto it = _propertyVisitors.find(declareNode->GetDeclaredName());
            if (it != _propertyVisitors.end())
            {
                if (!declareNode->_isConst) { … "must be declared as const" … }
                it->second->Extract(node, context);
            }
        }
    }
}
```

So: **every `const` declaration in scope `@compiler@config` whose name matches one of the
registered properties**, driven from `Compiler::ExtractDeviceConfig`
(`compiler/cpp/compiler.cpp:1394-1440`) which walks the whole ParseTree.

Preflight: the constructor requires `MemoryResource`, `MemoryType`, `MemoryConfiguration`
to exist in `@compiler@device@schema`, else
`CompileError::MissingDeviceConfigSchema` with the message
"Device definition schema types are missing…" (`device_config_visitor.cpp:571-593`).

**The complete property list** (`device_config_visitor.cpp:599-637`) — 36 scalar properties
plus 2 aggregates:

| property | extracted C++ type | required Kanagawa form |
|---|---|---|
| `integer_mul_src_width` | `size_t` | `LeafType`, `KnownValueType::Int`, ≥ 0 |
| `almost_empty_depth` | `size_t` | ″ |
| `unsigned_integer_mul_name` | `std::string` | `StringType`, `KnownValueType::String` |
| `signed_integer_mul_name` | `std::string` | ″ |
| `verilog_dont_merge_pragma_name` | `std::string` | ″ |
| `memory_init_file_type` | `MemoryInitFileType` | `EnumType`, int 0 (`Mif`) or 1 (`Mem`) |
| `small_lut_size` | `size_t` | int |
| `lut_based_shift_register_available` | `bool` | `BoolType`, int |
| `block_ram_supports_hardened_bypass` | `bool` | ″ |
| `global_data_propagation_ram_alignment` | `size_t` | int |
| `min_fifo_depth` | `size_t` | int |
| `min_dual_clock_fifo_depth` | `size_t` | int |
| `min_almost_full_depth` | `size_t` | int |
| `supports_auto_pipelining` | `bool` | bool |
| `min_auto_pipeline_depth` | `size_t` | int |
| `supports_luts` | `bool` | bool |
| `block_ram_supports_true_dual_port` | `bool` | bool |
| `use_internal_buffer_fifo_optimization` | `bool` | bool |
| `fifo_depth_pow2` | `bool` | bool |
| `block_ram_supports_ecc` | `bool` | bool |
| `block_ram_supports_quad_port` | `bool` | bool |
| `registers_require_power_on_initial_value` | `bool` | bool |
| `use_thread_rate_in_fifo_sizing` | `bool` | bool |
| `fifo_width_alignment` | `size_t` | int |
| `fifo_depth_alignment` | `size_t` | int |
| `fifo_fixed_cost` | `size_t` | int |
| `fifo_bits_per_register` | `size_t` | int |
| `array_mux_cost_factor` | `float` | `FloatType`, int (bit-cast) |
| `array_register_cost_factor` | `float` | ″ |
| `array_write_port_cost_factor` | `float` | ″ |
| `default_clock_frequency_mhz` | `size_t` | int |
| `trade_area_for_speed` | `bool` | bool |
| `device_name` | `std::string` | string |
| `vendor` | `std::string` | string |
| `device_family` | `std::string` | string |
| `hal_device_family` | `std::string` | string |
| `memory_resources` | `std::vector<DeviceMemoryResource>` | array of `MemoryResource` |
| `memory_configurations` | `std::vector<DeviceMemoryConfiguration>` | array of `MemoryConfiguration` |

Every one of these must be *found*; `Finalize` (`device_config_visitor.cpp:675-768`)
collects missing names and `ExtractDeviceConfig` raises
`CompileError::MissingDeviceConfigProperty` listing them (`compiler.cpp:1410-1438`).

Type gates, per extractor:

* `GetKnownValueHelper<NodeType>` (`device_config_visitor.cpp:45-67`) requires the
  **declared type node** to be exactly `BoolType` / `LeafType` / `FloatType` / `StringType`
  / `EnumType` for the corresponding property, then calls
  `TryGetVisitKnownValue(context, rhs, declaredType)` on the assignment RHS.
* `float` values are transported as the `uint32_t` bit pattern and
  `*reinterpret_cast<float*>` back (`device_config_visitor.cpp:167-171`, `:492-493`).
* Enum values must fold to 0/1/2 for `MemoryType` (`TryMpToMemoryType`,
  `device_config_visitor.cpp:6-25`) and 0/1 for `MemoryInitFileType`
  (`device_config_visitor.cpp:27-43`).
* `KnownValueType` has exactly three members: `None`, `Int`, `String`
  (`compiler/cpp/parse_tree.h:386-391`) — floats and enums travel as `Int`.

Aggregates (`device_config_visitor.cpp:263-391` and `:393-569`):

* the declared type must be an `ArrayType` whose `_elementType` is exactly the schema's
  `MemoryResource` / `MemoryConfiguration` `StructUnionType`;
* the RHS must reach an `InitializerListNode`. `GetInitializerListFromNode`
  (`device_config_visitor.cpp:228-261`, duplicated at `:536-561`) accepts either a direct
  initializer list **or** a `VariableAccessNode`, which it resolves via
  `context.LookupSymbol(scope, name, …)` → `DeclareNode` → `GetAssignNode()->GetRhs()`,
  **recursively**. This is exactly what `library/compiler/config.k:159` needs
  (`const auto memory_configurations = device::memory_configurations;`);
* each element must be a `BaseInitializerListNode`, and `TryGetField(fieldIndex, fieldName)`
  handles both positional and designated initialisation
  (`device_config_visitor.cpp:333`, `:463`). `library/device/mock/hardware/config.k:130-157`
  uses designated form;
* the field sets are fixed: `MemoryResource{type_id, num_available}` and
  `MemoryConfiguration{type_id, cost, width, depth}` — any other field name hits
  `assert(false)` (`device_config_visitor.cpp:361-364`, `:511-514`). Field
  declarations: `library/compiler/device/schema.k:26-49`;
* `Finalize` caps each memory kind at the fixed array size ("Device memory configuration
  must not exceed 10 elements", `device_config_visitor.cpp:710-719`), sorts each kind by
  descending depth then descending width (`:722-731`), and fills `_defaultFreqMhz` for all
  clocks from `default_clock_frequency_mhz` (`:757`).

### 9.3 What the Rust constexpr evaluator must fully fold

Everything reaching `@compiler@config` must be reduced by the Rust frontend to one of:
an integer literal, a bool literal, a string literal, a float literal (as a 32-bit
pattern), an enum value, or an `InitializerList` (possibly via a chain of `const`
variable references, which the C++ side will follow itself). Concretely the folding
capability required is that of `evaluateConstexpr` (`Constexpr.hs:128-…`):

* **Binary integer ops** (`Constexpr.hs:176-198`): `Add`, `Sub`, `Mul`, `LutMul`,
  `Div` (error "Division by zero is undefined"), `Mod` (error "Remainder by zero is
  undefined"), `BitwiseAnd`, `BitwiseOr`, `BitwiseXor`, `ShiftLeft`, `ShiftRight`,
  `Equal`, `NotEqual`, `Greater`, `GreaterEqual`, `Less`, `LessEqual`. Arbitrary-precision
  (`mp_int` on the C++ side, `Integer` in Haskell). Required by
  `library/device/mock/hardware/config.k:151` (`20774 / 5`).
* **Bool ops** (`:204-210`): `Equal`, `NotEqual`, `Or`, `And`, `Xor`.
* **String ops** (`:212-215`): `Equal`, `NotEqual`.
* **Type comparisons** (`:244-250`): `==`, `!=`, `<=`, `<`, `>=`, `>` on types via `<:`.
* **Short-circuit partial folds** (`:229-242`): `x & 0`, `x | allOnes`, `x || true`,
  `x && false`.
* **Unary** (`:252-284`): `Neg` (int and float), `Invert` (with width truncation via
  `truncateUInt`/`truncateInt`, `Internal.hs:34-41`), `BitSizeof`, `ByteSizeof`, `Not`.
* **`clog2`** (`:168-169`, `Internal.hs:53-57`).
* **`bitoffsetof` / `byteoffsetof`** (`:162-166`).
* **Constant propagation through `const` variables and enum constants** (`:286-293`):
  a `NamedValue` whose symbol is `Variable _ _ (Const t) _ (Just v) _` folds to `v` when
  `v` is a literal / enum / function, with an integral cast when `t` is int or float;
  an `EnumConstant` folds to `EnumValueF`.
* **Aggregate constant propagation** (`:142-152`): inside a template instance, a
  `NamedValue` of const struct/array/union type is replaced by its `InitializerList`.
* **Parametric type folding** (`:298-…`): `ParamIntF`/`ParamUintF` → `IntegerF`/`UnsignedF`.
* **`static if` / `static for`** (`staticControl`, `Constexpr.hs:41-126`).

---

## 10. Namespace semantics and declaration keying (summary table)

There is **no `namespace` keyword** — `reserved` (`Parser/Lexer.hs:164-219`) contains
exactly: `as, atomic, auto, barrier, bitsizeof, bitoffsetof, bool, break, bytesizeof,
byteoffsetof, cast, case, class, clog2, concat, const, decltype, default, do, else, enum,
export, extern, false, fan_out, float32, for, if, import, inline, int, lutmul, module, mux,
noinline, private, public, reorder, return, static, static_assert, string, struct, switch,
template, true, typename, uint, union, unrolled_for, using, void, while`. Namespaces come
solely from `module` declarations.

“Nested namespaces” exist only as dotted module names, which mangle to a **single flat
`Name`** (`@a@b@c`), *not* a multi-component qualifier. `a1.b1.b1` and `a1.b2.b2` are
unrelated namespaces with no containment relation — confirmed by `imports.k:164-193`, where
importing both and using `b1` and `b2` is fine, and by the requirement that
`module a1.a20 { module a1.a1 }` needs a *direct* import (`imports.k:664-666`).

How each declaration is keyed:

| context | `declScopeName` / key prefix | example key |
|---|---|---|
| file with no `module` | `[]` | `["Foo"]` |
| inside `module a.b` | `["@a@b"]` | `["@a@b","Foo"]` |
| class member | `[…,"C"]` | `["@a@b","C","m"]` |
| function body local | `[…,"f","$"]` | `["@a@b","f","$","x"]` |
| struct/union body (`quasiScoped`) | `[…,"$quasiN"]` | not in the symbol map (§3.1) |
| block / loop / case body (`localScoped`) | `[…,"$quasiN"]` | `["@a@b","f","$","$quasi0","x"]` |
| lambda | `[…,"?lambdaN","$"]` | — |
| `export T;` | synthetic name `$exportN` | `["@a@b","$export0"]` |
| any error node | `["$errors"]` | `["$errors"]` |

---

## 11. Proposed Rust design

### 11.1 Crate/query layout

```
kg-vfs      FileId ↔ AbsPath, file text (HIGH durability for library/, LOW for open buffers)
kg-config   ResolveConfig: import_dirs, target_device, base_library, no_implicit_base,
            defines, usings, parse_docs   (HIGH durability; changing it invalidates the world)
kg-parser   parse(FileId) -> (SyntaxTree, Vec<Diag>)          -- pure, no name lookups
kg-resolve  everything in this document
```

### 11.2 Module graph queries

```rust
/// Canonical identity of a module. Synthetic modules have no FileId.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ModuleSource { File(FileId), Cmdargs, Options }

/// The dotted name as written, e.g. ["a1","b1"] or [".cmdargs"].
type ModuleName = SmallVec<[SmolStr; 4]>;

/// Mangled namespace: "@a1@b1".  Interned.
struct Namespace(SmolStr);

// --- queries -------------------------------------------------------------

/// §1.3.  Ordered candidate directories. Depends only on ResolveConfig.
#[salsa::input] fn search_dirs(cfg) -> Arc<Vec<AbsPathBuf>>;   // device dirs then plain dirs

/// §1.2 + §1.3.  `from_dir` is the *found* directory of the importing file.
#[salsa::query] fn resolve_module_path(name: ModuleName, from_dir: AbsPathBuf)
    -> Result<ModuleSource, ImportError>;

/// §1.4 + §8.  Post-order DFS over imports, memoised per ModuleSource.
/// Returns the module's public surface only — NOT its bodies — so that editing a
/// function body does not invalidate importers.
#[salsa::query] fn module_interface(m: ModuleSource) -> Arc<ModuleInterface>;

/// §8.  The whole compilation's file list in post-order-DFS order.
#[salsa::query] fn program_files(cfg) -> Arc<Vec<ModuleSource>>;
```

`ModuleInterface` is the Rust image of `SymbolTable.Module` minus the parts a consumer
cannot see:

```rust
struct ModuleInterface {
    namespace: Namespace,
    /// §2.4.  namespace -> (QualifiedName -> ItemId)
    visible: FxHashMap<Namespace, FxHashMap<QualName, ItemId>>,
    /// §2.4 rule 8: contains `export T;` or a `[[reset]]` fn.
    needed: bool,
}
```

Firewall trick: make `module_interface` a `#[salsa::tracked]` returning an
`Arc<ModuleInterface>` with `#[return_ref]` + interning, so that reformatting or editing a
function body in `library/data/array.k` produces an identical interface and stops
invalidation at that boundary. This is the single most important incrementality decision.

### 11.3 Per-file item tree

Parsing must stay pure, but §7.1 shows the *parser* needs a symbol oracle. Split it:

```rust
/// Purely syntactic, no name resolution. `<` stays ambiguous.
fn parse(FileId) -> SyntaxTree;

/// Item tree: one entry per declaration, in source order, with the scope path
/// it was declared in and the synthetic index counters already assigned.
struct ItemTree {
    items:  Vec<Item>,                 // ItemId = index
    scopes: Vec<ScopeData>,            // ScopeId = index; scopes[0] is the file scope
    module_header: Option<ModuleHeader>,
    imports: Vec<ImportDecl>,          // in source order
}
struct Item {
    scope: ScopeId,
    qual:  QualName,                   // declScopeName ++ [name]  (§3.1)
    kind:  ItemKind,                   // Alias|Struct|Union|Class|Enum|EnumConst|Fn|FnDecl
                                       // |Var{flags}|Param|Capture|Template|ExportType
    is_template: bool,
    is_type: bool,
}
struct ScopeData {
    parent: Option<ScopeId>,
    kind: ScopeKind,                   // §4.2, five kinds
    qualifier: QualName,               // already includes $quasiN / $ / ?lambdaN
    names: FxHashMap<SmolStr, ItemId>,
}
enum ScopeKind { Module, Unscoped, Named, Quasi, Local, Function }
```

Then run a **disambiguation pass** that consumes `ItemTree` + imported interfaces and
rewrites the ambiguous `<` sites and expression-vs-type sites. This reproduces §7.1 without
making the parser depend on other files. (The alternative — reproducing the Haskell design
literally, i.e. `parse(file, symbol_table)` — makes every file's parse depend on every
transitive import's *contents* and is incompatible with incremental editing.)

### 11.4 Scope tree and the name-resolution query

```rust
/// Fully-qualified reference to a definition anywhere in the program.
#[derive(Copy, Clone, PartialEq, Eq, Hash)] struct DefId(ModuleSource, ItemId);

/// §2.7 and §3.3, unified.
#[salsa::query]
fn resolve_name(
    cfg:      ResolveConfig,
    origin:   ModuleSource,   // file we are resolving in
    scope:    ScopeId,        // innermost lexical scope
    qualifier: &[SmolStr],    // explicit `::` prefix, empty for unqualified
    name:     &SmolStr,
) -> Resolution;

enum Resolution {
    Found(DefId),
    NotFound,
    Ambiguous(SmallVec<[DefId; 2]>),          // -> "symbol is ambiguous ..."
    ImportQualifierConflict { def: DefId, import: ImportId },  // §2.7 step 4
}
```

Algorithm — a direct transcription of `findSymbol`:

```
if qualifier.is_empty():
    for s in scope_chain(scope):                    # innermost -> outermost
        if let Some(d) = s.names.get(name):
            hits = [d] + find_in_imports(qualified_name_of(d))   # §2.7 step 2!
            break
    else:
        hits = own_module_symbols.lookup_in_scope(scope.qualifier, [name])
             + find_in_imports([name])
    if hits.len() == 1: check_import_qualifier_conflict(name, hits[0])
else:
    hits = own_module_symbols.lookup_in_scope(scope.qualifier, qualifier ++ [name])
         + find_in_imports(qualifier ++ [name])
dedup_preserving_order(hits)                         # `nub`
```

`find_in_imports` must reproduce `SymbolTable.hs:263-278` exactly, including:
* iterate imports in **reverse source order**;
* subtract every earlier-iterated import's hits (`f_i \ f_0 \ … \ f_{i-1}`);
* record `needed = true` on any import whose residual set is non-empty;
* per import, for each `(ns, map)` in the interface's `visible`, try key
  `name` when `name[0] == ns`, else `[ns] ++ strip_prefix(alias, name)?`.

Because the "needed" bookkeeping is a *side effect of resolution*, model it as a separate
accumulator query rather than mutation:

```rust
#[salsa::query] fn import_usage(origin: ModuleSource) -> Arc<BitSet /*ImportId*/>;
#[salsa::query] fn redundant_import_diags(origin: ModuleSource) -> Arc<Vec<Diag>>;
```

`import_usage` walks every name occurrence in the file once and unions the attributions.

### 11.5 Global symbol map (`getSymbols`) — do NOT rebuild it 15×

Model `getSymbols` as one query:

```rust
#[salsa::query] fn global_symbols(cfg) -> Arc<FxHashMap<QualName, DefId>>;
```

built by concatenating `ItemTree`s in `program_files` order, first-wins on duplicate keys
(§3.1). Reproduce the three `SymbolsConfig` variants as three queries
(`global_symbols`, `global_symbols_no_template`, `global_symbols_template_decl`) rather
than a parameter, so each is cached independently. `lookup_in_scope` is a pure function
over the map, not a query.

The Haskell rebuild-per-pass (§3.2) is an artefact of purity, not a semantic requirement:
what *is* semantic is that later passes observe an AST that earlier passes rewrote. In
Rust, keep the map keyed by `DefId` and invalidate only when the item tree changes.

### 11.6 Durability plan

| input | durability | rationale |
|---|---|---|
| `library/**` file text | `Durability::HIGH` | never edited in an LSP session; a HIGH input change is the only thing that may invalidate the base-library interfaces |
| `ResolveConfig` (`--import-dir`, `--target-device`, `--base-library`, `--no-implicit-base`, `--define`, `--using`) | `Durability::HIGH` | changing any of these must invalidate the whole world, including the two synthetic modules |
| user project files not open in the editor | `Durability::MEDIUM` | |
| open editor buffers | `Durability::LOW` | |

Extra rules that make the HIGH durability actually pay off:

* `search_dirs` and `resolve_module_path` depend only on `ResolveConfig` and the VFS
  *existence* set — put file-existence in its own input so that creating an unrelated file
  does not invalidate every path resolution;
* the two synthetic modules are **derived inputs**: `cmdargs_source(cfg) -> String` and
  `options_source(cfg) -> String`, generated byte-exactly per §5.2/§5.3, then fed through
  the ordinary `parse`/`item_tree`/`module_interface` chain with `ModuleSource::Cmdargs` /
  `::Options`. This keeps one code path for real and synthetic modules, matching
  `IO.hs:100-102`;
* interning `Namespace` and `QualName` components is required for the `FxHashMap<QualName,
  _>` keys to be cheap — the reference implementation already does this
  (`Name = Hashed String`, `Parser/Syntax.hs:51`);
* cycle detection (§8) must be done in the *query* that computes `program_files` /
  `module_interface`, using salsa's cycle recovery to emit `Circular dependency` at the
  offending import rather than panicking.

### 11.7 Test oracle

`test/syntax/imports.k` (1072 lines, 120 assertion points) plus
`test/syntax/shadow-warning.k` (159 lines, 14 assertion points), `test/syntax/options.k`, `test/syntax/declaration-order.k`,
`test/syntax/custom-base.k`, `test/syntax/base-test.k` and the `test/syntax/imports/**`
fixtures constitute a complete differential test suite for everything in §§1-8. The runner
protocol is in `test/syntax/syntax-test.py:81-166`: lines accumulate into a temp file until
a line matching `expected:(\d*)` or `warning:(\S*)` is seen, at which point the accumulated
program is compiled; `expected:N` asserts the first `Error N` in stdout and exit code
(`syntax-test.py:137-163`), `warning:X` asserts the first
`warning: … [--Wno-X]` (`syntax-test.py:121-124, 144-156`), `warning:0` asserts none, and a
bare `warning:` asserts nothing about warnings.

---

## 12. Open questions

Each names the probe program that would settle it.

1. **Is `checkRedundantImports`'s first equation reachable?**
   `Parser.hs:234` matches `ModuleDecl _ _ a`, but `moduleDecl` always returns
   `Seq [ModuleDecl …]` (`Parser.hs:189`), so `Parser.hs:235` (`Seq a`) matches first and
   the warnings are prepended to the outer `Seq`, not inside the module body.
   Semantically irrelevant (warnings are collected tree-wide by `Error.hs:94-99`), but a
   reimplementation should confirm placement if it ever pretty-prints the tree.
   *Probe*: `kanagawa --pretty` on a module file with an unused import and inspect whether
   the warning node appears inside or outside `module … { }`.

2. **Do two command-line files see each other's symbols at parse time?**
   `IO.hs:70` gives each file a *fresh copy* of the base symbol table, so the answer should
   be "no", with resolution deferred to `global_symbols`. But the consequence for
   template `<` disambiguation is untested by the suite.
   *Probe*: `a.k` = `template <auto N> struct T { uint<N> x; }`; `b.k` = `inline void
   main(){ static T<10> t; }`; compile `kanagawa a.k b.k` and then `kanagawa b.k a.k`.
   If order matters, the Rust design must model command-line file order.

3. **Does a `Src` inside `.cmdargs.k` / `.options.k` ever reach `makeErrorBundles`?**
   `Main.hs:87` and `:96` call `makeErrorBundles readFileUtf8`, which would attempt to open
   the non-existent file `.cmdargs.k`.
   *Probe*: `kanagawa --define 'X#1/0' foo.k` (a division-by-zero constexpr in a generated
   `const auto`), or `--using 'T#uint32' ` where `T` shadows something warn-worthy.

4. **What exactly is `exposedSymbols`'s behaviour for a duplicate name in the export list?**
   `Map.fromList` (`SymbolTable.hs:119`) is last-wins, and `checkSymbols` runs over the
   `Map`, so `module M { x, x }` would check `x` only once and never diagnose the
   duplication.
   *Probe*: `module foo { x, x } const uint32 x = 1;` — expect `expected:0`; confirm.

5. **`getSymbols`' first-wins duplicate rule across files.**
   §3.1 derives "earlier file wins" from `HashMap.unions` left-bias and the `reverse` in
   `getSymbols''`. No test in the tree exercises two files defining the same
   fully-qualified name (the parser's own redefinition check catches the same-file case).
   *Probe*: two modules with the *same* declared name in different directories, both
   imported — but §1.4 forbids that, so the only route is two command-line files each
   lacking a `module` header and each defining `struct S`.

6. **Behaviour when `--import-dir` is given but `--base-library`'s default `base.k` is
   absent.** `IO.hs:74` picks `base.k` whenever any `-I` exists; if no `-I` directory
   contains `base.k`, `IO.hs:83` throws `FileException`.
   *Probe*: `kanagawa -I /tmp/empty foo.k`.

7. **`module A \ B` where `A` is an aliased import.** `moduleSymbols`
   (`SymbolTable.hs:154-156`) finds the import by namespace, ignoring `importedAs`, so the
   difference would use the *unaliased* symbol map while `filterImported` would have
   returned `mempty` for that import. The `diffs` map bypasses the alias rule.
   *Probe*: `module M { module A \ B } import a1.a1 as A` — but `checkModules` requires the
   *namespace* to match, so write `module M { module a1.a1 \ a1.b1.b1 } import a1.a1 as A
   import a1.b1.b1` and check whether `a1.a1`'s symbols become unqualified-visible despite
   the alias.

8. **Is the "global, mutable state can't be exposed outside a module" branch reachable?**
   `Parser.hs:221-222` fires when an exported name resolves to a non-`const` `Variable`.
   At module scope the only variable form is `globalVariable` = `DeclareGlobal`
   (`Parser.hs:246,461`), and a non-`const` `DeclareGlobal` is separately rejected by
   `checkVarDecl` with "Global state is not allowed" (`TypeCheck.hs:126`) — but that is
   a *frontend* check that runs after parsing, so the export-list error should win and be
   the first `Error 1` reported. No test covers it.
   *Probe*: `module foo { x }` + `uint32 x;` — check which of the two messages appears.
