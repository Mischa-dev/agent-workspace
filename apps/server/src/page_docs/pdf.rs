//! A page as a PDF (C1.23): `GET /api/v1/pages/{id}/export?format=pdf`.
//!
//! Typeset here, in-process, from the page's document tree: no browser, no
//! outside service, nothing to install for a self-hoster. The text is set in
//! the Inter face the server already carries for board pictures, embedded
//! whole (Identity-H, with a ToUnicode map so the text can be searched and
//! copied). Bold is drawn by stroking the glyphs and italic by slanting
//! them, since one face is shipped.
//!
//! Embeds show in the app live; here a board is its picture, a sheet's range
//! its values as a table (both only when the reader may see them, decided by
//! the caller, which passes [`EmbedPicture`]s), and anything else its label
//! and link.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;

use skrifa::{
    FontRef, GlyphId, MetadataProvider,
    instance::{LocationRef, Size},
};

use super::markdown::Align;
use super::tree::{Block, Cell, Inline, Item, Marks, Node};

/// One table cell as the layout sees it: its text, whether it is a header,
/// and its alignment.
type TableCell = (Vec<Inline>, bool, Option<Align>);

pub const CONTENT_TYPE: &str = "application/pdf";

const FONT: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.otf");

/// US Letter, in points.
const PAGE_WIDTH: f32 = 612.0;
const PAGE_HEIGHT: f32 = 792.0;
const MARGIN: f32 = 64.0;
const BODY_SIZE: f32 = 10.5;
const CODE_SIZE: f32 = 9.0;
const LINK_COLOR: (f32, f32, f32) = (0.10, 0.33, 0.75);

/// What an embed shows in the PDF, resolved by the caller with the reader's
/// access. An embed with none shows its label and link.
#[derive(Clone, Debug)]
pub enum EmbedPicture {
    /// A picture, as straight RGBA pixels (a board).
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// Rows of cell text (a sheet's range), the first row a header.
    Table(Vec<Vec<String>>),
}

/// The page as PDF bytes. `origin` makes the app's own links absolute.
pub fn export(
    title: &str,
    blocks: &[Block],
    origin: Option<&str>,
    embeds: &HashMap<String, EmbedPicture>,
) -> Vec<u8> {
    let font = Face::load();
    let mut layout = Layout::new(&font, origin, embeds);
    if !title.trim().is_empty() {
        layout.heading(0, &[Inline::text(title.trim(), Marks::default())]);
    }
    layout.blocks(blocks, 0.0);
    let pages = layout.finish();
    write_document(title.trim(), &font, pages)
}

// ---------------------------------------------------------------------------
// The face

struct Face<'a> {
    font: FontRef<'a>,
    ascent: f32,
    descent: f32,
    cap_height: f32,
    bounds: [f32; 4],
}

impl Face<'static> {
    fn load() -> Self {
        let font = FontRef::new(FONT).expect("the bundled font reads");
        let metrics = font.metrics(Size::new(1000.0), LocationRef::default());
        let bounds = metrics
            .bounds
            .map(|b| [b.x_min, b.y_min, b.x_max, b.y_max])
            .unwrap_or([-200.0, -300.0, 1200.0, 1000.0]);
        Self {
            ascent: metrics.ascent,
            descent: metrics.descent,
            cap_height: metrics.cap_height.unwrap_or(metrics.ascent * 0.7),
            bounds,
            font,
        }
    }
}

impl Face<'_> {
    /// The glyph for a character (a space for controls; the font's missing
    /// glyph when it has none).
    fn glyph(&self, character: char) -> u16 {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        self.font
            .charmap()
            .map(character)
            .map_or(0, |glyph| glyph.to_u32() as u16)
    }

    /// A glyph's advance in thousandths of an em.
    fn advance(&self, glyph: u16) -> f32 {
        self.font
            .glyph_metrics(Size::new(1000.0), LocationRef::default())
            .advance_width(GlyphId::new(u32::from(glyph)))
            .unwrap_or(500.0)
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|character| self.advance(self.glyph(character)))
            .sum::<f32>()
            * size
            / 1000.0
    }
}

// ---------------------------------------------------------------------------
// Layout: blocks into positioned runs, page by page

#[derive(Clone, Debug, Default)]
struct Style {
    size: f32,
    bold: bool,
    italic: bool,
    strike: bool,
    link: Option<String>,
    gray: bool,
}

/// A word (or a space) with its style, before lines are made.
#[derive(Clone, Debug)]
struct Piece {
    text: String,
    style: Style,
    space: bool,
}

#[derive(Debug)]
enum Draw {
    Text {
        x: f32,
        y: f32,
        text: String,
        style: Style,
        width: f32,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        fill: f32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        gray: f32,
    },
    Image {
        index: usize,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

struct Page {
    draws: Vec<Draw>,
    links: Vec<([f32; 4], String)>,
}

struct Images {
    list: Vec<(Vec<u8>, u32, u32)>,
}

struct Layout<'a> {
    face: &'a Face<'a>,
    origin: Option<String>,
    embeds: &'a HashMap<String, EmbedPicture>,
    pages: Vec<Page>,
    images: Images,
    /// The baseline cursor: distance from the bottom of the page.
    y: f32,
}

struct Finished {
    pages: Vec<Page>,
    images: Vec<(Vec<u8>, u32, u32)>,
}

impl<'a> Layout<'a> {
    fn new(
        face: &'a Face<'a>,
        origin: Option<&str>,
        embeds: &'a HashMap<String, EmbedPicture>,
    ) -> Self {
        Self {
            face,
            origin: origin.map(|origin| origin.trim_end_matches('/').to_owned()),
            embeds,
            pages: vec![Page {
                draws: Vec::new(),
                links: Vec::new(),
            }],
            images: Images { list: Vec::new() },
            y: PAGE_HEIGHT - MARGIN,
        }
    }

    fn finish(self) -> Finished {
        Finished {
            pages: self.pages,
            images: self.images.list,
        }
    }

    fn page(&mut self) -> &mut Page {
        self.pages.last_mut().expect("a page")
    }

    fn new_page(&mut self) {
        self.pages.push(Page {
            draws: Vec::new(),
            links: Vec::new(),
        });
        self.y = PAGE_HEIGHT - MARGIN;
    }

    /// Room for `height` more points on this page, else a new page.
    fn need(&mut self, height: f32) {
        if self.y - height < MARGIN && self.y < PAGE_HEIGHT - MARGIN - 1.0 {
            self.new_page();
        }
    }

    fn gap(&mut self, points: f32) {
        self.y -= points;
    }

    fn href(&self, href: &str) -> String {
        match (&self.origin, href.starts_with('/')) {
            (Some(origin), true) => format!("{origin}{href}"),
            _ => href.to_owned(),
        }
    }

    fn blocks(&mut self, blocks: &[Block], indent: f32) {
        for block in blocks {
            self.block(block, indent);
        }
    }

    fn block(&mut self, block: &Block, indent: f32) {
        match &block.node {
            Node::Paragraph(content) => {
                self.paragraph(content, indent, BODY_SIZE, false, None);
                self.gap(6.0);
            }
            Node::Heading { level, content } => self.heading(*level, content),
            Node::CodeBlock { text, .. } => self.code(text, indent),
            Node::Blockquote(children) => {
                let top = self.y;
                let page = self.pages.len();
                self.blocks(children, indent + 14.0);
                if self.pages.len() == page {
                    let bottom = self.y + 4.0;
                    self.page().draws.push(Draw::Line {
                        x1: MARGIN + indent + 3.0,
                        y1: top + 2.0,
                        x2: MARGIN + indent + 3.0,
                        y2: bottom,
                        gray: 0.75,
                    });
                }
            }
            Node::BulletList(items) => self.list(items, indent, None),
            Node::OrderedList { start, items } => self.list(items, indent, Some(*start)),
            Node::HorizontalRule => {
                self.need(14.0);
                self.gap(6.0);
                let y = self.y;
                self.page().draws.push(Draw::Line {
                    x1: MARGIN + indent,
                    y1: y,
                    x2: PAGE_WIDTH - MARGIN,
                    y2: y,
                    gray: 0.75,
                });
                self.gap(8.0);
            }
            Node::Table(rows) => {
                let rows = rows
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|cell: &Cell| (cell.content.clone(), cell.header, cell.align))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                self.table(&rows, indent);
            }
            Node::Embed { href, label } => self.embed(href, label, indent),
        }
    }

    fn heading(&mut self, level: u8, content: &[Inline]) {
        let size = match level {
            0 => 22.0,
            1 => 18.0,
            2 => 15.0,
            3 => 13.0,
            _ => 11.5,
        };
        self.gap(if level <= 1 { 6.0 } else { 4.0 });
        self.paragraph(content, 0.0, size, true, None);
        self.gap(if level == 0 { 10.0 } else { 6.0 });
    }

    /// Inline content wrapped to the width left after `indent`. `prefix` is
    /// drawn hanging to the left of the first line (a bullet or a number).
    fn paragraph(
        &mut self,
        content: &[Inline],
        indent: f32,
        size: f32,
        bold: bool,
        prefix: Option<(&str, f32)>,
    ) {
        let pieces = pieces(content, size, bold);
        let left = MARGIN + indent;
        let width = PAGE_WIDTH - MARGIN - left;
        let lines = self.lines(&pieces, width);
        let leading = size * 1.35;
        for (index, line) in lines.iter().enumerate() {
            self.need(leading);
            self.y -= leading;
            let y = self.y + size * 0.3;
            if index == 0
                && let Some((text, at)) = prefix
            {
                let style = Style {
                    size,
                    ..Style::default()
                };
                let width = self.face.width(text, size);
                self.page().draws.push(Draw::Text {
                    x: left - at,
                    y,
                    text: text.to_owned(),
                    style,
                    width,
                });
            }
            let mut x = left;
            for piece in line {
                let piece_width = self.face.width(&piece.text, piece.style.size);
                if !piece.space || piece.style.strike || piece.style.link.is_some() {
                    if let Some(link) = &piece.style.link {
                        let href = self.href(link);
                        self.page()
                            .links
                            .push(([x, y - size * 0.25, x + piece_width, y + size * 0.8], href));
                    }
                    self.page().draws.push(Draw::Text {
                        x,
                        y,
                        text: piece.text.clone(),
                        style: piece.style.clone(),
                        width: piece_width,
                    });
                }
                x += piece_width;
            }
        }
        if lines.is_empty() {
            self.need(leading);
            self.y -= leading;
        }
    }

    /// Greedy line breaking; a word wider than the line is cut by characters.
    fn lines(&self, pieces: &[Piece], width: f32) -> Vec<Vec<Piece>> {
        let mut lines = vec![Vec::new()];
        let mut used = 0.0;
        for piece in pieces {
            if piece.text == "\n" {
                lines.push(Vec::new());
                used = 0.0;
                continue;
            }
            let piece_width = self.face.width(&piece.text, piece.style.size);
            let line = lines.last_mut().expect("a line");
            if piece.space {
                if !line.is_empty() {
                    line.push(piece.clone());
                    used += piece_width;
                }
                continue;
            }
            if used + piece_width <= width || line.is_empty() && piece_width <= width {
                line.push(piece.clone());
                used += piece_width;
                continue;
            }
            // Start a new line, dropping trailing spaces.
            while line.last().is_some_and(|last: &Piece| last.space) {
                line.pop();
            }
            if piece_width <= width {
                lines.push(vec![piece.clone()]);
                used = piece_width;
                continue;
            }
            // A word too long for any line: cut it.
            if !line.is_empty() {
                lines.push(Vec::new());
            }
            let mut chunk = String::new();
            let mut chunk_width = 0.0;
            for character in piece.text.chars() {
                let character_width = self.face.width(&character.to_string(), piece.style.size);
                if chunk_width + character_width > width && !chunk.is_empty() {
                    lines.last_mut().expect("a line").push(Piece {
                        text: std::mem::take(&mut chunk),
                        style: piece.style.clone(),
                        space: false,
                    });
                    lines.push(Vec::new());
                    chunk_width = 0.0;
                }
                chunk.push(character);
                chunk_width += character_width;
            }
            used = chunk_width;
            lines.last_mut().expect("a line").push(Piece {
                text: chunk,
                style: piece.style.clone(),
                space: false,
            });
        }
        for line in &mut lines {
            while line.last().is_some_and(|last| last.space) {
                line.pop();
            }
        }
        if lines.len() == 1 && lines[0].is_empty() {
            lines.clear();
        }
        lines
    }

    fn code(&mut self, text: &str, indent: f32) {
        let left = MARGIN + indent;
        let width = PAGE_WIDTH - MARGIN - left;
        let style = Style {
            size: CODE_SIZE,
            ..Style::default()
        };
        let leading = CODE_SIZE * 1.4;
        let mut lines = Vec::new();
        for raw in text.split('\n') {
            let mut line = String::new();
            let mut line_width = 0.0;
            for character in raw.replace('\t', "    ").chars() {
                let character_width = self.face.width(&character.to_string(), CODE_SIZE);
                if line_width + character_width > width - 12.0 && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    line_width = 0.0;
                }
                line.push(character);
                line_width += character_width;
            }
            lines.push(line);
        }
        self.gap(2.0);
        let mut first = true;
        for line in lines {
            self.need(leading + if first { 4.0 } else { 0.0 });
            if first {
                self.gap(4.0);
                first = false;
            }
            let top = self.y;
            self.y -= leading;
            let y = self.y;
            self.page().draws.push(Draw::Rect {
                x: left,
                y,
                width,
                height: top - y,
                fill: 0.95,
            });
            let text_width = self.face.width(&line, CODE_SIZE);
            self.page().draws.push(Draw::Text {
                x: left + 6.0,
                y: y + CODE_SIZE * 0.4,
                text: line,
                style: style.clone(),
                width: text_width,
            });
        }
        self.gap(10.0);
    }

    fn list(&mut self, items: &[Item], indent: f32, ordered: Option<u32>) {
        for (index, item) in items.iter().enumerate() {
            let marker = match (item.checked, ordered) {
                (Some(true), _) => "[x]".to_owned(),
                (Some(false), _) => "[ ]".to_owned(),
                (None, Some(start)) => format!("{}.", start as usize + index),
                (None, None) => "\u{2022}".to_owned(),
            };
            let inner = indent + 18.0;
            for (position, child) in item.content.iter().enumerate() {
                match (&child.node, position) {
                    (Node::Paragraph(content), 0) => {
                        let at = self.face.width(&marker, BODY_SIZE) + 5.0;
                        self.paragraph(content, inner, BODY_SIZE, false, Some((&marker, at)));
                        self.gap(2.0);
                    }
                    _ => self.block(child, inner),
                }
            }
        }
        self.gap(4.0);
    }

    fn table(&mut self, rows: &[Vec<TableCell>], indent: f32) {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let left = MARGIN + indent;
        let width = PAGE_WIDTH - MARGIN - left;
        let column_width = width / columns as f32;
        let padding = 4.0;
        let size = BODY_SIZE - 1.0;
        let leading = size * 1.35;
        self.gap(2.0);
        for row in rows {
            let cells = (0..columns)
                .map(|column| {
                    row.get(column)
                        .map(|(content, header, _)| {
                            let pieces = pieces(content, size, *header);
                            self.lines(&pieces, column_width - 2.0 * padding)
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>();
            let height = cells.iter().map(Vec::len).max().unwrap_or(0).max(1) as f32 * leading
                + 2.0 * padding;
            self.need(height);
            let top = self.y;
            let bottom = top - height;
            let header = row.first().is_some_and(|cell| cell.1);
            if header {
                self.page().draws.push(Draw::Rect {
                    x: left,
                    y: bottom,
                    width,
                    height,
                    fill: 0.93,
                });
            }
            for (column, lines) in cells.iter().enumerate() {
                let x0 = left + column as f32 * column_width;
                let align = row.get(column).and_then(|cell| cell.2);
                for (line_index, line) in lines.iter().enumerate() {
                    let y = top - padding - (line_index as f32 + 1.0) * leading + size * 0.3;
                    let line_width: f32 = line
                        .iter()
                        .map(|piece| self.face.width(&piece.text, piece.style.size))
                        .sum();
                    let mut x = match align {
                        Some(Align::Right) => x0 + column_width - padding - line_width,
                        Some(Align::Center) => x0 + (column_width - line_width) / 2.0,
                        _ => x0 + padding,
                    };
                    for piece in line {
                        let piece_width = self.face.width(&piece.text, piece.style.size);
                        if !piece.space {
                            self.page().draws.push(Draw::Text {
                                x,
                                y,
                                text: piece.text.clone(),
                                style: piece.style.clone(),
                                width: piece_width,
                            });
                        }
                        x += piece_width;
                    }
                }
            }
            // Borders: the row's top and bottom, and every column edge.
            for y in [top, bottom] {
                self.page().draws.push(Draw::Line {
                    x1: left,
                    y1: y,
                    x2: left + width,
                    y2: y,
                    gray: 0.7,
                });
            }
            for column in 0..=columns {
                let x = left + column as f32 * column_width;
                self.page().draws.push(Draw::Line {
                    x1: x,
                    y1: top,
                    x2: x,
                    y2: bottom,
                    gray: 0.7,
                });
            }
            self.y = bottom;
        }
        self.gap(10.0);
    }

    fn embed(&mut self, href: &str, label: &str, indent: f32) {
        let caption = |layout: &mut Self| {
            let marks = Marks {
                link: Some(href.to_owned()),
                ..Marks::default()
            };
            layout.paragraph(
                &[Inline::text(label.to_owned(), marks)],
                indent,
                BODY_SIZE - 1.0,
                false,
                None,
            );
        };
        match self.embeds.get(href) {
            Some(EmbedPicture::Image {
                rgba,
                width,
                height,
            }) if *width > 0 && *height > 0 => {
                let available = PAGE_WIDTH - 2.0 * MARGIN - indent;
                let scale = (available / *width as f32)
                    .min((PAGE_HEIGHT - 2.0 * MARGIN) * 0.6 / *height as f32)
                    .min(1.0);
                let (draw_width, draw_height) = (*width as f32 * scale, *height as f32 * scale);
                self.need(draw_height + 20.0);
                self.gap(draw_height);
                let index = self.images.list.len();
                self.images.list.push((rgba.clone(), *width, *height));
                let y = self.y;
                self.page().draws.push(Draw::Image {
                    index,
                    x: MARGIN + indent,
                    y,
                    width: draw_width,
                    height: draw_height,
                });
                caption(self);
                self.gap(8.0);
            }
            Some(EmbedPicture::Table(rows)) => {
                caption(self);
                let rows = rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        row.iter()
                            .map(|text| {
                                (
                                    vec![Inline::text(text.clone(), Marks::default())],
                                    index == 0,
                                    None,
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();
                self.table(&rows, indent);
            }
            _ => {
                caption(self);
                let link = self.href(href);
                let style = Style {
                    size: BODY_SIZE - 2.0,
                    gray: true,
                    ..Style::default()
                };
                self.paragraph(
                    &[Inline::text(link, Marks::default())],
                    indent,
                    style.size,
                    false,
                    None,
                );
                self.gap(6.0);
            }
        }
    }
}

/// Inline content as words and spaces, each with its style.
fn pieces(content: &[Inline], size: f32, bold: bool) -> Vec<Piece> {
    let mut pieces = Vec::new();
    for inline in content {
        match inline {
            Inline::HardBreak => pieces.push(Piece {
                text: "\n".to_owned(),
                style: Style::default(),
                space: false,
            }),
            Inline::Text { text, marks } => {
                let style = Style {
                    size: if marks.code { size * 0.9 } else { size },
                    bold: bold || marks.bold,
                    italic: marks.italic,
                    strike: marks.strike,
                    link: marks.link.clone(),
                    gray: false,
                };
                let mut word = String::new();
                for character in text.chars() {
                    if character.is_whitespace() {
                        if !word.is_empty() {
                            pieces.push(Piece {
                                text: std::mem::take(&mut word),
                                style: style.clone(),
                                space: false,
                            });
                        }
                        pieces.push(Piece {
                            text: " ".to_owned(),
                            style: style.clone(),
                            space: true,
                        });
                    } else {
                        word.push(character);
                    }
                }
                if !word.is_empty() {
                    pieces.push(Piece {
                        text: word,
                        style,
                        space: false,
                    });
                }
            }
        }
    }
    pieces
}

// ---------------------------------------------------------------------------
// Writing the file

struct Writer {
    bytes: Vec<u8>,
    offsets: BTreeMap<usize, usize>,
}

impl Writer {
    fn object(&mut self, id: usize, body: &str) {
        self.offsets.insert(id, self.bytes.len());
        let _ = write!(self.bytes, "{id} 0 obj\n{body}\nendobj\n");
    }

    fn stream(&mut self, id: usize, dictionary: &str, data: &[u8], compress: bool) {
        self.offsets.insert(id, self.bytes.len());
        let (data, filter) = if compress {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            let _ = encoder.write_all(data);
            (
                encoder.finish().unwrap_or_default(),
                " /Filter /FlateDecode",
            )
        } else {
            (data.to_vec(), "")
        };
        let _ = write!(
            self.bytes,
            "{id} 0 obj\n<< {dictionary}{filter} /Length {} >>\nstream\n",
            data.len()
        );
        self.bytes.extend_from_slice(&data);
        self.bytes.extend_from_slice(b"\nendstream\nendobj\n");
    }
}

/// A PDF text string: UTF-16BE with a byte order mark, in hex.
fn text_string(value: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in value.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

fn write_document(title: &str, face: &Face<'_>, finished: Finished) -> Vec<u8> {
    let Finished { pages, images } = finished;
    let mut writer = Writer {
        bytes: b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec(),
        offsets: BTreeMap::new(),
    };
    // Fixed objects: 1 catalog, 2 pages, 3 font, 4 CID font, 5 descriptor,
    // 6 font file, 7 ToUnicode, 8 info. Then images (two objects each: the
    // colour and the alpha), then per page: the page, its content, and its
    // link annotations.
    let image_base = 9;
    let page_base = image_base + images.len() * 2;
    let mut used = BTreeMap::<u16, char>::new();
    let mut page_ids = Vec::new();
    let mut next = page_base;
    let mut contents = Vec::new();
    for page in &pages {
        let page_id = next;
        let content_id = next + 1;
        let link_ids = (0..page.links.len())
            .map(|index| next + 2 + index)
            .collect::<Vec<_>>();
        next += 2 + page.links.len();
        page_ids.push(page_id);
        let content = content_stream(face, page, &mut used);
        contents.push((page_id, content_id, link_ids, content));
    }

    let kids = page_ids
        .iter()
        .map(|id| format!("{id} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    writer.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    writer.object(
        2,
        &format!(
            "<< /Type /Pages /Kids [{kids}] /Count {} >>",
            page_ids.len()
        ),
    );
    writer.object(
        3,
        "<< /Type /Font /Subtype /Type0 /BaseFont /Inter-Regular /Encoding /Identity-H /DescendantFonts [4 0 R] /ToUnicode 7 0 R >>",
    );
    let mut widths = String::new();
    for glyph in used.keys() {
        let _ = write!(widths, "{glyph} [{:.0}] ", face.advance(*glyph));
    }
    writer.object(
        4,
        &format!(
            "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Inter-Regular /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 5 0 R /DW 500 /W [{widths}] >>"
        ),
    );
    writer.object(
        5,
        &format!(
            "<< /Type /FontDescriptor /FontName /Inter-Regular /Flags 32 /FontBBox [{:.0} {:.0} {:.0} {:.0}] /ItalicAngle 0 /Ascent {:.0} /Descent {:.0} /CapHeight {:.0} /StemV 80 /FontFile3 6 0 R >>",
            face.bounds[0], face.bounds[1], face.bounds[2], face.bounds[3],
            face.ascent, face.descent, face.cap_height
        ),
    );
    writer.stream(6, "/Subtype /OpenType", FONT, true);
    writer.stream(7, "", to_unicode(&used).as_bytes(), false);
    writer.object(
        8,
        &format!(
            "<< /Title {} /Producer (Agent Workspace) >>",
            text_string(title)
        ),
    );
    for (index, (rgba, width, height)) in images.iter().enumerate() {
        let image_id = image_base + index * 2;
        let mask_id = image_id + 1;
        let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
        let mut alpha = Vec::with_capacity(rgba.len() / 4);
        for pixel in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&pixel[..3]);
            alpha.push(pixel[3]);
        }
        writer.stream(
            image_id,
            &format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mask_id} 0 R"
            ),
            &rgb,
            true,
        );
        writer.stream(
            mask_id,
            &format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8"
            ),
            &alpha,
            true,
        );
    }
    let image_resources = (0..images.len())
        .map(|index| format!("/Im{index} {} 0 R", image_base + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    for ((page_id, content_id, link_ids, content), page) in contents.into_iter().zip(&pages) {
        let annotations = if link_ids.is_empty() {
            String::new()
        } else {
            format!(
                " /Annots [{}]",
                link_ids
                    .iter()
                    .map(|id| format!("{id} 0 R"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        writer.object(
            page_id,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH} {PAGE_HEIGHT}] /Resources << /Font << /F1 3 0 R >> /XObject << {image_resources} >> >> /Contents {content_id} 0 R{annotations} >>"
            ),
        );
        writer.stream(content_id, "", content.as_bytes(), false);
        for (id, (rect, href)) in link_ids.iter().zip(&page.links) {
            writer.object(
                *id,
                &format!(
                    "<< /Type /Annot /Subtype /Link /Rect [{:.2} {:.2} {:.2} {:.2}] /Border [0 0 0] /A << /S /URI /URI {} >> >>",
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    literal(href)
                ),
            );
        }
    }
    let count = next;
    let xref = writer.bytes.len();
    let _ = write!(writer.bytes, "xref\n0 {count}\n0000000000 65535 f \n");
    for id in 1..count {
        let offset = writer.offsets.get(&id).copied().unwrap_or(0);
        let _ = writeln!(writer.bytes, "{offset:010} 00000 n ");
    }
    let _ = write!(
        writer.bytes,
        "trailer\n<< /Size {count} /Root 1 0 R /Info 8 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    );
    writer.bytes
}

/// A PDF literal string with the characters that need it escaped; anything
/// outside printable ASCII is percent-encoded (it's a URI).
fn literal(value: &str) -> String {
    let mut out = String::from("(");
    for byte in value.bytes() {
        match byte {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(byte as char);
            }
            0x20..=0x7E => out.push(byte as char),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out.push(')');
    out
}

fn content_stream(face: &Face<'_>, page: &Page, used: &mut BTreeMap<u16, char>) -> String {
    let mut out = String::new();
    for draw in &page.draws {
        match draw {
            Draw::Rect {
                x,
                y,
                width,
                height,
                fill,
            } => {
                let _ = writeln!(
                    out,
                    "q {fill:.2} g {x:.2} {y:.2} {width:.2} {height:.2} re f Q"
                );
            }
            Draw::Line {
                x1,
                y1,
                x2,
                y2,
                gray,
            } => {
                let _ = writeln!(
                    out,
                    "q {gray:.2} G 0.6 w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S Q"
                );
            }
            Draw::Image {
                index,
                x,
                y,
                width,
                height,
            } => {
                let _ = writeln!(
                    out,
                    "q {width:.2} 0 0 {height:.2} {x:.2} {y:.2} cm /Im{index} Do Q"
                );
            }
            Draw::Text {
                x,
                y,
                text,
                style,
                width,
            } => {
                let (red, green, blue) = if style.link.is_some() {
                    LINK_COLOR
                } else if style.gray {
                    (0.4, 0.4, 0.4)
                } else {
                    (0.1, 0.1, 0.12)
                };
                let mut glyphs = String::new();
                for character in text.chars() {
                    let glyph = face.glyph(character);
                    used.entry(glyph).or_insert(character);
                    let _ = write!(glyphs, "{glyph:04X}");
                }
                let skew = if style.italic { 0.2 } else { 0.0 };
                let render = if style.bold {
                    format!(
                        "2 Tr {:.2} w {red:.2} {green:.2} {blue:.2} RG ",
                        style.size * 0.03
                    )
                } else {
                    "0 Tr ".to_owned()
                };
                let _ = writeln!(
                    out,
                    "q {red:.2} {green:.2} {blue:.2} rg BT /F1 {:.2} Tf {render}1 0 {skew:.2} 1 {x:.2} {y:.2} Tm <{glyphs}> Tj ET Q",
                    style.size
                );
                if style.strike || style.link.is_some() {
                    let line_y = if style.strike {
                        y + style.size * 0.3
                    } else {
                        y - style.size * 0.12
                    };
                    let _ = writeln!(
                        out,
                        "q {red:.2} {green:.2} {blue:.2} RG 0.5 w {x:.2} {line_y:.2} m {:.2} {line_y:.2} l S Q",
                        x + width
                    );
                }
            }
        }
    }
    out
}

/// The map from glyphs back to text, so the PDF can be searched and copied.
fn to_unicode(used: &BTreeMap<u16, char>) -> String {
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries = used.iter().collect::<Vec<_>>();
    for chunk in entries.chunks(100) {
        let _ = writeln!(out, "{} beginbfchar", chunk.len());
        for (glyph, character) in chunk {
            let mut units = String::new();
            let mut buffer = [0u16; 2];
            for unit in character.encode_utf16(&mut buffer) {
                let _ = write!(units, "{unit:04X}");
            }
            let _ = writeln!(out, "<{glyph:04X}> <{units}>");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}

/// The text a PDF from [`export`] shows, read back through its ToUnicode
/// map: what a reader's search or copy gets. It reads only the PDFs this
/// module writes (uncompressed text, one run per line), for checking them.
pub fn text_of(pdf: &[u8]) -> String {
    let source = String::from_utf8_lossy(pdf);
    let mut map = HashMap::new();
    for line in source.lines() {
        if let Some((glyph, unicode)) = line
            .strip_prefix('<')
            .and_then(|rest| rest.split_once("> <"))
            && let Some(unicode) = unicode.strip_suffix('>')
            && glyph.len() == 4
            && let (Ok(glyph), Ok(unit)) = (
                u16::from_str_radix(glyph, 16),
                u32::from_str_radix(&unicode[..4.min(unicode.len())], 16),
            )
        {
            map.insert(glyph, char::from_u32(unit).unwrap_or('?'));
        }
    }
    let mut text = String::new();
    for line in source.lines() {
        if let Some(start) = line.find("Tm <")
            && let Some(end) = line[start + 4..].find('>')
        {
            let hex = &line[start + 4..start + 4 + end];
            for index in (0..hex.len()).step_by(4) {
                if let Ok(glyph) = u16::from_str_radix(&hex[index..index + 4], 16) {
                    text.push(map.get(&glyph).copied().unwrap_or('?'));
                }
            }
            text.push(' ');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{EmbedPicture, export, text_of};
    use crate::page_docs::tree::from_markdown;

    #[test]
    fn a_page_becomes_a_pdf_with_its_words() {
        let markdown = "# Launch plan\n\nWe ship **on Friday** with a [link](https://example.com).\n\n| Item | Cost |\n| --- | ---: |\n| Rent | 1200 |\n\n```\nfn main() {}\n```\n\n- one\n- two\n\n[AWD-12 Fix login](/projects/p/work-items/t \"embed\")\n";
        let blocks = from_markdown(markdown);
        let mut embeds = HashMap::new();
        embeds.insert(
            "/whiteboards/b".to_owned(),
            EmbedPicture::Image {
                rgba: vec![255; 4 * 4],
                width: 2,
                height: 2,
            },
        );
        let pdf = export("Q4 plan", &blocks, Some("https://app.test"), &embeds);
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let text = text_of(&pdf);
        for words in [
            "Q4", "Launch", "plan", "Friday", "Rent", "1200", "main()", "AWD-12",
        ] {
            assert!(text.contains(words), "{words} missing from {text}");
        }
        assert!(
            text.contains("https://app.test/projects/p/work-items/t"),
            "{text}"
        );
    }

    #[test]
    fn a_long_page_runs_onto_more_pages() {
        let markdown = "A paragraph that goes on for a while.\n\n".repeat(200);
        let pdf = export("Long", &from_markdown(&markdown), None, &HashMap::new());
        let source = String::from_utf8_lossy(&pdf);
        let count = source
            .split("/Count ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|count| count.parse::<usize>().ok())
            .unwrap_or(0);
        assert!(count > 3, "{count} pages");
    }
}
