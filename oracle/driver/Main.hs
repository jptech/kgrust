{-# LANGUAGE PatternSynonyms #-}
-- | A differential oracle for the Rust port's `kg-hir` lowering.
--
-- The round-trip gate in `xtask ir` cannot catch a *deterministic* lowering
-- bug, because the same bug sits on both sides of
-- `lower(parse(pretty(lower(parse(f)))))` and cancels. Reversing the ternary
-- arms — inverting every `a ? b : c` in the corpus — leaves that gate at
-- 403/403. This is the instrument whose two sides do not share the port's
-- lowering: it prints the *reference's own* tree, so a disagreement is real.
--
-- `prettySource` is what `--dump-source` prints (`Main.hs:90-91` in the
-- reference's executable, which links a C++ backend this driver deliberately
-- does not).
module Main (main) where

import Control.Monad.State (execStateT)
import System.Directory (canonicalizePath)
import Data.Either (lefts, rights)
import Data.Char (toLower)
import Data.Foldable (toList)
import Data.List (intercalate, nub)
import Prettyprinter (defaultLayoutOptions, layoutPretty)
import Prettyprinter.Render.String (renderString)
import System.Environment (getArgs)
import System.Exit (exitFailure)
import System.IO (hPutStrLn, stderr)

import Text.Megaparsec (errorBundlePretty)

import Language.Kanagawa.Error (errors, makeErrorBundles, warnings)
import Language.Kanagawa.Warning (WarningKind(..), WarningState(..), enabledWarnings, enableSwitch, disableSwitch)
import Language.Kanagawa.Frontend (frontend)
import Language.Kanagawa.Internal (readFileUtf8)
import Language.Kanagawa.Type (unnoteType, showPretty, Typed, TypedExp(typeOf))
import Language.Kanagawa.Parser.IO (parseProgram)
import Language.Kanagawa.Parser.Options
import Language.Kanagawa.Parser.Syntax (ExpF(..), ExpLike(..), Note(..), NotedExp, pattern NotedExp, toString)
import Language.Kanagawa.Parser.SymbolTable (ExpSrc)
import Language.Kanagawa.PrettyPrint (prettySource)
import Language.Kanagawa.Recursion (cata)

-- | The tree as the reference holds it, notes elided.
--
-- `prettySource` is lossy about structure — it reconstructs a switch's `break`
-- rather than revealing whether the parser kept one — so source comparison can
-- only catch what survives printing. `ExpF` derives `Show`
-- (`Parser/Syntax.hs:584`) and `Note`'s own `Eq`/`Ord` already ignore the
-- annotation (`:594-599`), which is the reference saying that the note is not
-- part of the tree's identity. Eliding it here says the same thing.
tree :: Show e => NotedExp n e -> String
tree = unRaw . cata (\(Note _ e) -> Raw (show e))

-- | `show` on `ExpF e a` needs `Show a`, and a `String` child would come back
-- quoted — so each level of the tree would escape the level below it and a
-- deep file renders as a wall of backslashes. A newtype whose `Show` is the
-- identity splices the child in raw.
newtype Raw = Raw { unRaw :: String }

instance Show Raw where
    show = unRaw

main :: IO ()
main = do
    argv <- getArgs
    case argv of
        [] -> hPutStrLn stderr "usage: kg-oracle [-I DIR]... [--no-base] [--parse-docs] [--define D]... [--using U]... [--target-device T] [--Werror] [--Wall] [--W<kind>]... [--Wno-<kind>]... [--tree|--notes|--errors] [--roots-only] [--passes N] [--template-passes N] [--template-iterations N] FILE" >> exitFailure
        _  -> do
            let mode | elem "--loop" argv   = Loop
                     | elem "--errors" argv = Errors
                     | elem "--typed" argv  = TypedNotes
                     | elem "--notes" argv   = Notes
                     | elem "--tree" argv    = Tree
                     | otherwise             = Source
                rootsOnly = elem "--roots-only" argv
                counts = ( intFlag "--passes" argv
                         , intFlag "--template-passes" argv
                         , intFlag "--template-iterations" argv )
                werror = if elem "--Werror" argv then Just (warningState argv) else Nothing
                rest = dropFlags argv
            case mode of
                Loop -> loop (parseArgs rest defaultOptions)
                _ -> run mode rootsOnly counts werror (parseArgs rest defaultOptions)

data Mode = Source | Tree | Notes | TypedNotes | Errors | Loop

-- | `--passes N` runs the last N entries of the reference's pass list.
--
-- `desugarPasses` folds with `foldr ($)` and takes
-- `drop (length passes - frontendPasses) passes` (`Frontend.hs:38`), so the
-- *last* entry runs *first* and `N` counts passes in execution order: `1` is
-- `redundantNameScope` and friends, `8` is through `instantiateTemplates`.
-- Omitted means all of them.
-- | `instantiateTemplates` honours `templatePasses` only when
-- `templateIterations == 0`, and then runs the *last* `templatePasses` entries
-- of its own inner list (`Template.hs:62-64`), which under `foldr ($)` execute
-- first. So `--template-passes N --template-iterations 0` runs exactly the
-- first N inner sub-passes, and the N-to-N+1 diff is one sub-pass alone.
intFlag :: String -> [String] -> Int
intFlag name (f : n : _) | f == name = read n
intFlag name (_ : rest) = intFlag name rest
intFlag _ [] = maxBound

dropFlags :: [String] -> [String]
dropFlags ("--tree" : rest) = dropFlags rest
dropFlags ("--errors" : rest) = dropFlags rest
dropFlags ("--notes" : rest) = dropFlags rest
dropFlags ("--typed" : rest) = dropFlags rest
dropFlags ("--roots-only" : rest) = dropFlags rest
dropFlags ("--passes" : _ : rest) = dropFlags rest
dropFlags ("--template-passes" : _ : rest) = dropFlags rest
dropFlags ("--template-iterations" : _ : rest) = dropFlags rest
dropFlags ("--Werror" : rest) = dropFlags rest
dropFlags ("--loop" : rest) = dropFlags rest
dropFlags (x : rest)
    | Just _ <- warningSwitch x = dropFlags rest
    | otherwise = x : dropFlags rest
dropFlags [] = []

parseArgs :: [String] -> ParseOptions -> ParseOptions
parseArgs [] o = o
parseArgs ("-I" : d : rest) o = parseArgs rest o { importDir = importDir o ++ [d] }
parseArgs ("--no-base" : rest) o = parseArgs rest o { noImplicitBase = True }
-- The doc-comment switch: `Parser/IO.hs:94` builds the parser's `Config` from
-- this field, so the oracle grades `kg-syntax::doc_comments` placement rules
-- against the reference itself.
parseArgs ("--parse-docs" : rest) o = parseArgs rest o { parseDocs = True }
parseArgs ("--base" : b : rest) o = parseArgs rest o { baseLibrary = b }
-- The three `ParseOptions` fields the reference's `getParseOptions` fills
-- from the command line (`app/Main.hs:146-162`), taken verbatim: `define`
-- and `using` feed the generated `.cmdargs` module (`Parser/IO.hs:49-55`),
-- `targetDevice` the device import directory (`Parser/IO.hs:110`).
parseArgs ("--define" : d : rest) o = parseArgs rest o { define = define o ++ [d] }
parseArgs ("--using" : u : rest) o = parseArgs rest o { using = using o ++ [u] }
parseArgs ("--target-device" : t : rest) o = parseArgs rest o { targetDevice = t }
-- The five fields `buildOptionsSpecialModule` spells into `.options`
-- (`Parser/IO.hs:57-65`), which a program reads as `options::stall` and the
-- like; cmdargs matches `--backend`'s value case-insensitively.
parseArgs ("--backend" : b : rest) o = parseArgs rest o { backend = if map toLower b == "sv" then Sv else None }
parseArgs ("--stall" : n : rest) o = parseArgs rest o { stall = read n }
parseArgs ("--optimize" : n : rest) o = parseArgs rest o { optimize = read n }
parseArgs ("--max-threads-default" : n : rest) o = parseArgs rest o { maxThreadsDefault = read n }
parseArgs ("--max-threads-limit" : n : rest) o = parseArgs rest o { maxThreadsLimit = read n }
parseArgs (f : rest) o = parseArgs rest o { files = files o ++ [f] }

-- | `--roots-only` emits the trees of the files named on the command line and
-- nothing else.
--
-- `frontend` needs the whole program — `getSymbols` is over every parsed file,
-- and dropping the imports is what made this driver answer questions about a
-- program with no imports for weeks — so the closure is still parsed and still
-- run through the passes; only the *printing* is filtered. That matters because
-- `--notes` on one library file prints 11 MB, ~10x more than a provenance
-- differential needs, and cost scales with the closure rather than the file.
run :: Mode -> Bool -> (Int, Int, Int) -> Maybe [WarningState WarningKind] -> ParseOptions -> IO ()
run mode rootsOnly (passes, tpasses, titers) werror opt = do
    -- `execStateT`, not `evalStateT`, and this was wrong until it was checked
    -- against `app/Main.hs:69`. `parseProgram` *returns* the command-line roots
    -- and *accumulates* every file it parsed — imports included — in the state.
    -- Taking the return value hands `frontend` a program with no imports, so
    -- `undefinedSymbols` reports every library symbol missing and no
    -- cross-module symbol question can be answered. Every whole-program
    -- conclusion drawn from this tool before this line changed is suspect.
    parsedFiles <- flip execStateT [] $ parseProgram opt
    roots <- mapM canonicalizePath (files opt)
    let results = map snd parsedFiles
        -- `parseProgram`'s state is `(canonicalPath, result)` in parse order
        -- (`Parser/IO.hs:95`), and `frontend` is a length- and order-preserving
        -- map over the same list (`Frontend.hs:35-38`), so the paths of the
        -- `Right` results line up with the trees the passes return.
        rootAt = map ((`elem` roots) . fst) [p | p@(_, Right _) <- parsedFiles]
        keep xs | rootsOnly = map snd $ filter fst $ zip rootAt xs
                | otherwise = xs
    case lefts results of
        -- The marker line stays first so anything matching on it still works;
        -- the bundle follows. Printing only the marker cost a week: 102 of the
        -- 551 `--strict` failures were known solely as "the oracle rejects it"
        -- until a survey rebuilt this driver to find out what it actually said.
        errs@(_ : _) -> do
            hPutStrLn stderr "PARSE-ERROR"
            mapM_ (hPutStrLn stderr . errorBundlePretty) errs
        [] -> let parsed = map fst (rights results) in case mode of
            -- `Main.hs:82`: the reference appends the whole program into one
            -- tree before collecting, so an error in an imported module is
            -- reported once for the program, not once per file.
            -- `Main.hs:76,82` appends the program into one tree before
            -- collecting; `append` is defined in the reference's executable,
            -- not its library, and `errors` is a traversal, so collecting per
            -- tree and concatenating gives the same set.
            Errors -> do
                let trees = frontend passes tpasses titers parsed
                bundles <- makeErrorBundles readFileUtf8 $ nub $ concatMap (errors . unnoteType) trees
                mapM_ (putStr . errorBundlePretty) bundles
                -- `--Werror`: with no error, the reference prints every enabled
                -- warning and fails on the first (`app/Main.hs:93-96`), so the
                -- warnings are the errors a must-reject snippet is graded by.
                case werror of
                    Just state | null bundles -> do
                        ws <- makeErrorBundles readFileUtf8 $ concatMap (warnings (enabledWarnings state) . unnoteType) trees
                        mapM_ (putStr . errorBundlePretty) ws
                        if null ws then return () else putStrLn "Error 1: Warnings treated as errors"
                    _ -> return ()
            _ | passes == maxBound && tpasses == maxBound && titers == maxBound ->
                    mapM_ (emit mode) (keep parsed)
              -- `--typed` needs the note to *be* `Typed`, which only
              -- `frontend` produces — so it lives on this branch alone, and
              -- an unprocessed parse degrades to `--notes` in `emit`.
              | TypedNotes <- mode ->
                    mapM_ (putStrLn . typedNoted) (keep $ frontend passes tpasses titers parsed)
              | otherwise ->
                    mapM_ (emit mode) (keep $ frontend passes tpasses titers parsed)


-- | `--loop`: what `instantiateTemplates` left and how
-- many rounds it took, for `cargo xtask loop-left`.
--
-- `left SCOPE|QUALIFIER|NAME` for every `TemplateInstance` the tree after
-- pass 8 still holds outside every `Template` — across the whole program,
-- imports included, in pre-order — each spelled by the reference's own
-- `getScope`, `getQualifier` and `getName` (`Parser/Syntax.hs:666-760`).
-- Then `rounds R`: the smallest `--template-iterations R` whose tree equals
-- the fixpoint's. `instantiateTemplates` (`Template.hs:52-57`) counts its
-- budget down one per round that decided something, and at 0 runs the same
-- pass list and `deduplicateInstances` as the round that decides nothing,
-- so the tree at `R` is the fixpoint's exactly when the fixpoint made `R`
-- rounds or fewer — monotone, so a search finds it. `Note`'s `Eq` ignores
-- the note (`Parser/Syntax.hs:573-575`): the trees compare as trees.
-- Nothing here changes what the frontend computes. No hint from the port's
-- own count: it would enter the oracle cache's key, and every change that
-- moved a round count would run the reference again.
loop :: ParseOptions -> IO ()
loop opt = do
    parsedFiles <- execStateT (parseProgram opt) []
    case lefts (map snd parsedFiles) of
        errs@(_ : _) -> do
            hPutStrLn stderr "PARSE-ERROR"
            mapM_ (hPutStrLn stderr . errorBundlePretty) errs
        [] -> do
            let parsed = map fst (rights (map snd parsedFiles))
                full = frontend 8 maxBound maxBound parsed
                reaches n = frontend 8 maxBound n parsed == full
            mapM_ (putStrLn . ("left " ++)) (concatMap leftInstances full)
            putStrLn $ "rounds " ++ maybe ">64" show (smallest reaches)
  where
    -- The smallest `n` in [0, 64] with `reaches n`.
    smallest reaches = search 0 1
      where
        -- `lo` fails (or is 0 and untested), `hi` is the next probe.
        search lo hi
            | hi > 64 = if reaches 64 then bisect lo 64 else Nothing
            | reaches hi = if lo == 0 && reaches 0 then Just 0 else bisect lo hi
            | otherwise = search hi (hi * 2)
        -- `lo` fails, `hi` reaches.
        bisect lo hi
            | hi - lo <= 1 = Just hi
            | reaches mid = bisect lo mid
            | otherwise = bisect mid hi
          where mid = (lo + hi) `div` 2

leftInstances :: NotedExp n e -> [String]
leftInstances (NotedExp _ e) = case e of
    TemplateF{} -> []
    TemplateInstanceF a _ -> label a : concatMap leftInstances (toList e)
    _ -> concatMap leftInstances (toList e)
  where
    label x = intercalate "::" (map toString (getScope x)) ++ "|" ++ intercalate "::" (map toString (getQualifier x)) ++ "|" ++ getName x

-- | Polymorphic in the note, because `frontend` re-notes the tree as
-- `Typed n` (`Frontend.hs:35`) and neither renderer looks at the note.
emit :: (Show e, Show n) => Mode -> NotedExp n e -> IO ()
emit Tree  e = putStrLn $ tree e
emit Notes e = putStrLn $ noted e
emit TypedNotes e = putStrLn $ noted e
emit _     e = putStrLn $ renderString $ layoutPretty defaultLayoutOptions $ prettySource 4 e

-- | Like `tree`, but keeps the note. `--tree` elides it, which makes provenance
-- unfalsifiable — the one thing the pass 8 design turns on.
noted :: (Show e, Show n) => NotedExp n e -> String
noted = unRaw . cata (\(Note n e) -> Raw (show e ++ " @" ++ show n))

-- | `noted` plus the note's `typeof`, rendered by the reference's own
-- `showPretty` between US/RS delimiters no type can contain — the read-back's
-- type-agreement gate parses these out and compares them against the types
-- the port read home over the synthetic file. The reference's pretty printer
-- and the port's `kg_ty::subtype::show_pretty` are two independent renderers;
-- the *types* are the compared quantity, the strings only the format.
typedNoted :: (Show e, Show n) => NotedExp (Typed n) e -> String
typedNoted = unRaw . cata render
  where
    render (Note n e) =
        Raw (show e ++ " @" ++ show n ++ (' ' : '\x1f' : showPretty (typeOf n)) ++ ['\x1e'])

-- | `warnings_state` as the reference's command line builds it
-- (`app/Options/CmdArgs.hs:111-136`): the default four enabled, then each
-- switch *appended* in order, `--Wall` contributing its two.
warningState :: [String] -> [WarningState WarningKind]
warningState argv = defaults ++ concat [ss | Just ss <- map warningSwitch argv]
  where
    defaults = [Enable RedundantImport, Enable Deprecated, Enable InitializerOverrides, Enable Shadow]

warningSwitch :: String -> Maybe [WarningState WarningKind]
warningSwitch "--Wall" = Just [Enable Conversion, Enable TransactionSizeMissing]
warningSwitch flag = lookup flag switches
  where
    kinds = [Conversion, Deprecated, InitializerOverrides, RedundantImport, Shadow, TransactionSizeMissing]
    switches = [("--" ++ enableSwitch k, [Enable k]) | k <- kinds]
            ++ [("--" ++ disableSwitch k, [Disable k]) | k <- kinds]
