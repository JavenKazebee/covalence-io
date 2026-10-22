//! The text form of state paths, for people: expressions, the API, logs
//! (ADR 0017, state spec §1.1).
//!
//! `vh.outputs[1].source`, `vh.outputs["SDI 4"].source`. A key that is an
//! identifier is written `.key`; an integer key `[1]`; any other key as a
//! JSON string in brackets.

use covalence_protocol::state::{Path, PathSegment};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid path at byte {at}: {why}")]
pub struct PathError {
    pub at: usize,
    pub why: &'static str,
}

/// Formats `root` (an alias or reserved root) followed by `path`.
pub fn format(root: &str, path: &[PathSegment]) -> String {
    let mut out = root.to_owned();
    for segment in path {
        match segment {
            PathSegment::Index(i) => out.push_str(&format!("[{i}]")),
            PathSegment::Key(k) if is_ident(k) => {
                out.push('.');
                out.push_str(k);
            }
            PathSegment::Key(k) => {
                out.push('[');
                out.push_str(&serde_json::to_string(k).expect("strings always serialise"));
                out.push(']');
            }
        }
    }
    out
}

/// Parses a text path into its root and segments.
pub fn parse(text: &str) -> Result<(String, Path), PathError> {
    let bytes = text.as_bytes();
    let ident_end = |from: usize| {
        let mut end = from;
        while end < bytes.len() && (bytes[end] == b'_' || bytes[end].is_ascii_alphanumeric()) {
            end += 1;
        }
        end
    };
    let err = |at, why| Err(PathError { at, why });

    let mut i = ident_end(0);
    if !is_ident(&text[..i]) {
        return err(0, "expected a root name");
    }
    let root = text[..i].to_owned();
    let mut path = Vec::new();

    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                let end = ident_end(i + 1);
                if !is_ident(&text[i + 1..end]) {
                    return err(i + 1, "expected a key after `.`");
                }
                path.push(PathSegment::Key(text[i + 1..end].to_owned()));
                i = end;
            }
            b'[' if bytes.get(i + 1) == Some(&b'"') => {
                // Find the closing quote, skipping escaped characters.
                let mut end = i + 2;
                loop {
                    match bytes.get(end) {
                        None => return err(i, "unterminated string key"),
                        Some(b'\\') => end += 2,
                        Some(b'"') => break,
                        Some(_) => end += 1,
                    }
                }
                let Ok(key) = serde_json::from_str::<String>(&text[i + 1..=end]) else {
                    return err(i + 1, "invalid string key");
                };
                if bytes.get(end + 1) != Some(&b']') {
                    return err(end + 1, "expected `]`");
                }
                path.push(PathSegment::Key(key));
                i = end + 2;
            }
            b'[' => {
                let Some(close) = text[i..].find(']') else {
                    return err(i, "expected `]`");
                };
                let Ok(index) = text[i + 1..i + close].parse::<i64>() else {
                    return err(i + 1, "expected an integer or a quoted key");
                };
                path.push(PathSegment::Index(index));
                i += close + 1;
            }
            _ => return err(i, "expected `.` or `[`"),
        }
    }
    Ok((root, path))
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use PathSegment::{Index, Key};

    fn key(k: &str) -> PathSegment {
        Key(k.into())
    }

    #[test]
    fn round_trips() {
        let cases: &[(&str, Path)] = &[
            ("vh", vec![]),
            (
                "vh.outputs[1].source",
                vec![key("outputs"), Index(1), key("source")],
            ),
            (
                "vh.outputs[\"SDI 4\"].source",
                vec![key("outputs"), key("SDI 4"), key("source")],
            ),
            (
                "pp.t[\"a.b\"][\"q\\\"]\"][-2]",
                vec![key("t"), key("a.b"), key("q\"]"), Index(-2)],
            ),
            ("vars._x9", vec![key("_x9")]),
            // A numeric string key stays a string key.
            ("vh.m[\"7\"]", vec![key("m"), key("7")]),
        ];
        for (text, path) in cases {
            let (root, root_path) = (text.split(['.', '[']).next().unwrap(), path);
            assert_eq!(format(root, root_path), *text);
            assert_eq!(
                parse(text),
                Ok((root.to_owned(), root_path.clone())),
                "{text}"
            );
        }
    }

    #[test]
    fn rejects_malformed_paths() {
        for text in [
            "",
            "1vh",
            "vh.",
            "vh..a",
            "vh[",
            "vh[x]",
            "vh[\"a\"",
            "vh[\"a\"x",
            "vh a",
            "vh.a-b",
        ] {
            assert!(parse(text).is_err(), "{text:?} parsed");
        }
    }
}
