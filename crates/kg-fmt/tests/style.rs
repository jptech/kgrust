//! The worked before/after examples of `docs/spec/style.md` §12.
//!
//! Each is drawn from a real corpus line and states what `kg fmt` must produce
//! with the §11 defaults. They are assertions rather than snapshots because the
//! expected output is not this crate's opinion — it is the spec's.

use kg_base::TextRange;
use kg_config::Fmt;
use kg_fmt::{check_file, format_file, format_range};

fn fmt(src: &str) -> String {
    format_file(src, &Fmt::default()).expect("the fixture should parse and format")
}

#[test]
fn example_1_tabs_trailing_space_and_the_template_keyword() {
    // `library/numeric/complex.k:8-14` — the only tab-indented file in the
    // corpus, with a trailing space and no final newline.
    let before = "//| Complex number\ntemplate<typename T> \nstruct complex\n{\n\tT real_part;\n\tT imag_part;\n}";
    let after = "//| Complex number\ntemplate <typename T>\nstruct complex\n{\n    T real_part;\n    T imag_part;\n}\n";
    assert_eq!(fmt(before), after);
}

#[test]
fn example_2_a_trailing_comma_export_list_becomes_leading_comma() {
    // `library/numeric/int/multi_word.k:5-11` — the only trailing-comma export
    // list in 208. The continuation indent moves from 6 (aligned under the
    // first item) to 4 (aligned under `{`), which is what 843 of 844
    // leading-comma continuations do.
    let before = "module numeric.int.multi_word\n    { add_with_carry,\n      sub_with_borrow,\n      min_with_metadata\n    }\n";
    let after = "module numeric.int.multi_word\n    { add_with_carry\n    , sub_with_borrow\n    , min_with_metadata\n    }\n";
    assert_eq!(fmt(before), after);
}

#[test]
fn example_3_doc_comments_inside_an_export_list_survive() {
    // `library/data/array.k:7-16` — byte-identical is the required output.
    let src = "module data.array\n    { array\n    , //| # Constructing arrays\n      indices\n    , copy_array\n    }\n";
    assert_eq!(fmt(src), src);
    assert!(check_file(src, &Fmt::default()));
}

#[test]
fn example_4_the_corpus_s_one_kr_function_definition() {
    // `test/library/control/async/rendezvous/lenient.k:36-38` — 6817 Allman
    // function definitions to 2 K&R.
    let before = "class C\n{\npublic:\n    void run(unit::tag_t tag)    {\n        f(tag);\n    }\n}\n";
    let after = "class C\n{\npublic:\n    void run(unit::tag_t tag)\n    {\n        f(tag);\n    }\n}\n";
    assert_eq!(fmt(before), after);
}

#[test]
fn example_5_a_kr_lambda_in_a_pipelined_for() {
    // `test/library/numeric/int/multi_word.k:57-62`. The brace moves to its own
    // line at the indent of the line that *started the statement*, not at the
    // lambda's column.
    let before = "inline void t(unit::tag_t tag)\n{\n    pipelined_for(16, [tag](uint8 itr){\n        uint32 a = rng1(itr);\n        uint32 b = rng2(itr);\n    });\n}\n";
    let after = "inline void t(unit::tag_t tag)\n{\n    pipelined_for(16, [tag](uint8 itr)\n    {\n        uint32 a = rng1(itr);\n        uint32 b = rng2(itr);\n    });\n}\n";
    assert_eq!(fmt(before), after);
}

#[test]
fn example_7_the_aligned_enum_table_is_reemitted_verbatim() {
    // `library/processor/risc_v/isa.k:185-192`. This violates three defaults at
    // once — the only leading-comma enum body (58 : 1), a brace that hugs its
    // first constant, and an aligned `=` column (31 of 1690) — and is
    // nevertheless correct output, because §10.3 makes a run of lines sharing a
    // column an atom.
    let src = "enum ABI : Reg\n{ zero = 0   //< Hard-wired zero\n, ra   = 1   //< Return address\n, sp   = 2   //< Stack pointer\n, gp   = 3   //< Global pointer\n}\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn example_8_attribute_placement_is_preserved_per_attribute() {
    // `library/compiler/hal.k:8-17` and `library/control/async.k:38-43`: both
    // byte-identical, because the split is per attribute, not per author
    // (§10.8). `[[schedule(…)]]` is 53/53 own-line and `[[pipelined]]` 228/228
    // same-line, so there is no majority to normalize against.
    let own_line = "class fmul32\n{\npublic:\n    [[latency(LATENCY)]]\n    float32 op(float32 x, float32 y);\n}\n\n[[name(\"_hardware_dsp__fmul32\")]]\nextern fmul32;\n";
    assert_eq!(fmt(own_line), own_line);

    let same_line = "class C\n{\npublic:\n    [[async]] void exec(task_t task, then_t then)\n    {\n        then(task());\n    }\n}\n";
    assert_eq!(fmt(same_line), same_line);
}

#[test]
fn example_9_a_long_trailing_doc_never_forces_a_break() {
    // `library/codec/crc.k:38-45`: byte-identical, even though one line is 179
    // columns. `line_width` is a break-if-you-must threshold for *lists*, and
    // the overflow is entirely inside a `//<` doc comment whose column is
    // shared with three others.
    let src = "class crc\n{\npublic:\n    //| Process up to `MaxBytes` of new input data.\n    inline uint<Width> compute\n        ( count_t<ThreadCount> identifier   //< Which of the `ThreadCount` CRCs to update.\n        , uint8[MaxBytes] input             //< Input data.\n        , count_t<MaxBytes> input_bytes     //< Number of valid bytes in the input data.\n        , bool reset                        //< Indicates if CRC value should be reset before processing `input`.  Should be set to `true` on the first call associated with a particular input stream.\n        )\n    {\n        return 0;\n    }\n}\n";
    assert_eq!(fmt(src), src);
}

#[test]
fn example_10_static_assert_keeps_its_tight_paren() {
    // `library/base.k:48-57`. `static assert (` → `static assert(` (494 : 67
    // corpus, 156 : 8 library), trailing spaces stripped, final newline added,
    // and *no* blank lines inserted between the four one-line declarations.
    let before = "// Comment above.\nstatic assert (decltype(F::mul) == (float32, float32) -> float32);\nstatic assert (decltype(F::add) == (float32, float32) -> float32);\n\n// Another.\nstatic assert (version <= device_schema::schema_version);  ";
    let after = "// Comment above.\nstatic assert(decltype(F::mul) == (float32, float32) -> float32);\nstatic assert(decltype(F::add) == (float32, float32) -> float32);\n\n// Another.\nstatic assert(version <= device_schema::schema_version);\n";
    assert_eq!(fmt(before), after);
}

#[test]
fn a_parameter_list_that_must_break_takes_the_leading_comma_shape() {
    // §3.3: 89 leading : 4 trailing, and in 113 of 125 the `(` moves to its own
    // line at +4.
    let long = format!(
        "inline void configure(uint32 {a}, uint32 {b}, uint32 {c}, uint32 {d})\n{{\n}}\n",
        a = "first_parameter_name",
        b = "second_parameter_name",
        c = "third_parameter_name",
        d = "fourth_parameter_name"
    );
    let out = fmt(&long);
    assert!(out.contains("inline void configure\n    ( uint32 first_parameter_name\n"), "{out}");
    assert!(out.contains("\n    , uint32 second_parameter_name\n"), "{out}");
    assert!(out.contains("\n    )\n"), "{out}");
}

#[test]
fn imports_form_one_block_with_a_blank_line_on_each_side() {
    // §9.3: 307 of 307 import blocks are followed by a blank line, 300 of 307
    // are one contiguous run.
    let src = "module a.b { X }\nimport data.counter\n\nimport data.memory\nclass C\n{\n}\n";
    let out = fmt(src);
    assert_eq!(
        out,
        "module a.b { X }\n\nimport data.counter\nimport data.memory\n\nclass C\n{\n}\n"
    );
}

#[test]
fn sort_imports_is_opt_in() {
    let src = "import sync.lock\nimport data.counter\n\nclass C\n{\n}\n";
    assert!(fmt(src).starts_with("import sync.lock\nimport data.counter\n"));

    let sorted = format_file(src, &Fmt { sort_imports: true, ..Fmt::default() }).unwrap();
    assert!(sorted.starts_with("import data.counter\nimport sync.lock\n"), "{sorted}");
}

#[test]
fn format_range_touches_only_the_items_it_covers() {
    let src = "inline void a()\n{\n}\n\ninline void b(){g();}\n\ninline void c()\n{\n}\n";
    let start = src.find("inline void b").unwrap() as u32;
    let range = TextRange::new(start.into(), (start + 5).into());
    let edits = format_range(src, range, &Fmt::default()).expect("should produce edits");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0].new_text, "inline void b() { g(); }");
    // The declarations on either side are untouched.
    assert!(u32::from(edits[0].range.start()) >= start);
    assert!(usize::from(edits[0].range.end()) <= src.find("inline void c").unwrap());
}

#[test]
fn format_range_declines_a_file_that_does_not_parse() {
    let src = "class {\n";
    assert_eq!(format_range(src, TextRange::new(0.into(), 3.into()), &Fmt::default()), None);
}

#[test]
fn a_stray_semicolon_after_a_type_body_stays_where_it_was() {
    // §2.4 would drop it (2323 : 48), but dropping a token is not a layout
    // change: the formatter's contract is that the token sequence of the output
    // is the token sequence of the input, and `kg fmt` is not a linter.
    let src = "class C\n{\n};\n\nclass D\n{\n}\n";
    assert_eq!(fmt(src), src);
}
