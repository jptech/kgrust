//! The site's HTML, stylesheet and search.

use kg_base::TextSize;

use crate::markdown::{escape, summary, to_html};
use crate::model::{Decl, Entry, Module, Piece};
use crate::{Site, Written};

/// The module page.
#[allow(clippy::too_many_arguments)]
pub fn module_page(
    site: &Site,
    modules: &[Module],
    pages: &[String],
    labels: &[String],
    module: &Module,
    page: &str,
    shown_path: &str,
    resolve: &dyn Fn(TextSize) -> Option<String>,
    link: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut main = String::new();
    main.push_str("<p class=\"eyebrow\">module</p>\n");
    main.push_str(&format!("<h1 class=\"title\">{}</h1>\n", escape(&module.name)));
    main.push_str(&format!("<p class=\"path\">{}</p>\n", escape(shown_path)));
    if let Some(doc) = &module.doc {
        main.push_str(&format!("<div class=\"doc lead\">{}</div>\n", to_html(doc, link)));
    }

    if !module.decls.is_empty() {
        main.push_str("<section class=\"contents\">\n<h2>Contents</h2>\n");
        main.push_str(&contents(module, modules, pages));
        main.push_str("</section>\n");

        main.push_str("<section class=\"declarations\">\n");
        for decl in &module.decls {
            main.push_str(&declaration(decl, resolve, link));
        }
        main.push_str("</section>\n");
    } else if module.doc.is_none() {
        main.push_str("<p class=\"empty\">Nothing exposed or documented here.</p>\n");
    }
    frame(site, &module.name, pages, labels, Some(page), &main)
}

/// The front page: every module, grouped by the first part of its name.
pub fn index_page(site: &Site, modules: &[Module], pages: &[String], labels: &[String], written: &Written) -> String {
    let mut main = String::new();
    main.push_str(&format!("<h1 class=\"title\">{}</h1>\n", escape(&site.title)));
    main.push_str(&format!(
        "<p class=\"lede\">{} modules · {} declarations · {} documented</p>\n",
        written.modules, written.declarations, written.documented
    ));
    let mut order: Vec<usize> = (0..modules.len()).collect();
    order.sort_by(|&a, &b| labels[a].cmp(&labels[b]));
    let mut group: Option<&str> = None;
    for i in order {
        let module = &modules[i];
        let head = module.name.split('.').next().unwrap_or("");
        if group != Some(head) {
            if group.is_some() {
                main.push_str("</ul>\n");
            }
            main.push_str(&format!("<h2 class=\"group\">{}</h2>\n<ul class=\"modules\">\n", escape(head)));
            group = Some(head);
        }
        let line = module.doc.as_deref().map(|d| summary(d, 160)).unwrap_or_default();
        main.push_str(&format!(
            "<li><a href=\"{}\">{}</a><span>{}</span></li>\n",
            escape(&pages[i]),
            escape(&labels[i]),
            escape(&line)
        ));
    }
    if group.is_some() {
        main.push_str("</ul>\n");
    }
    frame(site, &site.title, pages, labels, None, &main)
}

/// The index of what the module exposes: the export list's order and
/// sections where it has them, the declarations' otherwise.
fn contents(module: &Module, modules: &[Module], pages: &[String]) -> String {
    let find = |name: &str| module.decls.iter().find(|d| d.name == name);
    let item = |d: &Decl| {
        format!(
            "<li><a href=\"#{}\">{}</a><span class=\"kind\">{}</span><span class=\"gist\">{}</span></li>\n",
            escape(&d.anchor),
            escape(&d.name),
            d.kind,
            escape(&d.doc.as_deref().map(|t| summary(t, 110)).unwrap_or_default())
        )
    };
    let mut out = String::from("<ul class=\"index\">\n");
    if module.index.is_empty() {
        for d in &module.decls {
            out.push_str(&item(d));
        }
    } else {
        for entry in &module.index {
            match entry {
                Entry::Section(title) => {
                    let title = title.trim_start_matches('#').trim();
                    out.push_str(&format!("</ul>\n<h3 class=\"section\">{}</h3>\n<ul class=\"index\">\n", escape(title)));
                }
                Entry::Name(name) => {
                    if let Some(d) = find(name) {
                        out.push_str(&item(d));
                    }
                }
                Entry::Module(name) => {
                    let target = modules.iter().position(|m| &m.name == name);
                    let shown = match target {
                        Some(i) => format!("<a href=\"{}\">{}</a>", escape(&pages[i]), escape(name)),
                        None => escape(name),
                    };
                    out.push_str(&format!("<li>{shown}<span class=\"kind\">module</span></li>\n"));
                }
            }
        }
    }
    out.push_str("</ul>\n");
    out.replace("<ul class=\"index\">\n</ul>\n", "")
}

fn declaration(decl: &Decl, resolve: &dyn Fn(TextSize) -> Option<String>, link: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = format!("<article class=\"decl\" id=\"{}\">\n", escape(&decl.anchor));
    out.push_str(&format!(
        "<h3><a class=\"anchor\" href=\"#{a}\">{n}</a><span class=\"kind\">{k}</span></h3>\n",
        a = escape(&decl.anchor),
        n = escape(&decl.name),
        k = decl.kind
    ));
    out.push_str(&format!("<pre class=\"signature\"><code>{}</code></pre>\n", signature(&decl.signature, resolve)));
    if let Some(doc) = &decl.doc {
        out.push_str(&format!("<div class=\"doc\">{}</div>\n", to_html(doc, link)));
    }
    if !decl.params.is_empty() {
        out.push_str("<dl class=\"params\">\n");
        for p in &decl.params {
            out.push_str(&format!("<dt><code>{}</code></dt><dd>{}</dd>\n", escape(&p.name), to_html(&p.doc, link)));
        }
        out.push_str("</dl>\n");
    }
    if !decl.members.is_empty() {
        out.push_str("<div class=\"members\">\n");
        for m in &decl.members {
            out.push_str(&format!("<div class=\"member\" id=\"{}\">\n", escape(&m.anchor)));
            out.push_str(&format!(
                "<pre class=\"signature\"><code>{}</code></pre><span class=\"kind\">{}</span>\n",
                signature(&m.signature, resolve),
                m.kind
            ));
            if let Some(doc) = &m.doc {
                out.push_str(&format!("<div class=\"doc\">{}</div>\n", to_html(doc, link)));
            }
            out.push_str("</div>\n");
        }
        out.push_str("</div>\n");
    }
    out.push_str("</article>\n");
    out
}

fn signature(pieces: &[Piece], resolve: &dyn Fn(TextSize) -> Option<String>) -> String {
    pieces
        .iter()
        .map(|p| match p {
            Piece::Text(t) => escape(t),
            Piece::Name { text, at } => match resolve(*at) {
                Some(href) => format!("<a href=\"{}\">{}</a>", escape(&href), escape(text)),
                None => escape(text),
            },
        })
        .collect()
}

/// The page around `main`: the bar with the search, the module list.
fn frame(site: &Site, title: &str, pages: &[String], labels: &[String], current: Option<&str>, main: &str) -> String {
    let mut nav = String::new();
    let mut order: Vec<usize> = (0..pages.len()).collect();
    order.sort_by(|&a, &b| labels[a].cmp(&labels[b]));
    for i in order {
        let here = current == Some(pages[i].as_str());
        nav.push_str(&format!(
            "<li><a href=\"{}\" title=\"{l}\"{}>{l}</a></li>\n",
            escape(&pages[i]),
            if here { " aria-current=\"page\"" } else { "" },
            l = escape(&labels[i])
        ));
    }
    let page_title =
        if title == site.title { escape(title) } else { format!("{} — {}", escape(title), escape(&site.title)) };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="kg doc">
<title>{page_title}</title>
<link rel="stylesheet" href="style.css">
<script defer src="search-index.js"></script>
<script defer src="search.js"></script>
</head>
<body>
<header class="bar">
  <a class="brand" href="index.html">{site}</a>
  <div class="search">
    <input id="search" type="search" placeholder="Search  /" aria-label="Search declarations" autocomplete="off" spellcheck="false">
    <ol id="results" hidden></ol>
  </div>
</header>
<div class="layout">
<nav class="sidebar" aria-label="Modules">
<h2>Modules</h2>
<ul>
{nav}</ul>
</nav>
<main>
{main}</main>
</div>
</body>
</html>
"#,
        site = escape(&site.title),
    )
}

pub const STYLE: &str = r#":root {
  --bg: #fbfaf7;
  --fg: #1c1e22;
  --muted: #646b75;
  --faint: #8d939c;
  --accent: #2f5bd3;
  --rule: #e7e4dd;
  --panel: #f3f1ec;
  --code: #f1efe9;
  --kind: #6a4fd8;
  --sans: -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, system-ui, sans-serif;
  --mono: ui-monospace, "SF Mono", SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace;
  color-scheme: light dark;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #15171b;
    --fg: #e3e5e9;
    --muted: #9ba2ac;
    --faint: #737a85;
    --accent: #85a8ff;
    --rule: #2a2e35;
    --panel: #1b1e23;
    --code: #1f2228;
    --kind: #b8a6ff;
  }
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.6 var(--sans); }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
code, pre { font-family: var(--mono); font-size: 0.875em; }
:not(pre) > code { background: var(--code); padding: 0.1em 0.35em; border-radius: 4px; }
pre { background: var(--code); padding: 0.8em 1em; border-radius: 8px; overflow-x: auto; line-height: 1.5; }

.bar { position: sticky; top: 0; z-index: 2; display: flex; align-items: center; gap: 1.5rem;
       padding: 0.7rem 1.5rem; background: color-mix(in srgb, var(--bg) 88%, transparent);
       backdrop-filter: blur(8px); border-bottom: 1px solid var(--rule); }
.brand { font-weight: 650; color: var(--fg); letter-spacing: -0.01em; }
.search { position: relative; margin-left: auto; width: min(26rem, 55vw); }
#search { width: 100%; padding: 0.45rem 0.8rem; border: 1px solid var(--rule); border-radius: 8px;
          background: var(--panel); color: var(--fg); font: inherit; font-size: 0.9rem; }
#search:focus { outline: 2px solid color-mix(in srgb, var(--accent) 45%, transparent); border-color: transparent; }
#results { position: absolute; right: 0; left: 0; top: calc(100% + 6px); margin: 0; padding: 0.3rem; list-style: none;
           max-height: 70vh; overflow-y: auto; background: var(--bg); border: 1px solid var(--rule);
           border-radius: 10px; box-shadow: 0 12px 32px rgb(0 0 0 / 0.14); }
#results li a { display: block; padding: 0.45rem 0.6rem; border-radius: 6px; color: var(--fg); }
#results li a:hover, #results li a[aria-selected="true"] { background: var(--panel); text-decoration: none; }
#results .name { font-family: var(--mono); font-size: 0.9rem; }
#results .where { color: var(--faint); font-size: 0.8rem; margin-left: 0.5rem; }
#results .gist { display: block; color: var(--muted); font-size: 0.82rem; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

.layout { display: grid; grid-template-columns: 15rem minmax(0, 1fr); max-width: 76rem; margin: 0 auto; }
.sidebar { position: sticky; top: 3.4rem; align-self: start; max-height: calc(100vh - 3.4rem); overflow-y: auto;
           padding: 1.5rem 1rem 2rem 1.5rem; font-size: 0.875rem; }
.sidebar h2 { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.08em; color: var(--faint); margin: 0 0 0.6rem; }
.sidebar ul { list-style: none; margin: 0; padding: 0; }
.sidebar li a { display: block; padding: 0.15rem 0.5rem; border-radius: 5px; color: var(--muted); font-family: var(--mono); font-size: 0.8rem;
                 white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.sidebar li a[aria-current="page"] { background: var(--panel); color: var(--fg); font-weight: 600; }
main { padding: 2rem 2.5rem 5rem; min-width: 0; }

.eyebrow { margin: 0; color: var(--faint); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.1em; }
.title { margin: 0.1rem 0 0.2rem; font-size: 2.1rem; letter-spacing: -0.02em; font-family: var(--mono); font-weight: 600; }
.path, .lede { color: var(--muted); margin: 0 0 1.5rem; font-size: 0.9rem; }
.doc.lead { font-size: 1.05rem; }
h2 { font-size: 1.15rem; margin: 2.5rem 0 0.8rem; padding-bottom: 0.3rem; border-bottom: 1px solid var(--rule); }
h2.group { font-family: var(--mono); font-size: 1rem; }

ul.modules, ul.index { list-style: none; padding: 0; margin: 0 0 1rem; }
ul.modules li, ul.index li { display: grid; grid-template-columns: minmax(9rem, max-content) auto 1fr; gap: 0.8rem; align-items: baseline;
                             padding: 0.3rem 0; border-bottom: 1px dashed var(--rule); }
ul.modules li { grid-template-columns: minmax(12rem, max-content) 1fr; }
ul.modules li a, ul.index li a { font-family: var(--mono); font-size: 0.9rem; }
ul.modules li span, .gist { color: var(--muted); font-size: 0.9rem; overflow: hidden; text-overflow: ellipsis; }
h3.section { font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.08em; color: var(--faint); margin: 1.4rem 0 0.3rem; }
.kind { color: var(--kind); font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.06em; }

.decl { margin: 0 0 2.2rem; padding-top: 0.6rem; scroll-margin-top: 4rem; }
.decl > h3 { display: flex; align-items: baseline; gap: 0.7rem; margin: 0 0 0.5rem; font-family: var(--mono); font-size: 1.1rem; }
.decl > h3 a.anchor { color: var(--fg); }
.signature { margin: 0 0 0.8rem; white-space: pre-wrap; word-break: break-word; border-left: 3px solid var(--accent); }
.signature a { color: var(--accent); }
.doc > :first-child { margin-top: 0; }
.doc h3, .doc h4, .doc h5, .doc h6 { font-size: 1rem; margin: 1.2rem 0 0.4rem; }
dl.params { display: grid; grid-template-columns: max-content 1fr; gap: 0.2rem 1rem; margin: 0.8rem 0; font-size: 0.95rem; }
dl.params dt { margin: 0; } dl.params dd { margin: 0; color: var(--muted); } dl.params dd p { margin: 0; }
.members { margin: 1rem 0 0; border-top: 1px solid var(--rule); }
.member { display: flex; flex-wrap: wrap; align-items: baseline; gap: 0.2rem 0.8rem; padding: 0.55rem 0 0.55rem 0.9rem;
          border-bottom: 1px solid var(--rule); scroll-margin-top: 4rem; }
.member .signature { margin: 0; padding: 0; background: none; border: 0; font-size: 0.85rem; }
.member .doc { flex-basis: 100%; color: var(--muted); font-size: 0.95rem; }
.member .doc > :last-child { margin-bottom: 0; }
details.example { margin: 0.8rem 0; border: 1px solid var(--rule); border-radius: 8px; padding: 0.2rem 0.9rem; }
details.example summary { cursor: pointer; font-weight: 600; font-size: 0.9rem; padding: 0.3rem 0; color: var(--muted); }
details.example[open] summary { margin-bottom: 0.4rem; }
code.language-svgbob { line-height: 1.2; }
.empty { color: var(--muted); }

@media (max-width: 52rem) {
  .layout { grid-template-columns: 1fr; }
  .sidebar { display: none; }
  main { padding: 1.5rem 1.2rem 4rem; }
  ul.index li { grid-template-columns: 1fr auto; }
  ul.index li .gist { grid-column: 1 / -1; }
}
"#;

pub const SEARCH: &str = r#"// Search over every declaration on the site: window.KG_SEARCH, written
// beside this file by `kg doc`.
(function () {
  const input = document.getElementById("search");
  const list = document.getElementById("results");
  if (!input || !list) return;
  const items = window.KG_SEARCH || [];
  let shown = [];
  let selected = 0;

  function score(item, q) {
    const name = item.n.toLowerCase();
    const leaf = name.slice(name.lastIndexOf(".") + 1);
    if (leaf === q || name === q) return 0;
    if (leaf.startsWith(q)) return 1;
    if (name.includes(q)) return 2;
    if (item.m.toLowerCase().includes(q)) return 3;
    if (item.s.toLowerCase().includes(q)) return 4;
    return -1;
  }

  function el(tag, cls, text) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text) e.textContent = text;
    return e;
  }

  function render() {
    const q = input.value.trim().toLowerCase();
    list.replaceChildren();
    if (!q) { list.hidden = true; shown = []; return; }
    shown = items
      .map((item) => [score(item, q), item])
      .filter(([s]) => s >= 0)
      .sort((a, b) => a[0] - b[0] || a[1].n.length - b[1].n.length || a[1].n.localeCompare(b[1].n))
      .slice(0, 30)
      .map(([, item]) => item);
    selected = 0;
    shown.forEach((item, i) => {
      const a = el("a");
      a.href = item.h;
      a.setAttribute("aria-selected", i === selected ? "true" : "false");
      a.append(el("span", "name", item.n), el("span", "where", item.k + " · " + item.m));
      if (item.s) a.append(el("span", "gist", item.s));
      const li = el("li");
      li.append(a);
      list.append(li);
    });
    list.hidden = shown.length === 0;
  }

  function select(i) {
    const links = list.querySelectorAll("a");
    if (!links.length) return;
    selected = (i + links.length) % links.length;
    links.forEach((a, j) => a.setAttribute("aria-selected", j === selected ? "true" : "false"));
    links[selected].scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("input", render);
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); select(selected + 1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); select(selected - 1); }
    else if (e.key === "Enter" && shown[selected]) { location.href = shown[selected].h; }
    else if (e.key === "Escape") { input.value = ""; render(); input.blur(); }
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "/" && document.activeElement !== input) { e.preventDefault(); input.focus(); }
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".search")) list.hidden = true;
  });
})();
"#;
