//! A page as a document tree: the blocks and marks the web editor's schema
//! holds (Tiptap's names: paragraph, heading, codeBlock, blockquote,
//! bulletList, orderedList, listItem, horizontalRule, table, embed; bold,
//! italic, strike, code, link), with the lasting `id` each top-level block
//! carries.
//!
//! [`from_markdown`] builds a tree from Markdown exactly as the web draws it
//! (through [`super::markdown`]), and [`canon_blocks`] puts a tree in the
//! form its Markdown can say: the derived Markdown ([`super::write`]) is
//! checked by reading it back into this form.

use super::markdown::{self, Align, parse_markdown, safe_href};
use crate::page_blocks::is_js_space;

/// The marks on a run of text. Code is a mark too, as in the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub link: Option<String>,
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        !self.bold && !self.italic && !self.strike && !self.code && self.link.is_none()
    }

    fn intersect(&self, other: &Self) -> Self {
        Self {
            bold: self.bold && other.bold,
            italic: self.italic && other.italic,
            strike: self.strike && other.strike,
            code: self.code && other.code,
            link: if self.link == other.link {
                self.link.clone()
            } else {
                None
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Inline {
    Text { text: String, marks: Marks },
    HardBreak,
}

impl Inline {
    pub fn text(text: impl Into<String>, marks: Marks) -> Self {
        Self::Text {
            text: text.into(),
            marks,
        }
    }
}

/// A block, with its lasting id when it has one (top-level blocks always do
/// once they're in a stored document).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Block {
    pub id: Option<String>,
    pub node: Node,
}

impl Block {
    pub fn new(node: Node) -> Self {
        Self { id: None, node }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Paragraph(Vec<Inline>),
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    Blockquote(Vec<Block>),
    BulletList(Vec<Item>),
    OrderedList {
        start: u32,
        items: Vec<Item>,
    },
    HorizontalRule,
    /// Rows of cells; Markdown's tables have the header first.
    Table(Vec<Vec<Cell>>),
    /// Something of the app's shown live in the page, such as a
    /// collection's view: a same-origin path, and a label for while it loads
    /// and wherever it can't show. Only ever a top-level block, as its
    /// Markdown (`[label](/path "embed")`) is.
    Embed {
        href: String,
        label: String,
    },
}

impl Node {
    /// The editor's name for the node.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Paragraph(_) => "paragraph",
            Self::Heading { .. } => "heading",
            Self::CodeBlock { .. } => "codeBlock",
            Self::Blockquote(_) => "blockquote",
            Self::BulletList(_) => "bulletList",
            Self::OrderedList { .. } => "orderedList",
            Self::HorizontalRule => "horizontalRule",
            Self::Table(_) => "table",
            Self::Embed { .. } => "embed",
        }
    }
}

/// A list item; `checked` makes it a checklist item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Item {
    pub checked: Option<bool>,
    pub content: Vec<Block>,
}

/// A table cell: one paragraph of inline content.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    pub header: bool,
    pub align: Option<Align>,
    pub colspan: u32,
    pub rowspan: u32,
    /// The id on the cell's paragraph, when it has one.
    pub paragraph_id: Option<String>,
    pub content: Vec<Inline>,
}

impl Cell {
    pub fn new(header: bool, align: Option<Align>, content: Vec<Inline>) -> Self {
        Self {
            header,
            align,
            colspan: 1,
            rowspan: 1,
            paragraph_id: None,
            content,
        }
    }
}

/// The deepest lists and quotes nest in the web's Markdown.
pub const MAX_BLOCK_DEPTH: usize = 8;
/// The most columns a Markdown table keeps.
pub const MAX_TABLE_COLUMNS: usize = 40;
/// The largest number an ordered list can start at in Markdown.
pub const MAX_LIST_START: u32 = 999_999_999;

// ---------------------------------------------------------------------------
// From Markdown

/// The blocks of `source`, as the web draws them, in canonical form and
/// without ids.
pub fn from_markdown(source: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    for block in &parse_markdown(source) {
        convert_block(block, &mut blocks);
    }
    canon_blocks(&blocks)
}

fn convert_all(blocks: &[markdown::Block]) -> Vec<Block> {
    let mut out = Vec::new();
    for block in blocks {
        convert_block(block, &mut out);
    }
    out
}

fn convert_block(block: &markdown::Block, out: &mut Vec<Block>) {
    let node = match block {
        markdown::Block::Heading { level, children } => Node::Heading {
            level: *level,
            content: convert_inlines(children),
        },
        markdown::Block::Paragraph { children } => Node::Paragraph(convert_inlines(children)),
        markdown::Block::Code { lang, value } => Node::CodeBlock {
            language: lang.clone(),
            text: value.clone(),
        },
        markdown::Block::Quote { children } => Node::Blockquote(convert_all(children)),
        markdown::Block::List {
            ordered,
            start,
            items,
        } => {
            let items = items
                .iter()
                .map(|item| Item {
                    checked: item.checked,
                    content: convert_all(&item.children),
                })
                .collect();
            if *ordered {
                Node::OrderedList {
                    start: *start,
                    items,
                }
            } else {
                Node::BulletList(items)
            }
        }
        markdown::Block::Hr => Node::HorizontalRule,
        markdown::Block::Table { align, head, rows } => {
            let mut table = vec![
                head.iter()
                    .enumerate()
                    .map(|(column, cell)| {
                        Cell::new(
                            true,
                            align.get(column).copied().flatten(),
                            convert_inlines(cell),
                        )
                    })
                    .collect::<Vec<_>>(),
            ];
            for row in rows {
                table.push(
                    row.iter()
                        .enumerate()
                        .map(|(column, cell)| {
                            Cell::new(
                                false,
                                align.get(column).copied().flatten(),
                                convert_inlines(cell),
                            )
                        })
                        .collect(),
                );
            }
            Node::Table(table)
        }
        markdown::Block::Embed { href, label } => Node::Embed {
            href: href.clone(),
            label: label.clone(),
        },
        // The text past the web's parsing limit, which it shows as it is:
        // paragraphs of plain lines.
        markdown::Block::Overflow { value } => {
            let mut lines: Vec<Inline> = Vec::new();
            for line in value.split('\n') {
                if line.trim_matches(is_js_space).is_empty() {
                    if !lines.is_empty() {
                        out.push(Block::new(Node::Paragraph(std::mem::take(&mut lines))));
                    }
                    continue;
                }
                if !lines.is_empty() {
                    lines.push(Inline::HardBreak);
                }
                lines.push(Inline::text(line, Marks::default()));
            }
            if !lines.is_empty() {
                out.push(Block::new(Node::Paragraph(lines)));
            }
            return;
        }
    };
    out.push(Block::new(node));
}

fn convert_inlines(children: &[markdown::Inline]) -> Vec<Inline> {
    let mut out = Vec::new();
    collect_inlines(children, &Marks::default(), &mut out);
    out
}

fn collect_inlines(children: &[markdown::Inline], marks: &Marks, out: &mut Vec<Inline>) {
    for child in children {
        match child {
            markdown::Inline::Text { value } => out.push(Inline::text(value, marks.clone())),
            markdown::Inline::Strong { children } => collect_inlines(
                children,
                &Marks {
                    bold: true,
                    ..marks.clone()
                },
                out,
            ),
            markdown::Inline::Em { children } => collect_inlines(
                children,
                &Marks {
                    italic: true,
                    ..marks.clone()
                },
                out,
            ),
            markdown::Inline::Del { children } => collect_inlines(
                children,
                &Marks {
                    strike: true,
                    ..marks.clone()
                },
                out,
            ),
            markdown::Inline::Code { value } => out.push(Inline::text(
                value,
                Marks {
                    code: true,
                    ..marks.clone()
                },
            )),
            markdown::Inline::Link { href, children, .. } => collect_inlines(
                children,
                &Marks {
                    link: Some(href.clone()),
                    ..marks.clone()
                },
                out,
            ),
            markdown::Inline::TaskKey { key } => out.push(Inline::text(key, marks.clone())),
            markdown::Inline::Br => out.push(Inline::HardBreak),
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical form

/// Where inline content sits, which changes what Markdown can say there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineContext {
    /// Lines, with hard breaks between them.
    Paragraph,
    /// One line, trimmed of spaces and tabs.
    Heading,
    /// One line, trimmed of all whitespace.
    Cell,
}

/// `blocks` as their Markdown can say them: empty paragraphs gone, text
/// normalized the way Markdown reads it (no spaces around line breaks, no
/// marks on the whitespace at a mark's edges), tables rectangular. Ids stay.
pub fn canon_blocks(blocks: &[Block]) -> Vec<Block> {
    canon_blocks_in(blocks, false)
}

/// `in_list`: inside a list item, where a line of only spaces and tabs is a
/// blank line, so code there can't keep one.
fn canon_blocks_in(blocks: &[Block], in_list: bool) -> Vec<Block> {
    let mut out = Vec::new();
    for block in blocks {
        let node = match &block.node {
            Node::Paragraph(content) => {
                let content = canon_inline(content, InlineContext::Paragraph);
                if content.is_empty() {
                    continue;
                }
                Node::Paragraph(content)
            }
            Node::Heading { level, content } => Node::Heading {
                level: (*level).clamp(1, 6),
                content: canon_inline(content, InlineContext::Heading),
            },
            Node::CodeBlock { language, text } => Node::CodeBlock {
                language: canon_language(language.as_deref()),
                text: {
                    let text = canon_code(text);
                    if in_list {
                        text.split('\n')
                            .map(|line| {
                                if line.chars().all(is_tab_or_space) {
                                    ""
                                } else {
                                    line
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        text
                    }
                },
            },
            // A quote's lines start with `>`, so none is blank.
            Node::Blockquote(children) => Node::Blockquote(canon_blocks_in(children, false)),
            Node::BulletList(items) => Node::BulletList(canon_items(items)),
            Node::OrderedList { start, items } => Node::OrderedList {
                start: (*start).min(MAX_LIST_START),
                items: canon_items(items),
            },
            Node::HorizontalRule => Node::HorizontalRule,
            Node::Table(rows) => match canon_table(rows) {
                Some(rows) => Node::Table(rows),
                None => continue,
            },
            // A label is one line of text.
            Node::Embed { href, label } => Node::Embed {
                href: href.clone(),
                label: label
                    .replace("\r\n", " ")
                    .replace(['\r', '\n'], " ")
                    .replace('\0', "\u{fffd}"),
            },
        };
        out.push(Block {
            id: block.id.clone(),
            node,
        });
    }
    out
}

fn canon_items(items: &[Item]) -> Vec<Item> {
    items
        .iter()
        .map(|item| Item {
            checked: item.checked,
            content: canon_blocks_in(&item.content, true),
        })
        .collect()
}

/// A fence's info word, as the web reads it: the first word, lowercased.
pub(crate) fn canon_language(language: Option<&str>) -> Option<String> {
    let word: String = language?
        .trim_start_matches([' ', '\t'])
        .chars()
        .take_while(|&character| !is_js_space(character))
        .collect();
    (!word.is_empty() && !word.contains('`')).then(|| word.to_lowercase())
}

pub(crate) fn canon_code(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "\u{fffd}")
}

fn canon_table(rows: &[Vec<Cell>]) -> Option<Vec<Vec<Cell>>> {
    // A cell spanning columns stands for itself and empty cells after it.
    let expanded: Vec<Vec<Option<&Cell>>> = rows
        .iter()
        .map(|row| {
            let mut cells = Vec::new();
            for cell in row {
                cells.push(Some(cell));
                for _ in 1..cell.colspan.clamp(1, MAX_TABLE_COLUMNS as u32) {
                    cells.push(None);
                }
            }
            cells
        })
        .collect();
    let columns = expanded.first()?.len().min(MAX_TABLE_COLUMNS);
    if columns == 0 {
        return None;
    }
    let align: Vec<Option<Align>> = expanded[0]
        .iter()
        .take(columns)
        .map(|cell| cell.and_then(|cell| cell.align))
        .collect();
    Some(
        expanded
            .iter()
            .enumerate()
            .map(|(row, cells)| {
                (0..columns)
                    .map(|column| {
                        let source = cells.get(column).copied().flatten();
                        Cell {
                            header: row == 0,
                            align: align[column],
                            colspan: 1,
                            rowspan: 1,
                            paragraph_id: source.and_then(|cell| cell.paragraph_id.clone()),
                            content: source.map_or_else(Vec::new, |cell| {
                                canon_inline(&cell.content, InlineContext::Cell)
                            }),
                        }
                    })
                    .collect()
            })
            .collect(),
    )
}

fn canon_marks(marks: &Marks) -> Marks {
    Marks {
        link: marks.link.as_deref().and_then(canon_href),
        ..marks.clone()
    }
}

/// A link's address as Markdown can hold it: checked like the web checks
/// it, and with its parentheses percent-encoded (which browsers read the
/// same) when they don't pair up, since `(…)` around it must.
pub(crate) fn canon_href(href: &str) -> Option<String> {
    let (href, _) = safe_href(href)?;
    let mut depth = 0usize;
    for character in href.chars() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => {
                depth = usize::MAX;
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    Some(if depth == 0 {
        href
    } else {
        href.replace('(', "%28").replace(')', "%29")
    })
}

/// Adjacent text with the same marks joined; empty text dropped.
pub(crate) fn merge_inline(items: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Inline::Text { text, marks } => {
                if text.is_empty() {
                    continue;
                }
                if let Some(Inline::Text {
                    text: last,
                    marks: last_marks,
                }) = out.last_mut()
                    && *last_marks == marks
                {
                    last.push_str(&text);
                    continue;
                }
                out.push(Inline::Text { text, marks });
            }
            Inline::HardBreak => out.push(Inline::HardBreak),
        }
    }
    out
}

fn is_tab_or_space(character: char) -> bool {
    character == ' ' || character == '\t'
}

/// Takes `matches` characters off the start (or end) of the text items,
/// walking past items it empties, until one keeps something or a break.
fn trim_items(items: &mut Vec<Inline>, from: usize, forward: bool, matches: fn(char) -> bool) {
    let mut index = from;
    loop {
        let Some(Inline::Text { text, .. }) = items.get_mut(index) else {
            return;
        };
        let kept = if forward {
            text.trim_start_matches(matches).to_owned()
        } else {
            text.trim_end_matches(matches).to_owned()
        };
        let emptied = kept.is_empty();
        *text = kept;
        if !emptied {
            return;
        }
        items.remove(index);
        if forward {
            if index >= items.len() {
                return;
            }
        } else {
            if index == 0 {
                return;
            }
            index -= 1;
        }
    }
}

pub(crate) fn canon_inline(content: &[Inline], context: InlineContext) -> Vec<Inline> {
    let mut items = Vec::new();
    for inline in content {
        match inline {
            Inline::HardBreak => items.push(Inline::HardBreak),
            Inline::Text { text, marks } => {
                let marks = canon_marks(marks);
                let text = text
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
                    .replace('\0', "\u{fffd}");
                for (index, part) in text.split('\n').enumerate() {
                    if index > 0 {
                        items.push(Inline::HardBreak);
                    }
                    items.push(Inline::text(part, marks.clone()));
                }
            }
        }
    }
    let mut items = merge_inline(items);
    if context != InlineContext::Paragraph {
        // One line: a break is a space, marked as both sides agree.
        for index in 0..items.len() {
            if items[index] != Inline::HardBreak {
                continue;
            }
            let side = |at: Option<usize>| match at.and_then(|at| items.get(at)) {
                Some(Inline::Text { marks, .. }) => Some(marks.clone()),
                _ => None,
            };
            let marks = match (side(index.checked_sub(1)), side(Some(index + 1))) {
                (Some(before), Some(after)) => before.intersect(&after),
                _ => Marks::default(),
            };
            items[index] = Inline::text(" ", marks);
        }
        items = merge_inline(items);
    }
    match context {
        InlineContext::Paragraph => {
            while items.last() == Some(&Inline::HardBreak) {
                items.pop();
            }
            trim_items(&mut items, 0, true, is_tab_or_space);
            // Spaces and tabs next to a break don't survive Markdown.
            let mut index = 0;
            while index < items.len() {
                if items[index] == Inline::HardBreak {
                    if index + 1 < items.len() {
                        trim_items(&mut items, index + 1, true, is_tab_or_space);
                    }
                    if index > 0 {
                        let before = items.len();
                        trim_items(&mut items, index - 1, false, is_tab_or_space);
                        index -= before - items.len();
                    }
                }
                index += 1;
            }
            while items.last() == Some(&Inline::HardBreak) {
                items.pop();
            }
        }
        InlineContext::Heading => {
            trim_items(&mut items, 0, true, is_tab_or_space);
            if !items.is_empty() {
                let last = items.len() - 1;
                trim_items(&mut items, last, false, is_tab_or_space);
            }
        }
        InlineContext::Cell => {
            trim_items(&mut items, 0, true, is_js_space);
            if !items.is_empty() {
                let last = items.len() - 1;
                trim_items(&mut items, last, false, is_js_space);
            }
        }
    }
    for mark in [MarkKind::Bold, MarkKind::Italic, MarkKind::Strike] {
        items = strip_edge_whitespace(items, mark);
    }
    merge_inline(items)
}

#[derive(Clone, Copy)]
enum MarkKind {
    Bold,
    Italic,
    Strike,
}

impl MarkKind {
    fn on(self, marks: &Marks) -> bool {
        match self {
            Self::Bold => marks.bold,
            Self::Italic => marks.italic,
            Self::Strike => marks.strike,
        }
    }

    fn without(self, marks: &Marks) -> Marks {
        let mut marks = marks.clone();
        match self {
            Self::Bold => marks.bold = false,
            Self::Italic => marks.italic = false,
            Self::Strike => marks.strike = false,
        }
        marks
    }
}

/// Emphasis can't open before whitespace or close after it, so the
/// whitespace at the edges of each run of `mark` goes without it.
fn strip_edge_whitespace(items: Vec<Inline>, mark: MarkKind) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(items.len());
    let mut index = 0;
    while index < items.len() {
        let in_run = |at: usize| matches!(&items[at], Inline::Text { marks, .. } if mark.on(marks));
        if !in_run(index) {
            out.push(items[index].clone());
            index += 1;
            continue;
        }
        let start = index;
        while index < items.len() && in_run(index) {
            index += 1;
        }
        // The run's text, character by character, keeps its other marks.
        let mut characters: Vec<(char, Marks)> = Vec::new();
        for item in &items[start..index] {
            if let Inline::Text { text, marks } = item {
                characters.extend(text.chars().map(|character| (character, marks.clone())));
            }
        }
        let first = characters
            .iter()
            .position(|(character, _)| !is_js_space(*character));
        let last = characters
            .iter()
            .rposition(|(character, _)| !is_js_space(*character));
        for (at, (character, marks)) in characters.into_iter().enumerate() {
            let inside =
                first.is_some_and(|first| at >= first) && last.is_some_and(|last| at <= last);
            let marks = if inside { marks } else { mark.without(&marks) };
            out.push(Inline::text(character.to_string(), marks));
        }
    }
    merge_inline(out)
}

/// `blocks` without ids, for comparing content.
pub fn strip_ids(blocks: &[Block]) -> Vec<Block> {
    blocks
        .iter()
        .map(|block| Block {
            id: None,
            node: match &block.node {
                Node::Blockquote(children) => Node::Blockquote(strip_ids(children)),
                Node::BulletList(items) => Node::BulletList(strip_item_ids(items)),
                Node::OrderedList { start, items } => Node::OrderedList {
                    start: *start,
                    items: strip_item_ids(items),
                },
                Node::Table(rows) => Node::Table(
                    rows.iter()
                        .map(|row| {
                            row.iter()
                                .map(|cell| Cell {
                                    paragraph_id: None,
                                    ..cell.clone()
                                })
                                .collect()
                        })
                        .collect(),
                ),
                other => other.clone(),
            },
        })
        .collect()
}

fn strip_item_ids(items: &[Item]) -> Vec<Item> {
    items
        .iter()
        .map(|item| Item {
            checked: item.checked,
            content: strip_ids(&item.content),
        })
        .collect()
}

/// The plain text of inline content, breaks as newlines.
pub fn plain_text(content: &[Inline]) -> String {
    let mut out = String::new();
    for inline in content {
        match inline {
            Inline::Text { text, .. } => out.push_str(text),
            Inline::HardBreak => out.push('\n'),
        }
    }
    out
}
