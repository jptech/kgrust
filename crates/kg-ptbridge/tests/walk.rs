//! The walk over whole programs, read off the trace backend — for what the
//! corpus-wide `cargo xtask ptdiff` finds and a unit cannot pin.

use std::sync::{Arc, Mutex};

use kg_base::Utf8PathBuf;

/// The closure library's two modules as `library/data/closure*.k` declare
/// them, cut to the members these programs use.
const CORE: &str = "module data.closure.core\n    { Closure\n    , make_capture0\n    }\n\ntemplate <typename FunctionType, FunctionType fn, typename Capture>\nusing Closure = Capture;\n\ninline auto make_capture0\n    (\n    )\n{\n    struct Capture\n    {\n    }\n\n    Capture capture;\n\n    return capture;\n}\n";
const CLOSURE: &str = "module data.closure\n    { module data.closure.core\n    , make_capture1\n    }\n\nimport data.closure.core\n\ntemplate\n    < typename T0\n    >\ninline auto make_capture1\n    ( T0 x0\n    )\n{\n    struct Capture\n    {\n        T0 x0;\n    }\n\n    Capture capture =\n        { x0\n        };\n\n    return capture;\n}\n";

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("no panics hold this lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The trace of `main.k`'s program, the two closure modules beside it.
fn walk(main: &str) -> String {
    walk_to(main).0
}

/// [`walk`], and the reason the walk declined, if it did.
fn walk_to(main: &str) -> (String, Option<String>) {
    walk_with(&[], main)
}

/// `main.k`'s program under `/proj`, the closure modules and `extra` —
/// each `(path, text)` — beside it.
fn program(extra: &[(&str, &str)], main: &str) -> (kg_db::Db, kg_base::FileId) {
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = kg_db::Db::new(settings);
    db.set_file_text(&Utf8PathBuf::from("/proj/data/closure/core.k"), CORE.to_owned());
    db.set_file_text(&Utf8PathBuf::from("/proj/data/closure.k"), CLOSURE.to_owned());
    for (path, text) in extra {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{path}")), (*text).to_owned());
    }
    let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
    (db, root)
}

/// A trace backend writing into a buffer, and a way to read the buffer.
fn trace_backend() -> (kg_ptbridge::trace::Trace, impl Fn() -> String) {
    let buf = SharedBuf::default();
    let trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
    (trace, move || String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned())
}

/// [`walk_to`] with more modules under `/proj`, each `(path, text)`.
fn walk_with(extra: &[(&str, &str)], main: &str) -> (String, Option<String>) {
    let (db, root) = program(extra, main);
    let (mut trace, text) = trace_backend();
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    let outcome = kg_ptbridge::program::run(&db, &[root], options, None, &mut trace);
    trace.flush();
    assert!(outcome.initialised);
    (text(), outcome.declined.map(|d| d.reason))
}

/// `kg build` walks into a `Recording`, and the backend worker replays its
/// bytes into the real library (`kg_ptbridge::isolate`). Replayed into a
/// `Trace` instead, the text is the directly walked trace's byte for byte:
/// every argument kind survives the lift, the encoding and the inverse —
/// a string byte that is not UTF-8 (`é` reaches C as the one byte `0xe9`,
/// `ca_bytes`), a float, scopes, locations, the options — and each node id
/// maps to what its own call returned, `ParseAppendList`'s aliasing
/// included. Red with `str_arg` reading its bytes as UTF-8, or the replay
/// mapping `n<id>` to the call before.
#[test]
fn a_recording_replayed_into_a_trace_is_the_walk_traced() {
    let main = "module m\n{\nGo\n}\n\nstruct S\n{\nuint32 a;\nfloat32 b;\n}\n\ninline uint32 Twice(uint32 x)\n{\nreturn x * 2;\n}\n\ninline void Go()\n{\nS s = {1, 1.5};\nuint32 y = Twice(s.a);\nprint(\"caf\u{e9}\\n\");\n}\n";
    let (db, root) = program(&[], main);
    let codegen = kg_ptbridge::program::CodegenArgs {
        target: "Sv".to_owned(),
        output: "main".to_owned(),
        resource_usage: String::new(),
        dgml: String::new(),
        dgml_detailed: String::new(),
    };
    let options = || {
        let mut o = kg_backend_sys::options::OptionValues::harness_defaults();
        o.mem_to_reg = 0.1;
        o.cmd_args = "kg build main.k".to_owned();
        o
    };

    let (mut direct, direct_text) = trace_backend();
    let outcome = kg_ptbridge::program::run(&db, &[root], options(), Some(&codegen), &mut direct);
    direct.flush();
    assert!(outcome.declined.is_none(), "{:?}", outcome.declined.map(|d| d.reason));

    let mut recording = kg_ptbridge::record::Recording::new();
    kg_ptbridge::program::run(&db, &[root], options(), Some(&codegen), &mut recording);
    let bytes = recording.encode();
    let decoded = kg_ptbridge::record::Recording::decode(&bytes).expect("decodes");
    assert_eq!(decoded, recording, "the encoding is lossless");

    let (mut replayed, replayed_text) = trace_backend();
    let replay = decoded.replay(&mut replayed, |_, _| {}).expect("replays");
    replayed.flush();
    assert_eq!(replay, kg_ptbridge::record::Replay { initialised: true, codegen: Some(true) });
    let (direct_text, replayed_text) = (direct_text(), replayed_text());
    assert!(direct_text.contains("\"caf\\xe9"), "the program reaches the string: {direct_text}");
    assert!(direct_text.contains("ParseAppendList"), "and a list");
    assert_eq!(replayed_text, direct_text);
}

/// A recording cut short, or with a byte after its last call, is refused
/// rather than replayed as far as it goes.
#[test]
fn a_damaged_recording_is_refused() {
    let (db, root) = program(&[], "inline void main()\n{\n}\n");
    let mut recording = kg_ptbridge::record::Recording::new();
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    kg_ptbridge::program::run(&db, &[root], options, None, &mut recording);
    let bytes = recording.encode();
    assert!(kg_ptbridge::record::Recording::decode(&bytes).is_ok());
    assert!(kg_ptbridge::record::Recording::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(kg_ptbridge::record::Recording::decode(&longer).is_err());
}

/// The trace's calls without their sequence numbers and locations, one per
/// line — what two runs of one program agree on.
fn calls(trace: &str) -> Vec<String> {
    trace
        .lines()
        .filter_map(|l| l.splitn(3, ' ').nth(2))
        .filter(|c| !c.starts_with("SetLocation2") && !c.starts_with("UnknownLocation"))
        .map(str::to_owned)
        .collect()
}

/// The call right after the first one starting with `head`, without the
/// node it returns.
fn after<'a>(calls: &'a [String], head: &str) -> Option<&'a str> {
    let i = calls.iter().position(|c| c.starts_with(head))?;
    calls.get(i + 1).and_then(|c| c.split(" -> ").next())
}

/// `withUnmangledName` (`ParseTree.hs:177-180`) names a declaration only
/// when its type is an instance: the lambda hoisted out of `Do<uint16>`'s
/// copy is `Do<|$uint@16|>@?lambda0`, a function whose name merely holds a
/// copy's, and the reference's `ParseFunction` passes no unmangled name
/// for it (`lambdas.k::25`, the recorded walk, 2026-09-19). Red with the
/// walk reading any `<|` in a name as an instance: `"Do<uint16>@?lambda0"`.
#[test]
fn a_function_hoisted_out_of_a_copy_has_no_unmangled_name() {
    let main = "import data.closure\n\ntemplate <typename T>\ninline void Do(T a)\n{\nconst auto fn = [a](T x) -> T\n{\nreturn x - a;\n};\nfn(10);\n}\n\ninline void Go()\n{\nDo<uint32>(10);\nDo<uint16>(10);\n}\n";
    let trace = walk(main);
    assert!(trace.contains("ParseIdentifier \"Do<|$uint@16|>@?lambda0\""), "the hoisted function is walked");
    assert!(!trace.contains("\"Do<uint16>@?lambda0\""), "and has no unmangled name");
}

/// The same rule where the name *ends* in a copy's: the wrapper over
/// `is_valid<uint32>` is the function `main@$wrapper$is_valid<|$uint@32|>`,
/// typed a function, and no instance — so no unmangled name either
/// (scratchpad/t8/, 2026-09-19). Red with the walk trusting the name's
/// shape over the declaration's type.
#[test]
fn a_wrapper_over_a_copy_has_no_unmangled_name() {
    let main = "import data.closure.core\n\ntemplate <typename T>\ninline bool is_valid(T x)\n{\nreturn true;\n}\n\ntemplate <typename U>\ninline bool Go((uint32) -> bool Do, U x)\n{\nreturn Do(x);\n}\n\ninline void main()\n{\nbool b = Go(is_valid<uint32>, 10);\n}\n";
    let trace = walk(main);
    assert!(trace.contains("ParseIdentifier \"main@$wrapper$is_valid<|$uint@32|>\""), "the wrapper is walked");
    assert!(!trace.contains("\"main@$wrapper$is_valid<uint32>\""), "and has no unmangled name");
}

/// `bytesizeof(S3)` is `2` to the reference (`constexpr.k:101`): S3's
/// last field is `int<16 - bitsizeof(S1)>`, which the fixpoint folds the
/// round `S1` is typed, and S3 has a width the round after
/// (`Inference.hs:47-50`). The literal wears `bitsizeof`'s `TUnresolved`
/// (`Type.hs:749-750`), so the next round types it by itself:
/// `inferIntLiteralType`'s `uint2` (`Type.hs:485-491`). And `const auto`
/// over a `bitsizeof` is the literal's type, the operand being a struct,
/// no constant (`constexpr.k:105`). Red with the type table's fold unable
/// to read a width (the array declines at its dimension); with `bitsizeof
/// S3`'s operand counted as a typed leaf (the `const auto` declines); with
/// the folded `bitsizeof(S1)` operand of `16 - …` read as unresolved.
#[test]
fn a_size_folds_through_a_struct_whose_field_is_sized_by_one() {
    let main = "struct S1\n{\nint3 x;\n}\n\nstruct S3\n{\nS1 x;\nint<16 - bitsizeof(S1)> q;\n}\n\nusing A1 = S3[bytesizeof(S3)];\n\nconst auto S3_size = bitsizeof S3;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert_eq!(after(&calls, "ParseDecimalLiteral \"2\""), Some("ParseUintType 2"), "{calls:#?}");
    assert!(calls.iter().any(|c| c.starts_with("ParseArrayType")), "the dimension folded");
    assert!(calls.iter().any(|c| c.starts_with("ParseDecimalLiteral \"16\"")), "S3_size folded");
    assert_eq!(after(&calls, "ParseDecimalLiteral \"16\""), Some("ParseUintType 5"), "{calls:#?}");
}

/// A member initializer above the member it names binds what the parser
/// could see there: `Pick` is not yet declared in `P`, so `findSymbol`
/// finds the enclosing `Tests::Pick` and `makeQualified` writes it so
/// (`Parser.hs:686-688`); pass 1 keeps the qualifier, `P` holding a
/// `Pick` of its own (`Frontend.hs:160-167`), and pass 16 spells the
/// function by its scope (`Frontend.hs:273-283`). The copy `P<|@3|>`
/// keeps `Tests::Pick` — `risc-v.k`'s `ProgramTest` and its
/// `.custom_decode = CustomDecode` (the recorded walk, #394963). `Other`,
/// found nowhere at parse time, resolves to the copy's own member. Red
/// with pass 1 asked again at walk time, whose table has never held the
/// copy's scope: `Tests::P<|@3|>::Pick`.
#[test]
fn a_member_initializer_keeps_the_binding_the_parser_gave_it() {
    let main = "template <auto M>\nclass Core\n{\npublic:\n(uint8)->uint8 cb;\n(uint8)->uint8 cb2;\n}\n\nclass Tests\n{\ninline uint8 Pick(uint8 x)\n{\nreturn x;\n}\n\ntemplate <auto N>\nclass P\n{\nprivate:\nCore<N> core = {.cb = Pick, .cb2 = Other};\n\ninline uint8 Pick(uint8 x)\n{\nreturn x + N;\n}\n\ninline uint8 Other(uint8 x)\n{\nreturn x;\n}\n}\n\nP<3> p;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let name = |field: &str| -> Vec<&str> {
        // The last mention: `Core<|@3|>` declares the field first.
        let i = calls.iter().rposition(|c| c.starts_with(&format!("ParseIdentifier \"{field}\""))).expect("the designator is walked");
        calls[i + 1..].iter().take_while(|c| !c.starts_with("ParseScopedIdentifier")).filter_map(|c| c.strip_prefix("ParseIdentifier ")).map(|c| c.split(" -> ").next().unwrap_or(c)).collect()
    };
    assert_eq!(name("cb"), ["\"Tests\"", "\"Pick\""], "{calls:#?}");
    assert_eq!(name("cb2"), ["\"Tests\"", "\"P<|@3|>\"", "\"Other\""], "{calls:#?}");
}

/// A copy keeps every node's note (`desugarWithNote`, `Desugar.hs:91-94`:
/// `n' <> n` takes the copied node's type). `keep`'s `auto count` makes
/// its instance a template again (`abbreviatedFunctionTemplate`), copied
/// once more for `count`'s type; the deduced `T` is `expFromType`'s
/// `tuple2<|…|>[2]` whose element is *noted* `pair<uint32, uint32>`
/// (`Type.hs:1631,1638,1647`), and the second copy must carry that note
/// into `memory<T, 32>`'s argument for the instance to be shown through
/// the alias (`memory/pipelined.k`'s `pipelined_last`, the recorded walk,
/// #38677). Red with the copy walk's nodes minted untyped: re-inferred
/// from their text, `memory<data.tuple::tuple2<uint32, uint32>[2], 32>`.
#[test]
fn a_copy_of_a_copy_keeps_the_deduced_argument_s_note() {
    let tuple = "module data.tuple\n    { tuple2\n    , pair\n    }\n\ntemplate <typename T1, typename T2>\nstruct tuple2\n{\n    T1 first;\n    T2 second;\n}\n\ntemplate <typename T1, typename T2>\nusing pair = tuple2<T1, T2>;\n";
    let memory = "module data.memory\n    { memory\n    }\n\ntemplate <typename T, auto N>\nusing memory = [[memory]] T[N];\n";
    let main = "import data.closure\nimport data.tuple\nimport data.memory\n\ntemplate <typename T>\ninline T keep(auto count, (uint8) -> T body)\n{\n    static memory<T, 32> m;\n    m[0] = body(count);\n    return m[0];\n}\n\ninline uint32 go()\n{\n    auto r = keep(4, [](uint8 x) -> pair<uint32, uint32>[2]\n    {\n        pair<uint32, uint32>[2] v;\n        v[0].first = x;\n        return v;\n    });\n    return r[0].first;\n}\n";
    let (trace, declined) = walk_with(&[("data/tuple.k", tuple), ("data/memory.k", memory)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let typedefs: Vec<&String> = calls.iter().filter(|c| c.starts_with("ParseTypedef") && c.contains("\"memory<")).collect();
    assert!(!typedefs.is_empty(), "{calls:#?}");
    for t in typedefs {
        assert!(t.contains("\"memory<data.tuple::pair<uint32,\\x20uint32>[2],\\x2032>\""), "{t}");
    }
}

/// `Do<a && b>(10)` where the parser holds `Do` as the `else` branch's
/// variable is no call: without a template at `findSymbol`, no arguments
/// are parsed (`Parser.hs:669-680`), and the text is the chain `(Do < a) &&
/// (b > 10)` (`dependent-template.k::16`, the recorded walk). `Go` then
/// returns a `bool` for `C<uint3, 13, 14>`; the `template` keyword keeps
/// the other branch a call. Red with the committed tree kept: the walk
/// declines at `deduce: Unresolved`.
#[test]
fn a_call_on_a_name_that_is_no_template_is_a_relational_chain() {
    let main = "template <typename T, auto a, auto b>\nclass C\n{\npublic:\n    static if (bitsizeof T > 10)\n    {\n        template <auto N>\n        uint32 Do(uint32 x)\n        {\n            return x;\n        }\n    }\n    else\n    {\n        uint32 Do;\n    }\n\n    auto Go()\n    {\n        static if (bitsizeof T > 10)\n        {\n            return template\n                Do<a && b>(10);\n        }\n        else\n        {\n            return\n                Do<a && b>(10);\n        }\n    }\n}\n\ninline uint32 Run1()\n{\n    static C<uint32, true, false> obj1;\n    return obj1.Go();\n}\n\ninline bool Run2()\n{\n    static C<uint3, 13, 14> obj2;\n    return obj2.Go();\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    // `Do < 13` under `&&`; `14 > 10` folds (`Constexpr.hs:184-202`).
    let ops: Vec<&str> =
        calls.iter().filter_map(|c| c.strip_prefix("ParseBinaryOp ")).map(|c| c.split(' ').next().unwrap_or(c)).collect();
    assert_eq!(ops.len(), 2, "{calls:#?}");
}

/// `withUnmangledName` asks an alias's type (`ParseTree.hs:64,177-178`):
/// `using C_t = C<2>` is typed as the instance and hands the backend
/// `"C<2>"` (`constexpr.k:131`, the recorded walk). Red with the alias
/// asked about its own name, which is no instance's.
#[test]
fn an_alias_of_an_instance_names_it() {
    let main = "template <auto N>\nclass C\n{\npublic:\nconst uint32 y = N;\n}\n\nusing C_t = C<2>;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let typedef = calls.iter().find(|c| c.starts_with("ParseTypedef")).expect("the alias is walked");
    assert!(typedef.contains("\"C<2>\""), "{typedef}");
}

/// The backend reads an array's dimension as a number; one the fold
/// missed is a crash there, where the reference's frontend has already
/// rejected the program ("Array dimension must be a positive integer",
/// `constexpr.k::166`, a non-`const` member) or folded it. The walk
/// declines instead. Red with the dimension walked as written.
#[test]
fn an_array_dimension_that_did_not_fold_declines() {
    let main = "class Foo\n{\nprivate:\nuint8[Size] a;\npublic:\nauto Size = 32;\n}\n";
    let (_, declined) = walk_to(main);
    assert_eq!(declined.as_deref(), Some("fold:array dimension"));
}

/// `eval (ArrayF x y z) | any isEnum $ unfix z` makes each natural
/// dimension an untyped literal (`Constexpr.hs:154-157`), typed by the next
/// round as a bare literal: `bool[E::Count]` is `ParseDecimalLiteral "1"`,
/// `uint1` (`enum.k::39`'s recorded walk, `4 : uint3`), and so is a copy's
/// `bool[N]` with `N` bound to that constant (`constexpr.k::121`), which
/// the walk declined as unfolded. Red with the enum rule cut (an enum
/// value walked) or with the copy's `EnumValue` left unread (a decline).
#[test]
fn an_enum_dimension_is_walked_as_its_value() {
    let lib = "module lib\n    { E\n    }\n\nenum E : uint5\n{\n    X,\n    Count\n}\n";
    for main in [
        "import lib\n\ninline void main()\n{\n    bool[E::Count] x;\n}\n",
        "import lib\n\ntemplate <auto N>\nusing Foo = bool[N];\n\ninline void main()\n{\n    Foo<E::Count> x;\n    const auto y = x;\n}\n",
    ] {
        let (trace, declined) = walk_with(&[("lib.k", lib)], main);
        assert_eq!(declined, None, "the walk goes through: {main}");
        let calls = calls(&trace);
        let array = calls.iter().rposition(|c| c.starts_with("ParseArrayType")).expect("the array is walked");
        let literal = calls[..array].iter().rposition(|c| c.starts_with("ParseDecimalLiteral")).expect("a literal dimension");
        assert!(calls[literal].starts_with("ParseDecimalLiteral \"1\" "), "{:?}", &calls[literal..=array]);
        assert!(!calls[literal..array].iter().any(|c| c.starts_with("ParseEnumValue")), "{:?}", &calls[literal..=array]);
        assert!(calls[literal + 1].starts_with("ParseUintType 1 "), "{:?}", &calls[literal..=array]);
    }
}

/// `interpolatedStringPre`'s echo (`Frontend.hs:236-237`) is `showPretty`
/// of the hole as pass 1 leaves it, before any fold or instance: `{2+2=}` is
/// `(2 + 2) = `, not the folded `4 = ` (`test/logic/string.k`'s own
/// assertion), `bitsizeof val` keeps its operator, a call of a function
/// template is spelled as written, not as its copy (`test/library/data/
/// bits.k`, the recorded walk), and an enum constant keeps the qualifier
/// `redundantNameScope` leaves it (`kg-oracle --passes 1`). A shape the
/// printer does not spell — a cast — declines. Red with the echo taken
/// after the loop (`4 = `), or with the operator arms cut (a decline).
#[test]
fn an_echoed_hole_is_the_parsers_expression() {
    let main = |hole: &str| {
        format!("const uint32 val = 3840;\n\nenum E : uint8\n{{\n    A,\n    B\n}}\n\ntemplate <typename T>\ninline bool nor(T x)\n{{\n    return x == 0;\n}}\n\ninline void main()\n{{\n    uint8 x = 3;\n    const auto s = \"{{{hole}=}}\";\n}}\n")
    };
    let cases = [
        ("2+2", "(2 + 2) = "),
        ("bitsizeof val", "bitsizeof val = "),
        ("nor(x)", "nor(x) = "),
        ("-val", "-val = "),
        ("E::A", "E::A = "),
    ];
    for (hole, echoed) in cases {
        let (trace, declined) = walk_to(&main(hole));
        assert_eq!(declined, None, "{hole}");
        let calls = calls(&trace);
        let literal = format!("ParseStringLiteral \"{}\"", echoed.replace(' ', "\\x20"));
        let strings: Vec<_> = calls.iter().filter(|c| c.starts_with("ParseStringLiteral")).collect();
        assert!(calls.iter().any(|c| c.starts_with(&literal)), "{hole}: no {literal} in {strings:?}");
    }
    let (_, declined) = walk_to(&main("cast<uint8>(x)"));
    assert_eq!(declined.as_deref(), Some("interpolation hole: echo"));
}

/// An integer attribute over an import: `max_threads(C_t::x)` is `32` to
/// the reference (`constexpr.k::126`, the recorded walk), and the walk now
/// reads the IR inference's fold — `evaluateConstexpr` qualifies `C_t::x`
/// by the class the alias names (`Constexpr.hs:159-160`) and folds the
/// named constant across modules — so the attribute is walked as `32`.
/// This test once expected a decline: the port's fold was one file wide,
/// and a name walked as written crashed the backend. Red with the fold not
/// read (a decline) or the name walked as written.
#[test]
fn an_attribute_folds_across_modules() {
    let lib = "module lib\n    { C\n    , C_t\n    }\n\ntemplate <auto N>\nclass C\n{\npublic:\nconst uint32 y = 16;\nconst uint32 x = 2 * y;\n}\n\nusing C_t = C<2>;\n";
    let main = "import lib\n\nclass Main\n{\n[[max_threads(C_t::x)]]\nvoid main()\n{\n}\n}\n";
    let (trace, declined) = walk_with(&[("lib.k", lib)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let attr = calls.iter().position(|c| c.starts_with("ParseIntAttribute 4")).expect("max_threads");
    assert!(calls[attr - 3].starts_with("ParseDecimalLiteral \"32\""), "{:?}", &calls[attr - 3..=attr]);
}

/// `typ (TInstance t _ _) = typ t` (`Type.hs:1638`): `mk`'s `auto` return
/// is `Box<uint8>`, spelled as the copy it stands for, where the walk
/// declined (`deduce: Instance`, 52 corpus programs). And `checkVarDecl`'s
/// `reduceClosure` (`TypeCheck.hs:129,146-148`): a global initialized with
/// a closure that captures nothing is initialized with `{}` — the
/// reference's walk of `const auto add_and_increment = compose2(…)`
/// (`higher-order-functions.k::0`), which the backend otherwise refuses
/// ("function calls must be contained inside of functions"). The program
/// compiles clean under `kg-oracle --errors` (scratchpad/t15/). Red with
/// the instance arm gone (the walk declines) and with the initializer
/// walked as written.
#[test]
fn an_instance_is_spelled_as_its_copy_and_a_global_closure_is_braces() {
    let main = "import data.closure\n\nconst auto fn = [](uint32 x) -> uint32\n{\nreturn x;\n};\n\ntemplate <typename T>\nstruct Box\n{\nT v;\n}\n\ninline auto mk()\n{\nBox<uint8> b;\nreturn b;\n}\n\nclass Main\n{\npublic:\nvoid main()\n{\nauto b = mk();\n}\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let at = calls.iter().position(|c| c.starts_with("ParseIdentifier \"fn\"")).expect("the global is walked");
    let next: Vec<&str> = calls[at + 1..at + 3].iter().map(|c| c.split(" -> ").next().unwrap_or("")).collect();
    assert_eq!(next, ["ParseBaseList _", &*format!("ParseInitializerList {}", calls[at + 1].rsplit(' ').next().unwrap_or(""))]);
}

/// `bool x = Fn_T_auto(false)` on `h`'s `template <typename T> inline T
/// Fn_T_auto(auto x)`: round one makes `Fn_T_auto<|$bool|>` in `h`, `T`
/// from the declared type; its `auto x` makes it an abbreviated template
/// the round after, and the reference's next round deduces the call again,
/// against it — `Fn_T_auto<|$bool|><|$bool|>`, displayed by its first
/// instance, `"Fn_T_auto<bool>"` (`return-type-polymorphism.k::3`, the
/// recorded walk). Red with `main` given no round for it (the call's
/// deduced instance is never resolved: `template instance`), with the
/// resolve comparing the copy's alias to its name, and with the display
/// spelling both instances.
#[test]
fn a_call_to_a_copy_that_became_a_template_is_deduced_again() {
    let h = "module h { Fn_T_auto }\n\ntemplate <typename T>\ninline T Fn_T_auto(auto x)\n{\nreturn x;\n}\n";
    let main = "import h\n\ninline void main()\n{\nbool x = Fn_T_auto(false);\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.starts_with("ParseIdentifier \"Fn_T_auto<|$bool|><|$bool|>\"")), "{calls:#?}");
    let function = calls.iter().find(|c| c.starts_with("ParseFunction ") && c.contains("Fn_T_auto")).expect("the copy");
    assert!(function.contains("\"Fn_T_auto<bool>\""), "{function}");
}

/// A copy is named by its instance note (`withUnmangledName`,
/// `ParseTree.hs:176-179`), and a template named as its argument is noted
/// `TArg (showPretty e)` (`Type.hs:437`), whose printing leaves out the
/// module namespace the sorter qualified it with (`PrettyPrint.hs:146-148`)
/// — `Fn1<S1>`, as the reference's walk spells `FnFn<uint32, S1, Do>`
/// (`template-template-parameter.k::31`); `Fn1<h::S1>` read back from the
/// mangle was the wrong name. Red with the argument noted by its path.
#[test]
fn a_template_argument_is_named_as_written() {
    let h = "module h { S1, Fn1 }\n\ntemplate <typename T>\nstruct S1\n{\n    T x;\n}\n\n\
             template <template <typename> typename TT>\ninline void Fn1()\n{\n    static TT<uint32> x;\n}\n";
    let main = "import h\n\ninline void main()\n{\n    Fn1<S1>();\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let function = calls.iter().find(|c| c.starts_with("ParseFunction ") && c.contains("Fn1")).expect("the copy");
    assert!(function.contains("\"Fn1<S1>\""), "{function}");
}

/// `checkArraySize` rejects a dimension that is not positive before any
/// walk (`TypeCheck.hs:187-196`); `bool[I - 3]` in `Do<3>`'s copy is one
/// (`template.k::30`), which this frontend does not report inside a copy
/// yet, and which the backend compiles as `bool[0]`. The walk declines it;
/// an enum constant's dimension is still a number (`enum.k::39`). Red with
/// the dimension's sign unread, or with an enum's value unread.
#[test]
fn a_dimension_that_is_not_positive_declines_and_an_enum_one_does_not() {
    let zero = "inline void main()\n{\nbool[3 - 3] x;\n}\n";
    assert_eq!(walk_to(zero).1.as_deref(), Some("fold:array dimension"));
    let enumerated = "enum Bar : uint4\n{\nB4 = 4\n}\n\ninline void main()\n{\nbool[Bar::B4] x;\n}\n";
    assert_eq!(walk_to(enumerated).1, None);
}

/// `Go<Foo, {10, false}>()` on `h`'s `template <typename T, T x> inline T
/// Go()`: an aggregate is a value argument like a literal, decided and
/// mangled `@@10@False`, and its copy is made where the template lives —
/// the call named a copy nobody made, and the backend said "Undefined
/// function" (`struct-constexpr.k::0`). Red with a written aggregate read
/// as no value, or with the owner unable to mint one.
#[test]
fn an_aggregate_argument_is_minted_where_the_template_lives() {
    let h = "module h { Foo, Go }\n\nstruct Foo\n{\nuint8 a;\nbool b;\n}\n\ntemplate <typename T, T x>\ninline T Go()\n{\nreturn x;\n}\n";
    let main = "import h\n\ninline void main()\n{\nGo<Foo, {10, false}>();\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(
        calls.iter().any(|c| c.starts_with("ParseFunction ") && c.contains("\"Go<")),
        "the copy is declared: {calls:#?}"
    );
}

/// `fn(true)` over `const auto fn1 = Foo; const auto fn2 = fn1; const auto
/// fn = fn2;`: a
/// constant holding a function is its initializer (`Constexpr.hs:290`),
/// the fixpoint following the chain to `Foo`, and `checkVarDecl` leaves
/// nothing of the constants themselves (`TypeCheck.hs:139`) — the
/// reference's walk calls `Foo` and declares neither (`function-
/// constants.k::2`). The call named `fn1`, which the backend could not
/// find. Red with the chain followed one step, or with the constants
/// declared. Each constant is `SeqF []`, and stays an element of the
/// module's list — an empty list apiece (`function-constants.k::0,2`, the
/// recorded walks), red where the walk dropped them.
#[test]
fn a_call_through_a_chain_of_function_constants_calls_the_function() {
    let h = "module h { Foo }\n\ninline void Foo(bool b)\n{\n}\n";
    let main = "import h\n\nconst auto fn1 = Foo;\n\nconst auto fn2 = fn1;\n\nconst auto fn = fn2;\n\ninline void main()\n{\nfn(true);\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(!calls.iter().any(|c| c.starts_with("ParseIdentifier \"fn")), "{calls:#?}");
    let specifier = calls.iter().position(|c| c.starts_with("ParseFunctionSpecifier")).expect("the call");
    assert!(calls[..specifier].iter().rev().take(4).any(|c| c.starts_with("ParseIdentifier \"Foo\"")), "{calls:#?}");
    let direct = "import h\n\ninline void main()\n{\nFoo(true);\n}\n";
    let (plain, _) = walk_with(&[("h.k", h)], direct);
    let empties = |calls: &[String]| calls.iter().filter(|c| c.starts_with("ParseBaseList _")).count();
    assert_eq!(empties(&calls), empties(&crate::calls(&plain)) + 3, "one empty list per constant");
}

/// `trimExternClasses` (`Frontend.hs:315-327`): every function of a class
/// declared `extern` is `{ return; }`, or `{ return {}; }` for one with a
/// value — what it did is the other side's business. `Go(Do)` in an
/// extern class's method named a higher-order function nothing had made a
/// copy of, and the backend said "Undefined function" (`extern-class.k::53`).
/// Red with the class's functions walked as written, or with a value's
/// return given no `{}`.
#[test]
fn an_extern_classs_functions_are_trimmed_to_a_return() {
    let main = "class Foo\n{\npublic:\nvoid Do()\n{\nuint32 x = 3;\n}\nuint32 Get()\n{\nreturn 5;\n}\n}\n\nextern Foo;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls: Vec<String> = calls(&trace).into_iter().map(|c| c.split(" -> ").next().unwrap_or("").to_owned()).collect();
    let returns: Vec<&str> = calls.iter().filter(|c| c.starts_with("ParseReturn")).map(|c| c.split(' ').next().unwrap_or("")).collect();
    assert_eq!(returns, ["ParseReturn", "ParseReturnExpression"], "{calls:#?}");
    assert!(!calls.iter().any(|c| c.starts_with("ParseIdentifier \"x\"")), "Do's body is gone");
    let at = calls.iter().position(|c| c.starts_with("ParseReturnExpression")).expect("Get's return");
    assert!(calls[at - 1].starts_with("ParseInitializerList"), "{calls:#?}");
    // An `extern` in a decided `static if` the table did not type counts
    // the same (`extern-class.k::44`): the name resolves to the class.
    let in_static_if = "class Foo\n{\npublic:\nuint32 Get()\n{\n}\n}\n\nstatic if (true)\n{\nextern Foo;\n}\n";
    let (trace, declined) = walk_to(in_static_if);
    assert_eq!(declined, None, "the walk goes through");
    assert!(calls_of(&trace).iter().any(|c| c.starts_with("ParseReturnExpression")), "Get is trimmed");
    // And none that pass 7 does not see: in a branch not taken
    // (`extern-class.k::46`), in a template's body (`::48`), or under a
    // condition that needs an instance, which nothing has made yet
    // (`::49`) — `Get`, with no `return` written, keeps its empty body.
    let foo = "class Foo\n{\npublic:\nuint32 Get()\n{\n}\n}\n\n";
    for place in [
        "static if (false)\n{\nextern Foo;\n}\n",
        "template <typename T>\nclass Bar\n{\nextern Foo;\n}\n",
        "template <typename T>\nstruct S\n{\nT x;\n}\n\nstatic if (bitsizeof S<uint32> == 32)\n{\nextern Foo;\n}\n",
    ] {
        let (trace, _) = walk_to(&format!("{foo}{place}"));
        assert!(!calls_of(&trace).iter().any(|c| c.starts_with("ParseReturn")), "not trimmed: {place}");
    }
}

fn calls_of(trace: &str) -> Vec<String> {
    calls(trace)
}

/// `static C<E> c;` in `main`, `C` a template of `h`: the copy holds the
/// argument's own node (`resolveTypeParam`'s `argAt`, `TypeCheck.hs:
/// 334-339`), so `T x;` in `C<|@E|>` is `E` named from where it was
/// written — `ParseNamedType … ["main","$"]`, the scope the backend finds
/// `main`'s enum from (`type-identifier.k::9`). The owner minted the
/// argument under the template's own scope. Red with the written scope
/// not carried.
#[test]
fn a_type_argument_keeps_the_scope_it_was_written_in() {
    let h = "module h { C }\n\ntemplate <typename T>\nclass C\n{\nT x;\n}\n";
    let main = "import h\n\nenum E : uint32\n{\nOne = 1\n}\n\ninline void main()\n{\nstatic C<E> c;\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let x = calls.iter().position(|c| c.starts_with("ParseIdentifier \"x\"")).expect("the member");
    assert!(calls[x - 1].starts_with("ParseNamedType") && calls[x - 1].contains("[\"main\",\"$\"]"), "{:#?}", &calls[x - 4..=x]);
}

/// A member template of a class template, its instance decided inside the
/// class's copy: `Get<2>` in `Foo<uint8>`, made the round after. The
/// lowering qualified `return x;` through the template, `Foo::Get::x`,
/// and `instantiate` renames the chain's `Get` to the instance's name
/// where the chain spells the template (`Template.hs:135-137`) — a
/// comparison that, a round later, meets `Foo`'s copy by the print's
/// alias. Left unrenamed, the chain is no prefix of the use site's scope,
/// `resolveNames` spells it in full, and the backend said "Unknown symbol:
/// x" (`callbacks.k::44`). Red with the chain compared symbol by symbol.
#[test]
fn a_member_template_copys_parameter_is_named_bare() {
    let main = "template <typename T>\nclass Foo\n{\npublic:\ntemplate <auto N>\ninline uint32 Get(uint32 x)\n{\nreturn x;\n}\ninline uint32 Run()\n{\nreturn Get<2>(3);\n}\n}\n\nclass Main\n{\nFoo<uint8> foo;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let named: Vec<&[String]> = calls
        .windows(3)
        .filter(|w| w[0].starts_with("ParseIdentifier \"x\"") && w[2].starts_with("ParseScopedIdentifier"))
        .collect();
    assert!(!named.is_empty(), "x is named somewhere: {calls:#?}");
    assert!(named.iter().all(|w| w[1].starts_with("ParseBaseList n")), "x is bare wherever it is named: {named:#?}");
}

/// `Test<uint32, DoIt>` with `DoIt` from `h`: inside the copy, `Go1(Fn, x)`
/// passes the function on, and the route spells it through an alias for
/// its qualified name, `.h::DoIt` — which a call must name as the path it
/// stands for, `h::DoIt`, not as the alias's text: the backend said
/// "Undefined function: .helper.higher_order_functions::DoIt"
/// (`higher-order-functions.k::114`). Red with the alias minted without
/// its path, or the walk spelling the alias's text.
#[test]
fn a_function_named_by_its_qualified_alias_is_called_by_its_path() {
    let h = "module h { DoIt }\n\ninline uint32 DoIt(uint32 x)\n{\nreturn x;\n}\n";
    let main = "import data.closure\nimport h\n\ntemplate <typename T, (T) -> T Fn>\ninline uint32 Test(uint32 x)\n{\ninline uint32 Go1((T) -> T Do, uint32 x)\n{\nreturn Do(x);\n}\nreturn Go1(Fn, x);\n}\n\ninline void main()\n{\nTest<uint32, DoIt>(10);\n}\n";
    let (trace, _) = walk_with(&[("h.k", h)], main);
    let calls = calls(&trace);
    assert!(!calls.iter().any(|c| c.contains("\".h::DoIt\"")), "the alias's text is spelled: {calls:#?}");
}

/// A lambda capturing a closure: `foo`'s `[fn]() { fn(); }`, `fn` the
/// closure over `Run`'s lambda. Its `Closure` copy's display name spells
/// the capture's copy of `make_capture1` in full, and that copy's
/// argument — the inner closure — as the `Closure` copy it is, function
/// type and all: `unmangleQualifiedName` reads each through the symbol
/// map (`Type.hs:148-187`), where the mangled name holds no function type
/// (`Mangle.hs:42-43`). `static-if.k::47` and 120 programs beside it
/// stopped at this name. Red with the display read off the mangles alone,
/// or with a bare closure printed as `[capture] fn`.
#[test]
fn a_closure_copy_nested_in_a_display_name_is_spelled_from_its_instance() {
    let main = "import data.closure\n\ninline void bar(() -> void fn)\n{\n}\n\ninline void foo(() -> void fn)\n{\nbar([fn]()\n{\nfn();\n});\n}\n\ninline void Run()\n{\nfoo([](){});\n}\n";
    let (trace, _) = walk_to(main);
    let displays: Vec<String> = calls(&trace)
        .into_iter()
        .filter(|c| c.starts_with("ParseTypedef") && c.contains("make_capture1<"))
        .collect();
    assert!(!displays.is_empty(), "the nested copy is declared: {trace}");
    for d in &displays {
        let name = d.rsplit_once(" \"").map_or("", |(_, n)| n);
        assert!(name.contains("make_capture1<data.closure.core::Closure<("), "{d}");
        assert!(!name.contains("Closure<|") && !name.contains('['), "{d}");
    }
    // And `make_capture1`'s own copy, a function, by the same map.
    let copy = calls(&trace)
        .into_iter()
        .find(|c| c.starts_with("ParseFunction ") && c.contains("\"make_capture1<"))
        .expect("the capture's copy");
    assert!(copy.contains("\"make_capture1<data.closure.core::Closure<("), "{copy}");
}

/// A higher-order function with an `auto` parameter is a template the
/// route minted (`abbreviatedFunctionTemplate` over `higherOrderFunctions`,
/// `Function.hs:288-361`), and its copy is an instance though its nodes are
/// the rewrites', not written ones: `twice(inc)` makes `twice<|…|>` in `f`,
/// displayed `twice<…>` — where the walk gave it no name at all
/// (`higher-order-functions.k::0`'s `compose2<…>`, 105 programs). Red with
/// the display asked of the declaration's provenance.
#[test]
fn a_copy_of_a_minted_template_has_a_name() {
    let f = "module f { twice }\n\nimport data.closure\n\ninline auto twice((auto) -> auto f)\n{\nreturn [f](auto x)\n{\nreturn f(f(x));\n};\n}\n";
    let main = "import data.closure\nimport f\n\ninline uint32 inc(uint32 x)\n{\nreturn x + 1;\n}\n\ninline void main()\n{\nconst auto g = twice(inc);\n}\n";
    let (trace, _) = walk_with(&[("f.k", f)], main);
    let named = calls(&trace).into_iter().any(|c| c.starts_with("ParseFunction ") && c.contains("\"twice<"));
    assert!(named, "the copy is named: {trace}");
}

/// `Id(s)` in `main` on `h`'s `template <typename T> inline T Id(T x)`,
/// `s` a struct of `main`'s: the copy is made in `h`, and its argument is
/// the deduction's — `expFromType`'s `typeIdentifier [] s e`
/// (`Type.hs:1648`), a type named with no scope at all, where the owner
/// minted it under the template's (`["@h"]`). 108 programs diverged at
/// this in `data.closure`'s `make_captureN` copies, their argument a
/// deduced closure (`static-if.k::47`). Red with a deduced foreign
/// argument minted under the template's scope.
#[test]
fn a_deduced_argument_is_named_with_no_scope() {
    let h = "module h { Id }\n\ntemplate <typename T>\ninline T Id(T x)\n{\nreturn x;\n}\n";
    let main = "import h\n\nstruct S\n{\nuint8 a;\n}\n\ninline void main()\n{\nS s;\nS t = Id(s);\n}\n";
    let (trace, _) = walk_with(&[("h.k", h)], main);
    let calls = calls(&trace);
    let param = calls.iter().position(|c| c.starts_with("ParseFunctionParam") && c.contains("Id<|")).expect("the copy's parameter");
    let named = calls[..param].iter().rev().find(|c| c.starts_with("ParseNamedType")).expect("its type");
    assert!(named.contains(" [] "), "{named}");
}

/// `using type_t = count_t<4>;`, `count_t` an alias template: the alias's
/// type is the copy's instance in the reference (the mention reads the
/// copy's note), `"count_t<4>"` to the backend; here the table types it
/// through to what the copy aliases, and the walk gave it no name
/// (`higher-order-functions.k::0` and 97 beside it). Red with the alias's
/// body not consulted.
#[test]
fn an_alias_of_an_alias_templates_copy_names_it() {
    let main = "template <auto N>\nusing count_t = uint<N>;\n\nusing type_t = count_t<4>;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let typedef = calls.iter().rfind(|c| c.starts_with("ParseTypedef")).expect("type_t");
    assert!(typedef.contains("\"count_t<4>\""), "{typedef}");
}

/// A lambda in a member function that reads a member captures `this`
/// (`captureThis`), and its capture is `make_capture1<Foo&>`, made in
/// `data.closure` (`this-capture.k::0`, the recorded walk). The twin
/// prints the `ThisF` as `this`, which the table typed as nothing — the
/// reference types it through the class's `$this` (`Type.hs:1269`) — and
/// the owner could not mint a reference type (`typ (TReference s)`,
/// `Type.hs:1629`). Red with either. The copy's field is `Foo&`, so the
/// hoisted lambda's `auto $this = …x0` is declared `Foo&`: red with the
/// reference minted as its class, which the twin's printer spells as the
/// class, or with the alias it is minted as mangled by its text
/// (`make_capture1<|@Foo&|>`).
#[test]
fn a_captured_this_makes_its_capture() {
    let main = "import data.closure\n\nclass Foo\n{\npublic:\ninline void Run()\n{\nconst auto fn = []() -> void\n{\nauto x = _x;\n};\nfn();\n}\nuint8 _x;\n}\n\ninline void main()\n{\nstatic Foo foo;\nfoo.Run();\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.starts_with("ParseIdentifier \"make_capture1<|$ref@Foo|>\"")), "{trace}");
    let this = calls.iter().position(|c| c.starts_with("ParseIdentifier \"$this\"")).expect("the local");
    assert!(calls[this - 1].starts_with("ParseReference"), "{}", calls[this - 1]);
}

/// A class's member function given to a higher-order function in a lambda
/// is wrapped capturing `this` (`addLambdaWrappers`, `Function.hs:389,
/// 406-418`), hoisted as `C@Run@?lambda0@$wrapper$f`, and the capture the
/// hoisted lambda makes for it reads `this` — its own `$this` local
/// (`thisReference`, `Frontend.hs:453-456`): the reference walks a
/// `ParseNamedVariable` of `$this` there, and `ParseThis` only in `Run`
/// (`this-capture.k::9`, the recorded walk). Red with the walk's `ParseThis`
/// for every `this`, which the backend types as the global class.
#[test]
fn a_this_in_a_hoisted_lambda_is_its_this_local() {
    let main = "import data.closure\n\ninline void Foo(() -> void fn)\n{\n}\n\nclass C\n{\npublic:\ninline void f()\n{\n}\n\ninline void Run()\n{\nFoo([]()\n{\nFoo(f);\n});\n}\n}\n\ninline void main()\n{\nstatic C x;\nx.Run();\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.starts_with("ParseIdentifier \"C@Run@?lambda0@$wrapper$f\"")), "{trace}");
    let this: Vec<&String> = calls.iter().filter(|c| c.starts_with("ParseThis")).collect();
    assert_eq!(this.len(), 1, "{this:?}");
    assert!(this[0].contains("[\"C\",\"Run\",\"$\"]"), "{this:?}");
}

/// `Go(callback)` in a class template's lambda, `callback` a member of
/// function type: the wrapper is `$wrapper$callback`, capturing `this` as a
/// reference to the *copy* — `make_capture1<Foo<uint32>&>` — and `Go`'s own
/// copy is named after the closure over it. Every name below is read off the
/// reference's recorded walk (`this-capture.k::18`, `cargo xtask ptdiff
/// --dump`), which this port matches call for call through them.
///
/// It declined until 2026-09-19 ("call of a template", the route leaving
/// `Go` undeduced): the wrapper was built only in the *copy* and lost when
/// the next round rebuilt one, and the twin that would have decided the
/// capture did not parse — `moveFunctions` leaves the class qualifier on a
/// method callee and `$this.Foo<|$uint@32|>::callback()` is not a name any
/// parser reads back. Red with the member's written type unread (no wrapper
/// in the template), with the qualifier printed (no twin), or with the
/// qualifier compared as written (`redundant_qualifier`).
#[test]
fn a_member_of_a_class_template_is_wrapped_capturing_the_copy() {
    let main = "import data.closure\n\ninline void Go(() -> void Do)\n{\n}\n\ntemplate <typename T>\nclass Foo\n{\n() -> void callback;\n\npublic:\ninline void Run()\n{\nGo([]()\n{\nGo(callback);\n});\n}\n}\n\ninline void main()\n{\nstatic Foo<uint32> foo;\nfoo.Run();\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    for name in [
        "Foo<|$uint@32|>@Run@?lambda0@$wrapper$callback",
        "make_capture1<|$ref@Foo<|$uint@32|>|>",
        "make_capture1<|$ref@Foo<|$uint@32|>|>@$@Capture@Foo<|$uint@32|>@Run@?lambda0@$wrapper$callback|>",
    ] {
        assert!(trace.contains(name), "{name} is missing");
    }
    // The wrapper's body calls the member through `$this`, unqualified.
    assert!(!trace.contains("Foo<|$uint@32|>::callback"), "the callee keeps no class qualifier");
}

/// `static if (true)` in a class is decided at pass 6 (`staticControl` in
/// `inferType`, `Frontend.hs:69`, `Type/Inference.hs:50`), before pass 8
/// hoists the lambda that calls the member declared under it — so
/// `moveFunctions`' `memberAccess` finds `Do` in the class and the call is
/// `$this.Do()` (`this-capture.k::17`, the recorded walk): the hoisted
/// function reads its `$this` twice, declaring it and calling through it.
/// Red with the `static if` decided after the lambda has left the class,
/// or with its member keyed under the branch's `$quasi0`: a bare `Do()`.
#[test]
fn a_member_under_a_decided_static_if_is_called_through_this() {
    let main = "import data.closure\n\ninline void Go(() -> void fn)\n{\nfn();\n}\n\nclass Foo\n{\npublic:\ninline void Run()\n{\nGo([]()\n{\nDo();\n});\n}\n\nprivate:\nstatic if (true)\n{\ninline void Do()\n{\n}\n}\n}\n\ninline void main()\n{\nstatic Foo foo;\nfoo.Run();\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let this = calls(&trace).iter().filter(|c| c.starts_with("ParseIdentifier \"$this\"")).count();
    assert_eq!(this, 2, "{trace}");
}

/// A `static if` the loop decides and splices takes its branch's `$quasi0`
/// out of every scope under it and out of every name chain through it
/// (`removeNestedScope`, `Constexpr.hs:358-370`; the walk decides none of
/// its own since 2026-10-10): the reference declares `foo1` in
/// `["main","$"]` and names its type `main::$::Foo` (`static-if.k::14`,
/// the recorded walk). Red with the loop's splice keeping the branch's
/// scope — which the backend rejects ("Class/Memory local variables are
/// not supported").
#[test]
fn a_spliced_static_if_leaves_its_scope_behind() {
    let main = "inline void main()\n{\nstatic if (true)\n{\nstruct Foo\n{\nuint2 x;\n}\n\nFoo foo1;\n}\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(!calls.iter().any(|c| c.contains("\"$quasi0\"")), "{trace}");
    let declare = calls.iter().rfind(|c| c.starts_with("ParseDeclare")).expect("foo1");
    assert!(declare.contains(" 4 [\"main\",\"$\"]"), "{declare}");
}

/// A class's callback written through a function-type alias is the
/// function type `expFromType` spells ("middle-end doesn't handle function
/// type aliases", `TypeCheck.hs:141-142`), each parameter noted with its
/// own type, which an integer's node carries (`Type.hs:1655`,
/// `ParseTree.hs:53-54`) — `callbacks.k::0`, the recorded walk. Red with
/// the alias walked as a name, or with the parameter's node type unset.
#[test]
fn a_callback_through_an_alias_is_its_function_type() {
    let main = "using callback_t = (uint32 a) -> uint32;\n\nclass D\n{\nprivate:\ncallback_t callback1;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let declare = calls.iter().rposition(|c| c.starts_with("ParseDeclare")).expect("callback1");
    let ty = calls[..declare].iter().rposition(|c| c.starts_with("ParseFunctionType")).expect("a function type");
    assert!(calls[ty..declare].iter().all(|c| !c.starts_with("ParseNamedType")), "{:?}", &calls[ty..declare]);
    let param = calls[..ty].iter().rposition(|c| c.starts_with("ParseFunctionTypeParam")).expect("its parameter");
    assert!(calls[param + 1].starts_with("ParseUintType 32"), "{}", calls[param + 1]);
    assert!(calls[param + 2].starts_with("SetNodeType"), "{}", calls[param + 2]);
}

/// A class template given an attributed function type is copied — the
/// attributes minted as `expFromTAttr` spells them (`Type.hs:1662-1668`),
/// `same_types<|$fun@max_threads@2$uint@3$void…|>` — and the copy's
/// `ParseClass` carries its instance type, read off the note
/// `setTypeOfInstance` wrote (`ParseTree.hs:237-253`): the function-typed
/// argument NULL, an integer type by its width (`type-constexpr.k::16`,
/// the recorded walk). Red with an attributed function type opaque to the
/// census, or with the instance read off the mangled name, which cannot
/// split a `$fun` argument from the next.
#[test]
fn a_copy_over_an_attributed_function_type_carries_its_instance() {
    // The template in another module, so the owner mints the argument.
    let helper = "module helper.tc\n    { same_types\n    }\n\ntemplate <typename S, typename T>\nclass same_types\n{\n}\n";
    let main = "import helper.tc\n\ninline void main()\n{\nstatic same_types<[[max_threads(2)]] (uint3 x) -> void, uint4> test;\n}\n";
    let (trace, declined) = walk_with(&[("helper/tc.k", helper)], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.contains("same_types<|$fun@max_threads@2$uint@3$void$uint@4|>")), "{trace}");
    assert!(calls.iter().any(|c| c.starts_with("ParseTemplateArg \"S\" _")), "{trace}");
    let class = calls.iter().find(|c| c.starts_with("ParseClass")).expect("the copy");
    // `ParseClass name body instance …`: the instance is not NULL.
    assert_ne!(class.split(' ').nth(3), Some("_"), "{class}");
}

/// A `static for` whose body needs its variable as a constant is unrolled
/// by pass 6 (`Constexpr.hs:48-121`) — `$quasi1`, then one `$iN` scope per
/// iteration declaring `const i = N` — even in a module that declares and
/// uses no template: the route runs for it, as only a twin types the
/// iterations. The iteration's literal is typed by its value
/// (`inferIntLiteralType`, `Type.hs:485-491`): `uint1` for `0`
/// (`static-for.k::0`, the recorded walk). Red with the module unrouted
/// (the loop walked as written) or the literal untyped. The walk still
/// declines at the iteration's `uint32[i+1]`, whose fold the twin does
/// not reach yet.
#[test]
fn a_dependent_static_for_is_unrolled_without_a_template() {
    let main = "inline void main()\n{\nstatic for (const auto i : 2)\n{\nuint32[i+1] a;\n}\n}\n";
    let (trace, _) = walk_to(main);
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.starts_with("ParseIdentifier \"$i0\"")), "{trace}");
    let zero = calls.iter().position(|c| c.starts_with("ParseDecimalLiteral \"0\"")).expect("the induction literal");
    assert!(calls[zero + 1].starts_with("ParseUintType 1"), "{}", calls[zero + 1]);
    assert!(calls[zero + 2].starts_with("SetNodeType"), "{}", calls[zero + 2]);
}

/// An alias template's copy is named by `theType expr` — the note
/// `setTypeOfInstance` wrote at the declaration — not by the instance it
/// aliases: `using Pair = Tuple2<T1, T2>`'s copy hands the backend
/// `Pair<bool, uint4>` (`template-argument-deduction-from-instance.k::1`,
/// the recorded walk). Red with the aliased type asked first, which spells
/// `Tuple2<bool, uint4>`.
#[test]
fn an_alias_templates_copy_is_named_after_itself() {
    let main = "template <typename T1, typename T2>\nstruct Tuple2\n{\nT1 x;\nT2 y;\n}\n\ntemplate <typename T1, typename T2>\nusing Pair = Tuple2<T1, T2>;\n\ninline void main()\n{\nstatic Pair<bool, uint4> p;\n}\n";
    let (trace, _) = walk_to(main);
    let typedef = calls(&trace).into_iter().rfind(|c| c.starts_with("ParseTypedef")).expect("the copy");
    assert!(typedef.contains("\"Pair<bool,\\x20uint4>\""), "{typedef}");
}

/// `checkArguments` (`TypeCheck.hs:90-97`) wraps an argument its parameter
/// can convert from but does not accept as a subtype in a warning
/// (`warningNarrowingConversions`), and a `Warning`'s parse is its child's
/// (`ParseTree.hs:173`) — so the backend is told that node's type twice:
/// `Do(x + 1)` into a `uint32` parameter stamps `uint33` on the same node
/// twice (`higher-order-functions.k::3`, the recorded walk). Red without
/// the second stamp, and red if every argument got one.
#[test]
fn a_narrowing_argument_is_typed_twice() {
    let main = "inline uint32 Do(uint32 x)\n{\nreturn x;\n}\n\ninline uint32 Run(uint32 x)\n{\nreturn Do(x + 1) + Do(x);\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    // `x + 1` is `uint33`: its own stamp, then the warning node's.
    let binary = calls.iter().position(|c| c.starts_with("ParseBinaryOp")).expect("x + 1");
    assert!(calls[binary + 1].starts_with("ParseUintType 33"), "{}", calls[binary + 1]);
    assert!(calls[binary + 2].starts_with("SetNodeType"), "{}", calls[binary + 2]);
    assert!(calls[binary + 3].starts_with("ParseUintType 33"), "{}", calls[binary + 3]);
    assert!(calls[binary + 4].starts_with("SetNodeType"), "{}", calls[binary + 4]);
    // `x` is `uint32` and needs no conversion: one stamp, then the list.
    let named = calls.iter().rposition(|c| c.starts_with("ParseNamedVariable")).expect("x");
    assert!(calls[named + 1].starts_with("ParseUintType 32"), "{}", calls[named + 1]);
    assert!(calls[named + 2].starts_with("SetNodeType"), "{}", calls[named + 2]);
    assert!(calls[named + 3].starts_with("ParseBaseList"), "{}", calls[named + 3]);
}

/// `checkVarDecl`'s `initValueCheck` over an initializer `evaluateConstexpr`
/// folded: `checkConversion`'s literal arm judges it by `inferIntLiteralType
/// Nothing` of the value (`TypeCheck.hs:154`), so `const uint<5> x = 78;` is
/// `uint7` into `uint5` — a narrowing, stamped twice — and `const uint<8> y
/// = 78;` is no conversion at all (`constexpr.k::82`, the recorded walk).
#[test]
fn a_folded_narrowing_initializer_is_typed_twice() {
    let main = "const uint<5> x = 78;\nconst uint<8> y = 78;\n\ninline uint32 Run()\n{\nreturn x + y;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    // The stamps between a declaration's name and its `ParseDeclare`: the
    // initializer's own, and the warning node's.
    let stamps = |name: &str| {
        let ident = calls.iter().position(|c| c.starts_with(&format!("ParseIdentifier \"{name}\""))).expect("the declaration's name");
        let declare = ident + calls[ident..].iter().position(|c| c.starts_with("ParseDeclare")).expect("its declare");
        calls[ident..declare].iter().filter(|c| c.starts_with("SetNodeType")).count()
    };
    assert_eq!(stamps("x"), 2, "78 into uint5 narrows: {calls:#?}");
    assert_eq!(stamps("y"), 1, "78 into uint8 does not");
}

/// An initializer list that is no subtype of its target is checked element
/// by element against the type each initializes (`checkConversion'`,
/// `TypeCheck.hs:167-175`): an enum value meeting an integer field is its
/// literal (`:155-156`), and an element that narrows is wrapped in a
/// warning, typed twice — `Foo({E::X, false})` on an `int2` field walks
/// `3` stamped `uint2` twice (`conversion-warning.k::6`), a `uint32`
/// variable into an `int32` field is stamped twice (`::2`). The same holds
/// for a declaration's initializer (`initValueCheck`, `:131`). A list that
/// *is* a subtype is left whole (`s <: t = a`, `:164`): `{E::X, false}` for
/// a `uint2` field, `TEnum _ uint2 <: uint2` (`Type.hs:328`), keeps its
/// `EnumValue`.
#[test]
fn an_initializer_lists_elements_are_converted_to_their_fields() {
    let main = "enum E : uint2\n{\nX = 0b11\n}\n\nstruct S\n{\nint2 x;\nbool y;\n}\n\nstruct U\n{\nuint2 x;\nbool y;\n}\n\nstruct W\n{\nint32 x;\nbool y;\n}\n\ninline void Foo(S x)\n{\n}\n\ninline void Bar(U x)\n{\n}\n\ninline void Baz(W x)\n{\n}\n\ninline void main()\n{\nFoo({E::X, false});\nBar({E::X, false});\nuint32 v;\nBaz({v, false});\nS s = {E::X, true};\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    // The stamps after each `3`, up to the next element's `ParseBoolLiteral`.
    let stamps: Vec<usize> = (0..calls.len())
        .filter(|&i| calls[i].starts_with("ParseDecimalLiteral \"3\""))
        .map(|i| {
            let end = i + calls[i..].iter().position(|c| c.starts_with("ParseBoolLiteral")).expect("the next element");
            calls[i..end].iter().filter(|c| c.starts_with("SetNodeType")).count()
        })
        .collect();
    // The enum's own constant, then `Foo`'s, `Bar`'s `EnumValue` and the
    // declaration's.
    assert_eq!(stamps[stamps.len() - 3..], [2, 2, 2], "{stamps:?}");
    assert_eq!(calls.iter().filter(|c| c.starts_with("ParseEnumValue")).count(), 1, "only `Bar`'s stays an enum value");
    let v = calls.iter().rposition(|c| c.starts_with("ParseNamedVariable")).expect("v");
    let end = v + calls[v..].iter().position(|c| c.starts_with("ParseBoolLiteral")).expect("false");
    assert_eq!(calls[v..end].iter().filter(|c| c.starts_with("SetNodeType")).count(), 2, "{:#?}", &calls[v..end]);
}

/// An enum value given for an integer parameter is its literal, and
/// `checkArg` judges it as one — by `inferIntLiteralType Nothing` of the
/// value (`TypeCheck.hs:154-156`), not the enum's base type: `Do(Foo::One)`
/// over a `uint16` enum is `uint1` into `uint3`, no narrowing, stamped once
/// (`conversion-warning.k::15`, the recorded walk); `Do(Foo::Nine)` is
/// `uint4` into `uint3`, stamped twice.
#[test]
fn an_enum_argument_narrows_by_its_value() {
    let main = "enum Foo : uint16\n{\nOne = 1,\nNine = 9\n}\n\ninline void Do(uint3 x)\n{\n}\n\ninline void main()\n{\nDo(Foo::One);\nDo(Foo::Nine);\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let stamps = |lit: &str| {
        let at = calls.iter().rposition(|c| c.starts_with(&format!("ParseDecimalLiteral \"{lit}\""))).expect("the literal");
        let end = at + calls[at..].iter().position(|c| c.starts_with("ParseFunctionCall")).expect("the call");
        calls[at..end].iter().filter(|c| c.starts_with("SetNodeType")).count()
    };
    assert_eq!((stamps("1"), stamps("9")), (1, 2), "{calls:#?}");
}

/// An enum constant's folded value keeps the note of the expression it was
/// (`evaluateConstexpr`, `Constexpr.hs:117-283`): `-128` is the negation of
/// a `uint8`, typed `int9`, and `-1` an `int2` (`enum.k::27,33`, the
/// recorded walk) — not the bare literal's `int8` and `int1`. And an
/// integer declaration initialized with such a constant takes its literal,
/// judged by `inferIntLiteralType` of the value (`TypeCheck.hs:154-156`):
/// `const uint8 count = Foo::B;` over `B = -1` is `int1` into `uint8`, a
/// narrowing, stamped twice (`enum.k::44`).
#[test]
fn a_folded_enum_value_is_typed_as_its_expression() {
    let main = "enum Foo : int8\n{\nA = -128,\nB = -1\n}\n\nconst uint8 count = Foo::B;\n\ninline void main()\n{\nbool[count] x;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let after = |lit: &str| {
        let at = calls.iter().position(|c| c.starts_with(&format!("ParseDecimalLiteral \"{lit}\""))).expect("the literal");
        calls[at + 1].clone()
    };
    assert!(after("-128").starts_with("ParseIntType 9"), "{}", after("-128"));
    assert!(after("-1").starts_with("ParseIntType 2"), "{}", after("-1"));
    let count = calls.iter().position(|c| c.starts_with("ParseIdentifier \"count\"")).expect("count");
    let declare = count + calls[count..].iter().position(|c| c.starts_with("ParseDeclare")).expect("its declare");
    assert_eq!(calls[count..declare].iter().filter(|c| c.starts_with("SetNodeType")).count(), 2, "{:#?}", &calls[count..declare]);
}

/// An alias of a copy over a function type is displayed as `showUnmangled`
/// renders the instance, from its arguments: `using foo1 = Foo<fn_t>` is
/// `Foo<(uint32) -> uint32>` (`callbacks.k::38`, the recorded walk). The
/// mangle `Foo<|$fun$uint@32$uint@32|>` cannot be read back — `$fun` runs
/// its parameters and return together (`Mangle.hs:42-45`) — so the display
/// is the copy's own instance note, not its name.
#[test]
fn an_alias_of_a_copy_over_a_function_type_is_displayed() {
    let main = "using fn_t = (uint32 a) -> uint32;\n\ntemplate <typename T>\nclass Foo\n{\nprivate:\nT callback;\n}\n\nusing foo1 = Foo<fn_t>;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let typedef = calls.iter().rfind(|c| c.starts_with("ParseTypedef")).expect("foo1");
    assert!(typedef.contains("\"Foo<(uint32)\\x20->\\x20uint32>\""), "{typedef}");
}

/// A member call's argument is checked against the method's parameter
/// (`checkArg`, `TypeCheck.hs:97`): `_obj.increment(v)` with a `uint33`
/// into a `uint32` parameter is a narrowing, stamped twice
/// (`scheduling.k::5`, the recorded walk). The call's formals are the
/// method's: its name is typed as `method` answers, never with the
/// parameterless function a round-one `setTypeOfTemplateCall` stamp left
/// there before the class's members were known.
#[test]
fn a_member_calls_argument_narrows_into_its_parameter() {
    let main = "class R\n{\npublic:\nuint32 increment(uint32 x) { return x; }\n}\n\nclass C\n{\nprivate:\nR _obj;\n\npublic:\nuint32 main(uint33 v)\n{\nuint32 result;\nresult = _obj.increment(v);\nreturn result;\n}\n}\n\nexport C;\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let spec = calls.iter().rposition(|c| c.starts_with("ParseFunctionSpecifier")).expect("the callee");
    let call = spec + calls[spec..].iter().position(|c| c.starts_with("ParseFunctionCall")).expect("the call");
    // `v`'s scoped identifier and named variable, then the warning's second
    // stamp on the variable.
    assert_eq!(calls[spec..call].iter().filter(|c| c.starts_with("SetNodeType")).count(), 3, "{:#?}", &calls[spec..call]);
}

/// An array dimension over an unrolled iteration's induction variable folds
/// to an untyped literal under a note that had no type when `staticControl`
/// copied the body — the loop is unrolled at its own node, before its `const
/// auto` is typed (`Constexpr.hs:76`) — so it is typed as a bare literal:
/// `uint32[i+1]` in iteration 0 is `1 : uint1`, not `i + 1`'s `uint2`
/// (`static-for.k::0`, the recorded walk). A dimension naming a constant
/// keeps the constant's type: `bool[count]` over `const uint8 count = 5` is
/// `5 : uint8` (`enum.k::42`'s shape, which a blanket rule broke).
#[test]
fn an_unrolled_dimension_over_the_induction_variable_is_a_bare_literal() {
    let main = "const uint8 count = 5;\n\ninline void main()\n{\nstatic for (const auto i : 2)\n{\nuint32[i+1] a;\n}\nbool[count] b;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let after = |lit: &str| {
        let at = calls.iter().rposition(|c| c.starts_with(&format!("ParseDecimalLiteral \"{lit}\""))).expect("the literal");
        calls[at + 1].clone()
    };
    let first = calls.iter().position(|c| c.starts_with("ParseArrayType")).expect("the first iteration's array");
    let one = calls[..first].iter().rposition(|c| c.starts_with("ParseDecimalLiteral \"1\"")).expect("its dimension");
    assert!(calls[one + 1].starts_with("ParseUintType 1"), "{}", calls[one + 1]);
    assert!(after("5").starts_with("ParseUintType 8"), "{}", after("5"));
}

/// `inferExpTree'` postpones a function declared in a local scope
/// (`Inference.hs:58-62`): nothing in its body is typed where it is
/// written, and `moveFunctions` hoists it to `main@Bar` first
/// (`Function.hs:39`). Only the route hoists, so a module writing a local
/// function needs a twin, and without one the walk hands the backend an
/// untyped `x = 0` inside `Bar` (`shadow-warning.k::12`: ours
/// `ParseNamedVariable` where the reference stamps `ParseUintType 32`).
/// Red with the route blind to a local function: the second literal loses
/// its type.
#[test]
fn a_local_functions_body_is_typed() {
    let main = "inline void main()\n{\nuint32 x;\ninline void Bar()\n{\nuint32 x;\nx = 0;\n}\nx = 0;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert!(trace.contains("ParseIdentifier \"main@Bar\""), "the local function is hoisted");
    // Both `x = 0`s: the variable `uint32`, the literal `uint1`.
    let zeroes: Vec<usize> =
        calls.iter().enumerate().filter(|(_, c)| c.starts_with("ParseDecimalLiteral \"0\"")).map(|(i, _)| i).collect();
    assert_eq!(zeroes.len(), 2, "{calls:#?}");
    for i in zeroes {
        assert!(calls[i - 2].starts_with("ParseUintType 32"), "{}", calls[i - 2]);
        assert!(calls[i + 1].starts_with("ParseUintType 1"), "{}", calls[i + 1]);
        assert!(calls[i + 2].starts_with("SetNodeType"), "{}", calls[i + 2]);
    }
}

/// `deduceInstance'`'s type-mention arm (`Type.hs:1440-1444`) is not about
/// *position*: `Check(Foo<bool, bool>)` gives a function instance as an
/// argument, and a third parameter written with a default leaves the
/// mention short of its parameters, so the census postpones the whole thing
/// and no copy is ever made. Filled, the chain runs: the copy, the
/// `$wrapper$` lambda `addLambdaWrappers` makes of it, and the closure over
/// `make_capture0` (`template-param-default.k::7`, whose recorded walk this
/// matches). A *call's* mention is left alone — deduction binds it first
/// and the default only fills what deduction left (`:34`, `Fun(false)` over
/// `template <typename T = uint32>` is `Fun<bool>`). Red with either half.
#[test]
fn a_function_instance_given_as_an_argument_takes_its_defaults() {
    let helper = "module h\n    { Foo\n    , Fun\n    }\n\ntemplate <typename T, typename U, auto N = 4>\ninline int<N> Foo(T x)\n{\nreturn 0;\n}\n\ntemplate <typename T = uint32>\ninline T Fun(T x)\n{\nreturn x;\n}\n";
    let main = "import data.closure\nimport h\n\ninline void Check((bool) -> int4 Fn)\n{\n}\n\ninline void main()\n{\nCheck(Foo<bool, bool>);\nconst auto y = Fun(false);\n}\n";
    let (trace, declined) = walk_with(&[("h.k", helper)], main);
    assert_eq!(declined, None, "the walk goes through");
    assert!(trace.contains("Foo<|$bool$bool@4|>"), "the copy takes the default");
    assert!(trace.contains("$wrapper$@h$Foo<|$bool$bool@4|>"), "and is wrapped");
    assert!(trace.contains("make_capture0"), "over a closure with no capture");
    // The call's own parameter is deduced, never defaulted.
    assert!(trace.contains("Fun<|$bool|>"), "{trace}");
    assert!(!trace.contains("Fun<|$uint@32|>"), "a deduced parameter takes no default");
}

/// `evaluateConstexpr` splices a constant's initializer at pass 6
/// (`Constexpr.hs:256`), so by pass 8 the argument of `Check(fn)` under
/// `const auto fn = Foo<bool, bool>;` *is* the function: the reference names
/// the wrapper after it — `$wrapper$@helper@template_param_default$Foo<|…|>`
/// (`template-param-default.k::8`, the recorded walk) — and a wrapper named
/// `$wrapper$fn` calls a name no declaration answers, which the backend
/// rejects with "Undefined function: fn". Red with the constant unread.
#[test]
fn a_wrapper_is_named_after_the_function_a_constant_holds() {
    let helper = "module h\n    { Foo\n    }\n\ntemplate <typename T, typename U, auto N = 4>\ninline int<N> Foo(T x)\n{\nreturn 0;\n}\n";
    let main = "import data.closure\nimport h\n\ninline void Check((bool) -> int4 Fn)\n{\n}\n\ninline void main()\n{\nconst auto fn = Foo<bool, bool>;\nCheck(fn);\n}\n";
    let (trace, declined) = walk_with(&[("h.k", helper)], main);
    assert_eq!(declined, None, "the walk goes through");
    assert!(trace.contains("$wrapper$@h$Foo<|$bool$bool@4|>"), "{trace}");
    assert!(!trace.contains("$wrapper$fn"), "the constant is not the function");
}

/// `addLambdaWrappers` wraps the function arguments of a *member*
/// higher-order call too — `c.Go(DoIt, x)` — and only the route wraps, so
/// the caller's module needs one even when it declares nothing at all: no
/// template, no lambda, no higher-order function of its own
/// (`higher-order-functions.k::49`, whose `Closure<|@Run@$wrapper$…DoIt…|>`
/// this port made nowhere). The gate read a call's callee as a path and a
/// member call is a field. Red with the field arm cut.
#[test]
fn a_module_that_only_calls_a_higher_order_member_is_routed() {
    let helper = "module h\n    { C\n    , DoIt\n    }\n\nimport data.closure\n\nclass C\n{\nuint32 _x;\n\npublic:\ninline uint32 Go((uint32) -> uint32 Do, uint32 x)\n{\nreturn Do(x) + _x;\n}\n}\n\ninline uint32 DoIt(uint32 x)\n{\nreturn x;\n}\n";
    let main = "import data.closure\nimport h\n\ninline uint32 Run(uint32 x)\n{\nstatic C c;\nreturn c.Go(DoIt, x);\n}\n";
    let (trace, declined) = walk_with(&[("h.k", helper)], main);
    assert_eq!(declined, None, "the walk goes through");
    assert!(trace.contains("Run@$wrapper$@h$DoIt"), "the argument is wrapped: {trace}");
}

/// A copy's `uint<bitsizeof T>` is `uint32` once `T` is bound, and pass 6
/// folds it (`Constexpr.hs:50`) before pass 8 keys or mangles anything — so
/// `rc<uint<bitsizeof T>>(x)` inside `ae<uint32>`'s copy is the one instance
/// `rc<|$uint@32|>`, itself a template over the abbreviated parameter, whose
/// copy is `rc<|$uint@32|><|$uint@32|>`. Unfolded, the key and the mangled
/// name both carried the expression, the occurrence declined, and the copy
/// existed nowhere: the reference's
/// `reinterpret_cast<|$uint@32|><|$uint@32|>` is reached this way by every
/// `test/logic` program through `assert_equal` (`library/test/internal/assert.k:59`).
/// Red with the width unfolded in the census, in the mangler, or in `tArg`.
#[test]
fn a_copys_folded_width_is_one_instance() {
    let main = "template <typename T>\ninline T rc(auto x)\n{\nreturn cast<T>(x);\n}\n\ntemplate <typename T>\ninline void ae(T e, T a)\n{\nif (rc<uint<bitsizeof T>>(e) != rc<uint<bitsizeof T>>(a))\n{\n}\n}\n\ninline void main()\n{\nuint32 z;\nae(z, z);\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    assert!(trace.contains("rc<|$uint@32|><|$uint@32|>"), "the copy of the copy: {trace}");
    assert!(!trace.contains("bitsizeof"), "no width reaches the backend unfolded");
}

/// A *template* given as a template argument — `T1<S1>` over `template
/// <template <typename> typename TT> class T1` — and the same instance
/// written short, `T1<>`, when the parameter carries a default. Three
/// things have to hold for the reference's walk to be matched
/// (`template-template-parameter.k::1,4`):
///
/// * the copy carries its `setTypeOfInstance` note (`ParseClass`'s third
///   argument): `tArg` falls to `TArg $ showPretty e` for a template
///   (`Type.hs:436-437`) and the census's `ArgShape::Template` is what
///   carries it across modules. Without the note the backend throws "Type
///   error, expecting struct, union, or class" on `t.x`;
/// * its display spells that argument as the source wrote it — `T1<S1>`,
///   not the qualified name the mangle carries;
/// * `T1<>` fills the default and is the *same* copy.
///
/// Red with any of the three cut.
#[test]
fn a_template_given_as_an_argument_is_one_copy_written_or_defaulted() {
    // Across modules, where the copy lands beside its template and the
    // mangle carries the qualified path: the display still spells `S1`.
    let helper = "module h\n    { S1\n    , T1\n    }\n\ntemplate <typename T>\nstruct S1\n{\nT x;\n}\n\ntemplate <template <typename> typename TT = S1>\nclass T1\n{\npublic:\nTT<uint32> x;\n}\n";
    let importer = "import h\n\ninline void main()\n{\nstatic T1<S1> a;\nstatic T1<> b;\n}\n";
    let (across, declined) = walk_with(&[("h.k", helper)], importer);
    assert_eq!(declined, None, "the walk goes through");
    let across_classes: Vec<String> =
        calls(&across).into_iter().filter(|c| c.starts_with("ParseClass")).collect();
    assert_eq!(across_classes.len(), 1, "one copy across modules: {across_classes:?}");
    assert!(across_classes[0].contains("\"T1<S1>\""), "spelled as written: {across_classes:?}");

    let main = "template <typename T>\nstruct S1\n{\nT x;\n}\n\ntemplate <template <typename> typename TT = S1>\nclass T1\n{\npublic:\nTT<uint32> x;\n}\n\ninline void main()\n{\nstatic T1<S1> a;\nstatic T1<> b;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let classes: Vec<&String> = calls.iter().filter(|c| c.starts_with("ParseClass")).collect();
    assert_eq!(classes.len(), 1, "one copy for both spellings: {classes:?}");
    assert!(classes[0].contains("\"T1<S1>\""), "spelled as written: {classes:?}");
    // The third argument is the instance note, never null for a copy.
    let fields: Vec<&str> = classes[0].split_whitespace().collect();
    assert_ne!(fields[3], "_", "the copy carries its instance note: {classes:?}");
}

/// `checkEnumBaseType` (`TypeCheck.hs:201-203`) respells an enum's base as
/// its integer type before the walk: `enum Foo : DWORD` over `using DWORD =
/// uint32` passes `ParseUintType 32`, never the alias's identifier, which
/// aborts the backend with a C++ exception (`enum.k::24,25`, and the RISC-V
/// ISA's decoders). Red with the base walked as written.
#[test]
fn an_enum_base_named_through_an_alias_is_its_integer_type() {
    let h = "module h { Bar }\n\nenum Bar : int32\n{\n    A\n}\n";
    let (trace, declined) =
        walk_with(&[("h.k", h)], "import h\n\nusing DWORD = uint32;\n\nenum Foo : DWORD\n{\n    X\n}\n\nconst Foo x = Foo::X;\n");
    assert_eq!(declined, None);
    let calls = calls(&trace);
    // `parseEnum a b c`: the base is walked right after the name.
    assert_eq!(after(&calls, "ParseIdentifier \"Foo\""), Some("ParseUintType 32"), "{calls:?}");
}

/// `x++` is `Assign(x, x + 1)`, and the reference types the `+`
/// (`inferBinaryOp`, `Type.hs:746-771`): `uint32 + uint1` is `uint33`. When
/// `x` is declared in a spliced `static if` block the table leaves the name
/// untyped, and the operator went to the backend with no type — which it
/// aborts on (`static-if.k::20`). Red without `operand_type`'s local
/// fallback.
#[test]
fn an_increment_of_a_static_if_local_types_its_sum() {
    for text in [
        "inline void run()\n{\n    static uint32 x;\n    x++;\n}\n",
        "inline void run()\n{\n    static if (true)\n    {\n        static uint32 x;\n    }\n\n    x++;\n}\n",
    ] {
        let calls = calls(&walk(text));
        assert_eq!(after(&calls, "ParseBinaryOp"), Some("ParseUintType 33"), "{text}");
    }
}

/// Every unsuffixed literal reaches the backend typed by its value
/// (`inferIntLiteralType Nothing i`, `Type.hs:485-491`), including one the
/// table left unresolved — a declaration in a spliced top-level `static if`
/// block (`static-if.k::30`; twenty programs aborted the backend on the
/// untyped literal). Red with the fallback limited to literals the table
/// never keyed.
#[test]
fn a_literal_in_a_spliced_static_if_is_typed_by_its_value() {
    let text = "const uint32 Ten = 10;\n\nstatic if (Ten == 10)\n{\n    const uint32 x = 0;\n}\n";
    let calls = calls(&walk(text));
    assert_eq!(after(&calls, "ParseDecimalLiteral \"0\""), Some("ParseUintType 1"), "{calls:?}");
}

/// A folded value whose node the table left untyped is still typed: the
/// fold's literal is typed by value in the next inference round
/// (`inferIntLiteralType`, `Type.hs:485-491`). `byteoffsetof(S, b)` as an
/// array dimension is `1 : uint1`; untyped, it aborted the backend
/// (`constexpr.k::106`). Red with the literal typed only through the table.
#[test]
fn a_folded_dimension_is_typed_by_its_value() {
    let text = "struct S\n{\n    uint8 a;\n    uint8 b;\n}\n\ninline void main()\n{\n    uint8[byteoffsetof(S, b)] x;\n}\n";
    let calls = calls(&walk(text));
    assert_eq!(after(&calls, "ParseDecimalLiteral \"1\""), Some("ParseUintType 1"), "{calls:?}");
}

/// `x += y` over a constant `y` is `Assign(x, x + 10)` in the reference —
/// the fold replaced `y` — and `inferBinaryOp` types the sum `uint32 +
/// uint4 = uint33`; the walk's synthesized-binary fallback now reads a
/// folded operand as the literal it is (`static-if.k::50,52`, which aborted
/// the backend). Red without it.
#[test]
fn a_compound_assignment_over_a_constant_types_its_sum() {
    // The table leaves the name untyped where `static-if.k::50` has it: a
    // case under a function-level `static if`, over a constant from a
    // file-level one. A plain constant is typed there and never reaches
    // the fallback.
    let text = "const auto a = true;\n\nstatic if (a)\n{\n    const uint32 y = 10;\n}\n\ninline void f(uint32 x)\n{\n    switch (x)\n    {\n        static if (a)\n        {\n            case 3: x += y; break;\n        }\n    }\n}\n";
    let calls = calls(&walk(text));
    assert_eq!(after(&calls, "ParseBinaryOp"), Some("ParseUintType 33"), "{calls:?}");
}

/// A typed literal reaches the backend as its type (`inferIntLiteralType
/// (Just t)`): the `-10` a copy of `Bar<int32, -10>` substitutes for `T X`
/// is `int32` — `validateArg` re-stamps it (`TypeCheck.hs:281-283`) — though
/// the copy's table never typed the node (`type-check.k::22`, which aborted
/// the backend untyped). Red without the typed-literal arm.
#[test]
fn a_substituted_value_argument_keeps_its_parameter_type() {
    let text = "template <typename T, T X>\nclass Bar\n{\n    const T x = X;\n}\n\ninline void main()\n{\n    static Bar<int32, -10> b;\n}\n";
    let (trace, declined) = walk_to(text);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    assert_eq!(after(&calls, "ParseDecimalLiteral \"-10\""), Some("ParseIntType 32"), "{calls:?}");
}

/// Where the table has a key but left it unresolved — the product in a
/// hoisted local function over `decltype` aliases (`decltype.k::10`) — the
/// walk types the operator and the `return` by the reference's own rules
/// (`inferBinaryOp`; `infer (ReturnF (Just a)) = tUnConst $ typeOf a`):
/// `uint5 * uint5` is `uint10`. Untyped, the operator aborted the backend.
/// Red with either arm limited to nodes the table never keyed.
#[test]
fn a_product_the_table_left_unresolved_is_typed_as_the_reference_types_it() {
    let text = "inline void Run(uint5 x)\n{\n    using x_t = decltype(x);\n    using xx_t = decltype(x * x);\n\n    inline xx_t Fn(x_t i)\n    {\n        return i * i;\n    }\n\n    uint10 y = Fn(x);\n}\n";
    let (trace, declined) = walk_to(text);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    assert_eq!(after(&calls, "ParseBinaryOp 2"), Some("ParseUintType 10"), "{calls:?}");
    assert_eq!(after(&calls, "ParseReturnExpression"), Some("ParseUintType 10"), "{calls:?}");
}

/// `validate Auto (IntLiteralF _ i) = IntLiteralF Nothing i` (`TypeCheck.hs:
/// 283`): under an `auto` parameter a literal argument loses its type. The
/// fold of `x - 4` over a `const uint3 x` is a typed literal, but the copy
/// `F<0>` substitutes a bare `0`, so its `u & N` is `tAndLiteral`'s `uint1`
/// (`literal-operand.k::0`, which went `int6` once the fold kept its
/// type). Red without the strip.
#[test]
fn an_auto_parameter_drops_its_arguments_literal_type() {
    // As `literal-operand.k::0` has it: the argument passes from one copy
    // into the next under a second `static for`, in a module of its own.
    let h = "module h { AndLiteral, BinaryOpsLiteral }\n\nconst uint5 u = 0;\n\ntemplate <auto x, auto N>\ninline void AndLiteral()\n{\n    const auto iu = u & N;\n}\n\ntemplate <auto N>\ninline void BinaryOpsLiteral()\n{\n    static for (const auto x : 2)\n    {\n        AndLiteral<x, N>();\n    }\n}\n";
    let text = "import h\n\ninline void main()\n{\n    static for (const auto x : 8)\n    {\n        BinaryOpsLiteral<x - 4>();\n    }\n}\n";
    let (trace, declined) = walk_with(&[("h.k", h)], text);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    // Every copy's `iu` — the one over `0` is `uint1`, the others their own.
    let widths: Vec<&str> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.starts_with("ParseIdentifier \"iu\""))
        .map(|(i, _)| calls[i - 2].split(" -> ").next().unwrap_or(""))
        .collect();
    assert!(widths.contains(&"ParseUintType 1"), "{widths:?}");
    assert!(!widths.iter().any(|w| w.starts_with("ParseIntType")), "{widths:?}");
}

/// `validateArg` stamps a written integer argument to its parameter's width
/// in a deduced call too (`TypeCheck.hs:281-283`): `g<0>(3u8)` over `flags_t
/// F` (an alias of `uint16`) and a deduced `T` substitutes `0u16`, so the
/// copy's `F << 16` is `uint32` — `unit::test_case<Test, Group, Flags>`'s
/// shape. Red with `value_param_stamp` answering `None`.
#[test]
fn a_deduced_calls_written_value_argument_is_stamped() {
    let text = "using flags_t = uint16;\n\ntemplate <flags_t F, typename T>\ninline void g(T x)\n{\n    const auto X = F << 16;\n}\n\ninline void main()\n{\n    g<0>(3u8);\n}\n";
    let (trace, declined) = walk_to(text);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let at = calls.iter().position(|c| c.starts_with("ParseIdentifier \"X\"")).expect("X declared");
    assert!(calls[at - 2].starts_with("ParseUintType 32 "), "{:?}", &calls[at - 4..=at]);
}

/// A copy whose body returns a capturing lambda is typed, and walks:
/// `compose2(inc, add)` over `helper.fn`'s `compose2`, which returns `[f,
/// g](auto x, auto y) { … }` — `data.function`'s own, which 114 corpus
/// programs walk (2026-09-23). Three links, each red on its own:
///
/// * the capture's `make_capture2(f, g)` is deduced over two closures, and
///   a closure is spelled as its copy, `@@data@closure@core@Closure<|…|>`,
///   the name `data.closure`'s copy wears — not the closure's `[capture]
///   function` show (`elaborate::mint_leaf`);
/// * `helper.fn` imports nothing — its lambdas reach `data.closure` only
///   through what `desugarLambdas` mints — so its twin finds that copy in
///   the one module that declares it (`Imports::copy_anywhere`);
/// * the hoisted lambda's capture parameter is the copy's own local alias,
///   found from a function hoisted out of it, under the module's namespace
///   (`Declarations::lookup_walking_skipping`).
#[test]
fn a_copy_that_returns_a_capturing_lambda_is_typed() {
    // `base` imports `helper.fn`, as it imports `data.function`, so
    // `helper.fn` is one of the modules the implicit base is not imported
    // into; and two of its functions return a lambda, so `?lambda0$capture`
    // is declared twice.
    let base = "module base\n    { module data.closure\n    , module helper.fn\n    }\n\nimport data.closure\nimport helper.fn\n";
    let helper = "module helper.fn\n    { compose\n    , compose2\n    }\n\ninline auto compose((auto) -> auto f, (auto) -> auto g)\n{\n    return [f, g](auto x)\n        {\n            return f(g(x));\n        };\n}\n\ninline auto compose2((auto) -> auto f, (auto, auto) -> auto g)\n{\n    return [f, g](auto x, auto y)\n        {\n            return f(g(x, y));\n        };\n}\n";
    let main = "inline uint32 inc(uint32 x)\n{\n    return x + 1;\n}\n\ninline uint32 add(uint32 a, uint32 b)\n{\n    return a + b;\n}\n\nconst auto h = compose2(inc, add);\n";
    let (text, declined) = walk_on_base(base, &[("helper/fn.k", helper)], main);
    assert_eq!(declined, None);
    assert!(
        text.contains("\"make_capture2<|@@data@closure@core@Closure<|@$wrapper$inc@@data@closure@core@make_capture0@$@Capture|>@@data@closure@core@Closure<|@$wrapper$add@@data@closure@core@make_capture0@$@Capture|>|>\""),
        "{text}"
    );
}

/// Two `static for` loops of one member each declare `x`, in a file with a
/// copy — so the walk reads the twin's table, whose keys leave `$quasiN`
/// out as the source's do: every mention of either `x` is typed, `uint2`,
/// as the reference types it. Untyped, the backend met `x + 1` as an
/// assertion (`parse_tree.cpp:1266`, `test/logic/test_cases_13.k`,
/// 2026-09-23). Red with the twin's locals left unrecorded.
#[test]
fn a_loop_variable_declared_twice_is_typed_in_a_twin() {
    let main = "template <typename T>\ninline T id(T v)\n{\nreturn v;\n}\n\nclass Main\n{\nuint32[4] g;\npublic:\nvoid run()\n{\nstatic for(const auto x : 4)\n{\ng[x] = id(x);\n}\nstatic for(const auto x : 4)\n{\ng[x] = x + 1;\n}\n}\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let mut mentions = 0;
    for (i, call) in calls.iter().enumerate() {
        if call.starts_with("ParseScopedIdentifier") && calls[i - 2].starts_with("ParseIdentifier \"x\"") {
            mentions += 1;
            assert!(calls[i + 1].starts_with("ParseUintType 2"), "{}\n{}", calls[i - 2..i + 2].join("\n"), trace);
        }
    }
    assert_eq!(mentions, 4);
}

/// `data.closure` with `make_capture2` beside `make_capture1`, cut as
/// [`CLOSURE`] is.
const CLOSURE2: &str = "module data.closure\n    { module data.closure.core\n    , make_capture1\n    , make_capture2\n    }\n\nimport data.closure.core\n\ntemplate\n    < typename T0\n    >\ninline auto make_capture1\n    ( T0 x0\n    )\n{\n    struct Capture\n    {\n        T0 x0;\n    }\n\n    Capture capture =\n        { x0\n        };\n\n    return capture;\n}\n\ntemplate\n    < typename T0\n    , typename T1\n    >\ninline auto make_capture2\n    ( T0 x0\n    , T1 x1\n    )\n{\n    struct Capture\n    {\n        T0 x0;\n        T1 x1;\n    }\n\n    Capture capture =\n        { x0\n        , x1\n        };\n\n    return capture;\n}\n";

/// [`walk_with`] with an implicit base — `base`, at `/proj/base.k` — and
/// [`CLOSURE2`] for `data.closure`: the shape of the real library, where a
/// module reaches `data.closure` only through what its lambdas mint, and
/// the root through the base.
fn walk_on_base(base: &str, extra: &[(&str, &str)], main: &str) -> (String, Option<String>) {
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.base_library = Some(Utf8PathBuf::from("/proj/base.k"));
    let mut db = kg_db::Db::new(settings);
    for (path, text) in [("data/closure/core.k", CORE), ("data/closure.k", CLOSURE2), ("base.k", base)].iter().chain(extra) {
        db.set_file_text(&Utf8PathBuf::from(format!("/proj/{path}")), (*text).to_owned());
    }
    let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
    let (mut trace, text) = trace_backend();
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    let outcome = kg_ptbridge::program::run(&db, &[root], options, None, &mut trace);
    trace.flush();
    (text(), outcome.declined.map(|d| d.reason))
}

/// `control.loop`'s `pipelined_for`: `static pipelined<I, decltype(body)>
/// loop;` in a copy whose `body` is a closure. The census binds the
/// `decltype` as the closure's copy (`typ (TClosure _ _ _ s) =
/// typeIdentifier' s`), so the class copy is decided and made —
/// `pipelined<|$uint@2@@data@closure@core@Closure<|…|>|>`, as the reference
/// names it (`function-attributes.k::15`, the recorded walk; 35 corpus
/// programs declined there, 2026-09-23); its `Closure closure` parameter
/// is spelled as `expFromType` spells, no scope; and the copy carries its
/// `setTypeOfInstance` note, the `decltype` position typed by the shape.
#[test]
fn a_class_copy_over_a_closure_decltype_is_made() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let run = "module h.run\n    { run\n    }\n\ntemplate <typename I, typename Closure>\nclass pipelined\n{\npublic:\n    [[pipelined]] auto go(I tid, Closure closure)\n    {\n        return closure(tid);\n    }\n}\n\ntemplate <typename I>\ninline void run(auto count, (I) -> void body)\n{\n    static pipelined<I, decltype(body)> loop;\n    loop.go(count, body);\n}\n";
    let main = "import h.run\n\nclass Foo\n{\n    uint32[4] mem;\n\npublic:\n    void init()\n    {\n        run(4, [](uint2 i)\n        {\n            mem[i] = 0;\n        });\n    }\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/run.k", run)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let copy = "pipelined<|$uint@2@@data@closure@core@Closure<|@Foo@init@?lambda0@@data@closure@make_capture1<|$ref@Foo|>@$@Capture|>|>";
    let class = calls.iter().find(|c| c.starts_with("ParseClass") && c.contains("\"pipelined<uint2,")).expect("the copy is made");
    assert!(!class.contains(" _ [\"@h@run\"]"), "the copy's instance note: {class}");
    let param = (1..calls.len())
        .find(|&i| {
            calls[i].starts_with("ParseFunctionParam")
                && calls[i].contains(copy)
                && calls[i - 1].starts_with("ParseIdentifier \"closure\"")
        })
        .expect("go's `closure` parameter");
    assert!(calls[param - 2].starts_with("ParseNamedType") && calls[param - 2].contains(" [] ->"), "{}", calls[param - 2]);
    assert!(calls[param - 3].contains("ParseAppendList"), "{}", calls[param - 3]);
}

/// `control.loop`'s `pipelined_map`: the lambda over `body` is bound to a
/// `const auto fn`, and `static pipelined<I, decltype(fn)> loop;` binds the
/// `decltype` `const` and all — `typ (TConst x) = ConstF <$> typ' x`
/// (`Type.hs:1628`), mangled `$const` ahead of the closure's copy, as
/// `kg-oracle --tree` names it (`pipelined<|$uint@32$const@@data@closure@
/// core@Closure<|…|>|>`). The census read the `const` closure as opaque and
/// the copy was never made (seven corpus programs, `docs/history/gaps.md` §A2).
#[test]
fn a_class_copy_over_a_const_closure_decltype_is_made() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let run = "module h.run\n    { run\n    }\n\ntemplate <typename I, typename Closure>\nclass pipelined\n{\npublic:\n    [[pipelined]] auto go(I tid, Closure closure)\n    {\n        return closure(tid);\n    }\n}\n\ntemplate <typename I>\ninline void run(auto count, (I) -> void body)\n{\n    const auto fn = [body](I x)\n    {\n        body(x);\n    };\n\n    static pipelined<I, decltype(fn)> loop;\n    loop.go(count, fn);\n}\n";
    let main = "import h.run\n\nclass Foo\n{\n    uint32[4] mem;\n\npublic:\n    void init()\n    {\n        run(4, [](uint2 i)\n        {\n            mem[i] = 0;\n        });\n    }\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/run.k", run)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let copy = "\"pipelined<|$uint@2$const@@data@closure@core@Closure<|@@h@run@run<|";
    assert!(calls.iter().any(|c| c.starts_with(&format!("ParseIdentifier {copy}"))), "the copy's name");
    assert!(
        calls.iter().any(|c| c.starts_with("ParseClass") && c.contains("\"pipelined<uint2,\\x20const\\x20data.closure.core::Closure<")),
        "the copy is made"
    );
}

/// A written type argument keeps its own node in the owner's copy, scope and
/// all (`resolveTypeParam`, `TypeCheck.hs:334-339`): `F1<Foo>(foo)` in
/// `main` is `TypeIdentifier ["main", "$"] Foo` in `h.ls`'s copy, as
/// `test/logic/test_cases_11.k`'s `fixture<Test…>` is `["test_main", "$"]`
/// in `test.unit`'s (the recorded walks). The same call's deduction,
/// spelled unscoped, is the same instance — keyed by name, not scope — and
/// the written one is handed first.
#[test]
fn a_written_type_argument_keeps_its_scope_in_the_owners_copy() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.ls\n    { F1\n    }\n\ntemplate <typename T>\ninline uint32 F1(T x)\n{\n    return bitsizeof x;\n}\n";
    let main = "import h.ls\n\nclass Foo\n{\n    uint32 x;\n}\n\ninline void main()\n{\n    Foo foo;\n\n    F1<Foo>(foo);\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/ls.k", helper)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let x = calls
        .iter()
        .position(|c| c.starts_with("ParseFunctionParam") && c.contains("\"F1<|@Foo|>\""))
        .unwrap_or_else(|| panic!("the copy's parameter: {:?}", calls.iter().filter(|c| c.contains("F1")).collect::<Vec<_>>()));
    let ty = calls[..x].iter().rev().find(|c| c.starts_with("ParseNamedType")).expect("its type");
    assert!(ty.contains("[\"main\",\"$\"]"), "{ty}");
}

/// [`a_written_type_argument_keeps_its_scope_in_the_owners_copy`] for a
/// type the owner cannot name — `main`'s *local* `Foo`, an alias leaf the
/// owner mints (`local-structs.k::6`, the recorded walk). Only the written
/// argument's leaf wears its scope: a `decltype` of a closure in a copy's
/// body is a minted leaf too, spelled `[]` as `expFromType` spells it
/// (`closures.k::0`).
#[test]
fn a_written_local_type_argument_keeps_its_scope_in_the_owners_copy() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.ls\n    { F1\n    }\n\ntemplate <typename T>\ninline uint32 F1(T x)\n{\n    return bitsizeof x;\n}\n";
    let main = "import h.ls\n\ninline void main()\n{\n    struct Foo\n    {\n        uint32 x;\n    }\n\n    Foo foo;\n\n    F1<Foo>(foo);\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/ls.k", helper)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let x = calls
        .iter()
        .position(|c| c.starts_with("ParseFunctionParam") && c.contains("\"F1<|@main@$@Foo|>\""))
        .expect("the copy's parameter");
    let ty = calls[..x].iter().rev().find(|c| c.starts_with("ParseNamedType")).expect("its type");
    assert!(ty.contains("[\"main\",\"$\"]"), "{ty}");
}

/// A mention written short of a template-template default, whose template's
/// name two modules declare, with a class constant for an argument —
/// `test/library/data/bitarray/banked.k`'s `bitarray<Size, WordWidth,
/// Banks>` over `data.bitarray.banked`, which imports `data.bitarray`'s own
/// `bitarray` as `ba`. The reference completes both mentions at pass 4, the
/// default qualified by the module declaring `mem` rather than the one
/// writing it (`kg-oracle --passes 8 --template-iterations 4`:
/// `arr<|@4@2@@h@mem@mem|>`, `arr<|@4@@h@mem@mem|>`). Each of the three
/// was a missing copy on its own: the homonym left `Templates::located`
/// unanswered, `Size` was no literal, and the default spelled `@@h@bb@mem`.
#[test]
fn a_homonym_mention_short_of_an_imported_default_is_completed() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let mem = "module h.mem\n    { mem\n    }\n\ntemplate <typename T, auto N>\nclass mem\n{\n    T[N] _x;\npublic:\n    T get(uint2 i) { return _x[i]; }\n}\n";
    let ba = "module h.ba\n    { arr\n    }\n\nimport h.mem\n\ntemplate <auto N, template <typename, auto> typename M = mem>\nclass arr\n{\n    M<bool, N> _m;\npublic:\n    bool get(uint2 i) { return _m.get(i); }\n}\n";
    let bb = "module h.bb\n    { arr\n    }\n\nimport h.ba as ba\nimport h.mem\n\ntemplate <auto N, auto K, template <typename, auto> typename M = mem>\nclass arr\n{\n    ba::arr<N> _inner;\n    M<uint8, K> _m;\npublic:\n    bool get(uint2 i) { return _inner.get(i); }\n}\n";
    let main = "import h.bb\n\nclass Top\n{\n    const auto Size = 4;\n    using arr_t = arr<Size, 2>;\n    arr_t _a;\npublic:\n    bool Go(uint2 i) { return _a.get(i); }\n}\n\nexport Top;\n";
    let (trace, declined) = walk_on_base(base, &[("h/mem.k", mem), ("h/ba.k", ba), ("h/bb.k", bb)], main);
    assert_eq!(declined, None);
    for copy in ["arr<|@4@2@@h@mem@mem|>", "arr<|@4@@h@mem@mem|>", "mem<|$bool@4|>", "mem<|$uint@8@2|>"] {
        assert!(trace.contains(&format!("\"{copy}\"")), "{copy} is made");
    }
}

/// [`a_homonym_mention_short_of_an_imported_default_is_completed`] for a
/// call: deduction leaves the template-template parameter to its default,
/// and the name it binds is the one the writing module imports, qualified
/// by its own module (`kg-oracle --passes 8 --template-iterations 4`:
/// `Fn<|$bool@@h@mem@mem|>`, never `@@h@fn@mem`).
#[test]
fn a_calls_template_default_names_the_module_declaring_it() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let mem = "module h.mem\n    { mem\n    }\n\ntemplate <typename T, auto N>\nclass mem\n{\n    T[N] _x;\npublic:\n    T get(uint1 i) { return _x[i]; }\n}\n";
    let func = "module h.fn\n    { Fn\n    }\n\nimport h.mem\n\ntemplate <typename T, template <typename, auto> typename M = mem>\ninline T Fn(T x)\n{\n    static M<T, 2> m;\n    return m.get(0);\n}\n";
    let main = "import h.fn\n\nclass Top\n{\npublic:\n    bool Go(bool b) { return Fn(b); }\n}\n\nexport Top;\n";
    let (trace, declined) = walk_on_base(base, &[("h/mem.k", mem), ("h/fn.k", func)], main);
    assert_eq!(declined, None);
    for copy in ["Fn<|$bool@@h@mem@mem|>", "mem<|$bool@2|>"] {
        assert!(trace.contains(&format!("\"{copy}\"")), "{copy} is made");
    }
}

/// An alias copy is `TInstance` of what it aliases, and a mention of it that
/// type (`setTypeOf`'s `set`, `Type.hs:1053-1058`): `Pair<arr<uint32, 10>,
/// 16>`'s copy is named by its note, the alias spelled as the alias —
/// `Pair<h.al::arr<uint32, 10>, 16>` (`memory.k::17`'s `memory_ecc<data.
/// array::array<uint32, 10>, 16>`, the recorded walk) — where the mangle
/// reads back `uint32[10]`.
#[test]
fn an_alias_copy_argument_is_displayed_as_the_alias() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.al\n    { arr\n    , Pair\n    }\n\ntemplate <typename T, auto N>\nusing arr = T[N];\n\ntemplate <typename T, auto W>\nusing Pair = T;\n";
    let main = "import h.al\n\nclass Main\n{\n    Pair<arr<uint32, 10>, 16> p;\n\npublic:\n    uint32 read()\n    {\n        return p[0];\n    }\n}\n\nexport Main;\n";
    let (trace, declined) = walk_on_base(base, &[("h/al.k", helper)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let pair = calls.iter().find(|c| c.starts_with("ParseTypedef") && c.contains("\"Pair<")).expect("the copy");
    assert!(pair.contains("\"Pair<h.al::arr<uint32,\\x2010>,\\x2016>\""), "{pair}");
}

/// `withUnmangledName` names a declaration whose type is an instance
/// (`ParseTree.hs:177-180`), whatever its name looks like: a generic
/// lambda's hoisted copy, `Run@?lambda0<|$uint@32|>`, is shown
/// `Run@?lambda0<uint32>` (`higher-order-functions.k::1`, the recorded
/// walk), where the name's `@` had made this port take it for a function.
#[test]
fn a_hoisted_generic_lambdas_copy_is_displayed() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let h = "module h.go\n    { Go\n    }\n\ninline uint32 Go((uint32) -> uint32 Do, uint32 x)\n{\n    return Do(x);\n}\n";
    let main = "import h.go\n\ninline uint32 Run(uint32 x)\n{\n    const auto fn = [](auto x) -> uint32\n    {\n        return 2 * x;\n    };\n\n    return Go(fn, x);\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/go.k", h)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    assert!(
        calls.iter().any(|c| c.starts_with("ParseFunction ") && c.contains("\"Run@?lambda0<uint32>\"")),
        "{:#?}",
        calls.iter().filter(|c| c.starts_with("ParseFunction ")).collect::<Vec<_>>()
    );
}

/// A wrapper hoisted out of a copy — `Run<|$uint@32|>@$wrapper$Do<|…|>` —
/// wraps the callee the copy had resolved and is a function in the
/// reference, displayed as nothing (`higher-order-functions.k::57`, the
/// recorded walk), though this route makes it by instantiating a generic
/// wrapper; and its parent's mangle is not its own `@` split
/// (`compose2<|…@$wrapper$add|>` keeps its display, `::0`).
#[test]
fn a_wrapper_hoisted_out_of_a_copy_has_no_display() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let h = "module h.go\n    { Go2\n    }\n\ninline uint32 Go2((uint32) -> uint32 Do1, (uint32) -> uint32 Do2, uint32 x)\n{\n    return Do1(x) + Do2(x + 1);\n}\n";
    let main = "import h.go\n\ntemplate <typename T>\ninline T Do(T x)\n{\n    return x;\n}\n\ninline uint32 DoIt(uint32 x)\n{\n    return x;\n}\n\ntemplate <typename T>\ninline void Run()\n{\n    Go2(Do<T>, DoIt, 10);\n}\n\ninline void main()\n{\n    Run<uint32>();\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/go.k", h)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let name = "Run<|$uint@32|>@$wrapper$Do<|$uint@32|>";
    assert!(calls.iter().any(|c| c.starts_with(&format!("ParseIdentifier \"{name}\""))), "the wrapper is declared");
    let shown: Vec<&String> =
        calls.iter().filter(|c| c.starts_with("ParseFunction ") && c.contains("\"Run<|$uint@32|>@$wrapper$Do")).collect();
    assert!(shown.is_empty(), "no display: {shown:#?}");
    assert!(calls.iter().any(|c| c.starts_with("ParseFunction ") && c.contains("\"Go2<")), "the copy over it keeps its own");
}

/// A copy of this module's class template given to another module's
/// function template — `go<C<1>>()` over `static F t; t.run();` — is
/// spelled in the copy as the class copy's own name, `C<|@1|>`: the
/// owner cannot spell `C`, and the alias it mints must stand for that name,
/// not for the copy's display text, which the walk handed the backend as a
/// raw alias — a type it knew nothing of ("Left of . in a method call must
/// be an object", `test/logic/nested_atomic.k`, 2026-09-23). No identifier
/// the walk emits is ever a pool alias.
#[test]
fn a_class_copy_given_to_another_modules_template_is_named_as_itself() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let fx = "module h.fx\n    { go\n    }\n\ntemplate <typename F>\ninline void go()\n{\n    static F t;\n    t.run();\n}\n";
    let main = "import h.fx\n\ntemplate <auto N>\nclass C\n{\npublic:\n    void run()\n    {\n    }\n}\n\ninline void main()\n{\n    go<C<1>>();\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/fx.k", fx)], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let t = calls.iter().position(|c| c.starts_with("ParseIdentifier \"t\"")).expect("the static");
    assert!(calls[t - 1].starts_with("ParseNamedType"), "{}", calls[t - 1]);
    assert!(calls[t - 3].starts_with("ParseIdentifier \"C<|@1|>\""), "{}", calls[t - 3]);
    assert!(!trace.contains("ParseIdentifier \"__kg_m"), "{trace}");
}

/// A copy's name spells a function type's attributes in the order written:
/// `same_types<[[max_threads(2), thread_rate(1)]] (uint3 x) -> void,
/// [[thread_rate(1), max_threads(2)]] (uint3) -> void>` is
/// `same_types<|$fun@max_threads@2@thread_rate@1…$fun@thread_rate@1@max_threads@2…|>`
/// at the mention and at the copy both (`type-constexpr.k::17`, the
/// recorded walk). The route's owner minted the copy from the census's
/// shapes in `Set.toList` order, so the mention named a copy no module
/// declared and the walk declined it (booked as "undeclared copy",
/// 2026-09-23); the loop keys the copy off the written arguments. The twin route,
/// since deleted, was red here.
#[test]
fn a_copy_spells_function_type_attributes_as_written() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let st = "module h.st\n    { same_types\n    }\n\ntemplate <typename S, typename T>\nclass same_types\n{\n}\n";
    let main = "import h.st\n\ninline void main()\n{\n    static same_types<[[max_threads(2), thread_rate(1)]] (uint3 x) -> void, [[thread_rate(1), max_threads(2)]] (uint3) -> void> test;\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/st.k", st)], main);
    assert_eq!(declined, None);
    let name = "ParseIdentifier \"same_types<|$fun@max_threads@2@thread_rate@1$uint@3$void$fun@thread_rate@1@max_threads@2$uint@3$void|>\"";
    assert!(trace.contains(name), "{trace}");
}

/// A `static assert` over a runtime operand that a constant absorbs —
/// `false && (i == 4)`, `c || true` — folds (`evalBinaryBool`,
/// `Constexpr.hs:238-242`) and is gone before the backend. The guard
/// `typeResolved` asks of the runtime operand is its *type*, which the
/// table has; the file scope's "every name a known constant" refused it,
/// and the walk declined (`constexpr.k::139-143`, 2026-09-23).
#[test]
fn a_static_assert_a_constant_absorbs_folds() {
    let main = "inline void Run(uint32 i)\n{\nconst bool b = false;\nstatic assert(!(b && (i == 4)));\nbool c = i == 4;\nstatic assert(c || true);\n}\n\ninline void main()\n{\nRun(3);\n}\n";
    let (_, declined) = walk_to(main);
    assert_eq!(declined, None);
}

/// A copy's capture-less lambda in a module that imports no `data.closure`:
/// the capture is `decltype(make_capture0())`, which `desugarLambdas`
/// qualified and the reference finds in its whole-program map. The twin
/// prints the bare name; found under the namespaces the module's tree
/// qualifies with (`Imports::with_hidden_namespaces`), the capture types,
/// the lambda's body is inferred, and its `auto` return resolves — where
/// the walk declined `data.array`'s `and(a)` and ~30 corpus programs like
/// it (2026-09-23).
#[test]
fn a_capture_less_lambda_in_a_module_without_the_closure_import_is_typed() {
    let base = "module base\n    { module data.closure\n    , module h.arr\n    }\n\nimport data.closure\nimport h.arr\n";
    let arr = "module h.arr\n    { all_of\n    }\n\ntemplate <auto N>\ninline bool fold2((bool, bool) -> bool f, bool[N] x)\n{\n    return f(x[0], x[1]);\n}\n\ntemplate <auto N>\ninline bool all_of(bool[N] x)\n{\n    return fold2([](bool a, bool b)\n        {\n            return a && b;\n        }, x);\n}\n";
    let main = "inline void main()\n{\n    bool[2] a = {true, false};\n    bool r = all_of(a);\n}\n";
    let (_, declined) = walk_on_base(base, &[("h/arr.k", arr)], main);
    assert_eq!(declined, None);
}

/// A callback parameter whose function type is written through an alias
/// (`using Callback = (uint32) -> uint32`) is a higher-order function all
/// the same: `resolvedFuncParams` sees through the alias
/// (`Function.hs:133-141`), `Do` becomes a template, and the call through
/// `callback` is `closureCalls`' like any other
/// (`higher-order-functions.k::22`, which the walk declined as "call of a
/// variable" until pass 2 resolved aliases, 2026-09-27).
#[test]
fn an_alias_typed_callback_makes_a_higher_order_template() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "using Callback = (uint32) -> uint32;\n\ninline uint32 Do(Callback callback, uint32 x)\n{\nreturn callback(x);\n}\n\ninline uint32 Run(uint32 x)\n{\nconst auto fn = [](uint32 x) -> uint32\n{\nreturn 2 * x;\n};\n\nreturn Do(fn, x);\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
}

/// A call through a function-typed parameter that `closureCalls` did not
/// rewrite is declined, never handed to the backend, whose "Undefined
/// function" is not the reference's answer. `decltype(g)` is not an alias,
/// so `higherOrderFunctions` leaves `Do` a function and nothing rewrites
/// `callback(x)`; the reference rejects the program ("Couldn't deduce type
/// of the expression" at the `decltype`, `kg-oracle --errors` 2026-09-27)
/// where this frontend is silent — the walk's decline is what keeps the
/// silence from compiling. (Its earlier shapes, a local class calling the
/// closure it was given and an alias-typed callback, are rewritten now:
/// [`a_hoisted_class_names_its_functions_locals`],
/// [`an_alias_typed_callback_makes_a_higher_order_template`].)
#[test]
fn a_call_through_an_unrewritten_closure_is_declined() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline uint32 g(uint32 x)\n{\nreturn 2 * x;\n}\n\ninline uint32 Do(decltype(g) callback, uint32 x)\n{\nreturn callback(x);\n}\n\ninline uint32 Run(uint32 x)\n{\nconst auto fn = [](uint32 x) -> uint32\n{\nreturn 2 * x;\n};\n\nreturn Do(fn, x);\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined.as_deref(), Some("call of a variable"));
}

/// `obj.template Go<uint32>` outside a call is `methodInstanceSpecifier`
/// (`Parser.hs:766-770,849`): the method's instance as a value, a
/// `FunctionSpecifier` with its receiver as a call's callee is, so the
/// instance is made in `B` and the walk hands the backend
/// `ParseFunctionSpecifier` — where a member access over an unsorted path
/// was declined (`dependent-template.k::9,10`, `template-param-default.k
/// ::39`, the recorded walks).
#[test]
fn a_method_instance_as_a_value_is_a_function_specifier() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "class B\n{\npublic:\n    template <typename T>\n    T Go()\n    {\n        return {};\n    }\n}\n\nclass E\n{\npublic:\n    () -> uint32 callback;\n}\n\ninline void main()\n{\n    static B obj;\n    static E e = {.callback = obj.template Go<uint32>};\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert!(trace.contains("ParseFunctionSpecifier"), "{trace}");
    assert!(trace.contains("Go<|$uint@32|>"), "{trace}");
}

/// `isAbbreviatedTemplate` holds of a declaration with no body as of a
/// definition (`Parser/Syntax.hs:722-723`), and `removeTemplate` deletes
/// both (`Frontend.hs:300-301`): `void Do(auto x);` never reaches the
/// backend, where the walk declined its `auto` (`extern-class.k::15,37`).
#[test]
fn an_abbreviated_declaration_is_deleted_with_the_templates() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "class Foo\n{\npublic:\n    void Do(auto x);\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert!(!trace.contains("\"Do\""), "{trace}");
}

/// A class `moveFunctions` hoisted out of a function names that function's
/// locals: `exec(task_t t)` in `go`'s local class, with `using task_t =
/// decltype(task)` in `go`'s body. The twin prints the class at module
/// scope; the scope walk, its own scopes exhausted, looks in the function
/// it came out of before the module, as the reference's walk from where
/// the class was written does. `t` is then the closure it is, and
/// `closureCalls` rewrites `t()` — `control.async`'s `async_then_void`, 12
/// corpus programs (2026-09-23). Red with a hoisted class left out of
/// `hoisted_scopes`.
#[test]
fn a_hoisted_class_names_its_functions_locals() {
    let base = "module base\n    { module data.closure\n    , module h.go\n    }\n\nimport data.closure\nimport h.go\n";
    let go = "module h.go\n    { go\n    }\n\ninline void go(() -> void task)\n{\n    using task_t = decltype(task);\n\n    class runner\n    {\n    public:\n        void exec(task_t t)\n        {\n            t();\n        }\n    }\n\n    static runner r;\n    r.exec(task);\n}\n";
    let main = "inline void main()\n{\n    go([]{});\n}\n";
    let (_, declined) = walk_on_base(base, &[("h/go.k", go)], main);
    assert_eq!(declined, None);
}

/// A generic lambda called through its closure calls its deduced copy:
/// `fn(10)` over `const auto fn = [a](auto x) { … }` — `closureCalls`
/// names the lambda's function, an abbreviated template, and
/// `deduceTemplateArgs` deduces its copy right after in the reference's
/// round (`Template.hs:71-81`), here in the next one, which the rewrite
/// now asks for (`lambdas.k::15`, 2026-09-23). Before, the call named the
/// template and the walk declined it as "call of an undeclared function".
/// Red with the round not continued after a closure call.
#[test]
fn a_generic_lambda_called_through_its_closure_calls_its_copy() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void Go()\n{\nauto a = 10;\n\nconst auto fn = [a](auto x)\n{\nreturn x - a;\n};\n\nfn(10);\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
}

/// `atomically<uint8>(increment)` over a generic `increment` — the shape
/// `data.fifo` uses, which declined 18 corpus programs at `sync.atomic`'s
/// `(T) -> T modify` and then ~20 at the call through the closure
/// (2026-09-23). Two links, each red on its own:
///
/// * `Closure`'s `fn` is a value parameter typed by the earlier
///   `FunctionType`, checked as `validate typ@FunctionType{}` once
///   substituted — so a generic wrapper, `(C, auto) -> auto`, is the
///   template the reference lets through, not a mismatch that rejects the
///   instance;
/// * the copy's `modify(state)` names the generic wrapper, declared in the
///   module that *called* `atomically`, which elaborates after `h.at`:
///   each program pass starts from the last pass's twins, so the
///   deduction finds the wrapper there and calls its copy, as the
///   reference's whole-program rounds do.
#[test]
fn a_generic_function_given_to_a_higher_order_copy_reaches_its_call() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let at = "module h.at\n    { atomically\n    }\n\ntemplate <typename T, T InitialValue = {}>\ninline T atomically((T) -> T modify)\n{\n    static T state = InitialValue;\n    return modify(state);\n}\n";
    let op = "module h.op\n    { increment\n    }\n\ninline auto increment(auto x)\n{\n    return x + 1;\n}\n";
    let main = "import h.at\nimport h.op\n\ninline void main()\n{\n    uint8 p = atomically<uint8>(increment);\n}\n";
    let (_, declined) = walk_on_base(base, &[("h/at.k", at), ("h/op.k", op)], main);
    assert_eq!(declined, None);
}

/// A function type whose parameter substitutes to `void` — `(T) -> R` in
/// `closure<void, uint32>`'s copy — prints without it: the reference's type
/// drops a `void` parameter (`typeOf p == TVoid = Nothing`, `Type.hs:
/// 1352-1353`) and neither parser accepts one written, so a twin that
/// printed `(C, void) -> uint32` failed to reparse and the whole module lost
/// its twin — `helper/higher_order_functions.k`, which ten corpus programs
/// then declined in (2026-09-23). Red with the parameter printed.
#[test]
fn a_void_parameter_in_a_copy_is_no_parameter() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let cl = "module h.cl\n    { closure\n    }\n\ntemplate <typename T, typename R>\ninline auto closure((T) -> R fn)\n{\n    decltype(fn) x = fn;\n    return x;\n}\n";
    let main = "import h.cl\n\ninline void main()\n{\n    auto x = closure<void, uint32>([]() -> uint32 { return 0; });\n    x();\n}\n";
    let (_, declined) = walk_on_base(base, &[("h/cl.k", cl)], main);
    assert_eq!(declined, None);
}

/// A method call whose method the receiver's class declares only as a
/// template calls the instance its deduction made. `higherOrderFunctions`
/// makes `Go((t) -> void fn)` a member template, and the reference calls
/// `foo.Go<Capture, main@$wrapper$Do>` — `Go<|@@data@closure@core@
/// make_capture0@$@Capture@main@$wrapper$Do|>` (`closures.k::25`, the
/// recorded walk). The route made no copy inside the class's and declined
/// the bare `Go` ("call of a member template", 2026-09-23); the loop
/// deduces and makes it. The twin route, since deleted, was red here.
#[test]
fn a_call_of_a_member_the_class_holds_only_as_a_template_calls_its_copy() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "template <auto N>\nclass Foo\n{\npublic:\n    using t = uint32;\n\n    inline void Go((t) -> void fn)\n    {\n    }\n}\n\ninline void Do(auto x)\n{\n}\n\ninline void main()\n{\n    static Foo<32> foo;\n\n    foo.Go(Do);\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    let name = "ParseIdentifier \"Go<|@@data@closure@core@make_capture0@$@Capture@main@$wrapper$Do|>\"";
    assert_eq!(trace.matches(name).count(), 2, "the copy and its call: {trace}");
}

/// A function given as a template argument through a constant, when the
/// function is a copy of another module's template, is named by that
/// module: `const auto fn = Bar<uint32>;` then `T1<fn>(10)` calls
/// `T1<|@@helper@function_constants@Bar<|$uint@32|>|>`, the name the owner
/// declares the copy under (`getQualifiedName`; `function-constants.k::7`,
/// the recorded walk, 2026-09-23). Unqualified, the call named a copy no
/// one declares. Red with `copy_namespace` left out of `canonical_args`.
#[test]
fn a_function_copy_given_through_a_constant_is_named_by_its_module() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.fc\n    { Bar\n    , T1\n    }\n\ntemplate <typename T>\ninline T Bar(T x)\n{\n    return x;\n}\n\ntemplate <(uint32) -> uint32 F>\ninline uint32 T1(uint32 x)\n{\n    return F(x);\n}\n";
    let main = "import h.fc\n\nconst auto fn = Bar<uint32>;\n\ninline void main()\n{\n    T1<fn>(10);\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/fc.k", helper)], main);
    assert_eq!(declined, None);
    let calls: Vec<&str> = trace.lines().filter(|l| l.contains("ParseIdentifier \"T1<|")).collect();
    assert!(!calls.is_empty(), "{trace}");
    assert!(calls.iter().all(|l| l.contains("T1<|@@h@fc@Bar<|$uint@32|>|>")), "{calls:#?}");
}

/// A function declared inside a member function is a member function —
/// `localMemberFunctions` (`Frontend.hs:219-225`) — so given as an argument
/// it is wrapped capturing `this` and called through it
/// (`Function.hs:406-418`): `Go(Bar)` in `C::Run` makes the closure over
/// `make_capture1<C&>` and the wrapper `$wrapper$Bar`
/// (`local-functions.k::34`, the recorded walk, 2026-09-23). Left a free
/// function, the wrapper called `Run@Bar` with no receiver and the backend
/// answered "Undefined function". Red with the lowering's
/// `in_member_function` never set.
#[test]
fn a_local_function_of_a_method_is_wrapped_as_a_member() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void Go(()->void Do)\n{\n    Do();\n}\n\nclass C\n{\n    void Run()\n    {\n        inline auto Bar()\n        {\n        }\n\n        Go(Bar);\n    }\n}\n\nexport C;\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert!(trace.contains("make_capture1<|$ref@C|>"), "{trace}");
    assert!(trace.contains("\"C@Run@$wrapper$Bar\""), "{trace}");
}

/// A qualifier that names a type alias is the aliased class, by its own
/// path: `bar_t::foo_t` under `using bar_t = Foo<uint32>;` is
/// `Foo<|$uint@32|>::foo_t` to the backend (`using.k::39`, the recorded
/// walk, 2026-09-23), which has no scope named `bar_t` ("Error 24"). Red
/// with `aliased_class_path` answering `None`.
#[test]
fn a_qualifier_through_an_alias_names_the_aliased_class() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "template <typename T>\nclass Foo\n{\npublic:\n    using foo_t = T;\n}\n\ninline void main()\n{\n    using bar_t = Foo<uint32>;\n\n    inline void Do()\n    {\n        bar_t::foo_t x;\n    }\n\n    Do();\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    // The alias's own declaration names `bar_t` once; the qualified use
    // does not name it at all.
    assert_eq!(trace.matches("ParseIdentifier \"bar_t\"").count(), 1, "{trace}");
    let lines: Vec<&str> = trace.lines().filter(|l| !l.contains("Location")).collect();
    let at = lines.iter().position(|l| l.contains("ParseIdentifier \"foo_t\"") && !l.contains("Typedef")).expect("a use of foo_t");
    let use_ = lines.iter().rposition(|l| l.contains("ParseNamedType") && l.contains("main@Do")).expect("the use's type");
    assert!(lines[..use_].iter().rev().take(6).any(|l| l.contains("ParseIdentifier \"Foo<|$uint@32|>\"")), "{:#?}", &lines[at.saturating_sub(4)..use_ + 1]);
}

/// An enum constant in a class template's copy keeps its qualifier in the
/// twin: `Bar::Zero` in `Foo<uint32>::Run`, and `Foo<uint32>::Bar::Zero`
/// in `main`, printed as a bare `Zero` named nothing, and the fold that
/// makes the walk's `EnumValue` found no value (`enum.k::51`, the
/// backend's "Unknown symbol: Zero", 2026-09-23). Red with
/// `written_qualifier_before` demanding the qualifier's minted prefix be
/// written too.
#[test]
fn an_enum_constant_in_a_copy_is_its_value() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "template <typename T>\nclass Foo\n{\npublic:\n    enum Bar : uint1\n    {\n        Zero\n    }\n\n    inline void Run()\n    {\n        Bar b = Bar::Zero;\n    }\n}\n\ninline void main()\n{\n    static Foo<uint32> foo;\n    static Foo<uint32>::Bar bar = Foo<uint32>::Bar::Zero;\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert_eq!(trace.matches("ParseEnumValue").count(), 2, "{trace}");
}

/// A template parameter substituted as a qualifier is printed as the type:
/// `T::B2` in `C<Bar>`'s copy is `Bar::B2` in the twin, where leaf-only
/// printed `B2`, which names nothing (`enum.k::56`, 2026-09-23). The
/// substituted chain keeps the site's expansion. Red without the printer's
/// expansion arm.
#[test]
fn a_substituted_qualifier_is_the_type() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.en\n    { Bar\n    , C\n    }\n\nenum Bar : int32\n{\n    B1,\n    B2 = 3\n}\n\ntemplate <typename T>\nclass C\n{\n    T x = T::B2;\n}\n";
    let main = "import h.en\n\ninline void main()\n{\n    static C<Bar> c;\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/en.k", helper)], main);
    assert_eq!(declined, None);
    assert!(trace.contains("ParseEnumValue"), "{trace}");
}

/// A copy's argument that is itself a struct's copy is spelled by that
/// copy's qualified name in the class's instance type — `parseInstanceType`'s
/// `tIsStruct x` looks through the argument's `TInstance`
/// (`ParseTree.hs:246-247`): `ParseQualifiedName [… "Foo<|$bool$uint@32|>"]`,
/// `ParseTemplateArg "S"` (`template-instance-identifiers.k::20`, the
/// recorded walk). The IR inference types the argument as the copy's note,
/// a `TInstance`; read without its concrete type it was NULL. Red with
/// `typed_instance` matching the argument's type as it stands.
#[test]
fn a_struct_copy_given_as_a_template_argument_is_named_in_the_instance_type() {
    let main = "template <typename T, typename U>\nstruct Foo\n{\n    T a;\n    U b;\n}\n\ntemplate <typename S>\nclass A\n{\n    S s;\n}\n\ninline void main()\n{\n    static A<Foo<bool, uint32>> a;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let arg = calls.iter().position(|c| c.starts_with("ParseTemplateArg \"S\"")).expect("A's instance type");
    assert!(calls[arg - 1].starts_with("ParseQualifiedName") && calls[arg - 1].contains("\"Foo<|$bool$uint@32|>\"]"), "{:?}", &calls[arg - 2..=arg]);
}

/// An enum value converted to an integer parameter **is** its value —
/// `checkConversion n t (EnumValue _ x) | tIsInt t = checkConversion n t x`
/// (`TypeCheck.hs:155-156`) — so `Foo(E::X)` on a `Foo(uint2)` walks the
/// bare literal `7`, typed `uint3` (`conversion-warning.k::4`, the recorded
/// walk), where `T x = T::One` on an enum-typed `T` keeps the `EnumValue`.
/// Red with the walk's conversion arm unasked.
#[test]
fn an_enum_value_converted_to_an_integer_walks_its_value() {
    let main = "enum E : uint3\n{\n    X = 0b111\n}\n\ninline void Foo(uint2 x)\n{\n}\n\ninline void main()\n{\n    Foo(E::X);\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let call = calls.iter().rposition(|c| c.starts_with("ParseFunctionSpecifier")).expect("the call of Foo");
    assert!(
        calls[call + 1].starts_with("ParseDecimalLiteral \"7\"") && calls[call + 2].starts_with("ParseUintType 3"),
        "{:?}",
        &calls[call..call + 4]
    );
}

/// Only a *constant* holding a function is replaced by its initializer
/// (`Constexpr.hs:287-290`, the lookup's `Variable _ _ (Const typ) _ (Just
/// v) _`): a plain member callback initialized from another keeps the
/// other's name, `callback_b = callback_a` (`logic/helper/callback_tests.k`,
/// the recorded walk), where a `const` one is the function itself. Red with
/// the walk replacing any variable with a function initializer.
#[test]
fn only_a_constant_callback_is_replaced_by_its_initializer() {
    let main = "inline bool GreaterThan7(uint32 x)\n{\n    return x > 7;\n}\n\nclass Helper\n{\n    (uint32)->bool callback_a = GreaterThan7;\n    (uint32)->bool callback_b = callback_a;\n    const (uint32)->bool callback_c = GreaterThan7;\n    (uint32)->bool callback_d = callback_c;\n\npublic:\n    bool Test()\n    {\n        return callback_b(8) && callback_d(8);\n    }\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let after_name = |name: &str| {
        let i = calls.iter().position(|c| c.starts_with(&format!("ParseIdentifier \"{name}\""))).expect(name);
        calls[i + 1].clone()
    };
    assert!(after_name("callback_b").starts_with("ParseIdentifier \"callback_a\""), "{}", after_name("callback_b"));
    assert!(after_name("callback_d").starts_with("ParseIdentifier \"GreaterThan7\""), "{}", after_name("callback_d"));
}

/// `checkReturn` converts a `return` statement *directly* in a function's
/// body to the return type (`TypeCheck.hs:76,84`, `renote (fmap $ renote
/// checkReturn) $ body e`): `return Foo::Off;` from a `uint1` function is
/// the bare `1`, while the `return Foo::On;` inside the `if` keeps its
/// `EnumValue` (`kg-oracle --passes 19`: `return Foo::On;` / `return 1;`;
/// `enum.k::52`, the recorded walk). Red with the walk's return
/// conversions unasked (both walk `ParseEnumValue`).
#[test]
fn only_a_return_directly_in_the_body_converts_an_enum_value() {
    let main = "enum Foo : uint1\n{\n    On, Off\n}\n\ninline uint1 Run(bool b)\n{\n    if (b)\n    {\n        return Foo::On;\n    }\n    return Foo::Off;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let returns: Vec<usize> = calls.iter().enumerate().filter(|(_, c)| c.starts_with("ParseReturnExpression")).map(|(i, _)| i).collect();
    assert_eq!(returns.len(), 2, "{calls:?}");
    let before = |i: usize| calls[..i].iter().rev().find(|c| c.starts_with("ParseEnumValue") || c.starts_with("ParseDecimalLiteral")).cloned();
    assert!(before(returns[0]).is_some_and(|c| c.starts_with("ParseEnumValue")), "{:?}", before(returns[0]));
    assert!(before(returns[1]).is_some_and(|c| c.starts_with("ParseDecimalLiteral \"1\"")), "{:?}", before(returns[1]));
}

/// The clause recurses: an enum value whose value is another enum's
/// (`Y = E::X`, folded to `EnumValue F::Y (EnumValue E::X 7)`,
/// `Constexpr.hs:292`) converted to an integer is its innermost literal.
/// Red with the walk looking one level down only (it walks
/// `ParseEnumValue`).
#[test]
fn an_enum_value_of_an_enum_value_converted_to_an_integer_walks_its_value() {
    let main = "enum E : uint3\n{\n    X = 0b111\n}\n\nenum F : uint3\n{\n    Y = E::X\n}\n\ninline void Foo(uint3 x)\n{\n}\n\ninline void main()\n{\n    Foo(F::Y);\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let call = calls.iter().rposition(|c| c.starts_with("ParseFunctionSpecifier")).expect("the call of Foo");
    assert!(calls[call + 1].starts_with("ParseDecimalLiteral \"7\""), "{:?}", &calls[call..call + 4]);
}

/// A class's member variable used inside a lambda in a member template is
/// reached through `$this` — `moveFunctions` makes it `$this.x` in the
/// function hoisted out of the class (`Function.hs:52-57`), so the walk
/// calls `ParseIdentifier "$this"` … `ParseIdentifier "x"`,
/// `ParseAccessMember` (`local-classes.k::15`, the recorded walk). The route
/// left it `main@Foo::x` and declined ("member named through its class",
/// 2026-09-23) rather than hand the backend "Unknown symbol". The twin route,
/// since deleted, was red here.
#[test]
fn a_member_used_in_a_hoisted_lambda_is_reached_through_this() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void Go(() -> void Do)\n{\n}\n\ninline void main()\n{\n    class Foo\n    {\n    public:\n        template <typename T>\n        inline void Run()\n        {\n            Go([]()\n            {\n                x += 2;\n            });\n        }\n\n        uint32 x;\n    }\n\n    static Foo foo;\n    foo.Run<bool>();\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let this = calls.iter().position(|c| c.starts_with("ParseIdentifier \"$this\"")).expect("$this");
    let x = this + calls[this..].iter().position(|c| c.starts_with("ParseIdentifier \"x\"")).expect("x");
    assert!(calls[x + 1].starts_with("ParseAccessMember"), "{:?}", &calls[this..=x + 1]);
}

/// A member access naming another object's method is a method reference:
/// `deduceTemplateArgs` rewrites `MemberAccessF a b`, typed `TUndefined` on
/// a class-typed `a`, to `FunctionSpecifierF (Just a) (Class::b)`
/// (`Type.hs:1390-1391`), so `auto callback = d.c[1].u32_F_u32;` walks as
/// `ParseFunctionSpecifier` over the object (`callbacks.k::5`, the recorded
/// walk, 2026-09-24). As a member access the backend answered "Type error".
/// Red with `names_a_method` answering `false`.
#[test]
fn a_member_access_naming_a_method_is_a_method_reference() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "class C\n{\npublic:\n    inline uint32 u32_F_u32(uint32 x)\n    {\n        return x;\n    }\n}\n\nclass Foo\n{\n    auto callback = d.c[1].u32_F_u32;\n\n    class D\n    {\n    public:\n        C[2] c;\n    }\n\n    D d;\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    let lines: Vec<&str> = trace.lines().filter(|l| !l.contains("Location")).collect();
    // The last mention: the first is the method's own declaration.
    let at = lines.iter().rposition(|l| l.contains("ParseIdentifier \"u32_F_u32\"")).expect("the reference");
    assert!(lines[at + 1].contains("ParseBaseList") && lines[at + 3].contains("ParseFunctionSpecifier"), "{:#?}", &lines[at..at + 5]);
}

/// `data.fifo`'s `atomically<pointer_t>(increment)`, reduced: a class
/// template's member alias given explicitly to a higher-order template,
/// with a generic function as the callback. The written `pointer_t` is
/// spliced to its type at the call, as the reference's inference rounds
/// splice it before the instance is keyed and as the owner keys the copy —
/// `atomically<|$uint@5…|>`, where the name as written mangled
/// `…@F<|@3|>@pointer_t…`, a copy no one made. It was the decline at
/// `fifo.k:64` in 16 corpus programs (2026-09-24). Red with
/// `deduce_rewrite`'s splice removed.
#[test]
fn a_member_alias_and_a_generic_callback_reach_the_higher_order_copy() {
    let base = "module base\n    { module data.closure\n    , module h.at\n    , module h.op\n    }\n\nimport data.closure\nimport h.at\nimport h.op\n";
    let at = "module h.at\n    { atomically\n    }\n\ntemplate <typename T>\ninline T atomically((T) -> T modify)\n{\n    T v;\n    return modify(v);\n}\n";
    let op = "module h.op\n    { increment\n    }\n\ninline auto increment(auto a)\n{\n    return a + 1;\n}\n";
    let main = "template <auto N>\nclass F\n{\npublic:\n    using pointer_t = uint5;\n\n    inline pointer_t get()\n    {\n        return atomically<pointer_t>(increment);\n    }\n}\n\ninline void main()\n{\n    static F<3> f;\n    auto x = f.get();\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/at.k", at), ("h/op.k", op)], main);
    assert_eq!(declined, None);
    assert!(trace.contains("ParseIdentifier \"atomically<|$uint@5"), "{trace}");
}


/// An enum constant as a template argument — `Types<Base::RV32I>::Decoded`,
/// `risc_v/internal/decoder.k:200`, 9 corpus programs declined there
/// (2026-09-24). Three facts, each once a decline: the census's fold of the
/// argument is an enum value, which the route counted unmintable; the
/// reference keeps `EnumValueF x v` there and mangles by the name `x`,
/// qualified where it was written (`Mangle.hs:39`), so the owner's copy and
/// the site are both `Types<|@@h@isa@Base@RV32I|>`; and the copy spells the
/// substituted parameter `Base::RV32I` (`PrettyPrint.hs:98`), where the
/// value or a bare `RV32I` folded nothing. Red with any of the three undone.
#[test]
fn an_enum_valued_template_argument_names_its_copy_by_the_constant() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let isa = "module h.isa\n    { Base\n    , Types\n    }\n\nenum Base : uint2\n{\n    RV32I,\n    RV64I\n}\n\ntemplate <Base ISA>\nclass Types\n{\n    const auto int_size = ISA == Base::RV64I ? 64 : 32;\n\npublic:\n    using uint_t = uint<int_size>;\n\n    struct Decoded\n    {\n        uint_t imm;\n    }\n}\n";
    let main = "import h.isa\n\nstruct Decoded\n{\n    Types<Base::RV32I>::Decoded format;\n    bool store;\n}\n\ninline void main()\n{\n    Decoded d;\n    d.store = true;\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/isa.k", isa)], main);
    assert_eq!(declined, None);
    assert!(trace.contains("ParseIdentifier \"Types<|@@h@isa@Base@RV32I|>\""), "{trace}");
    // One copy, one name: an owner minting the constant by the fold's
    // unqualified path made a second, `Types<|@Base@RV32I|>`, that no site
    // names — the backend's "Error 24" on `risc_v/internal/decoder.k`.
    assert!(!trace.contains("Types<|@Base@RV32I|>"), "{trace}");
}

/// A value-shaped argument that is no enum constant — `C<T::Foo>` with `T`
/// a class, a type named as a value — is not mangled by its name: the
/// reference rejects the program (`template.k::41`), and a copy made of it
/// took the backend down (SIGSEGV, 2026-09-24). Declined.
#[test]
fn a_type_named_as_a_value_argument_makes_no_copy() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "template <typename T>\nclass C\n{\n    T::X x;\n}\n\ntemplate <typename T>\nclass D\n{\n    C<T::Foo> c;\n}\n\nclass Bar\n{\npublic:\n    class Foo\n    {\n    public:\n        using X = uint32;\n    }\n}\n\ninline void main()\n{\n    static D<Bar> c;\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined.as_deref(), Some("template instance"));
}

/// A `static if` in a class template's copy over an imported constant is
/// decided in the twin, where the copy's literal `staticControl` cannot fold
/// the name: `static if (True)` in `C<uint32>::Run` (`True` from the helper)
/// splices its branch, and the lambda inside is hoisted and its closure
/// typed. Left standing, the lambda's function stayed local and the walk
/// declined `fold:decltype` (`static-if.k::63-70`, 8 programs, 2026-09-24).
/// Red with `typed_static_ifs` leaving class copies out.
#[test]
fn a_copy_s_static_if_over_an_imported_constant_is_decided() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.si\n    { True\n    , Exec\n    }\n\nconst bool True = true;\n\ninline void Exec(() -> void Do)\n{\n    Do();\n}\n";
    let main = "import h.si\n\ntemplate <typename T>\nclass C\n{\npublic:\n    inline void Run()\n    {\n        {\n            static if (True)\n            {\n                Exec([](){});\n            }\n        }\n    }\n}\n\ninline void main()\n{\n    static C<uint32> c;\n}\n";
    let (_, declined) = walk_on_base(base, &[("h/si.k", helper)], main);
    assert_eq!(declined, None);
}

/// A copy's substituted array dimension is the number the reference's
/// `evaluateConstexpr` left there: `T[N+1]` in `Fn_T_N<uint4[3], 10>`'s copy
/// is `uint4[(10 + 1)][3]` in the twin, walked as `ParseDecimalLiteral "11"`
/// (`template-argument-deduction.k::53`, the recorded walk, 2026-09-24).
/// The twin's fold keys values by ranges the printed parenthesis does not
/// line up with, and the walk declined `fold:array dimension` (11 programs).
/// Red with `dimension_value` reading the fold alone.
#[test]
fn a_copy_s_literal_dimension_is_its_value() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.tad\n    { Fn_T_N\n    }\n\ntemplate <typename T, auto N>\ninline T[N+1] Fn_T_N(T[N] x)\n{\n    T[N+1] y;\n    y[0] = x[0];\n    return y;\n}\n";
    let main = "import h.tad\n\ninline uint4[11][3] Run()\n{\n    uint4[10][3] x;\n    return Fn_T_N(x);\n}\n";
    let (trace, declined) = walk_on_base(base, &[("h/tad.k", helper)], main);
    assert_eq!(declined, None);
    assert!(trace.contains("ParseDecimalLiteral \"11\""), "{trace}");
}


/// An enum constant converted at a declaration is walked as its value, a
/// literal typed by the enum's base: `uint3 x = Foo::One;` over `enum Foo
/// : uint16 { One = 1 }` is `ParseDecimalLiteral "1"` typed `uint16`
/// (`conversion-warning.k::14`, the recorded walk) — `checkConversion`'s
/// literal arm over the `EnumValue` the fold left (`Constexpr.hs:292`).
/// Since 2026-10-10 that value is read off the loop's tree, not the CST
/// folder (sprint W3.6). Red without the walk's `EnumValue` arm: the
/// initializer is walked as the enum value, typed first.
#[test]
fn an_enum_constant_converted_at_a_declaration_is_its_literal() {
    let main = "enum Foo : uint16\n{\n    One = 1\n}\n\ninline void main()\n{\n    uint3 x = Foo::One;\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let at = calls.iter().position(|c| c.starts_with("ParseDeclare") && c.contains("[\"main\",\"$\"]")).expect("x is declared");
    let before: Vec<&str> = calls[at - 3..at].iter().map(|c| c.split(" -> ").next().unwrap_or(c)).collect();
    assert_eq!(before[0], "ParseDecimalLiteral \"1\"", "{calls:#?}");
    assert_eq!(before[1], "ParseUintType 16", "{calls:#?}");
    assert!(before[2].starts_with("SetNodeType"), "{calls:#?}");
}

/// A `static assert` over a constant whose initializer needs a width folds
/// in the walk: `const auto x = bitsizeof Foo; static assert(x == 1);` with
/// `Foo` declared after it; the walk once declined `fold:static assert`
/// here (`type-identifier.k::3-7`, 2026-09-24). Since 2026-10-10 the
/// walk's folds are the loop's alone (sprint W3.6), and this pins that the
/// loop's tree holds the folded assertion.
#[test]
fn a_static_assert_over_a_width_constant_folds() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "const auto x = bitsizeof Foo;\nstatic assert(x == 1);\n\ninline void main()\n{\n    static Foo foo;\n}\n\nstruct Foo\n{\n    bool x;\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
}


/// A conditional whose arms fold is resolved, as the reference's bottom-up
/// fold has made its arms literals by the time it asks: `static assert(
/// bitsizeof x > bitsizeof y ? bitsizeof x == 4 : bitsizeof y == 2)` folds,
/// where each arm — a comparison of two constants — types unresolved here
/// and the mux was refused (`template-argument-deduction.k::41-44`,
/// 2026-09-24). Red with `eval_mux` gated on the table alone.
#[test]
fn a_conditional_whose_arms_fold_is_a_constant() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void main()\n{\n    uint4 x = 1;\n    uint2 y = 1;\n    static assert(bitsizeof x > bitsizeof y ? bitsizeof x == 4 : bitsizeof y == 2);\n}\n";
    let (_, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
}

/// A value parameter whose written type names an earlier parameter is
/// substituted before its argument is validated (`resolveTemplateParamType`,
/// `TypeCheck.hs:349-362`): `uint<bitsizeof T> N` under `T = uint8`, `uint<
/// clog2(N)> M` under `N = 10`, and a function-typed parameter `(uint<
/// bitsizeof T>) -> void F`. The census declined every such template, and
/// the walk declined the instance (`template.k::3,8,23,24`, 2026-09-24).
/// Red with `substitute_dependent_kind` answering `None`.
#[test]
fn a_parameter_typed_by_an_earlier_one_decides_its_instance() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    for main in [
        "template <typename T, uint<bitsizeof T> N>\nclass Foo\n{\n}\n\ninline void main()\n{\n    static Foo<uint8, 0> foo;\n}\n",
        "template <auto N, uint<clog2(N)> M>\nclass Foo\n{\n}\n\ninline void main()\n{\n    static Foo<10, 0> foo;\n}\n",
        "template <typename T, (uint<bitsizeof T>) -> void F>\nclass Foo\n{\n}\n\ninline void Fn(uint8 x)\n{\n}\n\ninline void main()\n{\n    static Foo<uint8, Fn> foo;\n}\n",
        "template <typename T, (int<clog2(bytesizeof (uint<bitsizeof T>)) + 1>) -> void F>\nclass Foo\n{\n}\n\ninline void Fn(int1 x)\n{\n}\n\ninline void main()\n{\n    static Foo<uint8, Fn> foo;\n}\n",
    ] {
        let (_, declined) = walk_on_base(base, &[], main);
        assert_eq!(declined, None, "{main}");
    }
}

/// A template-template argument the template's own module cannot spell —
/// a root file's `Foo` (`template-template-parameter.k::15`), or
/// `m.mem` from a module the template's does not import (`::13,14`) —
/// travels into the copy as an alias for its qualified path. The copy's
/// `TT<…>` sorts and decides as that template's instance, is handed to
/// the template's owner, and resolves to the name the owner declares its
/// copy under: `m::mem<|…|>`, the namespace kept as the prefix.
#[test]
fn a_template_template_argument_from_an_unimported_module_is_made() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let helper = "module h.t\n    { T3\n    , T4\n    }\n\ntemplate <auto N, template <typename, auto> typename TT>\nclass T3\n{\npublic:\n    TT<uint32, N> x;\n}\n\ntemplate <typename T, template <T> typename TT>\nclass T4\n{\npublic:\n    TT<false> x;\n}\n";
    let mem = "module m\n    { mem\n    }\n\ntemplate <typename T, auto N>\nclass mem\n{\npublic:\n    T[N] x;\n}\n";
    for main in [
        "import h.t\nimport m\n\ninline void main()\n{\n    static T3<15, mem> a;\n}\n",
        "import h.t\n\ntemplate <bool B>\nstruct Foo\n{\n    uint<B ? 32 : 64> x;\n}\n\ninline void main()\n{\n    static T4<bool, Foo> t;\n}\n",
    ] {
        let (_, declined) = walk_on_base(base, &[("h/t.k", helper), ("m.k", mem)], main);
        assert_eq!(declined, None, "{main}");
    }
}

/// A node a rewrite minted wears its donor's note, as the reference's
/// `copyNote` does: the `cast<Closure<…>>(make_capture0(…))` the wrapper
/// rewrite puts in `Go(DoIt)` stands where `DoIt` is written — the
/// reference's own trace locates the same cast at its lambda
/// (`higher-order-functions.k::0`, `SetLocation2 @…:7:17` before
/// `ParseCast`). Donated nodes went out with `UnknownLocation`, and an
/// assertion's call trace read "in file " with no file behind a wrapper
/// frame (`cargo xtask output`, `test/logic/static_local.k`).
#[test]
fn a_minted_node_is_located_at_its_donor() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void Go(() -> void f)\n{\n    f();\n}\n\ninline void DoIt()\n{\n}\n\ninline void main()\n{\n    Go(DoIt);\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    let lines: Vec<&str> = trace.lines().collect();
    let cast = lines.iter().rposition(|l| l.contains(" ParseCast ")).expect("main's closure cast");
    assert!(
        lines[cast - 1].contains("SetLocation2 @") && lines[cast - 1].contains(":12:8-"),
        "the cast at `DoIt`: {}",
        lines[cast - 1]
    );
}

/// A call initialising an `auto` declaration is deduced from its arguments
/// and its defaults alone: the reference never stamps an `auto` context
/// (`Type.hs:1121,1147-1148,1285-1286`), so the context informs nothing and
/// `U = uint<bitsizeof T + 2>`, `N = bitsizeof U + 3` fill from `T = bool`
/// (`template-param-default.k::1`, declined as an uninformed call before).
#[test]
fn a_call_initialising_an_auto_declaration_takes_its_defaults() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "template <typename T, typename U = uint<bitsizeof T + 2>, auto N = bitsizeof U + 3>\ninline int<N> Foo(T x)\n{\n    return 0;\n}\n\ninline void main()\n{\n    const auto x = Foo(true);\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert!(calls(&trace).iter().any(|c| c.contains("\"Foo<|$bool$uint@3@6|>\"")), "the copy Foo<bool, uint3, 6>");
}

/// An `auto` declaration whose initializer folds is typed by the literal
/// it folds to — `evaluateConstexpr` replaced it at pass 6, before
/// `deduceAuto` read it — spelled `const uint3` and noted on the
/// declaration (`constexpr.k::174`, the recorded walk), where the table
/// types a `clog2` `Unresolved` and the walk declined "deduce: Unresolved".
#[test]
fn an_auto_declaration_is_typed_by_its_folded_initializer() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "const int8 a = 127;\nconst auto x = clog2(a);\n\nstatic assert(x == 7);\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    let x = calls.iter().position(|c| c.starts_with("ParseIdentifier \"x\"")).expect("x");
    let declare = x + calls[x..].iter().position(|c| c.starts_with("ParseDeclare")).expect("x's declaration");
    assert!(calls[..declare].iter().rev().take(6).any(|c| c.starts_with("ParseConst")), "{:?}", &calls[declare.saturating_sub(6)..=declare]);
    assert!(
        calls.get(declare + 1).is_some_and(|c| c.starts_with("ParseUintType 3")) && calls.get(declare + 2).is_some_and(|c| c.starts_with("SetNodeType")),
        "{:?}",
        &calls[declare..declare + 3]
    );
}

/// A function-typed parameter's default naming a function — `(T) -> U Fn =
/// F1` — binds it by name, so `Bar<uint14, uint13>(10)` deduces and walks
/// (`template-param-default.k::11`). One not of the parameter's type under
/// the bindings — `Bar<uint14, uint12>` — is the reference's "Invalid
/// argument for template parameter `Fn`" (`::13`), never an instance: it
/// declines rather than compile.
#[test]
fn a_function_default_binds_by_name_when_its_type_fits() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = |u: &str| {
        format!("inline uint13 F1(uint14 x)\n{{\n    return x;\n}}\n\ntemplate <typename T, typename U, (T) -> U Fn = F1>\ninline U Bar(T x)\n{{\n    return Fn(x);\n}}\n\ninline void main()\n{{\n    const auto x = Bar<uint14, {u}>(10);\n}}\n")
    };
    let (trace, declined) = walk_on_base(base, &[], &main("uint13"));
    assert_eq!(declined, None);
    // Named as the default is written — bare — as `substituteDefault`
    // splices it (the recorded walk's `Bar<|$uint@14$uint@13@F1|>`).
    assert!(calls(&trace).iter().any(|c| c.contains("\"Bar<|$uint@14$uint@13@F1|>\"")), "the copy's bare name");
    let (_, declined) = walk_on_base(base, &[], &main("uint12"));
    assert!(declined.is_some(), "a default of the wrong type makes no copy");
}

/// A call argument the table leaves unresolved but the fold decides is
/// typed by its literal, as the reference's pass 6 left it before pass 4
/// deduced: `Show(clog2(x))` over a `const int32 x = 3` binds `uint2` and
/// makes `Show<|$uint@2|>` (`basics.k::22`, `println` there), where the
/// table's `clog2` is `Unresolved` and the call declined.
#[test]
fn a_folding_argument_deduces_by_its_literal() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void Show(auto x)\n{\n}\n\ninline void main()\n{\n    const int32 x = 3;\n    Show(clog2(x));\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None);
    assert!(calls(&trace).iter().any(|c| c.contains("\"Show<|$uint@2|>\"")), "the copy Show<uint2>");
}

/// A member template named bare inside its own class is that class's
/// member — `lookupInScope` tries the enclosing scopes first
/// (`Symbols.hs:82-87`) — so `Go<2>(x)` in `Foo::Run` decides `Foo::Go<2>`
/// (`callbacks.k::34`'s `max_threads_N_u32_F_u32<2>`), even beside a
/// top-level template of the same name. And a receiver call's member path
/// is the call's candidate only: `c.template Go<0>(x)` over a `C` with its
/// own `Go` never decides the top-level `Go<0>` (`test/library/control/
/// async/rendezvous/lenient.k`, whose `data.optional::join` it had made).
#[test]
fn a_bare_member_template_resolves_in_its_class() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let top = "template <typename T>\ninline T Go(T x)\n{\n    return x;\n}\n\n";
    let main = format!("{top}class Foo\n{{\npublic:\n    template <auto N>\n    uint32 Go(uint32 x)\n    {{\n        return x + N;\n    }}\n\n    uint32 Run(uint32 x)\n    {{\n        return Go<2>(x);\n    }}\n}}\n\nclass Bar\n{{\npublic:\n    void main()\n    {{\n        static Foo foo;\n        foo.template Go<0>(1);\n        foo.Run(1);\n    }}\n}}\n\nexport Bar;\n");
    let (trace, declined) = walk_on_base(base, &[], &main);
    assert_eq!(declined, None);
    let calls = calls(&trace);
    assert!(calls.iter().any(|c| c.contains("\"Go<|@2|>\"")), "Foo's member copy Go<2>");
    assert!(!calls.iter().any(|c| c.contains("\"Go<|@0|>\"") && c.starts_with("ParseFunction ") && !c.contains("Foo")), "no top-level Go<0>");
}

/// The walk's types are the IR inference's (`kg_sem`), which looks a name up
/// the way `inferExpType`'s `Variable` arm does — `lookupInScope` from the
/// use's own scope (`Symbols.hs:82-87`). `switch(x)` names the parameter:
/// the two `x`s declared in its cases are filed under the cases' scopes, so
/// the scrutinee is `uint32`, stamped (`statement.k::13`, the recorded
/// walk). Red with the walk reading the position-keyed table first, which
/// has no type for this `x`: `ParseNamedVariable` follows the scoped name
/// with no stamp between.
#[test]
fn a_name_shadowed_only_in_inner_cases_is_typed_as_the_outer_declaration() {
    let main = "inline void Run(uint32 x)\n{\nswitch(x)\n{\ncase 0:\nbool x;\nx = true;\nbreak;\ndefault:\nfloat32 x;\nx = 3.14;\nbreak;\n}\n}\n";
    let (trace, declined) = walk_to(main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    assert_eq!(after(&calls, "ParseScopedIdentifier"), Some("ParseUintType 32"), "{calls:#?}");
}

/// The walk's folds are the IR inference's (`kg_sem`): `evaluateConstexpr`
/// rewrote `static(i)` in each unrolled iteration's hoisted lambda into
/// that iteration's literal (`Constexpr.hs:117-283`), and the walk reads the
/// literal off the node (`static-for.k::19`, the recorded walk: `x` is
/// `ParseDecimalLiteral "0"` in `main@?lambda0$i0`). Red with the walk
/// asking only its own fold environment, which cannot see the iteration's
/// value there: the initializer is walked as a call of `main`.
#[test]
fn a_static_in_an_unrolled_lambda_is_its_iterations_literal() {
    let base = "module base\n    { module data.closure\n    }\n\nimport data.closure\n";
    let main = "inline void main()\n{\nstatic for (const auto i : 2)\n{\nconst auto fn = []()\n{\nauto x = static(i);\n};\n\nfn();\n}\n}\n";
    let (trace, declined) = walk_on_base(base, &[], main);
    assert_eq!(declined, None, "the walk goes through");
    let calls = calls(&trace);
    let initializers: Vec<_> = calls
        .windows(2)
        .filter(|w| w[0].starts_with("ParseIdentifier \"x\""))
        .map(|w| w[1].split(" -> ").next().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(initializers, ["ParseDecimalLiteral \"0\"", "ParseDecimalLiteral \"1\""], "{calls:#?}");
}

/// A file with a header is `Seq [ModuleDecl …]` (`moduleDecl`,
/// `Parser.hs:188`), and `checkRedundantImports` seeds that outer `Seq`
/// (`Parser.hs:235`), not the module's body: the seed stands before the
/// module and is walked before the namespace's name
/// (`imports.k::43`'s `a1/a6.k`, the recorded walk). Red with the seed put
/// into the module body.
#[test]
fn a_headed_modules_seed_is_walked_before_its_namespace() {
    let lib = "module lib.m\n    { F\n    }\n\nimport data.closure\n\ninline uint32 F() { return 1; }\n";
    let main = "import lib.m\n\ninline void main()\n{\n    uint32 x = F();\n}\n";
    let (db, root) = program(&[("lib/m.k", lib)], main);
    let (mut trace, text) = trace_backend();
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    kg_ptbridge::program::run(&db, &[root], options, None, &mut trace);
    trace.flush();
    let calls = calls(&text());
    let name = calls.iter().position(|c| c.starts_with("ParseIdentifier \"@lib@m\"")).expect("the namespace's name");
    assert!(calls[..name].last().is_some_and(|c| c.starts_with("ParseBaseList")), "the seed just before the name: {calls:#?}");
}

/// The walk's redundant-import seed is a setting
/// (`kg_config::Verdicts::ptbridge_seed`, `KG_PTBRIDGE_SEED=0` at the front
/// door; sprint W8.2): an import nothing uses seeds the file's `Seq` with
/// the parser's empty one (`checkRedundantImports`, `Parser.hs:234-241`),
/// and with the setting cleared, on the same database, the walk leaves it
/// out. Red with the setting not reaching the walk.
#[test]
fn the_redundant_import_seed_is_a_setting() {
    let main = "import data.closure\n\ninline void main()\n{\n}\n";
    let (mut db, root) = program(&[], main);
    let traced = |db: &kg_db::Db| {
        let (mut trace, text) = trace_backend();
        let options = kg_backend_sys::options::OptionValues::harness_defaults();
        kg_ptbridge::program::run(db, &[root], options, None, &mut trace);
        trace.flush();
        calls(&text())
    };
    let lists = |calls: &[String]| calls.iter().filter(|c| c.starts_with("ParseBaseList")).count();
    let seeded = traced(&db);
    let mut settings = db.settings().clone();
    settings.verdicts.ptbridge_seed = false;
    db.set_settings(settings);
    let bare = traced(&db);
    assert_eq!(lists(&seeded), lists(&bare) + 1, "{seeded:#?}\n{bare:#?}");
}
