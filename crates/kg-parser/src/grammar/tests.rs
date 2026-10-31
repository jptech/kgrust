//! Grammar tests.
//!
//! Three layers, tightest first:
//!
//! 1. one test per production, asserting the *shape* of the tree — a compact
//!    s-expression in which single-token wrappers collapse to their text, so
//!    `a + b * c` reads as `(BIN_EXPR a + (BIN_EXPR b * c))`;
//! 2. the two decisions that have no local answer — operator precedence and
//!    "is this `<` an angle bracket" — pinned exhaustively, including the
//!    inputs that must be *rejected*;
//! 3. [`corpus_parses_cleanly`], the gate: every `.k` file the reference ships
//!    must round-trip token-for-token and, when it is a whole program, parse
//!    with zero errors.

use std::path::{Path, PathBuf};

use kg_base::language::{Feature, Features};

use crate::{Input, LexedStr, StepOwned, SyntaxKind, parse_input, parse_source_file, to_steps};
use SyntaxKind::*;

// ---------------------------------------------------------------- harness

#[derive(Debug)]
enum Node {
    Token(SyntaxKind, String),
    Tree(SyntaxKind, Vec<Node>),
}

/// Wrappers that carry no information once they hold a single token: printing
/// `(PATH (PATH_SEGMENT (NAME_REF a)))` everywhere would bury the shape that is
/// actually under test.
fn is_transparent(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        PATH | PATH_SEGMENT
            | NAME_REF
            | NAME
            | LITERAL
            | PATH_TYPE
            | INT_TYPE
            | BOOL_TYPE
            | VOID_TYPE
            | AUTO_TYPE
            | STRING_TYPE
            | FLOAT_TYPE
    )
}

fn parse(src: &str) -> (Node, Vec<String>) {
    parse_in(src, Features::NONE)
}

/// [`parse`] in a language beyond the reference.
fn parse_in(src: &str, features: Features) -> (Node, Vec<String>) {
    let lexed = LexedStr::new(src);
    let mut input = Input::from_lexed(&lexed);
    input.set_features(features);
    let events = parse_input(&input);
    let steps = to_steps(&lexed, events);

    let mut stack: Vec<(SyntaxKind, Vec<Node>)> = Vec::new();
    let mut errors = Vec::new();
    let mut root = None;
    let mut text = String::new();

    for step in steps {
        match step {
            StepOwned::Enter(kind) => stack.push((kind, Vec::new())),
            StepOwned::Token(kind, t) => {
                text.push_str(&t);
                if !kind.is_trivia() {
                    stack.last_mut().expect("token outside the root").1.push(Node::Token(kind, t));
                }
            }
            StepOwned::Exit => {
                let (kind, children) = stack.pop().expect("unbalanced exit");
                let node = Node::Tree(kind, children);
                match stack.last_mut() {
                    Some(parent) => parent.1.push(node),
                    None => root = Some(node),
                }
            }
            StepOwned::Error(msg, offset) => errors.push(format!("{}: {msg}", u32::from(offset))),
        }
    }
    assert!(stack.is_empty(), "unbalanced tree");
    assert_eq!(text, src, "the parse is not lossless");
    (root.expect("no root node"), errors)
}

/// The single token a chain of transparent wrappers holds, if that is all it
/// holds. `PATH > PATH_SEGMENT > NAME_REF > IDENT` collapses to `a`, while
/// `PATH > PATH_SEGMENT > {NAME_REF, TEMPLATE_ARG_LIST}` does not.
fn lone_token(node: &Node) -> Option<&str> {
    match node {
        Node::Token(_, text) => Some(text),
        Node::Tree(kind, children) if is_transparent(*kind) && children.len() == 1 => {
            lone_token(&children[0])
        }
        _ => None,
    }
}

fn sexpr(node: &Node) -> String {
    if let Some(text) = lone_token(node) {
        return text.to_owned();
    }
    match node {
        Node::Token(_, text) => text.clone(),
        Node::Tree(kind, children) => {
            let inner = children.iter().map(sexpr).collect::<Vec<_>>().join(" ");
            if inner.is_empty() {
                format!("({kind:?})")
            } else {
                format!("({kind:?} {inner})")
            }
        }
    }
}

/// The node skeleton alone, with every token dropped — for comparing two
/// spellings of the same construct.
fn shape(node: &Node) -> String {
    match node {
        Node::Token(..) => String::new(),
        Node::Tree(kind, children) => {
            let inner = children
                .iter()
                .map(shape)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if inner.is_empty() {
                format!("({kind:?})")
            } else {
                format!("({kind:?} {inner})")
            }
        }
    }
}

#[track_caller]
fn tree_shape(src: &str) -> String {
    let (root, errors) = parse(src);
    assert!(errors.is_empty(), "unexpected errors in {src:?}: {errors:#?}");
    shape(&root)
}

/// The whole file's tree, as one line.
#[track_caller]
fn tree(src: &str) -> String {
    let (root, errors) = parse(src);
    assert!(errors.is_empty(), "unexpected errors in {src:?}: {errors:#?}");
    sexpr(&root)
}

/// Parses, asserts losslessness, and returns nothing — the assertion is that
/// there were no diagnostics.
#[track_caller]
fn clean(src: &str) {
    let (_, errors) = parse(src);
    assert!(errors.is_empty(), "unexpected errors in {src:?}: {errors:#?}");
}

/// Parses and asserts at least one diagnostic. Used for the constructs the
/// reference rejects.
#[track_caller]
fn rejected(src: &str) {
    let (_, errors) = parse(src);
    assert!(!errors.is_empty(), "expected a diagnostic for {src:?}");
}

/// The tree of `src` used as an expression, with the `static assert(...)`
/// scaffolding stripped off.
#[track_caller]
fn expr(src: &str) -> String {
    let program = format!("static assert({src});");
    let (root, errors) = parse(&program);
    assert!(errors.is_empty(), "unexpected errors in expression {src:?}: {errors:#?}");
    let Node::Tree(_, top) = &root else { unreachable!() };
    let Some(Node::Tree(STATIC_ASSERT_DECL, parts)) = top.first() else {
        panic!("expected a static assert, got {}", sexpr(&root))
    };
    let inner: Vec<String> = parts
        .iter()
        .skip_while(|n| !matches!(n, Node::Token(L_PAREN, _)))
        .skip(1)
        .take_while(|n| !matches!(n, Node::Token(R_PAREN, _)))
        .map(sexpr)
        .collect();
    inner.join(" ")
}

/// The tree of `src` used as a statement, with the enclosing function stripped.
#[track_caller]
fn stmt(src: &str) -> String {
    let program = format!("inline void f() {{ {src} }}");
    let (root, errors) = parse(&program);
    assert!(errors.is_empty(), "unexpected errors in statement {src:?}: {errors:#?}");
    let Node::Tree(_, top) = &root else { unreachable!() };
    let Some(Node::Tree(FN_DECL, parts)) = top.first() else {
        panic!("expected a function, got {}", sexpr(&root))
    };
    let Some(Node::Tree(BLOCK_STMT, body)) = parts.last() else {
        panic!("expected a body, got {}", sexpr(&root))
    };
    body.iter()
        .filter(|n| !matches!(n, Node::Token(L_CURLY | R_CURLY, _)))
        .map(sexpr)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tree of `src` used as a type, with the alias scaffolding stripped.
#[track_caller]
fn type_of(src: &str) -> String {
    let program = format!("using T = {src};");
    let (root, errors) = parse(&program);
    assert!(errors.is_empty(), "unexpected errors in type {src:?}: {errors:#?}");
    let Node::Tree(_, top) = &root else { unreachable!() };
    let Some(Node::Tree(ALIAS_DECL, parts)) = top.first() else {
        panic!("expected an alias, got {}", sexpr(&root))
    };
    parts
        .iter()
        .skip_while(|n| !matches!(n, Node::Token(EQ, _)))
        .skip(1)
        .take_while(|n| !matches!(n, Node::Token(SEMICOLON, _)))
        .map(sexpr)
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------- files

#[test]
fn empty_file() {
    assert_eq!(tree(""), "(SOURCE_FILE)");
    clean("   \n\n  ");
    clean("// just a comment\n");
}

#[test]
fn module_declaration() {
    assert_eq!(
        tree("module base { optional, module .cmdargs }"),
        "(SOURCE_FILE (MODULE_DECL module (MODULE_PATH base) \
         (MODULE_EXPORT_LIST { optional , module (MODULE_PATH . cmdargs) })))"
    );
}

#[test]
fn module_difference_is_flat() {
    // No `MODULE_DIFF` kind exists, so the `\` marks it structurally.
    assert_eq!(
        tree("module hardware.config { module intel.arria-10.config \\ hardware.config }"),
        "(SOURCE_FILE (MODULE_DECL module (MODULE_PATH hardware . config) \
         (MODULE_EXPORT_LIST { module (MODULE_PATH intel . arria - 10 . config) \\ \
         (MODULE_PATH hardware . config) })))"
    );
}

#[test]
fn imports() {
    assert_eq!(
        tree("import .cmdargs\nimport a.b\nimport numeric.float32.operator as F\n"),
        "(SOURCE_FILE (IMPORT_DECL import (MODULE_PATH . cmdargs)) \
         (IMPORT_DECL import (MODULE_PATH a . b)) \
         (IMPORT_DECL import (MODULE_PATH numeric . float32 . operator) as F))"
    );
    // Only at the start of a declaration list (`Parser.hs:243`).
    rejected("using x = uint8;\nimport a.b\n");
}

#[test]
fn stray_semicolons_after_declarations() {
    clean("using a = uint8;;;");
    clean("class C {};");
}

// ------------------------------------------------------------ declarations

#[test]
fn alias_declaration() {
    assert_eq!(tree("using a = uint8;"), "(SOURCE_FILE (ALIAS_DECL using a = uint8 ;))");
}

#[test]
fn enum_declaration() {
    assert_eq!(
        tree("enum E : uint2 { A, B = 3 }"),
        "(SOURCE_FILE (ENUM_DECL enum E : uint2 (DECL_LIST { (ENUM_VARIANT A) , \
         (ENUM_VARIANT B = 3) })))"
    );
    // The underlying type is mandatory (grammar.md §9 trap 11) and the body
    // needs at least one constant.
    rejected("enum E { A }");
    rejected("enum E : uint2 { }");
}

#[test]
fn struct_and_union() {
    assert_eq!(
        tree("struct S { uint8 a; bool b }"),
        "(SOURCE_FILE (STRUCT_DECL struct S (DECL_LIST { (VAR_DECL uint8 a) ; \
         (VAR_DECL bool b) })))"
    );
    clean("struct S { }");
    clean("union U { uint8 a; }");
    // Members carry neither an initializer nor attributes (§9 trap 12).
    rejected("struct S { uint8 a = 1; }");
    rejected("struct S { [[name(\"x\")]] uint8 a; }");
}

#[test]
fn class_with_members() {
    clean(
        "class C {
            private:
            uint8 _x;
            public:
            [[name(\"go\")]] uint8 y = 1;
            void run() { }
            uint8 proto(uint8 a);
            struct Inner { bool b }
            default = { .y = 2 };
        }",
    );
    assert_eq!(
        tree("class C { public: }"),
        "(SOURCE_FILE (CLASS_DECL class C (DECL_LIST { (VISIBILITY public :) })))"
    );
}

#[test]
fn export_and_extern() {
    assert_eq!(tree("export Main;"), "(SOURCE_FILE (EXPORT_DECL export Main ;))");
    assert_eq!(
        tree("[[name(\"top\")]] export Main;"),
        "(SOURCE_FILE (EXPORT_DECL (ATTR_LIST [ [ (ATTR name (ATTR_ARGS ( \"top\" ))) ] ]) \
         export Main ;))"
    );
    clean("extern Foo<uint8>;");
}

#[test]
fn static_assertions() {
    assert_eq!(
        tree("static_assert(true);"),
        "(SOURCE_FILE (STATIC_ASSERT_DECL static_assert ( true ) ;))"
    );
    assert_eq!(
        tree("static assert(true);"),
        "(SOURCE_FILE (STATIC_ASSERT_DECL static assert ( true ) ;))"
    );
}

#[test]
fn global_variable_and_function() {
    assert_eq!(tree("uint8 g = 1;"), "(SOURCE_FILE (VAR_DECL uint8 g = 1 ;))");
    assert_eq!(
        tree("inline void f(uint8 a) { }"),
        "(SOURCE_FILE (FN_DECL (FN_MODIFIER inline) void f \
         (PARAM_LIST ( (PARAM uint8 a) )) (BLOCK_STMT { })))"
    );
    assert_eq!(
        tree("class C { void f(); }"),
        "(SOURCE_FILE (CLASS_DECL class C (DECL_LIST { (FN_PROTO void f (PARAM_LIST ( )) ;) })))"
    );
    clean("noinline uint8 f() { return 1; }");
    clean("void f([[last]] uint8 a) { }");
}

#[test]
fn templates() {
    assert_eq!(
        tree("template<typename T> struct S { T x }"),
        "(SOURCE_FILE (TEMPLATE_DECL (TEMPLATE_PARAM_LIST template < (TYPE_PARAM typename T) >) \
         (STRUCT_DECL struct S (DECL_LIST { (VAR_DECL T x) }))))"
    );
    clean("template<typename T = uint8, uint32 N = 4> class C { }");
    clean("template<template<typename> typename TT> class C { }");
    clean("template<template<> typename TT> class C { }");
    clean("template<typename T> using A = T;");
    clean("template<typename T> inline void f() { }");
    clean("template<typename T> union U { T x }");
    // `template<>` is a syntax error even though `Foo<>` is not (§9 trap 23).
    rejected("template<> class C { }");
}

#[test]
fn default_initialization_has_no_node_kind() {
    // `SyntaxKind` lacks `DEFAULT_INIT`, so this valid construct is an `ERROR`
    // *node* — with no diagnostic attached.
    assert_eq!(tree("class C { default = 1; }"), {
        "(SOURCE_FILE (CLASS_DECL class C (DECL_LIST { (ERROR default = 1 ;) })))"
    });
    clean("inline void f() { static default = { .a = 1 }; }");
}

// ------------------------------------------------------------------- types

#[test]
fn primitive_types() {
    assert_eq!(type_of("void"), "void");
    assert_eq!(type_of("bool"), "bool");
    assert_eq!(type_of("float32"), "float32");
    assert_eq!(type_of("string"), "string");
    assert_eq!(type_of("auto"), "auto");
    assert_eq!(type_of("int32"), "int32");
    assert_eq!(type_of("uint8"), "uint8");
}

#[test]
fn parameterized_int_types() {
    assert_eq!(type_of("int<8>"), "(PARAM_INT_TYPE int < 8 >)");
    assert_eq!(type_of("uint<N>"), "(PARAM_INT_TYPE uint < N >)");
    assert_eq!(type_of("uint<clog2(N)>"), "(PARAM_INT_TYPE uint < (PREFIX_EXPR clog2 (PAREN_EXPR ( N ))) >)");
    // Bare `int` is not a type — the width is required (`Parser.hs:544`).
    rejected("using T = int;");
}

#[test]
fn const_and_array_types() {
    assert_eq!(type_of("const uint8"), "(CONST_TYPE const uint8)");
    assert_eq!(type_of("uint8[4]"), "(ARRAY_TYPE uint8 [ 4 ])");
    assert_eq!(type_of("uint8[4][2]"), "(ARRAY_TYPE uint8 [ 4 ] [ 2 ])");
    assert_eq!(
        type_of("[[memory]] uint32[512]"),
        "(ARRAY_TYPE (ATTR_LIST [ [ (ATTR memory) ] ]) uint32 [ 512 ])"
    );
    clean("class C { [[memory, ecc(scheme)]] uint32[8] m; }");
    // An attributed array takes exactly one dimension (`Parser.hs:529`).
    rejected("using T = [[memory]] uint32[4][2];");
}

#[test]
fn path_and_dependent_types() {
    // A type position wraps the path in `PATH_TYPE`; an expression position
    // leaves it bare. That is the one place the CST still distinguishes them.
    assert_eq!(type_of("a::b::C"), "(PATH_TYPE (PATH a :: b :: C))");
    assert_eq!(
        type_of("Foo<uint8>"),
        "(PATH_TYPE (PATH (PATH_SEGMENT Foo (TEMPLATE_ARG_LIST < uint8 >))))"
    );
    assert_eq!(
        type_of("typename T::type"),
        "(TYPENAME_TYPE typename (PATH_TYPE (PATH T :: type)))"
    );
    assert_eq!(type_of("decltype(x)"), "(DECLTYPE_TYPE decltype ( x ))");
}

#[test]
fn function_types() {
    assert_eq!(
        type_of("(uint8) -> void"),
        "(FN_TYPE (FN_TYPE_PARAM_LIST ( (FN_TYPE_PARAM uint8) )) -> void)"
    );
    assert_eq!(type_of("() -> auto"), "(FN_TYPE (FN_TYPE_PARAM_LIST ( )) -> auto)");
    clean("using T = (uint8 a, [[last]] bool b) -> uint32;");
    clean("using T = [[async, latency(1)]] (uint8) -> void;");
    clean("class C { [[async]] [[no_backpressure]] (uint32) -> void cb; }");
    // A function type may not be a function type's parameter or return type
    // (`Parser.hs:554, 557`).
    rejected("using T = ((uint8) -> void) -> void;");
}

// ------------------------------------------------------------- expressions

#[test]
fn literals() {
    assert_eq!(expr("1"), "1");
    assert_eq!(expr("0x_dead_beef"), "0x_dead_beef");
    assert_eq!(expr("42u32"), "42u32");
    assert_eq!(expr("1.5e3"), "1.5e3");
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("false"), "false");
    assert_eq!(expr("\"xy\""), "\"xy\"");
}

/// `dotSepName` is a lexeme of characters (`Parser/Lexer.hs:230-233`): a
/// dot joins two components only with no space on either side. `a1 .a1`
/// names `a1` (the rest is left behind), and `a1. a1` fails the whole name
/// at its start (`imports.k::117-119`, `kg-oracle --errors`). A `-` part is
/// glued the same way.
#[test]
fn a_module_path_is_one_lexeme() {
    let (errors, _) = parse_errors("import a1.b-2x.c\n");
    assert!(errors.is_empty(), "{errors:?}");
    let (errors, _) = parse_errors("import a1. a1\n");
    assert_eq!(errors.first().map(|e| e.0), Some(7), "{errors:?}");
    let (errors, _) = parse_errors("import a1 .a1\n");
    assert!(errors.first().is_some_and(|e| e.0 > 7), "the name is `a1`, the error after it: {errors:?}");
}

#[test]
fn interpolated_strings() {
    // The hole is an ordinary expression under an `INTERPOLATION`.
    assert_eq!(expr("\"x{y}\""), "(INTERPOLATED_STRING \"x (INTERPOLATION { y }) \")");
    assert_eq!(
        expr("\"{a + 1=}!\""),
        "(INTERPOLATED_STRING \" (INTERPOLATION { (BIN_EXPR a + 1) = }) !\")"
    );
    // Alignment, format, and two adjacent holes.
    assert_eq!(
        expr("\"{v,-5:X4}{w:b}\""),
        "(INTERPOLATED_STRING \" (INTERPOLATION { v , (PREFIX_EXPR - 5) : (FORMAT_SPEC X4) }) \
         (INTERPOLATION { w : (FORMAT_SPEC b) }) \")"
    );
    // A `<` in a hole is a comparison even inside a template argument list:
    // the hole is its own bracket.
    assert!(expr("f<\"{a < b}\">()").contains("(BIN_EXPR a < b)"));
    // A string inside a hole, and a hole inside that.
    assert_eq!(
        expr("\"{f(\"<{x:x8}>\")}\""),
        "(INTERPOLATED_STRING \" (INTERPOLATION { (CALL_EXPR f (ARG_LIST ( \
         (INTERPOLATED_STRING \"< (INTERPOLATION { x : (FORMAT_SPEC x8) }) >\") ))) }) \")"
    );
    // A malformed format is the format grammar's error, not the parser's:
    // everything before the `}` is the spec (`kg_db`'s pass 0 judges it).
    clean("const auto s = \"{x:d 10}\";");
    // Anything else before the `}` is an error, and the string still ends:
    // one error, and the text round-trips.
    let (errors, round_trip) = parse_errors("const auto s = \"{x y}\"; const auto t = 1;");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(round_trip);
}

#[test]
fn paths_and_calls() {
    assert_eq!(expr("a"), "a");
    assert_eq!(expr("a::b"), "(PATH a :: b)");
    assert_eq!(expr("f(1, 2)"), "(CALL_EXPR f (ARG_LIST ( 1 , 2 )))");
    assert_eq!(expr("f()"), "(CALL_EXPR f (ARG_LIST ( )))");
    assert_eq!(expr("a.b"), "(FIELD_EXPR a . b)");
    assert_eq!(expr("a.b(1)"), "(CALL_EXPR (FIELD_EXPR a . b) (ARG_LIST ( 1 )))");
    assert_eq!(expr("a[i]"), "(INDEX_EXPR a [ i ])");
    assert_eq!(expr("a[i].b[j]"), "(INDEX_EXPR (FIELD_EXPR (INDEX_EXPR a [ i ]) . b) [ j ])");
}

/// Edition 1.1's named arguments: a label is an `ARG_LABEL` beside the
/// argument it names, and only in a file whose language has the feature. In
/// the reference a call's arguments are bare expressions (`Parser.hs:871`),
/// so `f(x: 1)` is a parse error there — and still is here, without it.
#[test]
fn named_arguments_parse_only_where_the_language_has_them() {
    let named = Features::NONE.with(Feature::NamedArguments);
    let call = |src: &str| {
        let (root, errors) = parse_in(&format!("static assert({src});"), named);
        (sexpr(&root), errors)
    };
    let (tree, errors) = call("f(y: 2, x: 1)");
    assert_eq!(errors, Vec::<String>::new());
    assert!(tree.contains("(CALL_EXPR f (ARG_LIST ( (ARG_LABEL y :) 2 , (ARG_LABEL x :) 1 )))"), "{tree}");
    let (tree, errors) = call("a.f(1, y: g(z))");
    assert_eq!(errors, Vec::<String>::new());
    assert!(tree.contains("(ARG_LIST ( 1 , (ARG_LABEL y :) (CALL_EXPR g (ARG_LIST ( z ))) ))"), "{tree}");
    assert_eq!(expr("f(1, 2)"), "(CALL_EXPR f (ARG_LIST ( 1 , 2 )))", "positional calls are unchanged");

    assert!(!parse("static assert(f(y: 2));").1.is_empty(), "the reference has no named arguments");
    assert_eq!(call("f(x: 1, x: 2)").1, ["22: the argument `x` is named twice"]);
    assert_eq!(call("f(x: 1, 2)").1, ["22: a positional argument cannot follow a named one"]);
}

#[test]
fn call_modifiers_wrap_the_call() {
    assert_eq!(
        expr("[[call_rate(2), fifo_depth(4)]] f(x)"),
        "(CALL_EXPR (ATTR_LIST [ [ (ATTR call_rate (ATTR_ARGS ( 2 ))) , \
         (ATTR fifo_depth (ATTR_ARGS ( 4 ))) ] ]) f (ARG_LIST ( x )))"
    );
    clean("inline void f() { [[transaction_size(8)]] g(1); }");
}

#[test]
fn builtin_expression_forms() {
    assert_eq!(expr("cast<uint8>(x)"), "(CAST_EXPR cast < uint8 > ( x ))");
    assert_eq!(expr("concat(a)"), "(CONCAT_EXPR concat ( a ))");
    assert_eq!(expr("concat(a, b, c)"), "(CONCAT_EXPR concat ( a , b , c ))");
    assert_eq!(expr("mux(s, a, b)"), "(MUX_EXPR mux ( s , a , b ))");
    assert_eq!(expr("lutmul(a, b)"), "(LUT_MUL_EXPR lutmul ( a , b ))");
    assert_eq!(expr("fan_out<4>(x)"), "(FAN_OUT_EXPR fan_out < 4 > ( x ))");
    assert_eq!(expr("static(x)"), "(STATIC_EXPR static ( x ))");
    assert_eq!(expr("bitoffsetof(S, f)"), "(BIT_OFFSET_EXPR bitoffsetof ( S , f ))");
    assert_eq!(expr("byteoffsetof(S, f)"), "(BYTE_OFFSET_EXPR byteoffsetof ( S , f ))");
    // `mux` needs at least two arms, `lutmul` exactly two (`Parser.hs:866, 870`).
    rejected("static assert(mux(s));");
    rejected("static assert(lutmul(a));");
    rejected("static assert(concat());");
}

#[test]
fn initializer_lists() {
    // `{}` is the *designated* form (§9 trap 7); both land in one node.
    assert_eq!(expr("{}"), "(INIT_LIST_EXPR { })");
    assert_eq!(expr("{1, 2}"), "(INIT_LIST_EXPR { 1 , 2 })");
    assert_eq!(
        expr("{.x = 1, .y = 2}"),
        "(INIT_LIST_EXPR { (DESIGNATOR . x = 1) , (DESIGNATOR . y = 2) })"
    );
    assert_eq!(
        expr("{.a = {.b = 1}}"),
        "(INIT_LIST_EXPR { (DESIGNATOR . a = (INIT_LIST_EXPR { (DESIGNATOR . b = 1) })) })"
    );
    // Trailing commas are not allowed in either shape (`Parser.hs:860`).
    rejected("uint8 x = {1, };");
    rejected("uint8 x = {.x = 1, };");
    rejected("uint8 x = {, };");
    // `try designatedInitializerList <|> initializerList` (`Parser.hs:850-852`)
    // are two whole-list productions; there is no third reading that mixes
    // them, in either order.
    rejected("inline void f() { T v = {a, .x = 1}; }");
    rejected("inline void f() { T v = {.x = 1, b}; }");
}

#[test]
fn lambdas() {
    assert_eq!(
        expr("[]() -> uint8 { return 1; }"),
        "(LAMBDA_EXPR (CAPTURE_LIST [ ]) (PARAM_LIST ( )) -> uint8 \
         (BLOCK_STMT { (RETURN_STMT return 1 ;) }))"
    );
    assert_eq!(
        expr("[a, b] { }"),
        "(LAMBDA_EXPR (CAPTURE_LIST [ (CAPTURE a) , (CAPTURE b) ]) (BLOCK_STMT { }))"
    );
    clean("inline void f() { auto g = [](uint8 x) { return x; }; }");
}

#[test]
fn parens_and_ternary() {
    assert_eq!(expr("(a)"), "(PAREN_EXPR ( a ))");
    assert_eq!(expr("a ? b : c"), "(TERNARY_EXPR a ? b : c)");
}

#[test]
fn prefix_operators() {
    assert_eq!(expr("-a"), "(PREFIX_EXPR - a)");
    assert_eq!(expr("!a"), "(PREFIX_EXPR ! a)");
    assert_eq!(expr("~a"), "(PREFIX_EXPR ~ a)");
    assert_eq!(expr("bitsizeof a"), "(PREFIX_EXPR bitsizeof a)");
    assert_eq!(expr("bytesizeof a"), "(PREFIX_EXPR bytesizeof a)");
    assert_eq!(expr("clog2 a"), "(PREFIX_EXPR clog2 a)");
    // `clog2(x)` is the operator applied to a parenthesized term, not a call.
    assert_eq!(expr("clog2(x)"), "(PREFIX_EXPR clog2 (PAREN_EXPR ( x )))");
    // Prefix operators stack only from a looser level onto a tighter one, and
    // never twice within one level. The table is three levels, tightest first
    // (`Parser.hs:921-930`): `[Neg]`, then `[Not, Invert]`, then
    // `[BitSizeof, ByteSizeof, Clog2]`. megaparsec's `Prefix` is not chainable,
    // so at most one operator applies per level.
    assert_eq!(expr("bitsizeof -a"), "(PREFIX_EXPR bitsizeof (PREFIX_EXPR - a))");
    assert_eq!(expr("!-a"), "(PREFIX_EXPR ! (PREFIX_EXPR - a))");
    assert_eq!(expr("~-a"), "(PREFIX_EXPR ~ (PREFIX_EXPR - a))");
    // Same level twice: `!` and `~` are both level 2 (`Parser.hs:923-926`).
    rejected("static assert(!~a);");
    rejected("static assert(~!a);");
    // Tighter level outside a looser one.
    rejected("static assert(-bitsizeof a);");
    rejected("static assert(!bitsizeof a);");
    rejected("static assert(-~a);");
    // `operator "!"` forbids a following `!` (`Parser/Lexer.hs:262`).
    rejected("static assert(!!a);");
    rejected("static assert(~~a);");
    // `x - -y` needs the space; `x--y` lexes the decrement (§9 trap 2).
    assert_eq!(expr("a - -b"), "(BIN_EXPR a - (PREFIX_EXPR - b))");
    rejected("static assert(a--b);");
}

// --------------------------------------------------------------- precedence

#[test]
fn precedence_arithmetic() {
    assert_eq!(expr("a + b * c"), "(BIN_EXPR a + (BIN_EXPR b * c))");
    assert_eq!(expr("a * b + c"), "(BIN_EXPR (BIN_EXPR a * b) + c)");
    assert_eq!(expr("a - b - c"), "(BIN_EXPR (BIN_EXPR a - b) - c)");
    assert_eq!(expr("a / b % c"), "(BIN_EXPR (BIN_EXPR a / b) % c)");
    assert_eq!(expr("-a * b"), "(BIN_EXPR (PREFIX_EXPR - a) * b)");
    assert_eq!(expr("bitsizeof a + 1"), "(BIN_EXPR (PREFIX_EXPR bitsizeof a) + 1)");
    assert_eq!(expr("a * bitsizeof b"), "(BIN_EXPR a * (PREFIX_EXPR bitsizeof b))");
}

#[test]
fn precedence_shift_and_bitwise() {
    // `<<` and `>>` are fused by the parser; the lexer never emits them.
    assert_eq!(expr("a << b + c"), "(BIN_EXPR a << (BIN_EXPR b + c))");
    assert_eq!(expr("a >> b"), "(BIN_EXPR a >> b)");
    assert_eq!(expr("a & b | c"), "(BIN_EXPR (BIN_EXPR a & b) | c)");
    assert_eq!(expr("a | b ^ c"), "(BIN_EXPR a | (BIN_EXPR b ^ c))");
    assert_eq!(expr("a && b || c"), "(BIN_EXPR (BIN_EXPR a && b) || c)");
    assert_eq!(expr("a || b ^^ c"), "(BIN_EXPR a || (BIN_EXPR b ^^ c))");
    assert_eq!(expr("a == b && c"), "(BIN_EXPR (BIN_EXPR a == b) && c)");
}

#[test]
fn precedence_ternary_is_right_associative() {
    assert_eq!(
        expr("a ? b : c ? d : e"),
        "(TERNARY_EXPR a ? b : (TERNARY_EXPR c ? d : e))"
    );
    assert_eq!(expr("a || b ? c : d"), "(TERNARY_EXPR (BIN_EXPR a || b) ? c : d)");
}

#[test]
fn relational_and_equality_are_non_associative() {
    assert_eq!(expr("a < b"), "(BIN_EXPR a < b)");
    assert_eq!(expr("a <= b"), "(BIN_EXPR a <= b)");
    assert_eq!(expr("a >= b"), "(BIN_EXPR a >= b)");
    assert_eq!(expr("a != b"), "(BIN_EXPR a != b)");
    rejected("static assert(a < b < c);");
    rejected("static assert(a == b == c);");
    rejected("static assert(a <= b >= c);");
}

// ------------------------------------------------------------------- angles

#[test]
fn nested_template_arguments() {
    assert_eq!(
        type_of("Foo<Bar<T>>"),
        "(PATH_TYPE (PATH (PATH_SEGMENT Foo (TEMPLATE_ARG_LIST < \
         (PATH (PATH_SEGMENT Bar (TEMPLATE_ARG_LIST < T >))) >))))"
    );
    assert_eq!(
        type_of("Foo<int<8>>"),
        "(PATH_TYPE (PATH (PATH_SEGMENT Foo (TEMPLATE_ARG_LIST < (PARAM_INT_TYPE int < 8 >) >))))"
    );
    // `sepBy`, so an empty argument list is legal.
    assert_eq!(
        type_of("Foo<>"),
        "(PATH_TYPE (PATH (PATH_SEGMENT Foo (TEMPLATE_ARG_LIST < >))))"
    );
}

#[test]
fn comparisons_inside_and_outside_angles() {
    // `>=` is one token, so the final `>` still closes the list.
    assert_eq!(
        type_of("Foo<a >= b>"),
        "(PATH_TYPE (PATH (PATH_SEGMENT Foo (TEMPLATE_ARG_LIST < (BIN_EXPR a >= b) >))))"
    );
    // Parenthesizing restores the full operator table (`Parser.hs:845`).
    clean("using T = Foo<(a < b)>;");
    assert_eq!(expr("a < b"), "(BIN_EXPR a < b)");
    assert_eq!(expr("a >> b"), "(BIN_EXPR a >> b)");
}

/// `a < b && c > d` is a valid expression *and* a valid `a<b && c>` declaring
/// `d`. The reference never even reaches `templateArgs` unless `findSymbol`
/// says the name is a template (`Parser.hs:668-680, 690-697`), so for an
/// ordinary value it applies the operator table and reads two comparisons
/// joined by `&&`. Without a symbol table the position decides instead: in an
/// expression nothing can follow a template argument list that also starts an
/// operand, and `d` is exactly that.
#[test]
fn comparison_pairs_are_not_template_arguments() {
    assert_eq!(
        expr("a < b && c > d"),
        "(BIN_EXPR (BIN_EXPR a < b) && (BIN_EXPR c > d))"
    );
    assert_eq!(
        expr("a < b || c > d"),
        "(BIN_EXPR (BIN_EXPR a < b) || (BIN_EXPR c > d))"
    );
    assert_eq!(
        expr("a < b ? c : d > n"),
        "(TERNARY_EXPR (BIN_EXPR a < b) ? c : (BIN_EXPR d > n))"
    );
    assert_eq!(
        stmt("return a < b && c > d;"),
        "(RETURN_STMT return (BIN_EXPR (BIN_EXPR a < b) && (BIN_EXPR c > d)) ;)"
    );
    // Two arguments, not one template instantiation.
    assert_eq!(
        stmt("g(a < b, c > d);"),
        "(EXPR_STMT (CALL_EXPR g (ARG_LIST ( (BIN_EXPR a < b) , (BIN_EXPR c > d) ))) ;)"
    );
    clean("inline void f() { if (a < b && c > d) { } }");
    clean("inline bool f() { return a < b && c > d(); }");
    // The example in this module's own doc comment, with a name rather than
    // the literal that used to be doing all the work.
    clean("inline void f() { static assert(pos < v.size && v.size > n); }");
    clean("inline void f() { static assert(pos < v.size && v.size > 0); }");

    // …while a template instance used as an operand still reads as one, which
    // is what the deny-list has to keep working
    // (`test/logic/test_cases_24.k:153`).
    assert_eq!(
        expr("bitsizeof S<uint6> - 2"),
        "(BIN_EXPR (PREFIX_EXPR bitsizeof (PATH (PATH_SEGMENT S (TEMPLATE_ARG_LIST < uint6 >)))) - 2)"
    );
    clean("inline void f() { g(Foo<uint8>::v); }");
    clean("inline void f() { g(Foo<uint8>(1)); }");
    // In a *type* position the same shape is a declaration, as before.
    assert_eq!(stmt("Foo<uint8> x;"), "(VAR_DECL (PATH_TYPE (PATH (PATH_SEGMENT Foo \
                                       (TEMPLATE_ARG_LIST < uint8 >)))) x ;)");
}

/// `templateQualifier` is consumed at the head of *every* path segment, not
/// only the first: `prefixedScopedNameAndSymbol` opens with
/// `optional templateQualifier` and recurses into itself after each `::`
/// (`Parser.hs:653, 683`). So `X::template Y<T>` is an ordinary type, and the
/// declaration lookahead has to be able to step over it.
#[test]
fn template_qualifier_after_a_scope() {
    assert_eq!(
        stmt("X::template Y<uint8> v;"),
        "(VAR_DECL (PATH_TYPE (PATH X :: (PATH_SEGMENT template Y \
         (TEMPLATE_ARG_LIST < uint8 >)))) v ;)"
    );
    clean("inline void f() { typename X::template Y<uint8> v; }");
    clean("using T = X::template Y<uint8>;");
    clean("inline void f() { auto z = X::template Y<uint8>(); }");
    clean("inline void f() { obj.template Go<uint32>(); }");
}

#[test]
fn angles_in_call_position() {
    assert_eq!(expr("cast<uint8>(x)"), "(CAST_EXPR cast < uint8 > ( x ))");
    assert_eq!(expr("fan_out<4>(x)"), "(FAN_OUT_EXPR fan_out < 4 > ( x ))");
    assert_eq!(
        expr("f<uint8>(x)"),
        "(CALL_EXPR (PATH (PATH_SEGMENT f (TEMPLATE_ARG_LIST < uint8 >))) (ARG_LIST ( x )))"
    );
}

#[test]
fn dependent_member_template() {
    assert_eq!(
        expr("obj.template Go<uint32>()"),
        "(CALL_EXPR (FIELD_EXPR obj . template Go (TEMPLATE_ARG_LIST < uint32 >)) (ARG_LIST ( )))"
    );
    clean("inline void f() { () -> uint32 cb = obj.template Go<uint32>; }");
}

// -------------------------------------------------------------- statements

#[test]
fn simple_statements() {
    assert_eq!(stmt(";"), "(EMPTY_STMT ;)");
    assert_eq!(stmt("{ }"), "(BLOCK_STMT { })");
    assert_eq!(stmt("barrier;"), "(BARRIER_STMT barrier ;)");
    assert_eq!(stmt("return;"), "(RETURN_STMT return ;)");
    assert_eq!(stmt("return 1;"), "(RETURN_STMT return 1 ;)");
    assert_eq!(stmt("f();"), "(EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;)");
    assert_eq!(stmt("reorder f();"), "(REORDER_STMT reorder (EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;))");
    assert_eq!(stmt("atomic { }"), "(ATOMIC_STMT atomic (BLOCK_STMT { }))");
    assert_eq!(stmt("uint8 x;"), "(VAR_DECL uint8 x ;)");
    assert_eq!(stmt("uint8 x = 1;"), "(VAR_DECL uint8 x = 1 ;)");
    assert_eq!(stmt("static uint8 x = 1;"), "(VAR_DECL static uint8 x = 1 ;)");
    // A declaration whose type starts with `(` — invisible to `at_decl`.
    clean("inline void f() { () -> void cb; }");
}

#[test]
fn assignment_statements() {
    assert_eq!(stmt("x = 1;"), "(ASSIGN_STMT x = 1 ;)");
    assert_eq!(stmt("x[i] = 1;"), "(ASSIGN_STMT (INDEX_EXPR x [ i ]) = 1 ;)");
    for op in ["+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "&&=", "||=", "^^="] {
        clean(&format!("inline void f() {{ x {op} 1; }}"));
    }
    // There is no `<<=` or `>>=` (§9 trap 10).
    rejected("inline void f() { x <<= 1; }");
    rejected("inline void f() { x >>= 1; }");
}

#[test]
fn increment_and_decrement() {
    assert_eq!(stmt("x++;"), "(ASSIGN_STMT x + + ;)");
    assert_eq!(stmt("++x;"), "(ASSIGN_STMT + + x ;)");
    assert_eq!(stmt("x--;"), "(ASSIGN_STMT x - - ;)");
    assert_eq!(stmt("--x;"), "(ASSIGN_STMT - - x ;)");
}

#[test]
fn conditionals() {
    assert_eq!(
        stmt("if (c) f(); else g();"),
        "(IF_STMT if ( c ) (EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;) \
         (ELSE_BRANCH else (EXPR_STMT (CALL_EXPR g (ARG_LIST ( ))) ;)))"
    );
    clean("inline void f() { if (a) { } else if (b) { } else { } }");
}

#[test]
fn static_if_in_every_position() {
    // Item position.
    clean("static if (X) { using a = uint8; } else { using a = uint16; }");
    // Statement position, braced and unbraced.
    clean("inline void f() { static if (X) g(); else h(); }");
    clean("inline void f() { static if (X) { g(); } }");
    // Switch-block position (`Parser.hs:604`).
    clean("inline void f() { switch (x) { static if (X) { case 1: break; } } }");
}

#[test]
fn loops() {
    assert_eq!(
        stmt("for (const uint8 i : 4) f();"),
        "(FOR_STMT for ( (VAR_DECL (CONST_TYPE const uint8) i) : 4 ) \
         (EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;))"
    );
    // `do … while (…)` needs no trailing `;` (§3.9).
    assert_eq!(stmt("do { } while (c)"), "(DO_WHILE_STMT do (BLOCK_STMT { }) while ( c ))");
    clean("inline void f() { do { } while (c); }");
    clean("inline void f() { static for (const uint8 i : 4) { } }");
    clean("inline void f() { [[unordered, reorder_by_looping]] for (const uint8 i : 4) { } }");
    clean("inline void f() { [[fifo_depth(4)]] do { } while (c) }");
    // The induction variable is always `const` (`Parser.hs:451`).
    rejected("inline void f() { for (uint8 i : 4) { } }");
    // There is no bare `while` loop.
    rejected("inline void f() { while (c) { } }");
}

#[test]
fn switches() {
    assert_eq!(
        stmt("switch (x) { case 1: f(); break; default: break; }"),
        "(SWITCH_STMT switch ( x ) { \
         (SWITCH_CASE case 1 : (EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;) (BREAK_STMT break ;)) \
         (SWITCH_DEFAULT default : (BREAK_STMT break ;)) })"
    );
    // Every case must end with `break;` (§9 trap 9).
    rejected("inline void f() { switch (x) { case 1: g(); } }");
    // `break` exists nowhere else.
    rejected("inline void f() { break; }");
    // There is no `continue` *statement* — but `continue` is not reserved
    // either, so `continue;` is a perfectly good expression statement naming an
    // undefined symbol, and the complaint comes from name resolution.
    assert_eq!(stmt("continue;"), "(EXPR_STMT continue ;)");
}

#[test]
fn annotated_statements() {
    assert_eq!(
        stmt("[[schedule(1)]] f();"),
        "(ANNOTATED_STMT (ATTR_LIST [ [ (ATTR schedule (ATTR_ARGS ( 1 ))) ] ]) \
         (EXPR_STMT (CALL_EXPR f (ARG_LIST ( ))) ;))"
    );
    // A memory-attributed array in statement position is a *declaration*, not
    // an annotated statement.
    assert_eq!(
        stmt("[[memory]] uint32[8] m;"),
        "(VAR_DECL (ARRAY_TYPE (ATTR_LIST [ [ (ATTR memory) ] ]) uint32 [ 8 ]) m ;)"
    );
}

#[test]
fn adjacent_attribute_groups_are_one_list() {
    // `[[a]] [[b]]` == `[[a, b]]` (§9 trap 5): one `ATTR_LIST`, two `ATTR`s.
    // Only the punctuation differs, so the comparison is on node shape.
    assert_eq!(
        tree_shape("class C { [[pure]] [[latency(1)]] uint32 inc(uint32 x); }"),
        tree_shape("class C { [[pure, latency(1)]] uint32 inc(uint32 x); }")
    );
}

#[test]
fn unknown_attributes_are_not_reported_as_unknown() {
    // `optionalAttributes` yields nothing and the construct then fails at the
    // `[` (§9 trap 4) — so there *is* a diagnostic, but never one naming the
    // attribute.
    let (_, errors) = parse("inline void f() { [[assume_ordered]] do { } while (c) }");
    assert!(!errors.is_empty());
    assert!(
        !errors.iter().any(|e| e.contains("assume_ordered")),
        "the attribute must not be named: {errors:#?}"
    );
}

#[test]
fn every_attribute_position() {
    // §4.1, one program per row of the table.
    clean("class C { [[async, no_backpressure, pipelined, unordered]] void f() { } }");
    clean("class C { [[latency(1), max_threads(2), thread_rate(3)]] void f() { } }");
    clean("class C { [[reset, pure]] void f() { } }");
    clean("void f([[last]] uint8 a) { }");
    clean("using T = ([[last]] uint8) -> void;");
    clean("using T = [[async, no_backpressure, pipelined, unordered]] (uint8) -> void;");
    clean("using T = [[latency(1), max_threads(2), thread_rate(3)]] (uint8) -> void;");
    clean("class C { [[initialize, memory, non_replicated, quad_port]] uint8[4] m; }");
    clean("class C { [[ecc(scheme)]] uint8[4] m; }");
    clean("inline void f() { [[schedule(1)]] g(); }");
    clean("inline void f() { [[unordered, reorder_by_looping, fifo_depth(2)]] for (const uint8 i : 4) { } }");
    clean("inline void f() { [[call_rate(1), fifo_depth(2), transaction_size(4)]] g(); }");
    clean("[[name(\"x\")]] export Foo;");
    clean("[[name(\"x\")]] extern Foo;");
    clean("class C { [[name(\"x\")]] uint8 v; }");
    // `[[atomic]]` is synthesized, never written (§4.1 last row).
    rejected("inline void f() { [[atomic]] g(); }");
}

// ---------------------------------------------------------------- recovery

/// Every one of these must terminate and yield a lossless tree; `parse`
/// asserts both.
#[test]
fn recovery() {
    for src in [
        "",
        "uint8 x",
        "uint8 x = 1",
        "class C { void f() { ",
        ")",
        "} } )",
        "@#$%^",
        "class",
        "template<",
        "using = ;",
        "enum",
        "struct S { uint8",
        "inline void f() { if (",
        "inline void f() { switch (x) { case",
        "Foo<",
        "a < b",
        "inline void f() { x = ; }",
        "\"unterminated",
        "/* unterminated",
        "[[",
        "module",
        "import",
    ] {
        let (_, _errors) = parse(src);
    }
}

/// Random token soup, to prove the two properties recovery rests on:
/// [`Parser`]'s step limit never fires (no rule loops without consuming) and no
/// marker is ever dropped.
#[test]
fn token_soup_always_terminates() {
    const ALPHABET: &[&str] = &[
        "class", "struct", "template", "static", "if", "for", "do", "while", "switch", "case",
        "break", "return", "using", "enum", "export", "extern", "inline", "typename", "auto",
        "uint8", "int<", ">", "<", "(", ")", "{", "}", "[", "]", ";", ",", ":", "::", ".", "=",
        "==", "+", "-", "*", "/", "&&", "||", "?", "~", "!", "->", "\\", "#", "a", "1", "\"s\"",
        "[[", "]]", "bitsizeof", "cast", "mux", "concat", "fan_out", "lutmul", "default",
        "module", "import", "as", "barrier", "reorder", "atomic",
    ];
    // The same cheap LCG the lexer's fuzz test uses, so a failure reproduces.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    let mut input = String::new();
    for _ in 0..4_000 {
        input.clear();
        for _ in 0..(next() % 40) {
            input.push_str(ALPHABET[next() % ALPHABET.len()]);
            input.push(' ');
        }
        parse(&input);
    }
}

/// Which node kinds the grammar can actually produce, measured over the whole
/// corpus plus this file's own snippets.
///
/// The point is the *complement*: every kind listed as unreachable is a
/// deliberate, explained gap, and a new one appearing here means a production
/// was dropped.
#[test]
fn node_kind_coverage() {
    use std::collections::BTreeSet;

    let mut seen: BTreeSet<SyntaxKind> = BTreeSet::new();
    let mut record = |src: &str| {
        let lexed = LexedStr::new(src);
        let mut input = Input::from_lexed(&lexed);
        input.set_features(Features::all());
        let events = parse_input(&input);
        for step in to_steps(&lexed, events) {
            if let StepOwned::Enter(kind) = step {
                seen.insert(kind);
            }
        }
    };

    // Constructs the corpus happens not to contain, so the measurement is of
    // the grammar rather than of the corpus.
    for src in [
        "module a.b { x, module c \\ d }\nimport e.f as g\n",
        "class C { default = 1; union U { bool b } }",
        "inline void f() { barrier; reorder atomic { } [[schedule(1)]] g(); }",
        "inline void f() { switch (x) { case 1: break; default: break; } }",
        "inline void f() { do { } while (c) for (const uint8 i : 4) { } }",
        "inline void f() { auto g = [a](uint8 x) -> uint8 { return x; }; }",
        "inline void f() { x %= mux(a, b, c) + lutmul(d, e); }",
        "inline void f() { g(concat(fan_out<2>(a), static(b), cast<uint8>(c))); }",
        "inline void f() { uint8 v = {.x = bitoffsetof(S, y) + byteoffsetof(S, z)}; }",
        "template<typename T, template<typename> typename TT, uint32 N = 1> class C { }",
        "using A = [[async]] ([[last]] const uint8 a) -> decltype(x);",
        "using B = [[memory, ecc(s)]] typename T::u[4];",
        "static if (X) { } else { }",
        "static assert(a ? b : c);",
        "[[name(\"n\")]] export Foo<uint8>;",
        "extern Bar;",
        "class C { public: private: uint8 p(uint8 a); }",
        "inline void f() { static for (const uint8 i : 4) if (a) { } else { } }",
        "enum E : uint2 { A = 1 }",
        "inline void f() { g(1, y: 2); }",
        "uint8 g;;",
        ")",
    ] {
        record(src);
    }
    for path in &corpus() {
        if let Ok(text) = std::fs::read_to_string(path) {
            record(&text);
        }
    }

    let unreachable: Vec<SyntaxKind> = (0..SyntaxKind::__LAST as u16)
        .filter_map(|raw| SyntaxKind::try_from(raw).ok())
        .filter(|k| k.is_node() && !seen.contains(k))
        .collect();

    // Every one of these is explained:
    //
    // * `BASE_LIST` — Kanagawa classes have no base-class list
    //   (`class = "class" identifier "{" … "}"`, `Parser.hs:331-339`).
    // * `WHILE_STMT` — there is no bare `while` loop, only `do … while`
    //   (`Parser.hs:580-589`).
    // * `UNROLLED_FOR_STMT` — `unrolled_for` is reserved but never parsed;
    //   `UnrolledForF` is built only by `Constexpr.hs:54` (grammar.md §1.2).
    // * `DECL_STMT` — a local variable is a `VAR_DECL` like any other; the
    //   position is recoverable from the parent and nothing needs the split.
    // * `ERROR` here means the vocabulary is missing `DEFAULT_INIT`; it *is*
    //   produced, so it does not appear in this list.
    let names: Vec<String> = unreachable.iter().map(|k| format!("{k:?}")).collect();
    assert_eq!(
        names,
        vec![
            "BASE_LIST",
            "DECL_STMT",
            "UNROLLED_FOR_STMT",
            "WHILE_STMT",
        ],
        "the set of unreachable node kinds changed"
    );
}

#[test]
fn a_missing_semicolon_still_yields_the_rest_of_the_file() {
    let (root, errors) = parse("uint8 a = 1\nuint8 b = 2;\n");
    assert!(!errors.is_empty());
    let rendered = sexpr(&root);
    assert!(rendered.contains("VAR_DECL"), "{rendered}");
    assert_eq!(rendered.matches("VAR_DECL").count(), 2, "{rendered}");
}

// ------------------------------------------------------------------ corpus

/// `test/syntax/imports/binary.k` is four bytes of `0xC0DEDEAD`, a fixture for
/// "the compiler was handed a non-source file"; it is not UTF-8 and drops out
/// exactly as it does in the lexer's corpus test.
const NOT_UTF8: &str = "binary.k";

fn corpus_root() -> PathBuf {
    kg_base::reference_root()
}

fn relative(path: &Path) -> String {
    path.strip_prefix(corpus_root())
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn collect_k_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_k_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "k") {
            out.push(path);
        }
    }
}

/// Every `.k` file the reference ships.
///
/// This **panics** when the checkout is missing rather than returning nothing.
/// A gate that silently reports success on zero files is not a gate; the
/// reference's own harness (`test/syntax/syntax-test.py:81-96`) fails loudly
/// when its inputs are absent, and so does this one. If the corpus lives
/// somewhere else, point `KANAGAWA_ROOT` at it.
fn corpus() -> Vec<PathBuf> {
    let root = corpus_root();
    assert!(
        root.exists(),
        "the reference corpus is not at {}. Set KANAGAWA_ROOT to the kanagawa checkout; \
         these tests are the parser's conformance gate and do not run without it.",
        root.display()
    );
    let mut files = Vec::new();
    // The same four roots the lexer's corpus test walks, so the two agree on
    // the file count. `doc/` holds no `.k` files today and `tools/` holds the
    // documentation-generator fixtures; both are walked so new ones are picked
    // up automatically.
    for dir in ["library", "test", "doc", "tools"] {
        collect_k_files(&root.join(dir), &mut files);
    }
    // The same tripwire the lexer's corpus test carries: a corpus that has
    // quietly shrunk — a half-cloned checkout, a moved directory — would
    // otherwise pass every assertion below while checking almost nothing.
    assert_eq!(
        files.len(),
        482,
        "the reference corpus changed size ({} files under {})",
        files.len(),
        root.display()
    );
    files
}

/// `test/syntax/*.k` — and only its *direct* children — are snippet
/// accumulators, not translation units: the harness feeds the compiler one
/// snippet at a time (`test/syntax/syntax-test.py`, conformance.md §1).
fn is_snippet_file(name: &str) -> bool {
    name.strip_prefix("test/syntax/").is_some_and(|rest| !rest.contains('/'))
}

/// Files that are whole translation units and must therefore parse with zero
/// errors.
///
/// `test/syntax/imports/` is excluded on purpose: those are fixtures for the
/// *import* tests and several are deliberately malformed. `a1/a21.k` is
/// `module a1.a21 {module}`, whose exposed list is a bare `module` keyword with
/// no module name — `import a1.a21` is marked `expected:1`
/// (`test/syntax/imports.k:668`), so rejecting it is the correct behaviour.
/// `test/syntax/helper/` *is* included: those are ordinary modules the fixtures
/// import, and they must all parse.
fn is_whole_program(name: &str) -> bool {
    // `tools/chkdoc/contexts/` is excluded: those two files are *templates*
    // for the doc checker, where `***` marks the substitution point
    // (`tools/chkdoc/chkdoc.cpp`), so they are not Kanagawa at all.
    if name.starts_with("tools/chkdoc/") {
        return false;
    }
    const CLEAN: [&str; 7] = [
        "library/",
        "doc/",
        "tools/",
        "test/logic/",
        "test/library/",
        "test/interface/",
        "test/syntax/helper/",
    ];
    CLEAN.iter().any(|prefix| name.starts_with(prefix))
}

/// The offset, line number and source line of the first error, so a corpus
/// failure localizes without a second run.
fn locate(text: &str, offset: usize) -> String {
    let offset = offset.min(text.len());
    let start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let line = text[..offset].bytes().filter(|&b| b == b'\n').count() + 1;
    format!("line {line}, offset {offset}: {}", text[start..end].trim())
}

fn parse_errors(src: &str) -> (Vec<(usize, String)>, bool) {
    errors_of(src)
}

fn errors_of(src: &str) -> (Vec<(usize, String)>, bool) {
    let lexed = LexedStr::new(src);
    let events = parse_source_file(&lexed);
    let steps = to_steps(&lexed, events);
    let mut errors = Vec::new();
    let mut round_trip = String::new();
    for step in steps {
        match step {
            StepOwned::Token(_, t) => round_trip.push_str(&t),
            StepOwned::Error(msg, offset) => errors.push((usize::from(offset), msg)),
            _ => {}
        }
    }
    (errors, round_trip == src)
}

/// Splits a `test/syntax` fixture the way `syntax-test.py:81-96` does: every
/// line is `strip()`ped and appended, a line *containing* `expected:` or
/// `warning:` ends the snippet and is itself discarded, and the file is then
/// reset to the (import-only) header — so snippets share no text.
fn split_snippets(text: &str) -> Vec<(String, Option<u32>)> {
    let mut out = Vec::new();
    let mut current = String::new();
    for raw in text.split('\n') {
        let line = raw.trim();
        if let Some(rest) = line.split_once("expected:") {
            let digits: String = rest.1.chars().take_while(char::is_ascii_digit).collect();
            let code = digits.parse::<u32>().unwrap_or(0);
            out.push((std::mem::take(&mut current), Some(code)));
        } else if line.contains("warning:") {
            // A warning marker also compiles the snippet and expects success.
            out.push((std::mem::take(&mut current), Some(0)));
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    out
}

#[test]
fn corpus_parses_cleanly() {
    let files = corpus();

    let mut failures: Vec<(usize, String)> = Vec::new();
    let mut whole_programs = 0usize;
    let mut snippet_files = 0usize;
    let mut snippets = 0usize;
    let mut skipped = 0usize;

    for path in &files {
        let name = relative(path);
        let Ok(bytes) = std::fs::read(path) else {
            failures.push((0, format!("{name}: unreadable")));
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            assert!(name.ends_with(NOT_UTF8), "unexpected non-UTF-8 file {name}");
            skipped += 1;
            continue;
        };

        let (errors, lossless) = parse_errors(text);
        if !lossless {
            failures.push((usize::MAX, format!("{name}: NOT LOSSLESS")));
        }

        if is_snippet_file(&name) {
            snippet_files += 1;
            for (n, (snippet, expected)) in split_snippets(text).into_iter().enumerate() {
                if expected != Some(0) || snippet.trim().is_empty() {
                    continue;
                }
                snippets += 1;
                let (errors, lossless) = parse_errors(&snippet);
                if !lossless {
                    failures.push((usize::MAX, format!("{name} snippet {n}: NOT LOSSLESS")));
                }
                if let Some((offset, msg)) = errors.first() {
                    failures.push((
                        errors.len(),
                        format!(
                            "{name} snippet {n} ({} error(s)): {msg} @ {}",
                            errors.len(),
                            locate(&snippet, *offset)
                        ),
                    ));
                }
            }
        } else if is_whole_program(&name) {
            whole_programs += 1;
            if let Some((offset, msg)) = errors.first() {
                failures.push((
                    errors.len(),
                    format!(
                        "{name} ({} error(s)): {msg} @ {}",
                        errors.len(),
                        locate(text, *offset)
                    ),
                ));
            }
        }
    }

    assert_eq!(skipped, 1, "expected exactly the {NOT_UTF8} fixture to be skipped");
    println!(
        "parsed {whole_programs} whole programs, {snippets} clean snippets from \
         {snippet_files} fixture files ({} files found)",
        files.len()
    );
    // What was actually checked, so a classification that stops matching any
    // file fails here instead of passing vacuously.
    assert_eq!(whole_programs, 355, "the whole-program count changed");
    assert_eq!(snippet_files, 73, "the syntax-fixture count changed");
    assert_eq!(snippets, 1847, "the clean-snippet count changed");

    if !failures.is_empty() {
        // Worst first, so the biggest breakage is the first thing read.
        failures.sort_by(|a, b| b.0.cmp(&a.0));
        let shown: Vec<&String> = failures.iter().take(40).map(|(_, s)| s).collect();
        panic!(
            "{} corpus failure(s), worst {}:\n{}",
            failures.len(),
            shown.len(),
            shown.iter().map(|s| format!("  {s}")).collect::<Vec<_>>().join("\n")
        );
    }
}

/// The `ERROR` node is the recovery vehicle, so a whole program producing one
/// is normally a bug. Exactly one construct breaks that rule here: `default =
/// expr;`, which is valid Kanagawa with no kind in the frozen vocabulary. This
/// test is what keeps that exception from quietly growing.
#[test]
fn the_only_error_nodes_in_whole_programs_are_default_initializers() {
    let files = corpus();
    let mut unexplained: Vec<String> = Vec::new();
    let mut default_inits = 0usize;

    for path in &files {
        let name = relative(path);
        if !is_whole_program(&name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let lexed = LexedStr::new(&text);
        let events = parse_source_file(&lexed);
        let steps = to_steps(&lexed, events);
        for (i, step) in steps.iter().enumerate() {
            if !matches!(step, StepOwned::Enter(ERROR)) {
                continue;
            }
            // The first non-trivia token inside identifies the construct.
            let first = steps[i + 1..].iter().find_map(|s| match s {
                StepOwned::Token(k, _) if !k.is_trivia() => Some(*k),
                _ => None,
            });
            match first {
                Some(DEFAULT_KW | STATIC_KW) => default_inits += 1,
                other => unexplained.push(format!("{name}: ERROR node starting with {other:?}")),
            }
        }
    }
    assert!(unexplained.is_empty(), "{unexplained:#?}");
    assert_eq!(default_inits, 7, "the corpus\'s `default = …;` count changed");
}


/// `kg_ty::TypeKey` identifies a node by `(file, range, kind)` and its
/// documentation claims no two distinct nodes of one kind share a full range.
/// That was true of every clean parse and false of eight error-recovered
/// corpus files, where `type_()`'s fallthrough completed an `ERROR` around
/// `err_recover`'s own `ERROR`: two nodes, one key, and the fixpoint's
/// maintained resolved-count disagreed with a walk of the tree by exactly the
/// number of such pairs (`cargo xtask bench` panicked on `test/syntax/loops.k`
/// and seven others in a debug build). The claim is now pinned over the
/// *whole* corpus, snippet accumulators included, because that is where
/// recovery runs.
#[test]
fn no_two_nodes_of_one_kind_share_a_range() {
    let files = corpus();
    let mut collisions: Vec<String> = Vec::new();

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let lexed = LexedStr::new(&text);
        let events = parse_source_file(&lexed);
        let steps = to_steps(&lexed, events);

        let mut offset = 0usize;
        let mut open: Vec<(SyntaxKind, usize)> = Vec::new();
        let mut seen: std::collections::HashSet<(SyntaxKind, usize, usize)> =
            std::collections::HashSet::new();
        for step in &steps {
            match step {
                StepOwned::Enter(kind) => open.push((*kind, offset)),
                StepOwned::Token(_, t) => offset += t.len(),
                StepOwned::Exit => {
                    let (kind, start) = open.pop().expect("balanced steps");
                    if !seen.insert((kind, start, offset)) {
                        collisions.push(format!(
                            "{}: two {kind:?} nodes at {start}..{offset}",
                            relative(path)
                        ));
                    }
                }
                StepOwned::Error(..) => {}
            }
        }
    }
    assert!(collisions.is_empty(), "{collisions:#?}");
}

// --------------------------------------------- statement / declaration slots

/// `statement` (`Parser.hs:481-493`) has no declaration alternative. Every
/// slot that takes a `statement` rather than a `statementOrDecl` therefore
/// rejects one, and `test/syntax/statement.k` is twelve consecutive
/// `expected:1` snippets saying exactly that.
#[test]
fn statement_only_positions_reject_declarations() {
    for src in [
        // statement.k:4, :11, :19, :29, :42, :50, :58, :66, :77, :84
        "inline void main() { reorder bool x; }",
        "inline void main() { if (true) bool x; }",
        "inline void main() { if (true) {} else bool x; }",
        "inline void main() { if (true) inline void Foo() {} }",
        "struct Foo{}; inline void main() { if (true) export Foo; }",
        "inline void main() { if (true) using foo = uint32; }",
        "inline void main() { if (true) static assert(true); }",
        "inline void main() { if (true) template<typename T> inline void Foo(){} }",
        "inline void main() { if (true) static uint32 x; }",
        "inline void main() { for (const auto i : 10) bool x; }",
        // `atomic` (`:559`), `annotatedStmt` (`:567`) and `doWhile` (`:586`)
        // take a `statement` too.
        "void f() { atomic bool x; }",
        "void f() { [[schedule(1)]] uint32 x; }",
        "void f() { do uint32 x; while (c) }",
        "void f() { if (c) static default = 1; }",
        "void f() { if (c) static assert(true); }",
    ] {
        rejected(src);
    }

    // The four `statementOrDecl` positions still take one: a body, a braced
    // scope, a `switch` case (`:602-603`) and a `static if` branch (`:493`).
    clean("void f() { uint32 x; }");
    clean("void f() { { uint32 x; } }");
    clean("void f() { if (c) { uint32 x; } }");
    clean("inline void f() { switch (x) { case 1: bool y; y = true; break; } }");
    clean("void f() { static if (X) uint32 x; }");
    clean("void f() { static for (const uint32 i : 4) { uint32 x; } }");
    // `static if` and `static for` are themselves `statement`s (`controlFlow`,
    // `Parser.hs:578-583`), so an unbraced branch may hold one — and a
    // `static if`'s own branches are `statementOrDecl` (`:582`) wherever it
    // sits.
    clean("void f() { if (c) static for (const uint32 i : 4) { } }");
    clean("void f() { if (c) static if (X) uint32 x; }");
}

/// The same twelve snippets, read out of the reference's own fixture so the
/// test cannot drift from it — plus the three `expected:0` ones, which pin the
/// other side.
#[test]
fn reference_statement_fixture_agrees() {
    let path = corpus_root().join("test/syntax/statement.k");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut checked = 0usize;
    for (n, (snippet, expected)) in split_snippets(&text).into_iter().enumerate() {
        if snippet.trim().is_empty() {
            continue;
        }
        let (errors, lossless) = parse_errors(&snippet);
        assert!(lossless, "snippet {n} is not lossless");
        match expected {
            Some(0) => assert!(errors.is_empty(), "snippet {n} {snippet:?}: {errors:#?}"),
            Some(_) => assert!(!errors.is_empty(), "snippet {n} {snippet:?} should be rejected"),
            None => continue,
        }
        checked += 1;
    }
    assert_eq!(checked, 14, "statement.k changed shape: 12 rejected, 2 accepted");
}

/// `decl` (`Parser.hs:245-251`) and `memberDecl` (`:341-350`) contain neither
/// `staticVariable` (`:464`) nor `staticFor` (`:591`) nor
/// `defaultInitialization` behind a `static` (`:474`); those three are
/// `statementOrDecl` only. `static` reaches a declaration position through
/// `staticIf` and through `commonDecl`'s `staticAssert`, and nothing else.
#[test]
fn static_forms_are_body_only() {
    for src in [
        "static uint32 x = 1;",
        "class C { static uint32 x = 1; }",
        "static for (const uint32 i : r) { }",
        "class C { static for (const uint32 i : r) { } }",
        "static default = 3;",
        "class C { static default = 3; }",
        "static(x);",
        // `staticIf p` parses its branches with the *caller's* production
        // (`Parser.hs:593-595`), so these are declarations, never statements.
        "static if (X) { return 1; }",
        "class C { static if (X) { return 1; } }",
        "static if (X) { break; }",
    ] {
        rejected(src);
    }

    clean("inline void f() { static uint32 x = 1; }");
    clean("inline void f() { static for (const uint32 i : r) { } }");
    clean("inline void f() { static default = 3; }");
    clean("inline void f() { static(x); }");
    clean("static assert(true);");
    clean("class C { static assert(true); }");
    clean("static if (X) { using a = uint8; } else { using a = uint16; }");
    clean("class C { static if (X) { uint8 a; } }");
}

/// `decl <- if isNothing modi && kind == MemberFunction then optional semi
/// else return Nothing` (`Parser.hs:417-419`). Both halves of that condition
/// are syntactic: which declaration list this is, and whether an
/// `inline`/`noinline` was just read.
#[test]
fn prototypes_are_unmodified_members_only() {
    clean("class C { void f(); }");
    clean("class C { template<typename T> void f(); }");
    clean("class C { [[async]] void f(); }");
    clean("class C { uint32 f(uint32 a); }");

    for src in [
        "void f();",
        "class C { inline void f(); }",
        "class C { noinline void f(); }",
        "[[async]] void f();",
        "template<typename T> void f();",
        "void g() { void f(); }",
    ] {
        rejected(src);
    }
}

/// `incDecStmt` (`Parser.hs:607-614`) and `assignStmt` (`:616-638`) both take
/// `lvalue` (`:803-805`) — a qualified identifier plus an `access` chain of
/// `[e]` and `.name` — never an `expression`.
#[test]
fn assignment_targets_are_lvalues() {
    assert_eq!(
        stmt("a.b[0].c = 1;"),
        "(ASSIGN_STMT (FIELD_EXPR (INDEX_EXPR (FIELD_EXPR a . b) [ 0 ]) . c) = 1 ;)"
    );
    clean("void f() { x = 1; }");
    clean("void f() { x[i] += 1; }");
    clean("void f() { ++x; }");
    clean("void f() { x.y--; }");

    for src in [
        "void f() { 1 = 2; }",
        "void f() { g() = 1; }",
        "void f() { (a) = 1; }",
        "void f() { a + b = 1; }",
        "void f() { cast<uint8>(x) = 1; }",
        "void f() { g() += 1; }",
        "void f() { ++g(); }",
        "void f() { a + b++; }",
        // `++x` is `Assign x (x + 1)`; there is no operator table behind the
        // operand, so the `+ 1` has nowhere to go.
        "void f() { ++x + 1; }",
        // `y = x++` is not an expression either.
        "void f() { y = x++; }",
    ] {
        rejected(src);
    }

    // The prefix form takes the lvalue and stops: no `BIN_EXPR` swallowed.
    let (root, _) = parse("void f() { ++x + 1; }");
    assert!(!sexpr(&root).contains("BIN_EXPR"), "{}", sexpr(&root));
}

// ------------------------------------------------------------- robustness

/// Nesting depth is native stack depth in a recursive-descent parser, and a
/// stack overflow is `SIGABRT` — not something the language server hosting
/// this can catch. Every one of these used to abort the process; each is run
/// on a 512 KiB stack, a quarter of what a `kg-lsp` worker gets.
#[test]
fn deep_nesting_does_not_overflow_the_stack() {
    let cases: Vec<String> = vec![
        format!("{}int32{} x;", "decltype(".repeat(20_000), ")".repeat(20_000)),
        format!(
            "void f() {{ auto g = {}1{}; }}",
            "[](){ return ".repeat(20_000),
            "; }".repeat(20_000)
        ),
        format!("void f() {{ {}{} }}", "{".repeat(30_000), "}".repeat(30_000)),
        format!("void f() {{ {} ; }}", "if(a) ".repeat(30_000)),
        format!("uint32 x = {}1{};", "(".repeat(30_000), ")".repeat(30_000)),
        format!("using T = {}uint32{};", "Foo<".repeat(20_000), ">".repeat(20_000)),
        format!("void f() {{ {} ; }}", "reorder ".repeat(30_000)),
        format!("void f() {{ x = {}1; }}", "-".repeat(20_000)),
        "(".repeat(30_000),
        "<".repeat(30_000),
        "[".repeat(30_000),
    ];
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            for (n, src) in cases.iter().enumerate() {
                let (_, lossless) = parse_errors(src);
                assert!(lossless, "case {n} is not lossless");
            }
        })
        .expect("spawn")
        .join()
        .expect("a deeply nested input must not abort the process");
}

/// Parses without building the `Node` tree, which the deliberately enormous
/// inputs below would make the *test harness* recurse over.
#[track_caller]
fn clean_big(src: &str) {
    let (errors, lossless) = parse_errors(src);
    assert!(lossless, "the parse is not lossless");
    assert!(
        errors.is_empty(),
        "{} unexpected error(s), first: {:?}",
        errors.len(),
        errors.first()
    );
}

/// How many nodes of `kind` the parse produced.
fn count_nodes(src: &str, kind: SyntaxKind) -> usize {
    let lexed = LexedStr::new(src);
    let events = parse_source_file(&lexed);
    to_steps(&lexed, events)
        .iter()
        .filter(|s| matches!(s, StepOwned::Enter(k) if *k == kind))
        .count()
}

/// How much headroom the recursion budget leaves real code.
///
/// The budget is only defensible if no legitimate file comes near it, so this
/// measures the deepest point every whole program in the reference corpus
/// reaches and insists on an order of magnitude of slack.
#[test]
fn the_recursion_budget_has_headroom() {
    let mut worst = (0u32, String::new());
    for path in &corpus() {
        let name = relative(path);
        if !is_whole_program(&name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let lexed = LexedStr::new(&text);
        let _ = parse_source_file(&lexed);
        let peak = super::depth::peak();
        if peak > worst.0 {
            worst = (peak, name);
        }
    }
    println!("deepest corpus file: {} at depth {}", worst.1, worst.0);
    assert!(
        worst.0 * 6 <= super::depth::LIMIT,
        "{} reaches depth {} of a {} budget — too little slack",
        worst.1,
        worst.0,
        super::depth::LIMIT
    );
}

/// Every speculative scan in this parser used to be re-run from scratch at
/// each nesting level, or at each element of a run, which turned four
/// perfectly ordinary constructs quadratic and tripped the parser's global
/// step budget on valid input. These are the four, each an order of magnitude
/// past where it used to die, and each parsing with **no** diagnostics.
#[test]
fn long_valid_inputs_stay_linear() {
    // `at_template_arg_list` on every `<`: 3,384 terms used to panic.
    let terms: Vec<String> = (0..20_000).map(|i| format!("a{i} < b{i}")).collect();
    clean_big(&format!("void f() {{ bool r = {}; }}", terms.join(" && ")));

    // `at_attrs` over a whole run after every group: 1,725 used to panic.
    clean_big(&format!("void f() {{ {} g(); }}", "[[fifo_depth(1)]]".repeat(20_000)));
    clean_big(&format!("void f() {{ {} g(); }}", "[[schedule(1)]]".repeat(20_000)));

    // `at_fn_type` at every nesting level: 4,651 used to panic. This one is
    // still *rejected* — the recursion budget stops it — but it is rejected in
    // linear time instead of aborting the process.
    let (errors, lossless) =
        parse_errors(&format!("uint32 x = {}1{};", "(".repeat(20_000), ")".repeat(20_000)));
    assert!(lossless);
    assert!(!errors.is_empty());
}

/// A construct longer than the old 4,096-token scan window was not merely
/// slow, it was *unrecognized*: the parse collapsed into thousands of
/// cascading errors on input the reference accepts. The scans are bounded by
/// end of input now, and by the tokens that prove the construct is not there.
#[test]
fn long_constructs_are_recognized() {
    let n = 8_192;
    let ones = vec!["1"; n].join(",");

    let src = format!("Foo<{ones}> x;");
    clean_big(&src);
    assert_eq!(count_nodes(&src, TEMPLATE_ARG_LIST), 1, "still one argument list");
    assert_eq!(count_nodes(&src, LITERAL), n, "with every argument in it");
    assert_eq!(count_nodes(&src, VAR_DECL), 1);

    // The same window bounded the declaration lookahead, the function-type
    // scan and the attribute-argument scan.
    clean_big(&format!("void f() {{ A<{ones}> x; }}"));
    let fn_type = format!("using T = ({}) -> void;", vec!["uint32"; n].join(","));
    clean_big(&fn_type);
    assert_eq!(count_nodes(&fn_type, FN_TYPE), 1);
    clean_big(&format!("[[latency({})]] void f(){{}}", vec!["1"; n].join("+")));
}

/// An attributed array takes exactly one dimension: `memoryAttr type3 "["
/// e "]"` (`Parser.hs:529`), and the reference rejects `[[memory]]
/// uint32[16][10]` at the second `[` — checked against `kg-oracle
/// --errors`, same column.
#[test]
fn an_attributed_array_takes_one_dimension() {
    let (errors, lossless) = parse_errors("using C = [[memory]] uint32[16][10];\n");
    assert!(lossless, "the parse is lossless either way");
    assert_eq!(errors.first().map(|(at, _)| *at), Some(31), "the second `[` is the error");
    assert!(parse_errors("using D = [[memory]] uint32[16];\n").0.is_empty());
}

/// Edition 1.1's trailing commas: one comma before the closing bracket of
/// every list the reference separates with `sepBy` or a `comma >>` recursion
/// (`Parser.hs:289-291,363,377,551,697,860,871`), each a parse error there.
/// Two commas, or a comma alone, are still errors. Red with any one list's
/// `Parser::at_trailing` left out.
#[test]
fn a_trailing_comma_closes_a_list_where_the_language_has_them() {
    let trailing = Features::NONE.with(Feature::TrailingCommas);
    let lists = [
        ("call arguments", "static assert(f(1, 2,));"),
        ("parameters", "inline void f(uint8 x, uint8 y,) {}"),
        ("template parameters", "template <typename T, auto N,> inline void f() {}"),
        ("template arguments", "using t = Foo<uint8, 4,>;"),
        ("function type parameters", "inline void f((uint8, uint8,) -> uint8 g) {}"),
        ("a positional initializer list", "static assert({1, 2,} == x);"),
        ("a designated initializer list", "static assert({.a = 1, .b = 2,} == x);"),
        ("enum constants", "enum E : uint2 { A, B, }"),
    ];
    for (what, src) in lists {
        assert_eq!(parse_in(src, trailing).1, Vec::<String>::new(), "{what} with a trailing comma");
        assert!(!parse(src).1.is_empty(), "the reference rejects {what} with a trailing comma");
    }
    assert!(!parse_in("static assert(f(1,,));", trailing).1.is_empty(), "two commas");
    assert!(!parse_in("static assert(f(,));", trailing).1.is_empty(), "a comma alone");
    let named = trailing.with(Feature::NamedArguments);
    assert_eq!(parse_in("static assert(f(1, y: 2,));", named).1, Vec::<String>::new(), "after a named argument");
}
