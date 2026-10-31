//! A file's language (`docs/editions/README.md`) through the public API: which
//! language the database reads a file in, what a change of it invalidates,
//! and edition 1.1's named arguments as `kg check` reports them.

use kg_base::language::{Edition, Features, Language};
use kg_base::{FileId, Utf8PathBuf};
use kg_config::{LanguageMap, Settings};
use kg_db::Db;

const NAMED: Language = Language { edition: Edition::V1_1, extra: Features::NONE };

/// Settings for the project rooted at `/proj`, written in `language`.
fn settings(language: Language) -> Settings {
    let mut settings = Settings::fallback("/proj");
    settings.implicit_base = false;
    settings.language = LanguageMap::new("/proj".as_ref(), language);
    settings
}

fn db(language: Language) -> Db {
    Db::new(settings(language))
}

fn write(db: &mut Db, text: &str) -> FileId {
    db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), text.to_owned())
}

/// Every error message `kg check` gives, and where — `line:column`, from 1.
fn errors_of(db: &Db, file: FileId, text: &str) -> Vec<String> {
    let index = kg_base::LineIndex::new(text);
    db.check(&[file])
        .into_iter()
        .filter(|d| d.is_error())
        .map(|d| {
            let at = index.line_col(d.range.range.start());
            format!("{}:{}: {}", at.line + 1, at.col + 1, d.message)
        })
        .collect()
}

const CALL: &str = "inline uint8 sub(uint8 x, uint8 y)\n{\n    return x - y;\n}\n\ninline uint8 main()\n{\n    return sub(y: 1, x: 3);\n}\n";

/// A label is a parse error in the reference (`Parser.hs:871`) and stays
/// one in a project of edition 1.0; in a project of 1.1 the same file checks
/// clean. Red with `Db::features_of` ignoring the settings' language.
#[test]
fn a_file_is_read_in_its_own_packages_language() {
    let mut reference = db(Language::REFERENCE);
    let file = write(&mut reference, CALL);
    assert!(!reference.parse(file).errors().is_empty(), "edition 1.0 has no named arguments");

    let mut named = db(NAMED);
    let file = write(&mut named, CALL);
    assert_eq!(named.parse(file).errors(), &[]);
    assert_eq!(errors_of(&named, file, CALL), Vec::<String>::new());
}

/// A manifest edited to another edition reaches a file whose text did not
/// change: its parse is remembered with the language it was parsed in.
/// Red with the parse memo keyed by the text alone.
#[test]
fn a_change_of_language_reparses_an_unchanged_file() {
    let mut db = db(Language::REFERENCE);
    let file = write(&mut db, CALL);
    assert!(!db.parse(file).errors().is_empty());
    db.set_settings(settings(NAMED));
    assert_eq!(db.parse(file).errors(), &[]);
    db.set_settings(settings(Language::REFERENCE));
    assert!(!db.parse(file).errors().is_empty());
}

/// What a call that names its arguments can get wrong, each reported at the
/// argument it is about, once — the pass that places arguments runs on every
/// round of the loop, and must not meet a call it has faulted again.
/// Arguments may be named in any order but one that would let a call's
/// effect be seen in another order than written: the backend evaluates a
/// call's arguments in the list's order (`compiler/cpp/ir.cpp:10912-10919`).
#[test]
fn a_misnamed_argument_is_reported_at_its_label() {
    let text = "class C\n{\nprivate:\n    uint32 _m;\n    uint32 g() { _m++; return _m; }\n    uint32 h() { return _m; }\n    uint16 sub(uint8 x, uint8 y)\n    {\n        return x - y;\n    }\n\npublic:\n    uint32 run(uint8 p)\n    {\n        uint16 a = sub(y: p, z: p);\n        uint16 b = sub(y: p);\n        uint16 c = sub(p, x: p);\n        uint32 d = sub(y: g(), x: h());\n        uint32 e = sub(y: _m, x: g());\n        uint32 f = sub(y: g(), x: p);\n        uint32 n = p(x: 1);\n        return a + b + c + d + e + f + n;\n    }\n}\n";
    let mut db = db(NAMED);
    let file = write(&mut db, text);
    let order = "and one of them calls a function: write them in the parameters' order, or bind one to a local first";
    assert_eq!(
        errors_of(&db, file, text),
        [
            "15:30: `sub` has no parameter named `z`".to_owned(),
            "16:20: `sub` is missing an argument for `x`".to_owned(),
            "17:27: the parameter `x` is given an argument twice".to_owned(),
            format!("18:32: in `sub`'s parameter order `x` would be evaluated before `y`, which is written first, {order}"),
            format!("19:31: in `sub`'s parameter order `x` would be evaluated before `y`, which is written first, {order}"),
            "21:20: `p` is not a function whose parameters have names".to_owned(),
        ]
    );
}

/// Where `definition_at` lands from inside the `nth` (from 0) occurrence of
/// `needle`, as `line:column` from 1.
fn lands(db: &Db, file: FileId, text: &str, needle: &str, nth: usize) -> Option<String> {
    let at = text.match_indices(needle).nth(nth).expect("the needle is in the text").0 + 1;
    let found = db.definition_at(&[file], file, kg_base::TextSize::new(at as u32))?;
    let at = kg_base::LineIndex::new(text).line_col(found.range.range.start());
    Some(format!("{}:{}", at.line + 1, at.col + 1))
}

/// A label names the callee's parameter — of a function, of a method through
/// its receiver, of a function template — and never what its spelling names
/// in the caller's scope: `y: y` is the parameter, then the local. Every
/// navigation feature asks here (references and rename resolve each
/// occurrence so), and a rename that left a label behind would break the
/// call. Red with labels resolved as the names they are spelled as.
#[test]
fn a_label_is_its_parameter() {
    let text = "inline uint8 sub(uint8 x, uint8 y)\n{\n    return x - y;\n}\n\ntemplate <typename T>\ninline T pick(bool first, T a, T b)\n{\n    return first ? a : b;\n}\n\nclass C\n{\npublic:\n    uint8 add(uint8 value, uint8 shift) { return value << shift; }\n}\n\nclass Top\n{\nprivate:\n    C _c;\n\npublic:\n    uint8 run()\n    {\n        uint8 y = 2;\n        uint8 n = sub(y: y, x: 3);\n        uint8 m = _c.add(shift: 1, value: n);\n        return pick(first: true, b: m, a: n);\n    }\n}\n";
    let mut db = db(NAMED);
    let file = write(&mut db, text);
    assert_eq!(errors_of(&db, file, text), Vec::<String>::new());
    assert_eq!(lands(&db, file, text, "y: y", 0).as_deref(), Some("1:33"), "the label is `sub`'s `y`");
    assert_eq!(lands(&db, file, text, "y, x: 3", 0).as_deref(), Some("26:15"), "the value is the local");
    assert_eq!(lands(&db, file, text, "x: 3", 0).as_deref(), Some("1:24"));
    assert_eq!(lands(&db, file, text, "shift: 1", 0).as_deref(), Some("15:34"), "a method's, through `_c`");
    assert_eq!(lands(&db, file, text, "b: m", 0).as_deref(), Some("7:34"), "a template's");
}

/// A label that names no parameter lands nowhere — not on the callee's one
/// parameter, and not on a declaration of its spelling in scope, the guesses
/// `definition_at` never makes. Red with a label's spelling unchecked against
/// the parameters'.
#[test]
fn a_label_that_names_no_parameter_lands_nowhere() {
    let text = "inline uint8 neg(uint8 x)\n{\n    return -x;\n}\n\ninline uint8 main()\n{\n    uint8 z = 2;\n    return neg(z: z);\n}\n";
    let mut db = db(NAMED);
    let file = write(&mut db, text);
    assert_eq!(lands(&db, file, text, "z: z", 0), None);
    assert_eq!(lands(&db, file, text, "z)", 0).as_deref(), Some("8:11"));
}
