//! A board's scene in memory, for agents' operations, reads and rendering.
//!
//! Agents describe what they want ("add a box labelled API", "connect A to
//! B", "lay these out top to bottom") and this module turns that into
//! Excalidraw elements: labels become bound text, connections become arrows
//! bound at both ends, and moving a shape re-routes its arrows. Every element
//! an operation touches gets one new version and versionNonce, so it merges
//! with browsers' edits by the same rule as theirs.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rand_core::{OsRng, RngCore};
use rmcp::schemars;
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::fractional;

pub const DEFAULT_STROKE: &str = "#1e1e1e";
pub const DEFAULT_FONT_SIZE: f64 = 20.0;
/// Excalidraw's default font in 0.18 (Excalifont).
pub const DEFAULT_FONT_FAMILY: i64 = 5;
pub const LINE_HEIGHT: f64 = 1.25;
/// Space between a container's edge and its label, as Excalidraw keeps it.
const LABEL_PADDING: f64 = 5.0;
/// Space between a shape and the end of an arrow bound to it.
const ARROW_GAP: f64 = 6.0;
/// Roughly how wide a character of Excalidraw's hand-drawn font is, as a
/// share of the font size. Browsers re-measure text when they edit it.
const CHARACTER_WIDTH: f64 = 0.58;

pub type Object = Map<String, Value>;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    pub fn right(&self) -> f64 {
        self.x + self.width
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
    pub fn union(boxes: impl IntoIterator<Item = Bounds>) -> Option<Bounds> {
        let mut iterator = boxes.into_iter();
        let first = iterator.next()?;
        let (mut left, mut top, mut right, mut bottom) =
            (first.x, first.y, first.right(), first.bottom());
        for b in iterator {
            left = left.min(b.x);
            top = top.min(b.y);
            right = right.max(b.right());
            bottom = bottom.max(b.bottom());
        }
        Some(Bounds {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        })
    }
}

pub fn number(element: &Object, field: &str) -> f64 {
    element
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
}

pub fn text_field<'a>(element: &'a Object, field: &str) -> Option<&'a str> {
    element.get(field).and_then(Value::as_str)
}

pub fn kind(element: &Object) -> &str {
    text_field(element, "type").unwrap_or_default()
}

pub fn id(element: &Object) -> &str {
    text_field(element, "id").unwrap_or_default()
}

pub fn is_deleted(element: &Object) -> bool {
    element
        .get("isDeleted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_linear(kind: &str) -> bool {
    matches!(kind, "arrow" | "line" | "freedraw")
}

pub fn points(element: &Object) -> Vec<(f64, f64)> {
    element
        .get("points")
        .and_then(Value::as_array)
        .map(|points| {
            points
                .iter()
                .filter_map(|point| {
                    let pair = point.as_array()?;
                    Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where an element sits on the canvas, unrotated.
pub fn bounds(element: &Object) -> Bounds {
    let (x, y) = (number(element, "x"), number(element, "y"));
    if is_linear(kind(element)) {
        let points = points(element);
        if !points.is_empty() {
            let left = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
            let right = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
            let top = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let bottom = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            return Bounds {
                x: x + left,
                y: y + top,
                width: right - left,
                height: bottom - top,
            };
        }
    }
    Bounds {
        x,
        y,
        width: number(element, "width").abs(),
        height: number(element, "height").abs(),
    }
}

pub fn random_nonce() -> i64 {
    i64::from(OsRng.next_u32() >> 1)
}

fn random_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_-";
    let mut bytes = [0u8; 21];
    OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect()
}

/// How big text is drawn: the widest line and the lines' total height.
pub fn measure_text(text: &str, font_size: f64) -> (f64, f64) {
    let lines = text.split('\n').collect::<Vec<_>>();
    let widest = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as f64;
    (
        (widest * font_size * CHARACTER_WIDTH).max(font_size * 0.5),
        lines.len() as f64 * font_size * LINE_HEIGHT,
    )
}

/// Breaks text into lines no wider than `width`, at spaces where it can.
pub fn wrap_text(text: &str, font_size: f64, width: f64) -> String {
    let per_line = ((width / (font_size * CHARACTER_WIDTH)).floor() as usize).max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word.to_owned();
            while word.chars().count() > per_line {
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                let head = word.chars().take(per_line).collect::<String>();
                word = word.chars().skip(per_line).collect();
                lines.push(head);
            }
            let candidate = if line.is_empty() {
                word.clone()
            } else {
                format!("{line} {word}")
            };
            if candidate.chars().count() > per_line && !line.is_empty() {
                lines.push(std::mem::replace(&mut line, word));
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// One thing an agent asked for. Flat on purpose: every operation takes the
/// fields it needs and ignores none silently (see `apply`).
#[derive(Clone, Debug, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Operation {
    /// add, update, delete, connect, move, align, distribute, layout, group,
    /// ungroup, or order.
    pub op: String,
    /// The element to change, or for add and connect the id to give the new
    /// one (letters, digits, _ or -). Pick readable ids such as "api" so
    /// later operations can name them.
    pub id: Option<String>,
    /// The elements move, delete, align, distribute, layout, group, ungroup
    /// and order act on. layout without ids lays out every shape.
    pub ids: Option<Vec<String>>,
    /// For add: rectangle, ellipse, diamond, text, arrow, line, or frame.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    /// For move: how far to shift.
    pub dx: Option<f64>,
    pub dy: Option<f64>,
    /// A text element's text, or the label inside a shape or on an arrow.
    /// An empty string removes a label.
    pub text: Option<String>,
    /// For connect (or an added arrow): the element the arrow starts at.
    pub from: Option<String>,
    /// For connect (or an added arrow): the element the arrow ends at.
    pub to: Option<String>,
    /// For a line or an unbound arrow: points relative to x and y.
    pub points: Option<Vec<[f64; 2]>>,
    /// Put the element in this frame; an empty string takes it out.
    pub frame_id: Option<String>,
    /// A frame's name.
    pub name: Option<String>,
    /// A colour such as #1e1e1e or transparent.
    pub stroke_color: Option<String>,
    pub background_color: Option<String>,
    /// solid, hachure, cross-hatch, or zigzag.
    pub fill_style: Option<String>,
    /// solid, dashed, or dotted.
    pub stroke_style: Option<String>,
    pub stroke_width: Option<f64>,
    /// 0 to 100.
    pub opacity: Option<f64>,
    /// 0 (clean) to 2 (sketchy).
    pub roughness: Option<f64>,
    pub font_size: Option<f64>,
    /// left, center, or right.
    pub text_align: Option<String>,
    /// Rounded corners for rectangles and diamonds.
    pub rounded: Option<bool>,
    /// For arrows: end (default), start, both, or none.
    pub arrowheads: Option<String>,
    pub locked: Option<bool>,
    /// A web link, or a path in this app such as /projects/…/work-items/….
    pub link: Option<String>,
    /// For align: left, center, right, top, middle, or bottom.
    pub edge: Option<String>,
    /// For distribute: horizontal or vertical.
    pub axis: Option<String>,
    /// For distribute and layout: the space between elements.
    pub gap: Option<f64>,
    /// For layout: down (default), right, or grid.
    pub direction: Option<String>,
    /// For order: front or back.
    pub position: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OperationResult {
    pub op: String,
    /// The elements it created or changed.
    pub ids: Vec<String>,
}

pub struct Scene {
    elements: Vec<Object>,
    by_id: HashMap<String, usize>,
    /// Versions as loaded, so each touched element is bumped exactly once.
    loaded_versions: HashMap<String, i64>,
    changed: BTreeSet<String>,
    /// Elements whose position or size changed, for re-routing arrows.
    moved: BTreeSet<String>,
    now_ms: i64,
}

type OpError = String;

impl Scene {
    /// A scene from stored elements, in stacking order.
    pub fn new(elements: Vec<Object>, now_ms: i64) -> Self {
        let mut scene = Self {
            elements: Vec::new(),
            by_id: HashMap::new(),
            loaded_versions: HashMap::new(),
            changed: BTreeSet::new(),
            moved: BTreeSet::new(),
            now_ms,
        };
        for element in elements {
            let id = id(&element).to_owned();
            scene.loaded_versions.insert(
                id.clone(),
                element.get("version").and_then(Value::as_i64).unwrap_or(0),
            );
            scene.by_id.insert(id, scene.elements.len());
            scene.elements.push(element);
        }
        scene
    }

    pub fn elements(&self) -> &[Object] {
        &self.elements
    }

    pub fn live(&self) -> impl Iterator<Item = &Object> {
        self.elements.iter().filter(|element| !is_deleted(element))
    }

    fn get(&self, id: &str) -> Option<&Object> {
        self.by_id.get(id).map(|index| &self.elements[*index])
    }

    fn get_live(&self, id: &str) -> Result<&Object, OpError> {
        self.get(id)
            .filter(|element| !is_deleted(element))
            .ok_or_else(|| format!("There's no element {id} on the board."))
    }

    fn touch(&mut self, id: &str) -> Option<&mut Object> {
        let index = *self.by_id.get(id)?;
        self.changed.insert(id.to_owned());
        Some(&mut self.elements[index])
    }

    fn ids_of(&self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = operation
            .ids
            .clone()
            .or_else(|| operation.id.clone().map(|id| vec![id]))
            .unwrap_or_default();
        if ids.is_empty() {
            return Err(format!("{} needs ids.", operation.op));
        }
        for id in &ids {
            self.get_live(id)?;
        }
        Ok(ids)
    }

    fn last_index(&self) -> Option<String> {
        self.elements
            .iter()
            .filter_map(|element| text_field(element, "index"))
            .filter(|index| fractional::is_valid(index))
            .max()
            .map(str::to_owned)
    }

    fn first_index(&self) -> Option<String> {
        self.elements
            .iter()
            .filter_map(|element| text_field(element, "index"))
            .filter(|index| fractional::is_valid(index))
            .min()
            .map(str::to_owned)
    }

    fn new_id(&self, wanted: Option<&str>) -> Result<String, OpError> {
        match wanted {
            Some(wanted) => {
                if !super::elements::valid_id(wanted) {
                    return Err(format!(
                        "{wanted} isn't a usable id: use letters, digits, _ or - (at most 100)."
                    ));
                }
                if self.get(wanted).is_some_and(|element| !is_deleted(element)) {
                    return Err(format!("There's already an element {wanted} on the board."));
                }
                Ok(wanted.to_owned())
            }
            None => loop {
                let candidate = random_id();
                if self.get(&candidate).is_none() {
                    break Ok(candidate);
                }
            },
        }
    }

    /// Adds a new element on top of everything (or replaces a deleted one
    /// with the same id, as a newer version of it).
    fn insert(&mut self, mut element: Object) {
        let id = id(&element).to_owned();
        let index = fractional::key_between(self.last_index().as_deref(), None)
            .unwrap_or_else(|| "a0".to_owned());
        element.insert("index".to_owned(), json!(index));
        self.changed.insert(id.clone());
        self.moved.insert(id.clone());
        match self.by_id.get(&id) {
            Some(position) => self.elements[*position] = element,
            None => {
                self.by_id.insert(id, self.elements.len());
                self.elements.push(element);
            }
        }
    }

    fn base(&self, kind: &str, id: &str, b: Bounds) -> Object {
        let mut element = Map::new();
        element.insert("id".to_owned(), json!(id));
        element.insert("type".to_owned(), json!(kind));
        element.insert("x".to_owned(), json!(b.x));
        element.insert("y".to_owned(), json!(b.y));
        element.insert("width".to_owned(), json!(b.width));
        element.insert("height".to_owned(), json!(b.height));
        element.insert("angle".to_owned(), json!(0));
        element.insert("strokeColor".to_owned(), json!(DEFAULT_STROKE));
        element.insert("backgroundColor".to_owned(), json!("transparent"));
        element.insert("fillStyle".to_owned(), json!("solid"));
        element.insert("strokeWidth".to_owned(), json!(2));
        element.insert("strokeStyle".to_owned(), json!("solid"));
        element.insert("roughness".to_owned(), json!(1));
        element.insert("opacity".to_owned(), json!(100));
        element.insert("groupIds".to_owned(), json!([]));
        element.insert("frameId".to_owned(), Value::Null);
        element.insert(
            "roundness".to_owned(),
            match kind {
                "rectangle" => json!({ "type": 3 }),
                "diamond" | "arrow" | "line" => json!({ "type": 2 }),
                _ => Value::Null,
            },
        );
        element.insert("seed".to_owned(), json!(random_nonce()));
        element.insert("version".to_owned(), json!(1));
        element.insert("versionNonce".to_owned(), json!(random_nonce()));
        element.insert("isDeleted".to_owned(), json!(false));
        element.insert("boundElements".to_owned(), json!([]));
        element.insert("updated".to_owned(), json!(self.now_ms));
        element.insert("link".to_owned(), Value::Null);
        element.insert("locked".to_owned(), json!(false));
        element
    }

    fn text_element(&self, id: &str, text: &str, x: f64, y: f64, font_size: f64) -> Object {
        let (width, height) = measure_text(text, font_size);
        let mut element = self.base(
            "text",
            id,
            Bounds {
                x,
                y,
                width,
                height,
            },
        );
        element.insert("text".to_owned(), json!(text));
        element.insert("originalText".to_owned(), json!(text));
        element.insert("fontSize".to_owned(), json!(font_size));
        element.insert("fontFamily".to_owned(), json!(DEFAULT_FONT_FAMILY));
        element.insert("textAlign".to_owned(), json!("left"));
        element.insert("verticalAlign".to_owned(), json!("top"));
        element.insert("containerId".to_owned(), Value::Null);
        element.insert("autoResize".to_owned(), json!(true));
        element.insert("lineHeight".to_owned(), json!(LINE_HEIGHT));
        element
    }

    /// The bound text inside a shape or on an arrow, if it has one.
    fn label_of(&self, container: &str) -> Option<String> {
        self.live()
            .find(|element| {
                kind(element) == "text" && text_field(element, "containerId") == Some(container)
            })
            .map(|element| id(element).to_owned())
    }

    fn add_bound(&mut self, container: &str, bound: &str, bound_kind: &str) {
        if let Some(element) = self.touch(container) {
            let mut list = element
                .get("boundElements")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !list
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_str) == Some(bound))
            {
                list.push(json!({ "id": bound, "type": bound_kind }));
            }
            element.insert("boundElements".to_owned(), Value::Array(list));
        }
    }

    fn remove_bound(&mut self, container: &str, bound: &str) {
        let has = self.get(container).is_some_and(|element| {
            element
                .get("boundElements")
                .and_then(Value::as_array)
                .is_some_and(|list| {
                    list.iter()
                        .any(|entry| entry.get("id").and_then(Value::as_str) == Some(bound))
                })
        });
        if has && let Some(element) = self.touch(container) {
            let list = element
                .get("boundElements")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry.get("id").and_then(Value::as_str) != Some(bound))
                .collect::<Vec<_>>();
            element.insert("boundElements".to_owned(), Value::Array(list));
        }
    }

    /// Sets, replaces or (with empty text) removes a container's label.
    fn set_label(&mut self, container: &str, text: &str, font_size: Option<f64>) {
        let existing = self.label_of(container);
        if text.is_empty() {
            if let Some(label) = existing {
                if let Some(element) = self.touch(&label) {
                    element.insert("isDeleted".to_owned(), json!(true));
                }
                self.remove_bound(container, &label);
            }
            return;
        }
        let label = match existing {
            Some(label) => label,
            None => {
                let wanted = format!("{container}-label");
                let label = self
                    .new_id(Some(&wanted))
                    .or_else(|_| self.new_id(None))
                    .unwrap_or(wanted);
                let mut element = self.text_element(
                    &label,
                    text,
                    0.0,
                    0.0,
                    font_size.unwrap_or(DEFAULT_FONT_SIZE),
                );
                element.insert("containerId".to_owned(), json!(container));
                element.insert("textAlign".to_owned(), json!("center"));
                element.insert("verticalAlign".to_owned(), json!("middle"));
                let container_element = self.get(container).cloned();
                if let Some(container_element) = container_element {
                    for field in ["strokeColor", "groupIds", "frameId"] {
                        if let Some(value) = container_element.get(field) {
                            element.insert(field.to_owned(), value.clone());
                        }
                    }
                }
                self.insert(element);
                self.add_bound(container, &label, "text");
                label
            }
        };
        if let Some(element) = self.touch(&label) {
            element.insert("originalText".to_owned(), json!(text));
            element.insert("text".to_owned(), json!(text));
            if let Some(size) = font_size {
                element.insert("fontSize".to_owned(), json!(size));
            }
        }
        self.moved.insert(container.to_owned());
    }

    /// Puts each moved container's label back in its middle, wrapping it to
    /// fit and growing the container when the text is taller than it.
    fn place_labels(&mut self) {
        let containers = self.moved.iter().cloned().collect::<Vec<_>>();
        for container in containers {
            let Some(label) = self.label_of(&container) else {
                continue;
            };
            let Some(container_element) = self.get(&container).cloned() else {
                continue;
            };
            let Some(label_element) = self.get(&label).cloned() else {
                continue;
            };
            let font_size = Some(number(&label_element, "fontSize"))
                .filter(|size| *size > 0.0)
                .unwrap_or(DEFAULT_FONT_SIZE);
            let original = text_field(&label_element, "originalText")
                .or_else(|| text_field(&label_element, "text"))
                .unwrap_or_default()
                .to_owned();
            let container_kind = kind(&container_element).to_owned();
            if container_kind == "arrow" {
                let b = bounds(&container_element);
                let points = points(&container_element);
                let (cx, cy) = match points.as_slice() {
                    [] => b.center(),
                    points => {
                        let middle = points[points.len() / 2];
                        let previous = points[(points.len() - 1) / 2];
                        let x = number(&container_element, "x");
                        let y = number(&container_element, "y");
                        (
                            x + (middle.0 + previous.0) / 2.0,
                            y + (middle.1 + previous.1) / 2.0,
                        )
                    }
                };
                let (width, height) = measure_text(&original, font_size);
                if let Some(element) = self.touch(&label) {
                    element.insert("text".to_owned(), json!(original));
                    element.insert("width".to_owned(), json!(width));
                    element.insert("height".to_owned(), json!(height));
                    element.insert("x".to_owned(), json!(cx - width / 2.0));
                    element.insert("y".to_owned(), json!(cy - height / 2.0));
                }
                continue;
            }
            let b = bounds(&container_element);
            // Ellipses and diamonds have less room inside than their box.
            let inner = match container_kind.as_str() {
                "ellipse" => b.width / std::f64::consts::SQRT_2,
                "diamond" => b.width / 2.0,
                _ => b.width,
            } - LABEL_PADDING * 2.0;
            let wrapped = wrap_text(&original, font_size, inner.max(font_size));
            let (width, height) = measure_text(&wrapped, font_size);
            let needed = height
                + LABEL_PADDING * 2.0
                + match container_kind.as_str() {
                    "ellipse" | "diamond" => height,
                    _ => 0.0,
                };
            if needed > b.height
                && let Some(element) = self.touch(&container)
            {
                element.insert("height".to_owned(), json!(needed));
            }
            let b = self.get(&container).map(bounds).unwrap_or(b);
            let (cx, cy) = b.center();
            if let Some(element) = self.touch(&label) {
                element.insert("text".to_owned(), json!(wrapped));
                element.insert("width".to_owned(), json!(width.min(inner.max(font_size))));
                element.insert("height".to_owned(), json!(height));
                element.insert(
                    "x".to_owned(),
                    json!(cx - width.min(inner.max(font_size)) / 2.0),
                );
                element.insert("y".to_owned(), json!(cy - height / 2.0));
            }
        }
    }

    /// Re-routes every arrow bound to something that moved, as a straight
    /// line between the two shapes' edges.
    fn route_arrows(&mut self) {
        let arrows = self
            .live()
            .filter(|element| kind(element) == "arrow")
            .filter_map(|element| {
                let start = binding_target(element, "startBinding");
                let end = binding_target(element, "endBinding");
                let touched = [&start, &end]
                    .iter()
                    .any(|target| target.as_ref().is_some_and(|id| self.moved.contains(id)))
                    || self.moved.contains(id(element));
                (touched && (start.is_some() || end.is_some()))
                    .then(|| (id(element).to_owned(), start, end))
            })
            .collect::<Vec<_>>();
        for (arrow, start, end) in arrows {
            let Some(current) = self.get(&arrow).cloned() else {
                continue;
            };
            let existing = points(&current);
            let x = number(&current, "x");
            let y = number(&current, "y");
            let first = existing
                .first()
                .map(|p| (x + p.0, y + p.1))
                .unwrap_or((x, y));
            let last = existing
                .last()
                .map(|p| (x + p.0, y + p.1))
                .unwrap_or((x, y));
            let start_shape = start
                .as_deref()
                .and_then(|id| self.get(id))
                .filter(|e| !is_deleted(e))
                .cloned();
            let end_shape = end
                .as_deref()
                .and_then(|id| self.get(id))
                .filter(|e| !is_deleted(e))
                .cloned();
            let start_center = start_shape
                .as_ref()
                .map(|shape| bounds(shape).center())
                .unwrap_or(first);
            let end_center = end_shape
                .as_ref()
                .map(|shape| bounds(shape).center())
                .unwrap_or(last);
            let from = match &start_shape {
                Some(shape) => edge_point(shape, end_center),
                None => first,
            };
            let to = match &end_shape {
                Some(shape) => edge_point(shape, start_center),
                None => last,
            };
            if let Some(element) = self.touch(&arrow) {
                set_line(element, from, to);
            }
            self.moved.insert(arrow);
        }
    }

    /// Runs operations in order. Any refusal stops the whole batch, so a
    /// board is never left half changed.
    pub fn apply(&mut self, operations: &[Operation]) -> Result<Vec<OperationResult>, OpError> {
        let mut results = Vec::new();
        for (position, operation) in operations.iter().enumerate() {
            let ids = self.apply_one(operation).map_err(|message| {
                format!("Operation {} ({}): {message}", position + 1, operation.op)
            })?;
            results.push(OperationResult {
                op: operation.op.clone(),
                ids,
            });
        }
        self.route_arrows();
        self.place_labels();
        self.route_arrows();
        self.stamp();
        Ok(results)
    }

    fn apply_one(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        match operation.op.as_str() {
            "add" => self.add(operation),
            "connect" => self.connect(operation),
            "update" => self.update(operation),
            "delete" => self.delete(operation),
            "move" => self.shift(operation),
            "align" => self.align(operation),
            "distribute" => self.distribute(operation),
            "layout" => self.layout(operation),
            "group" => self.group(operation),
            "ungroup" => self.ungroup(operation),
            "order" => self.order(operation),
            other => Err(format!(
                "{other} isn't an operation: use add, update, delete, connect, move, align, distribute, layout, group, ungroup, or order."
            )),
        }
    }

    fn add(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let kind = operation
            .kind
            .as_deref()
            .ok_or("add needs a type: rectangle, ellipse, diamond, text, arrow, line, or frame.")?;
        let new_id = self.new_id(operation.id.as_deref())?;
        let (x, y) = (operation.x.unwrap_or(0.0), operation.y.unwrap_or(0.0));
        let mut created = vec![new_id.clone()];
        match kind {
            "rectangle" | "ellipse" | "diamond" | "frame" => {
                let (width, height) = match kind {
                    "frame" => (800.0, 600.0),
                    _ => (160.0, 80.0),
                };
                let b = Bounds {
                    x,
                    y,
                    width: positive(operation.width, "width")?.unwrap_or(width),
                    height: positive(operation.height, "height")?.unwrap_or(height),
                };
                let mut element = self.base(kind, &new_id, b);
                if kind == "frame" {
                    element.insert(
                        "name".to_owned(),
                        operation.name.clone().map_or(Value::Null, Value::String),
                    );
                    element.insert("roughness".to_owned(), json!(0));
                }
                style(&mut element, operation)?;
                self.insert(element);
                self.set_common(&new_id, operation)?;
                if let Some(text) = operation.text.as_deref().filter(|text| !text.is_empty())
                    && kind != "frame"
                {
                    self.set_label(&new_id, text, operation.font_size);
                    created.extend(self.label_of(&new_id));
                }
            }
            "text" => {
                let text = operation
                    .text
                    .as_deref()
                    .ok_or("A text element needs text.")?;
                let font_size = font_size(operation.font_size)?.unwrap_or(DEFAULT_FONT_SIZE);
                let mut element = self.text_element(&new_id, text, x, y, font_size);
                style(&mut element, operation)?;
                if let Some(align) = operation.text_align.as_deref() {
                    element.insert("textAlign".to_owned(), json!(text_align(align)?));
                }
                self.insert(element);
                self.set_common(&new_id, operation)?;
            }
            "arrow" | "line" => {
                if kind == "arrow" && (operation.from.is_some() || operation.to.is_some()) {
                    return self.connect(operation);
                }
                let points = operation.points.clone().unwrap_or_else(|| {
                    vec![
                        [0.0, 0.0],
                        [
                            operation.width.unwrap_or(160.0),
                            operation.height.unwrap_or(0.0),
                        ],
                    ]
                });
                if points.len() < 2 {
                    return Err("A line needs at least two points.".to_owned());
                }
                let mut element = self.base(
                    kind,
                    &new_id,
                    Bounds {
                        x,
                        y,
                        width: 0.0,
                        height: 0.0,
                    },
                );
                element.insert(
                    "points".to_owned(),
                    Value::Array(points.iter().map(|p| json!([p[0], p[1]])).collect()),
                );
                fit_linear(&mut element);
                linear_defaults(&mut element, kind);
                style(&mut element, operation)?;
                arrowheads(&mut element, operation.arrowheads.as_deref(), kind)?;
                self.insert(element);
                self.set_common(&new_id, operation)?;
                if let Some(text) = operation.text.as_deref().filter(|text| !text.is_empty())
                    && kind == "arrow"
                {
                    self.set_label(&new_id, text, operation.font_size);
                    created.extend(self.label_of(&new_id));
                }
            }
            other => {
                return Err(format!(
                    "{other} can't be added: use rectangle, ellipse, diamond, text, arrow, line, or frame."
                ));
            }
        }
        Ok(created)
    }

    fn connect(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let from = operation.from.as_deref().ok_or("connect needs from.")?;
        let to = operation.to.as_deref().ok_or("connect needs to.")?;
        let start = self.get_live(from)?.clone();
        let end = self.get_live(to)?.clone();
        for (shape, name) in [(&start, from), (&end, to)] {
            if matches!(kind(shape), "arrow" | "line" | "freedraw") {
                return Err(format!(
                    "{name} is a line; connect shapes, text, or frames."
                ));
            }
        }
        let new_id = self.new_id(operation.id.as_deref())?;
        let a = edge_point(&start, bounds(&end).center());
        let b = edge_point(&end, bounds(&start).center());
        let mut element = self.base(
            "arrow",
            &new_id,
            Bounds {
                x: a.0,
                y: a.1,
                width: 0.0,
                height: 0.0,
            },
        );
        set_line(&mut element, a, b);
        linear_defaults(&mut element, "arrow");
        element.insert(
            "startBinding".to_owned(),
            json!({ "elementId": from, "focus": 0, "gap": ARROW_GAP }),
        );
        element.insert(
            "endBinding".to_owned(),
            json!({ "elementId": to, "focus": 0, "gap": ARROW_GAP }),
        );
        style(&mut element, operation)?;
        arrowheads(&mut element, operation.arrowheads.as_deref(), "arrow")?;
        self.insert(element);
        self.add_bound(from, &new_id, "arrow");
        self.add_bound(to, &new_id, "arrow");
        self.set_common(&new_id, operation)?;
        let mut created = vec![new_id.clone()];
        if let Some(text) = operation.text.as_deref().filter(|text| !text.is_empty()) {
            self.set_label(&new_id, text, operation.font_size);
            created.extend(self.label_of(&new_id));
        }
        Ok(created)
    }

    /// Frame, lock and link, which every kind of element takes.
    fn set_common(&mut self, target: &str, operation: &Operation) -> Result<(), OpError> {
        if let Some(frame) = operation.frame_id.as_deref() {
            let value = if frame.is_empty() {
                Value::Null
            } else {
                let frame_element = self.get_live(frame)?;
                if !matches!(kind(frame_element), "frame" | "magicframe") {
                    return Err(format!("{frame} isn't a frame."));
                }
                json!(frame)
            };
            let label = self.label_of(target);
            for member in std::iter::once(target.to_owned()).chain(label) {
                if let Some(element) = self.touch(&member) {
                    element.insert("frameId".to_owned(), value.clone());
                }
            }
        }
        if let Some(locked) = operation.locked
            && let Some(element) = self.touch(target)
        {
            element.insert("locked".to_owned(), json!(locked));
        }
        if let Some(link) = operation.link.as_deref() {
            if !super::elements::safe_link(link) || link.len() > 2_000 {
                return Err("A link must be a web, mail or in-app link.".to_owned());
            }
            if let Some(element) = self.touch(target) {
                element.insert(
                    "link".to_owned(),
                    if link.trim().is_empty() {
                        Value::Null
                    } else {
                        json!(link.trim())
                    },
                );
            }
        }
        Ok(())
    }

    fn update(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let target = operation
            .id
            .as_deref()
            .ok_or("update needs an id.")?
            .to_owned();
        let current = self.get_live(&target)?.clone();
        let element_kind = kind(&current).to_owned();
        if current.get("locked").and_then(Value::as_bool) == Some(true)
            && operation.locked != Some(false)
        {
            return Err(format!(
                "{target} is locked. Pass locked: false to unlock it first."
            ));
        }
        let mut changed = vec![target.clone()];
        {
            let element = self.touch(&target).expect("live element");
            style(element, operation)?;
            if is_linear(&element_kind) {
                arrowheads(element, operation.arrowheads.as_deref(), &element_kind)?;
            }
            if let Some(name) = &operation.name {
                element.insert("name".to_owned(), json!(name));
            }
        }
        // Moving to x, y moves its label too.
        let b = bounds(&current);
        let dx = operation.x.map_or(0.0, |x| x - number(&current, "x"));
        let dy = operation.y.map_or(0.0, |y| y - number(&current, "y"));
        if dx != 0.0 || dy != 0.0 {
            self.translate(&target, dx, dy);
        }
        let width = positive(operation.width, "width")?;
        let height = positive(operation.height, "height")?;
        if width.is_some() || height.is_some() {
            let element = self.touch(&target).expect("live element");
            if is_linear(&element_kind) {
                let scale_x = width.map_or(1.0, |w| if b.width > 0.0 { w / b.width } else { 1.0 });
                let scale_y =
                    height.map_or(1.0, |h| if b.height > 0.0 { h / b.height } else { 1.0 });
                let scaled = points(element)
                    .into_iter()
                    .map(|(px, py)| json!([px * scale_x, py * scale_y]))
                    .collect();
                element.insert("points".to_owned(), Value::Array(scaled));
                fit_linear(element);
            } else {
                if let Some(width) = width {
                    element.insert("width".to_owned(), json!(width));
                }
                if let Some(height) = height {
                    element.insert("height".to_owned(), json!(height));
                }
            }
            self.moved.insert(target.clone());
        }
        if let Some(points) = &operation.points {
            if !is_linear(&element_kind) || points.len() < 2 {
                return Err("points only apply to lines and arrows, with at least two.".to_owned());
            }
            let element = self.touch(&target).expect("live element");
            element.insert(
                "points".to_owned(),
                Value::Array(points.iter().map(|p| json!([p[0], p[1]])).collect()),
            );
            fit_linear(element);
        }
        if let Some(text) = operation.text.as_deref() {
            if element_kind == "text" {
                let font_size = font_size(operation.font_size)?.unwrap_or_else(|| {
                    Some(number(&current, "fontSize"))
                        .filter(|s| *s > 0.0)
                        .unwrap_or(DEFAULT_FONT_SIZE)
                });
                let (w, h) = measure_text(text, font_size);
                let container = text_field(&current, "containerId").map(str::to_owned);
                let element = self.touch(&target).expect("live element");
                element.insert("text".to_owned(), json!(text));
                element.insert("originalText".to_owned(), json!(text));
                element.insert("fontSize".to_owned(), json!(font_size));
                element.insert("width".to_owned(), json!(w));
                element.insert("height".to_owned(), json!(h));
                if let Some(container) = container {
                    self.moved.insert(container);
                }
            } else if matches!(
                element_kind.as_str(),
                "rectangle" | "ellipse" | "diamond" | "arrow"
            ) {
                self.set_label(&target, text, font_size(operation.font_size)?);
                changed.extend(self.label_of(&target));
            } else {
                return Err(format!("A {element_kind} can't hold text."));
            }
        } else if let Some(size) = font_size(operation.font_size)? {
            let label = if element_kind == "text" {
                Some(target.clone())
            } else {
                self.label_of(&target)
            };
            if let Some(label) = label
                && let Some(element) = self.touch(&label)
            {
                element.insert("fontSize".to_owned(), json!(size));
                self.moved.insert(target.clone());
            }
        }
        if let Some(align) = operation.text_align.as_deref() {
            let align = text_align(align)?;
            let label = if element_kind == "text" {
                Some(target.clone())
            } else {
                self.label_of(&target)
            };
            if let Some(label) = label
                && let Some(element) = self.touch(&label)
            {
                element.insert("textAlign".to_owned(), json!(align));
            }
        }
        if element_kind == "arrow" && (operation.from.is_some() || operation.to.is_some()) {
            for (field, end) in [
                ("startBinding", &operation.from),
                ("endBinding", &operation.to),
            ] {
                let Some(end) = end else { continue };
                let previous = binding_target(&current, field);
                if let Some(previous) = previous {
                    self.remove_bound(&previous, &target);
                }
                if end.is_empty() {
                    if let Some(element) = self.touch(&target) {
                        element.insert(field.to_owned(), Value::Null);
                    }
                    continue;
                }
                self.get_live(end)?;
                if let Some(element) = self.touch(&target) {
                    element.insert(
                        field.to_owned(),
                        json!({ "elementId": end, "focus": 0, "gap": ARROW_GAP }),
                    );
                }
                self.add_bound(end, &target, "arrow");
            }
            self.moved.insert(target.clone());
        }
        self.set_common(&target, operation)?;
        Ok(changed)
    }

    fn delete(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let mut removed = BTreeSet::new();
        for target in &ids {
            removed.insert(target.clone());
            removed.extend(self.label_of(target));
        }
        for target in removed.clone() {
            if let Some(element) = self.touch(&target) {
                element.insert("isDeleted".to_owned(), json!(true));
            }
        }
        // Unhook what pointed at them: arrows' ends, containers' lists, frames.
        let others = self
            .live()
            .map(|element| id(element).to_owned())
            .collect::<Vec<_>>();
        for other in others {
            let element = self.get(&other).cloned().unwrap_or_default();
            for field in ["startBinding", "endBinding"] {
                if binding_target(&element, field).is_some_and(|target| removed.contains(&target))
                    && let Some(element) = self.touch(&other)
                {
                    element.insert(field.to_owned(), Value::Null);
                }
            }
            if text_field(&element, "frameId").is_some_and(|frame| removed.contains(frame))
                && let Some(element) = self.touch(&other)
            {
                element.insert("frameId".to_owned(), Value::Null);
            }
            for target in &removed {
                self.remove_bound(&other, target);
            }
        }
        Ok(removed.into_iter().collect())
    }

    /// Moves elements and what travels with them: their labels and, for a
    /// frame, everything in it.
    fn translate(&mut self, target: &str, dx: f64, dy: f64) {
        let mut moving = vec![target.to_owned()];
        moving.extend(self.label_of(target));
        if self
            .get(target)
            .is_some_and(|element| matches!(kind(element), "frame" | "magicframe"))
        {
            moving.extend(
                self.live()
                    .filter(|element| text_field(element, "frameId") == Some(target))
                    .map(|element| id(element).to_owned()),
            );
        }
        let mut seen = BTreeSet::new();
        for member in moving {
            if !seen.insert(member.clone()) {
                continue;
            }
            if let Some(element) = self.touch(&member) {
                let x = number(element, "x") + dx;
                let y = number(element, "y") + dy;
                element.insert("x".to_owned(), json!(x));
                element.insert("y".to_owned(), json!(y));
            }
            self.moved.insert(member);
        }
    }

    fn shift(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let (dx, dy) = match (operation.dx, operation.dy, operation.x, operation.y) {
            (None, None, None, None) => return Err("move needs dx and dy, or x and y.".to_owned()),
            (dx, dy, None, None) => (dx.unwrap_or(0.0), dy.unwrap_or(0.0)),
            (_, _, x, y) => {
                // x and y place the group's top-left corner there.
                let group = Bounds::union(ids.iter().filter_map(|id| self.get(id)).map(bounds))
                    .ok_or("Nothing to move.")?;
                (
                    x.map_or(0.0, |x| x - group.x),
                    y.map_or(0.0, |y| y - group.y),
                )
            }
        };
        for target in &ids {
            self.translate(target, dx, dy);
        }
        Ok(ids)
    }

    fn align(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let boxes = ids
            .iter()
            .map(|id| (id.clone(), bounds(self.get(id).expect("live"))))
            .collect::<Vec<_>>();
        let group = Bounds::union(boxes.iter().map(|(_, b)| *b)).ok_or("Nothing to align.")?;
        let edge = operation.edge.as_deref().unwrap_or("left");
        for (target, b) in boxes {
            let (dx, dy) = match edge {
                "left" => (group.x - b.x, 0.0),
                "right" => (group.right() - b.right(), 0.0),
                "center" => (group.center().0 - b.center().0, 0.0),
                "top" => (0.0, group.y - b.y),
                "bottom" => (0.0, group.bottom() - b.bottom()),
                "middle" => (0.0, group.center().1 - b.center().1),
                other => {
                    return Err(format!(
                        "{other} isn't an edge: use left, center, right, top, middle, or bottom."
                    ));
                }
            };
            self.translate(&target, dx, dy);
        }
        Ok(ids)
    }

    fn distribute(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let horizontal = match operation.axis.as_deref().unwrap_or("horizontal") {
            "horizontal" => true,
            "vertical" => false,
            other => {
                return Err(format!(
                    "{other} isn't an axis: use horizontal or vertical."
                ));
            }
        };
        let mut boxes = ids
            .iter()
            .map(|id| (id.clone(), bounds(self.get(id).expect("live"))))
            .collect::<Vec<_>>();
        boxes.sort_by(|a, b| {
            let (left, right) = if horizontal {
                (a.1.x, b.1.x)
            } else {
                (a.1.y, b.1.y)
            };
            left.total_cmp(&right)
        });
        let group = Bounds::union(boxes.iter().map(|(_, b)| *b)).ok_or("Nothing to distribute.")?;
        let total = boxes
            .iter()
            .map(|(_, b)| if horizontal { b.width } else { b.height })
            .sum::<f64>();
        let span = if horizontal {
            group.width
        } else {
            group.height
        };
        let gap = match operation.gap {
            Some(gap) => gap,
            None if boxes.len() > 1 => (span - total) / (boxes.len() - 1) as f64,
            None => 0.0,
        };
        let mut cursor = if horizontal { group.x } else { group.y };
        for (target, b) in boxes {
            let (dx, dy) = if horizontal {
                (cursor - b.x, 0.0)
            } else {
                (0.0, cursor - b.y)
            };
            self.translate(&target, dx, dy);
            cursor += if horizontal { b.width } else { b.height } + gap;
        }
        Ok(ids)
    }

    /// Lays shapes out in layers by their arrows (a simple layered graph
    /// layout), or in a grid.
    fn layout(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = match &operation.ids {
            Some(ids) if !ids.is_empty() => {
                for id in ids {
                    self.get_live(id)?;
                }
                ids.clone()
            }
            _ => self
                .live()
                .filter(|element| {
                    !matches!(
                        kind(element),
                        "arrow" | "line" | "freedraw" | "frame" | "magicframe"
                    ) && text_field(element, "containerId").is_none()
                })
                .map(|element| id(element).to_owned())
                .collect(),
        };
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let boxes = ids
            .iter()
            .map(|id| (id.clone(), bounds(self.get(id).expect("live"))))
            .collect::<BTreeMap<_, _>>();
        let origin = Bounds::union(boxes.values().copied()).expect("not empty");
        let gap = operation.gap.unwrap_or(60.0).max(0.0);
        let direction = operation.direction.as_deref().unwrap_or("down");
        let positions = match direction {
            "grid" => grid_layout(&ids, &boxes, gap),
            "down" | "right" => {
                let edges = self
                    .live()
                    .filter(|element| kind(element) == "arrow")
                    .filter_map(|element| {
                        let from = binding_target(element, "startBinding")?;
                        let to = binding_target(element, "endBinding")?;
                        (boxes.contains_key(&from) && boxes.contains_key(&to) && from != to)
                            .then_some((from, to))
                    })
                    .collect::<Vec<_>>();
                layered_layout(&ids, &boxes, &edges, gap, direction == "right")
            }
            other => {
                return Err(format!(
                    "{other} isn't a direction: use down, right, or grid."
                ));
            }
        };
        for (target, (x, y)) in positions {
            let b = boxes[&target];
            self.translate(&target, origin.x + x - b.x, origin.y + y - b.y);
        }
        Ok(ids)
    }

    fn group(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let group = random_id();
        let mut members = ids.clone();
        for target in &ids {
            members.extend(self.label_of(target));
        }
        for member in &members {
            if let Some(element) = self.touch(member) {
                let mut groups = element
                    .get("groupIds")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                groups.push(json!(group));
                element.insert("groupIds".to_owned(), Value::Array(groups));
            }
        }
        Ok(ids)
    }

    fn ungroup(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let mut members = ids.clone();
        for target in &ids {
            members.extend(self.label_of(target));
        }
        for member in &members {
            if let Some(element) = self.touch(member) {
                let mut groups = element
                    .get("groupIds")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                groups.pop();
                element.insert("groupIds".to_owned(), Value::Array(groups));
            }
        }
        Ok(ids)
    }

    fn order(&mut self, operation: &Operation) -> Result<Vec<String>, OpError> {
        let ids = self.ids_of(operation)?;
        let front = match operation.position.as_deref().unwrap_or("front") {
            "front" => true,
            "back" => false,
            other => return Err(format!("{other} isn't a position: use front or back.")),
        };
        let mut members = Vec::new();
        for target in &ids {
            members.push(target.clone());
            members.extend(self.label_of(target));
        }
        if !front {
            members.reverse();
        }
        for member in members {
            let index = if front {
                fractional::key_between(self.last_index().as_deref(), None)
            } else {
                fractional::key_between(None, self.first_index().as_deref())
            };
            if let (Some(index), Some(element)) = (index, self.touch(&member)) {
                element.insert("index".to_owned(), json!(index));
            }
        }
        self.sort();
        Ok(ids)
    }

    fn sort(&mut self) {
        self.elements.sort_by(|a, b| {
            text_field(a, "index")
                .unwrap_or("~")
                .cmp(text_field(b, "index").unwrap_or("~"))
                .then_with(|| id(a).cmp(id(b)))
        });
        self.by_id = self
            .elements
            .iter()
            .enumerate()
            .map(|(position, element)| (id(element).to_owned(), position))
            .collect();
    }

    /// Gives every changed element its next version, a new versionNonce and
    /// the time, once.
    fn stamp(&mut self) {
        for target in self.changed.clone() {
            let loaded = self.loaded_versions.get(&target).copied();
            let now = self.now_ms;
            if let Some(element) = self.touch(&target) {
                let version = match loaded {
                    Some(version) => version + 1,
                    None => element.get("version").and_then(Value::as_i64).unwrap_or(1),
                };
                element.insert("version".to_owned(), json!(version.max(1)));
                element.insert("versionNonce".to_owned(), json!(random_nonce()));
                element.insert("updated".to_owned(), json!(now));
            }
        }
    }

    /// The elements this batch created or changed, ready to store.
    pub fn changed(&self) -> Vec<Object> {
        self.changed
            .iter()
            .filter_map(|id| self.get(id).cloned())
            .collect()
    }
}

fn binding_target(element: &Object, field: &str) -> Option<String> {
    element
        .get(field)
        .and_then(|binding| binding.get("elementId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Where a line from a shape's center toward `toward` leaves the shape, plus
/// the arrow gap.
pub fn edge_point(shape: &Object, toward: (f64, f64)) -> (f64, f64) {
    let b = bounds(shape);
    let (cx, cy) = b.center();
    let (dx, dy) = (toward.0 - cx, toward.1 - cy);
    let length = (dx * dx + dy * dy).sqrt();
    if length < f64::EPSILON || b.width <= 0.0 || b.height <= 0.0 {
        return (cx, cy);
    }
    let (half_w, half_h) = (b.width / 2.0, b.height / 2.0);
    let t = match kind(shape) {
        "ellipse" => 1.0 / ((dx / half_w).powi(2) + (dy / half_h).powi(2)).sqrt(),
        "diamond" => 1.0 / (dx.abs() / half_w + dy.abs() / half_h),
        _ => {
            let tx = if dx.abs() > f64::EPSILON {
                half_w / dx.abs()
            } else {
                f64::INFINITY
            };
            let ty = if dy.abs() > f64::EPSILON {
                half_h / dy.abs()
            } else {
                f64::INFINITY
            };
            tx.min(ty)
        }
    };
    let t = t.min(1.0);
    (
        cx + dx * t + dx / length * ARROW_GAP,
        cy + dy * t + dy / length * ARROW_GAP,
    )
}

fn set_line(element: &mut Object, from: (f64, f64), to: (f64, f64)) {
    element.insert("x".to_owned(), json!(from.0));
    element.insert("y".to_owned(), json!(from.1));
    element.insert(
        "points".to_owned(),
        json!([[0.0, 0.0], [to.0 - from.0, to.1 - from.1]]),
    );
    element.insert("width".to_owned(), json!((to.0 - from.0).abs()));
    element.insert("height".to_owned(), json!((to.1 - from.1).abs()));
}

/// Keeps a line's first point at its x, y (as Excalidraw does) and its width
/// and height matching its points.
fn fit_linear(element: &mut Object) {
    let points = points(element);
    let Some(first) = points.first().copied() else {
        return;
    };
    let x = number(element, "x") + first.0;
    let y = number(element, "y") + first.1;
    let shifted = points
        .iter()
        .map(|(px, py)| (px - first.0, py - first.1))
        .collect::<Vec<_>>();
    let width = shifted
        .iter()
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max)
        - shifted.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let height = shifted
        .iter()
        .map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max)
        - shifted.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    element.insert("x".to_owned(), json!(x));
    element.insert("y".to_owned(), json!(y));
    element.insert(
        "points".to_owned(),
        Value::Array(shifted.iter().map(|p| json!([p.0, p.1])).collect()),
    );
    element.insert("width".to_owned(), json!(width));
    element.insert("height".to_owned(), json!(height));
}

fn linear_defaults(element: &mut Object, kind: &str) {
    element.insert("lastCommittedPoint".to_owned(), Value::Null);
    element.insert("startBinding".to_owned(), Value::Null);
    element.insert("endBinding".to_owned(), Value::Null);
    element.insert("startArrowhead".to_owned(), Value::Null);
    element.insert(
        "endArrowhead".to_owned(),
        if kind == "arrow" {
            json!("arrow")
        } else {
            Value::Null
        },
    );
    if kind == "arrow" {
        element.insert("elbowed".to_owned(), json!(false));
    }
}

fn arrowheads(element: &mut Object, value: Option<&str>, kind: &str) -> Result<(), OpError> {
    let Some(value) = value else {
        return Ok(());
    };
    if kind != "arrow" {
        return Err("arrowheads only apply to arrows.".to_owned());
    }
    let (start, end) = match value {
        "end" => (Value::Null, json!("arrow")),
        "start" => (json!("arrow"), Value::Null),
        "both" => (json!("arrow"), json!("arrow")),
        "none" => (Value::Null, Value::Null),
        other => {
            return Err(format!(
                "{other} isn't an arrowheads value: use end, start, both, or none."
            ));
        }
    };
    element.insert("startArrowhead".to_owned(), start);
    element.insert("endArrowhead".to_owned(), end);
    Ok(())
}

fn positive(value: Option<f64>, field: &str) -> Result<Option<f64>, OpError> {
    match value {
        Some(value) if !(value.is_finite() && value > 0.0 && value <= 100_000.0) => {
            Err(format!("{field} must be a positive number up to 100,000."))
        }
        value => Ok(value),
    }
}

fn font_size(value: Option<f64>) -> Result<Option<f64>, OpError> {
    match value {
        Some(value) if !(value.is_finite() && (4.0..=400.0).contains(&value)) => {
            Err("fontSize must be between 4 and 400.".to_owned())
        }
        value => Ok(value),
    }
}

fn text_align(value: &str) -> Result<&'static str, OpError> {
    match value {
        "left" => Ok("left"),
        "center" => Ok("center"),
        "right" => Ok("right"),
        other => Err(format!(
            "{other} isn't a textAlign: use left, center, or right."
        )),
    }
}

/// A colour a board can draw: #rgb, #rrggbb (with alpha), a named colour, or
/// transparent.
pub fn valid_color(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit());
    }
    (3..=24).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_lowercase())
}

fn style(element: &mut Object, operation: &Operation) -> Result<(), OpError> {
    for (field, value) in [
        ("strokeColor", &operation.stroke_color),
        ("backgroundColor", &operation.background_color),
    ] {
        if let Some(value) = value {
            if !valid_color(value) {
                return Err(format!(
                    "{value} isn't a colour: use #rrggbb or transparent."
                ));
            }
            element.insert(field.to_owned(), json!(value));
        }
    }
    if let Some(fill) = operation.fill_style.as_deref() {
        if !matches!(fill, "solid" | "hachure" | "cross-hatch" | "zigzag") {
            return Err(format!(
                "{fill} isn't a fillStyle: use solid, hachure, cross-hatch, or zigzag."
            ));
        }
        element.insert("fillStyle".to_owned(), json!(fill));
    }
    if let Some(stroke) = operation.stroke_style.as_deref() {
        if !matches!(stroke, "solid" | "dashed" | "dotted") {
            return Err(format!(
                "{stroke} isn't a strokeStyle: use solid, dashed, or dotted."
            ));
        }
        element.insert("strokeStyle".to_owned(), json!(stroke));
    }
    if let Some(width) = operation.stroke_width {
        if !(width.is_finite() && (0.5..=40.0).contains(&width)) {
            return Err("strokeWidth must be between 0.5 and 40.".to_owned());
        }
        element.insert("strokeWidth".to_owned(), json!(width));
    }
    if let Some(opacity) = operation.opacity {
        if !(opacity.is_finite() && (0.0..=100.0).contains(&opacity)) {
            return Err("opacity must be between 0 and 100.".to_owned());
        }
        element.insert("opacity".to_owned(), json!(opacity.round()));
    }
    if let Some(roughness) = operation.roughness {
        if !(roughness.is_finite() && (0.0..=2.0).contains(&roughness)) {
            return Err("roughness must be 0, 1 or 2.".to_owned());
        }
        element.insert("roughness".to_owned(), json!(roughness.round()));
    }
    if let Some(rounded) = operation.rounded {
        let kind = kind(element).to_owned();
        if !matches!(kind.as_str(), "rectangle" | "diamond") {
            return Err("rounded only applies to rectangles and diamonds.".to_owned());
        }
        element.insert(
            "roundness".to_owned(),
            if rounded {
                json!({ "type": if kind == "rectangle" { 3 } else { 2 } })
            } else {
                Value::Null
            },
        );
    }
    Ok(())
}

fn grid_layout(
    ids: &[String],
    boxes: &BTreeMap<String, Bounds>,
    gap: f64,
) -> Vec<(String, (f64, f64))> {
    let columns = (ids.len() as f64).sqrt().ceil().max(1.0) as usize;
    let cell_w = boxes.values().map(|b| b.width).fold(0.0, f64::max);
    let cell_h = boxes.values().map(|b| b.height).fold(0.0, f64::max);
    ids.iter()
        .enumerate()
        .map(|(position, id)| {
            let (column, row) = (position % columns, position / columns);
            let b = boxes[id];
            (
                id.clone(),
                (
                    column as f64 * (cell_w + gap) + (cell_w - b.width) / 2.0,
                    row as f64 * (cell_h + gap) + (cell_h - b.height) / 2.0,
                ),
            )
        })
        .collect()
}

/// Layers by longest path from the sources (cycles broken at back edges),
/// orders each layer by its parents' average place, and centers the layers
/// on each other. Positions are relative to the layout's top-left.
fn layered_layout(
    ids: &[String],
    boxes: &BTreeMap<String, Bounds>,
    edges: &[(String, String)],
    gap: f64,
    rightward: bool,
) -> Vec<(String, (f64, f64))> {
    let position = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index))
        .collect::<HashMap<_, _>>();
    let count = ids.len();
    let mut successors = vec![Vec::new(); count];
    for (from, to) in edges {
        successors[position[from.as_str()]].push(position[to.as_str()]);
    }
    // Drop back edges found by depth-first search, so cycles still lay out.
    let mut state = vec![0u8; count];
    let mut forward = vec![Vec::new(); count];
    fn visit(node: usize, successors: &[Vec<usize>], state: &mut [u8], forward: &mut [Vec<usize>]) {
        state[node] = 1;
        for &next in &successors[node] {
            match state[next] {
                0 => {
                    forward[node].push(next);
                    visit(next, successors, state, forward);
                }
                2 => forward[node].push(next),
                _ => {}
            }
        }
        state[node] = 2;
    }
    for node in 0..count {
        if state[node] == 0 {
            visit(node, &successors, &mut state, &mut forward);
        }
    }
    let mut indegree = vec![0usize; count];
    for targets in &forward {
        for &target in targets {
            indegree[target] += 1;
        }
    }
    let mut layer = vec![0usize; count];
    let mut queue = (0..count)
        .filter(|node| indegree[*node] == 0)
        .collect::<Vec<_>>();
    let mut cursor = 0;
    while cursor < queue.len() {
        let node = queue[cursor];
        cursor += 1;
        for &next in &forward[node] {
            layer[next] = layer[next].max(layer[node] + 1);
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push(next);
            }
        }
    }
    let depth = layer.iter().copied().max().unwrap_or(0) + 1;
    let mut layers = vec![Vec::new(); depth];
    for node in 0..count {
        layers[layer[node]].push(node);
    }
    let mut predecessors = vec![Vec::new(); count];
    for (from, targets) in forward.iter().enumerate() {
        for &target in targets {
            predecessors[target].push(from);
        }
    }
    let mut order = vec![0.0f64; count];
    for nodes in &layers {
        for (index, node) in nodes.iter().enumerate() {
            order[*node] = index as f64;
        }
    }
    for current in layers.iter_mut().skip(1) {
        current.sort_by(|a, b| {
            let score = |node: &usize| {
                let parents = &predecessors[*node];
                if parents.is_empty() {
                    order[*node]
                } else {
                    parents.iter().map(|parent| order[*parent]).sum::<f64>() / parents.len() as f64
                }
            };
            score(a).total_cmp(&score(b))
        });
        for (index, node) in current.iter().enumerate() {
            order[*node] = index as f64;
        }
    }
    let size = |node: usize| {
        let b = boxes[&ids[node]];
        if rightward {
            (b.height, b.width)
        } else {
            (b.width, b.height)
        }
    };
    let spans = layers
        .iter()
        .map(|nodes| {
            nodes.iter().map(|node| size(*node).0).sum::<f64>()
                + gap * nodes.len().saturating_sub(1) as f64
        })
        .collect::<Vec<_>>();
    let widest = spans.iter().copied().fold(0.0, f64::max);
    let mut placed = Vec::new();
    let mut depth_cursor = 0.0;
    for (nodes, span) in layers.iter().zip(&spans) {
        let thickness = nodes.iter().map(|node| size(*node).1).fold(0.0, f64::max);
        let mut along = (widest - span) / 2.0;
        for &node in nodes {
            let (length, depth_size) = size(node);
            let across = depth_cursor + (thickness - depth_size) / 2.0;
            let point = if rightward {
                (across, along)
            } else {
                (along, across)
            };
            placed.push((ids[node].clone(), point));
            along += length + gap;
        }
        depth_cursor += thickness + gap * 1.5;
    }
    placed
}

/// What an agent sees of an element: where it is, what it says, and what it
/// connects, without Excalidraw's drawing details.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ElementSummary {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub angle: Option<f64>,
    /// A text element's text, or the label inside a shape or on an arrow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// For a label: the shape or arrow it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_id: Option<String>,
    /// For an arrow: the element it starts at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// For an arrow: the element it ends at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_color: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<String>,
    /// A frame's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// How many points a line, arrow or freehand stroke has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_count: Option<usize>,
    /// The stored version: it goes up by one with every change.
    pub version: i64,
}

pub fn summarize(element: &Object, scene: &Scene) -> ElementSummary {
    let element_kind = kind(element).to_owned();
    let b = bounds(element);
    let own_id = id(element).to_owned();
    let text = if element_kind == "text" {
        text_field(element, "originalText")
            .or_else(|| text_field(element, "text"))
            .map(str::to_owned)
    } else {
        scene
            .label_of(&own_id)
            .and_then(|label| scene.get(&label))
            .and_then(|label| {
                text_field(label, "originalText").or_else(|| text_field(label, "text"))
            })
            .map(str::to_owned)
    };
    let angle = Some(number(element, "angle")).filter(|angle| angle.abs() > 1e-9);
    let groups = element
        .get("groupIds")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    ElementSummary {
        id: own_id,
        x: round(b.x),
        y: round(b.y),
        width: round(b.width),
        height: round(b.height),
        angle,
        text,
        container_id: text_field(element, "containerId").map(str::to_owned),
        from: binding_target(element, "startBinding"),
        to: binding_target(element, "endBinding"),
        stroke_color: text_field(element, "strokeColor").map(str::to_owned),
        background_color: text_field(element, "backgroundColor")
            .filter(|color| *color != "transparent")
            .map(str::to_owned),
        group_ids: groups,
        frame_id: text_field(element, "frameId").map(str::to_owned),
        name: text_field(element, "name").map(str::to_owned),
        link: text_field(element, "link").map(str::to_owned),
        locked: element
            .get("locked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        point_count: is_linear(&element_kind).then(|| points(element).len()),
        version: element.get("version").and_then(Value::as_i64).unwrap_or(0),
        kind: element_kind,
    }
}

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// The words on a board, for search: text and labels, in stacking order.
pub fn text_content(elements: &[Object]) -> String {
    let mut words = String::new();
    for element in elements.iter().filter(|element| !is_deleted(element)) {
        let text = match kind(element) {
            "text" => text_field(element, "originalText").or_else(|| text_field(element, "text")),
            "frame" | "magicframe" => text_field(element, "name"),
            _ => None,
        };
        if let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) {
            if !words.is_empty() {
                words.push('\n');
            }
            words.push_str(text);
            if words.chars().count() >= 20_000 {
                return words.chars().take(20_000).collect();
            }
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{Operation, Scene, bounds, kind, text_field};

    fn op(value: serde_json::Value) -> Operation {
        serde_json::from_value(value).expect("operation")
    }

    fn scene_with(operations: Vec<serde_json::Value>) -> Scene {
        let mut scene = Scene::new(Vec::new(), 1_700_000_000_000);
        scene
            .apply(&operations.into_iter().map(op).collect::<Vec<_>>())
            .expect("applies");
        scene
    }

    #[test]
    fn labels_become_bound_text_and_connections_bind_both_ends() {
        let scene = scene_with(vec![
            serde_json::json!({"op": "add", "type": "rectangle", "id": "api", "x": 0, "y": 0, "text": "API"}),
            serde_json::json!({"op": "add", "type": "ellipse", "id": "db", "x": 400, "y": 0, "text": "Database"}),
            serde_json::json!({"op": "connect", "id": "reads", "from": "api", "to": "db", "text": "reads"}),
        ]);
        let get = |id: &str| scene.get(id).expect(id).clone();
        let label = get("api-label");
        assert_eq!(text_field(&label, "containerId"), Some("api"));
        assert_eq!(text_field(&label, "text"), Some("API"));
        let api = get("api");
        let bound = api["boundElements"].as_array().expect("bound");
        assert!(
            bound
                .iter()
                .any(|entry| entry["id"] == "api-label" && entry["type"] == "text")
        );
        assert!(
            bound
                .iter()
                .any(|entry| entry["id"] == "reads" && entry["type"] == "arrow")
        );
        let arrow = get("reads");
        assert_eq!(arrow["startBinding"]["elementId"], "api");
        assert_eq!(arrow["endBinding"]["elementId"], "db");
        // The arrow runs from the box's right edge to the ellipse's left edge.
        let x = arrow["x"].as_f64().expect("x");
        assert!((x - 166.0).abs() < 1.0, "starts at {x}");
        let end = x + arrow["points"][1][0].as_f64().expect("end");
        assert!((end - 394.0).abs() < 1.0, "ends at {end}");
        // Every element is new: version 1, all with stacking indices in order.
        let indices = scene
            .elements()
            .iter()
            .map(|element| text_field(element, "index").expect("index").to_owned())
            .collect::<Vec<_>>();
        let mut sorted = indices.clone();
        sorted.sort();
        assert_eq!(indices, sorted);
        assert!(
            scene
                .elements()
                .iter()
                .all(|element| element["version"] == 1)
        );
    }

    #[test]
    fn moving_a_shape_moves_its_label_and_reroutes_its_arrows_with_one_version_each() {
        let built = scene_with(vec![
            serde_json::json!({"op": "add", "type": "rectangle", "id": "a", "x": 0, "y": 0, "text": "A"}),
            serde_json::json!({"op": "add", "type": "rectangle", "id": "b", "x": 400, "y": 0}),
            serde_json::json!({"op": "connect", "id": "ab", "from": "a", "to": "b"}),
        ]);
        let mut scene = Scene::new(built.elements().to_vec(), 1_700_000_001_000);
        scene
            .apply(&[op(
                serde_json::json!({"op": "move", "ids": ["b"], "dx": 0, "dy": 300}),
            )])
            .expect("moves");
        let changed = scene.changed();
        let ids = changed
            .iter()
            .map(|e| super::id(e).to_owned())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"b".to_owned()) && ids.contains(&"ab".to_owned()));
        assert!(
            !ids.contains(&"a-label".to_owned()),
            "untouched label stays: {ids:?}"
        );
        for element in &changed {
            assert_eq!(element["version"], 2, "{}", super::id(element));
        }
        let arrow = scene.get("ab").expect("arrow");
        let end_y = arrow["y"].as_f64().expect("y") + arrow["points"][1][1].as_f64().expect("dy");
        assert!(end_y > 250.0, "arrow follows b down: {end_y}");
    }

    #[test]
    fn deleting_a_shape_takes_its_label_and_unhooks_arrows() {
        let built = scene_with(vec![
            serde_json::json!({"op": "add", "type": "rectangle", "id": "a", "text": "A"}),
            serde_json::json!({"op": "add", "type": "rectangle", "id": "b", "x": 300}),
            serde_json::json!({"op": "connect", "id": "ab", "from": "a", "to": "b"}),
        ]);
        let mut scene = Scene::new(built.elements().to_vec(), 2);
        scene
            .apply(&[op(serde_json::json!({"op": "delete", "ids": ["a"]}))])
            .expect("deletes");
        assert_eq!(scene.get("a").expect("a")["isDeleted"], true);
        assert_eq!(scene.get("a-label").expect("label")["isDeleted"], true);
        assert_eq!(scene.get("ab").expect("arrow")["startBinding"], Value::Null);
        assert_eq!(scene.get("ab").expect("arrow")["isDeleted"], false);
    }

    #[test]
    fn a_refused_operation_changes_nothing() {
        let mut scene = Scene::new(Vec::new(), 1);
        let error = scene
            .apply(&[
                op(serde_json::json!({"op": "add", "type": "rectangle", "id": "a"})),
                op(serde_json::json!({"op": "connect", "from": "a", "to": "missing"})),
            ])
            .expect_err("refused");
        assert!(error.contains("Operation 2"), "{error}");
        assert!(error.contains("missing"), "{error}");
    }

    #[test]
    fn layout_puts_a_chain_in_layers_and_align_lines_things_up() {
        let built = scene_with(vec![
            serde_json::json!({"op": "add", "type": "rectangle", "id": "a", "x": 500, "y": 500}),
            serde_json::json!({"op": "add", "type": "rectangle", "id": "b", "x": 0, "y": 0}),
            serde_json::json!({"op": "add", "type": "rectangle", "id": "c", "x": 900, "y": 20}),
            serde_json::json!({"op": "connect", "from": "a", "to": "b"}),
            serde_json::json!({"op": "connect", "from": "b", "to": "c"}),
            serde_json::json!({"op": "layout", "direction": "down"}),
        ]);
        let y = |id: &str| bounds(built.get(id).expect(id)).y;
        assert!(
            y("a") < y("b") && y("b") < y("c"),
            "{} {} {}",
            y("a"),
            y("b"),
            y("c")
        );
        let aligned = scene_with(vec![
            serde_json::json!({"op": "add", "type": "rectangle", "id": "a", "x": 10, "y": 0}),
            serde_json::json!({"op": "add", "type": "rectangle", "id": "b", "x": 90, "y": 200, "width": 50}),
            serde_json::json!({"op": "align", "ids": ["a", "b"], "edge": "left"}),
        ]);
        assert_eq!(bounds(aligned.get("b").expect("b")).x, 10.0);
    }

    #[test]
    fn long_labels_wrap_and_grow_their_shape() {
        let scene = scene_with(vec![serde_json::json!({
            "op": "add", "type": "rectangle", "id": "a", "width": 120, "height": 40,
            "text": "a fairly long label that cannot fit on one line"
        })]);
        let label = scene.get("a-label").expect("label");
        assert!(text_field(label, "text").expect("text").contains('\n'));
        assert!(bounds(scene.get("a").expect("a")).height > 40.0);
        assert_eq!(kind(label), "text");
    }
}
