//! Pages as blocks (AWD-53): a page's Markdown split into its top-level
//! blocks, by the rules the web app draws them with.
//!
//! A page is stored as a live document (`page_docs`), and its Markdown is
//! worked out from that. This module finds where each top-level block of a
//! Markdown text starts and ends, so the words a page's Markdown already
//! has, or that an agent sent, can be kept for the blocks they say. Lasting
//! block ids, and the `page_blocks` and `page_edit` tools that use them, are
//! in `page_docs` and `mcp/memory_tools.rs`.
//!
//! What it is, exactly:
//!
//! - **The blocks the web app draws.** They're found with the top-level
//!   rules of the web's own Markdown parser (`apps/web/src/ui/markdown/
//!   parse.ts`): heading, paragraph, code, quote, list, rule (`hr`), table,
//!   and the source past the parser's limit (`overflow`). It isn't a
//!   CommonMark parser, and it looks only at the top level: a list's items
//!   and a quote's contents stay inside their block. `blockParity.json`, next
//!   to parse.ts, holds the cases both parsers are tested against. Where
//!   parse.ts relies on JavaScript's `\s`, `trim()` or `.`, this follows
//!   JavaScript's character rules, not Rust's (U+FEFF, U+0085, U+2028, U+2029).
//! - **Nothing is lost or rewritten.** Each block is a byte range of the
//!   text exactly as given (line endings, tabs, trailing spaces and all),
//!   running from its first line to its last non-blank one; between blocks
//!   there is only blank space. Markdown the app doesn't draw specially
//!   (HTML, footnotes, setext underlines, front matter) stays as text inside
//!   whichever block holds it.
//! - **The limit, where the two sides differ.** The web parses the first
//!   100,000 UTF-16 code units of the body (after `\r\n` and `\r` become
//!   `\n`) and shows the rest as plain text. When that limit falls inside a
//!   character that takes two code units, the web splits the character; a
//!   byte range can't, so here the whole character starts the `overflow`
//!   block, which then says `limitInsideCharacter: true`.

/// The web parser's `MAX_SOURCE`: how many UTF-16 code units of the body,
/// after line endings become `\n`, it parses.
pub const PARSE_LIMIT: usize = 100_000;

/// The web parser's `MAX_TABLE_COLUMNS`.
const MAX_TABLE_COLUMNS: usize = 40;

/// What a top-level block is, named as the web parser names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// `text` is the heading's Markdown without its `#` markers.
    Heading {
        level: u8,
        text: String,
    },
    Paragraph,
    /// `lang` is a fence's info word, lowercased.
    Code {
        fenced: bool,
        lang: Option<String>,
    },
    Quote,
    /// `tasks` counts the items that start with `[ ]` or `[x]`.
    List {
        ordered: bool,
        start: u32,
        items: usize,
        tasks: usize,
    },
    Hr,
    Table {
        columns: usize,
        rows: usize,
    },
    /// The body past [`PARSE_LIMIT`], which the web shows as plain text.
    Overflow {
        limit_inside_character: bool,
    },
}

/// A top-level block: what it is, and its byte range in the page body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub start: usize,
    pub end: usize,
}

/// A page body, seen as its top-level blocks.
#[derive(Debug)]
pub struct PageBlocks {
    blocks: Vec<Block>,
}

impl PageBlocks {
    /// Finds the top-level blocks of a page body.
    pub fn parse(source: &str) -> Self {
        let (cut, limit_inside_character) = parse_limit(source);
        let mut blocks = top_level_blocks(&source[..cut]);
        if cut < source.len() {
            blocks.push(Block {
                kind: BlockKind::Overflow {
                    limit_inside_character,
                },
                start: cut,
                end: source.len(),
            });
        }
        Self { blocks }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }
}

/// Where the web parser stops: the byte offset after [`PARSE_LIMIT`] UTF-16
/// code units, counting `\r\n` and `\r` as one `\n`, and whether the limit
/// falls inside a two-unit character (which then goes wholly past it).
fn parse_limit(source: &str) -> (usize, bool) {
    let mut units = 0;
    let mut chars = source.char_indices().peekable();
    while let Some((at, character)) = chars.next() {
        let width = if character == '\r' {
            if chars.peek().is_some_and(|&(_, next)| next == '\n') {
                chars.next();
            }
            1
        } else {
            character.len_utf16()
        };
        if units + width > PARSE_LIMIT {
            return (at, units < PARSE_LIMIT);
        }
        units += width;
    }
    (source.len(), false)
}

// ---------------------------------------------------------------------------
// The top-level rules of parse.ts's parseBlocks, on byte ranges. Each check
// below names the pattern it follows.

/// A line's content, without its ending.
#[derive(Clone, Copy)]
struct Line<'a> {
    start: usize,
    end: usize,
    text: &'a str,
}

/// Lines as parse.ts's `split('\n')` sees them after normalizing `\r\n` and
/// `\r`: always at least one, and an empty last one after a final ending.
fn lines(source: &str) -> Vec<Line<'_>> {
    let bytes = source.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' | b'\r' => {
                lines.push(Line {
                    start,
                    end: at,
                    text: &source[start..at],
                });
                at += if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = at;
            }
            _ => at += 1,
        }
    }
    lines.push(Line {
        start,
        end: source.len(),
        text: &source[start..],
    });
    lines
}

fn top_level_blocks(source: &str) -> Vec<Block> {
    let lines = lines(source);
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].text;
        if is_blank(line) {
            i += 1;
            continue;
        }
        let from = i;
        let kind = if let Some(fence) = fence_open(line) {
            i += 1;
            while i < lines.len() {
                let closes = fence_closes(lines[i].text, fence.marker, fence.length);
                i += 1;
                if closes {
                    break;
                }
            }
            BlockKind::Code {
                fenced: true,
                lang: fence.lang,
            }
        } else if let Some((level, text)) = heading(line) {
            i += 1;
            BlockKind::Heading { level, text }
        } else if is_rule(line) {
            i += 1;
            BlockKind::Hr
        } else if is_quote(line) {
            while i < lines.len() && is_quote(lines[i].text) {
                i += 1;
            }
            BlockKind::Quote
        } else if list_item(line).is_some() {
            let (kind, next) = list(&lines, i);
            i = next;
            kind
        } else if indent_of(line) >= 4 {
            while i < lines.len() && (indent_of(lines[i].text) >= 4 || is_blank(lines[i].text)) {
                i += 1;
            }
            BlockKind::Code {
                fenced: false,
                lang: None,
            }
        } else if let Some((columns, rows)) = table(&lines, i) {
            i += 2 + rows;
            BlockKind::Table { columns, rows }
        } else {
            i += 1;
            while i < lines.len() && !is_blank(lines[i].text) && !starts_block(lines[i].text) {
                i += 1;
            }
            BlockKind::Paragraph
        };
        // A block runs from its first line to its last non-blank one.
        let mut last = i - 1;
        while last > from && is_blank(lines[last].text) {
            last -= 1;
        }
        blocks.push(Block {
            kind,
            start: lines[from].start,
            end: lines[last].end,
        });
    }
    blocks
}

/// ECMAScript's WhiteSpace and LineTerminator characters: what JavaScript's
/// `\s` matches and `trim()` removes. Unlike Rust's `char::is_whitespace`, it
/// takes U+FEFF and leaves out U+0085.
pub(crate) fn is_js_space(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JavaScript's `.` (without the `s` flag) stops at line terminators. Lines
/// never hold `\n` or `\r`, but they can hold these two, and then a pattern's
/// `.*` can't reach the `$` after them.
pub(crate) fn has_js_line_terminator(text: &str) -> bool {
    text.contains(['\u{2028}', '\u{2029}'])
}

/// `/^[ \t]*$/`
pub(crate) fn is_blank(line: &str) -> bool {
    line.bytes().all(|byte| byte == b' ' || byte == b'\t')
}

/// The leading-whitespace width, with tabs to the next multiple of four.
pub(crate) fn indent_of(line: &str) -> usize {
    let mut width = 0;
    for byte in line.bytes() {
        match byte {
            b' ' => width += 1,
            b'\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

pub(crate) fn leading_spaces(line: &str) -> usize {
    line.bytes().take_while(|&byte| byte == b' ').count()
}

/// The line after ` {0,3}`, when a pattern's marker must come next.
pub(crate) fn after_indent(line: &str) -> Option<&str> {
    let spaces = leading_spaces(line);
    (spaces <= 3).then(|| &line[spaces..])
}

pub(crate) fn run_of(text: &str, byte: u8) -> usize {
    text.bytes().take_while(|&b| b == byte).count()
}

/// `startsBlock`: a line that ends a paragraph, a table or a lazy list line.
pub(crate) fn starts_block(line: &str) -> bool {
    fence_open(line).is_some()
        || heading(line).is_some()
        || is_rule(line)
        || is_quote(line)
        || list_item(line).is_some()
}

pub(crate) struct Fence {
    pub(crate) marker: u8,
    pub(crate) length: usize,
    pub(crate) lang: Option<String>,
}

/// FENCE: `/^( {0,3})(`{3,}|~{3,})[ \t]*([^\s`]*)[^`]*$/`
pub(crate) fn fence_open(line: &str) -> Option<Fence> {
    let rest = after_indent(line)?;
    let marker = *rest.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let length = run_of(rest, marker);
    let tail = &rest[length..];
    if length < 3 || tail.contains('`') {
        return None;
    }
    let info: String = tail
        .trim_start_matches([' ', '\t'])
        .chars()
        .take_while(|&character| !is_js_space(character))
        .collect();
    Some(Fence {
        marker,
        length,
        lang: (!info.is_empty()).then(|| info.to_lowercase()),
    })
}

/// The closing fence: `/^ {0,3}(`{3,}|~{3,})[ \t]*$/`, of the opening's
/// character and at least its length.
pub(crate) fn fence_closes(line: &str, marker: u8, length: usize) -> bool {
    after_indent(line).is_some_and(|rest| {
        let run = run_of(rest, marker);
        run >= 3 && run >= length && is_blank(&rest[run..])
    })
}

/// HEADING: `/^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/`, with
/// its text: group 2, the shortest that leaves only a closing `#` run (with
/// space before it) and trailing space.
pub(crate) fn heading(line: &str) -> Option<(u8, String)> {
    let rest = after_indent(line)?;
    let level = run_of(rest, b'#');
    if !(1..=6).contains(&level) {
        return None;
    }
    let tail = &rest[level..];
    if tail.is_empty() {
        return Some((level as u8, String::new()));
    }
    // `.*?` can't cross U+2028 or U+2029, so no part of the pattern matches them.
    if !tail.starts_with([' ', '\t']) || has_js_line_terminator(tail) {
        return None;
    }
    let content = tail
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t']);
    let without_closing = content.trim_end_matches('#');
    let text = if without_closing.len() < content.len() && without_closing.ends_with([' ', '\t']) {
        without_closing.trim_end_matches([' ', '\t'])
    } else {
        content
    };
    Some((level as u8, text.to_owned()))
}

/// RULE: `/^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/`
pub(crate) fn is_rule(line: &str) -> bool {
    let Some(rest) = after_indent(line) else {
        return false;
    };
    let Some(&marker) = rest.as_bytes().first() else {
        return false;
    };
    matches!(marker, b'-' | b'*' | b'_')
        && rest
            .bytes()
            .all(|byte| byte == marker || byte == b' ' || byte == b'\t')
        && rest.bytes().filter(|&byte| byte == marker).count() >= 3
}

/// QUOTE: `/^ {0,3}> ?/`
pub(crate) fn is_quote(line: &str) -> bool {
    after_indent(line).is_some_and(|rest| rest.starts_with('>'))
}

pub(crate) struct ListItem<'a> {
    pub(crate) indent: usize,
    pub(crate) marker: &'a str,
    /// The space after the marker (`match[3]`), when there is any.
    pub(crate) spacing: Option<&'a str>,
    /// The rest of the line (`match[4]`).
    pub(crate) content: &'a str,
}

/// LIST_ITEM: `/^( {0,3})([-*+]|\d{1,9}[.)])(?:([ \t]+)(.*))?$/`
pub(crate) fn list_item(line: &str) -> Option<ListItem<'_>> {
    let indent = leading_spaces(line);
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let marker_length = match rest.as_bytes().first()? {
        b'-' | b'*' | b'+' => 1,
        _ => {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if !(1..=9).contains(&digits)
                || !matches!(rest.as_bytes().get(digits), Some(b'.' | b')'))
            {
                return None;
            }
            digits + 1
        }
    };
    let after = &rest[marker_length..];
    let spaces = after
        .bytes()
        .take_while(|&byte| byte == b' ' || byte == b'\t')
        .count();
    // `(.*)$` can't cross U+2028 or U+2029.
    if (!after.is_empty() && spaces == 0) || has_js_line_terminator(after) {
        return None;
    }
    Some(ListItem {
        indent,
        marker: &rest[..marker_length],
        spacing: (spaces > 0).then(|| &after[..spaces]),
        content: &after[spaces..],
    })
}

/// TASK: `/^\[([ xX])\](?:[ \t]+|$)/`
pub(crate) fn is_task(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'['
        && matches!(bytes[1], b' ' | b'x' | b'X')
        && bytes[2] == b']'
        && matches!(bytes.get(3), None | Some(b' ' | b'\t'))
}

/// parseList's extent: the list's summary and the line after it.
fn list(lines: &[Line<'_>], from: usize) -> (BlockKind, usize) {
    let Some(first) = list_item(lines[from].text) else {
        return (BlockKind::Paragraph, from + 1);
    };
    let ordered = first.marker.as_bytes()[0].is_ascii_digit();
    let kind = if ordered {
        &first.marker[first.marker.len() - 1..]
    } else {
        first.marker
    };
    let start = if ordered {
        first.marker[..first.marker.len() - 1]
            .parse::<u32>()
            .unwrap_or(1)
    } else {
        1
    };
    let (mut items, mut tasks) = (0, 0);
    let mut i = from;
    while i < lines.len() {
        let Some(item) = list_item(lines[i].text) else {
            break;
        };
        let same_kind = if ordered {
            item.marker.as_bytes()[0].is_ascii_digit() && item.marker.ends_with(kind)
        } else {
            item.marker == kind
        };
        if !same_kind || is_rule(lines[i].text) {
            break;
        }
        let spaces = indent_of(item.spacing.unwrap_or(" "));
        let content_indent = item.indent
            + item.marker.len()
            + if spaces > 4 || item.content.is_empty() {
                1
            } else {
                spaces
            };
        // The item's first body line starts with space when `spaces > 4`, so
        // it is then never a task.
        if spaces <= 4 && is_task(item.content) {
            tasks += 1;
        }
        let mut last_blank = is_blank(item.content);
        i += 1;
        while i < lines.len() {
            let next = lines[i].text;
            if is_blank(next) {
                // A blank line stays in the item only when indented content follows.
                let ahead = next_non_blank(lines, i);
                if ahead < lines.len() && indent_of(lines[ahead].text) >= content_indent {
                    i = ahead;
                    last_blank = true;
                    continue;
                }
                break;
            }
            if indent_of(next) >= content_indent {
                last_blank = false;
                i += 1;
                continue;
            }
            // Lazy continuation of the item's paragraph.
            if !starts_block(next) && !last_blank {
                i += 1;
                continue;
            }
            break;
        }
        items += 1;
        // Blank lines between items keep the list going.
        let ahead = next_non_blank(lines, i);
        if ahead < lines.len() && ahead != i && list_item(lines[ahead].text).is_some() {
            i = ahead;
        }
    }
    (
        BlockKind::List {
            ordered,
            start,
            items,
            tasks,
        },
        i,
    )
}

fn next_non_blank(lines: &[Line<'_>], from: usize) -> usize {
    let mut at = from;
    while at < lines.len() && is_blank(lines[at].text) {
        at += 1;
    }
    at
}

/// TABLE_DELIMITER: `/^ {0,3}\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/`
pub(crate) fn is_table_delimiter(line: &str) -> bool {
    let spaces = leading_spaces(line);
    let mut rest = &line[spaces.min(3)..];
    if spaces <= 3 {
        rest = rest.strip_prefix('|').unwrap_or(rest);
    }
    rest = rest.trim_start_matches([' ', '\t']);
    loop {
        rest = rest.strip_prefix(':').unwrap_or(rest);
        let dashes = run_of(rest, b'-');
        if dashes == 0 {
            return false;
        }
        rest = &rest[dashes..];
        rest = rest.strip_prefix(':').unwrap_or(rest);
        rest = rest.trim_start_matches([' ', '\t']);
        match rest.strip_prefix('|') {
            None => return rest.is_empty(),
            Some(after) => {
                rest = after.trim_start_matches([' ', '\t']);
                if rest.is_empty() {
                    return true;
                }
            }
        }
    }
}

/// splitRow's cell count, up to 40: `\|` and pipes in code spans don't split.
fn cell_count(line: &str) -> usize {
    // JavaScript's trim(): it removes a leading U+FEFF and keeps U+0085.
    let mut row = line.trim_matches(is_js_space);
    row = row.strip_prefix('|').unwrap_or(row);
    if row.ends_with('|') && !row.ends_with("\\|") {
        row = &row[..row.len() - 1];
    }
    let bytes = row.as_bytes();
    let (mut cells, mut in_code, mut at) = (1, false, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if bytes.get(at + 1) == Some(&b'|') => at += 1,
            b'`' => in_code = !in_code,
            b'|' if !in_code => cells += 1,
            _ => {}
        }
        at += 1;
    }
    cells.min(MAX_TABLE_COLUMNS)
}

/// A table starting at `from`: its column and body-row counts.
fn table(lines: &[Line<'_>], from: usize) -> Option<(usize, usize)> {
    let head = lines[from].text;
    let delimiter = lines.get(from + 1)?.text;
    if !head.contains('|') || !is_table_delimiter(delimiter) {
        return None;
    }
    let columns = cell_count(head);
    if columns != cell_count(delimiter) {
        return None;
    }
    let rows = lines[from + 2..]
        .iter()
        .take_while(|line| {
            !is_blank(line.text) && line.text.contains('|') && !starts_block(line.text)
        })
        .count();
    Some((columns, rows))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// The guarantee everything else rests on: the blocks are in order,
    /// don't overlap, sit on character boundaries, and only blank space lies
    /// outside them.
    fn assert_lossless(source: &str) -> PageBlocks {
        let page = PageBlocks::parse(source);
        let mut at = 0;
        for block in page.blocks() {
            assert!(
                block.start >= at && block.start < block.end,
                "{block:?} in {source:?}"
            );
            assert!(source.is_char_boundary(block.start) && source.is_char_boundary(block.end));
            assert!(
                source[at..block.start]
                    .bytes()
                    .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n')),
                "non-blank text outside a block before {block:?} in {source:?}"
            );
            at = block.end;
        }
        assert!(
            source[at..]
                .bytes()
                .all(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        );
        page
    }

    /// Each block as the shared fixture describes it.
    fn summary(kind: &BlockKind) -> Value {
        match kind {
            BlockKind::Heading { level, .. } => json!({"type": "heading", "level": level}),
            BlockKind::Paragraph => json!({"type": "paragraph"}),
            BlockKind::Code { lang, .. } => json!({"type": "code", "lang": lang}),
            BlockKind::Quote => json!({"type": "quote"}),
            BlockKind::List {
                ordered,
                start,
                items,
                tasks,
            } => {
                json!({"type": "list", "ordered": ordered, "start": start, "items": items, "tasks": tasks})
            }
            BlockKind::Hr => json!({"type": "hr"}),
            BlockKind::Table { columns, rows } => {
                json!({"type": "table", "columns": columns, "rows": rows})
            }
            BlockKind::Overflow { .. } => json!({"type": "overflow"}),
        }
    }

    /// A fixture case's Markdown: given, or built as `repeat` × `count` + `then`.
    fn fixture_markdown(case: &Value) -> String {
        if let Some(markdown) = case["markdown"].as_str() {
            return markdown.to_owned();
        }
        let generate = &case["generate"];
        let repeat = generate["repeat"].as_str().expect("repeat");
        let count = generate["count"].as_u64().expect("count") as usize;
        repeat.repeat(count) + generate["then"].as_str().unwrap_or_default()
    }

    #[test]
    fn the_blocks_match_the_web_parser_on_the_shared_cases() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../web/src/ui/markdown/blockParity.json"))
                .expect("fixture");
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(cases.len() >= 20, "the fixture lost its cases");
        for case in cases {
            let markdown = fixture_markdown(case);
            let page = assert_lossless(&markdown);
            let got: Vec<Value> = page
                .blocks()
                .iter()
                .map(|block| summary(&block.kind))
                .collect();
            assert_eq!(Value::Array(got), case["blocks"], "{}", case["name"]);
        }
    }

    #[test]
    fn odd_and_unsupported_markdown_is_kept_exactly() {
        for source in [
            "",
            "\n\n  \t\n",
            "no ending",
            "# Title\r\n\r\nWindows\r\nlines\r\n",
            "old\rMac\r\rlines\r",
            "mixed\r\n\n\r\rendings",
            "trailing spaces   \n\ttabs\t\n",
            "<div class=\"x\">\n  <b>html</b>\n</div>\n",
            "[^1]: a footnote\n\n[ref]: https://example.com \"title\"\n",
            "---\ntitle: front matter\n---\n",
            "Setext\n======\n",
            "```\nunclosed fence\n\n\n",
            "- a\n\n\n- b\n\n\n",
            "\u{feff}BOM first",
            "nbsp\u{a0}\u{a0}\n\u{a0}\n",
            "emoji 😀 and 中文 and e\u{301}\n# 🎉 heading\n",
            "| a \\| b | `c | d` |\n|---|---|\n| 1 | 2 |",
            "> quote\n>\n> - list in quote\n\n    indented\n\n\n    code after blanks\n",
        ] {
            assert_lossless(source);
        }
    }

    #[test]
    fn generated_documents_stay_lossless() {
        const PIECES: [&str; 28] = [
            "# h",
            "## h ##",
            "text",
            "more text",
            "",
            "  ",
            "\t",
            "- item",
            "  nested",
            "1. one",
            "2) two",
            "> quote",
            "```rust",
            "~~~",
            "```",
            "    code",
            "| a | b |",
            "|---|---|",
            "---",
            "* * *",
            "[ ] x",
            "- [x] done",
            "😀",
            "a\u{301}",
            "# A\u{2028}B",
            "- A\u{2029}B",
            "\u{feff}| a | b |",
            "```\u{85}X",
        ];
        const ENDINGS: [&str; 3] = ["\n", "\r\n", "\r"];
        // A small fixed generator, so a failure always reproduces.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for _ in 0..2_000 {
            let mut source = String::new();
            for _ in 0..next(30) {
                source.push_str(PIECES[next(PIECES.len())]);
                source.push_str(ENDINGS[next(ENDINGS.len())]);
            }
            assert_lossless(&source);
        }
    }

    #[test]
    fn the_parse_limit_counts_utf16_units_after_line_endings_become_one() {
        // Exactly the limit: nothing overflows.
        let exact = "a".repeat(PARSE_LIMIT);
        let page = assert_lossless(&exact);
        assert_eq!(page.blocks().len(), 1);

        // `\r\n` counts once: 50,000 of them are 100,000 units, so only "b" is past it.
        let crlf = "a\r\n".repeat(PARSE_LIMIT / 2) + "b";
        let page = assert_lossless(&crlf);
        let overflow = page.blocks().last().expect("overflow");
        assert_eq!(
            overflow.kind,
            BlockKind::Overflow {
                limit_inside_character: false
            }
        );
        assert_eq!(&crlf[overflow.start..], "b");

        // A two-unit character across the limit goes wholly past it, and says so.
        let astral = "a".repeat(PARSE_LIMIT - 1) + "😀tail";
        let page = assert_lossless(&astral);
        let overflow = page.blocks().last().expect("overflow");
        assert_eq!(
            overflow.kind,
            BlockKind::Overflow {
                limit_inside_character: true
            }
        );
        assert_eq!(&astral[overflow.start..], "😀tail");

        // One before it fits: then the limit falls between characters.
        let fits = "a".repeat(PARSE_LIMIT - 2) + "😀tail";
        let page = assert_lossless(&fits);
        let overflow = page.blocks().last().expect("overflow");
        assert_eq!(
            overflow.kind,
            BlockKind::Overflow {
                limit_inside_character: false
            }
        );
        assert_eq!(&fits[overflow.start..], "tail");
    }

    #[test]
    fn heading_text_follows_the_web_pattern() {
        for (line, expected) in [
            ("# Title", Some((1, "Title"))),
            ("## Title ##", Some((2, "Title"))),
            ("### Title#", Some((3, "Title#"))),
            ("#", Some((1, ""))),
            ("## #", Some((2, "#"))),
            ("# a # b #", Some((1, "a # b"))),
            ("   ###### six\t", Some((6, "six"))),
            ("    # four spaces", None),
            ("####### seven", None),
            ("#no space", None),
        ] {
            assert_eq!(
                heading(line),
                expected.map(|(level, text)| (level, text.to_owned())),
                "{line:?}"
            );
        }
    }
}
