//! Output compared modulo source locations — the owner's parity bar
//! ("identical modulo locations", `xtask/src/output.rs`), and since
//! 2026-09-27 also what the backend cache's verifier calls a location-only
//! difference ([`crate::cache::difference`]). One definition for both: the
//! backend's own output varies run to run in its location comments on at
//! least one program (`test/logic/optimization_test_cases.k`, which of a
//! merged assertion's source lines a comment cites), and a verifier that
//! called that a failed premise was red at random.

/// How many location-bearing lines or rows a file holds.
pub fn locations(name: &str, bytes: &[u8]) -> usize {
    if name.ends_with(".sv") {
        String::from_utf8_lossy(bytes).lines().filter(|l| split_location(l).1.is_some()).count()
    } else if name.ends_with(".pdrpt") {
        serde_json::from_slice::<serde_json::Value>(bytes).ok().map_or(0, |v| report_rows(&v).map_or(0, |(rows, _)| rows.len()))
    } else {
        0
    }
}

/// Equal once locations are blanked: `Ok((agreeing, total))` location
/// lines or rows, or where the first real difference is.
pub fn modulo_locations(name: &str, a: &[u8], b: &[u8]) -> Result<(usize, usize), String> {
    if name.ends_with(".sv") {
        let (a, b) = (String::from_utf8_lossy(a), String::from_utf8_lossy(b));
        let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
        let (mut agree, mut total) = (0, 0);
        for (i, pair) in la.iter().zip(lb.iter().chain(std::iter::repeat(&""))).enumerate() {
            let ((code_a, loc_a), (code_b, loc_b)) = (split_location(pair.0), split_location(pair.1));
            if blank_message_lines(code_a) != blank_message_lines(code_b) || loc_a.is_some() != loc_b.is_some() {
                return Err(format!("line {}: theirs `{}` ours `{}`", i + 1, clip(pair.0), clip(pair.1)));
            }
            if loc_a.is_some() {
                total += 1;
                agree += usize::from(loc_a == loc_b);
            }
        }
        if lb.len() > la.len() {
            return Err(format!("line {}: ours has {} more line(s)", la.len() + 1, lb.len() - la.len()));
        }
        return Ok((agree, total));
    }
    if name.ends_with(".pdrpt") {
        let parse = |x: &[u8]| serde_json::from_slice::<serde_json::Value>(x).map_err(|e| format!("not JSON: {e}"));
        let (mut va, mut vb) = (parse(a)?, parse(b)?);
        let rows = |v: &serde_json::Value| report_rows(v).map(|(r, _)| r.to_vec()).unwrap_or_default();
        let (ra, rb) = (rows(&va), rows(&vb));
        let agree = ra.iter().zip(&rb).filter(|(x, y)| x == y).count();
        blank_report_locations(&mut va);
        blank_report_locations(&mut vb);
        if va != vb {
            return Err("differs outside its location columns".to_owned());
        }
        return Ok((agree, ra.len()));
    }
    Err("differs, and holds no locations".to_owned())
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 70 { format!("{}…", s.chars().take(70).collect::<String>()) } else { s.to_owned() }
}

/// A `.sv` line and its trailing location comment, when it has one. CIRCT
/// writes `\t// file.k:14:9`, `file.k:14:{9,16}`, `file.k:12:5, :14:9`,
/// and — for code inlined from a call — the call-site chain
/// `{{file.k:9:5 <- 'callee'(file.k:20:3)} <- 'caller'(file.k:31:1)}`,
/// whose quoted names are any text (`'fixture<|@Test1|>@?lambda0'`). A
/// comment is a location when, its quoted names aside, it is made only of
/// those shapes' characters and names a `.k` position; any other comment
/// is code, so an unknown shape reads as a difference, never agreement.
pub fn split_location(line: &str) -> (&str, Option<&str>) {
    let Some(at) = line.rfind("\t// ") else { return (line, None) };
    let comment = &line[at + 4..];
    let mut bare = String::new();
    let mut quoted = false;
    for c in comment.chars() {
        if c == '\'' {
            quoted = !quoted;
        } else if !quoted {
            bare.push(c);
        }
    }
    // Outside the names, every word is a `.k` file, a number or the `<-`
    // of a call site, between the shapes' punctuation.
    let word = |c: char| c.is_ascii_alphanumeric() || "_./-$".contains(c);
    let words_ok = bare
        .split(|c: char| !word(c))
        .all(|w| w.is_empty() || w == "-" || w.ends_with(".k") || w.bytes().all(|b| b.is_ascii_digit()));
    let punctuation_ok = bare.chars().all(|c| word(c) || ":{},<() ".contains(c));
    let positioned = bare.match_indices(".k:").any(|(i, _)| bare[i + 3..].starts_with(|c: char| c.is_ascii_digit() || c == '{'));
    if !quoted && positioned && words_ok && punctuation_ok {
        (&line[..at], Some(comment))
    } else {
        (line, None)
    }
}

/// An assertion's `$fatal` text carries source lines of its own — "Assertion
/// at: loop.k line: 134" and each call-trace frame's "in file loop.k at line
/// 139" — which are locations like the comments. Only the digits after
/// those two phrases are blanked; the frame's function name is compared.
fn blank_message_lines(code: &str) -> std::borrow::Cow<'_, str> {
    if !code.contains(" line: ") && !code.contains(" at line ") {
        return std::borrow::Cow::Borrowed(code);
    }
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(at) = [" line: ", " at line "].iter().filter_map(|p| rest.find(p).map(|i| (i, p.len()))).min() {
        let (i, n) = at;
        out.push_str(&rest[..i + n]);
        rest = &rest[i + n..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            out.push('#');
        }
        rest = &rest[digits..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// The report's `data` rows and the columns among them that are locations:
/// `relativePath`, `rangeStartLine`, `rangeStartCharacter`, `rangeEndLine`,
/// `rangeEndCharacter`, named by the header row.
fn report_rows(v: &serde_json::Value) -> Option<(&[serde_json::Value], Vec<usize>)> {
    let data = v.get("data")?.as_array()?;
    let (header, rows) = data.split_first()?;
    let columns = header
        .as_array()?
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            matches!(
                c.as_str(),
                Some("relativePath" | "rangeStartLine" | "rangeStartCharacter" | "rangeEndLine" | "rangeEndCharacter")
            )
        })
        .map(|(i, _)| i)
        .collect();
    Some((rows, columns))
}

fn blank_report_locations(v: &mut serde_json::Value) {
    let Some((_, columns)) = report_rows(v) else { return };
    let Some(data) = v.get_mut("data").and_then(|d| d.as_array_mut()) else { return };
    for row in data.iter_mut().skip(1) {
        if let Some(cells) = row.as_array_mut() {
            for &i in &columns {
                if let Some(cell) = cells.get_mut(i) {
                    *cell = serde_json::Value::Null;
                }
            }
        }
    }
}
