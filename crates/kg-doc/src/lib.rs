//! API documentation from Kanagawa's doc comments — the site `kg doc`
//! writes.
//!
//! The comments are the reference's: `//|` before a declaration and `//<`
//! after it, in Pandoc's Markdown (`kanagawa/doc/sandcastle.md`), read by
//! [`kg_syntax::file_docs`] — the same reading the editor's hover shows. The
//! page structure is the reference generator's (`model`); the output is a
//! static site with no external assets: an index of modules, a page per
//! module, a stylesheet with light and dark themes, and a search over every
//! declaration (`render`).
//!
//! A name in a signature links to the declaration it resolves to — the
//! program's own resolution ([`kg_db::db::Db::definitions_at`]), so an
//! overloaded or shadowed name links where the compiler would look — when
//! that declaration is on one of the site's pages.

mod markdown;
mod model;
mod render;

use std::collections::HashMap;

use camino::{Utf8Path, Utf8PathBuf};
use kg_base::{FileId, TextRange};
use kg_db::db::Db;

pub use model::{Decl, Entry, Module, Param, Piece};

/// Where the site goes, and what it is called.
#[derive(Debug, Clone)]
pub struct Site {
    pub title: String,
    pub out: Utf8PathBuf,
}

/// What was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub modules: usize,
    /// Declarations on the pages, members included.
    pub declarations: usize,
    /// Of those, the ones with documentation.
    pub documented: usize,
    /// The site's front page.
    pub index: Utf8PathBuf,
}

/// The pages of `files`, each a module the database can parse.
pub fn modules(db: &Db, files: &[(FileId, Utf8PathBuf)]) -> Vec<Module> {
    files.iter().map(|(file, path)| model::module(db, *file, path.clone())).collect()
}

/// Writes the site for `files` into `site.out`.
pub fn write(db: &Db, files: &[(FileId, Utf8PathBuf)], site: &Site) -> std::io::Result<Written> {
    let modules = modules(db, files);
    let pages = page_names(&modules);

    // Every declaration on a page, by its name token: what a resolved name
    // links to.
    let mut targets: HashMap<(FileId, TextRange), String> = HashMap::new();
    // By spelling, for inline code in documentation: this page's first,
    // then any other page's where the spelling is documented once.
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    for (module, page) in modules.iter().zip(&pages) {
        for decl in &module.decls {
            for d in std::iter::once(decl).chain(&decl.members) {
                let href = format!("{page}#{}", d.anchor);
                targets.insert((module.file, d.name_range), href.clone());
                if std::ptr::eq(d, decl) {
                    by_name.entry(d.name.clone()).or_default().push(href);
                }
            }
        }
    }
    let resolve = |file: FileId, at: kg_base::TextSize| -> Option<String> {
        let defs = db.definitions_at(&[file], file, at);
        let def = defs.first()?;
        targets.get(&(def.range.file, def.range.range)).cloned()
    };

    std::fs::create_dir_all(&site.out)?;
    let common = common_dir(files.iter().map(|(_, p)| p.as_path()));
    let labels = labels(&modules, &common);
    let mut written = Written { modules: modules.len(), declarations: 0, documented: 0, index: site.out.join("index.html") };
    let mut search = Vec::new();
    for (module, page) in modules.iter().zip(&pages) {
        let local: HashMap<&str, String> =
            module.decls.iter().rev().map(|d| (d.name.as_str(), format!("{page}#{}", d.anchor))).collect();
        let link = |name: &str| -> Option<String> {
            local.get(name).cloned().or_else(|| by_name.get(name).filter(|h| h.len() == 1).map(|h| h[0].clone()))
        };
        let shown_path = module.path.strip_prefix(&common).map_or(module.path.as_str(), |p| p.as_str());
        let html = render::module_page(
            site,
            &modules,
            &pages,
            &labels,
            module,
            page,
            shown_path,
            &|at| resolve(module.file, at),
            &link,
        );
        std::fs::write(site.out.join(page), html)?;
        for decl in &module.decls {
            for d in std::iter::once(decl).chain(&decl.members) {
                written.declarations += 1;
                written.documented += usize::from(d.doc.is_some());
                let name = if std::ptr::eq(d, decl) { d.name.clone() } else { format!("{}.{}", decl.name, d.name) };
                search.push(serde_json::json!({
                    "n": name,
                    "k": d.kind,
                    "m": module.name,
                    "h": format!("{page}#{}", d.anchor),
                    "s": d.doc.as_deref().map(|t| markdown::summary(t, 140)).unwrap_or_default(),
                }));
            }
        }
    }
    std::fs::write(site.out.join("index.html"), render::index_page(site, &modules, &pages, &labels, &written))?;
    std::fs::write(site.out.join("style.css"), render::STYLE)?;
    std::fs::write(site.out.join("search.js"), render::SEARCH)?;
    std::fs::write(
        site.out.join("search-index.js"),
        format!("window.KG_SEARCH = {};\n", serde_json::Value::Array(search)),
    )?;
    Ok(written)
}

/// How the site names each module: its name, and where two files declare
/// the same one — the library's per-device variants, `device/mock/hardware/
/// config.k` — the directory that tells them apart, `hardware.config
/// (device/mock)`.
fn labels(modules: &[Module], common: &Utf8Path) -> Vec<String> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for m in modules {
        *count.entry(m.name.as_str()).or_insert(0) += 1;
    }
    modules
        .iter()
        .map(|m| {
            if count[m.name.as_str()] < 2 {
                return m.name.clone();
            }
            let relative = m.path.strip_prefix(common).unwrap_or(&m.path);
            let mut dirs: Vec<&str> = relative.parent().map(|p| p.iter().collect()).unwrap_or_default();
            // `a.b.c` lives at `…/a/b/c.k`: what precedes `a/b` is the difference.
            let inner: Vec<&str> = m.name.split('.').collect();
            for part in inner.iter().rev().skip(1) {
                if dirs.last() == Some(part) {
                    dirs.pop();
                }
            }
            if dirs.is_empty() { m.name.clone() } else { format!("{} ({})", m.name, dirs.join("/")) }
        })
        .collect()
}

/// Each module's page file: its name, made unique.
fn page_names(modules: &[Module]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    modules
        .iter()
        .map(|m| {
            let base: String =
                m.name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            if *n == 1 { format!("{base}.html") } else { format!("{base}-{n}.html") }
        })
        .collect()
}

/// The deepest directory every path is under, for showing each one short.
fn common_dir<'a>(mut paths: impl Iterator<Item = &'a Utf8Path>) -> Utf8PathBuf {
    let Some(first) = paths.next() else { return Utf8PathBuf::new() };
    let mut common: Utf8PathBuf = first.parent().map(Utf8Path::to_owned).unwrap_or_default();
    for path in paths {
        while !path.starts_with(&common) {
            if !common.pop() {
                return Utf8PathBuf::new();
            }
        }
    }
    common
}
