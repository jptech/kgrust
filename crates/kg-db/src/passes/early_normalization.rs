//! Execution-order pass 1 — `Frontend.hs:74-79`.

use kg_const::ConstValue;
use kg_syntax::{AstNode, SyntaxKind, ast};

use crate::sink::Sink;

/// The two diagnostic-producing algebras of pass 1.
///
/// The full pass is `desugar (validateTypedLiterals . interpolatedStringPre .
/// freeNonInlineFunctions . localMemberFunctions . redundantNameScope symbols)`
/// (`Frontend.hs:74-79`) — five algebras fused into one bottom-up `cata`, run
/// right-to-left per node. Three of the five are pure rewrites; only these two
/// produce an `ErrorF`, and both are decidable from the syntax tree alone.
pub(crate) fn pass_1_early_normalization(root: &ast::SourceFile, d: &mut Sink) {
    for node in root.syntax().descendants() {
        if let Some(f) = ast::FnDecl::cast(node.clone()) {
            free_non_inline_functions(&f, d);
        }
        if let Some(l) = ast::Literal::cast(node) {
            validate_typed_literals(&l, d);
        }
    }
}

/// The parser's rejection of a type as an attribute's argument, over the
/// file ([`attribute_argument_is_a_value`]).
pub(crate) fn pass_0_attribute_arguments(root: &ast::SourceFile, d: &mut Sink) {
    for a in root.syntax().descendants().filter_map(ast::Attr::cast) {
        attribute_argument_is_a_value(&a, d);
    }
}

/// `typeSpecifier'` (`Parser.hs:712-721`): a type position naming a template
/// with no `<…>` is the parser's `symbolError s "template arguments
/// missing"` — `static X x;` for a `template <auto N = uint32> struct X`
/// (`template-param-default.k::36,37`, `kg-oracle --errors` 2026-09-08,
/// a `PARSE-ERROR`). Before pass 1's checkpoint, so it outranks everything
/// the passes say. Judged only for a template *this file* declares: the
/// parser's table holds what is in scope, and a same-named template in a
/// module this one does not import is not — where a name could be either,
/// pass 9's "Type name not in scope" is the reference's answer and the
/// index cannot tell the two apart.
pub(crate) fn pass_0_template_arguments_missing(
    file: kg_base::FileId,
    root: &ast::SourceFile,
    templates: &super::templates::Templates,
    binders: &super::templates::Binders,
    interner: &kg_base::Interner,
    d: &mut Sink,
) {
    for node in root.syntax().descendants() {
        let Some(path_type) = ast::PathType::cast(node.clone()) else { continue };
        // Not every path written as a type is a `typeSpecifier`: a
        // template-template argument names a template on purpose
        // (`T1<S1>`, `templateArgsResolved`'s `tIsTemplate`), so does such a
        // parameter's default (`typename TT = S1`), and an `export`/`extern`
        // of a template is pass 13's own verdict (`Frontend.hs:340-341`).
        let excused = node.ancestors().skip(1).any(|a| {
            matches!(
                a.kind(),
                SyntaxKind::TEMPLATE_ARG_LIST
                    | SyntaxKind::TYPE_PARAM
                    | SyntaxKind::EXPORT_DECL
                    | SyntaxKind::EXTERN_DECL
            )
        });
        if excused {
            continue;
        }
        let Some(path) = path_type.path() else { continue };
        let mut segments = path.path_segments();
        let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
        if segment.template_arg_list().is_some() {
            continue;
        }
        let Some(ident) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
        if super::templates::shadowed_before_module_scope(binders, &node, ident.text()) {
            continue;
        }
        // A template is registered once its parameters are parsed: inside
        // its own list, `template <Foo N> class Foo`, the name is in no table
        // and the reference accepts (difffuzz [05]); inside its body it is,
        // and a bare `Foo x;` there is this error, as the oracle says.
        if node.ancestors().filter(|a| a.kind() == SyntaxKind::TEMPLATE_PARAM_LIST).any(|list| {
            list.parent().and_then(ast::TemplateDecl::cast).and_then(|t| t.decl()).is_some_and(|decl| {
                decl.syntax()
                    .children()
                    .find(|c| c.kind() == SyntaxKind::NAME)
                    .is_some_and(|n| n.text() == ident.text())
            })
        }) {
            continue;
        }
        // A template this file declares, or one an unaliased import makes
        // visible from the very file the index found it in — the parse-time
        // table's two halves. A same-named template in a module this one
        // does not import is not in that table, and pass 9's "Type name not
        // in scope" is the reference's answer there.
        let sym = interner.intern(ident.text());
        let Some((declared_in, _)) = templates.located(sym) else { continue };
        if declared_in != file && templates.visible_through_import(file, sym) != Some(declared_in) {
            continue;
        }
        d.error(
            path.syntax().text_range(),
            format!("`{}`: template arguments missing", ident.text()),
        );
    }
}

/// `[[max_threads(int32)]]`: the reference's *parser* rejects a type as an
/// attribute's argument — `kg-oracle --errors --passes 1` prints it as a
/// `PARSE-ERROR`, "Invalid use of a type as an expression" at the argument
/// (`type-check.k::17`, 2026-09-08) — so the frontend is never entered and
/// pass 1's non-inline error on the same line is never made
/// (`passes::run_through_8` runs pass 1 only past the parser's checks).
/// Only a written type node is judged, since no table exists this early.
fn attribute_argument_is_a_value(attr: &ast::Attr, d: &mut Sink) {
    let Some(arg) = attr.attr_args().and_then(|a| a.expr()) else { return };
    if let ast::Expr::Type(t) = &arg
        && !matches!(t, ast::Type::DecltypeType(_))
    {
        d.error(arg.syntax().text_range(), "Invalid use of a type as an expression");
    }
}

/// `freeNonInlineFunctions` (`Frontend.hs:227-231`):
///
/// ```haskell
/// freeNonInlineFunctions e@(FunctionF _ FreeFunction _ m _ _ _ _)
///     | (Just (FunctionModifier Inline)) <- m = e
///     | otherwise = desugarError "Free functions must be declared `inline`"
/// ```
///
/// `FreeFunction` is the `funcKind` the parser attaches at exactly two places:
/// a module-level `decl` (`Parser.hs:249-250`) and a function nested in another
/// function's body via `statementOrDecl` (`Parser.hs:473,478`). `MemberFunction`
/// comes only from `memberDecl` (`Parser.hs:348-349`), i.e. a class body — which
/// is why a class method needs no `inline` and a free one does.
///
/// `FunctionDeclF` — a body-less prototype, [`ast::FnProto`] here — is a
/// different constructor and is not matched, so it is not checked.
///
/// `localMemberFunctions` (`Frontend.hs:219-225`) retags every function nested
/// inside a *member* function as `MemberFunction` — but pass 1 is a `cata`, so
/// the nested function is visited **before** its enclosing method and is
/// still `FreeFunction` when `freeNonInlineFunctions` sees it: a non-inline
/// function in a method's body is rejected (`kg-oracle --no-base --errors`,
/// 2026-10-10). So the kind is the parser's alone: a member where the
/// nearest enclosing list is a class's body — a `static if` there is
/// `staticIf memberDecl`, its branches the body's — and free in a body or at
/// module scope, `static if` branches included. Until sprint W3.7 anything
/// under a class's body was skipped, which missed the nested one.
///
/// The error is at the function's note, its first token past any comment —
/// the return type, or an attribute (`[[async]] void g()` at the `[[`);
/// past a template's parameter list, which is the `Template` node's.
fn free_non_inline_functions(f: &ast::FnDecl, d: &mut Sink) {
    if is_member_function(f.syntax()) {
        return;
    }
    let inline = f.fn_modifier().is_some_and(|m| m.inline_kw_token().is_some());
    if !inline {
        let start = kg_syntax::trimmed_range(f.syntax()).start();
        let end = f.name().map_or_else(|| f.syntax().text_range().end(), |n| n.syntax().text_range().end());
        d.error(kg_base::TextRange::new(start, end.max(start)), "Free functions must be declared `inline`");
    }
}

/// Whether `block` is a branch of a `static if` — its own, or its `else`'s
/// (`ELSE_BRANCH` wraps that one).
fn static_if_branch(block: &kg_syntax::SyntaxNode) -> bool {
    let mut parent = block.parent();
    if parent.as_ref().is_some_and(|p| p.kind() == SyntaxKind::ELSE_BRANCH) {
        parent = parent.and_then(|p| p.parent());
    }
    parent.is_some_and(|p| p.kind() == SyntaxKind::STATIC_IF_STMT)
}

/// Whether the parser builds the function at `node` a `MemberFunction`
/// (`memberDecl`, `Parser.hs:341-350`): its nearest enclosing list, past a
/// template wrapping it and through any `static if` branch, is a class's
/// body — not a function's or a block's, and not the module's.
fn is_member_function(node: &kg_syntax::SyntaxNode) -> bool {
    for ancestor in node.ancestors().skip(1) {
        match ancestor.kind() {
            SyntaxKind::DECL_LIST => return true,
            SyntaxKind::BLOCK_STMT if static_if_branch(&ancestor) => {}
            SyntaxKind::BLOCK_STMT | SyntaxKind::SOURCE_FILE => return false,
            _ => {}
        }
    }
    false
}

/// `validateTypedLiterals` (`Frontend.hs:210-217`):
///
/// ```haskell
/// validateTypedLiterals (IntLiteralF (Just t) x)
///     | literalWidth t == 0 = desugarError $ "Invalid literal type specifier `" ++ … ++ "`"
///     | outOfRange t        = desugarError $ "Literal out of range for `" ++ … ++ "`"
///   where
///     outOfRange (LiteralType SignedLiteral w)   = x /= truncateInt w x
///     outOfRange (LiteralType UnsignedLiteral w) = x /= truncateUInt w x
/// ```
///
/// Only a literal that carries an `i<W>`/`u<W>` suffix is checked (`Just t`).
/// The value parsed out of the token is always non-negative — the reference has
/// no negative integer literals, `-1i8` being `UnaryF Neg (IntLiteral (Just i8) 1)`
/// (`Parser.hs:948`) — so `128i8` is out of range while `-128i8` is not.
fn validate_typed_literals(l: &ast::Literal, d: &mut Sink) {
    let Some(token) = l.int_number_token() else { return };
    // A token the value parser rejects is a lexical error the parser has already
    // reported; inventing a second diagnostic for it would be noise.
    let Some(ConstValue::Int { ty: Some(ty), value }) =
        ConstValue::parse_int_literal(token.text())
    else {
        return;
    };
    let range = token.text_range();
    if ty.width == 0 {
        d.error(range, format!("Invalid literal type specifier `{ty}`"));
    } else if value != ty.truncate(&value) {
        // `outOfRange` (`Frontend.hs:214-216`).
        d.error(range, format!("Literal out of range for `{ty}`"));
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::{accepts, rejects};

    /// `Parser.hs:712-721`, `kg-oracle --errors` 2026-09-08: a template used
    /// as a type without arguments is the parser's error at the name; with
    /// arguments, or with a template parameter shadowing the name, it is not.
    #[test]
    fn a_template_used_as_a_type_needs_its_arguments() {
        let x = "template <auto N = uint32>\nstruct X\n{\n    uint<N> x;\n}\n\n";
        let text = format!("{x}inline void main()\n{{\n    static X x;\n}}\n");
        let out = crate::check_source(&text);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].message, "`X`: template arguments missing");
        assert_eq!(&text[out[0].range.range], "X");
        crate::tests::accepts(&format!("{x}inline void main()\n{{\n    static X<3> x;\n}}\n"));
        crate::tests::accepts(&format!("{x}template <typename X>\ninline void Go()\n{{\n    static X x;\n}}\n"));
    }

    /// `test/syntax/local-functions.k` — a free function needs `inline`, a class
    /// method does not, and a function nested in a free function needs it too.
    /// The parser's table while a template is parsed: its own name enters
    /// after its parameter list, each parameter after itself
    /// (`SymbolTable.hs:194-207`). `kg-oracle --errors`, 2026-09-23: the
    /// first two compile (difffuzz [05], [10]); a bare `Foo` in the class
    /// body and an earlier value parameter used as a type are still
    /// rejected. Red with either guard removed.
    #[test]
    fn a_template_parameter_list_sees_only_what_was_parsed_before_it() {
        accepts("template <Foo N>\nclass Foo\n{}\n");
        accepts("template <Foo Foo>\nclass Foo\n{}\n");
        rejects("template <typename T>\nclass Foo\n{\n    Foo x;\n}\n");
        rejects("template <auto N, N M>\nclass Foo\n{}\n");
    }

    /// A function parameter shadows a `typename` parameter of the same name
    /// once registered (`findInScopes`, innermost first): `x.x` and `g(x)`
    /// over `uint32 x` read the value, as the full reference compiler does
    /// (difffuzz, 2026-09-23). Without the parameter, still the errors.
    /// Red with either shadowing guard removed.
    #[test]
    fn a_function_parameter_shadows_a_typename_parameter() {
        accepts("struct S\n{\n    uint32 x;\n}\n\ntemplate <typename x>\ninline uint32 F(S x)\n{\n    return x.x;\n}\n");
        rejects("struct S\n{\n    uint32 x;\n}\n\ntemplate <typename x>\ninline uint32 F(S y)\n{\n    return x.x;\n}\n");
        accepts("inline void g(uint32 a)\n{}\n\ntemplate <typename x>\ninline void F(uint32 x)\n{\n    g(x);\n}\n");
        rejects("inline void g(uint32 a)\n{}\n\ntemplate <typename x>\ninline void F(uint32 y)\n{\n    g(x);\n}\n");
    }

    #[test]
    fn free_functions_must_be_inline() {
        rejects("void main() {}");
        accepts("inline void main() {}");
        accepts("class C\n{\nvoid Run() {}\n}");
        rejects("inline uint32 Run(uint32 x)\n{\nuint32 Go(uint32 y) { return y; }\nreturn Go(x);\n}");
        accepts("inline uint32 Run(uint32 x)\n{\ninline uint32 Go(uint32 y) { return y; }\nreturn Go(x);\n}");
        // A prototype is `FunctionDeclF`, a different constructor
        // (`Frontend.hs:228` matches `FunctionF` only).
        accepts("class C\n{\nvoid Run();\n}");
    }

    /// The parser's kind, not `localMemberFunctions`' (which runs after the
    /// check, the `cata` visiting the nested function first): a function in
    /// a method's body is free, one in a class body's `static if` a member,
    /// one in a module-scope `static if` free — each at its first token,
    /// an attribute's or past a template's parameters (`kg-oracle --no-base
    /// --errors`, 2026-10-10: 2:1, 8:1, 18:9, 24:13 below; the method `Run`
    /// and the class body's `M` and `N`, the latter in an `else`, nothing —
    /// `library/data/buffer/accumulating.k`'s `[[reset]] void reset()` is
    /// one). Red with a class's body skipped whole, as it was, with the
    /// error at the name, and with an `else` branch read as a body.
    #[test]
    fn a_function_in_a_methods_body_is_free() {
        let text = "template <typename T>\nT f(T x)\n{\n    return x;\n}\n\n// a comment\n[[async]] void g()\n{\n}\n\nclass C\n{\npublic:\n    void Run()\n    {\n        template <typename T>\n        T h(T x)\n        {\n            return x;\n        }\n        static if (true)\n        {\n            void k() {}\n        }\n    }\n    static if (true)\n    {\n        void M() {}\n    }\n    else\n    {\n        void N() {}\n    }\n}\n";
        let out = crate::check_source(text);
        let index = kg_base::LineIndex::new(text);
        let at: Vec<(String, u32, u32)> = out
            .iter()
            .map(|d| {
                let pos = index.line_col(d.range.range.start());
                (d.message.clone(), pos.line + 1, pos.col + 1)
            })
            .collect();
        let free = "Free functions must be declared `inline`".to_owned();
        assert_eq!(at, [(free.clone(), 2, 1), (free.clone(), 8, 1), (free.clone(), 18, 9), (free, 24, 13)]);
    }

    /// `Frontend.hs:210-217`, pinned by `test/syntax/literals.k:118-172`.
    ///
    /// The clause tests the literal *token*, which is never negative
    /// (`Parser.hs:948` makes `-x` a `UnaryF Neg`), so `-128i8` is rejected even
    /// though the value it denotes fits: `truncateInt 8 128 == -128 /= 128`.
    /// `literals.k:166-169` (`0x80i8`, `expected:1`) is that exact case.
    #[test]
    fn typed_literals_must_fit_their_suffix() {
        accepts("const auto x = 0xFFu8;");
        rejects("const auto x = 0x100u8;");
        accepts("const auto x = 0x7Fi8;");
        rejects("const auto x = 0x80i8;");
        rejects("const uint32 x = 10u0;");
        rejects("const uint32 x = 10i0;");
        // No suffix: `IntLiteralF Nothing`, which the clause does not match.
        accepts("const auto x = 256;");
        // `-10i32` is in range; the negation is a separate node
        // (`literals.k:128-135`).
        accepts("const auto x = -10i32;");
    }
}

/// The parse-time table's kind checks on a template's own parameters —
/// rejections of the reference's *parser*, since `typeSpecifier` and `term`
/// consult the table mid-parse and a `typename` parameter, a value parameter
/// and a function-typed parameter are three different symbols. Probed
/// against `kg-oracle --errors`, 2026-09-10 (`scratchpad/p2/tp*.k`,
/// `test/syntax/template.k::10-22`):
///
/// * a value or function-typed parameter written as a type — `N x;`, `N
///   Bar()`, `const N x;`, `F x;`, and `F F1` or `(N, N) -> R F` inside the
///   parameter list itself;
/// * a `typename` parameter written as a value — `T.x`, `T()`, `T++`.
///
/// Where the error lands is the parser's business, and it differs by what
/// failed. Inside the *parameter list* and inside a *function* template's
/// body, at the offending token (`unexpected N`, `unexpected '.'`). Inside a
/// *class* template's body, at the `class` keyword: the class-template
/// alternative is under `try`, so a failure anywhere in its body backtracks
/// and the function-template alternative reports "unexpected class Fo" at
/// the keyword (`tp2.k`: even `uint32 y = 1 +;` in a class template's method
/// lands there). `T++` is the exception in both: `symbolError t "illegal use
/// of this type as an expression"` (`Parser.hs:787`) is registered and
/// parsing continues, so it sits on the `T`. Only the position is graded for
/// a parse-time rejection; the messages here say what was wrong.
///
/// A `T()` in a function template's body is left alone: the token the
/// reference stops at was not probed.
pub(crate) fn pass_0_template_parameter_kinds(root: &ast::SourceFile, d: &mut Sink) {
    for node in root.syntax().descendants() {
        let Some(template) = ast::TemplateDecl::cast(node.clone()) else { continue };
        let Some(list) = template.template_param_list() else { continue };
        let name_of = |n: Option<ast::Name>| n.and_then(|n| n.ident_token()).map(|t| t.text().to_string());
        let typenames: Vec<String> = list
            .type_params()
            .filter(|p| p.template_param_list().is_none())
            .filter_map(|p| name_of(p.name()))
            .collect();
        let (mut values, mut functions) = (Vec::new(), Vec::new());
        for p in list.value_params() {
            let Some(name) = name_of(p.name()) else { continue };
            if matches!(p.ty(), Some(ast::Type::FnType(_))) {
                functions.push(name);
            } else {
                values.push(name);
            }
        }
        let Some(decl) = template.decl() else { continue };
        let class_keyword = matches!(
            decl.syntax().kind(),
            SyntaxKind::CLASS_DECL | SyntaxKind::STRUCT_DECL | SyntaxKind::UNION_DECL
        )
        .then(|| decl.syntax().first_token())
        .flatten();

        for inner in node.descendants() {
            if inner.kind() != SyntaxKind::PATH {
                continue;
            }
            // A nested template's parameters are its own pass's business.
            if inner.ancestors().skip(1).take_while(|a| a != &node).any(|a| a.kind() == SyntaxKind::TEMPLATE_DECL) {
                continue;
            }
            let Some(path) = ast::Path::cast(inner.clone()) else { continue };
            let mut segments = path.path_segments();
            let (Some(segment), None) = (segments.next(), segments.next()) else { continue };
            if segment.template_arg_list().is_some() {
                continue;
            }
            let Some(token) = segment.name_ref().and_then(|n| n.ident_token()) else { continue };
            let name = token.text();
            let Some(parent) = inner.parent() else { continue };
            let in_param_list = inner.ancestors().any(|a| a.kind() == SyntaxKind::TEMPLATE_PARAM_LIST);
            // Each parameter is registered once parsed (`parseSymbol`,
            // `SymbolTable.hs:194-207`), so inside the list a mention sees
            // only the parameters before its own: `template <Foo Foo>` names
            // no parameter in `Foo`'s type, and the reference accepts it
            // (difffuzz [10]).
            if in_param_list
                && !list.syntax().children().any(|p| {
                    p.text_range().end() <= inner.text_range().start()
                        && p.children().any(|n| n.kind() == SyntaxKind::NAME && n.text() == name)
                })
            {
                continue;
            }
            // A function parameter or a local of the same name, registered by
            // the time the parser reaches the mention, shadows the template
            // parameter (`findInScopes`, innermost first): `x.x` over a
            // parameter `Bar<> x` of `template <typename x>` is a member
            // access (difffuzz, 2026-09-23). A declaration still being parsed
            // does not — `() -> T T` types its parameter with the template's
            // `T` (`parseSymbol`, `SymbolTable.hs:194-207`).
            if let Some(decl) = crate::facts::innermost_declaration(&inner, name)
                && node.text_range().contains_range(decl.text_range())
                && decl.text_range().end() <= inner.text_range().start()
            {
                continue;
            }
            // Where a hard parse failure is reported: the offending token in
            // the parameter list or a function template, the keyword of a
            // class template.
            let hard = |at: kg_base::TextRange| -> kg_base::TextRange {
                match &class_keyword {
                    Some(kw) if !in_param_list => kw.text_range(),
                    _ => at,
                }
            };

            if parent.kind() == SyntaxKind::PATH_TYPE {
                let kind = if values.iter().any(|v| v == name) {
                    "a non-type"
                } else if functions.iter().any(|f| f == name) {
                    "a function-typed"
                } else {
                    continue;
                };
                d.error(hard(token.text_range()), format!("`{name}`: {kind} template parameter cannot be used as a type"));
                continue;
            }
            if !typenames.iter().any(|t| t == name) {
                continue;
            }
            match parent.kind() {
                SyntaxKind::ASSIGN_STMT => {
                    d.error(token.text_range(), format!("`{name}`: illegal use of this type as an expression"));
                }
                SyntaxKind::FIELD_EXPR => {
                    let is_base = ast::FieldExpr::cast(parent.clone())
                        .and_then(|f| f.base())
                        .is_some_and(|b| b.syntax() == &inner);
                    let dot = inner.next_sibling_or_token().filter(|t| t.kind() == SyntaxKind::DOT);
                    if let (true, Some(dot)) = (is_base, dot) {
                        d.error(hard(dot.text_range()), format!("`{name}`: a type has no members to access"));
                    }
                }
                SyntaxKind::CALL_EXPR if class_keyword.is_some() && !in_param_list => {
                    let is_callee = ast::CallExpr::cast(parent.clone())
                        .and_then(|c| c.callee())
                        .is_some_and(|c| c.syntax() == &inner);
                    if is_callee {
                        d.error(hard(token.text_range()), format!("`{name}`: a type cannot be called"));
                    }
                }
                _ => {}
            }
        }
    }
}

/// An attribute on a variable declaration the grammar has no place for —
/// the reference's parser, having read `[[async]]` as `funcAttr`, expects a
/// function (`Parser.hs:395-410`), and having read `[[memory]]` as `type'`'s
/// first alternative, expects an array (`:529`). Probed 2026-09-10
/// (`scratchpad/p2/at*.k`, `test/syntax/basics.k::340-345`), each position
/// the parser's:
///
/// * a function attribute before the type: after the name (`;`, `=`);
/// * with `static` between attribute and type: at the type; with `static`
///   after the attribute: at `static`;
/// * a memory attribute on a type that is not an array: at the name, in a
///   function body and in a struct alike.
///
/// A function *type*'s own attributes (`funcTypeAttr`, `Parser.hs:381-389`)
/// before a written function type are that type's — `[[async]] (uint32) ->
/// void cb;` is a variable of an attributed function type, wherever it
/// stands (`test/interface/external_class.k:12`), and not looked at.
///
/// A class member keeps its `[[name("…")]]` (`classVariable`, `:462`); any
/// other attribute list fails that parse, and one of function attributes
/// alone is read by `function MemberFunction` next (`:341-348`), which
/// expects `(` after the name — `[[reset]] (bool) -> void callback;`
/// (`callbacks.k::81`), `[[latency(2)]] uint32 x = 1;` at the `=`
/// (probed 2026-09-24). A list mixing in anything else fails at its `[`,
/// with a message this port does not spell, and is left alone.
pub(crate) fn pass_0_attributes_on_a_variable(root: &ast::SourceFile, d: &mut Sink) {
    const FN_TYPE_ATTRS: [&str; 7] =
        ["async", "no_backpressure", "pipelined", "unordered", "latency", "max_threads", "thread_rate"];
    const FUNCTION_ATTRS: [&str; 9] = [
        "async", "no_backpressure", "pipelined", "unordered", "latency", "max_threads",
        "thread_rate", "reset", "pure",
    ];
    const MEMORY_ATTRS: [&str; 5] = ["memory", "initialize", "non_replicated", "quad_port", "ecc"];
    for node in root.syntax().descendants() {
        let Some(var) = ast::VarDecl::cast(node.clone()) else { continue };
        let Some(parent) = node.parent() else { continue };
        let in_class = parent.kind() == SyntaxKind::DECL_LIST
            && parent.parent().is_some_and(|p| p.kind() == SyntaxKind::CLASS_DECL);
        let attrs: Vec<(String, kg_base::TextRange)> = var
            .attr_lists()
            .flat_map(|l| l.attrs().map(move |a| (a, l.syntax().text_range())))
            .filter_map(|(a, r)| a.ident_token().map(|t| (t.text().to_string(), r)))
            .collect();
        if matches!(var.ty(), Some(ast::Type::FnType(_))) && attrs.iter().all(|(a, _)| FN_TYPE_ATTRS.contains(&a.as_str())) {
            continue;
        }
        if in_class
            && (attrs.is_empty()
                || var.static_kw_token().is_some()
                || !attrs.iter().all(|(a, _)| FUNCTION_ATTRS.contains(&a.as_str())))
        {
            continue;
        }
        let Some(name_node) = var.name() else { continue };
        let Some(name) = name_node.ident_token() else { continue };
        // `inline uint32 foo;`: `funcModifier` precedes a function's return
        // type and nothing else (`Parser.hs:399-410`), so the parser expects
        // `(` after the name — "unexpected ';'" there
        // (`test/syntax/basics.k::341,343,345`).
        if var.syntax().children().any(|c| c.kind() == SyntaxKind::FN_MODIFIER) {
            let mut next = name_node.syntax().next_sibling_or_token();
            while let Some(n) = &next
                && n.kind().is_trivia()
            {
                next = n.next_sibling_or_token();
            }
            if let Some(next) = next {
                d.error(next.text_range(), "a function modifier introduces a function, not a variable");
            }
            continue;
        }
        if let Some((_, list)) = attrs.iter().find(|(a, _)| FUNCTION_ATTRS.contains(&a.as_str())) {
            let at = match var.static_kw_token() {
                Some(s) if s.text_range().start() > list.start() => s.text_range(),
                Some(_) => match var.ty() {
                    Some(t) => kg_syntax::trimmed_range(t.syntax()),
                    None => continue,
                },
                None => {
                    let mut next = name_node.syntax().next_sibling_or_token();
                    while let Some(n) = &next
                        && n.kind().is_trivia()
                    {
                        next = n.next_sibling_or_token();
                    }
                    match next {
                        Some(next) => next.text_range(),
                        None => continue,
                    }
                }
            };
            d.error(at, "an attribute here introduces a function, not a variable");
        } else if attrs.iter().any(|(a, _)| MEMORY_ATTRS.contains(&a.as_str()))
            && !matches!(var.ty(), Some(ast::Type::ArrayType(_)))
        {
            d.error(name.text_range(), "a memory attribute belongs to an array type");
        }
    }
}

#[cfg(test)]
mod imported_template_tests {
    use crate::tests::program;

    const H: &str = "module h { Bar }\n\ntemplate <typename T>\nstruct Bar\n{\n    T x;\n}\n";

    /// The parse-time table holds what the imports make visible
    /// (`findInImport`, `SymbolTable.hs:276-278`), so `static Bar bar;` over
    /// `import h` is `typeSpecifier'`'s "template arguments missing"
    /// (`Parser.hs:720-721`) exactly as it is for a template this file
    /// declares (`struct-template.k::4`, `union-template.k::4`,
    /// `template-instance-identifiers.k::1`; `kg-oracle --errors`
    /// 2026-09-10, at the name).
    #[test]
    fn a_bare_imported_template_name_is_a_parse_error() {
        let out = program(&[("main.k", "import h\n\ninline void main()\n{\n    static Bar bar;\n}\n"), ("h.k", H)]);
        assert_eq!(
            out.into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect::<Vec<_>>(),
            vec!["`Bar`: template arguments missing @ main.k:Bar"]
        );
    }

    /// An aliased import hides its names from unqualified use
    /// (`SymbolTable.hs:276-278`): `Bar` alone is not in the table, and
    /// whatever pass 9 says about it, the parser's verdict is not this one.
    #[test]
    fn an_aliased_import_does_not_put_the_name_in_the_table() {
        let out = program(&[("main.k", "import h as hh\n\ninline void main()\n{\n    static Bar bar;\n}\n"), ("h.k", H)]);
        assert!(!out.iter().any(|(m, _, _)| m.contains("template arguments missing")), "{out:?}");
    }
}

#[cfg(test)]
mod parse_time_kind_tests {
    use crate::tests::{accepts, program};

    fn at(text: &str) -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(_, at, _)| at).collect()
    }

    /// `test/syntax/template.k::10-22` and `scratchpad/p2/tp*.k`, positions
    /// from `kg-oracle --errors`, 2026-09-10.
    #[test]
    fn a_template_parameter_of_the_wrong_kind() {
        // In a class template's body: the `class` keyword.
        assert_eq!(at("template <uint32 N>\nclass Foo\n{\n    N x;\n}\n"), vec!["main.k:class"]);
        assert_eq!(at("template <uint32 N>\nclass Foo\n{\n    N Bar()\n    {\n        return 0;\n    }\n}\n"), vec!["main.k:class"]);
        assert_eq!(at("template <typename T, T N>\nclass Foo\n{\n    const N x;\n}\n"), vec!["main.k:class"]);
        assert_eq!(at("template <typename T, typename R, (T a, T b) -> R F>\nclass Foo\n{\n    F x;\n}\n"), vec!["main.k:class"]);
        assert_eq!(at("template <typename T>\nclass Foo\n{\n    inline void Bar()\n    {\n        T.x;\n    }\n}\n"), vec!["main.k:class"]);
        assert_eq!(at("template <typename T>\nclass Foo\n{\n    inline void Bar()\n    {\n        T();\n    }\n}\n"), vec!["main.k:class"]);
        // `T++` is a registered symbol error, on the `T`.
        assert_eq!(at("template <typename T>\nclass Foo\n{\n    inline void Bar()\n    {\n        T++;\n    }\n}\n"), vec!["main.k:T"]);
        // In the parameter list and in a function template: the token.
        assert_eq!(at("template <uint32 N, typename R, (N a, N b) -> R F>\nclass Foo\n{\n}\n"), vec!["main.k:N", "main.k:N"]);
        assert_eq!(at("template <typename T, typename R, (T a, T b) -> R F, F F1>\nclass Foo\n{\n}\n"), vec!["main.k:F"]);
        assert_eq!(at("template <uint32 N>\ninline void Go()\n{\n    N x;\n}\n"), vec!["main.k:N"]);
        assert_eq!(at("template <typename T>\ninline void Go()\n{\n    T.x;\n}\n"), vec!["main.k:."]);
        accepts("template <typename T, uint32 N>\nclass Foo\n{\n    T x;\n    uint<N> y;\n    inline uint32 Bar()\n    {\n        return bitsizeof T + N;\n    }\n}\n");
    }

    /// `test/syntax/basics.k::340-345` and `scratchpad/p2/at*.k`.
    #[test]
    fn an_attribute_a_variable_cannot_carry() {
        assert_eq!(at("inline void main()\n{\n    [[async]] uint32 foo;\n}\n"), vec!["main.k:;"]);
        assert_eq!(at("inline void main()\n{\n    [[async]] uint32 foo = 0;\n}\n"), vec!["main.k:="]);
        assert_eq!(at("inline void main()\n{\n    [[async]] uint32[1] foo;\n}\n"), vec!["main.k:;"]);
        // `static [[async]]` this parser fails on its own; the position rule
        // above is for the day it does not.
        assert!(!at("inline void main()\n{\n    static [[async]] uint32 x;\n}\n").is_empty());
        assert_eq!(at("inline void main()\n{\n    [[async]] static uint32 x;\n}\n"), vec!["main.k:static"]);
        // At module scope this parser already fails the declaration itself.
        assert!(!at("[[async]] uint32 foo;\n").is_empty());
        // A memory attribute on a non-array type this parser fails on its own.
        assert!(!at("inline void main()\n{\n    [[memory]] uint32 x;\n}\n").is_empty());
        assert!(!at("struct S\n{\n    [[memory]] uint32 a;\n}\n").is_empty());
        accepts("inline void main()\n{\n    static [[memory]] uint32[512] mem;\n    [[memory]] uint32[4] x;\n}\n");
        // `funcModifier` on a variable (`basics.k::341,343,345`).
        assert_eq!(at("inline void main()\n{\n    inline uint32 foo;\n}\n"), vec!["main.k:;"]);
        assert_eq!(at("inline void main()\n{\n    inline uint32 foo = 0;\n}\n"), vec!["main.k:="]);
        assert_eq!(at("inline void main()\n{\n    inline uint32[1] foo;\n}\n"), vec!["main.k:;"]);
        accepts("class C\n{\n    [[name(\"n\")]] uint32 x;\npublic:\n    void main()\n    {\n    }\n}\nexport C;\n");
    }

    /// A class member of function attributes alone is read as a member
    /// function (`Parser.hs:341-348`): `callbacks.k::81`, and the 2026-09-24
    /// probes. A function type's own attributes are that type's, a callback
    /// (`test/interface/external_class.k:12`), in a class and out of one.
    #[test]
    fn a_class_member_of_function_attributes_is_a_function() {
        let class = |member: &str| at(&format!("class Foo\n{{\n    {member}\n}}\n"));
        assert_eq!(class("[[reset]] (bool) -> void callback;"), vec!["main.k:;"]);
        assert_eq!(class("[[latency(2)]] uint32 x = 1;"), vec!["main.k:="]);
        assert_eq!(class("[[pipelined]] uint32 x;"), vec!["main.k:;"]);
        assert_eq!(class("[[pure]] (bool) -> void cb;"), vec!["main.k:;"]);
        accepts("class Foo\n{\n    [[async]] (uint32) -> void cb;\n    [[latency(2)]] (uint32) -> uint32 cb2;\n}\n");
        assert_eq!(
            at("inline void main()\n{\n    [[pipelined]] (bool) -> void cb;\n}\n"),
            vec!["main.k:[[pipelined]] (bool) -> void cb;"],
            "a variable of an attributed function type — the `const` rule's, not this one's"
        );
    }
}

/// The reference's grammar reaches an index, a member or a call only from a
/// name. `value` and `lvalue` are a qualified name followed by `[e]` and
/// `.m` accesses (`access`, `Parser.hs:803-819`); a call's callee is a
/// qualified name or such an lvalue followed by `.m` (`functionSpecifier`,
/// `methodSpecifier`, `:728-734`, `functionCall`, `:864`); a parenthesized
/// expression, a call and every other term are followed by an operator or
/// by nothing (`term`, `:838-857`). This parser's postfix is one loop
/// (divergence #13), so `(x)[i]`, `f()[0]`, `(s).a`, `c.f().a`, `(f)(1)`
/// and `x[1](2)` parse here; each is a `PARSE-ERROR` there (`kg-oracle
/// --errors` 2026-10-10, the reference at the postfix token in an
/// expression, at the name in a statement's — difffuzz [47], [48], [50]:
/// `(_mem)[i];`). Reported at the postfix token, against the immediate
/// base: a chain's inner links report their own. A type written in an
/// expression — `uint32[2]`, `decltype(x)[2]` — is an `ARRAY_TYPE` here,
/// never an index, and a type base is left alone.
pub(crate) fn pass_0_postfix_on_a_name(root: &ast::SourceFile, d: &mut Sink) {
    use ast::Expr::{FieldExpr, IndexExpr, Path, Type};
    for node in root.syntax().descendants() {
        let (refused, at, message) = if let Some(i) = ast::IndexExpr::cast(node.clone()) {
            let refused = i.base().is_some_and(|b| !matches!(b, Path(_) | FieldExpr(_) | IndexExpr(_) | Type(_)));
            (refused, i.l_brack_token(), "unexpected `[`: only a name, a member or an element of one is indexed")
        } else if let Some(f) = ast::FieldExpr::cast(node.clone()) {
            let refused = f.base().is_some_and(|b| !matches!(b, Path(_) | FieldExpr(_) | IndexExpr(_) | Type(_)));
            (refused, f.dot_token(), "unexpected `.`: only a name, a member or an element of one has members")
        } else if let Some(c) = ast::CallExpr::cast(node.clone()) {
            // An element is a base but no callee: `x[1](2)`.
            let refused = c.callee().is_some_and(|b| !matches!(b, Path(_) | FieldExpr(_) | Type(_)));
            let at = c.arg_list().and_then(|a| a.l_paren_token());
            (refused, at, "unexpected `(`: only a function's name or a method is called")
        } else {
            continue;
        };
        if refused && let Some(at) = at {
            d.error(at.text_range(), message);
        }
    }
}

/// `typeSpecifier'` (`Parser.hs:712-726`): a type position whose name the
/// parse-time table holds as a value — a variable, a parameter, a function,
/// a non-type template parameter — is `unexpectedSymbol s "type
/// specifier"`, a `PARSE-ERROR` at the name. Read off the sort
/// (`is_value_as_type`, `kg_hir::sort`'s `ValueInTypePosition`), which
/// restates the table: `Class Class;` over a parameter `bool Class`, and
/// `D D = {…};` in a class, whose variables seed its scope (difffuzz
/// 2026-10-10 [45], [46]; `kg-oracle --errors`, "unexpected Class").
pub(crate) fn pass_0_values_as_types(root: &ast::SourceFile, is_value_as_type: &dyn Fn(kg_base::TextRange) -> bool, d: &mut Sink) {
    for node in root.syntax().descendants() {
        if node.kind() != SyntaxKind::PATH || node.parent().is_none_or(|p| p.kind() != SyntaxKind::PATH_TYPE) {
            continue;
        }
        if !is_value_as_type(node.text_range()) {
            continue;
        }
        let Some(name) = ast::Path::cast(node.clone())
            .and_then(|p| p.path_segments().last())
            .and_then(|s| s.name_ref())
            .and_then(|n| n.ident_token())
        else {
            continue;
        };
        // An enclosing template's own value parameter is
        // [`pass_0_template_parameter_kinds`]'s, with its positions.
        let a_template_parameter = node.ancestors().filter_map(ast::TemplateDecl::cast).any(|t| {
            t.template_param_list().is_some_and(|l| {
                l.syntax().children().any(|p| {
                    p.children().any(|c| c.kind() == SyntaxKind::NAME && c.first_token().is_some_and(|t| t.text() == name.text()))
                })
            })
        });
        if a_template_parameter {
            continue;
        }
        d.error(node.text_range(), format!("`{}` names a value, not a type", name.text()));
    }
}

#[cfg(test)]
mod value_as_type_tests {
    use crate::tests::program;

    fn first(text: &str) -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `typeSpecifier'`'s `unexpectedSymbol` (`Parser.hs:723`), each at the
    /// name as `kg-oracle --errors` puts it (2026-10-10): a parameter, a
    /// class's variable (seeded, so the one being declared), a function, a
    /// constant (difffuzz [45], [46]). A local not yet declared is not in
    /// the table — `Foo Foo;` names the struct — and a `typename` parameter
    /// is a type. Red with the check unwired.
    #[test]
    fn a_name_the_table_holds_as_a_value_is_no_type() {
        assert_eq!(first("inline void Run(bool Class)\n{\n    Class Class;\n}\n"), vec!["`Class` names a value, not a type @ main.k:Class"]);
        assert_eq!(first("class Main\n{\n    D D;\n}\n"), vec!["`D` names a value, not a type @ main.k:D"]);
        assert_eq!(first("inline void g()\n{\n}\ninline void main()\n{\n    g x;\n}\n"), vec!["`g` names a value, not a type @ main.k:g"]);
        assert_eq!(first("const uint32 K = 3;\ninline void main()\n{\n    K x;\n}\n"), vec!["`K` names a value, not a type @ main.k:K"]);
        assert_eq!(first("struct Foo\n{\n    uint32 a;\n}\ninline void main()\n{\n    Foo Foo;\n}\n"), Vec::<String>::new());
        assert_eq!(first("template <typename T>\ninline void f()\n{\n    T x;\n}\n"), Vec::<String>::new());
    }
}

#[cfg(test)]
mod postfix_tests {
    use crate::tests::program;

    const CLASS: &str = "struct S\n{\n    uint32 a;\n}\nclass C\n{\n    uint32[4] x;\n    S s;\n    uint32[4] arr()\n    {\n        return x;\n    }\n    S st()\n    {\n        return s;\n    }\n    uint32 one(uint32 v)\n    {\n        return v;\n    }\n    void g()\n    {\n        uint32 y;\n";

    fn first(body: &str) -> Vec<String> {
        let text = format!("{CLASS}        {body}\n    }}\n}}\n");
        program(&[("main.k", text.as_str())]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `access`, `functionSpecifier` and `term` (`Parser.hs:728-734,803-819,
    /// 838-857`): an index, a member or a call follows a name, never a
    /// parenthesized expression or a call (`kg-oracle --errors`
    /// 2026-10-10, each at the postfix token; difffuzz [47], [48], [50]).
    /// Each red with the check unwired.
    #[test]
    fn an_index_a_member_or_a_call_follows_only_a_name() {
        let indexed = "unexpected `[`: only a name, a member or an element of one is indexed @ main.k:[";
        let accessed = "unexpected `.`: only a name, a member or an element of one has members @ main.k:.";
        let called = "unexpected `(`: only a function's name or a method is called @ main.k:(";
        assert_eq!(first("(x)[1];"), vec![indexed]);
        assert_eq!(first("y = arr()[0];"), vec![indexed]);
        assert_eq!(first("y = (s).a;"), vec![accessed]);
        assert_eq!(first("y = st().a;"), vec![accessed]);
        assert_eq!(first("y = (one)(1);"), vec![called]);
        for fine in ["y = x[1] + s.a + one(x[2]);", "y = (x[1]);", "y = bitsizeof(decltype(x)[2]);", "y = bitsizeof(uint32[2][3]);"] {
            assert_eq!(first(fine), Vec::<String>::new(), "{fine}");
        }
    }
}

/// Two more of the parser's own rejections, decided after the fact:
///
/// * `expression` (`Parser.hs:823-828`) parses a full expression and, if
///   what came back *is a type*, registers "Invalid use of a type as an
///   expression" at its start. A call argument and a variable initializer
///   are such positions (a template argument is `expressionTemplate` and
///   is not), and so are `mux`'s arguments (`Parser.hs:870`) and a
///   returned value — `mux(1, uint32, 2)` as an argument, an initializer
///   or a `return`'s value at the `uint32`, which pass 19's operand check
///   met only while no fold had deleted the arm, and `return uint32;`
///   (`kg-oracle --errors`, 2026-10-10, each a parse error there):
///   `umul27(uint16, 4)` at the `uint16`, `Go2(DoIt, Do<uint32>,
///   10)` at the instance, `const int32 x = T;` at the `T` of an enclosing
///   template's `typename` parameter — this last inside a template body,
///   since the parser does not care (`test/syntax/type-check.k::13,18`,
///   `higher-order-functions.k::54`). What is a type is answered by the
///   syntax (a written type, a `typename` parameter of an enclosing
///   template) or by `named_type` (a path the parser built as a type).
/// * `namedLValue` (`Parser.hs:795-800`): the receiver of a member call is
///   parsed as an lvalue, and a `const` variable there is "illegal use of
///   this constant as an lvalue" at the name (`test/syntax/const-correctness.k::18`,
///   `c.Foo(x)` over a module-scope `const C c`).
pub(crate) fn pass_0_types_and_constants_as_values(
    root: &ast::SourceFile,
    named_type: &dyn Fn(&ast::Expr) -> bool,
    templates: &super::templates::Templates,
    interner: &kg_base::Interner,
    d: &mut Sink,
) {
    let typename_params = |from: &kg_syntax::SyntaxNode| -> Vec<String> {
        from.ancestors()
            .filter_map(ast::TemplateDecl::cast)
            .filter_map(|t| t.template_param_list())
            .flat_map(|l| l.type_params().collect::<Vec<_>>())
            .filter(|p| p.template_param_list().is_none())
            .filter_map(|p| p.name()?.ident_token().map(|t| t.text().to_string()))
            .collect()
    };
    let is_type = |e: &ast::Expr| -> bool {
        match e {
            ast::Expr::Type(_) => true,
            ast::Expr::Path(p) => {
                let mut segments = p.path_segments();
                let (Some(first), None) = (segments.next(), segments.next()) else {
                    return named_type(e);
                };
                // `Do<uint32>` naming a class template is a type — `isType`
                // of the instance (`Parser.hs:825-827`;
                // `higher-order-functions.k::54`).
                if first.template_arg_list().is_some()
                    && let Some(ident) = first.name_ref().and_then(|n| n.ident_token())
                    && let Some((_, decl)) = templates.located(interner.intern(ident.text()))
                    && matches!(
                        decl.decl(),
                        Some(
                            ast::Item::ClassDecl(_)
                                | ast::Item::StructDecl(_)
                                | ast::Item::UnionDecl(_)
                                | ast::Item::EnumDecl(_)
                                | ast::Item::AliasDecl(_)
                        )
                    )
                {
                    return true;
                }
                let named_typename = first.template_arg_list().is_none()
                    && first
                        .name_ref()
                        .and_then(|n| n.ident_token())
                        .is_some_and(|t| {
                            typename_params(e.syntax()).iter().any(|p| p == t.text())
                                // …unless a parameter or local of the same
                                // name, registered by then, shadows it: `x`
                                // in `Foo(uint32 x)` of `template <typename
                                // x, …>` is the value (difffuzz, 2026-09-23).
                                && crate::facts::innermost_declaration(e.syntax(), t.text())
                                    .is_none_or(|d| d.text_range().end() > e.syntax().text_range().start())
                        });
                named_typename || named_type(e)
            }
            _ => false,
        }
    };
    for node in root.syntax().descendants() {
        if let Some(call) = ast::CallExpr::cast(node.clone()) {
            for arg in call.arg_list().map(|l| l.exprs().collect::<Vec<_>>()).unwrap_or_default() {
                if is_type(&arg) {
                    d.error(arg.syntax().text_range(), "Invalid use of a type as an expression");
                }
            }
            // `namedLValue` on the receiver of a member call.
            if let Some(ast::Expr::FieldExpr(field)) = call.callee()
                && let Some(ast::Expr::Path(base)) = field.base()
            {
                let mut segments = base.path_segments();
                if let (Some(first), None) = (segments.next(), segments.next())
                    && first.template_arg_list().is_none()
                    && let Some(token) = first.name_ref().and_then(|n| n.ident_token())
                    && let Some(decl) = crate::facts::innermost_declaration(base.syntax(), token.text())
                    && ast::VarDecl::cast(decl).is_some_and(|v| matches!(v.ty(), Some(ast::Type::ConstType(_))))
                {
                    d.error(
                        token.text_range(),
                        format!("`{}`: illegal use of this constant as an lvalue", token.text()),
                    );
                }
            }
            continue;
        }
        // `namedLValue` on an assignment's target, `++` and `--` included
        // (`assign`, `incDecStmt`, `Parser.hs:604-638`): a base name the
        // table holds as a `const` variable is the parser's error at the
        // name, before pass 9 says anything of a member it lacks — `s.Do =
        // Do;` over `const S s` (difffuzz 2026-10-10 [30], `kg-oracle
        // --errors`). A variable declared after the target is left alone.
        if node.kind() == SyntaxKind::ASSIGN_STMT
            && let Some(mut target) = node.children().find_map(ast::Expr::cast)
        {
            loop {
                target = match &target {
                    ast::Expr::FieldExpr(f) => match f.base() {
                        Some(b) => b,
                        None => break,
                    },
                    ast::Expr::IndexExpr(i) => match i.base() {
                        Some(b) => b,
                        None => break,
                    },
                    _ => break,
                };
            }
            if let ast::Expr::Path(base) = &target {
                let mut segments = base.path_segments();
                if let (Some(first), None) = (segments.next(), segments.next())
                    && first.template_arg_list().is_none()
                    && let Some(token) = first.name_ref().and_then(|n| n.ident_token())
                    && let Some(decl) = crate::facts::innermost_declaration(base.syntax(), token.text())
                    && decl.text_range().end() <= token.text_range().start()
                    && ast::VarDecl::cast(decl).is_some_and(|v| matches!(v.ty(), Some(ast::Type::ConstType(_))))
                {
                    d.error(token.text_range(), format!("`{}`: illegal use of this constant as an lvalue", token.text()));
                }
            }
        }
        if let Some(var) = ast::VarDecl::cast(node.clone())
            && let Some(init) = var.expr()
            && is_type(&init)
        {
            d.error(init.syntax().text_range(), "Invalid use of a type as an expression");
        }
        let values: Vec<ast::Expr> = match node.kind() {
            SyntaxKind::MUX_EXPR => ast::MuxExpr::cast(node.clone()).map(|m| m.exprs().collect()).unwrap_or_default(),
            SyntaxKind::RETURN_STMT => ast::ReturnStmt::cast(node.clone()).and_then(|r| r.expr()).into_iter().collect(),
            // `decltype = … <*> parens expression` (`Parser.hs:432`): `main`
            // in `decltype(main)` inside class `main`'s method `main` names
            // the class — the method is not in the table inside itself — and
            // is the type error at the name (difffuzz 2026-10-10 [32],
            // `kg-oracle --errors`).
            SyntaxKind::DECLTYPE_TYPE => node.children().filter_map(ast::Expr::cast).collect(),
            _ => Vec::new(),
        };
        for value in values.iter().filter(|v| is_type(v)) {
            d.error(value.syntax().text_range(), "Invalid use of a type as an expression");
        }
        // An array dimension is `brackets expression` (`Parser.hs:529,535`)
        // — `T[N]` over `template <typename T, typename N>` at the `N`
        // (`template-argument-deduction.k::68`). Only inside a template:
        // elsewhere pass 19's `check_type_as_expression` judges the same
        // position over the finished table, and a template's text is what
        // it declines.
        if let Some(array) = ast::ArrayType::cast(node.clone())
            && node.ancestors().any(|a| a.kind() == SyntaxKind::TEMPLATE_DECL)
        {
            for dim in array.exprs() {
                let mut dim = dim;
                while let ast::Expr::ParenExpr(p) = &dim {
                    let Some(inner) = p.expr() else { break };
                    dim = inner;
                }
                if is_type(&dim) {
                    d.error(dim.syntax().text_range(), "Invalid use of a type as an expression");
                }
            }
        }
    }
}

#[cfg(test)]
mod value_position_tests {
    use crate::tests::{accepts, program};

    fn at(text: &str) -> Vec<String> {
        program(&[("main.k", text)]).into_iter().map(|(m, at, _)| format!("{m} @ {at}")).collect()
    }

    /// `Parser.hs:823-828`, positions from `kg-oracle --errors` 2026-09-10.
    #[test]
    fn a_type_where_a_value_is_expected() {
        assert_eq!(
            at("template <typename T>\nclass C\n{\n    const int32 x = T;\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:T"]
        );
        assert_eq!(
            at("inline uint32 Go(uint32 a, uint32 b)\n{\n    return a + b;\n}\n\ninline void main()\n{\n    uint32 x = Go(uint16, 4);\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:uint16"]
        );
        assert_eq!(
            at("template <typename T>\nclass Do\n{\n}\n\ninline void Go(uint32 a)\n{\n}\n\ninline void main()\n{\n    Go(Do<uint32>);\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:Do<uint32>"]
        );
        accepts("inline void main()\n{\n    static assert(bitsizeof uint32 == 32);\n    uint32 x = cast<uint32>(3);\n}\n");
    }

    /// `Parser.hs:795-800`: a const receiver of a member call.
    #[test]
    fn a_constant_as_the_receiver_of_a_member_call() {
        assert_eq!(
            at("class C\n{\npublic:\n    void Foo(uint32 x)\n    {\n    }\n}\n\nconst C c = {};\n\ninline void Do(uint32 x)\n{\n    c.Foo(x);\n}\n"),
            vec!["`c`: illegal use of this constant as an lvalue @ main.k:c"]
        );
        accepts("class C\n{\npublic:\n    void Foo(uint32 x)\n    {\n    }\n}\n\ninline void Do(uint32 x)\n{\n    static C c;\n    c.Foo(x);\n}\n");
    }

    /// `decltype`'s operand is an `expression` (`Parser.hs:432,823-828`): a
    /// type there is the parser's error at it — `decltype(S)`, and `main`
    /// inside class `main`'s method `main`, which is not in the table
    /// inside itself, so the name is the class (difffuzz 2026-10-10 [32];
    /// `kg-oracle --errors`). Red with `decltype`'s operand unasked.
    #[test]
    fn a_type_as_decltypes_operand() {
        assert_eq!(
            at("struct S\n{\n    uint32 a;\n}\n\ninline void main()\n{\n    decltype(S) x;\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:S"]
        );
        assert_eq!(
            at("class main\n{\n    void main()\n    {\n        static assert(decltype(main) == () -> void);\n    }\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:main"]
        );
        accepts("inline void main()\n{\n    uint32 v;\n    decltype(v) x;\n}\n");
    }

    /// `Parser.hs:604-638,795-800`: a const variable as an assignment's
    /// base, `++` and a compound one included — the parser's error at the
    /// name, before pass 9's "Undefined member" of a member the struct
    /// lacks (difffuzz 2026-10-10 [30]; `kg-oracle --errors`). Red with
    /// the assignment's check removed.
    #[test]
    fn a_constant_as_the_base_of_an_assignment() {
        let s = "struct S\n{\n    uint32[2] a;\n}\n\n";
        let at_s = vec!["`s`: illegal use of this constant as an lvalue @ main.k:s"];
        assert_eq!(at(&format!("{s}inline void main()\n{{\n    const S s = {{}};\n    s.b = 1;\n}}\n")), at_s);
        assert_eq!(at(&format!("{s}inline void main()\n{{\n    const S s = {{}};\n    s.a[0] += 1;\n}}\n")), at_s);
        assert_eq!(
            at("inline void main()\n{\n    const uint32 x = 1;\n    x++;\n}\n"),
            vec!["`x`: illegal use of this constant as an lvalue @ main.k:x"]
        );
    }

    /// The parser rejects a type where a value goes wherever it parses one,
    /// a template's body included: `Go(S)` inside an uninstantiated
    /// template is "Invalid use of a type as an expression" at the `S`, a
    /// `PARSE-ERROR` (`kg-oracle --no-base --errors`, 2026-10-10). The CST
    /// engine typed no template body (`Inference.hs:63`) and said nothing;
    /// the parse built `S` a type (`crate::parsed`). Red with the CST
    /// engine's table as the oracle.
    #[test]
    fn a_type_as_a_value_in_a_template_body_is_the_parsers_error() {
        let s = "struct S\n{\n    uint8 a;\n}\n\ninline uint32 Go(uint32 a)\n{\n    return a;\n}\n\n";
        assert_eq!(
            at(&format!("{s}template <typename T>\ninline void f()\n{{\n    uint32 x = Go(S);\n}}\n")),
            vec!["Invalid use of a type as an expression @ main.k:S"]
        );
    }

    /// An array dimension naming a `typename` parameter, in a template's own
    /// text (`template-argument-deduction.k::68`, at the `N`); an `auto`
    /// parameter there is a value.
    #[test]
    fn a_typename_parameter_as_an_array_dimension() {
        assert_eq!(
            at("template <typename T, typename N>\ninline void Foo(T[N] x)\n{\n}\n"),
            vec!["Invalid use of a type as an expression @ main.k:N"]
        );
        accepts("template <typename T, auto N>\ninline void Foo(T[N] x)\n{\n}\n");
    }
}
