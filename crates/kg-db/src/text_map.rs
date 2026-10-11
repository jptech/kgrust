//! Where a range of one text is in another: the text a loop was computed
//! on, and the text the editor holds now (`crate::looped::EditorLoop`).
//!
//! The editor reads the last loop computed for a program on a buffer that
//! no longer parses (sprint W3.4), and every question it asks is a range
//! of the current text — a declaration's node, a member access's base —
//! that the loop knows by the range it had then. Text the edits did not
//! touch is the same text at another offset; text they did is not in the
//! loop at all.
//!
//! # Which characters are unchanged
//!
//! A line-level diff (Myers' greedy O((N+M)D), on line hashes), each
//! replaced run of lines then trimmed of the characters it begins and ends
//! with in both texts. A buffer is usually edited in one place, so the
//! common prefix and suffix are taken first and the diff runs on what is
//! between; past [`MAX_EDIT_LINES`] differing lines it gives up on the
//! middle, of which only the characters it begins and ends with in both
//! texts then map.
//!
//! # Which ranges map
//!
//! A range maps where its **first and last characters** are both
//! unchanged: its start lies inside an unchanged run, and so does the
//! character before its end. Each end maps through its own run, so a node
//! that *encloses* an edit — a function whose body is being typed in —
//! maps to the node it was, and the loop's answer there is the last one it
//! had: graceful staleness, `docs/plans/ir-elaboration.md` §2.6. A node
//! whose own first or last character is new does not map: `foo` edited to
//! `foobar` is not the old `foo`, and `foo` typed fresh before a `.` is
//! nothing the loop knew. The loop's index then looks for a node of the
//! same kind at exactly the mapped range, so a match is a node that began
//! and ended at the same unchanged characters.

use kg_base::{TextRange, TextSize};

/// More differing lines than this between two texts and the diff stops: the
/// changed middle then maps only at its edges. An editor's last good loop is a few
/// edits old; a file rewritten past this is no longer the program the loop
/// read, and quadratic memory in the edit's size is the diff's price.
pub const MAX_EDIT_LINES: usize = 512;

/// The unchanged runs between two texts, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMap {
    /// `(old start, new start, length)`, in bytes, increasing in both texts.
    runs: Vec<(u32, u32, u32)>,
}

impl TextMap {
    /// The map from `old` to `new`.
    pub fn new(old: &str, new: &str) -> TextMap {
        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        let mut push = |o: usize, n: usize, len: usize| {
            if len == 0 {
                return;
            }
            if let Some(last) = runs.last_mut()
                && (last.0 + last.2) as usize == o
                && (last.1 + last.2) as usize == n
            {
                last.2 += len as u32;
                return;
            }
            runs.push((o as u32, n as u32, len as u32));
        };
        let (a, b) = (lines(old), lines(new));
        // The common head and tail by lines, then the diff between.
        let head = a.iter().zip(&b).take_while(|(x, y)| x.1 == y.1).count();
        let tail = a[head..].iter().rev().zip(b[head..].iter().rev()).take_while(|(x, y)| x.1 == y.1).count();
        let (am, bm) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
        let start = |ls: &[(usize, &str)], i: usize, text: &str| ls.get(i).map_or(text.len(), |l| l.0);
        push(0, 0, start(&a, head, old));
        // Equal runs of the middle, in line indices of `am`/`bm`; the gaps
        // between them are the replaced hunks.
        let hashes = |ls: &[(usize, &str)]| ls.iter().map(|l| hash(l.1)).collect::<Vec<u64>>();
        let equal = diff(&hashes(am), &hashes(bm), MAX_EDIT_LINES).unwrap_or_default();
        let (mut ai, mut bi) = (0usize, 0usize);
        let hunk = |ai: usize, aj: usize, bi: usize, bj: usize, push: &mut dyn FnMut(usize, usize, usize)| {
            let (o0, o1) = (start(&a, head + ai, old), start(&a, head + aj, old));
            let (n0, n1) = (start(&b, head + bi, new), start(&b, head + bj, new));
            let (x, y) = (&old[o0..o1], &new[n0..n1]);
            let p = common_prefix(x, y);
            let s = common_suffix(&x[p..], &y[p..]);
            push(o0, n0, p);
            push(o1 - s, n1 - s, s);
        };
        for (ea, eb, len) in equal.into_iter().chain(std::iter::once((am.len(), bm.len(), 0))) {
            hunk(ai, ea, bi, eb, &mut push);
            let (o, n) = (start(&a, head + ea, old), start(&b, head + eb, new));
            let (o1, _) = (start(&a, head + ea + len, old), ());
            push(o, n, o1 - o);
            ai = ea + len;
            bi = eb + len;
        }
        let (o, n) = (start(&a, a.len() - tail, old), start(&b, b.len() - tail, new));
        push(o, n, old.len() - o);
        TextMap { runs }
    }

    /// The map of a text to itself.
    pub fn identity(len: TextSize) -> TextMap {
        TextMap { runs: vec![(0, 0, u32::from(len))] }
    }

    /// Whether every character of the old text is unchanged in the new, at
    /// the same offset.
    pub fn is_identity(&self, old_len: TextSize, new_len: TextSize) -> bool {
        old_len == new_len && self.runs.as_slice() == [(0, 0, u32::from(old_len))]
            || old_len == TextSize::from(0) && new_len == TextSize::from(0)
    }

    /// `range` of the new text in the old, where its first and last
    /// characters are unchanged (the module docs).
    pub fn to_old(&self, range: TextRange) -> Option<TextRange> {
        self.map(range, |r| (r.1, r.0, r.2))
    }

    /// `range` of the old text in the new, by the same rule.
    pub fn to_new(&self, range: TextRange) -> Option<TextRange> {
        self.map(range, |r| (r.0, r.1, r.2))
    }

    /// Maps through the runs keyed `(from, to, len)` by `key`.
    fn map(&self, range: TextRange, key: impl Fn(&(u32, u32, u32)) -> (u32, u32, u32)) -> Option<TextRange> {
        let (start, end) = (u32::from(range.start()), u32::from(range.end()));
        // The run holding the character at `at`.
        let at = |at: u32| {
            let i = self.runs.partition_point(|r| key(r).0 <= at);
            let (from, to, len) = key(self.runs.get(i.checked_sub(1)?)?);
            (at < from + len).then_some(to + (at - from))
        };
        if start == end {
            let s = at(start).or_else(|| Some(at(start.checked_sub(1)?)? + 1))?;
            return Some(TextRange::empty(TextSize::from(s)));
        }
        let s = at(start)?;
        let e = at(end - 1)? + 1;
        Some(TextRange::new(TextSize::from(s), TextSize::from(e)))
    }
}

/// Each line of `text` with its start, the newline kept with it.
fn lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        out.push((at, line));
        at += line.len();
    }
    out
}

fn hash(line: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    line.hash(&mut h);
    h.finish()
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut p = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(p) {
        p -= 1;
    }
    p
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut s = a.bytes().rev().zip(b.bytes().rev()).take_while(|(x, y)| x == y).count();
    while !a.is_char_boundary(a.len() - s) || !b.is_char_boundary(b.len() - s) {
        s -= 1;
    }
    s
}

/// The equal runs of a shortest edit script from `a` to `b`, as `(start in
/// a, start in b, length)` in order — Myers' greedy algorithm, `None` past
/// `max_d` differences. Each step's frontier is kept, `2d + 1` entries for
/// step `d`, to walk the script back.
fn diff(a: &[u64], b: &[u64], max_d: usize) -> Option<Vec<(usize, usize, usize)>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let limit = ((n + m) as usize).min(max_d) as isize;
    // `v[k + off]`: the furthest x on diagonal `k = x - y`.
    let off = limit + 1;
    let mut v = vec![0isize; (2 * off + 1) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=limit {
        trace.push(v[(off - d) as usize..=(off + d) as usize].to_vec());
        for k in (-d..=d).step_by(2) {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) { v[i + 1] } else { v[i - 1] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                end = Some(d);
                break 'search;
            }
        }
    }
    let end = end?;
    let mut runs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=end).rev() {
        let k = x - y;
        if d == 0 {
            if x > 0 {
                runs.push((0, 0, x as usize));
            }
            break;
        }
        // The frontier before step `d`, indexed from diagonal `-d`.
        let before = &trace[d as usize];
        let at = |k: isize| before[(k + d) as usize];
        let down = k == -d || (k != d && at(k - 1) < at(k + 1));
        let pk = if down { k + 1 } else { k - 1 };
        let (px, py) = (at(pk), at(pk) - pk);
        let (sx, sy) = if down { (px, py + 1) } else { (px + 1, py) };
        if x > sx {
            runs.push((sx as usize, sy as usize, (x - sx) as usize));
        }
        x = px;
        y = py;
    }
    runs.reverse();
    Some(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(a: u32, b: u32) -> TextRange {
        TextRange::new(TextSize::from(a), TextSize::from(b))
    }

    /// Every range the new text shares with the old, found by its text.
    fn maps(old: &str, new: &str, word: &str) -> Option<TextRange> {
        let at = new.find(word).expect("in the new text") as u32;
        TextMap::new(old, new).to_old(r(at, at + word.len() as u32))
    }

    #[test]
    fn text_before_and_after_one_edit_maps() {
        let old = "a;\nbb;\ncc;\n";
        let new = "a;\nbb;\nx.\ncc;\n";
        assert_eq!(maps(old, new, "bb"), Some(r(3, 5)));
        assert_eq!(maps(old, new, "cc"), Some(r(7, 9)));
        assert_eq!(maps(old, new, "x."), None, "typed text is nothing the old text held");
    }

    /// Edits in two places map what lies between them — a common prefix
    /// and suffix alone would map neither.
    #[test]
    fn text_between_two_edits_maps() {
        let old = "aa;\nbb;\ncc;\ndd;\nee;\n";
        let new = "aa;\nbX;\ncc;\ndd;\neY;\n";
        assert_eq!(maps(old, new, "cc"), Some(r(8, 10)));
        assert_eq!(maps(old, new, "dd"), Some(r(12, 14)));
        assert_eq!(maps(old, new, "aa"), Some(r(0, 2)));
        // Within a changed line, the characters before and after the edit.
        assert_eq!(maps(old, new, "b"), Some(r(4, 5)));
        assert_eq!(maps(old, new, "bX"), None);
    }

    /// A range whose first and last characters are unchanged maps to the
    /// range they bounded, whatever changed inside it; one whose own edge
    /// changed does not.
    #[test]
    fn a_range_maps_by_its_first_and_last_characters() {
        let old = "f() { a; }\n";
        let new = "f() { a. }\n";
        assert_eq!(TextMap::new(old, new).to_old(r(0, 10)), Some(r(0, 10)));
        let (old, new) = ("foo;", "foobar;");
        let map = TextMap::new(old, new);
        assert_eq!(map.to_old(r(0, 6)), None, "`foobar` is not the old `foo`");
        assert_eq!(map.to_old(r(0, 3)), Some(r(0, 3)));
        assert_eq!(map.to_new(r(0, 3)), Some(r(0, 3)));
        assert_eq!(map.to_new(r(3, 4)), Some(r(6, 7)));
    }

    #[test]
    fn deleted_and_inserted_lines_shift_what_follows() {
        let old = "one\ntwo\nthree\nfour\n";
        let new = "one\nthree\nNEW\nNEW2\nfour\n";
        let map = TextMap::new(old, new);
        assert_eq!(maps(old, new, "three"), Some(r(8, 13)));
        assert_eq!(maps(old, new, "four"), Some(r(14, 18)));
        assert_eq!(map.to_new(r(4, 7)), None, "`two` is gone");
        assert_eq!(map.to_new(r(14, 18)), Some(r(19, 23)));
    }

    #[test]
    fn the_same_text_is_the_identity() {
        let text = "a\nb\nc";
        let map = TextMap::new(text, text);
        assert!(map.is_identity(TextSize::from(5), TextSize::from(5)));
        assert_eq!(map, TextMap::identity(TextSize::from(5)));
        assert_eq!(TextMap::new("", ""), TextMap { runs: Vec::new() });
    }

    /// Past the bound the middle maps nothing but its edges, and the head
    /// and tail still map.
    #[test]
    fn a_rewrite_past_the_bound_maps_only_its_head_and_tail() {
        let mut old = String::from("head\n");
        let mut new = String::from("head\n");
        for i in 0..MAX_EDIT_LINES + 10 {
            old.push_str(&format!("o{i}\n"));
            new.push_str(&format!("n{i}\n"));
            if i % 2 == 0 {
                old.push_str("same\n");
                new.push_str("same\n");
            }
        }
        old.push_str("tail\n");
        new.push_str("tail\n");
        let map = TextMap::new(&old, &new);
        assert_eq!(maps(&old, &new, "head"), Some(r(0, 4)));
        assert!(maps(&old, &new, "tail").is_some());
        assert_eq!(maps(&old, &new, "same"), None);
        assert!(map.runs.len() <= 3);
    }

    /// The diff is a shortest script: each run it reports is equal text,
    /// and every line not in a run differs.
    #[test]
    fn the_diff_reports_equal_runs() {
        let a = [1, 2, 3, 4, 5, 6];
        let b = [1, 9, 3, 4, 7, 6, 8];
        let runs = diff(&a, &b, 64).expect("small");
        for &(x, y, len) in &runs {
            assert_eq!(a[x..x + len], b[y..y + len]);
        }
        assert_eq!(runs.iter().map(|r| r.2).sum::<usize>(), 4, "1, 3 4, 6 — the longest common subsequence");
    }
}
