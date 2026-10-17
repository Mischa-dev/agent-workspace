//! Drawing a board without a browser, so agents can look at what they made
//! and the boards list can show previews.
//!
//! This is a clean rendition (no hand-drawn wobble) of Excalidraw's shapes:
//! enough to check layout, labels and connections. PNGs are drawn from the
//! same SVG with resvg, using the Inter font bundled in `assets/fonts`
//! (OFL-1.1), so the output doesn't depend on fonts installed on the host.

use std::{collections::HashMap, fmt::Write as _, sync::OnceLock};

use base64::Engine as _;
use resvg::{tiny_skia, usvg};
use serde_json::Value;

use super::scene::{
    Bounds, Object, bounds, id, is_deleted, kind, number, points, text_field, valid_color,
};

const FONT: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.otf");
const FONT_FAMILY: &str = "Inter";
const PADDING: f64 = 32.0;
/// The largest image side, in pixels, a PNG is drawn at.
pub const MAX_PNG_SIDE: u32 = 4_096;

/// An image placed on the board: its type and bytes.
pub struct File {
    pub mime_type: String,
    pub data: Vec<u8>,
}

pub struct Options {
    /// Draw only these elements (and their labels), cropped to them.
    pub only: Option<Vec<String>>,
    /// Fill the background (white) instead of leaving it transparent.
    pub background: bool,
    pub dark: bool,
}

pub struct Rendered {
    pub svg: String,
    pub width: f64,
    pub height: f64,
    /// The canvas area drawn, in board coordinates.
    pub area: Bounds,
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            character if character.is_control() && character != '\n' && character != '\t' => {}
            character => escaped.push(character),
        }
    }
    escaped
}

fn color(element: &Object, field: &str, fallback: &str, dark: bool) -> String {
    let value = text_field(element, field)
        .filter(|value| valid_color(value))
        .unwrap_or(fallback);
    if dark && (value == "#1e1e1e" || value == "#000000" || value == "black") {
        return "#e3e3e8".to_owned();
    }
    value.to_owned()
}

fn fmt(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if rounded == rounded.trunc() {
        format!("{}", rounded as i64)
    } else {
        format!("{rounded}")
    }
}

fn dash(element: &Object) -> &'static str {
    match text_field(element, "strokeStyle") {
        Some("dashed") => r#" stroke-dasharray="10 8""#,
        Some("dotted") => r#" stroke-dasharray="2 6" stroke-linecap="round""#,
        _ => "",
    }
}

/// The fill for a shape: solid, or a hatch pattern defined in `defs`.
fn fill(element: &Object, defs: &mut String, dark: bool) -> String {
    let background = color(element, "backgroundColor", "transparent", dark);
    if background == "transparent" {
        return "none".to_owned();
    }
    match text_field(element, "fillStyle") {
        Some(style @ ("hachure" | "cross-hatch" | "zigzag")) => {
            let pattern = format!("fill-{}", id(element));
            let _ = write!(
                defs,
                r#"<pattern id="{}" width="10" height="10" patternUnits="userSpaceOnUse" patternTransform="rotate(-45)"><line x1="0" y1="5" x2="10" y2="5" stroke="{}" stroke-width="1.5"/>{}</pattern>"#,
                escape(&pattern),
                escape(&background),
                if style == "cross-hatch" {
                    format!(
                        r#"<line x1="5" y1="0" x2="5" y2="10" stroke="{}" stroke-width="1.5"/>"#,
                        escape(&background)
                    )
                } else {
                    String::new()
                }
            );
            format!("url(#{})", escape(&pattern))
        }
        _ => background,
    }
}

/// A smooth path through points (Catmull-Rom as cubic Béziers), the way
/// Excalidraw draws rounded lines and arrows.
fn smooth_path(points: &[(f64, f64)]) -> String {
    let mut path = format!("M{} {}", fmt(points[0].0), fmt(points[0].1));
    if points.len() == 2 {
        let _ = write!(path, " L{} {}", fmt(points[1].0), fmt(points[1].1));
        return path;
    }
    for index in 0..points.len() - 1 {
        let p0 = points[index.saturating_sub(1)];
        let p1 = points[index];
        let p2 = points[index + 1];
        let p3 = points[(index + 2).min(points.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let _ = write!(
            path,
            " C{} {} {} {} {} {}",
            fmt(c1.0),
            fmt(c1.1),
            fmt(c2.0),
            fmt(c2.1),
            fmt(p2.0),
            fmt(p2.1)
        );
    }
    path
}

fn arrowhead(
    svg: &mut String,
    kind: &str,
    tip: (f64, f64),
    from: (f64, f64),
    stroke: &str,
    width: f64,
) {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < f64::EPSILON {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let size = (12.0 + width * 2.0).min(length * 0.8);
    let back = (tip.0 - ux * size, tip.1 - uy * size);
    let side = (-uy * size * 0.5, ux * size * 0.5);
    let left = (back.0 + side.0, back.1 + side.1);
    let right = (back.0 - side.0, back.1 - side.1);
    let stroke = escape(stroke);
    match kind {
        "triangle" | "triangle_outline" => {
            let fill = if kind == "triangle" {
                stroke.as_str()
            } else {
                "none"
            };
            let _ = write!(
                svg,
                r#"<polygon points="{},{} {},{} {},{}" fill="{fill}" stroke="{stroke}" stroke-width="{}" stroke-linejoin="round"/>"#,
                fmt(tip.0),
                fmt(tip.1),
                fmt(left.0),
                fmt(left.1),
                fmt(right.0),
                fmt(right.1),
                fmt(width)
            );
        }
        "dot" | "circle" | "circle_outline" => {
            let fill = if kind == "circle_outline" {
                "none"
            } else {
                stroke.as_str()
            };
            let _ = write!(
                svg,
                r#"<circle cx="{}" cy="{}" r="{}" fill="{fill}" stroke="{stroke}" stroke-width="{}"/>"#,
                fmt(tip.0),
                fmt(tip.1),
                fmt(size / 3.0),
                fmt(width)
            );
        }
        "bar" => {
            let _ = write!(
                svg,
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{stroke}" stroke-width="{}" stroke-linecap="round"/>"#,
                fmt(tip.0 + side.0),
                fmt(tip.1 + side.1),
                fmt(tip.0 - side.0),
                fmt(tip.1 - side.1),
                fmt(width)
            );
        }
        _ => {
            let _ = write!(
                svg,
                r#"<polyline points="{},{} {},{} {},{}" fill="none" stroke="{stroke}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"/>"#,
                fmt(left.0),
                fmt(left.1),
                fmt(tip.0),
                fmt(tip.1),
                fmt(right.0),
                fmt(right.1),
                fmt(width)
            );
        }
    }
}

fn text(svg: &mut String, element: &Object, dark: bool) {
    let content = text_field(element, "text").unwrap_or_default();
    if content.is_empty() {
        return;
    }
    let size = Some(number(element, "fontSize"))
        .filter(|size| *size > 0.0)
        .unwrap_or(20.0);
    let line_height = Some(number(element, "lineHeight"))
        .filter(|height| *height > 0.0)
        .unwrap_or(1.25)
        * size;
    let b = bounds(element);
    let (anchor, x) = match text_field(element, "textAlign") {
        Some("center") => ("middle", b.x + b.width / 2.0),
        Some("right") => ("end", b.right()),
        _ => ("start", b.x),
    };
    let stroke = color(element, "strokeColor", "#1e1e1e", dark);
    let _ = write!(
        svg,
        r#"<text font-family="{FONT_FAMILY}" font-size="{}" fill="{}" text-anchor="{anchor}">"#,
        fmt(size),
        escape(&stroke)
    );
    for (line_number, line) in content.split('\n').enumerate() {
        // The baseline sits about 80% of the font size into each line.
        let baseline =
            b.y + line_number as f64 * line_height + (line_height - size) / 2.0 + size * 0.8;
        let _ = write!(
            svg,
            r#"<tspan x="{}" y="{}">{}</tspan>"#,
            fmt(x),
            fmt(baseline),
            escape(line)
        );
    }
    svg.push_str("</text>");
}

fn element_svg(
    svg: &mut String,
    defs: &mut String,
    element: &Object,
    files: &HashMap<String, File>,
    dark: bool,
) {
    let b = bounds(element);
    let stroke = color(element, "strokeColor", "#1e1e1e", dark);
    let stroke_width = Some(number(element, "strokeWidth"))
        .filter(|w| *w > 0.0)
        .unwrap_or(2.0);
    let opacity = Some(number(element, "opacity"))
        .filter(|o| (0.0..=100.0).contains(o))
        .unwrap_or(100.0)
        / 100.0;
    let angle = number(element, "angle");
    let (cx, cy) = b.center();
    let mut open = format!(r#"<g opacity="{}""#, fmt(opacity));
    if angle.abs() > 1e-9 {
        let _ = write!(
            open,
            r#" transform="rotate({} {} {})""#,
            fmt(angle.to_degrees()),
            fmt(cx),
            fmt(cy)
        );
    }
    open.push('>');
    svg.push_str(&open);
    let common = format!(
        r#" stroke="{}" stroke-width="{}"{}"#,
        escape(&stroke),
        fmt(stroke_width),
        dash(element)
    );
    match kind(element) {
        "rectangle" | "embeddable" => {
            let radius = if element
                .get("roundness")
                .is_some_and(|value| !value.is_null())
            {
                (b.width.min(b.height) * 0.25).min(32.0)
            } else {
                0.0
            };
            let _ = write!(
                svg,
                r#"<rect x="{}" y="{}" width="{}" height="{}" rx="{}" fill="{}"{common}/>"#,
                fmt(b.x),
                fmt(b.y),
                fmt(b.width),
                fmt(b.height),
                fmt(radius),
                fill(element, defs, dark)
            );
            if kind(element) == "embeddable"
                && let Some(link) = text_field(element, "link")
            {
                let mut label = serde_json::Map::new();
                label.insert("text".to_owned(), Value::String(link.to_owned()));
                label.insert("x".to_owned(), (b.x + 8.0).into());
                label.insert("y".to_owned(), (b.y + 8.0).into());
                label.insert("width".to_owned(), (b.width - 16.0).into());
                label.insert("height".to_owned(), 20.0.into());
                label.insert("fontSize".to_owned(), 14.into());
                label.insert("strokeColor".to_owned(), "#1971c2".into());
                text(svg, &label, dark);
            }
        }
        "ellipse" => {
            let _ = write!(
                svg,
                r#"<ellipse cx="{}" cy="{}" rx="{}" ry="{}" fill="{}"{common}/>"#,
                fmt(cx),
                fmt(cy),
                fmt(b.width / 2.0),
                fmt(b.height / 2.0),
                fill(element, defs, dark)
            );
        }
        "diamond" => {
            let _ = write!(
                svg,
                r#"<polygon points="{},{} {},{} {},{} {},{}" fill="{}" stroke-linejoin="round"{common}/>"#,
                fmt(cx),
                fmt(b.y),
                fmt(b.right()),
                fmt(cy),
                fmt(cx),
                fmt(b.bottom()),
                fmt(b.x),
                fmt(cy),
                fill(element, defs, dark)
            );
        }
        "frame" | "magicframe" => {
            let _ = write!(
                svg,
                r##"<rect x="{}" y="{}" width="{}" height="{}" rx="8" fill="none" stroke="#999999" stroke-width="1"/>"##,
                fmt(b.x),
                fmt(b.y),
                fmt(b.width),
                fmt(b.height)
            );
            let name = text_field(element, "name").unwrap_or("Frame");
            let _ = write!(
                svg,
                r##"<text font-family="{FONT_FAMILY}" font-size="14" fill="#888888" x="{}" y="{}">{}</text>"##,
                fmt(b.x),
                fmt(b.y - 6.0),
                escape(name)
            );
        }
        "line" | "arrow" | "freedraw" => {
            let x = number(element, "x");
            let y = number(element, "y");
            let absolute = points(element)
                .into_iter()
                .map(|(px, py)| (x + px, y + py))
                .collect::<Vec<_>>();
            if absolute.len() >= 2 {
                let element_kind = kind(element);
                let rounded = element_kind == "freedraw"
                    || element
                        .get("roundness")
                        .is_some_and(|value| !value.is_null());
                let path = if rounded && element_kind != "freedraw" {
                    smooth_path(&absolute)
                } else {
                    let mut path = format!("M{} {}", fmt(absolute[0].0), fmt(absolute[0].1));
                    for point in &absolute[1..] {
                        let _ = write!(path, " L{} {}", fmt(point.0), fmt(point.1));
                    }
                    path
                };
                let closed = element_kind == "line"
                    && absolute.first() == absolute.last()
                    && absolute.len() > 2;
                let width = if element_kind == "freedraw" {
                    stroke_width * 1.5
                } else {
                    stroke_width
                };
                let _ = write!(
                    svg,
                    r#"<path d="{path}{}" fill="{}" stroke="{}" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round"{}/>"#,
                    if closed { " Z" } else { "" },
                    if closed {
                        fill(element, defs, dark)
                    } else {
                        "none".to_owned()
                    },
                    escape(&stroke),
                    fmt(width),
                    dash(element)
                );
                if element_kind == "arrow" {
                    if let Some(head) = text_field(element, "endArrowhead") {
                        let n = absolute.len();
                        arrowhead(
                            svg,
                            head,
                            absolute[n - 1],
                            absolute[n - 2],
                            &stroke,
                            stroke_width,
                        );
                    }
                    if let Some(head) = text_field(element, "startArrowhead") {
                        arrowhead(svg, head, absolute[0], absolute[1], &stroke, stroke_width);
                    }
                }
            }
        }
        "text" => text(svg, element, dark),
        "image" => {
            let file = text_field(element, "fileId").and_then(|file| files.get(file));
            match file {
                Some(file) => {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&file.data);
                    let _ = write!(
                        svg,
                        r#"<image x="{}" y="{}" width="{}" height="{}" preserveAspectRatio="none" href="data:{};base64,{encoded}"/>"#,
                        fmt(b.x),
                        fmt(b.y),
                        fmt(b.width),
                        fmt(b.height),
                        escape(&file.mime_type)
                    );
                }
                None => {
                    let _ = write!(
                        svg,
                        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#f1f3f5" stroke="#adb5bd" stroke-width="1"/>"##,
                        fmt(b.x),
                        fmt(b.y),
                        fmt(b.width),
                        fmt(b.height)
                    );
                }
            }
        }
        _ => {}
    }
    svg.push_str("</g>");
}

/// Draws the live elements (or `only` those and their labels) as SVG.
pub fn svg(elements: &[Object], files: &HashMap<String, File>, options: &Options) -> Rendered {
    let wanted = options.only.as_ref().map(|only| {
        let mut set = only
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        for element in elements {
            if let Some(container) = text_field(element, "containerId")
                && set.contains(container)
            {
                set.insert(id(element).to_owned());
            }
            if let Some(frame) = text_field(element, "frameId")
                && set.contains(frame)
            {
                set.insert(id(element).to_owned());
            }
        }
        set
    });
    let drawn = elements
        .iter()
        .filter(|element| !is_deleted(element))
        .filter(|element| wanted.as_ref().is_none_or(|set| set.contains(id(element))))
        .collect::<Vec<_>>();
    let content = Bounds::union(drawn.iter().map(|element| {
        let b = bounds(element);
        if matches!(kind(element), "frame" | "magicframe") {
            Bounds {
                y: b.y - 24.0,
                height: b.height + 24.0,
                ..b
            }
        } else {
            b
        }
    }))
    .unwrap_or(Bounds {
        x: 0.0,
        y: 0.0,
        width: 400.0,
        height: 300.0,
    });
    let area = Bounds {
        x: content.x - PADDING,
        y: content.y - PADDING,
        width: content.width + PADDING * 2.0,
        height: content.height + PADDING * 2.0,
    };
    let mut body = String::new();
    let mut defs = String::new();
    for element in &drawn {
        element_svg(&mut body, &mut defs, element, files, options.dark);
    }
    let background = if options.background {
        format!(
            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}"/>"#,
            fmt(area.x),
            fmt(area.y),
            fmt(area.width),
            fmt(area.height),
            if options.dark { "#121212" } else { "#ffffff" }
        )
    } else {
        String::new()
    };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{}" height="{}" viewBox="{} {} {} {}"><defs>{defs}</defs>{background}{body}</svg>"#,
        fmt(area.width),
        fmt(area.height),
        fmt(area.x),
        fmt(area.y),
        fmt(area.width),
        fmt(area.height)
    );
    Rendered {
        svg,
        width: area.width,
        height: area.height,
        area,
    }
}

fn options() -> &'static usvg::Options<'static> {
    static OPTIONS: OnceLock<usvg::Options<'static>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = usvg::Options {
            font_family: FONT_FAMILY.to_owned(),
            ..usvg::Options::default()
        };
        options.fontdb_mut().load_font_data(FONT.to_vec());
        options
    })
}

/// Draws SVG as a PNG, scaled by `scale` but never wider or taller than
/// `MAX_PNG_SIDE`. Returns the PNG and its pixel size.
pub fn png(svg: &str, scale: f64) -> Result<(Vec<u8>, u32, u32), &'static str> {
    let tree = usvg::Tree::from_str(svg, options()).map_err(|_| "the board couldn't be drawn")?;
    let size = tree.size();
    let largest = f64::from(size.width().max(size.height())).max(1.0);
    let scale = scale.clamp(0.1, 4.0).min(f64::from(MAX_PNG_SIDE) / largest);
    let width = ((f64::from(size.width()) * scale).ceil() as u32).max(1);
    let height = ((f64::from(size.height()) * scale).ceil() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("the board is too big to draw")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale as f32, scale as f32),
        &mut pixmap.as_mut(),
    );
    let png = pixmap
        .encode_png()
        .map_err(|_| "the board couldn't be encoded")?;
    Ok((png, width, height))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{Options, png, svg};
    use crate::whiteboards::scene::{Operation, Scene};

    fn options() -> Options {
        Options {
            only: None,
            background: true,
            dark: false,
        }
    }

    #[test]
    fn draws_shapes_labels_and_arrows_and_escapes_text() {
        let mut scene = Scene::new(Vec::new(), 1);
        let operations: Vec<Operation> = serde_json::from_value(json!([
            {"op": "add", "type": "rectangle", "id": "a", "width": 600, "text": "<script>alert(1)</script> & co"},
            {"op": "add", "type": "diamond", "id": "b", "x": 800, "backgroundColor": "#ffc9c9", "fillStyle": "hachure"},
            {"op": "connect", "from": "a", "to": "b"},
            {"op": "add", "type": "frame", "id": "f", "x": -100, "y": -100, "name": "Plan \" quote"},
        ]))
        .expect("operations");
        scene.apply(&operations).expect("applies");
        let rendered = svg(scene.elements(), &HashMap::new(), &options());
        assert!(
            rendered
                .svg
                .contains("&lt;script&gt;alert(1)&lt;/script&gt; &amp; co")
        );
        assert!(!rendered.svg.contains("<script"));
        assert!(rendered.svg.contains("<polygon"));
        assert!(rendered.svg.contains("<path"));
        assert!(rendered.svg.contains("Plan &quot; quote"));
        let (bytes, width, height) = png(&rendered.svg, 1.0).expect("png");
        assert_eq!(&bytes[1..4], b"PNG");
        assert!(width > 300 && height > 80, "{width}x{height}");
    }

    #[test]
    fn huge_boards_are_scaled_down_to_fit() {
        let mut scene = Scene::new(Vec::new(), 1);
        let operations: Vec<Operation> = serde_json::from_value(json!([
            {"op": "add", "type": "rectangle", "id": "a", "x": 0, "y": 0},
            {"op": "add", "type": "rectangle", "id": "b", "x": 20000, "y": 0},
        ]))
        .expect("operations");
        scene.apply(&operations).expect("applies");
        let rendered = svg(scene.elements(), &HashMap::new(), &options());
        let (_, width, _) = png(&rendered.svg, 2.0).expect("png");
        assert!(width <= super::MAX_PNG_SIDE);
    }
}
