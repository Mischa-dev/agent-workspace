//! A board read as notes (C1.32): every written thing on it (a shape with a
//! label, such as a sticky note, or a text on its own) with the frame it
//! sits in. `whiteboard_to_tasks` makes a task of each, labelled by frame,
//! and `page_make` from a board writes a heading per frame with its notes as
//! bullets. Arrow labels and frame names aren't notes.

use std::collections::HashMap;

use super::scene::{Object, bounds, id, is_deleted, kind, text_field};

/// One written thing on a board.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// The element the note is: the shape for a labelled shape.
    pub id: String,
    pub text: String,
    /// The name of the frame it sits in, if any.
    pub frame: Option<String>,
    x: f64,
    y: f64,
}

/// A frame and the notes in it, in reading order.
#[derive(Debug, PartialEq)]
pub struct Section {
    /// None for notes outside every frame.
    pub frame: Option<String>,
    pub notes: Vec<Note>,
}

fn written(element: &Object) -> Option<String> {
    text_field(element, "originalText")
        .or_else(|| text_field(element, "text"))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The board's notes, frame by frame (frames left to right, then top to
/// bottom; notes outside every frame first), each top to bottom, then left to
/// right.
pub fn sections(elements: &[Object]) -> Vec<Section> {
    let live: Vec<&Object> = elements.iter().filter(|e| !is_deleted(e)).collect();
    let by_id: HashMap<&str, &Object> = live.iter().map(|e| (id(e), *e)).collect();
    let frame_name = |element: &Object| {
        text_field(element, "frameId")
            .and_then(|frame| by_id.get(frame))
            .filter(|frame| matches!(kind(frame), "frame" | "magicframe"))
            .map(|frame| frame_name_of(frame))
    };
    let mut notes = Vec::new();
    for element in &live {
        if kind(element) != "text" {
            continue;
        }
        let Some(text) = written(element) else {
            continue;
        };
        let (holder, frame) = match text_field(element, "containerId").and_then(|c| by_id.get(c)) {
            Some(container) if matches!(kind(container), "rectangle" | "ellipse" | "diamond") => {
                (*container, frame_name(container))
            }
            // A label on an arrow or a line says how things connect.
            Some(_) => continue,
            None => (*element, frame_name(element)),
        };
        let at = bounds(holder);
        notes.push(Note {
            id: id(holder).to_owned(),
            text,
            frame,
            x: at.x,
            y: at.y,
        });
    }
    let mut frames: Vec<(&str, String, f64, f64)> = live
        .iter()
        .filter(|e| matches!(kind(e), "frame" | "magicframe"))
        .map(|frame| {
            let at = bounds(frame);
            (id(frame), frame_name_of(frame), at.x, at.y)
        })
        .collect();
    frames.sort_by(|a, b| a.3.total_cmp(&b.3).then(a.2.total_cmp(&b.2)));
    let reading = |a: &Note, b: &Note| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x));
    let mut loose: Vec<Note> = notes
        .iter()
        .filter(|n| n.frame.is_none())
        .cloned()
        .collect();
    loose.sort_by(reading);
    let mut out = Vec::new();
    if !loose.is_empty() {
        out.push(Section {
            frame: None,
            notes: loose,
        });
    }
    let mut seen: Vec<String> = Vec::new();
    for (_, name, ..) in frames {
        // Frames with the same name read as one.
        if seen.contains(&name) {
            continue;
        }
        let mut inside: Vec<Note> = notes
            .iter()
            .filter(|n| n.frame.as_deref() == Some(name.as_str()))
            .cloned()
            .collect();
        seen.push(name.clone());
        if inside.is_empty() {
            continue;
        }
        inside.sort_by(reading);
        out.push(Section {
            frame: Some(name),
            notes: inside,
        });
    }
    out
}

fn frame_name_of(frame: &Object) -> String {
    text_field(frame, "name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Frame")
        .to_owned()
}

/// The board as Markdown: a heading per frame, its notes as bullets.
pub fn markdown(sections: &[Section]) -> String {
    let mut body = String::new();
    for section in sections {
        if let Some(frame) = &section.frame {
            body.push_str("## ");
            body.push_str(&one_line(frame));
            body.push_str("\n\n");
        }
        for note in &section.notes {
            body.push_str("- ");
            body.push_str(&note.text.replace('\n', "\n  "));
            body.push('\n');
        }
        body.push('\n');
    }
    body.trim_end().to_owned() + "\n"
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: serde_json::Value) -> Object {
        value.as_object().expect("object").clone()
    }

    fn board() -> Vec<Object> {
        vec![
            object(
                json!({"id": "f1", "type": "frame", "name": "Now", "x": 0, "y": 0, "width": 500, "height": 500}),
            ),
            object(
                json!({"id": "f2", "type": "frame", "name": "Later", "x": 600, "y": 0, "width": 500, "height": 500}),
            ),
            object(json!({"id": "s1", "type": "rectangle", "x": 10, "y": 100, "frameId": "f1"})),
            object(
                json!({"id": "s1-label", "type": "text", "text": "Ship\nit", "originalText": "Ship\nit", "containerId": "s1", "frameId": "f1", "x": 12, "y": 102}),
            ),
            object(json!({"id": "s2", "type": "rectangle", "x": 10, "y": 10, "frameId": "f1"})),
            object(
                json!({"id": "s2-label", "type": "text", "text": "Plan", "containerId": "s2", "x": 12, "y": 12}),
            ),
            object(
                json!({"id": "t1", "type": "text", "text": "Write docs", "frameId": "f2", "x": 610, "y": 10}),
            ),
            object(json!({"id": "a1", "type": "arrow", "x": 0, "y": 0})),
            object(json!({"id": "a1-label", "type": "text", "text": "then", "containerId": "a1"})),
            object(
                json!({"id": "loose", "type": "text", "text": "Loose idea", "x": 2000, "y": 2000}),
            ),
            object(json!({"id": "gone", "type": "text", "text": "Deleted", "isDeleted": true})),
        ]
    }

    #[test]
    fn notes_come_frame_by_frame_in_reading_order() {
        let sections = sections(&board());
        let flat: Vec<(Option<&str>, &str)> = sections
            .iter()
            .flat_map(|s| s.notes.iter().map(|n| (n.frame.as_deref(), n.id.as_str())))
            .collect();
        assert_eq!(
            flat,
            vec![
                (None, "loose"),
                (Some("Now"), "s2"),
                (Some("Now"), "s1"),
                (Some("Later"), "t1"),
            ]
        );
    }

    #[test]
    fn a_board_reads_as_headings_and_bullets() {
        assert_eq!(
            markdown(&sections(&board())),
            "- Loose idea\n\n## Now\n\n- Plan\n- Ship\n  it\n\n## Later\n\n- Write docs\n"
        );
    }
}
