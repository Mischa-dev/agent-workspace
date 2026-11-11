//! The contract between the server and the web page editor (AWD-53): the
//! documents the server makes from Markdown, which the editor must show
//! without changing them (`apps/web/src/memory/live/pageDocuments.json`,
//! written here), and the edits the editor makes, which the server must take
//! (`apps/web/src/memory/live/pageEdits.json`, written by the editor's
//! tests). After a change on either side, rewrite that side's file with
//! `UPDATE_DOC_PARITY=1`: `cargo test -p agent-workspace-server page_docs::parity`
//! here, `corepack pnpm --filter @agent-workspace/web exec vitest run src/memory/live`
//! on the web.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

use super::{store, tree, ydoc};

const DOCUMENTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../web/src/memory/live/pageDocuments.json"
);
const EDITS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../web/src/memory/live/pageEdits.json"
);

const ABOUT: &str = "Documents the server makes from Markdown (apps/server/src/page_docs/parity.rs writes them, with fixed block ids). The web editor's tests open each one and check it shows without a change; pageEdits.json holds the edits they make, which the server checks it takes. Rewrite with UPDATE_DOC_PARITY=1.";

/// Markdown that between them uses every block, mark and attribute.
const SAMPLES: &[(&str, &str)] = &[
    ("empty", ""),
    (
        "paragraphs",
        "One line\nand the next.\n\nA second paragraph with a hard  \nbreak.\n",
    ),
    (
        "marks",
        "Some **bold**, *italic*, ~~struck~~, `code`, **bold `code`**, [a link](https://example.com), [a page](/projects) and AWD-12.\n",
    ),
    (
        "headings",
        "# One\n\n## Two\n\n### Three\n\n#### Four\n\n##### Five\n\n###### Six\n",
    ),
    (
        "lists",
        "- one\n- two\n  - nested\n\n3. three\n4. four\n\n- [ ] to do\n- [x] done\n",
    ),
    ("code", "```rust\nfn main() {}\n```\n\n```\nplain\n```\n"),
    ("quote and rule", "> quoted\n> > deeper\n\n---\n\nafter\n"),
    (
        "table",
        "| a | b | c |\n|:--|:-:|--:|\n| 1 | **2** | `3` |\n| 4 |  | 6 |\n",
    ),
    (
        "blocks in items",
        "- first\n\n  second paragraph\n\n  ```\n  code in an item\n  ```\n- > a quote in an item\n-\n",
    ),
];

/// A sample's document as the server makes it, with its blocks' ids fixed.
fn sample_state(markdown: &str) -> Vec<u8> {
    let mut blocks = tree::from_markdown(markdown);
    if blocks.is_empty() {
        // What an empty page holds: one empty paragraph.
        blocks.push(tree::Block {
            id: None,
            node: tree::Node::Paragraph(Vec::new()),
        });
    }
    for (index, block) in blocks.iter_mut().enumerate() {
        block.id = Some(format!("block{index}"));
    }
    let doc = ydoc::new_doc();
    ydoc::replace(&doc, &blocks);
    ydoc::normalize(&doc);
    ydoc::encode(&doc)
}

fn sample(name: &str) -> &'static str {
    SAMPLES
        .iter()
        .find(|(sample, _)| *sample == name)
        .unwrap_or_else(|| panic!("no sample document {name:?}"))
        .1
}

fn read_json(path: &str) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|_| panic!("{path} is missing"));
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("{path} isn't JSON"))
}

#[test]
fn documents_for_the_editor_are_current() {
    let documents: Vec<Value> = SAMPLES
        .iter()
        .map(|(name, markdown)| {
            let state = sample_state(markdown);
            // Opening a page changes nothing: its Markdown reads back as it was.
            let blocks = store::read_blocks(&state).expect("reads");
            assert_eq!(
                store::derive_markdown(markdown, &blocks).markdown,
                *markdown,
                "{name}"
            );
            json!({ "name": name, "markdown": markdown, "state": STANDARD.encode(state) })
        })
        .collect();
    let expected = json!({ "about": ABOUT, "documents": documents });
    if std::env::var_os("UPDATE_DOC_PARITY").is_some() {
        let mut text = serde_json::to_string_pretty(&expected).expect("JSON");
        text.push('\n');
        std::fs::write(DOCUMENTS, text).expect("writes pageDocuments.json");
        return;
    }
    assert_eq!(
        read_json(DOCUMENTS),
        expected,
        "pageDocuments.json is out of date: rewrite it with UPDATE_DOC_PARITY=1"
    );
}

#[test]
fn the_editors_edits_are_taken() {
    let edits = read_json(EDITS);
    let cases = edits["edits"].as_array().expect("edits");
    assert!(cases.len() >= 10, "pageEdits.json has too few edits");
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let markdown = sample(case["document"].as_str().expect("document"));
        let update = STANDARD
            .decode(case["update"].as_str().expect("update"))
            .expect("base64");
        let expected = case["markdown"].as_str().expect("markdown");
        // An empty state vector: the reply is the whole document.
        match store::apply_browser_update(&sample_state(markdown), markdown, &update, &[0]) {
            Ok(Ok(applied)) if applied.change.derived.markdown == expected => {}
            Ok(Ok(applied)) => failures.push(format!(
                "{name}: wrote {:?}, expected {expected:?}",
                applied.change.derived.markdown
            )),
            Ok(Err(refused)) => failures.push(format!("{name}: refused, {refused:?}")),
            Err(error) => failures.push(format!("{name}: failed, {error:?}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
