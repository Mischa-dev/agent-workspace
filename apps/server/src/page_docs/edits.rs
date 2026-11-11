//! Block edits on a stored document: what `page_edit` does once a page has
//! lasting block ids. Each edit names a block by its id (or, within one
//! version, its position), and its hash proves the caller read the block as
//! it is now. The result is the page's new list of blocks, for
//! [`super::ydoc::replace`] to apply: kept blocks keep their ids and their
//! exact contents, a replaced block's id goes to the first block of its
//! replacement, and everything else in the document is left alone, so a
//! person typing elsewhere on the page loses nothing.

use std::fmt;

use sha2::{Digest, Sha256};

use super::tree::{Block, from_markdown, strip_ids};

/// The most edits one call may make.
pub const MAX_EDITS: usize = 100;

/// What an edit does to the block it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOp {
    /// Put new Markdown in the block's place.
    Replace,
    /// Add new blocks just before it.
    InsertBefore,
    /// Add new blocks just after it.
    InsertAfter,
    /// Take it out.
    Delete,
    /// Add new blocks at the end of the page; names no block.
    Append,
}

/// How an edit names its block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The block's lasting id.
    Id(String),
    /// Where the block is in the version the caller read.
    Position(usize),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(f, "block {id}"),
            Self::Position(position) => write!(f, "block {position}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub op: EditOp,
    pub target: Option<Target>,
    pub hash: Option<String>,
    pub markdown: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    NoEdits,
    TooManyEdits {
        limit: usize,
    },
    /// Edit `index` (from 0) needs a block, or (append) mustn't name one.
    Target {
        index: usize,
        needed: bool,
    },
    /// There's no block with that id, or at that position.
    NoSuchBlock {
        index: usize,
        target: Target,
        blocks: usize,
    },
    /// Replace and delete need the block's hash.
    HashNeeded {
        index: usize,
    },
    /// The block isn't what the caller read.
    HashMismatch {
        index: usize,
        target: Target,
    },
    /// Replace and insert need Markdown with something in it; delete takes none.
    Markdown {
        index: usize,
        needed: bool,
    },
    /// Two edits replace or delete the same block.
    SameBlock {
        index: usize,
        target: Target,
    },
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEdits => write!(f, "pass at least one edit"),
            Self::TooManyEdits { limit } => write!(f, "at most {limit} edits at once"),
            Self::Target {
                index,
                needed: true,
            } => write!(f, "edit {index} needs a block: its id from page_blocks"),
            Self::Target {
                index,
                needed: false,
            } => write!(
                f,
                "edit {index} appends, so it names no block and takes no hash"
            ),
            Self::NoSuchBlock {
                index,
                target: Target::Id(id),
                ..
            } => write!(
                f,
                "edit {index}: there's no block {id} on the page now; read it again"
            ),
            Self::NoSuchBlock {
                index,
                target: Target::Position(position),
                blocks,
            } => write!(
                f,
                "edit {index}: there's no block {position}; the page has {blocks}"
            ),
            Self::HashNeeded { index } => write!(
                f,
                "edit {index} needs the block's hash from page_blocks, so it can't undo a change you haven't seen"
            ),
            Self::HashMismatch { index, target } => write!(
                f,
                "edit {index}: {target} has changed since you read it (its hash differs); read it again"
            ),
            Self::Markdown {
                index,
                needed: true,
            } => write!(f, "edit {index} needs Markdown with something in it"),
            Self::Markdown {
                index,
                needed: false,
            } => write!(f, "edit {index} deletes, so it takes no Markdown"),
            Self::SameBlock { index, target } => write!(
                f,
                "edit {index} changes {target} again; replace or delete each block once"
            ),
        }
    }
}

impl std::error::Error for EditError {}

/// A digest of a block's contents (not its id or how its Markdown is
/// written): the first 16 hex digits of a SHA-256.
pub fn content_hash(block: &Block) -> String {
    let stripped = strip_ids(std::slice::from_ref(block));
    let digest = Sha256::digest(format!("{stripped:?}").as_bytes());
    hex::encode(&digest[..8])
}

/// What an edit leaves at one block.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fate {
    Kept,
    Replaced(Vec<Block>),
    Deleted,
}

/// Applies `edits` to `blocks` (the page's blocks now, with their ids) and
/// returns the blocks the page should hold. All edits name blocks as they
/// are now, never as an earlier edit in the same call left them; they apply
/// together or not at all.
pub fn apply(blocks: &[Block], edits: &[Edit]) -> Result<Vec<Block>, EditError> {
    if edits.is_empty() {
        return Err(EditError::NoEdits);
    }
    if edits.len() > MAX_EDITS {
        return Err(EditError::TooManyEdits { limit: MAX_EDITS });
    }
    let count = blocks.len();
    let mut fates = vec![Fate::Kept; count];
    let mut before: Vec<Vec<Block>> = vec![Vec::new(); count];
    let mut after: Vec<Vec<Block>> = vec![Vec::new(); count];
    let mut appended: Vec<Block> = Vec::new();
    for (index, edit) in edits.iter().enumerate() {
        let wants_markdown = edit.op != EditOp::Delete;
        let parsed = match (&edit.markdown, wants_markdown) {
            (Some(_), false) => {
                return Err(EditError::Markdown {
                    index,
                    needed: false,
                });
            }
            (None, true) => {
                return Err(EditError::Markdown {
                    index,
                    needed: true,
                });
            }
            (Some(markdown), true) => {
                let parsed = from_markdown(markdown);
                if parsed.is_empty() {
                    return Err(EditError::Markdown {
                        index,
                        needed: true,
                    });
                }
                parsed
            }
            (None, false) => Vec::new(),
        };
        if edit.op == EditOp::Append {
            if edit.target.is_some() || edit.hash.is_some() {
                return Err(EditError::Target {
                    index,
                    needed: false,
                });
            }
            appended.extend(parsed);
            continue;
        }
        let Some(target) = edit.target.clone() else {
            return Err(EditError::Target {
                index,
                needed: true,
            });
        };
        let position = match &target {
            Target::Id(id) => blocks
                .iter()
                .position(|block| block.id.as_deref() == Some(id.as_str())),
            Target::Position(position) => (*position < count).then_some(*position),
        };
        let Some(position) = position else {
            return Err(EditError::NoSuchBlock {
                index,
                target,
                blocks: count,
            });
        };
        match &edit.hash {
            Some(hash) if *hash != content_hash(&blocks[position]) => {
                return Err(EditError::HashMismatch { index, target });
            }
            None if matches!(edit.op, EditOp::Replace | EditOp::Delete) => {
                return Err(EditError::HashNeeded { index });
            }
            _ => {}
        }
        match edit.op {
            EditOp::Replace | EditOp::Delete => {
                if fates[position] != Fate::Kept {
                    return Err(EditError::SameBlock { index, target });
                }
                fates[position] = if edit.op == EditOp::Replace {
                    Fate::Replaced(parsed)
                } else {
                    Fate::Deleted
                };
            }
            EditOp::InsertBefore => before[position].extend(parsed),
            EditOp::InsertAfter => after[position].extend(parsed),
            EditOp::Append => unreachable!("handled above"),
        }
    }
    let mut result = Vec::with_capacity(count + appended.len());
    for (position, block) in blocks.iter().enumerate() {
        result.append(&mut before[position]);
        match std::mem::replace(&mut fates[position], Fate::Kept) {
            Fate::Kept => result.push(block.clone()),
            Fate::Replaced(mut replacement) => {
                // The block lives on as the first block of what replaced it.
                replacement[0].id = block.id.clone();
                result.append(&mut replacement);
            }
            Fate::Deleted => {}
        }
        result.append(&mut after[position]);
    }
    result.append(&mut appended);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::tree::{Inline, Marks, Node};

    fn paragraph(id: &str, text: &str) -> Block {
        Block {
            id: Some(id.to_owned()),
            node: Node::Paragraph(vec![Inline::text(text, Marks::default())]),
        }
    }

    fn page() -> Vec<Block> {
        vec![
            paragraph("aaaa1", "one"),
            paragraph("bbbb2", "two"),
            paragraph("cccc3", "three"),
        ]
    }

    fn edit(
        op: EditOp,
        target: Option<Target>,
        hash: Option<String>,
        markdown: Option<&str>,
    ) -> Edit {
        Edit {
            op,
            target,
            hash,
            markdown: markdown.map(str::to_owned),
        }
    }

    fn texts(blocks: &[Block]) -> Vec<(Option<&str>, String)> {
        blocks
            .iter()
            .map(|block| {
                let text = match &block.node {
                    Node::Paragraph(content) | Node::Heading { content, .. } => {
                        crate::page_docs::tree::plain_text(content)
                    }
                    other => other.name().to_owned(),
                };
                (block.id.as_deref(), text)
            })
            .collect()
    }

    #[test]
    fn edits_by_id_keep_every_other_block_and_its_id() {
        let blocks = page();
        let hash = content_hash(&blocks[1]);
        let result = apply(
            &blocks,
            &[
                edit(
                    EditOp::Replace,
                    Some(Target::Id("bbbb2".into())),
                    Some(hash),
                    Some("# Two\n\nand a half"),
                ),
                edit(
                    EditOp::InsertBefore,
                    Some(Target::Id("aaaa1".into())),
                    None,
                    Some("zero"),
                ),
                edit(EditOp::Append, None, None, Some("four")),
            ],
        )
        .expect("edits apply");
        assert_eq!(
            texts(&result),
            vec![
                (None, "zero".to_owned()),
                (Some("aaaa1"), "one".to_owned()),
                (Some("bbbb2"), "Two".to_owned()),
                (None, "and a half".to_owned()),
                (Some("cccc3"), "three".to_owned()),
                (None, "four".to_owned()),
            ]
        );
    }

    #[test]
    fn a_stale_or_missing_hash_is_refused() {
        let blocks = page();
        let stale = content_hash(&paragraph("bbbb2", "two, before someone typed"));
        assert_eq!(
            apply(
                &blocks,
                &[edit(
                    EditOp::Delete,
                    Some(Target::Id("bbbb2".into())),
                    Some(stale),
                    None
                )]
            ),
            Err(EditError::HashMismatch {
                index: 0,
                target: Target::Id("bbbb2".into())
            })
        );
        assert_eq!(
            apply(
                &blocks,
                &[edit(
                    EditOp::Delete,
                    Some(Target::Id("bbbb2".into())),
                    None,
                    None
                )]
            ),
            Err(EditError::HashNeeded { index: 0 })
        );
        // The hash is of the contents, not the id.
        assert_eq!(
            content_hash(&paragraph("bbbb2", "two")),
            content_hash(&paragraph("zzzz9", "two"))
        );
    }

    #[test]
    fn unknown_blocks_doubled_edits_and_empty_markdown_are_refused() {
        let blocks = page();
        let hash = content_hash(&blocks[0]);
        assert!(matches!(
            apply(
                &blocks,
                &[edit(
                    EditOp::InsertAfter,
                    Some(Target::Id("gone1".into())),
                    None,
                    Some("x")
                )]
            ),
            Err(EditError::NoSuchBlock { .. })
        ));
        assert!(matches!(
            apply(
                &blocks,
                &[edit(
                    EditOp::InsertAfter,
                    Some(Target::Position(3)),
                    None,
                    Some("x")
                )]
            ),
            Err(EditError::NoSuchBlock { blocks: 3, .. })
        ));
        assert_eq!(
            apply(
                &blocks,
                &[
                    edit(
                        EditOp::Delete,
                        Some(Target::Position(0)),
                        Some(hash.clone()),
                        None
                    ),
                    edit(
                        EditOp::Replace,
                        Some(Target::Id("aaaa1".into())),
                        Some(hash.clone()),
                        Some("again")
                    ),
                ]
            ),
            Err(EditError::SameBlock {
                index: 1,
                target: Target::Id("aaaa1".into())
            })
        );
        assert_eq!(
            apply(
                &blocks,
                &[edit(
                    EditOp::Replace,
                    Some(Target::Position(0)),
                    Some(hash),
                    Some("  \n\n")
                )]
            ),
            Err(EditError::Markdown {
                index: 0,
                needed: true
            })
        );
        assert_eq!(
            apply(
                &blocks,
                &[edit(
                    EditOp::Append,
                    Some(Target::Position(0)),
                    None,
                    Some("x")
                )]
            ),
            Err(EditError::Target {
                index: 0,
                needed: false
            })
        );
        assert_eq!(apply(&blocks, &[]), Err(EditError::NoEdits));
    }
}
