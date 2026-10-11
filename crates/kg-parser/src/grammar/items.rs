//! Declarations: files, modules, classes, structs, unions, enums, templates,
//! functions and variables.
//!
//! Top-level `decl` (`Parser.hs:245-251`), class `memberDecl` (`:341-350`) and
//! body-local `statementOrDecl` (`:471-479`) are three ordered choices over
//! largely the same alternatives, differing only in which of them they include
//! and in an unbounded `try` that tells a variable from a function. Here they
//! are one [`item`] dispatched on its first token, with the
//! variable-versus-function question answered by the token after the name —
//! `(` or not (grammar.md §8.2, §8.3).
//!
//! # A missing node kind
//!
//! `defaultInitialization` (`Parser.hs:314`) — `default = expr;` — has no kind
//! in the frozen [`SyntaxKind`] vocabulary. It is emitted as [`ERROR`] and
//! reported, per the rewrite's rule about not editing that enum. Nothing is
//! wrong with such input: it appears throughout
//! `test/logic/helper/designated_initializer_tests.k`. `MODULE_DIFF` is missing
//! too, but that one has a lossless flat encoding — see [`exposed_item`].

use super::{ExprMode, ITEM_RECOVERY, attrs, exprs, paths, stmts, types};
use crate::{Parser, SyntaxKind, TokenSet};
use SyntaxKind::*;

const PARAM_RECOVERY: TokenSet = TokenSet::new(&[COMMA, R_PAREN, L_CURLY, SEMICOLON]);
const TEMPLATE_PARAM_RECOVERY: TokenSet = TokenSet::new(&[COMMA, R_ANGLE, SEMICOLON, L_CURLY]);
const ENUM_RECOVERY: TokenSet = TokenSet::new(&[COMMA, R_CURLY, SEMICOLON]);
const EXPOSED_RECOVERY: TokenSet = TokenSet::new(&[COMMA, R_CURLY]);

/// Which of the reference's three declaration lists this position is.
///
/// They are not interchangeable. `decl` (`Parser.hs:245-251`) has no
/// `staticVariable`, no `staticFor` and no `defaultInitialization`;
/// `memberDecl` (`:341-350`) adds `defaultInitialization` and is the *only*
/// place a function may end in `;` instead of a body (`:417-419`); and
/// `statementOrDecl` (`:471-479`) is the only one with the `static` variable,
/// `static for` and `static default` forms.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    /// Top level: `decl`.
    File,
    /// Inside a `class`: `memberDecl`.
    Member,
    /// Inside a function or lambda body: `statementOrDecl`.
    Body,
}

/// `program = (moduleDecl | declarations)` (`Parser.hs:110-111`).
pub(crate) fn source_file_contents(p: &mut Parser<'_>) {
    if p.at(MODULE_KW) {
        module_decl(p);
    }
    // `declarations = parseImports >> many decl` (`Parser.hs:243`): imports may
    // appear only at the very start of a declaration list, never interleaved.
    while p.at(IMPORT_KW) {
        import_decl(p);
    }
    while !p.at_end() {
        let pos = p.token_pos();
        file_item(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a declaration");
        }
    }
}

/// `decl` — one top-level declaration.
pub(crate) fn file_item(p: &mut Parser<'_>) {
    item(p, Ctx::File);
}

/// `memberDecl` — one class member.
pub(crate) fn member_item(p: &mut Parser<'_>) {
    item(p, Ctx::Member);
}

/// The declaration half of `statementOrDecl`.
pub(crate) fn body_item(p: &mut Parser<'_>) {
    item(p, Ctx::Body);
}

/// One declaration, in any of the three positions that admit one.
pub(crate) fn item(p: &mut Parser<'_>, ctx: Ctx) {
    // Classes nest, and a class body holds declarations; see [`super::depth`].
    let Some(_guard) = super::depth::enter() else {
        p.err_recover(super::depth::TOO_DEEP, ITEM_RECOVERY);
        return;
    };
    match p.current() {
        // `hidden (skipMany semi)` — a stray `;` after a declaration is
        // silently accepted (`Parser.hs:251, 266`).
        SEMICOLON => {
            let m = p.start();
            p.bump(SEMICOLON);
            m.complete(p, EMPTY_STMT);
        }
        TEMPLATE_KW => template_decl(p, ctx),
        USING_KW => alias(p),
        ENUM_KW => enum_decl(p),
        STRUCT_KW => record(p, STRUCT_DECL),
        UNION_KW => record(p, UNION_DECL),
        CLASS_KW => class_decl(p),
        STATIC_ASSERT_KW => static_assert(p),
        PUBLIC_KW | PRIVATE_KW => visibility(p),
        DEFAULT_KW => default_init(p),
        STATIC_KW => static_item(p, ctx),
        IMPORT_KW => {
            p.error("imports may only appear at the start of a declaration list");
            import_decl(p);
        }
        MODULE_KW => {
            p.error("a module declaration may only open a file");
            module_decl(p);
        }
        _ => decl(p, ctx),
    }
}

/// Everything spelled with a leading `static` in a *declaration* position.
///
/// `decl` and `memberDecl` reach `static` only through `staticIf`
/// (`Parser.hs:247, 344`) — and `static assert` through `commonDecl`'s
/// `staticAssert` (`:255`). `staticVariable` (`:464`), `staticFor` (`:591`) and
/// `static defaultInitialization` (`:474`) live in `statementOrDecl` alone, so
/// outside a function body they are syntax errors. They are still parsed, so
/// that one diagnostic is all the reader gets.
fn static_item(p: &mut Parser<'_>, ctx: Ctx) {
    if ctx == Ctx::Body {
        stmts::static_stmt(p, true);
        return;
    }
    if p.nth_at(1, IF_KW) {
        // `staticIf p` takes the *caller's* production (`Parser.hs:593-595`),
        // so a top-level branch holds declarations and a class-body branch
        // holds members — never statements.
        stmts::static_if(p, if ctx == Ctx::Member { member_item } else { file_item });
        return;
    }
    if p.nth_at(1, IDENT) && p.nth_text(1) == "assert" {
        static_assert(p);
        return;
    }
    p.error("`static` is only allowed on a declaration inside a function body");
    stmts::static_stmt(p, true);
}

// ------------------------------------------------------------ modules

/// `moduleDecl` header (`Parser.hs:185-192`). No terminating `;`.
///
/// The reference nests the whole rest of the file inside the `ModuleDeclF`
/// node; here the declarations stay siblings, matching what
/// [`SyntaxKind::MODULE_DECL`] documents itself as (`module a.b { x, y };`) and
/// keeping the tree shallow for a formatter.
fn module_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(MODULE_KW);
    paths::module_path(p);
    let list = p.start();
    p.expect(L_CURLY);
    if !p.at(R_CURLY) {
        loop {
            let pos = p.token_pos();
            exposed_item(p);
            if p.token_pos() == pos {
                p.err_recover("expected an exposed name", EXPOSED_RECOVERY);
                if p.token_pos() == pos {
                    break;
                }
            }
            if !p.eat(COMMA) {
                break;
            }
        }
    }
    p.expect(R_CURLY);
    list.complete(p, MODULE_EXPORT_LIST);
    m.complete(p, MODULE_DECL);
}

/// `exposedItem = moduleDiff | moduleIdentifier | identifier`
/// (`Parser.hs:187, 191-192`).
///
/// There is no `MODULE_DIFF` kind, so `module a \ b` is written flat: the
/// `module` keyword, a [`MODULE_PATH`], a `\` and a second [`MODULE_PATH`],
/// all directly inside the [`MODULE_EXPORT_LIST`]. That is lossless and
/// unambiguous — a diff is exactly an entry with a `\` in it.
fn exposed_item(p: &mut Parser<'_>) {
    if p.at(MODULE_KW) {
        p.bump(MODULE_KW);
        paths::module_path(p);
        if p.eat(BACKSLASH) {
            paths::module_path(p);
        }
        return;
    }
    paths::name_ref(p);
}

/// `import moduleName ("as" name)?` (`Parser.hs:196`). No terminating `;`.
fn import_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(IMPORT_KW);
    paths::module_path(p);
    if p.eat(AS_KW) {
        paths::name(p);
    }
    m.complete(p, IMPORT_DECL);
}

// ------------------------------------------------------------- type declarations

/// `using identifier "=" typ ";"` (`Parser.hs:274-276`).
fn alias(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(USING_KW);
    paths::name(p);
    p.expect(EQ);
    types::type_(p);
    p.expect(SEMICOLON);
    m.complete(p, ALIAS_DECL);
}

/// `enum identifier ":" typ "{" enumConst ("," enumConst)* "}"`
/// (`Parser.hs:282-292`).
///
/// The underlying type is **not** optional and the body needs at least one
/// constant (grammar.md §9 trap 11).
fn enum_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(ENUM_KW);
    paths::name(p);
    p.expect(COLON);
    types::type_(p);

    let body = p.start();
    p.expect(L_CURLY);
    loop {
        let pos = p.token_pos();
        enum_variant(p);
        if p.token_pos() == pos {
            p.err_recover("expected an enum constant", ENUM_RECOVERY);
            if p.token_pos() == pos {
                break;
            }
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    p.expect(R_CURLY);
    body.complete(p, DECL_LIST);
    m.complete(p, ENUM_DECL);
}

fn enum_variant(p: &mut Parser<'_>) {
    let m = p.start();
    paths::name(p);
    if p.eat(EQ) {
        exprs::expr(p, ExprMode::Full);
    }
    m.complete(p, ENUM_VARIANT);
}

/// `struct` / `union` (`Parser.hs:294-305`).
///
/// The body is `memberVariable sepEndBy semi`, so an empty body and a trailing
/// `;` are both legal (grammar.md §9 trap 22), and a member is a bare
/// `type name` — no attributes, no initializer (trap 12).
fn record(p: &mut Parser<'_>, node: SyntaxKind) {
    let m = p.start();
    p.bump_any();
    paths::name(p);

    let body = p.start();
    p.expect(L_CURLY);
    while !p.at_end() && !p.at(R_CURLY) {
        let pos = p.token_pos();
        field(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a field declaration");
            continue;
        }
        if !p.eat(SEMICOLON) {
            break;
        }
    }
    p.expect(R_CURLY);
    body.complete(p, DECL_LIST);
    m.complete(p, node);
}

fn field(p: &mut Parser<'_>) {
    let m = p.start();
    types::type_(p);
    paths::name(p);
    m.complete(p, VAR_DECL);
}

/// `class identifier "{" SEQ<memberDecl> "}"` (`Parser.hs:331-339`).
///
/// The reference parses a class body up to three times so that members are
/// visible to each other regardless of order (grammar.md §3.4). That is a
/// name-resolution device, not a syntactic one, and has no counterpart here.
fn class_decl(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(CLASS_KW);
    paths::name(p);
    decl_list(p);
    m.complete(p, CLASS_DECL);
}

/// A braced list of declarations: a class body.
fn decl_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.expect(L_CURLY);
    while !p.at_end() && !p.at(R_CURLY) {
        let pos = p.token_pos();
        member_item(p);
        if p.token_pos() == pos {
            p.err_and_bump("expected a class member");
        }
    }
    p.expect(R_CURLY);
    m.complete(p, DECL_LIST);
}

fn visibility(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump_any();
    p.expect(COLON);
    m.complete(p, VISIBILITY);
}

/// `static_assert(e);` or `static assert(e);` (`Parser.hs:571-574`).
///
/// The second spelling goes through `keyword2`, which is exactly why `assert`
/// is not in the reserved set.
pub(crate) fn static_assert(p: &mut Parser<'_>) {
    let m = p.start();
    if !p.eat(STATIC_ASSERT_KW) {
        p.bump(STATIC_KW);
        p.expect(IDENT);
    }
    p.expect(L_PAREN);
    exprs::expr(p, ExprMode::Full);
    p.expect(R_PAREN);
    p.expect(SEMICOLON);
    m.complete(p, STATIC_ASSERT_DECL);
}

/// `default = expression;` (`Parser.hs:314`), optionally `static`-prefixed in
/// statement position (`:474`).
///
/// Emitted as [`ERROR`] only because the vocabulary has no `DEFAULT_INIT`; see
/// the module docs.
pub(crate) fn default_init(p: &mut Parser<'_>) {
    let m = p.start();
    p.eat(STATIC_KW);
    p.expect(DEFAULT_KW);
    p.expect(EQ);
    exprs::expr(p, ExprMode::Full);
    p.expect(SEMICOLON);
    m.complete(p, ERROR);
}

// ------------------------------------------------------------------ templates

/// `templateParameters` then the declaration it wraps (`Parser.hs:352-375`).
///
/// `paramList` requires at least one parameter, so `template<>` is a syntax
/// error even though a template *argument* list may be empty (§9 trap 23).
fn template_decl(p: &mut Parser<'_>, ctx: Ctx) {
    let m = p.start();
    let params = p.start();
    p.bump(TEMPLATE_KW);
    open_angle(p);
    loop {
        let pos = p.token_pos();
        template_param(p);
        if p.token_pos() == pos {
            p.err_recover("expected a template parameter", TEMPLATE_PARAM_RECOVERY);
            if p.token_pos() == pos {
                break;
            }
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    close_angle(p);
    params.complete(p, TEMPLATE_PARAM_LIST);
    item(p, ctx);
    m.complete(p, TEMPLATE_DECL);
}

/// `typenameParam | templateTemplateParam | nonTypeParam` (`Parser.hs:362-371`),
/// in that order.
fn template_param(p: &mut Parser<'_>) {
    let m = p.start();
    if p.at(TYPENAME_KW) {
        p.bump(TYPENAME_KW);
        paths::name(p);
        if p.eat(EQ) {
            types::type_(p);
        }
        m.complete(p, TYPE_PARAM);
        return;
    }
    if p.at(TEMPLATE_KW) {
        // `template <paramKind, ...> typename Name (= templateSpecifier)?`.
        // The inner list is `MP.sepBy`, so it may legitimately be empty here.
        p.bump(TEMPLATE_KW);
        open_angle(p);
        if !p.at_angle(R_ANGLE) {
            loop {
                let pos = p.token_pos();
                param_kind(p);
                if p.token_pos() == pos {
                    break;
                }
                if !p.eat(COMMA) {
                    break;
                }
            }
        }
        close_angle(p);
        p.expect(TYPENAME_KW);
        paths::name(p);
        if p.eat(EQ) {
            types::type_(p);
        }
        m.complete(p, TYPE_PARAM);
        return;
    }
    types::type_(p);
    paths::name(p);
    if p.eat(EQ) {
        // `expressionTemplate` (`Parser.hs:367`): `>` must still close the list.
        exprs::expr(p, ExprMode::TemplateArg);
    }
    m.complete(p, VALUE_PARAM);
}

/// `paramKind = typename | typ` (`Parser.hs:371`).
fn param_kind(p: &mut Parser<'_>) {
    if p.at(TYPENAME_KW) && !p.nth_at(1, IDENT) {
        let m = p.start();
        p.bump(TYPENAME_KW);
        m.complete(p, TYPENAME_TYPE);
        return;
    }
    types::type_(p);
}

fn open_angle(p: &mut Parser<'_>) {
    if p.at_angle(L_ANGLE) {
        p.bump(L_ANGLE);
    } else {
        p.error("expected `<`");
    }
}

fn close_angle(p: &mut Parser<'_>) {
    if p.at_angle(R_ANGLE) {
        p.bump(R_ANGLE);
    } else {
        p.error("expected `>`");
    }
}

// ------------------------------------------------- variables and functions

/// Everything that begins with an optional `[[ ... ]]` group: `export`,
/// `extern`, every variable and every function.
///
/// The first two are settled by their keyword. The rest, after an optional
/// `static` and an optional `inline` / `noinline`, are all "a type, a name,
/// and then either a parameter list or an initializer" — and that one token,
/// `(` or not, *is* the declaration-versus-function decision the reference
/// spends an unbounded `try` on (`Parser.hs:246, 348`, grammar.md §8.2-8.3).
pub(crate) fn decl(p: &mut Parser<'_>, ctx: Ctx) {
    if !at_decl_start(p) {
        p.err_recover("expected a declaration", ITEM_RECOVERY);
        return;
    }
    let m = p.start();
    attrs::opt_attrs(p, attrs::ITEM_ATTRS);

    if p.at(EXPORT_KW) {
        p.bump(EXPORT_KW);
        types::type_(p);
        p.expect(SEMICOLON);
        m.complete(p, EXPORT_DECL);
        return;
    }
    if p.at(EXTERN_KW) {
        p.bump(EXTERN_KW);
        types::type_(p);
        p.expect(SEMICOLON);
        m.complete(p, EXTERN_DECL);
        return;
    }

    p.eat(STATIC_KW);
    let mut modified = false;
    if matches!(p.current(), INLINE_KW | NOINLINE_KW) {
        let fm = p.start();
        p.bump_any();
        fm.complete(p, FN_MODIFIER);
        modified = true;
    }

    types::type_(p);
    paths::name(p);

    if p.at(L_PAREN) {
        param_list(p);
        // `decl <- if isNothing modi && kind == MemberFunction then optional
        // semi else return Nothing` (`Parser.hs:417-419`): a free function, or
        // a member function carrying `inline`/`noinline`, is committed to a
        // braced body and fails at the `;`. Both halves of that condition are
        // known right here.
        if p.at(SEMICOLON) {
            if ctx != Ctx::Member || modified {
                p.error("only an unmodified member function may be declared without a body");
            }
            p.bump(SEMICOLON);
            m.complete(p, FN_PROTO);
        } else {
            if p.at(L_CURLY) {
                stmts::block(p);
            } else {
                p.error("expected a function body or `;`");
            }
            m.complete(p, FN_DECL);
        }
    } else {
        if p.eat(EQ) {
            exprs::expr(p, ExprMode::Full);
        }
        p.expect(SEMICOLON);
        m.complete(p, VAR_DECL);
    }
}

fn at_decl_start(p: &Parser<'_>) -> bool {
    p.at_ts(types::TYPE_FIRST)
        || matches!(
            p.current(),
            L_BRACK | L_PAREN | EXPORT_KW | EXTERN_KW | INLINE_KW | NOINLINE_KW | STATIC_KW
        )
}

/// `funcParams` (`Parser.hs:377-382`).
pub(crate) fn param_list(p: &mut Parser<'_>) {
    let m = p.start();
    p.bump(L_PAREN);
    if !p.at(R_PAREN) {
        loop {
            let pos = p.token_pos();
            param(p);
            if p.token_pos() == pos {
                p.err_recover("expected a parameter", PARAM_RECOVERY);
                if p.token_pos() == pos {
                    break;
                }
            }
            if !p.eat(COMMA) {
                break;
            }
        }
    }
    p.expect(R_PAREN);
    m.complete(p, PARAM_LIST);
}

fn param(p: &mut Parser<'_>) {
    let m = p.start();
    attrs::opt_attrs(p, attrs::PARAM_ATTRS);
    types::type_(p);
    paths::name(p);
    m.complete(p, PARAM);
}
