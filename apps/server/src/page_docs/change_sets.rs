//! A page's change sets (C1.9): what a write touched, by lasting block id,
//! and what it replaced, so it can be undone for the blocks nobody has
//! changed since.
//!
//! The inverse keeps one entry per touched block: its Markdown before the
//! change (none when the change added it), the hash it had after (none when
//! the change took it out) and the block before it, to put a removed block
//! back in its place. Typing in the editor joins the writer's open change
//! set: a block touched again keeps its first "before" and takes the newest
//! "after", so one undo takes back the whole minute.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::{
    edits::{self, Edit, EditOp, Target},
    tree::Block,
    write::write_block,
};

/// The most blocks one change set keeps what they were for; beyond it the
/// inverse is dropped and undo offers a restore.
const MAX_INVERSE_BLOCKS: usize = 2_000;

/// One touched block.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    /// Its Markdown before the change; `None` when the change added it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Its hash after the change; `None` when the change took it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_hash: Option<String>,
    /// The block before it, when it was there before the change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

/// What a page change replaced.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Inverse {
    pub blocks: Vec<Entry>,
}

impl Inverse {
    /// The touched blocks, for `touched`.
    pub fn touched(&self) -> serde_json::Value {
        serde_json::json!({
            "blocks": self.blocks.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>()
        })
    }

    /// Whether it may be kept whole.
    pub fn keepable(&self) -> bool {
        self.blocks.len() <= MAX_INVERSE_BLOCKS
    }
}

fn markdown(block: &Block) -> String {
    write_block(block, None)
        .map(|(text, _)| text)
        .unwrap_or_default()
}

/// What changed between `before` and `after` (blocks with ids). Blocks
/// that only moved are left alone.
pub fn diff(before: &[Block], after: &[Block]) -> Inverse {
    let after_by_id: HashMap<&str, &Block> = after
        .iter()
        .filter_map(|block| Some((block.id.as_deref()?, block)))
        .collect();
    let before_ids: HashSet<&str> = before.iter().filter_map(|b| b.id.as_deref()).collect();
    let mut blocks = Vec::new();
    let mut previous: Option<&str> = None;
    for block in before {
        let Some(id) = block.id.as_deref() else {
            continue;
        };
        match after_by_id.get(id) {
            Some(now) => {
                let hash = edits::content_hash(now);
                if hash != edits::content_hash(block) {
                    blocks.push(Entry {
                        id: id.to_owned(),
                        before: Some(markdown(block)),
                        after_hash: Some(hash),
                        previous: previous.map(str::to_owned),
                    });
                }
            }
            None => blocks.push(Entry {
                id: id.to_owned(),
                before: Some(markdown(block)),
                after_hash: None,
                previous: previous.map(str::to_owned),
            }),
        }
        previous = Some(id);
    }
    for block in after {
        let Some(id) = block.id.as_deref() else {
            continue;
        };
        if !before_ids.contains(id) {
            blocks.push(Entry {
                id: id.to_owned(),
                before: None,
                after_hash: Some(edits::content_hash(block)),
                previous: None,
            });
        }
    }
    Inverse { blocks }
}

/// `earlier` followed by `later`, as one change: a block both touched
/// keeps its first "before" and its last "after"; one added and then taken
/// out was never there.
pub fn join(earlier: Inverse, later: Inverse) -> Inverse {
    let mut blocks = earlier.blocks;
    for entry in later.blocks {
        match blocks.iter().position(|kept| kept.id == entry.id) {
            Some(index) => {
                blocks[index].after_hash = entry.after_hash;
                if blocks[index].before.is_none() && blocks[index].after_hash.is_none() {
                    blocks.remove(index);
                }
            }
            None => blocks.push(entry),
        }
    }
    Inverse { blocks }
}

/// The result of undoing a change on the page as it is now.
pub struct Undone {
    pub blocks: Vec<Block>,
    /// Touched blocks put back.
    pub applied: usize,
    /// Touched blocks changed since, left as they are.
    pub skipped: Vec<String>,
}

/// Puts back what `inverse` replaced, block by block, skipping any block
/// someone has changed since (its hash isn't the one the change left).
pub fn undo(current: &[Block], inverse: &Inverse) -> Undone {
    let mut blocks = current.to_vec();
    let mut applied = 0;
    let mut skipped = Vec::new();
    // Changed and added blocks first, then removed ones go back, in page
    // order, so each finds the block it followed.
    let (removed, rest): (Vec<&Entry>, Vec<&Entry>) = inverse
        .blocks
        .iter()
        .partition(|entry| entry.after_hash.is_none());
    for entry in rest.into_iter().chain(removed) {
        let edit = match (&entry.before, &entry.after_hash) {
            (None, Some(hash)) => Edit {
                op: EditOp::Delete,
                target: Some(Target::Id(entry.id.clone())),
                hash: Some(hash.clone()),
                markdown: None,
            },
            (Some(before), Some(hash)) => Edit {
                op: EditOp::Replace,
                target: Some(Target::Id(entry.id.clone())),
                hash: Some(hash.clone()),
                markdown: Some(before.clone()),
            },
            (Some(before), None) => {
                let anchor = entry
                    .previous
                    .as_ref()
                    .filter(|id| blocks.iter().any(|b| b.id.as_deref() == Some(id.as_str())));
                match (anchor, blocks.first().and_then(|b| b.id.clone())) {
                    (Some(previous), _) => Edit {
                        op: EditOp::InsertAfter,
                        target: Some(Target::Id(previous.clone())),
                        hash: None,
                        markdown: Some(before.clone()),
                    },
                    (None, Some(first)) if entry.previous.is_none() => Edit {
                        op: EditOp::InsertBefore,
                        target: Some(Target::Id(first)),
                        hash: None,
                        markdown: Some(before.clone()),
                    },
                    _ => Edit {
                        op: EditOp::Append,
                        target: None,
                        hash: None,
                        markdown: Some(before.clone()),
                    },
                }
            }
            (None, None) => continue,
        };
        // A block put back must not already be there.
        if entry.after_hash.is_none()
            && blocks
                .iter()
                .any(|block| block.id.as_deref() == Some(entry.id.as_str()))
        {
            skipped.push(entry.id.clone());
            continue;
        }
        match edits::apply(&blocks, std::slice::from_ref(&edit)) {
            Ok(next) => {
                blocks = next;
                applied += 1;
            }
            Err(_) => skipped.push(entry.id.clone()),
        }
    }
    Undone {
        blocks,
        applied,
        skipped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_docs::tree::{from_markdown, strip_ids};

    fn with_ids(mut blocks: Vec<Block>, prefix: &str) -> Vec<Block> {
        for (index, block) in blocks.iter_mut().enumerate() {
            block.id = Some(format!("{prefix}{index}"));
        }
        blocks
    }

    #[test]
    fn diff_names_changed_added_and_removed_blocks() {
        let before = with_ids(from_markdown("One\n\nTwo\n\nThree\n"), "b");
        let mut after = before.clone();
        after[1] = with_ids(from_markdown("Two, changed\n"), "x").remove(0);
        after[1].id = Some("b1".into());
        after.remove(2);
        let mut added = with_ids(from_markdown("Four\n"), "n");
        after.append(&mut added);
        let inverse = diff(&before, &after);
        let ids: Vec<_> = inverse.blocks.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["b1", "b2", "n0"]);
        assert_eq!(inverse.blocks[0].before.as_deref(), Some("Two"));
        assert!(inverse.blocks[1].after_hash.is_none());
        assert_eq!(inverse.blocks[1].previous.as_deref(), Some("b1"));
        assert!(inverse.blocks[2].before.is_none());

        // Undo puts the page back.
        let undone = undo(&after, &inverse);
        assert!(undone.skipped.is_empty());
        assert_eq!(strip_ids(&undone.blocks), strip_ids(&before));
    }

    #[test]
    fn undo_leaves_blocks_changed_since() {
        let before = with_ids(from_markdown("One\n\nTwo\n"), "b");
        let mut after = before.clone();
        after[0] = with_ids(from_markdown("Uno\n"), "x").remove(0);
        after[0].id = Some("b0".into());
        after[1] = with_ids(from_markdown("Dos\n"), "x").remove(0);
        after[1].id = Some("b1".into());
        let inverse = diff(&before, &after);
        // Someone edits the second block again.
        let mut now = after.clone();
        now[1] = with_ids(from_markdown("Dos!\n"), "x").remove(0);
        now[1].id = Some("b1".into());
        let undone = undo(&now, &inverse);
        assert_eq!(undone.applied, 1);
        assert_eq!(undone.skipped, vec!["b1".to_owned()]);
        assert_eq!(
            strip_ids(&undone.blocks),
            strip_ids(&from_markdown("One\n\nDos!\n"))
        );
    }

    #[test]
    fn joined_typing_undoes_in_one_step() {
        let first = with_ids(from_markdown("One\n"), "b");
        let mut second = first.clone();
        second.push(with_ids(from_markdown("Tw\n"), "n").remove(0));
        let mut third = second.clone();
        third[1] = with_ids(from_markdown("Two\n"), "x").remove(0);
        third[1].id = Some("n0".into());
        let joined = join(diff(&first, &second), diff(&second, &third));
        assert_eq!(joined.blocks.len(), 1);
        let undone = undo(&third, &joined);
        assert_eq!(strip_ids(&undone.blocks), strip_ids(&first));
        // Added and then taken out: nothing to undo.
        let gone = join(diff(&first, &second), diff(&second, &first));
        assert!(gone.blocks.is_empty());
    }
}
