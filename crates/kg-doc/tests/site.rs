//! The page `kg doc` makes of a module, and the site around it.

use kg_base::Utf8PathBuf;
use kg_config::Settings;
use kg_db::Db;
use kg_doc::{Entry, Piece};

const THINGS: &str = "/*|
Operations on things.
*/
module data.things
    { //| # Making
      make
    , Thing
    , Color
    }

//| Make a thing.
template <typename T //< Element type
         , auto N    //< Count
         >
inline T[N] make(T x //< The value
                ) { T[N] r; return r; }
//< After.

//| A thing, see `make`.
class Thing
{
public:
    //| Count it.
    uint8 count() { return 0; }
private:
    uint8 _hidden;
    //| Documented, so shown.
    uint8 _shown;
}

enum Color : uint2
{
    //| The red one
    RED,
    GREEN
}

inline void unexposed() {}
";

fn db() -> (Db, Vec<(kg_base::FileId, Utf8PathBuf)>) {
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    let mut db = Db::new(settings);
    let path = Utf8PathBuf::from("/proj/data/things.k");
    let file = db.set_file_text(&path, THINGS.to_owned());
    (db, vec![(file, path)])
}

fn text(pieces: &[Piece]) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) | Piece::Name { text: t, .. } => t.as_str(),
        })
        .collect()
}

#[test]
fn a_module_page_reads_the_reference_structure() {
    let (db, files) = db();
    let module = &kg_doc::modules(&db, &files)[0];
    assert_eq!(module.name, "data.things");
    assert_eq!(module.doc.as_deref(), Some("Operations on things."));
    assert_eq!(
        module.index,
        [
            Entry::Section("# Making".to_owned()),
            Entry::Name("make".to_owned()),
            Entry::Name("Thing".to_owned()),
            Entry::Name("Color".to_owned())
        ]
    );
    let names: Vec<&str> = module.decls.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["make", "Thing", "Color"], "only what the module exposes, in source order");

    let make = &module.decls[0];
    assert_eq!(make.kind, "function");
    assert_eq!(text(&make.signature), "template <typename T, auto N> inline T[N] make(T x)");
    assert_eq!(make.doc.as_deref(), Some("Make a thing.\n\nAfter."));
    let params: Vec<(&str, &str)> = make.params.iter().map(|p| (p.name.as_str(), p.doc.as_str())).collect();
    assert_eq!(params, [("T", "Element type"), ("N", "Count"), ("x", "The value")]);

    let thing = &module.decls[1];
    let members: Vec<&str> = thing.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["count", "_shown"], "public, or private and documented");
    assert_eq!(thing.members[0].anchor, "Thing.count");

    let color = &module.decls[2];
    let constants: Vec<(&str, Option<&str>)> =
        color.members.iter().map(|m| (m.name.as_str(), m.doc.as_deref())).collect();
    assert_eq!(constants, [("RED", Some("The red one")), ("GREEN", None)]);
}

#[test]
fn the_site_is_written_and_links_what_it_documents() {
    let (db, files) = db();
    let out = std::env::temp_dir().join(format!("kg-doc-test-{}", std::process::id()));
    let out = Utf8PathBuf::from_path_buf(out).expect("UTF-8 temp dir");
    let site = kg_doc::Site { title: "Things".to_owned(), out: out.clone() };
    let written = kg_doc::write(&db, &files, &site).expect("the site is written");
    assert_eq!((written.modules, written.declarations, written.documented), (1, 7, 5));
    for file in ["index.html", "data.things.html", "style.css", "search.js", "search-index.js"] {
        assert!(out.join(file).is_file(), "{file} is written");
    }
    let page = std::fs::read_to_string(out.join("data.things.html")).unwrap();
    assert!(page.contains("<h3 class=\"section\">Making</h3>"), "the export list's section");
    assert!(page.contains("<a href=\"data.things.html#make\"><code>make</code></a>"), "inline code links its declaration");
    let index = std::fs::read_to_string(out.join("search-index.js")).unwrap();
    assert!(index.contains("\"n\":\"Thing.count\""), "members are searchable: {index}");
    let _ = std::fs::remove_dir_all(&out);
}

/// Edition 1.1's default arguments are part of the signature the page
/// shows, as written, a name in one linked as any name in a signature is.
/// Red with a parameter's default left out of the signature.
#[test]
fn a_default_argument_is_shown_in_the_signature() {
    use kg_base::language::{Edition, Features, Language};
    let mut settings = Settings::fallback("/proj");
    settings.import_dirs = vec![Utf8PathBuf::from("/proj")];
    settings.implicit_base = false;
    settings.language =
        kg_config::LanguageMap::new("/proj".as_ref(), Language { edition: Edition::V1_1, extra: Features::NONE });
    let mut db = Db::new(settings);
    let path = Utf8PathBuf::from("/proj/data/scaled.k");
    let source = "module data.scaled\n    { Mode\n    , scale\n    }\n\nenum Mode : uint2\n{\n    Idle,\n    Run\n}\n\ninline uint8 scale(uint8 value, uint8 shift = 1, Mode mode = Mode::Run) { return value << shift; }\n";
    let file = db.set_file_text(&path, source.to_owned());
    let module = &kg_doc::modules(&db, &[(file, path)])[0];
    let scale = module.decls.iter().find(|d| d.name == "scale").expect("exposed");
    assert_eq!(text(&scale.signature), "inline uint8 scale(uint8 value, uint8 shift = 1, Mode mode = Mode::Run)");
    let linked = scale.signature.iter().filter(|p| matches!(p, Piece::Name { text, .. } if text == "Mode")).count();
    assert_eq!(linked, 2, "the parameter's type and its default's enum");
}
