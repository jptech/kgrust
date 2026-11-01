use super::*;

/// Every line of every block's text is the file's text where the block says
/// it is.
#[track_caller]
fn maps_back(host: &str, blocks: &[Block]) {
    for block in blocks {
        let mut at = 0usize;
        for line in block.text.split_inclusive('\n') {
            let code = line.trim_end_matches('\n');
            let start = usize::from(block.host(TextSize::from(at as u32)));
            assert_eq!(host.get(start..start + code.len()), Some(code), "{:?} at {at} of {:?}", line, block.text);
            at += line.len();
        }
        let fence = host[block.fence].trim_start();
        assert!(fence.starts_with("```") || fence.starts_with("~~~"), "the fence: {fence:?}");
    }
}

fn texts(blocks: &[Block]) -> Vec<&str> {
    blocks.iter().map(|b| b.text.as_str()).collect()
}

const GUIDE: &str = "\
# Guide

Untagged, then tagged:

```
inline void untagged() {}
```

```kanagawa
uint32 x = 3;
```

```sh
kg check top.k
```

```cpp
inline void cpp() {}
```

```C++
int main() {}
```

~~~
inline void tilde() {}
~~~

* In a list:

    ```
    inline void listed()
    {
    }
    ```
";

#[test]
fn untagged_and_kanagawa_blocks_are_kanagawa() {
    let blocks = markdown_blocks(GUIDE, Select::Kanagawa);
    assert_eq!(
        texts(&blocks),
        [
            "inline void untagged() {}\n",
            "uint32 x = 3;\n",
            "inline void tilde() {}\n",
            "inline void listed()\n{\n}\n",
        ]
    );
    maps_back(GUIDE, &blocks);
    assert!(blocks.iter().all(|b| b.closed && b.directive == Directive::None));
}

#[test]
fn a_crlf_document_maps_line_by_line() {
    let md = GUIDE.replace('\n', "\r\n");
    let blocks = markdown_blocks(&md, Select::Kanagawa);
    assert_eq!(blocks.len(), 4);
    maps_back(&md, &blocks);
    // The parser's text runs over a line end the code does not hold: `b`
    // comes as "\nb", whose `\r` was the piece before's.
    let md = "```\r\na\r\nb\r\n```\r\n";
    let blocks = markdown_blocks(md, Select::Kanagawa);
    assert_eq!(texts(&blocks), ["a\nb\n"]);
    maps_back(md, &blocks);
}

/// A tab the parser splits between a list item's indentation and the code's
/// is spaces the file does not hold; each line still maps to its own.
#[test]
fn a_tab_indented_block_in_a_list_maps_line_by_line() {
    let md = "- An item:\n\n  ```\n\tinline void f()\n\t{\n\t}\n  ```\n";
    let blocks = markdown_blocks(md, Select::Kanagawa);
    assert_eq!(blocks.len(), 1);
    let block = &blocks[0];
    assert_eq!(block.text, "  inline void f()\n  {\n  }\n");
    assert_eq!(block.host(TextSize::from(1)), block.host(TextSize::from(0)), "the spaces have one place, past the tab");
    let lines: Vec<usize> = block
        .text
        .split_inclusive('\n')
        .scan(0usize, |at, line| {
            let start = *at;
            *at += line.len();
            Some(usize::from(block.host(TextSize::from(start as u32))))
        })
        .map(|host| md[..host].matches('\n').count() + 1)
        .collect();
    assert_eq!(lines, [4, 5, 6], "{:?}", block.text);
}

#[test]
fn chkdoc_takes_every_block_but_cpp_bash_and_md() {
    let blocks = markdown_blocks(GUIDE, Select::Chkdoc);
    assert_eq!(
        texts(&blocks),
        [
            "inline void untagged() {}\n",
            "uint32 x = 3;\n",
            "kg check top.k\n",
            "inline void cpp() {}\n",
            "inline void tilde() {}\n",
            "inline void listed()\n{\n}\n",
        ],
        "`sh` is not `bash`, and `cpp` is not `C++` (chkdoc.cpp:332)"
    );
    let md = "```bash\nls\n```\n\n```md\n# x\n```\n\n```C++11\nint x;\n```\n";
    assert!(markdown_blocks(md, Select::Chkdoc).is_empty());
}

#[test]
fn the_comment_on_the_line_before_the_fence_directs() {
    let md = "<!-- invalid -->\n```\nnot Kanagawa\n```\n\n<!-- emptyfunction.k -->\n```\nreturn;\n```\n\n<!-- prettier-ignore -->\n```\ninline void a() {}\n```\n\n<!-- invalid -->\n\n```\ninline void b() {}\n```\n";
    let blocks = markdown_blocks(md, Select::Kanagawa);
    let directives: Vec<&Directive> = blocks.iter().map(|b| &b.directive).collect();
    assert_eq!(
        directives,
        [&Directive::Invalid, &Directive::Context("emptyfunction.k".to_owned()), &Directive::None, &Directive::None],
        "a pragma says nothing; a blank line between parts the comment from the fence"
    );
    let range = blocks[1].directive_range.expect("the context's comment");
    assert_eq!(&md[range], "<!-- emptyfunction.k -->");
}

#[test]
fn an_empty_block_and_an_unclosed_one_are_seen_as_such() {
    let blocks = markdown_blocks("```\n```\n\n```\n\n```\n\n```\nopen\n", Select::Kanagawa);
    assert_eq!(texts(&blocks), ["", "\n", "open\n"]);
    assert_eq!(blocks.iter().map(|b| b.closed).collect::<Vec<_>>(), [true, true, false]);
}

#[test]
fn a_module_block_names_the_file_an_import_finds() {
    let blocks = markdown_blocks(
        "```\nmodule foo.bar\n{ f }\n```\n\n```\nmodule solo\n```\n\n```\nmodule foo.bar { f }\n```\n",
        Select::Kanagawa,
    );
    let paths: Vec<Option<Utf8PathBuf>> = blocks.iter().map(Block::module_path).collect();
    assert_eq!(paths, [Some("foo/bar.k".into()), Some("solo.k".into()), None]);
}

#[test]
fn an_offset_past_a_line_or_the_text_maps_to_the_block_s_end() {
    let md = "Text.\n\n```\nab\ncd\n```\nAfter.\n";
    let block = &markdown_blocks(md, Select::Kanagawa)[0];
    assert_eq!(usize::from(block.host(TextSize::from(2))), md.find("ab").unwrap() + 2, "a line's newline");
    assert_eq!(usize::from(block.host(TextSize::from(3))), md.find("cd").unwrap(), "the next line's start");
    assert_eq!(usize::from(block.end), md.rfind("```").unwrap(), "the closing fence");
    assert_eq!(block.host(TextSize::of(block.text.as_str())), block.end);
    // What a context adds after the block, its `;` and `}`, has no place in the
    // file but the block's end.
    assert_eq!(block.host(TextSize::of(block.text.as_str()) + TextSize::from(4)), block.end, "past the text");
    let md = "* Item.\n\n  ```\n  ab\n  ```\n";
    let block = &markdown_blocks(md, Select::Kanagawa)[0];
    assert_eq!(usize::from(block.end), md.rfind("```").unwrap(), "an indented closing fence");
    assert_eq!(block.host(TextSize::of(block.text.as_str())), block.end);
}

const DOCUMENTED: &str = "\
//| Doubles a value.
//
// ```kanagawa
// uint8 y = twice(3);
// y++;
// ```
//
// ```
// 6D 5A 1B A6
// ```
inline uint8 twice(uint8 x) { return x * 2; }

/*|
    A block comment.

    ```kanagawa
    inline void blocked() {}
    ```
*/
inline void other() {}
";

#[test]
fn a_doc_comment_s_kanagawa_blocks_alone_are_checked() {
    let root = kg_syntax::parse_file(DOCUMENTED).syntax_node();
    let blocks = doc_comment_blocks(&root);
    assert_eq!(texts(&blocks), ["uint8 y = twice(3);\ny++;\n", "inline void blocked() {}\n"], "the untagged hex is not");
    maps_back(DOCUMENTED, &blocks);
}

/// A fence indented in the doc comment's Markdown: a line's column counts
/// from where the doc comment's text of that line starts.
#[test]
fn an_indented_block_in_a_doc_comment_maps_to_its_own_columns() {
    let source = "//| In a list:\n//\n// * An item:\n//\n//     ```kanagawa\n//     uint8 y = 3;\n//     y++;\n//     ```\ninline void f() {}\n";
    let root = kg_syntax::parse_file(source).syntax_node();
    let blocks = doc_comment_blocks(&root);
    assert_eq!(texts(&blocks), ["uint8 y = 3;\ny++;\n"]);
    maps_back(source, &blocks);
}

#[test]
fn a_context_is_its_text_either_side_of_one_mark() {
    assert_eq!(Context::parse("c.k", "a\n***\nb"), Ok(Context { name: "c.k".into(), before: "a\n".into(), after: "b\n".into() }));
    assert_eq!(Context::parse("c.k", "a\nb\n"), Err(0));
    assert_eq!(Context::parse("c.k", "***\n***\n"), Err(2));
    assert_eq!(Context::body().after, ";\n}\n");
}

// ---------------------------------------------------------------------------
// through the frontend
// ---------------------------------------------------------------------------

/// A directory of its own for one test, under the system's temporary one;
/// gone when dropped.
struct Scratch(Utf8PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Utf8Path;
    fn deref(&self) -> &Utf8Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str, files: &[(&str, &str)]) -> Scratch {
    let dir = std::env::temp_dir().join(format!("kg-doc-check-{}-{name}", std::process::id()));
    let dir = Utf8PathBuf::from_path_buf(dir).expect("a UTF-8 temporary directory");
    let _ = std::fs::remove_dir_all(&dir);
    for (file, text) in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    Scratch(dir)
}

fn checker(dir: &Utf8Path) -> Checker {
    // No import directory: no base library, as `Parser/IO.hs:73-74` has it.
    Checker::new(Settings::fallback(dir), Select::Kanagawa)
}

fn verdicts(report: &FileReport) -> Vec<(u32, Verdict)> {
    report.blocks.iter().map(|c| (c.line, c.verdict.clone())).collect()
}

fn passed(context: &str) -> Verdict {
    Verdict::Passed { context: context.to_owned(), mini_base: false }
}

#[test]
fn a_block_passes_as_a_file_or_as_a_function_body() {
    let md = "```\ninline void f() {}\n```\n\n```\nuint32 x = 3;\nx++\n```\n";
    let dir = scratch("contexts", &[("README.md", md)]);
    let report = checker(&dir).check_file(&dir.join("README.md")).expect("readable");
    assert_eq!(verdicts(&report), [(1, passed("emptyfile.k")), (5, passed("emptyfunction.k"))]);
}

#[test]
fn a_failure_points_at_the_block_s_own_line_and_column() {
    let md = "# Title\n\nSome text.\n\n```kanagawa\ninline void f()\n{\n    uint32 x = ;\n}\n```\n";
    let dir = scratch("failure", &[("README.md", md)]);
    let mut checker = checker(&dir);
    let report = checker.check_file(&dir.join("README.md")).expect("readable");
    assert_eq!(verdicts(&report), [(5, Verdict::Failed)]);
    let diagnostics = &report.blocks[0].diagnostics;
    assert!(!diagnostics.is_empty());
    let first = &diagnostics[0];
    assert_eq!(first.range.file, report.file, "{first:?}");
    let at = usize::from(first.range.range.start());
    assert_eq!(&md[at..at + 1], ";", "the parse error is at the `;` of the README's line 8: {first:?}");
    assert!(first.notes.iter().any(|n| n == "the code block at line 5, checked as a whole file"), "{:?}", first.notes);
}

#[test]
fn a_statement_block_s_failure_is_reported_as_one() {
    let md = "Text.\n\n```\nuint32 x = 3;\nx = nothing_here;\n```\n";
    let dir = scratch("body-failure", &[("README.md", md)]);
    let report = checker(&dir).check_file(&dir.join("README.md")).expect("readable");
    assert_eq!(verdicts(&report), [(3, Verdict::Failed)]);
    let first = &report.blocks[0].diagnostics[0];
    let at = usize::from(first.range.range.start());
    assert!(md[at..].starts_with("nothing_here"), "furthest into the block, line 5: {first:?}");
    assert!(first.notes.iter().any(|n| n.ends_with("checked as the body of a function")), "{:?}", first.notes);
}

/// A warning passes a block, as a compile that only warns exits 0
/// (`chkdoc.cpp:110-112`), unless the project makes warnings errors, as
/// `--Werror` makes that compile fail (`Main.hs:98-99`).
#[test]
fn a_warning_fails_a_block_only_under_warnings_as_errors() {
    let md = "```\nimport .cmdargs as params\n```\n";
    let dir = scratch("werror", &[("doc.md", md)]);
    let report = checker(&dir).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report), [(1, passed("emptyfile.k"))]);
    let mut settings = Settings::fallback(&*dir);
    settings.warnings_as_errors = true;
    let report = Checker::new(settings, Select::Kanagawa).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report), [(1, Verdict::Failed)]);
    assert!(report.blocks[0].diagnostics[0].warning.is_some(), "{:?}", report.blocks[0].diagnostics);
}

/// A block of something else recovers into many errors; what it was checked
/// as, and the way out, are said once.
#[test]
fn a_block_s_notes_are_on_its_first_diagnostic() {
    let md = "```\ncmake \\\n    -S $HOME/kanagawa\n```\n\n```kanagawa\ncmake \\\n    -S $HOME/kanagawa\n```\n";
    let dir = scratch("notes", &[("BUILDING.md", md)]);
    let report = checker(&dir).check_file(&dir.join("BUILDING.md")).expect("readable");
    let [untagged, tagged] = &report.blocks[..] else { panic!("two blocks: {report:?}") };
    assert!(untagged.diagnostics.len() > 1 && tagged.diagnostics.len() > 1, "{report:?}");
    for block in [untagged, tagged] {
        assert!(block.diagnostics[1..].iter().all(|d| d.notes.is_empty()), "{:?}", block.diagnostics);
    }
    assert_eq!(untagged.diagnostics[0].notes.len(), 2, "{:?}", untagged.diagnostics[0]);
    assert!(untagged.diagnostics[0].notes[1].contains("an untagged block is read as Kanagawa"));
    assert_eq!(tagged.diagnostics[0].notes.len(), 1, "it says it is Kanagawa: {:?}", tagged.diagnostics[0]);
}

#[test]
fn an_invalid_block_is_skipped_and_the_block_s_own_problems_are_errors() {
    let md = "<!-- invalid -->\n```\nnot Kanagawa at all\n```\n\n```\n```\n\n<!-- nowhere.k -->\n```\ninline void f() {}\n```\n\n```\nunclosed\n";
    let dir = scratch("structure", &[("doc.md", md)]);
    let report = checker(&dir).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report), [(2, Verdict::Skipped), (6, Verdict::Failed), (10, Verdict::Failed), (14, Verdict::Failed)]);
    let messages: Vec<&str> = report.blocks[1..].iter().map(|c| c.diagnostics[0].message.as_str()).collect();
    assert_eq!(
        messages,
        ["this code block is empty", "cannot read `nowhere.k`, the context this code block names", "this code block is never closed"]
    );
    let range = report.blocks[2].diagnostics[0].range.range;
    assert_eq!(&md[range], "<!-- nowhere.k -->");
}

#[test]
fn a_block_may_pass_against_mini_base_alone() {
    let md = "```\nconst auto x = only_in_mini;\n```\n\n```\nconst auto y = only_in_base;\n```\n\n<!-- emptyfile.k -->\n```\nconst auto z = only_in_mini;\n```\n";
    let dir = scratch(
        "mini-base",
        &[("doc.md", md), ("lib/base.k", "const auto only_in_base = 1;\n"), ("lib/mini-base.k", "const auto only_in_mini = 2;\n")],
    );
    let mut settings = Settings::fallback(&*dir);
    settings.import_dirs = vec![dir.join("lib")];
    let report = Checker::new(settings, Select::Kanagawa).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(
        verdicts(&report),
        [
            (1, Verdict::Passed { context: "emptyfile.k".to_owned(), mini_base: true }),
            (5, passed("emptyfile.k")),
            (10, Verdict::Failed),
        ],
        "a named context is tried against the base library alone (chkdoc.cpp:190-196)"
    );
}

#[test]
fn a_named_context_beside_the_document_holds_the_block() {
    let md = "<!-- in-class.k -->\n```\nuint8 count() { return _n; }\n```\n\n<!-- no-mark.k -->\n```\ninline void f() {}\n```\n";
    let context = "class Holder\n{\n    uint8 _n = 0;\npublic:\n***\n}\n";
    let dir = scratch("named", &[("doc.md", md), ("in-class.k", context), ("no-mark.k", "// nowhere to go\n")]);
    let report = checker(&dir).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report), [(2, passed("in-class.k")), (7, Verdict::Failed)]);
    let refused = &report.blocks[1].diagnostics[0];
    assert_eq!(refused.message, "the context `no-mark.k` has 0 `***` lines; it needs exactly one, where the block goes");
    assert_eq!(&md[refused.range.range], "<!-- no-mark.k -->");
}

#[test]
fn a_module_block_is_importable_by_the_blocks_after_it() {
    let md = "```\nmodule shapes.square\n{ area }\n\ninline uint16 area(uint8 side) { return side * side; }\n```\n\n```\nimport shapes.square\n\ninline uint16 nine() { return area(3); }\n```\n\n```\nimport shapes.square\n\ninline void wrong() { bool b = area(1, 2); }\n```\n";
    let dir = scratch("modules", &[("doc.md", md)]);
    let mut checker = checker(&dir);
    let report = checker.check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report)[..2], [(1, passed("emptyfile.k")), (8, passed("emptyfile.k"))]);
    assert_eq!(report.blocks[2].verdict, Verdict::Failed);
    // Gone with the document: another file's blocks do not see it.
    let other = "```\nimport shapes.square\n```\n";
    std::fs::write(dir.join("other.md"), other).unwrap();
    let report = checker.check_file(&dir.join("other.md")).expect("readable");
    assert_eq!(verdicts(&report), [(1, Verdict::Failed)]);
}

/// `export-class.k`'s shape: the error is in the imported module, the
/// `while compiling` note at the export, and here both are blocks.
#[test]
fn an_error_in_a_module_block_and_its_note_land_in_the_document() {
    let md = "```\nmodule helper.exports\n    { PublicData\n    }\n\nclass PublicData\n{\npublic:\n    uint32 x;\n}\n```\n\n```\nimport helper.exports\n\nexport PublicData;\n```\n";
    let dir = scratch("in-module", &[("doc.md", md)]);
    let report = checker(&dir).check_file(&dir.join("doc.md")).expect("readable");
    assert_eq!(verdicts(&report), [(1, passed("emptyfile.k")), (13, Verdict::Failed)]);
    let first = &report.blocks[1].diagnostics[0];
    assert_eq!(first.range.file, report.file, "{first:?}");
    assert!(md[first.range.range].starts_with("uint32 x"), "{first:?}");
    let label = &first.labels[0];
    assert_eq!(label.range.file, report.file, "{first:?}");
    assert!(md[label.range.range].starts_with("export PublicData"), "{first:?}");
}

/// A doc comment may show its own module's header; the file being checked
/// stays what `import` finds.
#[test]
fn a_module_block_named_for_the_file_being_checked_does_not_replace_it() {
    let source = "module twice { twice }\n\n//| ```kanagawa\n// module twice\n// {}\n// ```\n//\n// ```kanagawa\n// import twice\n//\n// const auto six = twice(3);\n// ```\ninline uint8 twice(uint8 x) { return x * 2; }\n";
    let dir = scratch("own-module", &[("twice.k", source)]);
    let report = checker(&dir).check_file(&dir.join("twice.k")).expect("readable");
    assert_eq!(verdicts(&report), [(3, passed("emptyfile.k")), (8, passed("emptyfile.k"))]);
}

#[test]
fn a_fix_keeps_its_message_and_gives_up_its_edits() {
    let md = "```\ninline void Foo(uint32 x)\n{\n}\ninline void main()\n{\nint32 x;\nFoo(x);\n}\n```\n";
    let dir = scratch("fix", &[("doc.md", md)]);
    let mut settings = Settings::fallback(&*dir);
    settings.warnings.enable(kg_diag::WarningKind::Conversion);
    settings.warnings_as_errors = true;
    let report = Checker::new(settings, Select::Kanagawa).check_file(&dir.join("doc.md")).expect("readable");
    let fix = &report.blocks[0].diagnostics[0].fixes[0];
    assert!(fix.message.contains("cast<uint32>"), "{fix:?}");
    assert!(fix.edits.is_empty(), "they are to the checked text: {fix:?}");
}

#[test]
fn a_doc_comment_failure_points_into_the_source() {
    let dir = scratch("doc-comment", &[("twice.k", "//| Doubles.\n//\n// ```kanagawa\n// uint8 y = twice(3) +;\n// ```\ninline uint8 twice(uint8 x) { return x * 2; }\n")]);
    let report = checker(&dir).check_file(&dir.join("twice.k")).expect("readable");
    assert_eq!(verdicts(&report), [(3, Verdict::Failed)]);
    let text = std::fs::read_to_string(dir.join("twice.k")).unwrap();
    let at = usize::from(report.blocks[0].diagnostics[0].range.range.start());
    assert_eq!(&text[at..at + 1], ";", "line 4 of the source, past `// `: {:?}", report.blocks[0].diagnostics[0]);
}
