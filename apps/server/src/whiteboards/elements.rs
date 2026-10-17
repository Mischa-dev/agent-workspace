//! Checking Excalidraw elements that come in from browsers and agents, and
//! the rule that decides which copy of an element wins.
//!
//! Elements are stored as the JSON Excalidraw itself writes, so a browser
//! gets back exactly what it sent and new Excalidraw fields need no server
//! change. The server only insists on what it relies on (an id, a known
//! type, version and versionNonce, finite geometry) and refuses what could
//! hurt someone opening the board (script links, oversized payloads).

use serde_json::{Map, Value};

/// Element kinds a board may hold. `selection` is the editor's own rubber
/// band and `iframe` runs arbitrary HTML, so neither is stored.
pub const ELEMENT_TYPES: &[&str] = &[
    "rectangle",
    "diamond",
    "ellipse",
    "arrow",
    "line",
    "freedraw",
    "text",
    "image",
    "frame",
    "magicframe",
    "embeddable",
];

/// Largest single element, as JSON (a long freehand stroke is the big one).
pub const MAX_ELEMENT_BYTES: usize = 256 * 1024;
/// Most element rows a board keeps, deleted ones included.
pub const MAX_ELEMENTS: i64 = 20_000;
/// Most elements one write may carry.
pub const MAX_BATCH: usize = 2_000;
const MAX_TEXT_CHARACTERS: usize = 100_000;
const MAX_POINTS: usize = 50_000;
const MAX_COORDINATE: f64 = 1.0e7;

/// An element as it arrived, with the fields the server stores separately.
#[derive(Clone, Debug)]
pub struct Element {
    pub id: String,
    pub kind: String,
    pub version: i32,
    pub version_nonce: i64,
    pub is_deleted: bool,
    pub index: Option<String>,
    pub data: Map<String, Value>,
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Why an element was refused, in words for whoever sent it.
pub type Refusal = String;

pub fn parse(value: Value) -> Result<Element, Refusal> {
    let Value::Object(data) = value else {
        return Err("Each element must be an object.".to_owned());
    };
    let id = data
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or("Each element needs an id of letters, digits, _ or - (at most 100).")?
        .to_owned();
    let kind = data
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| ELEMENT_TYPES.contains(kind))
        .ok_or_else(|| format!("Element {id} has a type a board can't hold."))?
        .to_owned();
    let version = data
        .get("version")
        .and_then(Value::as_i64)
        .and_then(|version| i32::try_from(version).ok())
        .filter(|version| *version > 0)
        .ok_or_else(|| format!("Element {id} needs a positive whole version."))?;
    let version_nonce = data
        .get("versionNonce")
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("Element {id} needs a whole versionNonce."))?;
    let is_deleted = match data.get("isDeleted") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(deleted)) => *deleted,
        Some(_) => {
            return Err(format!(
                "Element {id} has an isDeleted that isn't true or false."
            ));
        }
    };
    let index = match data.get("index") {
        None | Some(Value::Null) => None,
        Some(Value::String(index)) if index.len() <= 100 => Some(index.clone()),
        Some(_) => {
            return Err(format!(
                "Element {id} has an index that isn't a short string."
            ));
        }
    };
    for field in ["x", "y", "width", "height"] {
        let finite = data
            .get(field)
            .and_then(Value::as_f64)
            .is_some_and(|value| value.is_finite() && value.abs() <= MAX_COORDINATE);
        if !finite {
            return Err(format!(
                "Element {id} needs a number {field} within ±10,000,000."
            ));
        }
    }
    if let Some(angle) = data.get("angle")
        && !angle.is_null()
        && !angle.as_f64().is_some_and(f64::is_finite)
    {
        return Err(format!("Element {id} has an angle that isn't a number."));
    }
    for field in ["text", "originalText"] {
        if let Some(text) = data.get(field)
            && !text.is_null()
            && !text.as_str().is_some_and(|text| {
                text.chars().count() <= MAX_TEXT_CHARACTERS && !text.contains('\0')
            })
        {
            return Err(format!(
                "Element {id} has {field} that's too long or not text."
            ));
        }
    }
    if let Some(points) = data.get("points") {
        let ok = points.as_array().is_some_and(|points| {
            points.len() <= MAX_POINTS
                && points.iter().all(|point| {
                    point.as_array().is_some_and(|pair| {
                        pair.len() >= 2
                            && pair.iter().take(2).all(|value| {
                                value.as_f64().is_some_and(|value| {
                                    value.is_finite() && value.abs() <= MAX_COORDINATE
                                })
                            })
                    })
                })
        });
        if !ok {
            return Err(format!(
                "Element {id} has points that aren't [x, y] number pairs."
            ));
        }
    }
    if let Some(link) = data.get("link")
        && !link.is_null()
    {
        let Some(link) = link.as_str() else {
            return Err(format!("Element {id} has a link that isn't text."));
        };
        if link.len() > 2_000 || !safe_link(link) {
            return Err(format!(
                "Element {id} has a link that isn't a web, mail or in-app link."
            ));
        }
    }
    if let Some(file_id) = data.get("fileId")
        && !file_id.is_null()
        && !file_id.as_str().is_some_and(valid_id)
    {
        return Err(format!("Element {id} has a fileId that isn't a file id."));
    }
    if let Some(groups) = data.get("groupIds")
        && !groups.as_array().is_some_and(|groups| {
            groups.len() <= 100
                && groups
                    .iter()
                    .all(|group| group.as_str().is_some_and(valid_id))
        })
    {
        return Err(format!(
            "Element {id} has groupIds that aren't a list of ids."
        ));
    }
    let size = serde_json::to_vec(&data)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX);
    if size > MAX_ELEMENT_BYTES {
        return Err(format!("Element {id} is larger than 256 KB."));
    }
    Ok(Element {
        id,
        kind,
        version,
        version_nonce,
        is_deleted,
        index,
        data,
    })
}

/// Links people can follow from a board: the web, mail, and paths inside the
/// app. Anything else (javascript:, data:, vbscript:) is refused.
pub fn safe_link(link: &str) -> bool {
    let link = link.trim();
    if link.is_empty() {
        return true;
    }
    if link.starts_with('/') && !link.starts_with("//") {
        return true;
    }
    if link.starts_with('#') {
        return true;
    }
    let lower = link.to_ascii_lowercase();
    ["https://", "http://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        && !link.chars().any(char::is_control)
}

/// Excalidraw's reconcile rule: an incoming copy replaces the stored one when
/// it's a later version, and on a tie the lower versionNonce wins, so every
/// client and the server settle on the same copy without coordinating.
pub fn incoming_wins(incoming: (i32, i64), stored: (i32, i64)) -> bool {
    incoming.0 > stored.0 || (incoming.0 == stored.0 && incoming.1 < stored.1)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{incoming_wins, parse, safe_link};

    fn rectangle() -> serde_json::Value {
        json!({
            "id": "box-1", "type": "rectangle", "x": 10, "y": 20, "width": 100, "height": 50,
            "version": 3, "versionNonce": 99, "isDeleted": false, "index": "a0"
        })
    }

    #[test]
    fn accepts_an_excalidraw_element_and_keeps_unknown_fields() {
        let mut value = rectangle();
        value["someFutureField"] = json!({ "kept": true });
        let element = parse(value).expect("valid");
        assert_eq!(element.id, "box-1");
        assert_eq!(element.version, 3);
        assert_eq!(element.index.as_deref(), Some("a0"));
        assert_eq!(element.data["someFutureField"]["kept"], json!(true));
    }

    #[test]
    fn refuses_what_could_hurt_someone_opening_the_board() {
        for (field, value) in [
            ("type", json!("iframe")),
            ("type", json!("selection")),
            ("id", json!("../etc")),
            ("version", json!(0)),
            ("x", json!("10")),
            ("width", json!(1.0e12)),
            ("link", json!("javascript:alert(1)")),
            ("link", json!(" JavaScript:alert(1)")),
            ("link", json!("data:text/html,hi")),
            ("fileId", json!("a/b")),
            ("points", json!([[0, 0], ["x", 1]])),
        ] {
            let mut element = rectangle();
            element[field] = value.clone();
            assert!(
                parse(element).is_err(),
                "{field} = {value} should be refused"
            );
        }
        let mut huge = rectangle();
        huge["text"] = json!("x".repeat(300_000));
        assert!(parse(huge).is_err());
    }

    #[test]
    fn links_people_can_follow() {
        assert!(safe_link("https://example.com/a"));
        assert!(safe_link("mailto:a@example.com"));
        assert!(safe_link("/projects/1/work-items/2"));
        assert!(!safe_link("//evil.example"));
        assert!(!safe_link("vbscript:x"));
    }

    #[test]
    fn later_versions_win_and_ties_go_to_the_lower_nonce() {
        assert!(incoming_wins((4, 50), (3, 10)));
        assert!(!incoming_wins((3, 50), (4, 10)));
        assert!(incoming_wins((3, 10), (3, 50)));
        assert!(!incoming_wins((3, 50), (3, 10)));
        // The same copy coming back changes nothing.
        assert!(!incoming_wins((3, 10), (3, 10)));
    }
}
