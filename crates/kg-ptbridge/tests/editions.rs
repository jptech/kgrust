//! Edition 1.1's named arguments reach the backend as the positional call
//! they name: the walk of a program that names its arguments is the walk of
//! its twin that writes them in the parameters' order (`docs/editions/1.1.md`).

use std::sync::{Arc, Mutex};

use kg_base::Utf8PathBuf;
use kg_base::language::{Edition, Features, Language};

#[derive(Clone, Default)]
struct SharedBuf(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for SharedBuf {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("no panics hold this lock").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The walk of `main` as the file of a project in `language`: its calls,
/// without sequence numbers and with every location dropped — a named
/// argument is longer than its value, so the twin's columns differ.
fn walk(language: Language, main: &str) -> Vec<String> {
    let mut settings = kg_config::Settings::fallback("/proj");
    settings.implicit_base = false;
    settings.language = kg_config::LanguageMap::new("/proj".as_ref(), language);
    let mut db = kg_db::Db::new(settings);
    let root = db.set_file_text(&Utf8PathBuf::from("/proj/main.k"), main.to_owned());
    assert_eq!(db.check(&[root]).iter().filter(|d| d.is_error()).count(), 0, "{main}");
    let buf = SharedBuf::default();
    let mut trace = kg_ptbridge::trace::Trace::new(Box::new(buf.clone()));
    let options = kg_backend_sys::options::OptionValues::harness_defaults();
    let outcome = kg_ptbridge::program::run(&db, &[root], options, None, &mut trace);
    trace.flush();
    assert!(outcome.declined.is_none(), "{:?}", outcome.declined.map(|d| d.reason));
    let text = String::from_utf8_lossy(&buf.0.lock().expect("no panics hold this lock")).into_owned();
    text.lines()
        .filter_map(|l| l.splitn(3, ' ').nth(2))
        .filter(|c| !c.starts_with("SetLocation2") && !c.starts_with("UnknownLocation"))
        .map(str::to_owned)
        .collect()
}

/// Reordered across every kind of callee the pass resolves: a method, a
/// member object's method, a template class's method (its parameters known
/// only once `Box<uint8>` is), an inline function of `auto` parameters
/// (deduced by label, `deduce::by_parameter`), a call nested in a named
/// argument, and a mix of positional and named. A named call evaluating a
/// call is left in written order — `g()` before `p` — since a parameter
/// commutes with anything. Red with the pass placing any argument in the
/// wrong slot, or leaving a label for the walk to decline.
#[test]
fn a_named_call_walks_as_its_positional_twin() {
    let named = "class Acc\n{\nprivate:\n    uint32 _total = 0;\npublic:\n    uint32 add(uint32 value, uint8 shift)\n    {\n        _total += value << shift;\n        return _total;\n    }\n}\n\ntemplate <typename T>\nclass Box\n{\npublic:\n    T scale(T x, T by)\n    {\n        return x * by;\n    }\n}\n\ninline auto pick(auto a, auto b, bool first)\n{\n    return first ? a : b;\n}\n\nclass C\n{\nprivate:\n    Acc _acc;\n    Box<uint8> _box;\n\n    uint16 sub(uint8 x, uint8 y)\n    {\n        return x - y;\n    }\n\n    uint8 g()\n    {\n        return 1;\n    }\n\npublic:\n    uint32 run(uint8 p, uint8 q)\n    {\n        uint16 a = sub(y: q, x: p);\n        uint16 b = sub(p, y: q);\n        uint32 c = _acc.add(shift: 2, value: a);\n        uint8 d = _box.scale(by: 3, x: p);\n        uint8 e = pick(first: true, b: q, a: p);\n        uint32 f = _acc.add(shift: p, value: _acc.add(value: 1, shift: 0));\n        uint16 h = sub(y: g(), x: p);\n        return a + b + c + d + e + f + h;\n    }\n}\n\nexport C;\n";
    let twin = named
        .replace("sub(y: q, x: p)", "sub(p, q)")
        .replace("sub(p, y: q)", "sub(p, q)")
        .replace("_acc.add(shift: 2, value: a)", "_acc.add(a, 2)")
        .replace("_box.scale(by: 3, x: p)", "_box.scale(p, 3)")
        .replace("pick(first: true, b: q, a: p)", "pick(p, q, true)")
        .replace("_acc.add(shift: p, value: _acc.add(value: 1, shift: 0))", "_acc.add(_acc.add(1, 0), p)")
        .replace("sub(y: g(), x: p)", "sub(p, g())");
    let labelled = |t: &str| t.match_indices(": ").any(|(i, _)| t.as_bytes()[i - 1].is_ascii_alphanumeric());
    assert!(labelled(named) && !labelled(&twin), "every named call has its twin");
    let language = Language { edition: Edition::V1_1, extra: Features::NONE };
    let ours = walk(language, named);
    assert!(ours.iter().any(|c| c.starts_with("ParseFunctionCall")), "the walk reaches the calls");
    assert_eq!(ours, walk(Language::REFERENCE, &twin));
}
