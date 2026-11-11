//! The web's Markdown, parsed on the server (AWD-53, third slice).
//!
//! A port of `apps/web/src/ui/markdown/parse.ts`: the same blocks and inlines
//! from the same text, so a page the server converts into a document looks
//! exactly as the web drew its Markdown. `parseParity.json`, next to
//! parse.ts, holds the cases both parsers are tested on; the tree it records
//! is this module's [`Block`] serialized as JSON.
//!
//! Where parse.ts counts in UTF-16 code units (its limits), this counts the
//! same way; where it relies on JavaScript's `\s` or `trim()`, this follows
//! JavaScript's character rules. One difference stays: when the parse limit
//! falls inside a character that takes two code units, JavaScript splits the
//! character and this keeps it whole in the overflow text.

use std::collections::HashMap;

use serde::Serialize;

use crate::page_blocks::{
    PARSE_LIMIT, after_indent, fence_closes, fence_open, has_js_line_terminator, heading,
    indent_of, is_blank, is_js_space, is_quote, is_rule, is_table_delimiter, list_item,
    starts_block,
};

const MAX_BLOCK_DEPTH: usize = 8;
const MAX_INLINE_DEPTH: usize = 6;
const MAX_TABLE_COLUMNS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Inline {
    Text {
        value: String,
    },
    Strong {
        children: Vec<Inline>,
    },
    Em {
        children: Vec<Inline>,
    },
    Del {
        children: Vec<Inline>,
    },
    Code {
        value: String,
    },
    Link {
        href: String,
        external: bool,
        children: Vec<Inline>,
    },
    TaskKey {
        key: String,
    },
    Br,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "center" => Some(Self::Center),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListItem {
    pub checked: Option<bool>,
    pub children: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Block {
    Heading {
        level: u8,
        children: Vec<Inline>,
    },
    Paragraph {
        children: Vec<Inline>,
    },
    Code {
        lang: Option<String>,
        value: String,
    },
    Quote {
        children: Vec<Block>,
    },
    List {
        ordered: bool,
        start: u32,
        items: Vec<ListItem>,
    },
    Hr,
    Table {
        align: Vec<Option<Align>>,
        head: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// Source past the parsing limit, shown as it is.
    Overflow {
        value: String,
    },
}

/// `parseMarkdown`: the blocks of a page.
pub fn parse_markdown(source: &str) -> Vec<Block> {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let cut = utf16_cut(&normalized, PARSE_LIMIT);
    let head = &normalized[..cut];
    let lines: Vec<String> = head.split('\n').map(str::to_owned).collect();
    let mut blocks = parse_blocks(&lines, 0);
    if cut < normalized.len() {
        blocks.push(Block::Overflow {
            value: normalized[cut..].to_owned(),
        });
    }
    blocks
}

/// The byte offset where `limit` UTF-16 code units end; a character that
/// straddles the limit goes wholly past it.
fn utf16_cut(text: &str, limit: usize) -> usize {
    let mut units = 0;
    for (at, character) in text.char_indices() {
        units += character.len_utf16();
        if units > limit {
            return at;
        }
    }
    text.len()
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

// ---------------------------------------------------------------------------
// Links

/// `safeHref`: the href for a link destination, or none when it mustn't be a
/// link. Allowed: http:, https:, mailto: and same-origin paths starting with
/// a single `/`. The second value says whether it leaves the app.
pub fn safe_href(raw: &str) -> Option<(String, bool)> {
    let href = raw.trim_matches(is_js_space);
    if href.is_empty() || utf16_len(href) > 2048 {
        return None;
    }
    // Control characters, whitespace and backslashes let browsers reinterpret a URL.
    if href
        .chars()
        .any(|character| character <= ' ' || character == '\u{7f}' || character == '\\')
    {
        return None;
    }
    if let Some(rest) = href.strip_prefix('/') {
        return (!rest.starts_with('/')).then(|| (href.to_owned(), false));
    }
    let scheme = scheme_of(href)?.to_ascii_lowercase();
    if scheme == "mailto" {
        return (utf16_len(href) > 7).then(|| (href.to_owned(), true));
    }
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let url = url::Url::parse(href).ok()?;
    url.host_str()
        .is_some_and(|host| !host.is_empty())
        .then(|| (href.to_owned(), true))
}

/// `/^([a-z][a-z0-9+.-]*):/i`
fn scheme_of(href: &str) -> Option<&str> {
    let colon = href.find(':')?;
    let scheme = &href[..colon];
    let mut characters = scheme.chars();
    let first = characters.next()?;
    (first.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '.' | '-')
        }))
    .then_some(scheme)
}

// ---------------------------------------------------------------------------
// Blocks

/// Remove up to `width` columns of leading whitespace, tabs to four.
fn dedent(line: &str, width: usize) -> String {
    let mut column = 0;
    let mut index = 0;
    for byte in line.bytes() {
        if column >= width {
            break;
        }
        match byte {
            b' ' => column += 1,
            b'\t' => column += 4 - column % 4,
            _ => break,
        }
        index += 1;
    }
    let mut out = String::new();
    if column > width {
        out.push_str(&" ".repeat(column - width));
    }
    out.push_str(&line[index..]);
    out
}

/// `line.replace(QUOTE, '')`, QUOTE being `/^ {0,3}> ?/`.
fn strip_quote(line: &str) -> &str {
    let Some(rest) = after_indent(line) else {
        return line;
    };
    match rest.strip_prefix('>') {
        Some(after) => after.strip_prefix(' ').unwrap_or(after),
        None => line,
    }
}

/// `/^[ \t]+/` removed.
fn trim_leading_blank(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
}

fn parse_blocks(lines: &[String], depth: usize) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].as_str();
        if is_blank(line) {
            i += 1;
            continue;
        }

        if let Some(fence) = fence_open(line) {
            let indent = line.len() - after_indent(line).map_or(line.len(), str::len);
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() {
                if fence_closes(&lines[i], fence.marker, fence.length) {
                    i += 1;
                    break;
                }
                body.push(dedent(&lines[i], indent));
                i += 1;
            }
            blocks.push(Block::Code {
                lang: fence.lang,
                value: body.join("\n"),
            });
            continue;
        }

        if let Some((level, text)) = heading(line) {
            blocks.push(Block::Heading {
                level,
                children: parse_inline(&text),
            });
            i += 1;
            continue;
        }

        if is_rule(line) {
            blocks.push(Block::Hr);
            i += 1;
            continue;
        }

        if depth < MAX_BLOCK_DEPTH && is_quote(line) {
            let mut inner = Vec::new();
            while i < lines.len() && is_quote(&lines[i]) {
                inner.push(strip_quote(&lines[i]).to_owned());
                i += 1;
            }
            blocks.push(Block::Quote {
                children: parse_blocks(&inner, depth + 1),
            });
            continue;
        }

        if depth < MAX_BLOCK_DEPTH && list_item(line).is_some() {
            let (block, next) = parse_list(lines, i, depth);
            blocks.push(block);
            i = next;
            continue;
        }

        if indent_of(line) >= 4 {
            let mut body = Vec::new();
            while i < lines.len() && (indent_of(&lines[i]) >= 4 || is_blank(&lines[i])) {
                body.push(dedent(&lines[i], 4));
                i += 1;
            }
            while body.last().is_some_and(|last| is_blank(last)) {
                body.pop();
            }
            blocks.push(Block::Code {
                lang: None,
                value: body.join("\n"),
            });
            continue;
        }

        if line.contains('|')
            && i + 1 < lines.len()
            && is_table_delimiter(&lines[i + 1])
            && let Some((block, next)) = parse_table(lines, i)
        {
            blocks.push(block);
            i = next;
            continue;
        }

        let mut paragraph = vec![trim_leading_blank(line)];
        i += 1;
        while i < lines.len() && !is_blank(&lines[i]) && !starts_block(&lines[i]) {
            paragraph.push(trim_leading_blank(&lines[i]));
            i += 1;
        }
        blocks.push(Block::Paragraph {
            children: parse_inline(&paragraph.join("\n")),
        });
    }
    blocks
}

/// TASK: `/^\[([ xX])\](?:[ \t]+|$)/`, with whether it's checked and how much
/// of the text it took.
fn task_marker(text: &str) -> Option<(bool, usize)> {
    let bytes = text.as_bytes();
    if bytes.len() < 3 || bytes[0] != b'[' || bytes[2] != b']' {
        return None;
    }
    let checked = match bytes[1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    let spaces = bytes[3..]
        .iter()
        .take_while(|&&byte| byte == b' ' || byte == b'\t')
        .count();
    if spaces == 0 && bytes.len() > 3 {
        return None;
    }
    Some((checked, 3 + spaces))
}

fn parse_list(lines: &[String], from: usize, depth: usize) -> (Block, usize) {
    let Some(first) = list_item(&lines[from]) else {
        return (
            Block::Paragraph {
                children: parse_inline(&lines[from]),
            },
            from + 1,
        );
    };
    let ordered = first.marker.as_bytes()[0].is_ascii_digit();
    let kind = if ordered {
        first.marker[first.marker.len() - 1..].to_owned()
    } else {
        first.marker.to_owned()
    };
    let start = if ordered {
        first.marker[..first.marker.len() - 1]
            .parse::<u32>()
            .unwrap_or(1)
    } else {
        1
    };
    let mut items = Vec::new();
    let mut i = from;
    while i < lines.len() {
        let Some(item) = list_item(&lines[i]) else {
            break;
        };
        let marker = item.marker;
        let same_kind = if ordered {
            marker.as_bytes()[0].is_ascii_digit() && marker.ends_with(kind.as_str())
        } else {
            marker == kind
        };
        if !same_kind || is_rule(&lines[i]) {
            break;
        }
        let spacing = item.spacing.unwrap_or(" ");
        let spaces = indent_of(spacing);
        let content_indent = item.indent
            + marker.len()
            + if spaces > 4 || item.content.is_empty() {
                1
            } else {
                spaces
            };
        let mut body = vec![if spaces > 4 {
            format!("{}{}", dedent(spacing, 1), item.content)
        } else {
            item.content.to_owned()
        }];
        i += 1;
        while i < lines.len() {
            let next = lines[i].as_str();
            if is_blank(next) {
                // A blank line stays in the item only when indented content follows.
                let mut ahead = i;
                while ahead < lines.len() && is_blank(&lines[ahead]) {
                    ahead += 1;
                }
                if ahead < lines.len() && indent_of(&lines[ahead]) >= content_indent {
                    while i < ahead {
                        body.push(String::new());
                        i += 1;
                    }
                    continue;
                }
                break;
            }
            if indent_of(next) >= content_indent {
                body.push(dedent(next, content_indent));
                i += 1;
                continue;
            }
            // Lazy continuation of the item's paragraph.
            if !starts_block(next) && !body.last().is_some_and(|last| is_blank(last)) {
                body.push(trim_leading_blank(next).to_owned());
                i += 1;
                continue;
            }
            break;
        }
        let mut checked = None;
        if let Some((done, taken)) = task_marker(&body[0]) {
            checked = Some(done);
            body[0] = body[0][taken..].to_owned();
        }
        items.push(ListItem {
            checked,
            children: parse_blocks(&body, depth + 1),
        });
        // Blank lines between items keep the list going.
        let mut ahead = i;
        while ahead < lines.len() && is_blank(&lines[ahead]) {
            ahead += 1;
        }
        if ahead < lines.len() && ahead != i && list_item(&lines[ahead]).is_some() {
            i = ahead;
        }
    }
    (
        Block::List {
            ordered,
            start,
            items,
        },
        i,
    )
}

/// `splitRow`: a table line's cells, at most 40.
fn split_row(line: &str) -> Vec<String> {
    let mut row = line.trim_matches(is_js_space);
    row = row.strip_prefix('|').unwrap_or(row);
    if row.ends_with('|') && !row.ends_with("\\|") {
        row = &row[..row.len() - 1];
    }
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut in_code = false;
    let mut characters = row.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\\' && characters.peek() == Some(&'|') {
            cell.push('|');
            characters.next();
        } else if character == '`' {
            in_code = !in_code;
            cell.push(character);
        } else if character == '|' && !in_code {
            cells.push(cell.trim_matches(is_js_space).to_owned());
            cell = String::new();
        } else {
            cell.push(character);
        }
    }
    cells.push(cell.trim_matches(is_js_space).to_owned());
    cells.truncate(MAX_TABLE_COLUMNS);
    cells
}

fn parse_table(lines: &[String], from: usize) -> Option<(Block, usize)> {
    let head = split_row(&lines[from]);
    let delimiter = split_row(&lines[from + 1]);
    if head.len() != delimiter.len() {
        return None;
    }
    let align = delimiter
        .iter()
        .map(|cell| {
            let left = cell.starts_with(':');
            let right = cell.ends_with(':');
            match (left, right) {
                (true, true) => Some(Align::Center),
                (_, true) => Some(Align::Right),
                (true, _) => Some(Align::Left),
                _ => None,
            }
        })
        .collect();
    let mut rows = Vec::new();
    let mut i = from + 2;
    while i < lines.len()
        && !is_blank(&lines[i])
        && lines[i].contains('|')
        && !starts_block(&lines[i])
    {
        let cells = split_row(&lines[i]);
        rows.push(
            (0..head.len())
                .map(|column| parse_inline(cells.get(column).map_or("", String::as_str)))
                .collect(),
        );
        i += 1;
    }
    Some((
        Block::Table {
            align,
            head: head.iter().map(|cell| parse_inline(cell)).collect(),
            rows,
        },
        i,
    ))
}

// ---------------------------------------------------------------------------
// Inlines

/// Common PROJECT-123 shaped words that aren't task keys.
const NOT_TASK_KEYS: &[&str] = &[
    "UTF", "ISO", "SHA", "RFC", "GPT", "COVID", "CVE", "MD", "X", "TLS", "SSL", "P", "H", "ES",
];

/// `PUNCTUATION`: `/[!-/:-@[-`{-~]/`
fn is_punctuation(character: char) -> bool {
    character.is_ascii_punctuation()
}

/// `isWordChar`: `/[A-Za-z0-9_]/`
pub(crate) fn is_word(character: Option<char>) -> bool {
    character.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// `isSpace`: nothing there, or JavaScript's `\s`.
pub(crate) fn is_space(character: Option<char>) -> bool {
    character.is_none_or(is_js_space)
}

/// A paragraph's text as parse.ts indexes it: characters, with where each
/// starts in UTF-16 code units for the limits it counts in those.
struct Text {
    chars: Vec<char>,
    units: Vec<usize>,
}

impl Text {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let mut units = Vec::with_capacity(chars.len() + 1);
        let mut at = 0;
        for character in &chars {
            units.push(at);
            at += character.len_utf16();
        }
        units.push(at);
        Self { chars, units }
    }

    fn len(&self) -> usize {
        self.chars.len()
    }

    fn at(&self, index: usize) -> Option<char> {
        self.chars.get(index).copied()
    }

    fn before(&self, index: usize) -> Option<char> {
        index.checked_sub(1).and_then(|index| self.at(index))
    }

    fn slice(&self, from: usize, to: usize) -> String {
        self.chars[from..to].iter().collect()
    }

    /// `text.indexOf(character, from)`
    fn find(&self, character: char, from: usize) -> Option<usize> {
        (from..self.len()).find(|&index| self.chars[index] == character)
    }

    /// `text.indexOf(needle, from)`, for a run of one character.
    fn find_run(&self, character: char, length: usize, from: usize) -> Option<usize> {
        let mut index = from;
        while index + length <= self.len() {
            if self.chars[index..index + length]
                .iter()
                .all(|&c| c == character)
            {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    fn run(&self, index: usize, character: char) -> usize {
        self.chars[index..]
            .iter()
            .take_while(|&&c| c == character)
            .count()
    }

    /// The character index at or after `units` code units, for a limit
    /// counted in those.
    fn index_at_units(&self, units: usize) -> usize {
        self.units.partition_point(|&at| at < units).min(self.len())
    }
}

struct Context {
    /// Closer searches that already failed, keyed by marker; positions past
    /// this have none either.
    no_closer: HashMap<String, usize>,
}

/// `parseInline`
pub fn parse_inline(text: &str) -> Vec<Inline> {
    let text = Text::new(text);
    // Characters the link scanner may still look at; shared across the paragraph.
    let mut budget: isize = 200_000;
    inline_run(
        &text,
        0,
        false,
        &mut Context {
            no_closer: HashMap::new(),
        },
        &mut budget,
    )
}

fn push_text(out: &mut Vec<Inline>, value: &str) {
    if value.is_empty() {
        return;
    }
    if let Some(Inline::Text { value: last }) = out.last_mut() {
        last.push_str(value);
    } else {
        out.push(Inline::Text {
            value: value.to_owned(),
        });
    }
}

/// `findCloser`: a closing run of exactly `marker` at or after `from`.
fn find_closer(
    text: &Text,
    from: usize,
    character: char,
    length: usize,
    context: &mut Context,
) -> Option<usize> {
    let key: String = std::iter::repeat_n(character, length).collect();
    let failed_at = context.no_closer.get(&key).copied();
    if failed_at.is_some_and(|failed| from >= failed) {
        return None;
    }
    let mut index = from;
    while index < text.len() {
        let Some(found) = text.find(character, index) else {
            break;
        };
        if text.before(found) == Some('\\') {
            index = found + 1;
            continue;
        }
        let end = found + text.run(found, character);
        let run = end - found;
        let valid = run == length
            && !is_space(text.before(found))
            && found > from
            && (character != '_' || !is_word(text.at(end)));
        if valid {
            return Some(found);
        }
        index = end;
    }
    context
        .no_closer
        .insert(key, failed_at.map_or(from, |failed| failed.min(from)));
    None
}

fn inline_run(
    text: &Text,
    depth: usize,
    in_link: bool,
    context: &mut Context,
    budget: &mut isize,
) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut index = 0;
    let mut plain = String::new();
    macro_rules! flush {
        () => {
            push_text(&mut out, &plain);
            plain.clear();
        };
    }
    while index < text.len() {
        let character = text.chars[index];

        if character == '\\' {
            match text.at(index + 1) {
                Some('\n') => {
                    flush!();
                    out.push(Inline::Br);
                    index += 2;
                    continue;
                }
                Some(next) if is_punctuation(next) => {
                    plain.push(next);
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }

        if character == '\n' {
            let trailing = plain.len() - plain.trim_end_matches(' ').len();
            let hard = trailing >= 2;
            let kept = plain.trim_end_matches([' ', '\t']).len();
            plain.truncate(kept);
            if hard {
                flush!();
                out.push(Inline::Br);
            } else {
                plain.push('\n');
            }
            index += 1;
            continue;
        }

        if character == '`' {
            let ticks = text.run(index, '`');
            let end = index + ticks;
            let key: String = "`".repeat(ticks);
            let failed_at = context.no_closer.get(&key).copied();
            let mut close = if failed_at.is_some_and(|failed| end >= failed) {
                None
            } else {
                text.find_run('`', ticks, end)
            };
            while let Some(at) = close {
                if text.at(at + ticks) != Some('`') {
                    break;
                }
                let skip = at + text.run(at, '`');
                close = text.find_run('`', ticks, skip);
            }
            if close.is_none() && failed_at.is_none() {
                context.no_closer.insert(key.clone(), end);
            }
            if let Some(close) = close {
                flush!();
                let mut value = text.slice(end, close).replace('\n', " ");
                if utf16_len(&value) > 2 && value.starts_with(' ') && value.ends_with(' ') {
                    value = value[1..value.len() - 1].to_owned();
                }
                out.push(Inline::Code { value });
                index = close + ticks;
            } else {
                plain.push_str(&key);
                index = end;
            }
            continue;
        }

        if character == '[' && !in_link && depth < MAX_INLINE_DEPTH && *budget > 0 {
            if let Some(link) = read_link(text, index, budget) {
                flush!();
                let label = Text::new(&link.label);
                let children = inline_run(
                    &label,
                    depth + 1,
                    true,
                    &mut Context {
                        no_closer: HashMap::new(),
                    },
                    budget,
                );
                match safe_href(&link.destination) {
                    Some((href, external)) => out.push(Inline::Link {
                        href,
                        external,
                        children,
                    }),
                    None => out.extend(children),
                }
                index = link.end;
                continue;
            }
        }

        if character == '<' && !in_link {
            let close = text.find('>', index);
            let inside = match close {
                Some(close) if close > index => text.slice(index + 1, close),
                _ => String::new(),
            };
            if let Some(close) = close
                && is_angle_autolink(&inside)
                && let Some((href, external)) = safe_href(&inside)
            {
                flush!();
                out.push(Inline::Link {
                    href,
                    external,
                    children: vec![Inline::Text { value: inside }],
                });
                index = close + 1;
                continue;
            }
        }

        if (character == 'h' || character == 'H') && !in_link && !is_word(text.before(index)) {
            if let Some(url) = bare_url(text, index)
                && let Some((href, external)) = safe_href(&url)
            {
                flush!();
                let length = url.chars().count();
                out.push(Inline::Link {
                    href,
                    external,
                    children: vec![Inline::Text { value: url }],
                });
                index += length;
                continue;
            }
        }

        if character.is_ascii_uppercase()
            && !in_link
            && !text.before(index).unwrap_or(' ').is_ascii_alphanumeric()
            && !matches!(text.before(index), Some('_' | '-'))
        {
            if let Some(length) = task_key(text, index) {
                let key = text.slice(index, index + length);
                let project = &key[..key.find('-').unwrap_or(key.len())];
                if !NOT_TASK_KEYS.contains(&project) {
                    flush!();
                    out.push(Inline::TaskKey { key });
                    index += length;
                    continue;
                }
            }
        }

        if matches!(character, '*' | '_' | '~') && depth < MAX_INLINE_DEPTH {
            let run = text.run(index, character);
            let end = index + run;
            let opens_word =
                !is_space(text.at(end)) && (character != '_' || !is_word(text.before(index)));
            let usable = if character == '~' { run == 2 } else { run <= 3 };
            if opens_word && usable {
                if let Some(close) = find_closer(text, end, character, run, context) {
                    flush!();
                    let inner_text = Text::new(&text.slice(end, close));
                    let inner = inline_run(
                        &inner_text,
                        depth + 1,
                        in_link,
                        &mut Context {
                            no_closer: HashMap::new(),
                        },
                        budget,
                    );
                    out.push(match (character, run) {
                        ('~', _) => Inline::Del { children: inner },
                        (_, 1) => Inline::Em { children: inner },
                        (_, 2) => Inline::Strong { children: inner },
                        _ => Inline::Strong {
                            children: vec![Inline::Em { children: inner }],
                        },
                    });
                    index = close + run;
                    continue;
                }
            }
            plain.push_str(&text.slice(index, end));
            index = end;
            continue;
        }

        plain.push(character);
        index += 1;
    }
    flush!();
    out
}

/// `/^(?:https?:\/\/|mailto:)[^\s<>]+$/i`
pub(crate) fn is_angle_autolink(inside: &str) -> bool {
    let lower = inside.to_ascii_lowercase();
    let rest_at = ["https://", "http://", "mailto:"]
        .iter()
        .find(|prefix| lower.starts_with(**prefix))
        .map(|prefix| prefix.len());
    let Some(rest_at) = rest_at else {
        return false;
    };
    let rest = &inside[rest_at..];
    !rest.is_empty()
        && rest
            .chars()
            .all(|character| !is_js_space(character) && character != '<' && character != '>')
}

/// The bare link the web finds at the start of `text`, if any.
pub(crate) fn bare_url_at_start(text: &str) -> Option<String> {
    bare_url(&Text::new(text), 0)
}

/// AUTOLINK (`/https?:\/\/[^\s<>]+/iy`) at `index`, with trailing punctuation
/// and unbalanced closing parentheses taken off.
fn bare_url(text: &Text, index: usize) -> Option<String> {
    let lower = |at: usize| text.at(at).map(|character| character.to_ascii_lowercase());
    let mut at = index;
    for expected in ['h', 't', 't', 'p'] {
        if lower(at) != Some(expected) {
            return None;
        }
        at += 1;
    }
    if lower(at) == Some('s') {
        at += 1;
    }
    for expected in [':', '/', '/'] {
        if text.at(at) != Some(expected) {
            return None;
        }
        at += 1;
    }
    let rest = text.chars[at..]
        .iter()
        .take_while(|&&character| !is_js_space(character) && character != '<' && character != '>')
        .count();
    if rest == 0 {
        return None;
    }
    let mut url = text.slice(index, at + rest);
    let kept = url
        .trim_end_matches(['.', ',', ':', ';', '!', '?', '\'', '"', '*', '_', '~'])
        .len();
    url.truncate(kept);
    while url.ends_with(')') && url.matches(')').count() > url.matches('(').count() {
        url.pop();
    }
    Some(url)
}

/// TASK_KEY (`/[A-Z][A-Z0-9]{1,9}-[0-9]{1,9}(?![A-Za-z0-9_-])/y`): the
/// length of the key at `index`.
fn task_key(text: &Text, index: usize) -> Option<usize> {
    if !text.at(index)?.is_ascii_uppercase() {
        return None;
    }
    let prefix = text.chars[index + 1..]
        .iter()
        .take_while(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
        .count();
    if !(1..=9).contains(&prefix) || text.at(index + 1 + prefix) != Some('-') {
        return None;
    }
    let digits_at = index + 2 + prefix;
    let digits = text.chars[digits_at.min(text.len())..]
        .iter()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if !(1..=9).contains(&digits) {
        return None;
    }
    let after = text.at(digits_at + digits);
    if after.is_some_and(|character| {
        character.is_ascii_alphanumeric() || character == '_' || character == '-'
    }) {
        return None;
    }
    Some(prefix + 2 + digits)
}

struct LinkParts {
    label: String,
    destination: String,
    end: usize,
}

/// `readLink`: `[label](destination "title")` starting at `[`.
fn read_link(text: &Text, start: usize, budget: &mut isize) -> Option<LinkParts> {
    let mut depth = 0usize;
    let mut index = start;
    let mut label_end = None;
    // Bounded scan so a long run of `[` can't make this quadratic in practice.
    let limit = text.index_at_units(text.units[start] + 2000);
    while index < limit {
        match text.chars[index] {
            '\\' => {
                index += 2;
                continue;
            }
            '[' => depth += 1,
            ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    label_end = Some(index);
                    break;
                }
            }
            _ => {}
        }
        index += 1;
    }
    // An escape at the very end steps one past the text, as it does in parse.ts.
    let end_units = match text.units.get(index) {
        Some(&units) => units,
        None => text.units[text.len()] + (index - text.len()),
    };
    *budget -= isize::try_from(end_units - text.units[start]).unwrap_or(isize::MAX);
    let label_end = label_end?;
    if text.at(label_end + 1) != Some('(') {
        return None;
    }
    let destination_limit = text.index_at_units(text.units[label_end] + 2050);
    let mut parens = 0usize;
    let mut cursor = label_end + 2;
    while cursor < destination_limit {
        match text.chars[cursor] {
            '\n' => return None,
            '(' => parens += 1,
            ')' => {
                if parens == 0 {
                    let inside = text.slice(label_end + 2, cursor);
                    let inside = inside.trim_matches(is_js_space);
                    let destination = strip_link_title(inside);
                    let destination = destination
                        .strip_prefix('<')
                        .and_then(|rest| rest.strip_suffix('>'))
                        .filter(|rest| !has_js_line_terminator(rest))
                        .unwrap_or(destination);
                    return Some(LinkParts {
                        label: text.slice(start + 1, label_end),
                        destination: destination.to_owned(),
                        end: cursor + 1,
                    });
                }
                parens -= 1;
            }
            _ => {}
        }
        cursor += 1;
    }
    None
}

/// `inside.replace(/\s+(?:"[^"]*"|'[^']*')$/, '')`: a title at the end, with
/// space before it, comes off.
fn strip_link_title(inside: &str) -> &str {
    for quote in ['"', '\''] {
        if !inside.ends_with(quote) || inside.len() < 2 {
            continue;
        }
        let body = &inside[..inside.len() - 1];
        let Some(open) = body.rfind(quote) else {
            continue;
        };
        let before = &inside[..open];
        let kept = before.trim_end_matches(is_js_space);
        if kept.len() < before.len() {
            return kept;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn parses_like_the_web() {
        let parity: Value = serde_json::from_str(include_str!(
            "../../../web/src/ui/markdown/parseParity.json"
        ))
        .expect("parseParity.json is JSON");
        let cases = parity["cases"].as_array().expect("cases");
        assert!(cases.len() >= 60);
        let mut failures = Vec::new();
        for case in cases {
            let markdown = case["markdown"].as_str().expect("markdown");
            let blocks = serde_json::to_value(parse_markdown(markdown)).expect("blocks are JSON");
            if blocks != case["blocks"] {
                failures.push(format!(
                    "{}\n  web:    {}\n  server: {}",
                    case["name"], case["blocks"], blocks
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn hrefs_are_checked_like_the_web() {
        assert_eq!(
            safe_href(" https://example.com/a "),
            Some(("https://example.com/a".to_owned(), true))
        );
        assert_eq!(safe_href("/tasks"), Some(("/tasks".to_owned(), false)));
        assert_eq!(safe_href("//evil.com"), None);
        assert_eq!(safe_href("javascript:alert(1)"), None);
        assert_eq!(safe_href("mailto:"), None);
        assert_eq!(safe_href("https://a b"), None);
        assert_eq!(safe_href("https://"), None);
        assert_eq!(safe_href("tasks"), None);
    }

    #[test]
    fn text_past_the_limit_is_overflow() {
        let source = format!("{}tail", "a".repeat(PARSE_LIMIT));
        let blocks = parse_markdown(&source);
        assert_eq!(
            blocks.last(),
            Some(&Block::Overflow {
                value: "tail".to_owned()
            })
        );
    }
}
