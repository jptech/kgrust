//! Putting the trailing comments back.
//!
//! The document algebra measures everything it holds, and a `//<` doc comment
//! is often the longest thing on its line: `library/codec/crc.k:42` is 179
//! columns of which 145 are comment. Measured, it would push the parameter list
//! in front of it over the width and break a list the author wrote on one line
//! — the exact "naive width-driven formatter" output that `docs/spec/style.md`
//! §12, example 9 marks as **wrong**. §5.4 is explicit that `line_width` is a
//! threshold for breaking *lists*, and that long doc comments are not lists.
//!
//! So a comment that trails code and ends its line is lifted out of the
//! document before rendering ([`crate::lower`]) and appended here afterwards.
//! The landing place is not a guess: the input and the output hold the same
//! non-trivia tokens in the same order ([`crate::verify`]), so the comment goes
//! at the end of the line holding the *n*-th output token, where the *n*-th
//! input token is the one it trailed.
//!
//! A comment is only ever lifted when the separator that followed it ends the
//! line, which means its anchor token is still the last thing on its own line
//! in the output. That is what makes "append to the end of that line" the same
//! position it started in, and it is why no comment can cross a token.

use std::collections::HashSet;

use kg_syntax::{SyntaxElement, SyntaxNode};

/// Appends the lifted comments to `output`.
pub(crate) fn reattach(input: &SyntaxNode, lifted: &[u32], output: String) -> String {
    if lifted.is_empty() {
        return output;
    }
    let wanted: HashSet<u32> = lifted.iter().copied().collect();

    // Each lifted comment, paired with the index of the token it trailed.
    let mut pending: Vec<(usize, String)> = Vec::with_capacity(wanted.len());
    let mut seen = 0usize;
    for e in input.descendants_with_tokens() {
        let SyntaxElement::Token(t) = e else { continue };
        if !t.kind().is_trivia() {
            seen += 1;
            continue;
        }
        if t.kind().is_comment() && wanted.contains(&u32::from(t.text_range().start())) && seen > 0
        {
            pending.push((seen - 1, t.text().trim_end().to_owned()));
        }
    }
    if pending.is_empty() {
        return output;
    }

    let out_root = crate::reparse(&output).syntax_node();
    let starts = line_starts(&output);
    let token_line: Vec<usize> = out_root
        .descendants_with_tokens()
        .filter_map(|e| match e {
            SyntaxElement::Token(t) if !t.kind().is_trivia() => {
                let off = usize::from(t.text_range().start());
                Some(starts.partition_point(|&s| s <= off) - 1)
            }
            _ => None,
        })
        .collect();

    let mut lines: Vec<String> = output.split('\n').map(str::to_owned).collect();
    for (token, text) in pending {
        let Some(&line) = token_line.get(token) else { continue };
        let l = &mut lines[line];
        l.truncate(l.trim_end().len());
        if !l.is_empty() {
            l.push(' ');
        }
        l.push_str(&text);
    }
    lines.join("\n")
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    starts
}
