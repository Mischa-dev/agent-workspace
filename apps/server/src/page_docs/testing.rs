//! Random documents for the page document tests.

use super::markdown::Align;
use super::tree::{Block, Cell, Inline, Item, Marks, Node};

/// A small, seeded random source (xorshift), so failures replay.
pub struct Random(pub u64);

impl Random {
    pub fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    pub fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const TEXT: &[&str] = &[
    "a",
    "b",
    "word",
    "two words",
    " ",
    "  ",
    "\t",
    "*",
    "**",
    "_",
    "__",
    "~",
    "~~",
    "`",
    "``",
    "[",
    "]",
    "(",
    ")",
    "\\",
    "<",
    ">",
    "#",
    "|",
    "-",
    "+",
    "1.",
    "2)",
    ":",
    "//",
    "http",
    "https://x.y",
    "mailto:a@b.c",
    "é",
    "😀",
    "\u{3000}",
    "\u{a0}",
    "!",
    "[ ]",
    "[x]",
    "---",
    "=",
    "AWD-1",
    "\n",
    "'",
    "\"",
    "x_y",
    "*a*",
    "&amp;",
];

const HREFS: &[&str] = &[
    "https://a.b/c",
    "/tasks/1",
    "mailto:a@b.c",
    "https://x.y/(z)",
    "https://x.y/a)b",
    "javascript:alert(1)",
    "https://a.b/c_d",
];

fn random_text(random: &mut Random) -> String {
    (0..1 + random.below(3))
        .map(|_| *random.pick(TEXT))
        .collect()
}

fn random_marks(random: &mut Random) -> Marks {
    Marks {
        bold: random.chance(30),
        italic: random.chance(30),
        strike: random.chance(20),
        code: random.chance(12),
        link: random.chance(20).then(|| (*random.pick(HREFS)).to_owned()),
    }
}

fn random_inline(random: &mut Random, breaks: bool) -> Vec<Inline> {
    let mut content = Vec::new();
    let mut marks = random_marks(random);
    for _ in 0..random.below(7) {
        if breaks && random.chance(10) {
            content.push(Inline::HardBreak);
            continue;
        }
        // Runs next to each other often share some marks, as typed text does.
        if random.chance(60) {
            marks = random_marks(random);
        }
        let mut text = random_text(random);
        if marks.code && (marks.bold || marks.italic || marks.strike) {
            // Emphasis ends at the first matching run, even in code.
            text.retain(|character| !matches!(character, '*' | '_' | '~'));
        }
        if marks.link.is_some() {
            // Inside a link's label, Markdown can't end emphasis right
            // after a backslash, or hold code with unpaired brackets.
            text.retain(|character| !matches!(character, '\\' | '[' | ']'));
        }
        // A link's text is often its address.
        let text = match &marks.link {
            Some(href) if random.chance(25) => href.clone(),
            _ => text,
        };
        content.push(Inline::text(text, marks.clone()));
    }
    content
}

pub fn random_blocks(random: &mut Random, depth: usize, count: usize) -> Vec<Block> {
    (0..count)
        .map(|_| Block::new(random_node(random, depth)))
        .collect()
}

fn random_items(random: &mut Random, depth: usize, checks: bool) -> Vec<Item> {
    (0..1 + random.below(3))
        .map(|_| Item {
            checked: (checks && random.chance(50)).then(|| random.chance(50)),
            content: {
                let mut content = vec![Block::new(Node::Paragraph(random_inline(random, true)))];
                if random.chance(30) {
                    let count = 1 + random.below(2);
                    content.extend(random_blocks(random, depth + 1, count));
                }
                content
            },
        })
        .collect()
}

fn random_node(random: &mut Random, depth: usize) -> Node {
    let kinds = if depth >= 3 { 4 } else { 9 };
    match random.below(kinds) {
        0 | 1 => Node::Paragraph(random_inline(random, true)),
        2 => Node::Heading {
            level: 1 + random.below(6) as u8,
            content: random_inline(random, false),
        },
        3 => Node::CodeBlock {
            language: random
                .chance(40)
                .then(|| (*random.pick(&["js", "Rust", "a b", "`x"])).to_owned()),
            text: (0..random.below(4))
                .map(|_| {
                    *random.pick(&[
                        "let a = 1;",
                        "```",
                        "    indented",
                        "",
                        " ",
                        "~~~",
                        "\ttab",
                        "# not a heading",
                    ])
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        4 => {
            let count = 1 + random.below(3);
            Node::Blockquote(random_blocks(random, depth + 1, count))
        }
        5 => Node::BulletList(random_items(random, depth, true)),
        6 => Node::OrderedList {
            start: *random.pick(&[1, 1, 0, 3, 10, 999_999_999]),
            items: random_items(random, depth, false),
        },
        7 => Node::HorizontalRule,
        _ => {
            let columns = 1 + random.below(3);
            let rows = 1 + random.below(3);
            Node::Table(
                (0..rows)
                    .map(|row| {
                        (0..columns)
                            .map(|_| {
                                let mut cell = Cell::new(
                                    row == 0,
                                    *random.pick(&[
                                        None,
                                        Some(Align::Left),
                                        Some(Align::Center),
                                        Some(Align::Right),
                                    ]),
                                    random_inline(random, false),
                                );
                                if random.chance(5) {
                                    cell.colspan = 2;
                                }
                                cell
                            })
                            .collect()
                    })
                    .collect(),
            )
        }
    }
}
