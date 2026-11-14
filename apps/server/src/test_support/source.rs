//! One source scanner for every guard (P0.24): the policy inventory, the
//! module manifest test, both isolation nets and the structure guards read
//! the server's own source through these functions, so they agree on which
//! files count, where the test tail starts and what a route is.

use std::path::Path;

/// Every `.rs` file under `root`, as (path relative to `root` with `/`
/// separators, contents), sorted by path.
pub fn rust_files(root: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let entries = std::fs::read_dir(&folder)
            .unwrap_or_else(|error| panic!("read {}: {error}", folder.display()));
        for entry in entries {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                let relative = path
                    .strip_prefix(root)
                    .expect("file under root")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((relative, source));
            }
        }
    }
    found.sort();
    found
}

/// The server crate's `src/` folder.
pub fn server_src() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// The source before its test tail: everything up to the first
/// `#[cfg(test)]` line that starts a module (`mod tests`, `mod guard`, …).
/// One rule for every guard, so a test-only router or fixture never counts.
pub fn strip_tests(source: &str) -> &str {
    let mut offset = 0;
    while let Some(found) = source[offset..].find("#[cfg(test)]") {
        let at = offset + found;
        let line_start = source[..at].rfind('\n').map_or(0, |index| index + 1);
        let after = source[at + "#[cfg(test)]".len()..].trim_start();
        let starts_line = source[line_start..at].trim().is_empty();
        if starts_line && (after.starts_with("mod ") || after.starts_with("pub(crate) mod ")) {
            return &source[..line_start];
        }
        offset = at + "#[cfg(test)]".len();
    }
    source
}

/// One `.route("<path>", <handlers>)` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Route {
    /// The path template, as written.
    pub path: String,
    /// The HTTP methods registered: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`.
    pub methods: Vec<&'static str>,
}

/// A route registration, spelled in two parts so the scanner's own file
/// holds no route call.
const ROUTE_CALL: &str = concat!(".route", "(");

/// Every `.route(...)` call in `source` whose first argument is a string
/// literal, with the methods it registers. Run it on [`strip_tests`]'s
/// output to leave out test-only routers.
pub fn routes(source: &str) -> Vec<Route> {
    route_blocks(source)
        .into_iter()
        .filter_map(|block| {
            let path = first_argument_literal(block)?;
            let methods = [
                ("GET", "get("),
                ("POST", "post("),
                ("PUT", "put("),
                ("PATCH", "patch("),
                ("DELETE", "delete("),
            ]
            .into_iter()
            .filter(|(_, token)| has_call(block, token))
            .map(|(method, _)| method)
            .collect();
            Some(Route {
                path: path.to_owned(),
                methods,
            })
        })
        .collect()
}

/// `token` appears as a call, not as the end of a longer name.
pub fn has_call(block: &str, token: &str) -> bool {
    block.match_indices(token).any(|(index, _)| {
        !block[..index]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
    })
}

/// Each `.route(...)` call, from `.route` to its closing parenthesis.
pub fn route_blocks(source: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut remaining = source;
    while let Some(relative_start) = remaining.find(ROUTE_CALL) {
        let start = source.len() - remaining.len() + relative_start;
        let open = start + ".route".len();
        let mut depth = 0_u32;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, character) in source[open..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(open + offset + character.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { break };
        blocks.push(&source[start..end]);
        remaining = &source[end..];
    }
    blocks
}

/// The route call's first argument, when it is a plain string literal.
fn first_argument_literal(block: &str) -> Option<&str> {
    let arguments = block.strip_prefix(".route")?.strip_prefix('(')?;
    let open = arguments.find('"')?;
    if !arguments[..open].trim().is_empty() {
        return None;
    }
    let rest = &arguments[open + 1..];
    let close = rest.find('"')?;
    Some(&rest[..close])
}

/// A string literal in Rust source: its first line (1-based) and its text
/// with escapes left as written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Literal {
    pub line: usize,
    pub text: String,
}

/// Every string literal (`"…"`, `r"…"`, `r#"…"#`, `b"…"`) outside comments.
pub fn string_literals(source: &str) -> Vec<Literal> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut index = 0;
    let mut line = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\n' => {
                line += 1;
                index += 1;
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                while index + 1 < bytes.len() && !(bytes[index] == b'*' && bytes[index + 1] == b'/')
                {
                    if bytes[index] == b'\n' {
                        line += 1;
                    }
                    index += 1;
                }
                index += 2;
            }
            b'\'' => {
                // A char literal ('x', '\n', '"') or a lifetime ('a).
                if bytes.get(index + 1) == Some(&b'\\') {
                    index += 2;
                    while index < bytes.len() && bytes[index] != b'\'' {
                        index += 1;
                    }
                    index += 1;
                } else if let Some(width) = source[index + 1..].chars().next().map(char::len_utf8)
                    && bytes.get(index + 1 + width) == Some(&b'\'')
                {
                    index += 2 + width;
                } else {
                    index += 1;
                }
            }
            b'r' | b'b'
                if !index
                    .checked_sub(1)
                    .is_some_and(|previous| is_ident(bytes[previous])) =>
            {
                // r"…", r#"…"#, br"…", b"…"
                let mut cursor = index + 1;
                if byte == b'b' && bytes.get(cursor) == Some(&b'r') {
                    cursor += 1;
                }
                let raw = byte == b'r' || bytes.get(index + 1) == Some(&b'r');
                let mut hashes = 0;
                while raw && bytes.get(cursor) == Some(&b'#') {
                    hashes += 1;
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'"') {
                    index += 1;
                    continue;
                }
                let start_line = line;
                let body_start = cursor + 1;
                if raw {
                    let closing = format!("\"{}", "#".repeat(hashes));
                    let end = source[body_start..]
                        .find(&closing)
                        .map_or(bytes.len(), |found| body_start + found);
                    let text = &source[body_start..end];
                    line += text.matches('\n').count();
                    literals.push(Literal {
                        line: start_line,
                        text: text.to_owned(),
                    });
                    index = end + closing.len();
                } else {
                    let (text, end) = escaped_string(source, body_start);
                    line += text.matches('\n').count();
                    literals.push(Literal {
                        line: start_line,
                        text,
                    });
                    index = end;
                }
            }
            b'"' => {
                let start_line = line;
                let (text, end) = escaped_string(source, index + 1);
                line += text.matches('\n').count();
                literals.push(Literal {
                    line: start_line,
                    text,
                });
                index = end;
            }
            _ => index += 1,
        }
    }
    literals
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The text of a `"…"` literal whose body starts at `start`, and the index
/// just past its closing quote.
fn escaped_string(source: &str, start: usize) -> (String, usize) {
    let bytes = source.as_bytes();
    let mut index = start;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 2,
            b'"' => return (source[start..index].to_owned(), index + 1),
            _ => index += 1,
        }
    }
    (source[start..].to_owned(), bytes.len())
}

/// `text` with every run of whitespace turned into one space, so a matcher
/// doesn't depend on how a query was wrapped.
pub fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `haystack` holds `word` with no identifier character right after
/// it (`FROM messages` but not `FROM messages_archive`).
pub fn contains_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(index, _)| {
        !haystack[index + word.len()..]
            .bytes()
            .next()
            .is_some_and(is_ident)
    })
}

/// Lines of `source` that are code, not comments: a line whose first
/// non-space characters are `//` is left out.
pub fn code_lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .map(|(index, line)| (index + 1, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_cover_every_form() {
        let source = r####"
            let a = "plain \" quote";
            let b = r#"raw "with" quotes"#;
            let c = r"raw";
            let d = '"'; // "not a literal"
            fn f<'a>(x: &'a str) {}
            /* "nor this" */
            let e = br"bytes";
            let f = format!("one {}", "two");
        "####;
        let found = string_literals(source)
            .into_iter()
            .map(|literal| literal.text)
            .collect::<Vec<_>>();
        assert_eq!(
            found,
            [
                "plain \\\" quote",
                "raw \"with\" quotes",
                "raw",
                "bytes",
                "one {}",
                "two"
            ]
        );
    }

    #[test]
    fn the_test_tail_is_cut_once() {
        let source = "fn a() {}\n#[cfg(test)]\nfn helper() {}\n#[cfg(test)]\nmod tests {\n}\n";
        assert_eq!(
            strip_tests(source),
            "fn a() {}\n#[cfg(test)]\nfn helper() {}\n"
        );
        assert_eq!(strip_tests("fn a() {}"), "fn a() {}");
    }

    #[test]
    fn routes_carry_their_methods() {
        let source = concat!(
            ".route", "(\"/a/{id}\", get(read).post(write))\n",
            ".route", "(PATH, get(other))\n",
            ".route", "(\"/b\", axum::routing::patch(change))"
        );
        assert_eq!(
            routes(source),
            [
                Route {
                    path: "/a/{id}".to_owned(),
                    methods: vec!["GET", "POST"],
                },
                Route {
                    path: "/b".to_owned(),
                    methods: vec!["PATCH"],
                },
            ]
        );
    }

    #[test]
    fn words_end_at_identifier_boundaries() {
        assert!(contains_word("FROM messages m", "FROM messages"));
        assert!(!contains_word("FROM messages_archive", "FROM messages"));
        assert!(contains_word("x FROM messages", "FROM messages"));
    }
}
