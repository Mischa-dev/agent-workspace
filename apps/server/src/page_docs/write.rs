//! The Markdown of a document: a page's body once the page is a document.
//!
//! Each top-level block is written on its own, then read back with the web's
//! parser ([`super::tree::from_markdown`]); when the reading isn't the block,
//! it's written again more cautiously (another bold delimiter, then every
//! punctuation character escaped). Escaping is otherwise kept to what the
//! web's parser needs, so the text stays easy to read and diff. Blocks are
//! joined by blank lines, and two lists in a row get different markers so
//! they stay two lists.

use std::collections::HashSet;

use super::markdown::{Align, bare_url_at_start, is_angle_autolink, is_space, is_word};
use super::tree::{
    Block, Cell, Inline, InlineContext, Item, MAX_LIST_START, Marks, Node, canon_blocks,
    canon_inline, from_markdown, merge_inline, strip_ids,
};
use crate::page_blocks::{is_js_space, is_rule};

/// One way of writing inline content. Markdown can often say the same
/// thing several ways, and the web's parser reads some of them differently
/// (an emphasis run ends at the first matching run, even inside code), so
/// each paragraph, heading and cell is written the first way that reads back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    /// Bold's delimiter: `**` or `__`.
    bold: &'static str,
    italic: Italic,
    /// Links inside emphasis that runs exactly as long, not around it.
    link_inside: bool,
    /// Code spans padded with a space inside each fence.
    pad_code: bool,
    /// A mark that opens inside another and runs past it goes outside it
    /// instead (the other closes and opens again inside).
    renest: bool,
    /// Every ASCII punctuation character in text escaped.
    paranoid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Italic {
    /// `_`, or `*` next to letters and digits (where `_` can't open or close).
    Auto,
    Star,
    Underscore,
}

/// The ways to write inline content, most readable first; only the
/// paranoid ones when `paranoid`.
fn styles(paranoid: bool) -> impl Iterator<Item = Style> {
    let levels: &'static [bool] = if paranoid { &[true] } else { &[false, true] };
    levels.iter().flat_map(|&paranoid| {
        [false, true].into_iter().flat_map(move |pad_code| {
            [false, true].into_iter().flat_map(move |renest| {
                [false, true].into_iter().flat_map(move |link_inside| {
                    [Italic::Auto, Italic::Star, Italic::Underscore]
                        .into_iter()
                        .flat_map(move |italic| {
                            ["**", "__"].into_iter().map(move |bold| Style {
                                bold,
                                italic,
                                link_inside,
                                pad_code,
                                renest,
                                paranoid,
                            })
                        })
                })
            })
        })
    })
}

/// A document's Markdown, and where each block's Markdown is in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub markdown: String,
    pub blocks: Vec<WrittenBlock>,
}

/// A top-level block that has Markdown: its index among the document's
/// blocks and its byte range in [`Written::markdown`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrittenBlock {
    pub index: usize,
    pub start: usize,
    pub end: usize,
}

/// The Markdown of a document's top-level blocks. Blocks with no Markdown
/// (an empty paragraph) are left out.
pub fn write_document(blocks: &[Block]) -> Written {
    let mut markdown = String::new();
    let mut written = Vec::new();
    let mut previous = None;
    for (index, block) in blocks.iter().enumerate() {
        let Some((text, marker)) = write_block(block, previous) else {
            continue;
        };
        if !markdown.is_empty() {
            markdown.push_str("\n\n");
        }
        let start = markdown.len();
        markdown.push_str(&text);
        written.push(WrittenBlock {
            index,
            start,
            end: markdown.len(),
        });
        previous = marker;
    }
    Written {
        markdown,
        blocks: written,
    }
}

/// One top-level block's Markdown and the list marker it used, checked by
/// reading it back; `None` when it has none. `previous` is the marker of a
/// list just before it, which this block must not reuse.
pub fn write_block(block: &Block, previous: Option<char>) -> Option<(String, Option<char>)> {
    let canon = canon_blocks(std::slice::from_ref(block)).pop()?;
    let expected = strip_ids(std::slice::from_ref(&canon));
    let mut first = None;
    for paranoid in [false, true] {
        let (lines, marker) = BlockWriter { paranoid }.block(&canon.node, previous);
        let text = lines.join("\n");
        if from_markdown(&text) == expected {
            return Some((text, marker));
        }
        first.get_or_insert((text, marker));
    }
    // Nothing reads back exactly (emphasis around code that holds its
    // delimiter, say): the usual writing is the most readable.
    first
}

struct BlockWriter {
    paranoid: bool,
}

/// What a line of inline Markdown is made of before escaping.
#[derive(Debug)]
enum Piece {
    /// Text to escape; `in_link` when it's inside a link's label.
    Text { text: String, in_link: bool },
    /// Markdown syntax, written as it is.
    Syntax(String),
}

impl Piece {
    fn first(&self) -> Option<char> {
        match self {
            Self::Text { text, .. } | Self::Syntax(text) => text.chars().next(),
        }
    }

    fn last(&self) -> Option<char> {
        match self {
            Self::Text { text, .. } | Self::Syntax(text) => text.chars().next_back(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Open {
    Link(String),
    Bold,
    Italic(char),
    Strike,
}

impl BlockWriter {
    fn block(&self, node: &Node, previous: Option<char>) -> (Vec<String>, Option<char>) {
        match node {
            Node::Paragraph(content) => (self.inline(content, InlineContext::Paragraph), None),
            Node::Heading { level, content } => {
                let text = self.inline(content, InlineContext::Heading).concat();
                let hashes = "#".repeat(usize::from(*level));
                (
                    vec![if text.is_empty() {
                        hashes
                    } else {
                        format!("{hashes} {text}")
                    }],
                    None,
                )
            }
            Node::CodeBlock { language, text } => (code_block(language.as_deref(), text), None),
            Node::Blockquote(children) => {
                let inner = self.blocks(children, false);
                let lines = if inner.is_empty() {
                    vec![">".to_owned()]
                } else {
                    inner
                        .into_iter()
                        .map(|line| {
                            if line.is_empty() {
                                ">".to_owned()
                            } else {
                                format!("> {line}")
                            }
                        })
                        .collect()
                };
                (lines, None)
            }
            Node::BulletList(items) => self.list(items, None, previous),
            Node::OrderedList { start, items } => self.list(items, Some(*start), previous),
            Node::HorizontalRule => (vec!["---".to_owned()], None),
            Node::Table(rows) => (self.table(rows), None),
        }
    }

    /// Blocks inside a quote or list item, a blank line between them. In a
    /// list item, a list right after the item's own text follows it on the
    /// next line (`- two` then `  - nested`), the way people write nested
    /// lists, so a list stays tight; the paranoid writer keeps the blank line.
    fn blocks(&self, blocks: &[Block], in_item: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let mut previous = None;
        for (index, block) in blocks.iter().enumerate() {
            let (block_lines, marker) = self.block(&block.node, previous);
            let tight = in_item
                && !self.paranoid
                && index == 1
                && matches!(blocks[0].node, Node::Paragraph(_))
                && interrupts_paragraph(&block.node);
            if !lines.is_empty() && !tight {
                lines.push(String::new());
            }
            lines.extend(block_lines);
            previous = marker;
        }
        lines
    }

    fn list(
        &self,
        items: &[Item],
        start: Option<u32>,
        previous: Option<char>,
    ) -> (Vec<String>, Option<char>) {
        let candidates: &[char] = if start.is_some() {
            &['.', ')']
        } else {
            &['-', '*', '+']
        };
        let contents: Vec<Vec<String>> = items
            .iter()
            .map(|item| self.blocks(&item.content, true))
            .collect();
        let mut fallback = None;
        for &marker in candidates {
            if Some(marker) == previous {
                continue;
            }
            let (lines, first_lines) = compose_list(items, &contents, start, marker);
            // `- ---` is a rule, not an item: try another marker.
            if first_lines.iter().all(|line| !is_rule(line)) {
                return (lines, Some(marker));
            }
            fallback.get_or_insert((lines, Some(marker)));
        }
        fallback.unwrap_or_else(|| {
            let marker = candidates[0];
            (
                compose_list(items, &contents, start, marker).0,
                Some(marker),
            )
        })
    }

    fn table(&self, rows: &[Vec<Cell>]) -> Vec<String> {
        let row_line = |row: &[Cell]| {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| self.inline(&cell.content, InlineContext::Cell).concat())
                .collect();
            format!("| {} |", cells.join(" | "))
        };
        let Some(head) = rows.first() else {
            return Vec::new();
        };
        let mut lines = vec![row_line(head)];
        let delimiter: Vec<&str> = head
            .iter()
            .map(|cell| match cell.align {
                None => "---",
                Some(Align::Left) => ":--",
                Some(Align::Center) => ":-:",
                Some(Align::Right) => "--:",
            })
            .collect();
        lines.push(format!("| {} |", delimiter.join(" | ")));
        lines.extend(rows[1..].iter().map(|row| row_line(row)));
        lines
    }

    /// Inline content's lines (one, outside a paragraph), written the first
    /// way that reads back on its own.
    fn inline(&self, content: &[Inline], context: InlineContext) -> Vec<String> {
        if let Some(lines) = self.first_that_reads_back(content, context) {
            return lines;
        }
        // Nothing says it exactly (bold and strikethrough overlapping where
        // a space is, say). Next best: the spaces where emphasis changes
        // without it.
        let loose = loosen(content, context);
        if loose != content
            && let Some(lines) = self.first_that_reads_back(&loose, context)
        {
            return lines;
        }
        let careful = styles(true).next().map(|style| Writer { style });
        careful.map_or_else(Vec::new, |writer| writer.lines(content, context))
    }

    fn first_that_reads_back(
        &self,
        content: &[Inline],
        context: InlineContext,
    ) -> Option<Vec<String>> {
        // Long content gets the usual way and the careful one, no more.
        let size: usize = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.len(),
                Inline::HardBreak => 1,
            })
            .sum();
        let candidates: Vec<Style> = if size > LONG_INLINE {
            styles(self.paranoid)
                .take(1)
                .chain(styles(true).take(1))
                .collect()
        } else {
            styles(self.paranoid).collect()
        };
        let mut tried = HashSet::new();
        for style in candidates {
            let lines = Writer { style }.lines(content, context);
            if tried.insert(lines.clone()) && reads_back(&lines, content, context) {
                return Some(lines);
            }
        }
        None
    }
}

/// Inline content longer than this (in bytes) is only written two ways.
const LONG_INLINE: usize = 20_000;

/// `content` with bold, italic and strikethrough taken off each run of
/// whitespace where they change, which Markdown can always say.
fn loosen(content: &[Inline], context: InlineContext) -> Vec<Inline> {
    let mut characters: Vec<Option<(char, Marks)>> = Vec::new();
    for inline in content {
        match inline {
            Inline::Text { text, marks } => {
                characters.extend(
                    text.chars()
                        .map(|character| Some((character, marks.clone()))),
                );
            }
            Inline::HardBreak => characters.push(None),
        }
    }
    let emphasis = |slot: &Option<(char, Marks)>| {
        slot.as_ref()
            .map(|(_, marks)| (marks.bold, marks.italic, marks.strike))
    };
    let is_loose_space = |slot: &Option<(char, Marks)>| matches!(slot, Some((character, marks)) if is_js_space(*character) && !marks.code);
    let mut at = 0;
    while at < characters.len() {
        if !is_loose_space(&characters[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < characters.len() && is_loose_space(&characters[at]) {
            at += 1;
        }
        let own = emphasis(&characters[start]);
        let uniform = characters[start..at]
            .iter()
            .all(|slot| emphasis(slot) == own);
        let before = start
            .checked_sub(1)
            .and_then(|before| emphasis(&characters[before]));
        let after = characters.get(at).and_then(emphasis);
        let changes = !uniform
            || before.is_some_and(|before| Some(before) != own)
            || after.is_some_and(|after| Some(after) != own);
        if changes {
            for slot in &mut characters[start..at] {
                if let Some((_, marks)) = slot {
                    marks.bold = false;
                    marks.italic = false;
                    marks.strike = false;
                }
            }
        }
    }
    let rebuilt: Vec<Inline> = characters
        .into_iter()
        .map(|slot| match slot {
            Some((character, marks)) => Inline::text(character.to_string(), marks),
            None => Inline::HardBreak,
        })
        .collect();
    canon_inline(&merge_inline(rebuilt), context)
}

/// Whether inline content's lines read back as it, in a paragraph, heading
/// or table cell of their own.
fn reads_back(lines: &[String], content: &[Inline], context: InlineContext) -> bool {
    match context {
        InlineContext::Paragraph => {
            from_markdown(&lines.join("\n")) == [Block::new(Node::Paragraph(content.to_vec()))]
        }
        InlineContext::Heading => {
            let line = lines.concat();
            let source = if line.is_empty() {
                "#".to_owned()
            } else {
                format!("# {line}")
            };
            from_markdown(&source)
                == [Block::new(Node::Heading {
                    level: 1,
                    content: content.to_vec(),
                })]
        }
        InlineContext::Cell => {
            let source = format!("| {} | x |\n| --- | --- |", lines.concat());
            from_markdown(&source)
                == [Block::new(Node::Table(vec![vec![
                    Cell::new(true, None, content.to_vec()),
                    Cell::new(true, None, vec![Inline::text("x", Marks::default())]),
                ]]))]
        }
    }
}

/// Writes inline content in one [`Style`].
struct Writer {
    style: Style,
}

impl Writer {
    /// A paragraph's lines, one per hard break (a line with nothing on it
    /// is a lone backslash, which reads as a break), or the one line of a
    /// heading or cell.
    fn lines(&self, content: &[Inline], context: InlineContext) -> Vec<String> {
        if context != InlineContext::Paragraph {
            let mut line = self.render_line(content, context);
            // The web splits a table row at `|`s outside code, counting
            // every backtick as a code fence: after a cell with an odd
            // number, it wouldn't see the next `|`. An empty link to a
            // backtick reads as nothing and evens them up (in front, where
            // a bare address can't run into it).
            if context == InlineContext::Cell && line.matches('`').count() % 2 == 1 {
                line.insert_str(0, "[](`)");
            }
            return vec![line];
        }
        content
            .split(|inline| *inline == Inline::HardBreak)
            .map(|line| {
                let rendered = self.render_line(line, context);
                if rendered.is_empty() {
                    "\\".to_owned()
                } else {
                    rendered
                }
            })
            .collect()
    }

    fn render_line(&self, line: &[Inline], context: InlineContext) -> String {
        let texts: Vec<(&str, &Marks)> = line
            .iter()
            .filter_map(|inline| match inline {
                Inline::Text { text, marks } => Some((text.as_str(), marks)),
                Inline::HardBreak => None,
            })
            .collect();
        let pieces = self.pieces(&texts);
        self.render(&pieces, context)
    }

    /// Text and syntax for one line, marks nested so the ones that run
    /// longest open first.
    fn pieces(&self, texts: &[(&str, &Marks)]) -> Vec<Piece> {
        let mut pieces = Vec::new();
        let mut stack: Vec<Open> = Vec::new();
        for index in 0..texts.len() {
            let (text, marks) = texts[index];
            let leaf_link = self.leaf_link(texts, index, &pieces);
            let link = marks.link.as_deref().filter(|_| leaf_link.is_none());
            let wants = |open: &Open| match open {
                Open::Link(href) => link == Some(href.as_str()),
                Open::Bold => marks.bold,
                Open::Italic(_) => marks.italic,
                Open::Strike => marks.strike,
            };
            // How many texts from here on each mark still covers.
            let runs = |open: &Open| match open {
                Open::Link(href) => extent(texts, index, |marks| {
                    marks.link.as_deref() == Some(href.as_str())
                }),
                Open::Bold => extent(texts, index, |marks| marks.bold),
                Open::Italic(_) => extent(texts, index, |marks| marks.italic),
                Open::Strike => extent(texts, index, |marks| marks.strike),
            };
            let mut wanted: Vec<Open> = Vec::new();
            if let Some(href) = link {
                wanted.push(Open::Link(href.to_owned()));
            }
            if marks.bold {
                wanted.push(Open::Bold);
            }
            if marks.italic {
                wanted.push(Open::Italic('*'));
            }
            if marks.strike {
                wanted.push(Open::Strike);
            }
            let mut keep = stack.iter().take_while(|open| wants(open)).count();
            if self.style.renest {
                // A mark opening here that runs past one already open goes
                // outside it: close that one and open it again inside.
                let longest = wanted
                    .iter()
                    .filter(|open| !stack[..keep].iter().any(|kept| same_kind(kept, open)))
                    .map(|open| runs(open))
                    .max()
                    .unwrap_or(0);
                if let Some(position) = stack[..keep].iter().position(|open| runs(open) < longest) {
                    keep = position;
                }
            }
            while stack.len() > keep {
                if let Some(open) = stack.pop() {
                    close(&mut pieces, &open, &stack, self.style);
                }
            }
            // Marks to open here, the longest-running first.
            let mut opening: Vec<(usize, u8, Open)> = wanted
                .into_iter()
                .filter(|open| !stack.iter().any(|kept| same_kind(kept, open)))
                .map(|open| {
                    let rank = match open {
                        Open::Link(_) if self.style.link_inside => 4,
                        Open::Link(_) => 0,
                        Open::Bold => 1,
                        Open::Italic(_) => 2,
                        Open::Strike => 3,
                    };
                    (runs(&open), rank, open)
                })
                .collect();
            opening.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            for (run, _, open) in opening {
                let open = match open {
                    Open::Italic(_) => {
                        let underscore = match self.style.italic {
                            Italic::Auto => {
                                !is_word(pieces.last().and_then(Piece::last))
                                    && !is_word(
                                        texts
                                            .get(index + run)
                                            .and_then(|(text, _)| text.chars().next()),
                                    )
                            }
                            Italic::Star => false,
                            Italic::Underscore => true,
                        };
                        Open::Italic(if underscore { '_' } else { '*' })
                    }
                    other => other,
                };
                let syntax = match &open {
                    Open::Link(_) => "[".to_owned(),
                    Open::Bold => self.style.bold.to_owned(),
                    Open::Italic(character) => character.to_string(),
                    Open::Strike => "~~".to_owned(),
                };
                push_syntax(&mut pieces, syntax, &stack);
                stack.push(open);
            }
            let in_link = stack.iter().any(|open| matches!(open, Open::Link(_)));
            if marks.code {
                push_syntax(&mut pieces, code_span(text, self.style.pad_code), &stack);
            } else if let Some(link) = leaf_link {
                push_syntax(&mut pieces, link, &stack);
            } else {
                pieces.push(Piece::Text {
                    text: text.to_owned(),
                    in_link,
                });
            }
        }
        while let Some(open) = stack.pop() {
            close(&mut pieces, &open, &stack, self.style);
        }
        pieces
    }

    /// A link whose text is its address, written as the address itself
    /// (bare, as people type them) or in angle brackets.
    fn leaf_link(
        &self,
        texts: &[(&str, &Marks)],
        index: usize,
        pieces: &[Piece],
    ) -> Option<String> {
        let (text, marks) = texts[index];
        let href = marks.link.as_deref()?;
        let same = |at: Option<usize>| {
            at.and_then(|at| texts.get(at))
                .is_some_and(|(_, other)| other.link.as_deref() == Some(href))
        };
        if marks.code || text != href || same(index.checked_sub(1)) || same(Some(index + 1)) {
            return None;
        }
        let before = pieces.last().and_then(Piece::last);
        // A bare address runs until whitespace, so what follows must be
        // plain text that starts with some (or nothing).
        let ends = match texts.get(index + 1) {
            None => true,
            Some((text, marks)) => marks.is_empty() && text.chars().next().is_some_and(is_js_space),
        };
        let lower = href.to_ascii_lowercase();
        // Emphasis around an address that holds its delimiter would end
        // inside it: only plain links are written as their address.
        if !marks.is_empty_but_link() {
            return None;
        }
        if (lower.starts_with("http://") || lower.starts_with("https://"))
            && !is_word(before)
            && ends
            && bare_url_at_start(href).as_deref() == Some(href)
        {
            return Some(href.to_owned());
        }
        is_angle_autolink(href).then(|| format!("<{href}>"))
    }

    /// Escapes the text pieces and joins everything into the line.
    fn render(&self, pieces: &[Piece], context: InlineContext) -> String {
        let mut out = String::new();
        for (index, piece) in pieces.iter().enumerate() {
            match piece {
                Piece::Syntax(syntax) => {
                    if context == InlineContext::Cell {
                        out.push_str(&syntax.replace('|', "\\|"));
                    } else {
                        out.push_str(syntax);
                    }
                }
                Piece::Text { text, in_link } => {
                    let characters: Vec<char> = text.chars().collect();
                    let following = pieces.get(index + 1).and_then(Piece::first);
                    let last_piece = index + 1 == pieces.len();
                    let numbered = if out.is_empty() && context == InlineContext::Paragraph {
                        numbered_line(&characters)
                    } else {
                        None
                    };
                    for (at, &character) in characters.iter().enumerate() {
                        let previous = out.chars().next_back();
                        let next = characters.get(at + 1).copied().or(following);
                        let escape = (self.style.paranoid && character.is_ascii_punctuation())
                            || numbered == Some(at)
                            || (out.is_empty()
                                && context == InlineContext::Paragraph
                                && line_start_escape(&characters))
                            || (context == InlineContext::Heading
                                && character == '#'
                                && last_piece
                                && at + 1 == characters.len())
                            || needs_escape(
                                character,
                                previous,
                                next,
                                &characters[at + 1..],
                                &characters[..at],
                                *in_link,
                                context,
                            );
                        if escape && character.is_ascii_punctuation() {
                            out.push('\\');
                        }
                        out.push(character);
                    }
                }
            }
        }
        out
    }
}

impl Marks {
    /// Only a link: nothing that would wrap the bare address in syntax that
    /// changes where it ends.
    fn is_empty_but_link(&self) -> bool {
        !self.bold && !self.italic && !self.strike && !self.code
    }
}

/// The same mark, whatever character it's written with.
fn same_kind(a: &Open, b: &Open) -> bool {
    match (a, b) {
        (Open::Link(a), Open::Link(b)) => a == b,
        (Open::Bold, Open::Bold)
        | (Open::Italic(_), Open::Italic(_))
        | (Open::Strike, Open::Strike) => true,
        _ => false,
    }
}

/// How many texts from `index` on carry a mark.
fn extent(texts: &[(&str, &Marks)], index: usize, has: impl Fn(&Marks) -> bool) -> usize {
    texts[index..]
        .iter()
        .take_while(|(_, marks)| has(marks))
        .count()
}

/// Adds syntax, keeping two runs of the same emphasis character apart (they
/// would read as one run) with an empty link, `[]()`, which reads as
/// nothing. Inside a link's label that doesn't work, so they stay together.
fn push_syntax(pieces: &mut Vec<Piece>, syntax: String, stack: &[Open]) {
    let first = syntax.chars().next();
    let touching = match pieces.last() {
        Some(Piece::Syntax(last)) => last.chars().next_back() == first,
        _ => false,
    };
    if touching
        && matches!(first, Some('*' | '_' | '~'))
        && !stack.iter().any(|open| matches!(open, Open::Link(_)))
    {
        pieces.push(Piece::Syntax("[]()".to_owned()));
    }
    pieces.push(Piece::Syntax(syntax));
}

fn close(pieces: &mut Vec<Piece>, open: &Open, stack: &[Open], style: Style) {
    let syntax = match open {
        Open::Link(href) => format!("]({href})"),
        Open::Bold => style.bold.to_owned(),
        Open::Italic(character) => character.to_string(),
        Open::Strike => "~~".to_owned(),
    };
    // A closing run right after a backslash reads as escaped.
    if !matches!(open, Open::Link(_))
        && matches!(pieces.last(), Some(Piece::Text { text, .. }) if text.ends_with('\\'))
        && !stack.iter().any(|open| matches!(open, Open::Link(_)))
    {
        pieces.push(Piece::Syntax("[]()".to_owned()));
    }
    push_syntax(pieces, syntax, stack);
}

/// A code span: fenced by a backtick run the content doesn't hold, padded
/// with a space when asked, or where the web would otherwise take one off
/// or the fence would run into the content.
fn code_span(content: &str, pad: bool) -> String {
    let mut runs = HashSet::new();
    let mut run = 0;
    for character in content.chars() {
        if character == '`' {
            run += 1;
        } else if run > 0 {
            runs.insert(run);
            run = 0;
        }
    }
    if run > 0 {
        runs.insert(run);
    }
    let mut length = 1;
    while runs.contains(&length) {
        length += 1;
    }
    let fence = "`".repeat(length);
    let pad = pad
        || content.starts_with('`')
        || content.ends_with('`')
        || (content.starts_with(' ')
            && content.ends_with(' ')
            && content.encode_utf16().count() > 2);
    // In a table cell, `render` escapes the `|`s.
    if pad {
        format!("{fence} {content} {fence}")
    } else {
        format!("{fence}{content}{fence}")
    }
}

/// A fenced code block, its fence longer than any backtick run inside.
fn code_block(language: Option<&str>, text: &str) -> Vec<String> {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    let mut lines = vec![format!("{fence}{}", language.unwrap_or_default())];
    if !text.is_empty() {
        lines.extend(text.split('\n').map(str::to_owned));
    }
    lines.push(fence);
    lines
}

/// The list's lines, and each item's first line (to check for rules).
/// Whether a list can start right after a paragraph's last line without a
/// blank line (CommonMark): a bulleted list, or a numbered one from 1, whose
/// first item has something in it.
fn interrupts_paragraph(node: &Node) -> bool {
    let first = match node {
        Node::BulletList(items) => items.first(),
        Node::OrderedList { start: 1, items } => items.first(),
        _ => None,
    };
    first.is_some_and(|item| !item.content.is_empty())
}

fn compose_list(
    items: &[Item],
    contents: &[Vec<String>],
    start: Option<u32>,
    marker: char,
) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut first_lines = Vec::new();
    for (index, (item, content)) in items.iter().zip(contents).enumerate() {
        let label = match start {
            Some(start) => {
                let number = u32::try_from(index)
                    .ok()
                    .and_then(|index| start.checked_add(index))
                    .filter(|&number| number <= MAX_LIST_START)
                    .unwrap_or(start);
                format!("{number}{marker}")
            }
            None => marker.to_string(),
        };
        let mut first = label.clone();
        match item.checked {
            Some(true) => first.push_str(" [x]"),
            Some(false) => first.push_str(" [ ]"),
            None => {}
        }
        if let Some(line) = content.first() {
            first.push(' ');
            first.push_str(line);
        }
        first_lines.push(first.clone());
        lines.push(first);
        let indent = " ".repeat(label.len() + 1);
        for line in content.iter().skip(1) {
            if line.is_empty() {
                lines.push(String::new());
            } else {
                lines.push(format!("{indent}{line}"));
            }
        }
    }
    (lines, first_lines)
}

/// A line starting `12.` or `3)` would be a list item: where its `.` or `)`
/// is, to escape it.
fn numbered_line(characters: &[char]) -> Option<usize> {
    let digits = characters
        .iter()
        .take_while(|character| character.is_ascii_digit())
        .count();
    ((1..=9).contains(&digits) && matches!(characters.get(digits), Some('.' | ')')))
        .then_some(digits)
}

/// Whether a paragraph line starting with these characters needs its first
/// escaped: it would start a block (heading, quote, list, rule, fence, a
/// table's delimiter row) or a checklist item's box.
fn line_start_escape(characters: &[char]) -> bool {
    match characters.first() {
        Some('#' | '>' | '-' | '+' | '*' | '_' | '|' | ':' | '`' | '~' | '=') => true,
        Some('[') => matches!(
            (characters.get(1), characters.get(2)),
            (Some(' ' | 'x' | 'X'), Some(']'))
        ),
        _ => false,
    }
}

/// Whether a text character needs a backslash where it is.
#[allow(clippy::too_many_arguments)]
fn needs_escape(
    character: char,
    previous: Option<char>,
    next: Option<char>,
    after: &[char],
    before: &[char],
    in_link: bool,
    context: InlineContext,
) -> bool {
    match character {
        // A backslash escapes the punctuation after it, or the line break.
        '\\' => next.is_none_or(|next| next.is_ascii_punctuation()),
        '`' => true,
        // Could open (something follows) or close (something precedes).
        '*' => !(is_space(previous) && is_space(next)),
        '_' => (!is_word(previous) && !is_space(next)) || (!is_space(previous) && !is_word(next)),
        // Only a run of exactly two tildes is strikethrough.
        '~' => previous == Some('~') || next == Some('~'),
        '[' => in_link,
        ']' => in_link || next == Some('('),
        '<' => {
            !in_link && {
                let rest: String = after
                    .iter()
                    .take(6)
                    .collect::<String>()
                    .to_ascii_lowercase();
                rest.starts_with("http") || rest.starts_with("mailto")
            }
        }
        // `https://` in text would become a link.
        ':' => {
            !in_link && after.starts_with(&['/', '/']) && {
                let tail: String = before
                    .iter()
                    .rev()
                    .take(5)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
                    .to_ascii_lowercase();
                tail.ends_with("http") || tail.ends_with("https")
            }
        }
        '|' => context == InlineContext::Cell,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::page_docs::testing::{Random, random_blocks};
    use crate::page_docs::tree::Block;

    /// A short, readable form of a tree, for failure messages.
    fn show(blocks: &[Block]) -> String {
        fn inline(content: &[Inline]) -> String {
            content
                .iter()
                .map(|inline| match inline {
                    Inline::HardBreak => "⏎".to_owned(),
                    Inline::Text { text, marks } => {
                        let mut flags = String::new();
                        for (on, flag) in [
                            (marks.bold, 'b'),
                            (marks.italic, 'i'),
                            (marks.strike, 's'),
                            (marks.code, 'c'),
                        ] {
                            if on {
                                flags.push(flag);
                            }
                        }
                        if let Some(href) = &marks.link {
                            flags.push_str(&format!("@{href}"));
                        }
                        format!("{text:?}{flags}")
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        fn node(node: &Node) -> String {
            match node {
                Node::Paragraph(content) => format!("p[{}]", inline(content)),
                Node::Heading { level, content } => format!("h{level}[{}]", inline(content)),
                Node::CodeBlock { language, text } => format!("code({language:?})[{text:?}]"),
                Node::Blockquote(children) => format!("quote[{}]", show(children)),
                Node::BulletList(items) => format!("ul[{}]", items_text(items)),
                Node::OrderedList { start, items } => format!("ol{start}[{}]", items_text(items)),
                Node::HorizontalRule => "hr".to_owned(),
                Node::Table(rows) => format!(
                    "table[{}]",
                    rows.iter()
                        .map(|row| row
                            .iter()
                            .map(|cell| format!("{:?}:{}", cell.align, inline(&cell.content)))
                            .collect::<Vec<_>>()
                            .join(" | "))
                        .collect::<Vec<_>>()
                        .join(" // ")
                ),
            }
        }
        fn items_text(items: &[Item]) -> String {
            items
                .iter()
                .map(|item| format!("{:?}:{}", item.checked, show(&item.content)))
                .collect::<Vec<_>>()
                .join(" ; ")
        }
        blocks
            .iter()
            .map(|block| node(&block.node))
            .collect::<Vec<_>>()
            .join(" ,, ")
    }

    /// Every paragraph, heading and cell [`loosen`]ed.
    fn loosened(blocks: &[Block]) -> Vec<Block> {
        let items = |items: &[Item]| -> Vec<Item> {
            items
                .iter()
                .map(|item| Item {
                    checked: item.checked,
                    content: loosened(&item.content),
                })
                .collect()
        };
        blocks
            .iter()
            .map(|block| {
                let node = match &block.node {
                    Node::Paragraph(content) => {
                        Node::Paragraph(loosen(content, InlineContext::Paragraph))
                    }
                    Node::Heading { level, content } => Node::Heading {
                        level: *level,
                        content: loosen(content, InlineContext::Heading),
                    },
                    Node::Blockquote(children) => Node::Blockquote(loosened(children)),
                    Node::BulletList(list) => Node::BulletList(items(list)),
                    Node::OrderedList { start, items: list } => Node::OrderedList {
                        start: *start,
                        items: items(list),
                    },
                    Node::Table(rows) => Node::Table(
                        rows.iter()
                            .map(|row| {
                                row.iter()
                                    .map(|cell| Cell {
                                        content: loosen(&cell.content, InlineContext::Cell),
                                        ..cell.clone()
                                    })
                                    .collect()
                            })
                            .collect(),
                    ),
                    other => other.clone(),
                };
                Block { id: None, node }
            })
            .collect()
    }

    /// Why `blocks` don't read back from their Markdown, if they don't:
    /// the first top-level block that doesn't, alone if it fails alone.
    /// What Markdown can't say exactly may read back [`loosen`]ed.
    fn round_trip(blocks: &[Block]) -> Result<String, String> {
        let expected = strip_ids(&canon_blocks(blocks));
        let written = write_document(blocks);
        let read = from_markdown(&written.markdown);
        if read == expected {
            return Ok(written.markdown);
        }
        if loosened(&read) == loosened(&expected) {
            return Ok(written.markdown);
        }
        for block in &expected {
            let alone = std::slice::from_ref(block);
            let markdown = write_document(alone).markdown;
            let read = from_markdown(&markdown);
            if read != alone {
                return Err(format!(
                    "block: {markdown:?}\n  expected: {}\n  read:     {}",
                    show(alone),
                    show(&read)
                ));
            }
        }
        Err(format!(
            "document: {:?}\n  expected: {}\n  read:     {}",
            written.markdown,
            show(&expected),
            show(&read)
        ))
    }

    fn assert_all_round_trip(sources: impl IntoIterator<Item = Vec<Block>>) {
        let mut failures = Vec::new();
        let mut total = 0;
        for blocks in sources {
            total += 1;
            if let Err(failure) = round_trip(&blocks) {
                failures.push(failure);
            }
        }
        let shown: Vec<_> = failures.iter().take(6).cloned().collect();
        assert!(
            failures.is_empty(),
            "{} of {total} didn't read back\n{}",
            failures.len(),
            shown.join("\n")
        );
    }

    fn fixture_sources() -> Vec<String> {
        let mut sources = Vec::new();
        for fixture in [
            include_str!("../../../web/src/ui/markdown/parseParity.json"),
            include_str!("../../../web/src/ui/markdown/blockParity.json"),
        ] {
            let fixture: Value = serde_json::from_str(fixture).expect("fixture is JSON");
            for case in fixture["cases"].as_array().expect("cases") {
                let markdown = match case["markdown"].as_str() {
                    Some(markdown) => markdown.to_owned(),
                    None => {
                        let generate = &case["generate"];
                        let count = generate["count"].as_u64().expect("count") as usize;
                        generate["repeat"].as_str().expect("repeat").repeat(count)
                            + generate["then"].as_str().unwrap_or_default()
                    }
                };
                sources.push(markdown);
            }
        }
        sources
    }

    #[test]
    fn the_shared_markdown_cases_read_back() {
        let sources = fixture_sources();
        assert!(sources.len() > 100);
        assert_all_round_trip(sources.iter().map(|source| from_markdown(source)));
    }

    const TOKENS: &[&str] = &[
        "#",
        "##",
        "###",
        "####### ",
        " ",
        "  ",
        "   ",
        "    ",
        "\t",
        "\n",
        "\n",
        "\n",
        "\n\n",
        "\r\n",
        "\r",
        "*",
        "**",
        "***",
        "_",
        "__",
        "~",
        "~~",
        "`",
        "``",
        "```",
        "~~~",
        "```js\n",
        "\\",
        "\\\n",
        "\\*",
        "[",
        "]",
        "(",
        ")",
        "<",
        ">",
        "> ",
        ">",
        "- ",
        "* ",
        "+ ",
        "1. ",
        "2) ",
        "10. ",
        "- [ ] ",
        "- [x] ",
        "[X] ",
        "|",
        " | ",
        "|---|---|\n",
        "| :-- | --: |\n",
        "|:-:|\n",
        "---",
        "***\n",
        "___",
        "- - -",
        "https://a.com/x",
        "http://b.org/(y)",
        "HTTPS://C.IO.",
        "<https://d.net>",
        "<mailto:a@b.c>",
        "mailto:x@y.z",
        "[a](https://b.c)",
        "[t](/tasks \"t\")",
        "[x](javascript:1)",
        "](",
        "\"",
        "'",
        "AWD-12",
        "AB1-999",
        "X-1",
        "awd-3",
        "word",
        "two words",
        "é",
        "😀",
        "ß",
        ":",
        ".",
        ",",
        "!",
        "?",
        "=",
        "a",
        "b",
        "c",
        "0",
        "9",
        "\u{3000}",
        "\u{a0}",
    ];

    #[test]
    fn random_markdown_reads_back() {
        let mut random = Random(0x5eed_1234_abcd);
        let sources = (0..4000).map(|_| {
            let length = 1 + random.below(40);
            let markdown: String = (0..length).map(|_| *random.pick(TOKENS)).collect();
            from_markdown(&markdown)
        });
        assert_all_round_trip(sources.collect::<Vec<_>>());
    }

    #[test]
    fn random_documents_read_back() {
        let mut random = Random(0xd0c5_0042);
        let documents: Vec<Vec<Block>> = (0..4000)
            .map(|_| {
                let count = 1 + random.below(4);
                random_blocks(&mut random, 0, count)
            })
            .collect();
        assert_all_round_trip(documents);
    }
}
