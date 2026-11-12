//! A page as a Word document (.docx), and a Word document as a page
//! (AWD-53 criterion 9).
//!
//! [`export`] writes a page's blocks as WordprocessingML, which Word, Google
//! Docs, LibreOffice and Pages all open. [`import`] reads a .docx (from any
//! of them) back into blocks, keeping what a page can hold and naming what
//! it set aside. `docs/frontend/page-import-export.md` lists how each block
//! maps, and the fallback for each one that has no counterpart.

use std::collections::HashMap;

use roxmltree::{Document, Node as Xml, ParsingOptions};

use super::markdown::{Align, safe_href};
use super::tree::{Block, Cell, Inline, Item, MAX_BLOCK_DEPTH, Marks, Node};
use super::zip::{self, Package};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const HYPERLINK_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

/// The MIME type of a .docx file.
pub const CONTENT_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Code is set in these; a run in one of them reads back as code.
const MONOSPACE_FONTS: &[&str] = &[
    "consolas",
    "courier new",
    "courier",
    "menlo",
    "monaco",
    "roboto mono",
    "source code pro",
    "lucida console",
    "dejavu sans mono",
    "liberation mono",
    "jetbrains mono",
    "fira code",
    "fira mono",
    "sf mono",
    "cascadia code",
    "cascadia mono",
    "inconsolata",
    "ubuntu mono",
];

/// Twips of indent for each list or checklist level.
const INDENT_STEP: u32 = 720;
/// The deepest a nested walk of the document goes (content controls and
/// tracked changes nest).
const MAX_XML_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// Export

/// The .docx of a page called `title` with these blocks. Links to the app's
/// own paths are made absolute with `origin` (the app's public address),
/// when there is one.
pub fn export(title: &str, blocks: &[Block], origin: Option<&str>) -> Vec<u8> {
    let mut writer = Writer {
        body: String::new(),
        links: Vec::new(),
        ordered_lists: Vec::new(),
        origin: origin.map(|origin| origin.trim_end_matches('/').to_owned()),
    };
    if !title.trim().is_empty() {
        writer.paragraph(
            &Para {
                style: Some("Title"),
                ..Para::default()
            },
            &[Inline::text(title.trim(), Marks::default())],
        );
    }
    writer.blocks(blocks, &Context::default());
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        writer.body
    );
    let mut relationships = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdNumbering" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    for (index, href) in writer.links.iter().enumerate() {
        relationships.push_str(&format!(
            r#"<Relationship Id="rIdLink{}" Type="{HYPERLINK_TYPE}" Target="{}" TargetMode="External"/>"#,
            index + 1,
            escape(href)
        ));
    }
    relationships.push_str("</Relationships>");
    let core = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{}</dc:title></cp:coreProperties>"#,
        escape(title.trim())
    );
    zip::write(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes().to_vec()),
        ("_rels/.rels", PACKAGE_RELATIONSHIPS.as_bytes().to_vec()),
        ("docProps/core.xml", core.into_bytes()),
        ("word/document.xml", document.into_bytes()),
        ("word/styles.xml", STYLES.as_bytes().to_vec()),
        (
            "word/numbering.xml",
            numbering(&writer.ordered_lists).into_bytes(),
        ),
        ("word/_rels/document.xml.rels", relationships.into_bytes()),
    ])
}

struct Writer {
    body: String,
    /// Each link's target; `rIdLink{n}` is the n-th (from 1).
    links: Vec<String>,
    /// Each ordered list's start; its numbering is `numId` 2 + its index.
    ordered_lists: Vec<u32>,
    origin: Option<String>,
}

/// Where a block sits: how deep in lists, and whether in a quote.
#[derive(Clone, Copy, Default)]
struct Context {
    /// List levels the block is inside.
    level: u32,
    quote: bool,
}

#[derive(Default)]
struct Para<'a> {
    style: Option<&'a str>,
    /// `(numId, ilvl)`.
    numbering: Option<(usize, u32)>,
    indent: Option<u32>,
    align: Option<Align>,
    rule_below: bool,
}

impl Writer {
    fn blocks(&mut self, blocks: &[Block], context: &Context) {
        for block in blocks {
            self.block(block, context);
        }
    }

    fn block(&mut self, block: &Block, context: &Context) {
        let indent = (context.level > 0).then_some(context.level * INDENT_STEP);
        let quote = context.quote.then_some("Quote");
        match &block.node {
            Node::Paragraph(content) => self.paragraph(
                &Para {
                    style: quote,
                    indent,
                    ..Para::default()
                },
                content,
            ),
            Node::Heading { level, content } => {
                let style = format!("Heading{}", (*level).clamp(1, 6));
                self.paragraph(
                    &Para {
                        style: Some(&style),
                        indent,
                        ..Para::default()
                    },
                    content,
                );
            }
            Node::CodeBlock { text, .. } => {
                for line in text.split('\n') {
                    self.paragraph(
                        &Para {
                            style: Some("Code"),
                            indent,
                            ..Para::default()
                        },
                        &[Inline::text(line, Marks::default())],
                    );
                }
            }
            Node::Blockquote(children) => self.blocks(
                children,
                &Context {
                    quote: true,
                    ..*context
                },
            ),
            Node::BulletList(items) => self.list(items, None, context),
            Node::OrderedList { start, items } => {
                self.ordered_lists.push(*start);
                let num = self.ordered_lists.len() + 1;
                self.list(items, Some(num), context);
            }
            Node::HorizontalRule => self.paragraph(
                &Para {
                    rule_below: true,
                    indent,
                    ..Para::default()
                },
                &[],
            ),
            Node::Table(rows) => self.table(rows),
            // An embed shows live only in the app; elsewhere it's a link to it.
            Node::Embed { href, label } => {
                let marks = Marks {
                    link: Some(href.clone()),
                    ..Marks::default()
                };
                self.paragraph(
                    &Para {
                        style: quote,
                        indent,
                        ..Para::default()
                    },
                    &[Inline::text(label.clone(), marks)],
                );
            }
        }
    }

    /// A list's items. `ordered` is the numbering of an ordered list; a
    /// bullet list uses the shared bullets (`numId` 1), and a checklist item
    /// is a box character before its text, indented to its level.
    fn list(&mut self, items: &[Item], ordered: Option<usize>, context: &Context) {
        let ilvl = context.level.min(8);
        let inner = Context {
            level: context.level + 1,
            ..*context
        };
        for item in items {
            let mut rest = item.content.as_slice();
            let mut first: Vec<Inline> = Vec::new();
            if let Some(Block {
                node: Node::Paragraph(content),
                ..
            }) = rest.first()
            {
                first.clone_from(content);
                rest = &rest[1..];
            }
            let style = context.quote.then_some("Quote");
            if let Some(checked) = item.checked {
                let mut content = vec![Inline::text(
                    if checked { "\u{2611} " } else { "\u{2610} " },
                    Marks::default(),
                )];
                content.extend(first);
                self.paragraph(
                    &Para {
                        style,
                        indent: Some((context.level + 1) * INDENT_STEP),
                        ..Para::default()
                    },
                    &content,
                );
            } else {
                self.paragraph(
                    &Para {
                        style: Some(style.unwrap_or("ListParagraph")),
                        numbering: Some((ordered.unwrap_or(1), ilvl)),
                        ..Para::default()
                    },
                    &first,
                );
            }
            self.blocks(rest, &inner);
        }
    }

    fn table(&mut self, rows: &[Vec<Cell>]) {
        let columns = grid(rows);
        let width = 9360 / columns.max(1) as u32;
        self.body.push_str(
            r#"<w:tbl><w:tblPr><w:tblStyle w:val="PageTable"/><w:tblW w:w="5000" w:type="pct"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid>"#,
        );
        for _ in 0..columns {
            self.body
                .push_str(&format!(r#"<w:gridCol w:w="{width}"/>"#));
        }
        self.body.push_str("</w:tblGrid>");
        // Rows below a cell that spans rows hold a continuing cell in its
        // place: (rows left, columns it spans) for each grid column.
        let mut below: Vec<(u32, u32)> = vec![(0, 0); columns];
        for row in rows {
            self.body.push_str("<w:tr>");
            if row.iter().all(|cell| cell.header) {
                self.body.push_str(r#"<w:trPr><w:tblHeader/></w:trPr>"#);
            }
            let mut cells = row.iter();
            let mut column = 0;
            while column < columns {
                let (left, span) = below[column];
                if left > 0 {
                    below[column].0 -= 1;
                    self.cell_xml(span, Some("continue"), None, &[]);
                    column += span.max(1) as usize;
                    continue;
                }
                let Some(cell) = cells.next() else {
                    self.cell_xml(1, None, None, &[]);
                    column += 1;
                    continue;
                };
                let span = cell.colspan.clamp(1, (columns - column) as u32);
                let merge = (cell.rowspan > 1).then(|| {
                    below[column] = (cell.rowspan - 1, span);
                    "restart"
                });
                let style = cell.header.then_some("TableHeader");
                self.cell_xml(span, merge, Some((style, cell.align)), &cell.content);
                column += span as usize;
            }
            self.body.push_str("</w:tr>");
        }
        self.body.push_str("</w:tbl>");
    }

    fn cell_xml(
        &mut self,
        span: u32,
        merge: Option<&str>,
        paragraph: Option<(Option<&str>, Option<Align>)>,
        content: &[Inline],
    ) {
        self.body.push_str("<w:tc><w:tcPr>");
        if span > 1 {
            self.body
                .push_str(&format!(r#"<w:gridSpan w:val="{span}"/>"#));
        }
        match merge {
            Some("continue") => self.body.push_str("<w:vMerge/>"),
            Some(value) => self
                .body
                .push_str(&format!(r#"<w:vMerge w:val="{value}"/>"#)),
            None => {}
        }
        self.body.push_str("</w:tcPr>");
        let (style, align) = paragraph.unwrap_or((None, None));
        self.paragraph(
            &Para {
                style,
                align,
                ..Para::default()
            },
            content,
        );
        self.body.push_str("</w:tc>");
    }

    fn paragraph(&mut self, para: &Para<'_>, content: &[Inline]) {
        self.body.push_str("<w:p><w:pPr>");
        if let Some(style) = para.style {
            self.body
                .push_str(&format!(r#"<w:pStyle w:val="{style}"/>"#));
        }
        if let Some((num, level)) = para.numbering {
            self.body.push_str(&format!(
                r#"<w:numPr><w:ilvl w:val="{level}"/><w:numId w:val="{num}"/></w:numPr>"#
            ));
        }
        if para.rule_below {
            self.body.push_str(
                r#"<w:pBdr><w:bottom w:val="single" w:sz="6" w:space="1" w:color="auto"/></w:pBdr>"#,
            );
        }
        if let Some(indent) = para.indent {
            self.body
                .push_str(&format!(r#"<w:ind w:left="{indent}"/>"#));
        }
        if let Some(align) = para.align {
            self.body
                .push_str(&format!(r#"<w:jc w:val="{}"/>"#, align.name()));
        }
        self.body.push_str("</w:pPr>");
        for inline in content {
            match inline {
                Inline::HardBreak => self.body.push_str("<w:r><w:br/></w:r>"),
                Inline::Text { text, marks } => self.run(text, marks),
            }
        }
        self.body.push_str("</w:p>");
    }

    fn run(&mut self, text: &str, marks: &Marks) {
        let link = marks.link.as_deref().map(|href| self.link(href));
        if let Some(id) = &link {
            self.body.push_str(&format!(r#"<w:hyperlink r:id="{id}">"#));
        }
        self.body.push_str("<w:r><w:rPr>");
        if marks.code {
            self.body.push_str(r#"<w:rStyle w:val="InlineCode"/>"#);
        } else if link.is_some() {
            self.body.push_str(r#"<w:rStyle w:val="Hyperlink"/>"#);
        }
        if marks.bold {
            self.body.push_str("<w:b/>");
        }
        if marks.italic {
            self.body.push_str("<w:i/>");
        }
        if marks.strike {
            self.body.push_str("<w:strike/>");
        }
        self.body.push_str("</w:rPr>");
        // Tabs are their own element; a line break inside a paragraph's text
        // is a space where the page shows it.
        for (index, piece) in text.split('\t').enumerate() {
            if index > 0 {
                self.body.push_str("<w:tab/>");
            }
            if !piece.is_empty() {
                let piece = piece.replace(['\r', '\n'], " ");
                self.body.push_str(&format!(
                    r#"<w:t xml:space="preserve">{}</w:t>"#,
                    escape(&piece)
                ));
            }
        }
        self.body.push_str("</w:r>");
        if link.is_some() {
            self.body.push_str("</w:hyperlink>");
        }
    }

    /// The relationship id of a link to `href`, absolute when the app's
    /// address is known.
    fn link(&mut self, href: &str) -> String {
        let target = match (&self.origin, href.starts_with('/')) {
            (Some(origin), true) => format!("{origin}{href}"),
            _ => href.to_owned(),
        };
        let index = match self.links.iter().position(|known| *known == target) {
            Some(index) => index,
            None => {
                self.links.push(target);
                self.links.len() - 1
            }
        };
        format!("rIdLink{}", index + 1)
    }
}

/// How many grid columns a table needs: its widest row, counting spans and
/// the cells reaching down from rows above.
fn grid(rows: &[Vec<Cell>]) -> usize {
    let mut widest = 0;
    let mut below: Vec<u32> = Vec::new();
    for row in rows {
        let reaching = below.iter().filter(|left| **left > 0).count();
        let own: usize = row.iter().map(|cell| cell.colspan.max(1) as usize).sum();
        widest = widest.max(own + reaching);
        below = below
            .into_iter()
            .map(|left| left.saturating_sub(1))
            .collect();
        for cell in row {
            for _ in 0..cell.colspan.max(1) {
                below.push(cell.rowspan.saturating_sub(1));
            }
        }
        below.retain(|left| *left > 0);
    }
    widest.min(63)
}

fn numbering(ordered_lists: &[u32]) -> String {
    let mut xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="{W}"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>"#
    );
    let bullets = ["\u{2022}", "\u{25e6}", "\u{25aa}"];
    for level in 0..9u32 {
        xml.push_str(&format!(
            r#"<w:lvl w:ilvl="{level}"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#,
            bullets[level as usize % 3],
            (level + 1) * INDENT_STEP
        ));
    }
    xml.push_str(
        r#"</w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>"#,
    );
    let formats = ["decimal", "lowerLetter", "lowerRoman"];
    for level in 0..9u32 {
        xml.push_str(&format!(
            r#"<w:lvl w:ilvl="{level}"><w:start w:val="1"/><w:numFmt w:val="{}"/><w:lvlText w:val="%{}."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="360"/></w:pPr></w:lvl>"#,
            formats[level as usize % 3],
            level + 1,
            (level + 1) * INDENT_STEP
        ));
    }
    xml.push_str(r#"</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>"#);
    // Each ordered list is numbered on its own, from its own start, at
    // every level (the level it's at is the one used).
    for (index, start) in ordered_lists.iter().enumerate() {
        xml.push_str(&format!(
            r#"<w:num w:numId="{}"><w:abstractNumId w:val="1"/>"#,
            index + 2
        ));
        for level in 0..9 {
            xml.push_str(&format!(
                r#"<w:lvlOverride w:ilvl="{level}"><w:startOverride w:val="{start}"/></w:lvlOverride>"#
            ));
        }
        xml.push_str("</w:num>");
    }
    xml.push_str("</w:numbering>");
    xml
}

/// Text for XML: escaped, without the characters XML can't hold.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(character),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => {}
            _ => out.push(character),
        }
    }
    out
}

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#;

const PACKAGE_RELATIONSHIPS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="240"/></w:pPr><w:rPr><w:sz w:val="52"/><w:szCs w:val="52"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="320" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/><w:szCs w:val="30"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="220" w:after="60"/><w:outlineLvl w:val="4"/></w:pPr><w:rPr><w:b/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading6"><w:name w:val="heading 6"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="220" w:after="60"/><w:outlineLvl w:val="5"/></w:pPr><w:rPr><w:b/><w:i/><w:sz w:val="22"/><w:szCs w:val="22"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:pBdr><w:left w:val="single" w:sz="18" w:space="8" w:color="BFBFBF"/></w:pBdr><w:ind w:left="360"/></w:pPr><w:rPr><w:color w:val="595959"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="20"/><w:szCs w:val="20"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:contextualSpacing/></w:pPr></w:style><w:style w:type="paragraph" w:styleId="TableHeader"><w:name w:val="Table Header"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0"/></w:pPr><w:rPr><w:b/></w:rPr></w:style><w:style w:type="character" w:styleId="InlineCode"><w:name w:val="Inline Code"/><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="table" w:styleId="PageTable"><w:name w:val="Page Table"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/><w:left w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/><w:right w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="BFBFBF"/></w:tblBorders><w:tblCellMar><w:top w:w="60" w:type="dxa"/><w:left w:w="100" w:type="dxa"/><w:bottom w:w="60" w:type="dxa"/><w:right w:w="100" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style></w:styles>"#;

// ---------------------------------------------------------------------------
// Import

/// A Word document read as a page.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// The document's title paragraph (or its properties' title), when it
    /// has one.
    pub title: Option<String>,
    pub blocks: Vec<Block>,
    /// What the document held that a page can't, in words ("images",
    /// "footnotes"): left out, so the person can be told.
    pub set_aside: Vec<&'static str>,
}

/// Why a file can't be read as a Word document.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Not a .docx (or a damaged one).
    NotDocx,
    /// Bigger, unpacked, than a page import takes.
    TooBig,
}

impl From<zip::ZipError> for ImportError {
    fn from(error: zip::ZipError) -> Self {
        match error {
            zip::ZipError::TooBig => Self::TooBig,
            zip::ZipError::NotZip | zip::ZipError::Unsupported => Self::NotDocx,
        }
    }
}

/// The most XML nodes one part may have.
const MAX_XML_NODES: u32 = 4_000_000;

fn parse(text: &str) -> Result<Document<'_>, ImportError> {
    Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_XML_NODES,
            ..ParsingOptions::default()
        },
    )
    .map_err(|_| ImportError::NotDocx)
}

fn read_text(package: &Package<'_>, name: &str) -> Result<Option<String>, ImportError> {
    let Some(bytes) = package.read(name)? else {
        return Ok(None);
    };
    // UTF-8 (with or without a byte order mark) is all Word writes.
    let text = String::from_utf8(bytes).map_err(|_| ImportError::NotDocx)?;
    Ok(Some(text.trim_start_matches('\u{feff}').to_owned()))
}

/// A part's relationships: id to (type, target, whether it's outside the
/// package).
type Relationships = HashMap<String, (String, String, bool)>;

fn relationships(package: &Package<'_>, name: &str) -> Result<Relationships, ImportError> {
    let Some(text) = read_text(package, name)? else {
        return Ok(HashMap::new());
    };
    let document = parse(&text)?;
    Ok(document
        .root_element()
        .children()
        .filter(|node| is(*node, "Relationship"))
        .filter_map(|node| {
            Some((
                node.attribute("Id")?.to_owned(),
                (
                    node.attribute("Type").unwrap_or_default().to_owned(),
                    node.attribute("Target")?.to_owned(),
                    node.attribute("TargetMode") == Some("External"),
                ),
            ))
        })
        .collect())
}

/// `target` from a part in `directory`, as a part name.
fn resolve(directory: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_owned(),
        None => format!("{directory}{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// Reads a .docx. `origin` (the app's public address) turns links back to
/// the app's own pages into paths.
pub fn import(bytes: &[u8], origin: Option<&str>) -> Result<Imported, ImportError> {
    let package = Package::open(bytes)?;
    let package_relationships = relationships(&package, "_rels/.rels")?;
    let main = package_relationships
        .values()
        .find(|(kind, _, external)| !external && kind.ends_with("/officeDocument"))
        .map(|(_, target, _)| resolve("", target))
        .unwrap_or_else(|| "word/document.xml".to_owned());
    let (directory, file) = match main.rfind('/') {
        Some(at) => (&main[..=at], &main[at + 1..]),
        None => ("", main.as_str()),
    };
    let links = relationships(&package, &format!("{directory}_rels/{file}.rels"))?;
    let part_of = |suffix: &str| {
        links
            .values()
            .find(|(kind, _, external)| !external && kind.ends_with(suffix))
            .map(|(_, target, _)| resolve(directory, target))
    };

    let styles_text = match part_of("/styles") {
        Some(name) => read_text(&package, &name)?,
        None => None,
    };
    let styles_document = styles_text.as_deref().map(parse).transpose()?;
    let styles = styles_document
        .as_ref()
        .map(|document| Styles::read(document.root_element()))
        .unwrap_or_default();

    let numbering_text = match part_of("/numbering") {
        Some(name) => read_text(&package, &name)?,
        None => None,
    };
    let numbering_document = numbering_text.as_deref().map(parse).transpose()?;
    let numbering = numbering_document
        .as_ref()
        .map(|document| Numbering::read(document.root_element()))
        .unwrap_or_default();

    let text = read_text(&package, &main)?.ok_or(ImportError::NotDocx)?;
    let document = parse(&text)?;
    let root = document.root_element();
    if !is(root, "document") {
        return Err(ImportError::NotDocx);
    }
    let body = child(root, "body").ok_or(ImportError::NotDocx)?;

    let mut reader = Reader {
        links: &links,
        styles: &styles,
        numbering: &numbering,
        origin: origin.map(|origin| origin.trim_end_matches('/')),
        title: None,
        set_aside: Vec::new(),
        flat: Vec::new(),
    };
    reader.body(body, 0);
    let Reader {
        title,
        mut set_aside,
        flat,
        ..
    } = reader;

    let title = match title {
        Some(title) => Some(title),
        None => match read_text(&package, "docProps/core.xml")? {
            Some(core) => {
                let core = parse(&core)?;
                core.root_element()
                    .children()
                    .find(|node| is(*node, "title"))
                    .and_then(|node| node.text())
                    .map(|text| text.trim().to_owned())
                    .filter(|text| !text.is_empty())
            }
            None => None,
        },
    };
    set_aside.sort_unstable();
    set_aside.dedup();
    Ok(Imported {
        title,
        blocks: super::tree::canon_blocks(&group(flat)),
        set_aside,
    })
}

fn is(node: Xml<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn child<'a, 'input>(node: Xml<'a, 'input>, name: &str) -> Option<Xml<'a, 'input>> {
    node.children().find(|child| is(*child, name))
}

/// An attribute by its local name, whatever its namespace.
fn attr<'a>(node: Xml<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

fn val<'a>(node: Xml<'a, '_>) -> Option<&'a str> {
    attr(node, "val")
}

/// A relationship id (`r:id`).
fn relationship_id<'a>(node: Xml<'a, '_>) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| {
            attribute.name() == "id"
                && attribute
                    .namespace()
                    .is_some_and(|namespace| namespace.contains("relationships"))
        })
        .map(|attribute| attribute.value())
}

/// A toggle property (`<w:b/>`, `<w:b w:val="false"/>`).
fn toggle(properties: Option<Xml<'_, '_>>, name: &str) -> Option<bool> {
    let node = child(properties?, name)?;
    Some(!matches!(val(node), Some("0" | "false" | "off" | "none")))
}

fn monospace(properties: Option<Xml<'_, '_>>) -> Option<bool> {
    let fonts = child(properties?, "rFonts")?;
    let font = attr(fonts, "ascii").or_else(|| attr(fonts, "hAnsi"))?;
    Some(MONOSPACE_FONTS.contains(&font.trim().to_ascii_lowercase().as_str()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Normal,
    Title,
    Subtitle,
    Heading(u8),
    Quote,
    Code,
}

#[derive(Default)]
struct Style {
    name: String,
    based_on: Option<String>,
    /// `(numId, ilvl)` the style numbers its paragraphs with.
    numbering: Option<(String, u32)>,
    bold: Option<bool>,
    italic: Option<bool>,
    monospace: Option<bool>,
}

#[derive(Default)]
struct Styles(HashMap<String, Style>);

impl Styles {
    fn read(root: Xml<'_, '_>) -> Self {
        let mut styles = HashMap::new();
        for node in root.children().filter(|node| is(*node, "style")) {
            let Some(id) = attr(node, "styleId") else {
                continue;
            };
            let numbering = child(node, "pPr")
                .and_then(|properties| child(properties, "numPr"))
                .and_then(|numbering| {
                    let id = val(child(numbering, "numId")?)?.to_owned();
                    let level = child(numbering, "ilvl")
                        .and_then(val)
                        .and_then(|level| level.parse().ok())
                        .unwrap_or(0);
                    Some((id, level))
                });
            let run = child(node, "rPr");
            styles.insert(
                id.to_owned(),
                Style {
                    name: child(node, "name")
                        .and_then(val)
                        .unwrap_or(id)
                        .to_ascii_lowercase(),
                    based_on: child(node, "basedOn").and_then(val).map(str::to_owned),
                    numbering,
                    bold: toggle(run, "b"),
                    italic: toggle(run, "i"),
                    monospace: monospace(run),
                },
            );
        }
        Self(styles)
    }

    /// The style and those it's based on, nearest first.
    fn chain<'s>(&'s self, id: Option<&str>) -> Vec<&'s Style> {
        let mut chain = Vec::new();
        let mut next = id.map(str::to_owned);
        while let Some(id) = next {
            let Some(style) = self.0.get(&id) else {
                break;
            };
            chain.push(style);
            if chain.len() >= 8 {
                break;
            }
            next = style.based_on.clone();
        }
        chain
    }

    fn kind(&self, id: Option<&str>) -> Kind {
        for style in self.chain(id) {
            let name = style.name.replace(' ', "");
            let kind = match name.as_str() {
                "title" => Kind::Title,
                "subtitle" => Kind::Subtitle,
                "quote" | "intensequote" | "blocktext" => Kind::Quote,
                "code" | "htmlpreformatted" | "sourcecode" | "macrotext" | "codeblock"
                | "preformattedtext" => Kind::Code,
                _ => match name.strip_prefix("heading").map(str::parse::<u8>) {
                    Some(Ok(level)) if level >= 1 => Kind::Heading(level.min(6)),
                    _ => continue,
                },
            };
            return kind;
        }
        Kind::Normal
    }

    fn numbering(&self, id: Option<&str>) -> Option<(String, u32)> {
        self.chain(id)
            .into_iter()
            .find_map(|style| style.numbering.clone())
    }

    fn property(&self, id: Option<&str>, get: impl Fn(&Style) -> Option<bool>) -> bool {
        self.chain(id).into_iter().find_map(get).unwrap_or(false)
    }
}

/// A numbering level: whether it counts, where from, and its text (a
/// checklist's box shows there).
#[derive(Clone, Default)]
struct Level {
    ordered: bool,
    start: u32,
    text: String,
}

#[derive(Default)]
struct Numbering {
    /// numId to its abstract numbering and its overridden starts.
    nums: HashMap<String, (String, HashMap<u32, u32>)>,
    abstracts: HashMap<String, HashMap<u32, Level>>,
}

impl Numbering {
    fn read(root: Xml<'_, '_>) -> Self {
        let mut numbering = Self::default();
        for node in root.children() {
            if is(node, "abstractNum") {
                let Some(id) = attr(node, "abstractNumId") else {
                    continue;
                };
                let levels = node
                    .children()
                    .filter(|level| is(*level, "lvl"))
                    .filter_map(|level| {
                        let index = attr(level, "ilvl")?.parse().ok()?;
                        let format = child(level, "numFmt").and_then(val).unwrap_or("decimal");
                        Some((
                            index,
                            Level {
                                ordered: format != "bullet" && format != "none",
                                start: child(level, "start")
                                    .and_then(val)
                                    .and_then(|start| start.parse().ok())
                                    .unwrap_or(1),
                                text: child(level, "lvlText")
                                    .and_then(val)
                                    .unwrap_or_default()
                                    .to_owned(),
                            },
                        ))
                    })
                    .collect();
                numbering.abstracts.insert(id.to_owned(), levels);
            } else if is(node, "num") {
                let (Some(id), Some(abstract_id)) = (
                    attr(node, "numId"),
                    child(node, "abstractNumId").and_then(val),
                ) else {
                    continue;
                };
                let starts = node
                    .children()
                    .filter(|child| is(*child, "lvlOverride"))
                    .filter_map(|over| {
                        let level = attr(over, "ilvl")?.parse().ok()?;
                        let start = val(child(over, "startOverride")?)?.parse().ok()?;
                        Some((level, start))
                    })
                    .collect();
                numbering
                    .nums
                    .insert(id.to_owned(), (abstract_id.to_owned(), starts));
            }
        }
        numbering
    }

    fn level(&self, num: &str, level: u32) -> Level {
        let Some((abstract_id, starts)) = self.nums.get(num) else {
            return Level::default();
        };
        let mut found = self
            .abstracts
            .get(abstract_id)
            .and_then(|levels| levels.get(&level))
            .cloned()
            .unwrap_or_default();
        if let Some(start) = starts.get(&level) {
            found.start = *start;
        }
        found
    }
}

/// What a list paragraph belongs to: lists of different kinds don't join.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ListKind {
    Bullet,
    Check,
    Ordered(String),
}

#[derive(Debug)]
struct ListEntry {
    level: usize,
    kind: ListKind,
    start: u32,
    checked: Option<bool>,
    content: Vec<Inline>,
}

/// The document as a run of pieces, before lists, quotes and code are
/// gathered up.
#[derive(Debug)]
enum Flat {
    Block(Block),
    Code(String),
    Quote(Vec<Inline>),
    List(ListEntry),
}

struct Reader<'r> {
    links: &'r Relationships,
    styles: &'r Styles,
    numbering: &'r Numbering,
    origin: Option<&'r str>,
    title: Option<String>,
    set_aside: Vec<&'static str>,
    flat: Vec<Flat>,
}

impl Reader<'_> {
    fn body(&mut self, node: Xml<'_, '_>, depth: usize) {
        if depth > MAX_XML_DEPTH {
            return;
        }
        for child in node.children().filter(Xml::is_element) {
            match child.tag_name().name() {
                "p" => self.paragraph(child),
                "tbl" => {
                    if let Some(table) = self.table(child) {
                        self.flat.push(Flat::Block(Block::new(table)));
                    }
                }
                "sdt" | "sdtContent" | "customXml" | "ins" | "moveTo" | "smartTag" => {
                    self.body(child, depth + 1);
                }
                "altChunk" => self.set_aside.push("embedded documents"),
                _ => {}
            }
        }
    }

    fn paragraph(&mut self, paragraph: Xml<'_, '_>) {
        let properties = child(paragraph, "pPr");
        let style = properties
            .and_then(|properties| child(properties, "pStyle"))
            .and_then(val);
        let mut kind = self.styles.kind(style);
        if kind == Kind::Normal
            && let Some(level) = properties
                .and_then(|properties| child(properties, "outlineLvl"))
                .and_then(val)
                .and_then(|level| level.parse::<u8>().ok())
                .filter(|level| *level < 6)
        {
            kind = Kind::Heading(level + 1);
        }
        let direct_numbering = properties
            .and_then(|properties| child(properties, "numPr"))
            .and_then(|numbering| {
                let id = val(child(numbering, "numId")?)?.to_owned();
                let level = child(numbering, "ilvl")
                    .and_then(val)
                    .and_then(|level| level.parse().ok())
                    .unwrap_or(0);
                Some((id, level))
            });
        let numbering = direct_numbering
            .or_else(|| self.styles.numbering(style))
            .filter(|(id, _)| id != "0");
        let rule_below = properties
            .and_then(|properties| child(properties, "pBdr"))
            .and_then(|border| child(border, "bottom"))
            .is_some_and(|bottom| !matches!(val(bottom), Some("nil" | "none")));
        let indent = properties
            .and_then(|properties| child(properties, "ind"))
            .and_then(|indent| attr(indent, "left").or_else(|| attr(indent, "start")))
            .and_then(|left| left.parse::<i64>().ok())
            .unwrap_or(0);

        let mut runs = Runs::new(self);
        runs.walk(paragraph, None, 0);
        let mut content = runs.out;
        // Code keeps its tabs; elsewhere a tab is a space, as a page shows it.
        let code_text = plain(&content);
        untab(&mut content);
        self.set_aside.append(&mut runs.set_aside);

        let blank = content.iter().all(|inline| match inline {
            Inline::Text { text, .. } => text.trim().is_empty(),
            Inline::HardBreak => true,
        });
        if blank {
            if rule_below && kind != Kind::Code {
                self.flat
                    .push(Flat::Block(Block::new(Node::HorizontalRule)));
            } else if kind == Kind::Code {
                self.flat.push(Flat::Code(String::new()));
            }
            return;
        }
        match kind {
            Kind::Title if self.title.is_none() => {
                self.title = Some(plain(&content).trim().to_owned());
                return;
            }
            Kind::Title => kind = Kind::Heading(1),
            Kind::Subtitle => kind = Kind::Normal,
            _ => {}
        }

        // A checklist item: a box before the text, or as the bullet.
        let mut checked = take_box(&mut content);
        let mut list = None;
        if let Some((id, level)) = &numbering {
            let found = self.numbering.level(id, *level);
            if checked.is_none() {
                checked = box_state(&found.text);
            }
            list = Some((
                *level as usize,
                match (checked, found.ordered) {
                    (Some(_), _) => ListKind::Check,
                    (None, true) => ListKind::Ordered(id.clone()),
                    (None, false) => ListKind::Bullet,
                },
                found.start,
            ));
        } else if checked.is_some() {
            let level = (indent / i64::from(INDENT_STEP)).saturating_sub(1).max(0) as usize;
            list = Some((level, ListKind::Check, 1));
        }
        if let Some((level, kind, start)) = list {
            self.flat.push(Flat::List(ListEntry {
                level: level.min(MAX_BLOCK_DEPTH - 2),
                kind,
                start,
                checked,
                content,
            }));
            return;
        }

        let all_code = content.iter().all(|inline| match inline {
            Inline::Text { text, marks } => {
                text.trim().is_empty() || (marks.code && marks.link.is_none())
            }
            Inline::HardBreak => true,
        });
        let node = match kind {
            Kind::Heading(level) => Node::Heading { level, content },
            Kind::Code => {
                self.flat.push(Flat::Code(code_text));
                return;
            }
            Kind::Quote => {
                self.flat.push(Flat::Quote(content));
                return;
            }
            _ if all_code => {
                self.flat.push(Flat::Code(code_text));
                return;
            }
            _ => Node::Paragraph(content),
        };
        self.flat.push(Flat::Block(Block::new(node)));
    }

    fn table(&mut self, table: Xml<'_, '_>) -> Option<Node> {
        let mut rows = Vec::new();
        for (index, row) in table.children().filter(|row| is(*row, "tr")).enumerate() {
            let header = index == 0
                || child(row, "trPr")
                    .and_then(|properties| child(properties, "tblHeader"))
                    .is_some();
            let mut cells = Vec::new();
            for cell in row.descendants().filter(|cell| is(*cell, "tc")) {
                // Only this row's cells, not those of tables inside them.
                if cell
                    .ancestors()
                    .skip(1)
                    .find(|ancestor| is(*ancestor, "tr"))
                    != Some(row)
                {
                    continue;
                }
                let properties = child(cell, "tcPr");
                let span = properties
                    .and_then(|properties| child(properties, "gridSpan"))
                    .and_then(val)
                    .and_then(|span| span.parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 63);
                // A cell continuing one above is empty here: a page's
                // tables don't merge rows.
                let continues = properties
                    .and_then(|properties| child(properties, "vMerge"))
                    .is_some_and(|merge| val(merge) != Some("restart"));
                let mut content = Vec::new();
                let mut align = None;
                if !continues {
                    for paragraph in cell.descendants().filter(|node| is(*node, "p")) {
                        if align.is_none() {
                            align = child(paragraph, "pPr")
                                .and_then(|properties| child(properties, "jc"))
                                .and_then(val)
                                .and_then(|value| match value {
                                    "center" => Some(Align::Center),
                                    "right" | "end" => Some(Align::Right),
                                    "left" | "start" => Some(Align::Left),
                                    _ => None,
                                });
                        }
                        let mut runs = Runs::new(self);
                        runs.walk(paragraph, None, 0);
                        let Runs {
                            mut out,
                            set_aside: mut aside,
                            ..
                        } = runs;
                        untab(&mut out);
                        self.set_aside.append(&mut aside);
                        if out.is_empty() {
                            continue;
                        }
                        if !content.is_empty() {
                            content.push(Inline::HardBreak);
                        }
                        content.extend(out);
                    }
                }
                let mut cell = Cell::new(header, align, content);
                cell.colspan = span;
                cells.push(cell);
            }
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        (!rows.is_empty()).then_some(Node::Table(rows))
    }

    /// A link's target as a page link: the app's own pages as paths, and
    /// nothing a page wouldn't link to.
    fn href(&self, target: &str) -> Option<String> {
        let target = target.trim();
        let target = match self.origin {
            Some(origin) => match target.strip_prefix(origin) {
                Some(path) if path.starts_with('/') => path,
                _ => target,
            },
            None => target,
        };
        safe_href(target).map(|(href, _)| href)
    }
}

/// A paragraph's runs, read into inline content.
struct Runs<'r, 'p> {
    reader: &'p Reader<'r>,
    out: Vec<Inline>,
    set_aside: Vec<&'static str>,
    field: Field,
}

/// Where a run is in a field (`HYPERLINK "…"` written as runs).
enum Field {
    None,
    Instruction(String),
    Result(Option<String>),
}

impl<'r, 'p> Runs<'r, 'p> {
    fn new(reader: &'p Reader<'r>) -> Self {
        Self {
            reader,
            out: Vec::new(),
            set_aside: Vec::new(),
            field: Field::None,
        }
    }

    fn walk(&mut self, node: Xml<'_, '_>, link: Option<&str>, depth: usize) {
        if depth > MAX_XML_DEPTH {
            return;
        }
        for child in node.children().filter(Xml::is_element) {
            match child.tag_name().name() {
                "r" => self.run(child, link),
                "hyperlink" => {
                    let href = relationship_id(child)
                        .and_then(|id| self.reader.links.get(id))
                        .filter(|(kind, _, _)| {
                            kind == HYPERLINK_TYPE || kind.ends_with("/hyperlink")
                        })
                        .and_then(|(_, target, _)| self.reader.href(target));
                    self.walk(child, href.as_deref().or(link), depth + 1);
                }
                "fldSimple" => {
                    let href = attr(child, "instr")
                        .and_then(field_link)
                        .and_then(|target| self.reader.href(&target));
                    self.walk(child, href.as_deref().or(link), depth + 1);
                }
                "ins" | "moveTo" | "smartTag" | "customXml" | "sdt" | "sdtContent" | "dir"
                | "bdo" => self.walk(child, link, depth + 1),
                "oMath" | "oMathPara" => self.set_aside.push("equations"),
                _ => {}
            }
        }
    }

    fn run(&mut self, run: Xml<'_, '_>, link: Option<&str>) {
        let properties = child(run, "rPr");
        let run_style = properties
            .and_then(|properties| child(properties, "rStyle"))
            .and_then(val);
        let styles = self.reader.styles;
        let style_name = run_style
            .and_then(|id| styles.0.get(id))
            .map(|style| style.name.replace(' ', ""))
            .unwrap_or_default();
        // Only a character style's bold or italic counts: a paragraph
        // style's (a heading's bold) is the block's look, not a mark.
        let from_styles = |get: fn(&Style) -> Option<bool>| styles.property(run_style, get);
        let code = monospace(properties)
            .unwrap_or_else(|| styles.property(run_style, |style| style.monospace))
            || style_name.contains("code");
        let result_link = match &self.field {
            Field::Result(href) => href.clone(),
            _ => None,
        };
        let marks = Marks {
            bold: toggle(properties, "b")
                .unwrap_or_else(|| style_name == "strong" || from_styles(|style| style.bold)),
            italic: toggle(properties, "i")
                .unwrap_or_else(|| style_name == "emphasis" || from_styles(|style| style.italic)),
            strike: toggle(properties, "strike").unwrap_or(false)
                || toggle(properties, "dstrike").unwrap_or(false),
            code,
            link: link.map(str::to_owned).or(result_link),
        };
        for piece in run.children().filter(Xml::is_element) {
            match piece.tag_name().name() {
                "fldChar" => match attr(piece, "fldCharType") {
                    Some("begin") => self.field = Field::Instruction(String::new()),
                    Some("separate") => {
                        let href = match &self.field {
                            Field::Instruction(instruction) => {
                                field_link(instruction).and_then(|target| self.reader.href(&target))
                            }
                            _ => None,
                        };
                        self.field = Field::Result(href);
                    }
                    Some("end") => self.field = Field::None,
                    _ => {}
                },
                "instrText" => {
                    if let Field::Instruction(instruction) = &mut self.field
                        && instruction.len() < 4096
                    {
                        instruction.push_str(piece.text().unwrap_or_default());
                    }
                }
                _ if matches!(self.field, Field::Instruction(_)) => {}
                "t" => self.text(piece.text().unwrap_or_default(), &marks),
                "tab" | "ptab" => self.text("\t", &marks),
                "noBreakHyphen" => self.text("-", &marks),
                "br" => {
                    if !matches!(attr(piece, "type"), Some("page" | "column")) {
                        self.out.push(Inline::HardBreak);
                    }
                }
                "cr" => self.out.push(Inline::HardBreak),
                "drawing" | "pict" | "object" => self.set_aside.push("images and drawings"),
                "footnoteReference" | "endnoteReference" => self.set_aside.push("footnotes"),
                "commentReference" => self.set_aside.push("comments"),
                _ => {}
            }
        }
    }

    fn text(&mut self, text: &str, marks: &Marks) {
        if text.is_empty() {
            return;
        }
        if let Some(Inline::Text {
            text: last,
            marks: last_marks,
        }) = self.out.last_mut()
            && last_marks == marks
        {
            last.push_str(text);
            return;
        }
        self.out.push(Inline::text(text, marks.clone()));
    }
}

/// The target of a `HYPERLINK "…"` field; none for a link within the
/// document (`\l`).
fn field_link(instruction: &str) -> Option<String> {
    let rest = instruction.trim_start().strip_prefix("HYPERLINK")?;
    if rest.contains("\\l") {
        return None;
    }
    let start = rest.find('"')? + 1;
    let end = start + rest[start..].find('"')?;
    Some(rest[start..end].to_owned())
}

fn box_state(text: &str) -> Option<bool> {
    match text.trim_start().chars().next()? {
        '\u{2610}' => Some(false),
        '\u{2611}' | '\u{2612}' => Some(true),
        _ => None,
    }
}

/// Takes a leading checkbox character off the content, saying whether it
/// was ticked.
fn take_box(content: &mut Vec<Inline>) -> Option<bool> {
    let Some(Inline::Text { text, .. }) = content.first_mut() else {
        return None;
    };
    let state = box_state(text)?;
    let trimmed = text.trim_start();
    let rest = trimmed[trimmed.chars().next()?.len_utf8()..].trim_start();
    if rest.is_empty() {
        content.remove(0);
    } else {
        *text = rest.to_owned();
    }
    Some(state)
}

/// Tabs as spaces, for anything but code.
fn untab(content: &mut [Inline]) {
    for inline in content {
        if let Inline::Text { text, .. } = inline
            && text.contains('\t')
        {
            *text = text.replace('\t', " ");
        }
    }
}

fn plain(content: &[Inline]) -> String {
    content
        .iter()
        .map(|inline| match inline {
            Inline::Text { text, .. } => text.as_str(),
            Inline::HardBreak => "\n",
        })
        .collect()
}

/// Gathers lines of code into code blocks, quoted paragraphs into quotes
/// and list paragraphs into (nested) lists.
fn group(flat: Vec<Flat>) -> Vec<Block> {
    let mut out = Vec::new();
    let mut flat = flat.into_iter().peekable();
    while let Some(piece) = flat.next() {
        match piece {
            Flat::Block(block) => out.push(block),
            Flat::Code(line) => {
                let mut lines = vec![line];
                while let Some(Flat::Code(_)) = flat.peek() {
                    if let Some(Flat::Code(line)) = flat.next() {
                        lines.push(line);
                    }
                }
                while lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                if !lines.is_empty() {
                    out.push(Block::new(Node::CodeBlock {
                        language: None,
                        text: lines.join("\n"),
                    }));
                }
            }
            Flat::Quote(content) => {
                let mut paragraphs = vec![Block::new(Node::Paragraph(content))];
                while let Some(Flat::Quote(_)) = flat.peek() {
                    if let Some(Flat::Quote(content)) = flat.next() {
                        paragraphs.push(Block::new(Node::Paragraph(content)));
                    }
                }
                out.push(Block::new(Node::Blockquote(paragraphs)));
            }
            Flat::List(entry) => {
                let mut entries = vec![entry];
                while let Some(Flat::List(_)) = flat.peek() {
                    if let Some(Flat::List(entry)) = flat.next() {
                        entries.push(entry);
                    }
                }
                out.extend(lists(entries));
            }
        }
    }
    out
}

/// List entries as lists, deeper levels nested in the item before them.
fn lists(entries: Vec<ListEntry>) -> Vec<Block> {
    let Some(base) = entries.iter().map(|entry| entry.level).min() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut entries = entries.into_iter().peekable();
    while let Some(head) = entries.next() {
        let kind = head.kind.clone();
        let start = head.start;
        let mut items = vec![item(head)];
        while let Some(next) = entries.peek() {
            if next.level > base {
                let mut nested = Vec::new();
                while let Some(entry) = entries.next_if(|entry| entry.level > base) {
                    nested.push(entry);
                }
                if let Some(last) = items.last_mut() {
                    last.content.extend(lists(nested));
                }
                continue;
            }
            if next.kind != kind {
                break;
            }
            let Some(entry) = entries.next() else { break };
            items.push(item(entry));
        }
        out.push(Block::new(match kind {
            ListKind::Ordered(_) => Node::OrderedList { start, items },
            ListKind::Bullet | ListKind::Check => Node::BulletList(items),
        }));
    }
    out
}

fn item(entry: ListEntry) -> Item {
    Item {
        checked: entry.checked,
        content: if entry.content.is_empty() {
            Vec::new()
        } else {
            vec![Block::new(Node::Paragraph(entry.content))]
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::{tree::from_markdown, write::write_document};

    const SAMPLE: &str = "## Plan\n\nSome **bold**, *italic*, ~~gone~~ and `code` with a [link](https://example.com/a?b=1&c=2) and [a task](/projects/p/work-items/t).\n\n- one\n- two\n  - nested\n\n3. three\n4. four\n\n- [ ] open\n- [x] done\n\n> Quoted words\n\n```\nfn main() {\n\n    println!(\"<hi>\");\n}\n\tindented()\n```\n\n---\n\n| Name | Count |\n| :--- | ---: |\n| a | 1 |\n| b | 2 |\n\n[Open tasks](/projects/p/work-items?status=open \"embed\")";

    fn round_trip(markdown: &str) -> (Imported, String) {
        let bytes = export(
            "Weekly notes",
            &from_markdown(markdown),
            Some("https://app.test"),
        );
        let imported = import(&bytes, Some("https://app.test")).expect("reads back");
        let written = write_document(&imported.blocks).markdown;
        (imported, written)
    }

    #[test]
    fn a_page_survives_word_and_back() {
        let (imported, written) = round_trip(SAMPLE);
        assert_eq!(imported.title.as_deref(), Some("Weekly notes"));
        assert!(imported.set_aside.is_empty());
        // Everything but the embed (a link in Word) comes back as it was.
        let expected: Vec<Block> = from_markdown(SAMPLE)
            .into_iter()
            .map(|block| match block.node {
                Node::Embed { href, label } => Block::new(Node::Paragraph(vec![Inline::text(
                    label,
                    Marks {
                        link: Some(href),
                        ..Marks::default()
                    },
                )])),
                _ => block,
            })
            .collect();
        assert_eq!(imported.blocks, expected, "{written}");
    }

    #[test]
    fn own_links_become_absolute_in_word() {
        let bytes = export(
            "",
            &from_markdown("[t](/projects/p)"),
            Some("https://app.test/"),
        );
        let package = Package::open(&bytes).expect("test");
        let rels = String::from_utf8(
            package
                .read("word/_rels/document.xml.rels")
                .expect("test")
                .expect("test"),
        )
        .expect("test");
        assert!(
            rels.contains(r#"Target="https://app.test/projects/p""#),
            "{rels}"
        );
    }

    fn docx(body: &str, numbering: Option<&str>, styles: Option<&str>) -> Vec<u8> {
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}</w:body></w:document>"#
        );
        let mut rels = String::from(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rL" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/x" TargetMode="External"/><Relationship Id="rBad" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="javascript:alert(1)" TargetMode="External"/>"#,
        );
        let mut parts = vec![
            ("_rels/.rels", PACKAGE_RELATIONSHIPS.as_bytes().to_vec()),
            ("word/document.xml", document.into_bytes()),
        ];
        if let Some(numbering) = numbering {
            rels.push_str(r#"<Relationship Id="rN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#);
            parts.push((
                "word/numbering.xml",
                format!(r#"<w:numbering xmlns:w="{W}">{numbering}</w:numbering>"#).into_bytes(),
            ));
        }
        if let Some(styles) = styles {
            rels.push_str(r#"<Relationship Id="rS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="/word/styles.xml"/>"#);
            parts.push((
                "word/styles.xml",
                format!(r#"<w:styles xmlns:w="{W}">{styles}</w:styles>"#).into_bytes(),
            ));
        }
        rels.push_str("</Relationships>");
        parts.push(("word/_rels/document.xml.rels", rels.into_bytes()));
        zip::write(&parts)
    }

    fn markdown_of(bytes: &[u8]) -> (Imported, String) {
        let imported = import(bytes, None).expect("reads");
        let markdown = write_document(&imported.blocks).markdown;
        (imported, markdown)
    }

    #[test]
    fn a_google_style_document_reads_as_a_page() {
        // Google Docs: styles named "heading 1", code as Courier New runs,
        // a checklist's box as the bullet, links through fields.
        let styles = r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/></w:style>"#;
        let numbering = r#"<w:abstractNum w:abstractNumId="7"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="☐"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="8"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum><w:num w:numId="3"><w:abstractNumId w:val="7"/></w:num><w:num w:numId="4"><w:abstractNumId w:val="8"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>"#;
        let body = r#"<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Launch</w:t></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Goals</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">See </w:t></w:r><w:hyperlink r:id="rL"><w:r><w:rPr><w:b/></w:rPr><w:t>this</w:t></w:r></w:hyperlink><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> HYPERLINK "https://example.org/y" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>that</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve">, not </w:t></w:r><w:hyperlink r:id="rBad"><w:r><w:t>this</w:t></w:r></w:hyperlink><w:r><w:t>.</w:t></w:r><w:r><w:drawing/></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr></w:pPr><w:r><w:t>Book a room</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="4"/></w:numPr></w:pPr><w:r><w:t>Fifth</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="4"/></w:numPr></w:pPr><w:r><w:t>Inside</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="4"/></w:numPr></w:pPr><w:r><w:t>Sixth</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New"/></w:rPr><w:t>let x = 1;</w:t></w:r></w:p>
<w:p><w:r><w:rPr><w:rFonts w:ascii="Courier New" w:hAnsi="Courier New"/></w:rPr><w:t>let y = 2;</w:t></w:r></w:p>
<w:p></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>wide</w:t></w:r></w:p><w:p><w:r><w:t>two lines</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:p><w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:t>End</w:t><w:br w:type="page"/></w:r><w:del><w:r><w:delText>gone</w:delText></w:r></w:del><w:ins><w:r><w:t>!</w:t></w:r></w:ins></w:p>"#;
        let (imported, markdown) = markdown_of(&docx(body, Some(numbering), Some(styles)));
        assert_eq!(imported.title.as_deref(), Some("Launch"));
        assert_eq!(imported.set_aside, vec!["footnotes", "images and drawings"]);
        assert_eq!(
            markdown,
            "# Goals\n\nSee [**this**](https://example.com/x) and [that](https://example.org/y), not this.\n\n- [ ] Book a room\n\n5. Fifth\n   - Inside\n6. Sixth\n\n```\nlet x = 1;\nlet y = 2;\n```\n\n| A | B |\n| --- | --- |\n| wide two lines |  |\n\nEnd!"
        );
    }

    #[test]
    fn junk_is_not_a_document() {
        assert_eq!(import(b"hello", None), Err(ImportError::NotDocx));
        let not_word = zip::write(&[("word/document.xml", b"<x/>".to_vec())]);
        assert_eq!(import(&not_word, None), Err(ImportError::NotDocx));
        let dtd = zip::write(&[(
            "word/document.xml",
            br#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY a "aaaa">]><w:document xmlns:w="x"><w:body/></w:document>"#.to_vec(),
        )]);
        assert_eq!(import(&dtd, None), Err(ImportError::NotDocx));
    }
}
