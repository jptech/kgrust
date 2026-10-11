//! Documentation text to HTML.
//!
//! The reference writes Pandoc's Markdown (`kanagawa/doc/sandcastle.md`,
//! "Markup"); [`kg_syntax::to_commonmark`] rewrites the parts CommonMark has
//! no spelling for, and `pulldown-cmark` renders the rest. Three things on
//! top, each what the reference's generator does in its own way:
//!
//! - an `#### __Example__` or `__Examples__` heading folds what follows it,
//!   up to the next heading, into a collapsible block;
//! - inline code that names a documented declaration links to it;
//! - headings are demoted two levels, so a `#` inside a declaration's
//!   documentation sits under the declaration's own title.

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};

/// `text` as HTML. `link` maps a name in inline code to the page that
/// documents it, where one does.
pub fn to_html(text: &str, link: &dyn Fn(&str) -> Option<String>) -> String {
    let markdown = kg_syntax::to_commonmark(text, kg_syntax::Headings::Keep);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_HEADING_ATTRIBUTES;
    let events: Vec<Event<'_>> = Parser::new_ext(&markdown, options).collect();
    let mut out: Vec<Event<'_>> = Vec::with_capacity(events.len());
    let mut folded = false;
    let mut in_link = 0usize;
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Event::Start(Tag::Heading { level, id, classes, attrs }) => {
                let end = events[i..]
                    .iter()
                    .position(|e| matches!(e, Event::End(TagEnd::Heading(_))))
                    .map_or(events.len() - 1, |p| i + p);
                let title: String = events[i + 1..end]
                    .iter()
                    .filter_map(|e| match e {
                        Event::Text(t) | Event::Code(t) => Some(t.as_ref()),
                        _ => None,
                    })
                    .collect();
                if folded && *level <= HeadingLevel::H4 {
                    out.push(Event::Html("</details>\n".into()));
                    folded = false;
                }
                let title = title.trim();
                if matches!(title, "Example" | "Examples") {
                    out.push(Event::Html(format!("<details class=\"example\"><summary>{title}</summary>\n").into()));
                    folded = true;
                } else {
                    let demoted = demote(*level);
                    out.push(Event::Start(Tag::Heading {
                        level: demoted,
                        id: id.clone(),
                        classes: classes.clone(),
                        attrs: attrs.clone(),
                    }));
                    out.extend(events[i + 1..end].iter().cloned());
                    out.push(Event::End(TagEnd::Heading(demoted)));
                }
                i = end + 1;
                continue;
            }
            Event::Start(Tag::Link { .. }) => {
                in_link += 1;
                out.push(events[i].clone());
            }
            Event::End(TagEnd::Link) => {
                in_link = in_link.saturating_sub(1);
                out.push(events[i].clone());
            }
            Event::Code(code) if in_link == 0 => match link(code) {
                Some(href) => out.push(Event::Html(CowStr::from(format!(
                    "<a href=\"{}\"><code>{}</code></a>",
                    escape(&href),
                    escape(code)
                )))),
                None => out.push(events[i].clone()),
            },
            event => out.push(event.clone()),
        }
        i += 1;
    }
    if folded {
        out.push(Event::Html("</details>\n".into()));
    }
    let mut html_out = String::with_capacity(markdown.len() * 3 / 2);
    html::push_html(&mut html_out, out.into_iter());
    html_out
}

/// The first paragraph of `text` as plain text, for an index or a search
/// result; at most `limit` characters.
pub fn summary(text: &str, limit: usize) -> String {
    let first = text.split("\n\n").find(|p| !p.trim().is_empty() && !p.trim_start().starts_with('#')).unwrap_or("");
    let plain: String = first
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !matches!(c, '`' | '*'))
        .collect();
    if plain.chars().count() <= limit {
        return plain;
    }
    let cut: String = plain.chars().take(limit).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

fn demote(level: HeadingLevel) -> HeadingLevel {
    match level {
        HeadingLevel::H1 => HeadingLevel::H3,
        HeadingLevel::H2 => HeadingLevel::H4,
        HeadingLevel::H3 => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}

/// Text for HTML, in content or in a double-quoted attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn examples_fold_and_headings_sit_under_the_declaration() {
        let html = to_html("Intro.\n\n# Part\n\nText.\n\n#### __Examples__\n\n    >>> f(1)\n    2\n\n# After\n", &none);
        assert!(html.contains("<h3>Part</h3>"), "{html}");
        assert!(html.contains("<details class=\"example\"><summary>Examples</summary>"), "{html}");
        let folded = &html[html.find("<details").unwrap()..html.find("</details>").unwrap()];
        assert!(folded.contains("&gt;&gt;&gt; f(1)"), "{html}");
        assert!(html.find("</details>").unwrap() < html.find("<h3>After</h3>").unwrap(), "{html}");
    }

    #[test]
    fn inline_code_naming_a_declaration_links_to_it() {
        let link = |name: &str| (name == "map").then(|| "data.array.html#map".to_owned());
        let html = to_html("See `map` and `other`, and [`map`](x.html).", &link);
        assert!(html.contains("<a href=\"data.array.html#map\"><code>map</code></a>"), "{html}");
        assert!(html.contains("<code>other</code>"), "{html}");
        assert_eq!(html.matches("data.array.html#map").count(), 1, "a code span in a link is left alone: {html}");
    }

    #[test]
    fn a_summary_is_the_first_paragraph_in_plain_text() {
        assert_eq!(summary("Map `f` over *x*.\nTwice.\n\nMore.", 80), "Map f over x. Twice.");
        assert_eq!(summary("# Heading\n\nBody text here", 80), "Body text here");
        assert_eq!(summary("one two three four", 9), "one two…");
    }
}
