//! Snapshots of the cases where comment attachment is decided.
//!
//! Every one of these is a place where a plausible formatter loses a comment,
//! moves it across a token, or glues it onto the line below. They are snapshots
//! rather than assertions because what matters is the whole shape of the
//! output, and because a change in any of them should be looked at rather than
//! silently accepted.

use kg_config::Fmt;
use kg_fmt::format_file;

fn fmt(src: &str) -> String {
    format_file(src, &Fmt::default()).expect("the fixture should parse and format")
}

/// Formats twice and returns the result, asserting the two agree.
fn stable(src: &str) -> String {
    let once = fmt(src);
    assert_eq!(fmt(&once), once, "not idempotent");
    once
}

#[test]
fn a_trailing_doc_after_a_struct_field() {
    // `//<` documents what *precedes* it (`Parser/Lexer.hs:65`), so `kg-parser`
    // deliberately leaves it outside the following declaration; the formatter
    // has to find it as the previous sibling's trailing trivia and keep it on
    // that sibling's line. 525 of the corpus's 545 `//<` comments are trailing.
    insta::assert_snapshot!(stable(
        "struct Decoded\n\
         {\n\
         \x20   uint8 rd;      //< Destination register.\n\
         \x20   uint8 rs1;     //< First operand.\n\
         \x20   uint8 rs2;     //< Second operand.\n\
         }\n"
    ));
}

#[test]
fn a_comment_between_an_attribute_and_its_declaration() {
    // The attribute group, the comment and the declaration are three separate
    // children; the comment must stay between them and must not pull the
    // attribute onto the declaration's line or vice versa.
    insta::assert_snapshot!(stable(
        "class hal\n\
         {\n\
         public:\n\
         \x20   [[latency(LATENCY)]]\n\
         \x20   // The device supplies this.\n\
         \x20   float32 op(float32 x, float32 y);\n\
         }\n"
    ));
}

#[test]
fn a_comment_inside_an_empty_block() {
    // The block has no statements at all, so the comment is the whole of the
    // gap between `{` and `}` — the one place a formatter that only decorates
    // statements will drop it.
    insta::assert_snapshot!(stable(
        "inline void f()\n\
         {\n\
         \x20   // nothing to do yet\n\
         }\n\
         \n\
         inline void g()\n\
         {\n\
         \x20   /* nor here */\n\
         }\n"
    ));
}

#[test]
fn a_doc_comment_run_followed_by_plain_comments() {
    // `//|` opens a doc block that bare `//` lines continue (`Lexer.hs:84-87`),
    // and 2695 of 2696 continuations sit at the marker's own column. The run
    // must not be split, reordered, or rewritten into more `//|` lines.
    insta::assert_snapshot!(stable(
        "//| Basic FIFO.\n\
         //\n\
         // The second paragraph is a bare `//` line, which the lexer folds\n\
         // into the same doc comment.\n\
         \n\
         // A separate, ordinary comment.\n\
         // Still ordinary.\n\
         class FIFO\n\
         {\n\
         }\n"
    ));
}

#[test]
fn a_trailing_comment_at_eof_with_no_newline() {
    // 86 of 480 corpus files end without a newline. Trailing trivia at end of
    // file belongs to the root node, so it is reached through a gap that has no
    // element after it at all.
    insta::assert_snapshot!(stable(
        "inline void f()\n{\n}\n\n// the last word, unterminated"
    ));
}

#[test]
fn a_doc_comment_inside_a_module_export_list() {
    // `library/data/array.k:7-16` — one of the four "mixed comma" export lists.
    // A naive leading-comma re-emitter produces `, //| # Constructing arrays
    // indices` and swallows the item.
    insta::assert_snapshot!(stable(
        "module data.array\n\
         \x20   { array\n\
         \x20   , //| # Constructing arrays\n\
         \x20     indices\n\
         \x20   , copy_array\n\
         \x20   }\n"
    ));
}

#[test]
fn a_long_trailing_doc_does_not_break_the_code_in_front_of_it() {
    // `docs/spec/style.md` §12 example 9: the line is over 100 columns, but the
    // overflow is entirely inside a `//<` doc comment, and §5.4 forbids letting
    // that drive a break. The alignment of the four markers is load-bearing.
    insta::assert_snapshot!(stable(
        "class crc\n\
         {\n\
         public:\n\
         \x20   uint8 compute(uint8 identifier, uint8 input, bool reset);   //< Which of the CRCs to update, and whether to reset it before processing the input.\n\
         \x20   uint8 peek(uint8 identifier);                               //< Read without updating.\n\
         }\n"
    ));
}

#[test]
fn a_block_comment_spanning_lines_keeps_its_shape() {
    insta::assert_snapshot!(stable(
        "/*|\n\
         RISC-V ISA types, constants and helper functions.\n\
         \n\
         \x20   indented example line\n\
         */\n\
         module processor.risc_v.isa { Base }\n"
    ));
}

#[test]
fn a_comment_between_a_case_label_and_its_body() {
    insta::assert_snapshot!(stable(
        "inline void f()\n\
         {\n\
         \x20   switch (x)\n\
         \x20   {\n\
         \x20       case 0:\n\
         \x20           // fall through to the assignment\n\
         \x20           y = 1;\n\
         \x20           break;\n\
         \x20       default:  // nothing else is legal\n\
         \x20           break;\n\
         \x20   }\n\
         }\n"
    ));
}

#[test]
fn a_lambda_argument_keeps_its_allman_brace_at_the_statement_indent() {
    // `docs/spec/style.md` §12 example 5.
    insta::assert_snapshot!(stable(
        "inline void add_with_carry_test(unit::tag_t tag)\n\
         {\n\
         \x20   pipelined_for(16, [tag](uint8 itr){\n\
         \x20       uint32 a = rng1(itr);   // first\n\
         \x20       uint32 b = rng2(itr);\n\
         \x20   });\n\
         }\n"
    ));
}
