//! The live document behind a page: a Yjs document in the shape the web
//! editor keeps it. Tiptap's nodes sit in the `default` XML fragment, bound
//! by y-tiptap: each node an element named after it with its non-null
//! attributes, each run of text in an XmlText formatted with its marks'
//! attributes (`bold: {}`, `link: {href}`), hard breaks as elements.
//!
//! [`read`] turns the document into a [`Block`] tree, refusing anything the
//! editor couldn't have made; [`replace`] changes the document to match a
//! tree with as small an edit as y-tiptap would make (top-level blocks are
//! matched by their lasting ids), so people typing in it keep their place;
//! [`normalize`] repairs what two people's edits can leave behind (an
//! emptied list, say) and gives every top-level block its own id.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rand_core::{OsRng, RngCore};
use yrs::types::Attrs;
use yrs::types::text::YChange;
use yrs::updates::decoder::Decode;
use yrs::{
    Any, ClientID, Doc, Number, OffsetKind, Options, Out, ReadTxn, StateVector, Text, Transact,
    TransactionMut, Update, WriteTxn, Xml, XmlElementPrelim, XmlElementRef, XmlFragment,
    XmlFragmentRef, XmlOut, XmlTextPrelim, XmlTextRef,
};

use super::markdown::{Align, safe_href};
use super::tree::{Block, Cell, Inline, Item, MAX_LIST_START, Marks, Node};
use crate::page_blocks::embed_href;

/// The fragment Tiptap's collaboration extension binds to.
pub const FRAGMENT: &str = "default";
/// The Yjs client id of changes the server makes. The server edits a page
/// only while it holds the page's row lock, starting from the stored state,
/// so its changes never clash with each other.
pub const SERVER_CLIENT_ID: u64 = 1;
/// The most nesting the editor's document may have.
const MAX_DEPTH: usize = 32;

const PARAGRAPH: &str = "paragraph";
const HEADING: &str = "heading";
const CODE_BLOCK: &str = "codeBlock";
const BLOCKQUOTE: &str = "blockquote";
const BULLET_LIST: &str = "bulletList";
const ORDERED_LIST: &str = "orderedList";
const LIST_ITEM: &str = "listItem";
const HORIZONTAL_RULE: &str = "horizontalRule";
const TABLE: &str = "table";
const TABLE_ROW: &str = "tableRow";
const TABLE_CELL: &str = "tableCell";
const TABLE_HEADER: &str = "tableHeader";
const EMBED: &str = "embed";
const HARD_BREAK: &str = "hardBreak";

const BOLD: &str = "bold";
const ITALIC: &str = "italic";
const STRIKE: &str = "strike";
const CODE: &str = "code";
const LINK: &str = "link";
const MARKS: [&str; 5] = [BOLD, ITALIC, STRIKE, CODE, LINK];

/// Why a document isn't one the editor could have made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid(pub String);

fn invalid<T>(why: impl Into<String>) -> Result<T, Invalid> {
    Err(Invalid(why.into()))
}

/// A document the server edits: Yjs's text offsets in UTF-16 code units, as
/// in the browser.
pub fn new_doc() -> Doc {
    Doc::with_options(Options {
        client_id: ClientID::new(SERVER_CLIENT_ID),
        offset_kind: OffsetKind::Utf16,
        ..Options::default()
    })
}

/// A stored document (a Yjs update holding its whole state).
pub fn load(state: &[u8]) -> Result<Doc, Invalid> {
    let doc = new_doc();
    let update = Update::decode_v1(state).map_err(|_| Invalid("unreadable state".into()))?;
    {
        let mut txn = doc.transact_mut();
        txn.get_or_insert_xml_fragment(FRAGMENT);
        txn.apply_update(update)
            .map_err(|_| Invalid("unreadable state".into()))?;
    }
    Ok(doc)
}

/// The document's whole state, to store or send.
pub fn encode(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

/// What a peer whose state vector is `since` is missing.
pub fn encode_since(doc: &Doc, since: &[u8]) -> Result<Vec<u8>, Invalid> {
    let since =
        StateVector::decode_v1(since).map_err(|_| Invalid("unreadable state vector".into()))?;
    Ok(doc.transact().encode_diff_v1(&since))
}

pub fn state_vector(doc: &Doc) -> Vec<u8> {
    use yrs::updates::encoder::Encode;
    doc.transact().state_vector().encode_v1()
}

pub fn fragment(doc: &Doc) -> XmlFragmentRef {
    doc.get_or_insert_xml_fragment(FRAGMENT)
}

// ---------------------------------------------------------------------------
// Client updates

/// Why a client's update wasn't taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// Not a Yjs update.
    Unreadable,
    /// It builds on changes the server doesn't have: the client should send
    /// everything the server's state vector lacks.
    Resync,
    /// It would make a document the editor couldn't have made.
    Invalid(String),
}

/// Applies a browser's update to `doc` (a copy loaded for this request: on
/// refusal it's thrown away), then repairs and checks the result.
pub fn apply_client_update(doc: &Doc, update: &[u8]) -> Result<(), Refused> {
    let update = Update::decode_v1(update).map_err(|_| Refused::Unreadable)?;
    let server = ClientID::new(SERVER_CLIENT_ID);
    {
        // Only the server makes changes as the server: an update may carry
        // ones it already has, not new ones.
        let known = doc.transact().state_vector().get(&server);
        let forged = update.insertions(true).iter().any(|(client, ranges)| {
            *client == server && ranges.iter().any(|range| range.end > known)
        });
        if forged {
            return Err(Refused::Invalid("changes in the server's name".into()));
        }
    }
    // A malformed update can make the decoder panic; the copy goes with it.
    let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut txn = doc.transact_mut();
        let result = txn.apply_update(update);
        let pending = txn.store().pending_update().is_some() || txn.store().pending_ds().is_some();
        (result.is_ok(), pending)
    }));
    match applied {
        Ok((true, false)) => {}
        Ok((true, true)) => return Err(Refused::Resync),
        _ => return Err(Refused::Unreadable),
    }
    {
        let txn = doc.transact();
        if txn.root_refs().any(|(name, _)| name != FRAGMENT) {
            return Err(Refused::Invalid("a second document".into()));
        }
    }
    normalize(doc);
    read_doc(doc)
        .map(|_| ())
        .map_err(|Invalid(why)| Refused::Invalid(why))
}

// ---------------------------------------------------------------------------
// Reading

/// The document's blocks, or why it isn't one the editor could have made.
pub fn read_doc(doc: &Doc) -> Result<Vec<Block>, Invalid> {
    // The fragment first: getting it opens a write transaction.
    let fragment = fragment(doc);
    let txn = doc.transact();
    read(&txn, &fragment)
}

/// [`read_doc`] inside a transaction.
pub fn read<T: ReadTxn>(txn: &T, fragment: &XmlFragmentRef) -> Result<Vec<Block>, Invalid> {
    element_children(txn, fragment)?
        .iter()
        .map(|element| read_block(txn, element, 0))
        .collect()
}

/// A fragment's or container's children, which must all be elements.
fn element_children<T: ReadTxn>(
    txn: &T,
    parent: &impl XmlFragment,
) -> Result<Vec<XmlElementRef>, Invalid> {
    let mut children = Vec::new();
    for child in parent.children(txn) {
        match child {
            XmlOut::Element(element) => children.push(element),
            _ => return invalid("text where blocks belong"),
        }
    }
    // Values that aren't XML stop the walk short.
    if children.len() != parent.len(txn) as usize {
        return invalid("values where blocks belong");
    }
    Ok(children)
}

fn read_block<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
    depth: usize,
) -> Result<Block, Invalid> {
    if depth > MAX_DEPTH {
        return invalid("nested too deep");
    }
    let id = block_id(txn, element);
    let node = match element.tag().as_ref() {
        PARAGRAPH => Node::Paragraph(read_inline(txn, element, false)?),
        HEADING => Node::Heading {
            level: number_attribute(txn, element, "level")
                .map_or(1, |level| level.clamp(1, 6) as u8),
            content: read_inline(txn, element, false)?,
        },
        CODE_BLOCK => Node::CodeBlock {
            language: string_attribute(txn, element, "language"),
            text: read_inline(txn, element, true)?
                .iter()
                .map(|inline| match inline {
                    Inline::Text { text, .. } => text.as_str(),
                    Inline::HardBreak => "",
                })
                .collect(),
        },
        BLOCKQUOTE => Node::Blockquote(read_container(txn, element, depth)?),
        BULLET_LIST => Node::BulletList(read_items(txn, element, depth)?),
        ORDERED_LIST => Node::OrderedList {
            start: number_attribute(txn, element, "start")
                .map_or(1, |start| start.clamp(0, i64::from(MAX_LIST_START)) as u32),
            items: read_items(txn, element, depth)?,
        },
        HORIZONTAL_RULE => {
            if element.len(txn) > 0 {
                return invalid("a rule with content");
            }
            Node::HorizontalRule
        }
        TABLE => Node::Table(read_table(txn, element)?),
        // Only at the top level, as its Markdown is.
        EMBED if depth > 0 => return invalid("an embed inside another block"),
        EMBED => {
            if element.len(txn) > 0 {
                return invalid("an embed with content");
            }
            let Some(href) = string_attribute(txn, element, "href")
                .as_deref()
                .and_then(embed_href)
            else {
                return invalid("an embed without a path inside the app");
            };
            Node::Embed {
                href,
                label: string_attribute(txn, element, "label").unwrap_or_default(),
            }
        }
        other => return invalid(format!("an unknown block, {other:?}")),
    };
    Ok(Block { id, node })
}

/// Blocks inside a quote or list item: at least one.
fn read_container<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
    depth: usize,
) -> Result<Vec<Block>, Invalid> {
    let children = element_children(txn, element)?;
    if children.is_empty() {
        return invalid("an empty quote or list item");
    }
    children
        .iter()
        .map(|child| read_block(txn, child, depth + 1))
        .collect()
}

fn read_items<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
    depth: usize,
) -> Result<Vec<Item>, Invalid> {
    let children = element_children(txn, element)?;
    if children.is_empty() {
        return invalid("an empty list");
    }
    children
        .iter()
        .map(|child| {
            if child.tag().as_ref() != LIST_ITEM {
                return invalid("a list holding something other than items");
            }
            Ok(Item {
                checked: match child.get_attribute(txn, "checked") {
                    Some(Out::Any(Any::Bool(checked))) => Some(checked),
                    _ => None,
                },
                content: read_container(txn, child, depth + 1)?,
            })
        })
        .collect()
}

fn read_table<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> Result<Vec<Vec<Cell>>, Invalid> {
    let rows = element_children(txn, element)?;
    if rows.is_empty() {
        return invalid("a table without rows");
    }
    rows.iter()
        .map(|row| {
            if row.tag().as_ref() != TABLE_ROW {
                return invalid("a table holding something other than rows");
            }
            element_children(txn, row)?
                .iter()
                .map(|cell| read_cell(txn, cell))
                .collect()
        })
        .collect()
}

fn read_cell<T: ReadTxn>(txn: &T, cell: &XmlElementRef) -> Result<Cell, Invalid> {
    let header = match cell.tag().as_ref() {
        TABLE_HEADER => true,
        TABLE_CELL => false,
        _ => return invalid("a row holding something other than cells"),
    };
    let children = element_children(txn, cell)?;
    let [paragraph] = children.as_slice() else {
        return invalid("a cell without exactly one paragraph");
    };
    if paragraph.tag().as_ref() != PARAGRAPH {
        return invalid("a cell holding something other than a paragraph");
    }
    Ok(Cell {
        header,
        align: string_attribute(txn, cell, "align")
            .as_deref()
            .and_then(Align::from_name),
        colspan: number_attribute(txn, cell, "colspan")
            .map_or(1, |span| span.clamp(1, 1000) as u32),
        rowspan: number_attribute(txn, cell, "rowspan")
            .map_or(1, |span| span.clamp(1, 1000) as u32),
        paragraph_id: block_id(txn, paragraph),
        content: read_inline(txn, paragraph, false)?,
    })
}

/// A paragraph's, heading's or code block's content: runs of text and,
/// outside code, hard breaks.
fn read_inline<T: ReadTxn>(
    txn: &T,
    element: &XmlElementRef,
    code: bool,
) -> Result<Vec<Inline>, Invalid> {
    let mut content = Vec::new();
    let mut count = 0usize;
    for child in element.children(txn) {
        count += 1;
        match child {
            XmlOut::Text(text) => {
                for (chunk, marks) in text_runs(txn, &text)? {
                    if code && !marks.is_empty() {
                        return invalid("formatting in a code block");
                    }
                    content.push(Inline::Text { text: chunk, marks });
                }
            }
            XmlOut::Element(element) if !code && element.tag().as_ref() == HARD_BREAK => {
                if element.len(txn) > 0 {
                    return invalid("a line break with content");
                }
                content.push(Inline::HardBreak);
            }
            _ => return invalid("a block where text belongs"),
        }
    }
    if count != element.len(txn) as usize {
        return invalid("values where text belongs");
    }
    Ok(super::tree::merge_inline(content))
}

/// An XmlText's runs of text and their marks.
fn text_runs<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Result<Vec<(String, Marks)>, Invalid> {
    let mut runs = Vec::new();
    for diff in text.diff(txn, YChange::identity) {
        let Out::Any(Any::String(chunk)) = diff.insert else {
            return invalid("something other than text in text");
        };
        let mut marks = Marks::default();
        for (key, value) in diff
            .attributes
            .iter()
            .flat_map(|attributes| attributes.iter())
        {
            if matches!(value, Any::Null) {
                continue;
            }
            match key.as_ref() {
                BOLD => marks.bold = true,
                ITALIC => marks.italic = true,
                STRIKE => marks.strike = true,
                CODE => marks.code = true,
                LINK => {
                    let href = match value {
                        Any::Map(attributes) => match attributes.get("href") {
                            Some(Any::String(href)) => href.to_string(),
                            _ => return invalid("a link without an address"),
                        },
                        _ => return invalid("a link without an address"),
                    };
                    if safe_href(&href).is_none() {
                        return invalid("a link to an address pages don't allow");
                    }
                    marks.link = Some(href);
                }
                other => return invalid(format!("an unknown mark, {other:?}")),
            }
        }
        runs.push((chunk.to_string(), marks));
    }
    Ok(runs)
}

/// A block's id when it has a usable one.
fn block_id<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> Option<String> {
    string_attribute(txn, element, "id").filter(|id| valid_block_id(id))
}

fn string_attribute<T: ReadTxn>(txn: &T, element: &XmlElementRef, name: &str) -> Option<String> {
    match element.get_attribute(txn, name) {
        Some(Out::Any(Any::String(value))) => Some(value.to_string()),
        _ => None,
    }
}

fn number_attribute<T: ReadTxn>(txn: &T, element: &XmlElementRef, name: &str) -> Option<i64> {
    match element.get_attribute(txn, name) {
        Some(Out::Any(Any::Number(number))) => {
            number_value(number).map(|value| value.round() as i64)
        }
        _ => None,
    }
}

fn number_value(number: Number) -> Option<f64> {
    match number {
        Number::Int(value) => Some(value as f64),
        Number::Float(value) => value.is_finite().then_some(value),
    }
}

/// Ids the editor and the server make, and agents pass back. An id has a
/// letter in it, so it can't be mistaken for a block's position.
pub fn valid_block_id(id: &str) -> bool {
    (4..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && id.bytes().any(|byte| byte.is_ascii_alphabetic())
}

/// A new block id: a lowercase letter, then nine random lowercase letters
/// and digits.
pub fn new_block_id() -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut value = OsRng.next_u64();
    let mut id = String::with_capacity(10);
    id.push(char::from(DIGITS[10 + (value % 26) as usize]));
    value /= 26;
    for _ in 1..10 {
        id.push(char::from(DIGITS[(value % 36) as usize]));
        value /= 36;
    }
    id
}

// ---------------------------------------------------------------------------
// Repair

/// Repairs what concurrent edits can leave behind, as the editor's schema
/// needs it: quotes, lists, items and tables nothing is left in go, a cell
/// keeps one paragraph, the document keeps a block, and every top-level
/// block has its own id. Returns whether anything changed.
pub fn normalize(doc: &Doc) -> bool {
    let fragment = fragment(doc);
    let mut txn = doc.transact_mut();
    let mut changed = false;
    let children: Vec<XmlOut> = fragment.children(&txn).collect();
    for (index, child) in children.iter().enumerate().rev() {
        if let XmlOut::Element(element) = child
            && prune(&mut txn, element, 0, &mut changed)
        {
            fragment.remove_range(&mut txn, index as u32, 1);
            changed = true;
        }
    }
    if fragment.len(&txn) == 0 {
        fragment.insert(&mut txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        changed = true;
    }
    changed |= ensure_ids(&mut txn, &fragment);
    changed
}

/// Repairs inside `element`; true when it's a container left empty, to go.
fn prune(
    txn: &mut TransactionMut,
    element: &XmlElementRef,
    depth: usize,
    changed: &mut bool,
) -> bool {
    let tag = element.tag().clone();
    let container = matches!(
        tag.as_ref(),
        BLOCKQUOTE | BULLET_LIST | ORDERED_LIST | LIST_ITEM | TABLE
    );
    if depth > MAX_DEPTH || !(container || tag.as_ref() == TABLE_ROW || is_cell(&tag)) {
        return false;
    }
    let children: Vec<XmlOut> = element.children(txn).collect();
    for (index, child) in children.iter().enumerate().rev() {
        if let XmlOut::Element(child) = child
            && prune(txn, child, depth + 1, changed)
        {
            element.remove_range(txn, index as u32, 1);
            *changed = true;
        }
    }
    if is_cell(&tag) {
        let count = element.len(txn);
        if count == 0 {
            element.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
            *changed = true;
        } else if count > 1 {
            element.remove_range(txn, 1, count - 1);
            *changed = true;
        }
        return false;
    }
    container && element.len(txn) == 0
}

fn is_cell(tag: &str) -> bool {
    tag == TABLE_CELL || tag == TABLE_HEADER
}

/// Gives top-level blocks without a usable id, or with one an earlier block
/// has, a new one.
fn ensure_ids(txn: &mut TransactionMut, fragment: &XmlFragmentRef) -> bool {
    let mut seen = HashSet::new();
    let mut changed = false;
    let children: Vec<XmlOut> = fragment.children(txn).collect();
    for child in children {
        let XmlOut::Element(element) = child else {
            continue;
        };
        match block_id(txn, &element) {
            Some(id) if seen.insert(id.clone()) => {}
            _ => {
                let mut id = new_block_id();
                while !seen.insert(id.clone()) {
                    id = new_block_id();
                }
                element.insert_attribute(txn, "id", Any::String(id.into()));
                changed = true;
            }
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// Writing

/// A node as y-tiptap would put it in the document: an element with every
/// non-null attribute, or a run of text with its marks' attributes.
#[derive(Clone, Debug)]
enum PNode {
    Element {
        name: &'static str,
        attributes: Vec<(&'static str, Any)>,
        children: Vec<PNode>,
    },
    Text(Vec<(String, Attrs)>),
}

fn element(
    name: &'static str,
    attributes: Vec<(&'static str, Any)>,
    children: Vec<PNode>,
) -> PNode {
    PNode::Element {
        name,
        attributes,
        children,
    }
}

fn string(value: &str) -> Any {
    Any::String(value.into())
}

fn int(value: i64) -> Any {
    Any::Number(Number::Int(value))
}

fn id_attributes(id: &Option<String>) -> Vec<(&'static str, Any)> {
    id.iter().map(|id| ("id", string(id))).collect()
}

fn p_block(block: &Block) -> PNode {
    let mut attributes = id_attributes(&block.id);
    match &block.node {
        Node::Paragraph(content) => element(PARAGRAPH, attributes, p_inline(content)),
        Node::Heading { level, content } => {
            attributes.push(("level", int(i64::from(*level))));
            element(HEADING, attributes, p_inline(content))
        }
        Node::CodeBlock { language, text } => {
            if let Some(language) = language {
                attributes.push(("language", string(language)));
            }
            let children = if text.is_empty() {
                Vec::new()
            } else {
                vec![PNode::Text(vec![(text.clone(), Attrs::new())])]
            };
            element(CODE_BLOCK, attributes, children)
        }
        Node::Blockquote(children) => element(BLOCKQUOTE, attributes, p_blocks(children)),
        Node::BulletList(items) => element(BULLET_LIST, attributes, p_items(items)),
        Node::OrderedList { start, items } => {
            attributes.push(("start", int(i64::from(*start))));
            element(ORDERED_LIST, attributes, p_items(items))
        }
        Node::HorizontalRule => element(HORIZONTAL_RULE, attributes, Vec::new()),
        Node::Table(rows) => element(
            TABLE,
            attributes,
            rows.iter()
                .map(|row| element(TABLE_ROW, Vec::new(), row.iter().map(p_cell).collect()))
                .collect(),
        ),
        Node::Embed { href, label } => {
            attributes.push(("href", string(href)));
            if !label.is_empty() {
                attributes.push(("label", string(label)));
            }
            element(EMBED, attributes, Vec::new())
        }
    }
}

/// A quote's or item's blocks: the editor's hold at least one, so an empty
/// one holds an empty paragraph (which Markdown leaves out again).
fn p_blocks(blocks: &[Block]) -> Vec<PNode> {
    if blocks.is_empty() {
        vec![element(PARAGRAPH, Vec::new(), Vec::new())]
    } else {
        blocks.iter().map(p_block).collect()
    }
}

fn p_items(items: &[Item]) -> Vec<PNode> {
    items
        .iter()
        .map(|item| {
            let attributes = item
                .checked
                .map(|checked| vec![("checked", Any::Bool(checked))])
                .unwrap_or_default();
            element(LIST_ITEM, attributes, p_blocks(&item.content))
        })
        .collect()
}

fn p_cell(cell: &Cell) -> PNode {
    let mut attributes = vec![
        ("colspan", int(i64::from(cell.colspan))),
        ("rowspan", int(i64::from(cell.rowspan))),
    ];
    if let Some(align) = cell.align {
        attributes.push(("align", string(align.name())));
    }
    element(
        if cell.header {
            TABLE_HEADER
        } else {
            TABLE_CELL
        },
        attributes,
        vec![element(
            PARAGRAPH,
            id_attributes(&cell.paragraph_id),
            p_inline(&cell.content),
        )],
    )
}

/// Inline content as y-tiptap groups it: runs of text together, breaks as
/// elements between them.
fn p_inline(content: &[Inline]) -> Vec<PNode> {
    let mut children = Vec::new();
    let mut runs: Vec<(String, Attrs)> = Vec::new();
    for inline in super::tree::merge_inline(content.to_vec()) {
        match inline {
            Inline::Text { text, marks } => runs.push((text, mark_attributes(&marks))),
            Inline::HardBreak => {
                if !runs.is_empty() {
                    children.push(PNode::Text(std::mem::take(&mut runs)));
                }
                children.push(element(HARD_BREAK, Vec::new(), Vec::new()));
            }
        }
    }
    if !runs.is_empty() {
        children.push(PNode::Text(runs));
    }
    children
}

fn mark_attributes(marks: &Marks) -> Attrs {
    let mut attributes = Attrs::new();
    let empty = || Any::Map(Arc::new(HashMap::new()));
    for (on, name) in [
        (marks.bold, BOLD),
        (marks.italic, ITALIC),
        (marks.strike, STRIKE),
        (marks.code, CODE),
    ] {
        if on {
            attributes.insert(name.into(), empty());
        }
    }
    if let Some(href) = &marks.link {
        attributes.insert(
            LINK.into(),
            Any::Map(Arc::new(HashMap::from([("href".to_owned(), string(href))]))),
        );
    }
    attributes
}

fn insert_p(txn: &mut TransactionMut, parent: &impl XmlFragment, index: u32, node: &PNode) {
    match node {
        PNode::Element {
            name,
            attributes,
            children,
        } => {
            let element = parent.insert(txn, index, XmlElementPrelim::empty(*name));
            for (key, value) in attributes {
                element.insert_attribute(txn, *key, value.clone());
            }
            for (at, child) in children.iter().enumerate() {
                insert_p(txn, &element, at as u32, child);
            }
        }
        PNode::Text(runs) => {
            let whole: String = runs.iter().map(|(run, _)| run.as_str()).collect();
            let text = parent.insert(txn, index, XmlTextPrelim::new(whole));
            format_runs(txn, &text, runs, false);
        }
    }
}

/// Gives each run its marks one mark at a time, in [`MARKS`] order, and with
/// `clear` takes off the marks it lacks. yrs applies a run's attributes in
/// hash order, which would make the same change a different document each
/// time; one at a time, the same change always makes the same document.
fn format_runs(txn: &mut TransactionMut, text: &XmlTextRef, runs: &[(String, Attrs)], clear: bool) {
    let mut at = 0;
    for (run, attributes) in runs {
        let length = utf16_len(run);
        for name in MARKS {
            let value = match attributes.get(name) {
                Some(value) => value.clone(),
                None if clear => Any::Null,
                None => continue,
            };
            text.format(
                txn,
                at,
                length,
                Attrs::from([(Arc::<str>::from(name), value)]),
            );
        }
        at += length;
    }
}

/// Fills an empty document with `blocks`.
pub fn write(doc: &Doc, blocks: &[Block]) {
    let fragment = fragment(doc);
    let mut txn = doc.transact_mut();
    for (index, block) in blocks.iter().enumerate() {
        insert_p(&mut txn, &fragment, index as u32, &p_block(block));
    }
}

/// Changes the document to hold `blocks`. A top-level block whose id is in
/// the document is that block, changed in place where it still sits in the
/// same order (moved ones are taken out and put back); blocks without an id
/// are new. Inside a block the edit is y-tiptap's: text changes where it
/// differs, so others' cursors and concurrent typing survive.
pub fn replace(doc: &Doc, blocks: &[Block]) {
    let fragment = fragment(doc);
    let mut txn = doc.transact_mut();
    let current: Vec<(Option<String>, XmlElementRef)> = fragment
        .children(&txn)
        .filter_map(XmlOut::into_xml_element)
        .map(|element| (block_id(&txn, &element), element))
        .collect();
    let mut position: HashMap<&str, usize> = HashMap::new();
    for (index, (id, _)) in current.iter().enumerate() {
        if let Some(id) = id {
            position.entry(id.as_str()).or_insert(index);
        }
    }
    // The blocks that stay where they are: the longest run of matched ids
    // already in order.
    let matched: Vec<(usize, usize)> = blocks
        .iter()
        .enumerate()
        .filter_map(|(new, block)| {
            let id = block.id.as_deref()?;
            position.get(id).map(|&old| (new, old))
        })
        .collect();
    let kept: HashMap<usize, usize> = longest_increasing(&matched).into_iter().collect();
    let kept_old: HashSet<usize> = kept.values().copied().collect();
    for index in (0..current.len()).rev() {
        if !kept_old.contains(&index) {
            fragment.remove_range(&mut txn, index as u32, 1);
        }
    }
    // Ids taken: the kept blocks', then each new block's as it goes in.
    let mut used: HashSet<String> = kept
        .keys()
        .filter_map(|&index| blocks[index].id.clone())
        .collect();
    for (index, block) in blocks.iter().enumerate() {
        let mut node = p_block(block);
        if let Some(old) = kept.get(&index) {
            let element = &current[*old].1;
            if same_element(&XmlOut::Element(element.clone()), &node) {
                update_element(&mut txn, element, &node);
            } else {
                // Another kind of block now: the same id on a new element.
                fragment.remove_range(&mut txn, index as u32, 1);
                insert_p(&mut txn, &fragment, index as u32, &node);
            }
            continue;
        }
        // A new block, or one moved: it keeps its id unless that's taken.
        let mut id = block
            .id
            .clone()
            .filter(|id| valid_block_id(id) && !used.contains(id))
            .unwrap_or_else(new_block_id);
        while used.contains(&id) {
            id = new_block_id();
        }
        used.insert(id.clone());
        if let PNode::Element { attributes, .. } = &mut node {
            attributes.retain(|(key, _)| *key != "id");
            attributes.insert(0, ("id", string(&id)));
        }
        insert_p(&mut txn, &fragment, index as u32, &node);
    }
}

/// The longest subsequence of `pairs` (in order of the first number) whose
/// second numbers increase.
fn longest_increasing(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // Patience sorting: tails[k] is the index of the smallest second number
    // ending an increasing run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; pairs.len()];
    for (index, &(_, value)) in pairs.iter().enumerate() {
        let at = tails.partition_point(|&tail| pairs[tail].1 < value);
        if at > 0 {
            previous[index] = Some(tails[at - 1]);
        }
        if at == tails.len() {
            tails.push(index);
        } else {
            tails[at] = index;
        }
    }
    let mut run = Vec::new();
    let mut cursor = tails.last().copied();
    while let Some(index) = cursor {
        run.push(pairs[index]);
        cursor = previous[index];
    }
    run.reverse();
    run
}

// ---------------------------------------------------------------------------
// Updating in place (y-prosemirror's updateYFragment, without its mapping)

fn update_element(txn: &mut TransactionMut, y: &XmlElementRef, p: &PNode) {
    let PNode::Element {
        attributes,
        children,
        ..
    } = p
    else {
        return;
    };
    let current = element_attributes(txn, y);
    for (key, value) in attributes {
        if !current.get(*key).is_some_and(|old| any_eq(old, value)) {
            y.insert_attribute(txn, *key, value.clone());
        }
    }
    for key in current.keys() {
        if !attributes.iter().any(|(name, _)| name == key) {
            y.remove_attribute(txn, key);
        }
    }
    update_children(txn, y, children);
}

fn element_attributes<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> HashMap<String, Any> {
    element
        .attributes(txn)
        .filter_map(|(key, value)| match value {
            Out::Any(Any::Null) => None,
            Out::Any(value) => Some((key.to_owned(), value)),
            // A shared type as a value isn't the editor's: replace it.
            _ => Some((key.to_owned(), Any::Undefined)),
        })
        .collect()
}

fn update_children(txn: &mut TransactionMut, parent: &XmlElementRef, p_children: &[PNode]) {
    let y_children: Vec<XmlOut> = parent.children(txn).collect();
    let (y_count, p_count) = (y_children.len(), p_children.len());
    let shortest = y_count.min(p_count);
    let mut left = 0;
    let mut right = 0;
    while left < shortest && equal(txn, &y_children[left], &p_children[left]) {
        left += 1;
    }
    while right + left + 1 < shortest
        && equal(
            txn,
            &y_children[y_count - right - 1],
            &p_children[p_count - right - 1],
        )
    {
        right += 1;
    }
    while y_count - left - right > 0 && p_count - left - right > 0 {
        let (left_y, left_p) = (&y_children[left], &p_children[left]);
        let (right_y, right_p) = (
            &y_children[y_count - right - 1],
            &p_children[p_count - right - 1],
        );
        if let (XmlOut::Text(text), PNode::Text(runs)) = (left_y, left_p) {
            if !text_equal(txn, text, runs) {
                update_text(txn, text, runs);
            }
            left += 1;
            continue;
        }
        let mut update_left = same_element(left_y, left_p);
        let mut update_right = same_element(right_y, right_p);
        if update_left && update_right {
            // Update whichever side has more children in common.
            let factor_left = equality_factor(txn, left_y, left_p);
            let factor_right = equality_factor(txn, right_y, right_p);
            if factor_left < factor_right {
                update_left = false;
            } else {
                update_right = false;
            }
        }
        if update_left {
            if let XmlOut::Element(element) = left_y {
                update_element(txn, element, left_p);
            }
            left += 1;
        } else if update_right {
            if let XmlOut::Element(element) = right_y {
                update_element(txn, element, right_p);
            }
            right += 1;
        } else {
            parent.remove_range(txn, left as u32, 1);
            insert_p(txn, parent, left as u32, left_p);
            left += 1;
        }
    }
    let removed = y_count - left - right;
    if y_count == 1 && p_count == 0 && matches!(y_children[0], XmlOut::Text(_)) {
        // Empty the text rather than remove it, to keep others' typing in it.
        if let XmlOut::Text(text) = &y_children[0] {
            let length = text.len(txn);
            if length > 0 {
                text.remove_range(txn, 0, length);
            }
        }
    } else if removed > 0 {
        parent.remove_range(txn, left as u32, removed as u32);
    }
    for (offset, child) in p_children[left..p_count - right].iter().enumerate() {
        insert_p(txn, parent, (left + offset) as u32, child);
    }
}

fn same_element(y: &XmlOut, p: &PNode) -> bool {
    matches!((y, p), (XmlOut::Element(element), PNode::Element { name, .. }) if element.tag().as_ref() == *name)
}

fn equality_factor<T: ReadTxn>(txn: &T, y: &XmlOut, p: &PNode) -> usize {
    let (XmlOut::Element(element), PNode::Element { children, .. }) = (y, p) else {
        return 0;
    };
    let y_children: Vec<XmlOut> = element.children(txn).collect();
    let shortest = y_children.len().min(children.len());
    let mut left = 0;
    while left < shortest && equal(txn, &y_children[left], &children[left]) {
        left += 1;
    }
    let mut right = 0;
    while left + right < shortest
        && equal(
            txn,
            &y_children[y_children.len() - right - 1],
            &children[children.len() - right - 1],
        )
    {
        right += 1;
    }
    left + right
}

/// Whether a node in the document is already `p`.
fn equal<T: ReadTxn>(txn: &T, y: &XmlOut, p: &PNode) -> bool {
    match (y, p) {
        (
            XmlOut::Element(element),
            PNode::Element {
                name,
                attributes,
                children,
            },
        ) => {
            if element.tag().as_ref() != *name || element.len(txn) as usize != children.len() {
                return false;
            }
            let current = element_attributes(txn, element);
            current.len() == attributes.len()
                && attributes
                    .iter()
                    .all(|(key, value)| current.get(*key).is_some_and(|old| any_eq(old, value)))
                && element
                    .children(txn)
                    .zip(children)
                    .all(|(y_child, p_child)| equal(txn, &y_child, p_child))
        }
        (XmlOut::Text(text), PNode::Text(runs)) => text_equal(txn, text, runs),
        _ => false,
    }
}

fn text_equal<T: ReadTxn>(txn: &T, text: &XmlTextRef, runs: &[(String, Attrs)]) -> bool {
    let diffs = text.diff(txn, YChange::identity);
    diffs.len() == runs.len()
        && diffs.iter().zip(runs).all(|(diff, (run, attributes))| {
            let Out::Any(Any::String(chunk)) = &diff.insert else {
                return false;
            };
            let current: Vec<(&Arc<str>, &Any)> = diff
                .attributes
                .iter()
                .flat_map(|attributes| attributes.iter())
                .filter(|(_, value)| !matches!(value, Any::Null))
                .collect();
            chunk.as_ref() == run
                && current.len() == attributes.len()
                && current.iter().all(|(key, value)| {
                    attributes
                        .get(*key)
                        .is_some_and(|wanted| any_eq(value, wanted))
                })
        })
}

/// y-prosemirror's updateYText: the text changes where it differs (one
/// replaced stretch), then every run gets exactly its marks.
fn update_text(txn: &mut TransactionMut, text: &XmlTextRef, runs: &[(String, Attrs)]) {
    let current: String = text
        .diff(txn, YChange::identity)
        .into_iter()
        .filter_map(|diff| match diff.insert {
            Out::Any(Any::String(chunk)) => Some(chunk.to_string()),
            _ => None,
        })
        .collect();
    let wanted: String = runs.iter().map(|(run, _)| run.as_str()).collect();
    let (index, removed, inserted) = simple_diff(&current, &wanted);
    if removed > 0 {
        text.remove_range(txn, index, removed);
    }
    if !inserted.is_empty() {
        text.insert(txn, index, &inserted);
    }
    format_runs(txn, text, runs, true);
}

/// Where two strings differ: the UTF-16 offset of the first difference, how
/// many units of `old` to remove there and what to insert (common prefix and
/// suffix kept, never splitting a character).
fn simple_diff(old: &str, new: &str) -> (u32, u32, String) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let prefix = old_chars
        .iter()
        .zip(&new_chars)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_chars[prefix..]
        .iter()
        .rev()
        .zip(new_chars[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let units = |characters: &[char]| characters.iter().map(|c| c.len_utf16() as u32).sum::<u32>();
    (
        units(&old_chars[..prefix]),
        units(&old_chars[prefix..old_chars.len() - suffix]),
        new_chars[prefix..new_chars.len() - suffix].iter().collect(),
    )
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// Values equal as JavaScript sees them: 1 and 1.0 are the same number.
fn any_eq(a: &Any, b: &Any) -> bool {
    match (a, b) {
        (Any::Number(a), Any::Number(b)) => number_value(*a) == number_value(*b),
        (Any::Map(a), Any::Map(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| any_eq(value, other)))
        }
        (Any::Array(a), Any::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| any_eq(a, b))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::testing::{Random, random_blocks};
    use crate::page_docs::tree::{canon_blocks, from_markdown, strip_ids};
    use yrs::In;
    use yrs::types::Delta;
    use yrs::types::xml::{XmlDeltaPrelim, XmlIn};

    /// A change a test browser makes to its copy of a page.
    type Change = Box<dyn FnOnce(&mut TransactionMut, &XmlFragmentRef)>;

    fn blocks_of(doc: &Doc) -> Vec<Block> {
        read_doc(doc).expect("the editor's shape")
    }

    fn doc_with(blocks: &[Block]) -> Doc {
        let doc = new_doc();
        write(&doc, blocks);
        normalize(&doc);
        doc
    }

    #[test]
    fn documents_read_back_as_written() {
        let mut random = Random(0x0d0c);
        for _ in 0..2000 {
            let count = 1 + random.below(4);
            let blocks = canon_blocks(&random_blocks(&mut random, 0, count));
            let doc = doc_with(&blocks);
            let read = blocks_of(&doc);
            // Every top-level block now has an id; the content is the same.
            assert!(read.iter().all(|block| block.id.is_some()));
            let read = canon_blocks(&read);
            for (got, wanted) in strip_ids(&read).iter().zip(strip_ids(&blocks).iter()) {
                assert_eq!(got, wanted);
            }
            assert_eq!(read.len(), blocks.len());
            // Stored and loaded, it's the same document.
            let loaded = load(&encode(&doc)).expect("loads");
            assert_eq!(canon_blocks(&blocks_of(&loaded)), read);
        }
    }

    #[test]
    fn the_same_change_always_makes_the_same_document() {
        // Several marks on one run is where yrs's hash order showed through.
        let mut blocks = from_markdown(
            "Some **bold `code`**, ~~*both*~~ and **[a link](https://example.com)**.\n",
        );
        blocks[0].id = Some("block0".to_owned());
        let mut changed = blocks.clone();
        changed[0].node = Node::Paragraph(
            match &from_markdown("Some *bold `code`*, ~~**both**~~ and [a link](/a).\n")[0].node {
                Node::Paragraph(content) => content.clone(),
                other => panic!("{other:?}"),
            },
        );
        let make = || {
            let doc = new_doc();
            write(&doc, &blocks);
            let first = encode(&doc);
            replace(&doc, &changed);
            (first, encode(&doc))
        };
        let once = make();
        for _ in 0..20 {
            assert_eq!(make(), once);
        }
    }

    /// A random change to `blocks`: some kept as they are, some changed in
    /// place, some removed, some new, some moved.
    fn mutate(random: &mut Random, blocks: &[Block]) -> Vec<Block> {
        let mut next: Vec<Block> = Vec::new();
        for block in blocks {
            match random.below(10) {
                0 => {}
                1 => {
                    let fresh = random_blocks(random, 0, 1);
                    next.extend(canon_blocks(&fresh));
                    next.push(block.clone());
                }
                2 | 3 => {
                    let mut changed = canon_blocks(&random_blocks(random, 0, 1));
                    if let Some(first) = changed.first_mut() {
                        first.id = block.id.clone();
                    }
                    next.extend(changed);
                }
                _ => next.push(block.clone()),
            }
        }
        if next.len() > 1 && random.chance(30) {
            let from = random.below(next.len());
            let block = next.remove(from);
            let to = random.below(next.len() + 1);
            next.insert(to, block);
        }
        next
    }

    #[test]
    fn replacing_keeps_ids_and_reads_back() {
        let mut random = Random(0x5e91);
        for _ in 0..1500 {
            let count = 1 + random.below(5);
            let doc = doc_with(&canon_blocks(&random_blocks(&mut random, 0, count)));
            let before = blocks_of(&doc);
            let wanted = mutate(&mut random, &before);
            replace(&doc, &wanted);
            normalize(&doc);
            let after = blocks_of(&doc);
            let expected = if wanted.is_empty() {
                vec![Block::new(Node::Paragraph(Vec::new()))]
            } else {
                wanted.clone()
            };
            assert_eq!(
                strip_ids(&canon_blocks(&after)),
                strip_ids(&canon_blocks(&expected)),
                "{wanted:?}"
            );
            // A block that asked for an id it could keep has it.
            let mut seen = HashSet::new();
            for (block, got) in wanted.iter().zip(&after) {
                if let Some(id) = &block.id
                    && seen.insert(id.clone())
                {
                    assert_eq!(got.id.as_ref(), Some(id));
                }
            }
            let ids: HashSet<_> = after.iter().map(|block| block.id.clone()).collect();
            assert_eq!(ids.len(), after.len(), "ids are unique");
        }
    }

    /// A peer: a Yjs document with its own client id, synced with the
    /// server's.
    fn peer(of: &Doc, client: u64) -> Doc {
        let doc = Doc::with_options(Options {
            client_id: ClientID::new(client),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        doc.get_or_insert_xml_fragment(FRAGMENT);
        let update = Update::decode_v1(&encode(of)).expect("update");
        doc.transact_mut().apply_update(update).expect("applies");
        doc
    }

    fn sync(from: &Doc, to: &Doc) {
        let since = to.transact().state_vector();
        let update = from.transact().encode_diff_v1(&since);
        to.transact_mut()
            .apply_update(Update::decode_v1(&update).expect("update"))
            .expect("applies");
    }

    fn first_text(doc: &Doc, block: u32) -> XmlTextRef {
        let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
        let txn = doc.transact();
        let Some(XmlOut::Element(element)) = fragment.get(&txn, block) else {
            panic!("no block {block}");
        };
        let Some(XmlOut::Text(text)) = element.get(&txn, 0) else {
            panic!("no text in block {block}");
        };
        text
    }

    #[test]
    fn an_agents_edit_and_a_persons_typing_both_land() {
        let doc = doc_with(&from_markdown(
            "# Plan\n\nShip the editor this week.\n\nThen tidy up.",
        ));
        let person = peer(&doc, 77);
        // The person types in the second paragraph...
        {
            let text = first_text(&person, 2);
            let mut txn = person.transact_mut();
            text.insert(&mut txn, 4, " later");
        }
        // ...while an agent rewrites the first one through Markdown, by id.
        let mut blocks = blocks_of(&doc);
        blocks[1].node = from_markdown("Ship the **live** editor this week.")
            .remove(0)
            .node;
        replace(&doc, &blocks);
        sync(&person, &doc);
        sync(&doc, &person);
        let markdown = |doc: &Doc| super::super::write::write_document(&blocks_of(doc)).markdown;
        assert_eq!(
            markdown(&doc),
            "# Plan\n\nShip the **live** editor this week.\n\nThen later tidy up."
        );
        assert_eq!(markdown(&person), markdown(&doc));
        // The agent's edit changed the text in place: the paragraph's text
        // is the one the person had.
        assert_eq!(blocks_of(&doc)[1].id, blocks[1].id);
    }

    #[test]
    fn typing_in_the_same_paragraph_merges() {
        let doc = doc_with(&from_markdown("one two three"));
        let person = peer(&doc, 78);
        {
            let text = first_text(&person, 0);
            let mut txn = person.transact_mut();
            text.insert(&mut txn, 0, "zero ");
        }
        let mut blocks = blocks_of(&doc);
        blocks[0].node = from_markdown("one two three four").remove(0).node;
        replace(&doc, &blocks);
        sync(&person, &doc);
        assert_eq!(
            super::super::write::write_document(&blocks_of(&doc)).markdown,
            "zero one two three four"
        );
    }

    fn client_update(
        doc: &Doc,
        client: u64,
        change: impl FnOnce(&mut TransactionMut, &XmlFragmentRef),
    ) -> Vec<u8> {
        let other = peer(doc, client);
        let before = other.transact().state_vector();
        {
            let fragment = other.get_or_insert_xml_fragment(FRAGMENT);
            let mut txn = other.transact_mut();
            change(&mut txn, &fragment);
        }
        other.transact().encode_diff_v1(&before)
    }

    #[test]
    fn browsers_updates_that_the_editor_could_make_are_taken() {
        let doc = doc_with(&from_markdown("- a\n- b"));
        // Empties the list's items concurrently-style: the list goes.
        let update = client_update(&doc, 90, |txn, fragment| {
            let Some(XmlOut::Element(list)) = fragment.get(txn, 0) else {
                panic!("list");
            };
            list.remove_range(txn, 0, 2);
            let paragraph = fragment.insert(txn, 1, XmlElementPrelim::empty(PARAGRAPH));
            paragraph.insert(
                txn,
                0,
                XmlIn::from(XmlDeltaPrelim {
                    attributes: HashMap::new(),
                    delta: vec![Delta::Inserted(
                        In::Any(string("hi")),
                        Some(Box::new(mark_attributes(&Marks {
                            bold: true,
                            link: Some("https://example.com".into()),
                            ..Marks::default()
                        }))),
                    )],
                }),
            );
        });
        assert_eq!(apply_client_update(&doc, &update), Ok(()));
        let blocks = blocks_of(&doc);
        assert_eq!(blocks.len(), 1, "{blocks:?}");
        assert!(blocks[0].id.is_some());
        assert_eq!(
            super::super::write::write_document(&blocks).markdown,
            "[**hi**](https://example.com)"
        );

        // An embed the editor adds, with a label its Markdown escapes.
        let update = client_update(&doc, 92, |txn, fragment| {
            let embed = fragment.insert(txn, 1, XmlElementPrelim::empty(EMBED));
            embed.insert_attribute(txn, "href", string("/projects/p/collections/c?view=v"));
            embed.insert_attribute(txn, "label", string("Feedback [all]"));
        });
        assert_eq!(apply_client_update(&doc, &update), Ok(()));
        assert_eq!(
            super::super::write::write_document(&blocks_of(&doc)).markdown,
            "[**hi**](https://example.com)\n\n[Feedback \\[all\\]](/projects/p/collections/c?view=v \"embed\")"
        );
    }

    #[test]
    fn browsers_updates_the_editor_couldnt_make_are_refused() {
        let doc = doc_with(&from_markdown("text"));
        let refused = |change: Change| {
            let update = client_update(&doc, 91, change);
            matches!(
                apply_client_update(&load(&encode(&doc)).expect("load"), &update),
                Err(Refused::Invalid(_))
            )
        };
        assert!(refused(Box::new(|txn, fragment| {
            fragment.insert(txn, 0, XmlElementPrelim::empty("script"));
        })));
        assert!(refused(Box::new(|txn, fragment| {
            let paragraph = fragment.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
            let mut attributes = Attrs::new();
            attributes.insert(
                LINK.into(),
                Any::Map(Arc::new(HashMap::from([(
                    "href".to_owned(),
                    string("javascript:alert(1)"),
                )]))),
            );
            paragraph.insert(
                txn,
                0,
                XmlIn::from(XmlDeltaPrelim {
                    attributes: HashMap::new(),
                    delta: vec![Delta::Inserted(
                        In::Any(string("x")),
                        Some(Box::new(attributes)),
                    )],
                }),
            );
        })));
        assert!(refused(Box::new(|txn, fragment| {
            let paragraph = fragment.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
            let mut attributes = Attrs::new();
            attributes.insert("underline".into(), Any::Map(Arc::new(HashMap::new())));
            paragraph.insert(
                txn,
                0,
                XmlIn::from(XmlDeltaPrelim {
                    attributes: HashMap::new(),
                    delta: vec![Delta::Inserted(
                        In::Any(string("x")),
                        Some(Box::new(attributes)),
                    )],
                }),
            );
        })));
        assert!(refused(Box::new(|txn, fragment| {
            let quote = fragment.insert(txn, 0, XmlElementPrelim::empty(BLOCKQUOTE));
            let item = quote.insert(txn, 0, XmlElementPrelim::empty(LIST_ITEM));
            item.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        })));
        assert!(refused(Box::new(|txn, _| {
            let other = txn.get_or_insert_map("other");
            yrs::Map::insert(&other, txn, "key", "value");
        })));
        // An embed sits at the top level and shows a path inside the app.
        assert!(refused(Box::new(|txn, fragment| {
            let quote = fragment.insert(txn, 0, XmlElementPrelim::empty(BLOCKQUOTE));
            let embed = quote.insert(txn, 0, XmlElementPrelim::empty(EMBED));
            embed.insert_attribute(txn, "href", string("/projects"));
        })));
        for href in [
            "https://example.com/x",
            "//example.com",
            "/a b",
            "/a\u{a0}b",
            "",
        ] {
            assert!(
                refused(Box::new(move |txn, fragment| {
                    let embed = fragment.insert(txn, 0, XmlElementPrelim::empty(EMBED));
                    embed.insert_attribute(txn, "href", string(href));
                })),
                "{href:?}"
            );
        }
        assert!(refused(Box::new(|txn, fragment| {
            fragment.insert(txn, 0, XmlElementPrelim::empty(EMBED));
        })));
        assert!(refused(Box::new(|txn, fragment| {
            let embed = fragment.insert(txn, 0, XmlElementPrelim::empty(EMBED));
            embed.insert_attribute(txn, "href", string("/projects"));
            embed.insert(txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        })));
        // Changes in the server's name: a second copy of the server's
        // document, edited, as a browser would send it.
        let forger = load(&encode(&doc)).expect("load");
        let before = forger.transact().state_vector();
        {
            let fragment = fragment(&forger);
            let mut txn = forger.transact_mut();
            fragment.insert(&mut txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        }
        let forged = forger.transact().encode_diff_v1(&before);
        let result = apply_client_update(&load(&encode(&doc)).expect("load"), &forged);
        assert!(matches!(result, Err(Refused::Invalid(_))), "{result:?}");
        assert_eq!(
            apply_client_update(&load(&encode(&doc)).expect("load"), b"\xff\xff"),
            Err(Refused::Unreadable)
        );
    }

    #[test]
    fn an_update_on_changes_the_server_lacks_asks_for_a_resync() {
        let doc = doc_with(&from_markdown("text"));
        let other = peer(&doc, 92);
        let base = other.transact().state_vector();
        {
            let fragment = other.get_or_insert_xml_fragment(FRAGMENT);
            let mut txn = other.transact_mut();
            fragment.insert(&mut txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        }
        let middle = other.transact().state_vector();
        {
            let fragment = other.get_or_insert_xml_fragment(FRAGMENT);
            let mut txn = other.transact_mut();
            fragment.insert(&mut txn, 0, XmlElementPrelim::empty(PARAGRAPH));
        }
        // Only the second change: it builds on the first.
        let second = other.transact().encode_diff_v1(&middle);
        assert_eq!(apply_client_update(&doc, &second), Err(Refused::Resync));
        let both = other.transact().encode_diff_v1(&base);
        assert_eq!(
            apply_client_update(&load(&encode(&doc)).expect("load"), &both),
            Ok(())
        );
    }

    #[test]
    fn simple_diff_counts_utf16_and_keeps_characters_whole() {
        assert_eq!(simple_diff("abc", "abc"), (3, 0, String::new()));
        assert_eq!(simple_diff("a😀c", "a😀xc"), (3, 0, "x".to_owned()));
        assert_eq!(simple_diff("a😀c", "a😁c"), (1, 2, "😁".to_owned()));
        assert_eq!(simple_diff("aaa", "aa"), (2, 1, String::new()));
    }

    #[test]
    fn longest_increasing_keeps_the_most_in_order() {
        assert_eq!(
            longest_increasing(&[(0, 2), (1, 0), (2, 1), (3, 3)]),
            vec![(1, 0), (2, 1), (3, 3)]
        );
        assert_eq!(longest_increasing(&[]), vec![]);
    }
}
