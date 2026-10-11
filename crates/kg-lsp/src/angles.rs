//! Is this `<` a less-than, or does it open a template argument list?
//!
//! This is the one question a TextMate grammar cannot answer, and the reason
//! the reference extension scopes every `<` and `>` as
//! `keyword.operator.comparison` — including the ones in
//! `reverse<uint<Width>>(x)`. A CST answers it structurally: the token's parent
//! is a `TEMPLATE_ARG_LIST` or it is not.
//!
//! While the grammar is still being written the tree is mostly `ERROR` nodes,
//! so there is a second answer for that case: the same bounded token scan the
//! parser itself uses (`kg-parser/src/grammar/lookahead.rs`), reimplemented
//! here over the lexed token stream. It rests on the two properties that make
//! the language decidable without a symbol table — `<` and `>` are `no_assoc`,
//! so `a < b > c` is not an expression at all, and the angle operators are
//! removed from the operator table used inside template arguments — which
//! together mean *an angle region that closes at depth zero without crossing a
//! statement boundary is always a template argument list*.

use kg_base::{FxHashSet, TextRange, TextSize};
use kg_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use SyntaxKind::*;

/// How an angle bracket should be scoped.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AngleRole {
    /// `Foo<T>`, `int<N>`, `cast<T>(x)`, `template<typename T>`.
    Punctuation,
    /// `a < b`, and the `<`/`>` halves of `<=`, `>=`, `<<`, `>>`.
    Operator,
}

/// Upper bound on one speculative scan, matching the parser's.
const SCAN_LIMIT: usize = 4096;

/// Upper bound on all speculative scanning in one file, so that a pathological
/// buffer full of `<` costs linear time rather than quadratic. Past the budget
/// an unresolved angle is an operator, which is the pre-CST status quo.
const SCAN_BUDGET: usize = 250_000;

/// Tokens that cannot occur inside a template argument list at brace depth 0.
/// Seeing one means the `<` was a comparison after all. Mirrors
/// `lookahead::NOT_IN_TEMPLATE_ARGS`.
const NOT_IN_TEMPLATE_ARGS: &[SyntaxKind] = &[
    SEMICOLON,
    CLASS_KW,
    STRUCT_KW,
    UNION_KW,
    ENUM_KW,
    IMPORT_KW,
    MODULE_KW,
    PUBLIC_KW,
    PRIVATE_KW,
    USING_KW,
    EXPORT_KW,
    EXTERN_KW,
    RETURN_KW,
    IF_KW,
    ELSE_KW,
    FOR_KW,
    WHILE_KW,
    DO_KW,
    SWITCH_KW,
    CASE_KW,
    DEFAULT_KW,
    BREAK_KW,
    BARRIER_KW,
    REORDER_KW,
    ATOMIC_KW,
    STATIC_ASSERT_KW,
];

/// Node kinds whose direct `<`/`>` children are punctuation by construction.
fn is_angle_bearing_node(kind: SyntaxKind) -> bool {
    matches!(kind, TEMPLATE_ARG_LIST | TEMPLATE_PARAM_LIST | PARAM_INT_TYPE | CAST_EXPR | FAN_OUT_EXPR)
}

/// Start offsets of every angle token that opens or closes a template region,
/// as decided by the fallback scan.
#[derive(Clone, Debug, Default)]
pub struct AngleMap {
    punctuation: FxHashSet<TextSize>,
}

impl AngleMap {
    /// Scans `root`'s token stream. Only consulted for tokens whose parent node
    /// is `ERROR` or `SOURCE_FILE`; a parsed tree answers structurally instead.
    ///
    /// A file the parser understood needs no scan at all, and that is the
    /// common case on every keystroke — so the decision is made while the token
    /// list is being collected, and a clean tree costs one pass and nothing
    /// more.
    pub fn build(root: &SyntaxNode) -> AngleMap {
        AngleMap::build_with(root, false)
    }

    /// `force` runs the scan even on a tree that does not need it, so the scan
    /// stays under test however much of the grammar is finished.
    fn build_with(root: &SyntaxNode, force: bool) -> AngleMap {
        let mut needed = force;
        let toks: Vec<Tok> = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .map(|t| {
                if matches!(t.kind(), L_ANGLE | R_ANGLE) {
                    needed |= t.parent().is_none_or(|p| matches!(p.kind(), ERROR | SOURCE_FILE));
                }
                Tok { kind: t.kind(), range: t.text_range(), glued_eq: glued_eq(&t) }
            })
            .collect();

        let mut punctuation = FxHashSet::default();
        if !needed {
            return AngleMap { punctuation };
        }
        let mut budget = SCAN_BUDGET;
        for i in 0..toks.len() {
            if toks[i].kind != L_ANGLE || toks[i].glued_eq {
                continue;
            }
            if punctuation.contains(&toks[i].range.start()) {
                continue;
            }
            if budget == 0 {
                break;
            }
            if let Some((marks, cost)) = scan(&toks, i) {
                budget = budget.saturating_sub(cost);
                punctuation.extend(marks);
            } else {
                budget = budget.saturating_sub(SCAN_LIMIT.min(toks.len() - i));
            }
        }
        AngleMap { punctuation }
    }

    fn says_punctuation(&self, at: TextSize) -> bool {
        self.punctuation.contains(&at)
    }
}

/// The role of one `<` or `>` token.
pub fn angle_role(token: &SyntaxToken, map: &AngleMap) -> AngleRole {
    debug_assert!(matches!(token.kind(), L_ANGLE | R_ANGLE));

    match token.parent() {
        Some(parent) if is_angle_bearing_node(parent.kind()) => AngleRole::Punctuation,
        // An unparsed region: the tree has nothing to say, so ask the scan.
        Some(parent) if matches!(parent.kind(), ERROR | SOURCE_FILE) => {
            if map.says_punctuation(token.text_range().start()) {
                AngleRole::Punctuation
            } else {
                AngleRole::Operator
            }
        }
        // A `<` the parser placed inside a real node — a `BIN_EXPR`, say — is a
        // decision already made, and the scan must not second-guess it.
        _ => AngleRole::Operator,
    }
}

#[derive(Copy, Clone, Debug)]
struct Tok {
    kind: SyntaxKind,
    range: TextRange,
    /// `<=` and `>=` are a `<`/`>` glued to an `=`. The reference forbids an
    /// angle followed by `=` (`Parser/Lexer.hs:272-273`), so such a token is
    /// never a bracket.
    glued_eq: bool,
}

fn glued_eq(token: &SyntaxToken) -> bool {
    // The lexer never merges an angle with anything, so "followed by `=`" is a
    // question about the very next character of the source, which is the first
    // character of the immediately following token.
    match token.next_token() {
        Some(next) => {
            next.text_range().start() == token.text_range().end() && next.text().starts_with('=')
        }
        None => false,
    }
}

/// Scans forward from the `<` at `start`. On success returns the start offsets
/// of every angle token in the region, and the number of tokens visited.
fn scan(toks: &[Tok], start: usize) -> Option<(Vec<TextSize>, usize)> {
    let (mut angle, mut paren, mut brack, mut brace) = (0usize, 0usize, 0usize, 0usize);
    let mut marks = Vec::new();

    for offset in 0..SCAN_LIMIT {
        let i = start + offset;
        let tok = toks.get(i)?;
        let nested = paren > 0 || brack > 0 || brace > 0;

        match tok.kind {
            L_PAREN => paren += 1,
            R_PAREN => paren = paren.checked_sub(1)?,
            L_BRACK => brack += 1,
            R_BRACK => brack = brack.checked_sub(1)?,
            L_CURLY => brace += 1,
            R_CURLY => brace = brace.checked_sub(1)?,

            L_ANGLE if !nested && !tok.glued_eq => {
                angle += 1;
                marks.push(tok.range.start());
            }
            R_ANGLE if !nested && !tok.glued_eq => {
                marks.push(tok.range.start());
                angle -= 1;
                if angle == 0 {
                    return Some((marks, offset + 1));
                }
            }
            k if brace == 0 && NOT_IN_TEMPLATE_ARGS.contains(&k) => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kg_syntax::parse_file;

    /// Every `<`/`>` in `text`, paired with its role.
    ///
    /// The scan is forced on, so these assertions exercise the fallback path
    /// even once the grammar parses everything structurally — at which point
    /// the structural answer takes precedence anyway and both must agree.
    fn roles(text: &str) -> Vec<(String, AngleRole)> {
        let parse = parse_file(text);
        let root = parse.syntax_node();
        let map = AngleMap::build_with(&root, true);
        root.descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| matches!(t.kind(), L_ANGLE | R_ANGLE))
            .map(|t| (t.text().to_owned(), angle_role(&t, &map)))
            .collect()
    }

    /// Every angle token in `text` has `role`, and there is at least one.
    fn only(role: AngleRole, text: &str) -> bool {
        let rs = roles(text);
        !rs.is_empty() && rs.iter().all(|(_, r)| *r == role)
    }

    /// Every angle token in `text` has `role` — vacuously true if the parser
    /// merged them all into one composite operator token, which is what will
    /// happen to `<=`, `>=`, `<<` and `>>` once the grammar lands.
    fn never(role: AngleRole, text: &str) -> bool {
        roles(text).iter().all(|(_, r)| *r != role)
    }

    #[test]
    fn a_template_instantiation_is_punctuation() {
        assert!(only(AngleRole::Punctuation, "Foo<uint32> x;"));
        assert!(only(AngleRole::Punctuation, "template<typename T> class Foo {}"));
        assert!(only(AngleRole::Punctuation, "int<8> x;"));
        assert!(only(AngleRole::Punctuation, "cast<uint32>(y);"));
        assert!(only(AngleRole::Punctuation, "fan_out<4>(y);"));
    }

    #[test]
    fn nested_arguments_close_with_two_independent_rangles() {
        // The reference's lexer never merges `>>`, which is exactly what makes
        // this parse; both `>` here are punctuation.
        let rs = roles("reverse<uint<Width>>(x);");
        assert_eq!(rs.len(), 4);
        assert!(rs.iter().all(|(_, r)| *r == AngleRole::Punctuation), "{rs:?}");
    }

    #[test]
    fn a_comparison_is_an_operator() {
        assert!(only(AngleRole::Operator, "bool b = a < c;"));
        assert!(only(AngleRole::Operator, "bool b = a > c;"));
        // `a < b > c` is not an expression at all in this language, so the
        // scan reading it as the type `a<b>` naming `c` is the same reading
        // the reference reaches. It is a declaration, hence punctuation.
        assert!(only(AngleRole::Punctuation, "a<b> c;"));
    }

    #[test]
    fn a_glued_equals_is_never_a_bracket() {
        // `<=` and `>=` arrive as two tokens until the parser merges them.
        assert!(never(AngleRole::Punctuation, "bool b = a <= c;"));
        assert!(never(AngleRole::Punctuation, "bool b = a >= c;"));
    }

    #[test]
    fn a_shift_is_an_operator() {
        assert!(never(AngleRole::Punctuation, "uint32 x = a << 2;"));
        assert!(never(AngleRole::Punctuation, "uint32 x = a >> 2;"));
    }

    /// A property of the scan itself, asserted on the map rather than on the
    /// roles so that it stays true however the finished grammar chooses to
    /// recover from the same input.
    #[test]
    fn an_unclosed_angle_marks_nothing() {
        for text in ["Foo<uint32 x;", "a < b", "a < b < c < d < e", "<<<<<<<<"] {
            let parse = parse_file(text);
            let map = AngleMap::build_with(&parse.syntax_node(), true);
            assert!(map.punctuation.is_empty(), "runaway scan on {text:?}: {map:?}");
        }
    }

    #[test]
    fn a_parsed_tree_needs_no_scan_at_all() {
        // The common case on every keystroke: the parser already decided, and
        // `angle_role` reads the decision off the tree.
        let parse = parse_file("Foo<uint32> x;\n");
        if parse.errors().is_empty() {
            let map = AngleMap::build(&parse.syntax_node());
            assert!(map.punctuation.is_empty(), "the scan ran on a clean tree");
            assert!(only(AngleRole::Punctuation, "Foo<uint32> x;\n"));
        }
    }

    #[test]
    fn a_parenthesized_comparison_inside_arguments_stays_an_operator() {
        // `(...)` re-enables the full operator table (`Parser.hs:845`), so the
        // inner `<` is a comparison while the outer pair is punctuation.
        let rs = roles("Foo<(a < b)> x;");
        assert_eq!(rs.len(), 3, "{rs:?}");
        assert_eq!(rs[0].1, AngleRole::Punctuation);
        assert_eq!(rs[1].1, AngleRole::Operator);
        assert_eq!(rs[2].1, AngleRole::Punctuation);
    }
}
