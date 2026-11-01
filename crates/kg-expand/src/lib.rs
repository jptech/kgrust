//! `kg expand`: a program of a later language edition as the reference
//! program it stands for (`docs/editions/README.md`, "The tools";
//! `docs/plans/editions.md` §4.4).
//!
//! The output is the user's own text with only the edition's constructs
//! rewritten, each by the smallest edit that says the same in edition 1.0 —
//! never a print of the tree, so comments and layout stay as written:
//!
//! - a trailing comma is deleted, as the parser drops it from the list;
//! - a parameter's default is deleted, and every call that leans on it is
//!   given it, written in;
//! - a call's named arguments lose their labels and take their parameters'
//!   places, and a template's named arguments take theirs, each parameter
//!   before the last one written that no argument names given what the
//!   instance gave it.
//!
//! What a call and an instance were given is read off the loop's final
//! trees ([`placed`]) — what the backend walk reads — so the expansion is
//! what the frontend did, and `kg check` records nothing for it. Reordering
//! the text is that program: `kg_sem::named` places a call's arguments only
//! where no two it swaps could tell (`Effect::commutes`), and reports every
//! other reorder as an error, which a program `kg expand` writes has none of.
//!
//! **Fail toward silence.** What has no exact spelling is refused, naming
//! the construct, and no file of the program is written: a call its
//! template's instances place differently, a call no instance compiles, a
//! value no literal reads back as. And the expansion is proved before it is
//! given ([`verify`]): read in edition 1.0 it must parse and check clean and
//! walk into the backend call for call as the program does, or it is
//! refused too.

mod edit;
mod placed;
mod scan;
mod spell;
mod verify;

use kg_base::{FileId, FileRange, FxHashSet, TextRange, Utf8PathBuf};
use kg_db::Db;
use kg_diag::Diagnostic;
use kg_resolve::ModuleSource;

use edit::{Piece, Region};
use scan::{List, ListKind, Scan};
use spell::Spelled;

/// One file of the project, expanded.
#[derive(Debug)]
pub struct Expanded {
    pub file: FileId,
    pub path: Utf8PathBuf,
    pub text: String,
    /// Whether the text differs from the file's.
    pub changed: bool,
}

#[derive(Debug)]
pub enum Outcome {
    /// Every file of the project the program reaches, in program order.
    Expanded(Vec<Expanded>),
    /// There is no program to expand: `kg check`'s errors — or, for a
    /// program that uses nothing of a later edition and so is never checked,
    /// the syntax errors of its project's files.
    Errors(Vec<Diagnostic>),
    /// What has no exact spelling in edition 1.0, each at its construct.
    Refused(Vec<Diagnostic>),
}

/// A project file and what its text holds.
struct Source {
    file: FileId,
    path: Utf8PathBuf,
    text: String,
    scan: Scan,
}

/// The program rooted at `roots`, each of its project's files
/// (`kg_config::LanguageMap::is_project`) expanded. A file outside the
/// project — the standard library, a dependency with a manifest of its own
/// — keeps its own language and is neither expanded nor returned.
pub fn expand(db: &Db, roots: &[FileId]) -> Outcome {
    run(db, roots, false)
}

/// [`expand`] the long way round, whatever the program uses: checked, and each
/// of its lists read off the loop's trees, as a program with a default is.
/// `xtask editions` grades the reading with this over the reference's tree,
/// where every call must read back as written; a program of no edition
/// construct otherwise never comes this way.
pub fn expand_reading_every_list(db: &Db, roots: &[FileId]) -> Outcome {
    run(db, roots, true)
}

fn run(db: &Db, roots: &[FileId], every_list: bool) -> Outcome {
    let program = db.program(roots);
    let language = &db.settings().language;
    // A call of any file may lean on a default some module declares
    // (`kg_hir::facts::DEFAULTED`, `kg_sem::named`); without one, no call
    // is given anything a scan of its own text does not show.
    let defaulting: FxHashSet<String> =
        program.files.iter().flat_map(|&m| db.defaulting_functions(m).iter().cloned().collect::<Vec<_>>()).collect();
    let defaults = !defaulting.is_empty();
    let mut sources = Vec::new();
    let mut broken = false;
    for &module in &program.files {
        let ModuleSource::File(file) = module else { continue };
        let Some(path) = db.path(file) else { continue };
        if !language.is_project(&path) {
            continue;
        }
        let parse = db.parse(file);
        broken |= !parse.errors().is_empty();
        let text = parse.text();
        // A file of the reference's language holds none of a later
        // edition's syntax — it would be a parse error there.
        let scan = if parse.features().is_empty() && !defaults && !every_list {
            Scan::default()
        } else {
            scan::scan(&parse.syntax_node(), &text)
        };
        sources.push(Source { file, path, text, scan });
    }
    if broken {
        return Outcome::Errors(db.parse_stage(roots).into_iter().filter(Diagnostic::is_error).collect());
    }
    let labelled = sources.iter().any(|s| s.scan.labelled());
    let syntax = sources.iter().any(|s| !s.scan.commas.is_empty() || !s.scan.defaults.is_empty());
    if !labelled && !defaults && !syntax && !every_list {
        return Outcome::Expanded(sources.into_iter().map(|s| Expanded { file: s.file, path: s.path, text: s.text, changed: false }).collect());
    }

    let errors: Vec<Diagnostic> = db.check(roots).into_iter().filter(Diagnostic::is_error).collect();
    if !errors.is_empty() {
        return Outcome::Errors(errors);
    }
    if let Some(passes) = db.budget_exhausted(roots) {
        let at = FileRange { file: roots.first().copied().unwrap_or(FileId::SYNTHETIC), range: TextRange::default() };
        return Outcome::Refused(vec![Diagnostic::error(
            at,
            format!("cannot write a program the loop did not finish: it stopped after {passes} passes without converging"),
        )]);
    }

    let looped = (labelled || defaults || every_list).then(|| db.looped(roots));
    let copies = match &looped {
        Some(looped) => {
            let files: FxHashSet<FileId> = sources.iter().map(|s| s.file).collect();
            let paths: FxHashSet<FileRange> = sources
                .iter()
                .flat_map(|s| s.scan.lists.iter().filter(|l| l.kind == ListKind::Template && l.labelled()).filter_map(|l| l.path.map(|range| FileRange { file: s.file, range })))
                .collect();
            placed::read(looped, &files, &paths, db.interner())
        }
        None => placed::Copies::default(),
    };

    let mut refused = Vec::new();
    let mut rendered = Vec::new();
    for source in &sources {
        let at = |range: TextRange| FileRange { file: source.file, range };
        let mut regions: Vec<Region> = Vec::new();
        for &range in source.scan.commas.iter().chain(&source.scan.defaults) {
            regions.push(Region { range, list: None, pieces: Vec::new() });
        }
        for (id, list) in source.scan.lists.iter().enumerate() {
            // A list in a default goes with the default; the program has it
            // only as the value the calls were given.
            if source.scan.defaults.iter().any(|d| d.contains(list.open)) {
                continue;
            }
            let placement = match list.kind {
                ListKind::Call => call_placement(list, copies.calls.get(&at(list.range)), &defaulting, source, looped.as_deref(), db),
                ListKind::Template if list.labelled() => {
                    template_placement(list, list.path.and_then(|p| copies.instances.get(&at(p))), source, looped.as_deref(), db)
                }
                ListKind::Template => Ok(None),
            };
            match placement {
                Ok(Some(slots)) => regions.extend(rewrite(list, id, &slots, &source.text)),
                Ok(None) => {}
                Err(message) => refused.push(Diagnostic::error(at(list.range), message)),
            }
        }
        rendered.push(edit::render(&source.text, regions));
    }
    if !refused.is_empty() {
        return Outcome::Refused(refused);
    }

    let out: Vec<Expanded> = sources
        .iter()
        .zip(&rendered)
        .map(|(s, r)| Expanded { file: s.file, path: s.path.clone(), changed: r.text != s.text, text: r.text.clone() })
        .collect();
    if out.iter().any(|e| e.changed)
        && let Err(refusals) = verify::verify(db, roots, &out, &rendered)
    {
        return Outcome::Refused(refusals);
    }
    Outcome::Expanded(out)
}

/// What an argument's place in the expanded list holds.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Slot {
    /// The argument written `n`th.
    Written(usize),
    /// A value the loop gave the call or the instance.
    Given(Spelled),
}

/// A call's arguments as every copy the walk reads has them — `None` where
/// that is the list as written, which needs no edit.
fn call_placement(
    list: &List,
    copies: Option<&Vec<placed::CallCopy>>,
    defaulting: &FxHashSet<String>,
    source: &Source,
    looped: Option<&kg_db::looped::Looped>,
    db: &Db,
) -> Result<Option<Vec<Slot>>, String> {
    let Some(copies) = copies.filter(|c| !c.is_empty()) else {
        // No copy to read, and the default it may lean on is about to go.
        return if list.labelled() || defaulting.contains(callee(&list.name)) { Err(unreached(list)) } else { Ok(None) };
    };
    let mut placed: Vec<(Vec<Slot>, kg_base::Expansion)> = Vec::new();
    for copy in copies {
        let mut slots = Vec::new();
        let mut seen = vec![false; list.items.len()];
        for (n, arg) in copy.args.iter().enumerate() {
            match arg {
                placed::Arg::Added => {}
                placed::Arg::Labelled => {
                    return Err(format!("cannot write this call of `{}` in edition 1.0: the loop left its labels unplaced", list.name));
                }
                placed::Arg::Default(Ok(s)) => slots.push(Slot::Given(s.clone())),
                placed::Arg::Default(Err(why)) => {
                    return Err(format!(
                        "cannot write this call of `{}` in edition 1.0: its argument {} is the default it is given, {why}",
                        list.name,
                        n + 1
                    ));
                }
                placed::Arg::Value(head) => {
                    let Some(i) = list.items.iter().position(|item| item.full.contains_range(*head)) else {
                        return Err(cannot_read(list));
                    };
                    if std::mem::replace(&mut seen[i], true) {
                        return Err(cannot_read(list));
                    }
                    slots.push(Slot::Written(i));
                }
            }
        }
        if seen.iter().any(|s| !s) {
            return Err(cannot_read(list));
        }
        placed.push((slots, copy.via));
    }
    agree(list, placed, source, looped, db).map(|slots| {
        let as_written = !list.labelled() && slots.iter().enumerate().all(|(n, s)| *s == Slot::Written(n));
        (!as_written).then_some(slots)
    })
}

/// A template argument list's arguments as every instance named there has
/// them: each written argument at its parameter's place, and each place
/// before the last one written that none fills given the instance's
/// argument — its default, or a function template's deduction. The places
/// after are left out: the reference gives them their defaults itself, as
/// it gave the program (`resolveTemplateArgs`, `Type.hs:1406-1428`).
fn template_placement(
    list: &List,
    copies: Option<&Vec<placed::InstanceCopy>>,
    source: &Source,
    looped: Option<&kg_db::looped::Looped>,
    db: &Db,
) -> Result<Option<Vec<Slot>>, String> {
    let Some(copies) = copies.filter(|c| !c.is_empty()) else { return Err(unreached(list)) };
    let mut placed = Vec::new();
    for copy in copies {
        let interner = db.interner();
        if copy.template.segments().last().map(|s| interner.resolve(*s)).as_deref() != Some(list.name.as_str()) {
            return Err(cannot_read(list));
        }
        let names: Vec<kg_base::SmolStr> = copy.args.iter().map(|(n, _)| interner.resolve(*n)).collect();
        let mut at: Vec<Option<usize>> = vec![None; names.len()];
        for (i, item) in list.items.iter().enumerate() {
            let slot = match &item.label {
                Some(l) => names.iter().position(|n| n.as_str() == l),
                None => Some(i),
            };
            let Some(slot) = slot.filter(|&s| s < names.len() && at[s].is_none()) else { return Err(cannot_read(list)) };
            at[slot] = Some(i);
        }
        let last = at.iter().rposition(Option::is_some).map_or(0, |l| l + 1);
        let mut slots = Vec::with_capacity(last);
        for (slot, written) in at.iter().enumerate().take(last) {
            slots.push(match written {
                Some(i) => Slot::Written(*i),
                None => match spell::template_argument(&copy.args[slot].1) {
                    Ok(s) => Slot::Given(s),
                    Err(why) => {
                        return Err(format!(
                            "cannot write this template argument list of `{}` in edition 1.0, which must write its argument `{}` to reach the ones after: it is {why}",
                            list.name, names[slot]
                        ));
                    }
                },
            });
        }
        placed.push((slots, copy.via));
    }
    agree(list, placed, source, looped, db).map(Some)
}

/// The one placement every copy has, or the refusal that names two that
/// differ: one text cannot stand for both.
fn agree(
    list: &List,
    mut placed: Vec<(Vec<Slot>, kg_base::Expansion)>,
    source: &Source,
    looped: Option<&kg_db::looped::Looped>,
    db: &Db,
) -> Result<Vec<Slot>, String> {
    let (first, via) = placed.swap_remove(0);
    if let Some((other, other_via)) = placed.into_iter().find(|(s, _)| *s != first) {
        let shown = |slots: &[Slot]| {
            let args: Vec<String> = slots
                .iter()
                .map(|s| match s {
                    Slot::Written(i) => source.text[list.items[*i].value].to_owned(),
                    Slot::Given(s) => s.text.clone(),
                })
                .collect();
            let (open, close) = if list.kind == ListKind::Call { ("(", ")") } else { ("<", ">") };
            format!("{}{open}{}{close}", list.name, args.join(", "))
        };
        let instance = |via| looped.map(|l| l.instance_chain(via, db.interner())).filter(|s| !s.is_empty());
        let named = |via| instance(via).map_or_else(String::new, |i| format!(" in `{i}`"));
        return Err(format!(
            "cannot write this list in edition 1.0: the template instances that compile it give it different arguments — `{}`{} and `{}`{} — and one text cannot say both",
            shown(&first),
            named(via),
            shown(&other),
            named(other_via),
        ));
    }
    Ok(first)
}

/// The function a callee's text names: `f` of `a.b::f<3>`.
fn callee(text: &str) -> &str {
    let name = text.split('<').next().unwrap_or(text);
    name.rsplit([':', '.']).next().unwrap_or(name).trim()
}

fn unreached(list: &List) -> String {
    let what = if list.kind == ListKind::Call { "call" } else { "template argument list" };
    format!(
        "cannot write this {what} of `{}` in edition 1.0: the program compiles no copy of it — no template instance or `static if` branch it keeps holds it, or it was folded to a value — so nothing says where its arguments go",
        list.name
    )
}

fn cannot_read(list: &List) -> String {
    format!("cannot write this list of `{}` in edition 1.0: nothing says where the loop placed its arguments", list.name)
}

/// The regions that rewrite `list` as `slots` places it: each written
/// argument's place takes the argument at that slot, and the slots past the
/// written ones follow the last — on a line of their own where the list is
/// written one argument a line.
fn rewrite(list: &List, id: usize, slots: &[Slot], text: &str) -> Vec<Region> {
    let what = |text: &str| match list.kind {
        ListKind::Call => format!("`{text}`, the argument this call of `{}` is given,", list.name),
        ListKind::Template => format!("`{text}`, the argument this list of `{}` is given,", list.name),
    };
    let fill = |slot: &Slot| match slot {
        Slot::Written(i) => Piece::Source { range: list.items[*i].value, list: id },
        Slot::Given(s) => Piece::Text { text: s.text.clone(), what: what(&s.text) },
    };
    let comma = |text: String| Piece::Text { what: what("a comma"), text };
    let n = list.items.len();
    if n == 0 {
        let mut pieces = Vec::new();
        for (k, slot) in slots.iter().enumerate() {
            if k > 0 {
                pieces.push(comma(", ".to_owned()));
            }
            pieces.push(fill(slot));
        }
        return vec![Region { range: TextRange::empty(list.open), list: Some(id), pieces }];
    }
    let last = list.items[n - 1].span;
    let line_start = text[..usize::from(last.start())].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..usize::from(last.start())];
    let separator = if indent.chars().all(|c| c == ' ' || c == '\t') && line_start > usize::from(list.open) {
        let newline = if text[..line_start].ends_with("\r\n") { "\r\n" } else { "\n" };
        format!(",{newline}{indent}")
    } else {
        ", ".to_owned()
    };
    (0..n)
        .map(|k| {
            let mut pieces = vec![fill(&slots[k])];
            if k == n - 1 {
                for slot in &slots[n..] {
                    pieces.push(comma(separator.clone()));
                    pieces.push(fill(slot));
                }
            }
            Region { range: list.items[k].span, list: Some(id), pieces }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use kg_base::language::{Edition, Features, Language};
    use kg_config::{LanguageMap, Settings};

    use super::*;

    const V1_1: Language = Language { edition: Edition::V1_1, extra: Features::NONE };

    fn db(language: Language, files: &[(&str, &str)]) -> (Db, Vec<FileId>) {
        let mut settings = Settings::fallback("/proj");
        settings.implicit_base = false;
        settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
        settings.language = LanguageMap::new("/proj".as_ref(), language);
        let mut db = Db::new(settings);
        let ids = files.iter().map(|(name, text)| db.set_file_text(&Utf8PathBuf::from(format!("/proj/{name}")), (*text).to_owned())).collect();
        (db, ids)
    }

    /// The expansion of the one-file program `text`.
    fn expanded(text: &str) -> String {
        let (db, ids) = db(V1_1, &[("main.k", text)]);
        match expand(&db, &ids) {
            Outcome::Expanded(mut files) => files.remove(0).text,
            outcome => panic!("{outcome:?}"),
        }
    }

    fn refused(text: &str) -> Vec<String> {
        let (db, ids) = db(V1_1, &[("main.k", text)]);
        match expand(&db, &ids) {
            Outcome::Refused(refusals) => refusals.into_iter().map(|d| d.message).collect(),
            outcome => panic!("{outcome:?}"),
        }
    }

    const SUB: &str = "inline uint8 sub(uint8 x, uint8 y)\n{\n    return x - y;\n}\n\n";
    const MAIN: &str = "class Main\n{\npublic:\n    uint8 run(uint8 p)\n    {\n        return ";
    const END: &str = ";\n    }\n}\n\nexport Main;\n";

    fn call(call: &str) -> String {
        format!("{SUB}{MAIN}{call}{END}")
    }

    /// What uses no construct comes back byte for byte, its comments and
    /// its odd layout with it, and the long way round finds nothing either.
    /// Red with the render dropping a stretch of text between regions, or with
    /// a positional call read as moved.
    #[test]
    fn a_program_of_no_construct_is_itself() {
        let text = call("/* odd */ sub( p ,\n  1 ) /* kept */");
        assert_eq!(expanded(&text), text);
        let (db, ids) = db(V1_1, &[("main.k", &text)]);
        match expand_reading_every_list(&db, &ids) {
            Outcome::Expanded(files) => {
                assert_eq!(files[0].text, text);
                assert!(!files[0].changed);
            }
            outcome => panic!("{outcome:?}"),
        }
    }

    /// Labels go, arguments take their parameters' places, a comment beside an
    /// argument stays with the text around it, and the comment after a label
    /// moves with its value. Red with an item's place taking its own value.
    #[test]
    fn a_named_argument_takes_its_parameters_place() {
        assert_eq!(expanded(&call("sub(y: 1, x: p)")), call("sub(p, 1)"));
        assert_eq!(expanded(&call("sub(y: /* one */ 1, /* mid */ x: p,)")), call("sub(p, /* mid */ /* one */ 1)"));
    }

    /// An argument that is itself a call moves with its own edits.
    /// Red with a moved argument copied without them.
    #[test]
    fn an_argument_moves_with_its_own_edits() {
        assert_eq!(expanded(&call("sub(y: sub(y: 1, x: 2,), x: p)")), call("sub(p, sub(2, 1))"));
    }

    const BUMP: &str = "inline uint8 bump(uint8 x = 1, uint8 by = 2)\n{\n    return x + by;\n}\n\n";
    const BUMPED: &str = "inline uint8 bump(uint8 x, uint8 by)\n{\n    return x + by;\n}\n\n";

    /// A default is written into each call that leaves it out, into an empty
    /// list too, and leaves its parameter. One past a list written a line an
    /// argument goes on a line of its own. Red with the empty list's region
    /// dropped, or with the separator always `, `.
    #[test]
    fn a_default_is_written_where_it_was_left_out() {
        let program = |decl: &str, call: &str| format!("{decl}{MAIN}{call}{END}");
        assert_eq!(expanded(&program(BUMP, "bump()")), program(BUMPED, "bump(1, 2)"));
        assert_eq!(expanded(&program(BUMP, "bump(p)")), program(BUMPED, "bump(p, 2)"));
        assert_eq!(expanded(&program(BUMP, "bump(by: p)")), program(BUMPED, "bump(1, p)"));
        assert_eq!(
            expanded(&program(BUMP, "bump(\n            p,\n        )")),
            program(BUMPED, "bump(\n            p,\n            2\n        )")
        );
    }

    /// A default's value keeps the type the loop folded it to: `Step + 1` of a
    /// `uint8` is a `uint9`, which a bare `3` is not. Red with the suffix dropped
    /// — the proof then refuses, for the walks part.
    #[test]
    fn a_default_keeps_its_folded_type() {
        let program = |decl: &str, call: &str| format!("const uint8 Step = 2;\n{decl}{MAIN}{call}{END}");
        let decl = |default: &str| format!("inline uint32 grow(uint32 x{default})\n{{\n    return x;\n}}\n\n");
        assert_eq!(expanded(&program(&decl(", uint32 by = Step + 1"), "grow(p)")), program(&decl(", uint32 by"), "grow(p, 3u9)"));
    }

    /// A template's arguments take their parameters' places, what comes before
    /// the last one written is written as the instance has it, and what comes
    /// after is left to the reference's own defaults. Red with every parameter
    /// written, which the walks cannot tell.
    #[test]
    fn a_template_argument_takes_its_parameters_place() {
        let template = "template <auto A = 1, auto B = 2, auto C = 3>\nclass Sized\n{\n}\n\n";
        let program = |list: &str| format!("{template}class Main\n{{\n    Sized<{list}> _s;\n\npublic:\n    uint8 run(uint8 p)\n    {{\n        return p;\n    }}\n}}\n\nexport Main;\n");
        assert_eq!(expanded(&program("B: 5")), program("1, 5"));
        assert_eq!(expanded(&program("C: 5, A: 9,")), program("9, 2, 5"));
        assert_eq!(expanded(&program("7, C: 5")), program("7, 2, 5"));
    }

    /// The body of an abbreviated template is compiled only as its instances'
    /// copies, and is read from those; the function it was made from keeps its
    /// labels, and is never walked. Red with that function read too.
    #[test]
    fn an_abbreviated_templates_call_is_read_from_its_instance() {
        let program = |call: &str| format!("{SUB}inline auto twice(auto x)\n{{\n    return {call};\n}}\n\n{MAIN}twice(p){END}");
        assert_eq!(expanded(&program("sub(y: x, x: 1)")), program("sub(1, x)"));
    }

    /// A call in one file is given the default another file declares, and both
    /// are written. Red with the call's list left as written where the default's
    /// is in another file.
    #[test]
    fn a_file_with_no_construct_is_given_the_defaults_it_leans_on() {
        let lib = "module lib { bump }\n\ninline uint8 bump(uint8 x = 1)\n{\n    return x;\n}\n";
        let main = format!("import lib\n\n{MAIN}bump(){END}");
        let (db, ids) = db(V1_1, &[("lib.k", lib), ("main.k", &main)]);
        let Outcome::Expanded(files) = expand(&db, &ids[1..]) else { panic!("refused") };
        let text = |name: &str| files.iter().find(|f| f.path.as_str().ends_with(name)).map(|f| f.text.as_str());
        assert_eq!(text("lib.k"), Some("module lib { bump }\n\ninline uint8 bump(uint8 x)\n{\n    return x;\n}\n"));
        assert_eq!(text("main.k"), Some(format!("import lib\n\n{MAIN}bump(1){END}").as_str()));
    }

    /// What does not check is no program: its errors, and no text. A reorder two
    /// arguments could tell apart is one such (`kg_sem::named`). Red with
    /// the check's errors not looked at.
    #[test]
    fn an_impure_reorder_is_the_checks_error() {
        let impure = "inline uint8 sub(uint8 x, uint8 y)\n{\n    return x - y;\n}\n\nclass Main\n{\n    uint8 _n = 0;\n\n    uint8 next()\n    {\n        _n++;\n        return _n;\n    }\n\npublic:\n    uint8 run()\n    {\n        return sub(y: next(), x: next());\n    }\n}\n\nexport Main;\n";
        let (db, ids) = db(V1_1, &[("main.k", impure)]);
        let Outcome::Errors(errors) = expand(&db, &ids) else { panic!("expanded") };
        let checked: Vec<Diagnostic> = db.check(&ids).into_iter().filter(Diagnostic::is_error).collect();
        assert_eq!(errors.iter().map(|d| &d.message).collect::<Vec<_>>(), checked.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(!errors.is_empty());
    }

    /// A syntax error is reported though nothing would be rewritten, as `kg fmt`
    /// leaves alone what does not parse. Red with the fast path taken first.
    #[test]
    fn what_does_not_parse_is_no_program() {
        let (db, ids) = db(V1_1, &[("main.k", "inline void main(\n{\n")]);
        let Outcome::Errors(errors) = expand(&db, &ids) else { panic!("expanded") };
        assert!(!errors.is_empty());
    }

    /// A call in a template no one instantiates has no copy to read; a call
    /// that needs no edit is left alone in the same place. Red with the
    /// refusal gone: the label would stay in text that claims edition 1.0.
    #[test]
    fn a_call_no_instance_compiles_is_refused() {
        let unused = |call: &str, main: &str| format!("{SUB}template <typename T>\nclass Unused\n{{\npublic:\n    uint8 run(uint8 p)\n    {{\n        return {call};\n    }}\n}}\n\n{MAIN}{main}{END}");
        assert_eq!(refused(&unused("sub(y: 1, x: p)", "sub(y: 1, x: p)")).len(), 1);
        assert_eq!(expanded(&unused("sub(p, 1)", "sub(y: 1, x: p)")), unused("sub(p, 1)", "sub(p, 1)"));
    }

    /// The proof reads the text back: a text that checks clean but says another
    /// program is refused at the walks, and it reads it in edition 1.0. Red with
    /// the walks not compared, or with the text read in the project's language.
    #[test]
    fn the_proof_refuses_another_program() {
        let text = call("sub(y: 1, x: p)");
        let (db, ids) = db(V1_1, &[("main.k", &text)]);
        let proved = |text: String| {
            let rendered = vec![edit::render(&text, Vec::new())];
            let out = vec![Expanded { file: ids[0], path: Utf8PathBuf::from("/proj/main.k"), text, changed: true }];
            verify::verify(&db, &ids, &out, &rendered)
        };
        let refusals = proved(call("sub(p, 2)")).unwrap_err();
        assert!(refusals[0].message.contains("the walks into the backend show"), "{}", refusals[0].message);
        assert!(proved(call("sub(p, 1)")).is_ok());
        // Read in edition 1.0: the same program, in 1.1's syntax, is no proof.
        assert!(proved(call("sub(p, 1,)")).is_err());
    }

    /// A callee's text names its function by its last name, before any template
    /// argument. Red with the path kept: a call of `a::f` would not be
    /// known to lean on `f`'s default.
    #[test]
    fn a_callee_is_its_last_name() {
        assert_eq!(callee("f"), "f");
        assert_eq!(callee("a.b::f<3>"), "f");
        assert_eq!(callee("self.f"), "f");
    }
}
