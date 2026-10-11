//! `file:` URI ⇄ filesystem path.
//!
//! `lsp_types::Uri` is a thin newtype over `fluent_uri` and deliberately knows
//! nothing about files, so the two conversions live here. They are only ever
//! *lossy in one direction*: a URI that is not a `file:` URI (VS Code's
//! `untitled:`, notebook cells, remote schemes) has no path, and the server
//! still has to be able to talk about it — so [`uri_to_key`] falls back to the
//! URI text itself, which is a perfectly good [`kg_vfs::Vfs`] key even though
//! nothing will ever read it from disk.

use std::str::FromStr;

use camino::{Utf8Path, Utf8PathBuf};
use lsp_types::Uri;

/// Turns a URI into the path the VFS should intern it under.
///
/// `file:` URIs become real paths so that a manifest lookup and an on-disk read
/// both work. Everything else keeps its URI spelling, which is unique and
/// stable but never resolves on disk — exactly the behaviour an unsaved
/// `untitled:` buffer wants.
pub fn uri_to_key(uri: &Uri) -> Utf8PathBuf {
    uri_to_path(uri).unwrap_or_else(|| Utf8PathBuf::from(uri.as_str()))
}

/// The filesystem path of a `file:` URI, or `None` for any other scheme.
pub fn uri_to_path(uri: &Uri) -> Option<Utf8PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    // `file:///c:/x` has an empty authority; anything else is a UNC share we
    // rebuild as `\\host\share`, which `Utf8Path` handles on Windows and which
    // simply never occurs elsewhere.
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let decoded = percent_decode(path)?;

    // `/C:/Users/...` — strip the leading slash of a drive-letter path.
    let decoded = match decoded.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => decoded[1..].to_owned(),
        _ => decoded,
    };

    if authority.is_empty() {
        Some(Utf8PathBuf::from(decoded))
    } else {
        Some(Utf8PathBuf::from(format!("//{authority}{decoded}")))
    }
}

/// The `file:` URI of an absolute path.
pub fn path_to_uri(path: &Utf8Path) -> Option<Uri> {
    let text = path.as_str();
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str("file://");
    // A Windows path (`C:\x`) has no leading separator, so add the one that
    // makes the authority empty.
    if !text.starts_with('/') {
        out.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'\\' => out.push('/'),
            b'/' | b':' => out.push(byte as char),
            b if is_unreserved(b) => out.push(b as char),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    Uri::from_str(&out).ok()
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// Decodes `%XX` escapes. Returns `None` if the result is not UTF-8, which is
/// the only case where a path is genuinely unrepresentable here.
fn percent_decode(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_owned());
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // A stray `%` is kept verbatim rather than failing the whole
        // conversion; a malformed URI must not take a buffer offline.
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        Uri::from_str(text).unwrap()
    }

    #[test]
    fn plain_unix_path_round_trips() {
        let path = Utf8PathBuf::from("/home/dev/design/top.k");
        let u = path_to_uri(&path).unwrap();
        assert_eq!(u.as_str(), "file:///home/dev/design/top.k");
        assert_eq!(uri_to_path(&u), Some(path));
    }

    #[test]
    fn spaces_and_non_ascii_round_trip() {
        let path = Utf8PathBuf::from("/home/dev/my designs/héllo 𝄞.k");
        let u = path_to_uri(&path).unwrap();
        assert!(u.as_str().contains("%20"), "{}", u.as_str());
        assert_eq!(uri_to_path(&u), Some(path));
    }

    #[test]
    fn windows_drive_letters_lose_the_leading_slash() {
        assert_eq!(
            uri_to_path(&uri("file:///c:/work/top.k")),
            Some(Utf8PathBuf::from("c:/work/top.k"))
        );
    }

    #[test]
    fn a_non_file_scheme_keys_on_its_own_text() {
        let u = uri("untitled:Untitled-1");
        assert_eq!(uri_to_path(&u), None);
        assert_eq!(uri_to_key(&u), Utf8PathBuf::from("untitled:Untitled-1"));
    }
}
