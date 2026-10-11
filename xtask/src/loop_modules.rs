//! `cargo xtask loop-modules A B`: is a library module's elaborated state
//! independent of the root that asked for it, the copies aside? (sprint
//! W7.5c — a measurement, not a gate.)
//!
//! Each root is elaborated under the recorded compile's settings. For every
//! module both programs hold, its final top-level items are split into what
//! the module wrote — items whose `Src::via` is `Expansion::ROOT` — and the
//! copies instantiation made there. The written part is compared, structure
//! and every node's type in pre-order; the copies by instance, and, for a
//! copy both roots asked for, by the round that decided it
//! (`ExpansionData::round`).
//!
//! If the written parts agree and the shared copies are dated alike, a
//! memo per module could be exact as `ir-elaboration.md` §3.2 designs it:
//! a library module's state is its own plus the copies roots ask of it. If
//! they do not, what an edit to the root can carry across is the whole
//! program, not a module.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use kg_base::{Expansion, InstanceId};
use kg_db::{Db, ModuleSource};

/// One module as one root's loop left it.
struct Seen {
    written: String,
    /// Each copy's instance, and the round that decided it.
    copies: BTreeMap<InstanceId, u32>,
}

pub fn run(kanagawa_root: &Path, roots: &[String]) -> Result<()> {
    let kanagawa_root = &kanagawa_root.canonicalize()?;
    if roots.len() != 2 {
        anyhow::bail!("name two roots, relative to the reference checkout");
    }
    let programs = crate::ptrace::programs(kanagawa_root)?;
    let mut sides: Vec<BTreeMap<String, Seen>> = Vec::new();
    for r in roots {
        let Some(p) = programs.iter().find(|p| p.name == format!("file::{r}")) else {
            anyhow::bail!("no corpus program file::{r}");
        };
        let mut db = Db::new(crate::conformance::settings_for(kanagawa_root, &p.options));
        let path = camino::Utf8PathBuf::from_path_buf(kanagawa_root.join(r)).expect("UTF-8 paths");
        let text = std::fs::read_to_string(&path)?;
        let root = db.set_file_text(&path, text);
        sides.push(modules(&db, root));
    }
    let (a, b) = (&sides[0], &sides[1]);
    let (mut same, mut differ, mut shared_copies, mut dated_apart) = (0usize, 0usize, 0usize, 0usize);
    println!("loop-modules: {} against {}", roots[0], roots[1]);
    for (name, x) in a {
        let Some(y) = b.get(name) else { continue };
        let written_same = x.written == y.written;
        let shared: Vec<&InstanceId> = x.copies.keys().filter(|k| y.copies.contains_key(*k)).collect();
        let apart = shared.iter().filter(|k| x.copies[**k] != y.copies[**k]).count();
        shared_copies += shared.len();
        dated_apart += apart;
        if written_same {
            same += 1;
        } else {
            differ += 1;
        }
        if !written_same || apart > 0 || x.copies.len() != shared.len() || y.copies.len() != shared.len() {
            let first = if written_same { String::new() } else { format!("; written part differs — {}", first_difference(&x.written, &y.written)) };
            println!(
                "  {name}: copies {} / {} / shared {} (dated apart {apart}){first}",
                x.copies.len(),
                y.copies.len(),
                shared.len()
            );
        }
    }
    println!(
        "modules in both: {} — written part identical in {same}, different in {differ}; copies both asked for {shared_copies}, dated apart {dated_apart}",
        same + differ
    );
    Ok(())
}

fn modules(db: &Db, root: kg_base::FileId) -> BTreeMap<String, Seen> {
    let looped = db.looped(&[root]);
    let interner = db.interner();
    let mut out = BTreeMap::new();
    for (&module, m) in looped.modules.iter().zip(&looped.out.modules) {
        let ModuleSource::File(f) = module else { continue };
        let Some(path) = db.path(f) else { continue };
        let mut written = String::new();
        let mut copies = BTreeMap::new();
        for item in m.ir.top_level_items() {
            let via = m.ir.arena[item].src.via;
            if via == Expansion::ROOT {
                written.push_str(&kg_hir::structure::render_subtree(&m.ir, interner, item));
                let mut stack = vec![item];
                let mut kids = Vec::new();
                while let Some(id) = stack.pop() {
                    written.push_str(&if m.types.is_set(id) { format!("{:?}\n", m.types.get(id)) } else { "-\n".to_owned() });
                    kids.clear();
                    m.ir.arena[id].exp.children(&mut kids);
                    stack.extend(kids.iter().rev().copied());
                }
            } else if let Some(frame) = looped.out.expansions.frame(via) {
                copies.insert(frame.instance, frame.round);
            }
        }
        // Two `Db`s intern in different orders: types are compared spelled.
        let written = crate::edit_identity::spell_symbols(&written, db);
        out.insert(path.to_string(), Seen { written, copies });
    }
    out
}

fn first_difference(a: &str, b: &str) -> String {
    for (n, (x, y)) in a.lines().zip(b.lines()).enumerate() {
        if x != y {
            let cut = |s: &str| s.chars().take(120).collect::<String>();
            return format!("line {}: `{}` against `{}`", n + 1, cut(x), cut(y));
        }
    }
    format!("{} lines against {}", a.lines().count(), b.lines().count())
}
