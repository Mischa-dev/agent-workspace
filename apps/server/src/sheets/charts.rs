//! Charts on sheets (C1.18): a block of a sheet drawn as bars, a line, a
//! donut or one number. A chart keeps what it reads (`source`: a named
//! range, which moves with its cells, or a block such as Summary!A1:B12)
//! and works its figures out when it is read, so it always shows the
//! cells as they are. The grid shows a sheet's charts, a page embeds one by
//! its id, and agents read one as figures with an SVG picture.

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use crate::{
    dashboards::{Figures, range_figures},
    error::AppError,
    views::definition::ChartStyle,
};

/// The most charts one sheet keeps.
pub const MAX_CHARTS: i64 = 50;

/// A chart as stored.
#[derive(Clone, Debug, FromRow, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Chart {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub title: String,
    /// bar, line, donut or number.
    pub style: String,
    /// What it draws: a named range or a block such as Summary!A1:B12.
    pub source: String,
}

/// A sheet's charts, in order.
pub async fn list(connection: &mut PgConnection, sheet_id: Uuid) -> Result<Vec<Chart>, AppError> {
    Ok(sqlx::query_as::<_, Chart>(
        "SELECT id, title, style, source FROM sheet_charts \
         WHERE sheet_id = $1 ORDER BY position, created_at, id",
    )
    .bind(sheet_id)
    .fetch_all(connection)
    .await?)
}

/// One chart with its sheet, if it is one of a sheet `reader` sees.
pub async fn find(
    connection: &mut PgConnection,
    chart_id: Uuid,
    reader: Uuid,
) -> Result<Option<(Chart, Uuid, String)>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String, String, Uuid, String)>(
        "SELECT chart.id, chart.title, chart.style, chart.source, sheet.id, sheet.name \
         FROM sheet_charts chart JOIN sheets sheet ON sheet.id = chart.sheet_id \
         WHERE chart.id = $1 AND subject_visible_to('sheet', sheet.id, $2)",
    )
    .bind(chart_id)
    .bind(reader)
    .fetch_optional(connection)
    .await?;
    Ok(row.map(|(id, title, style, source, sheet_id, sheet_name)| {
        (
            Chart {
                id,
                title,
                style,
                source,
            },
            sheet_id,
            sheet_name,
        )
    }))
}

/// A chart's figures now; `None` when its source names nothing (a named
/// range removed, a tab gone).
pub async fn figures(
    connection: &mut PgConnection,
    chart: &Chart,
    sheet_id: Uuid,
    sheet_name: String,
) -> Result<Option<Figures>, AppError> {
    range_figures(
        connection,
        sheet_id,
        sheet_name,
        &chart.source,
        &chart.style,
    )
    .await
}

/// The style's stored name.
pub fn style_name(style: ChartStyle) -> &'static str {
    match style {
        ChartStyle::Bar => "bar",
        ChartStyle::Line => "line",
        ChartStyle::Donut => "donut",
        ChartStyle::Number => "number",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A number as a chart labels it: whole when it is, else two places.
fn shown(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value:.2}")
    }
}

const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 280.0;
const PALETTE: [&str; 7] = [
    "#4C6EF5", "#12B886", "#FAB005", "#FA5252", "#7950F2", "#15AABF", "#868E96",
];

/// The chart as an SVG picture, for agents and exports: its title, then
/// the figures as bars, a line, a donut or one number.
pub fn svg(title: &str, style: &str, figures: Option<&Figures>) -> String {
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {WIDTH} {HEIGHT}\" \
         width=\"{WIDTH}\" height=\"{HEIGHT}\" font-family=\"sans-serif\">\
         <rect width=\"100%\" height=\"100%\" fill=\"#FFFFFF\"/>\
         <text x=\"16\" y=\"26\" font-size=\"15\" font-weight=\"600\" fill=\"#212529\">{}</text>",
        escape(title)
    );
    let points = figures
        .map(|figures| {
            figures
                .groups
                .iter()
                .map(|group| (group.name.as_str(), group.value.unwrap_or(0.0)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if figures.is_none() || (points.is_empty() && style != "number") {
        out.push_str(
            "<text x=\"16\" y=\"60\" font-size=\"13\" fill=\"#868E96\">No figures to show.</text></svg>",
        );
        return out;
    }
    let (left, top, right, bottom) = (48.0, 44.0, WIDTH - 16.0, HEIGHT - 40.0);
    match style {
        "number" => {
            let total = figures.and_then(|figures| figures.value).unwrap_or(0.0);
            out.push_str(&format!(
                "<text x=\"{}\" y=\"{}\" font-size=\"48\" text-anchor=\"middle\" fill=\"#212529\">{}</text>",
                WIDTH / 2.0,
                HEIGHT / 2.0 + 16.0,
                escape(&shown(total))
            ));
        }
        "donut" => {
            let total: f64 = points.iter().map(|(_, value)| value.max(0.0)).sum();
            let (cx, cy, radius) = (150.0, 160.0, 90.0);
            let mut angle = -std::f64::consts::FRAC_PI_2;
            for (index, (label, value)) in points.iter().enumerate() {
                let share = if total > 0.0 {
                    value.max(0.0) / total
                } else {
                    0.0
                };
                let colour = PALETTE[index % PALETTE.len()];
                if share >= 0.9999 {
                    out.push_str(&format!(
                        "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{radius}\" fill=\"{colour}\"/>"
                    ));
                } else if share > 0.0 {
                    let end = angle + share * std::f64::consts::TAU;
                    let large = i32::from(share > 0.5);
                    out.push_str(&format!(
                        "<path d=\"M{cx},{cy} L{:.1},{:.1} A{radius},{radius} 0 {large} 1 {:.1},{:.1} Z\" fill=\"{colour}\"/>",
                        cx + radius * angle.cos(),
                        cy + radius * angle.sin(),
                        cx + radius * end.cos(),
                        cy + radius * end.sin(),
                    ));
                    angle = end;
                }
                if index < 10 {
                    let y = 60.0 + 20.0 * index as f64;
                    out.push_str(&format!(
                        "<rect x=\"270\" y=\"{}\" width=\"10\" height=\"10\" fill=\"{colour}\"/>\
                         <text x=\"286\" y=\"{y}\" font-size=\"12\" fill=\"#495057\">{} ({})</text>",
                        y - 9.0,
                        escape(label),
                        shown(*value)
                    ));
                }
            }
            out.push_str(&format!(
                "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{}\" fill=\"#FFFFFF\"/>",
                radius * 0.55
            ));
        }
        _ => {
            let most = points
                .iter()
                .map(|(_, value)| *value)
                .fold(0.0_f64, f64::max);
            let least = points
                .iter()
                .map(|(_, value)| *value)
                .fold(0.0_f64, f64::min);
            let span = if most - least > 0.0 {
                most - least
            } else {
                1.0
            };
            let y_of = |value: f64| bottom - (value - least) / span * (bottom - top);
            out.push_str(&format!(
                "<line x1=\"{left}\" y1=\"{0}\" x2=\"{right}\" y2=\"{0}\" stroke=\"#CED4DA\"/>\
                 <text x=\"{1}\" y=\"{2}\" font-size=\"11\" text-anchor=\"end\" fill=\"#868E96\">{3}</text>\
                 <text x=\"{1}\" y=\"{4}\" font-size=\"11\" text-anchor=\"end\" fill=\"#868E96\">{5}</text>",
                y_of(least.max(0.0).min(most)),
                left - 6.0,
                top + 4.0,
                escape(&shown(most)),
                bottom,
                escape(&shown(least)),
            ));
            let step = (right - left) / points.len() as f64;
            let labelled = points.len() <= 12;
            if style == "line" {
                let path = points
                    .iter()
                    .enumerate()
                    .map(|(index, (_, value))| {
                        format!(
                            "{:.1},{:.1}",
                            left + step * (index as f64 + 0.5),
                            y_of(*value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&format!(
                    "<polyline points=\"{path}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2\"/>",
                    PALETTE[0]
                ));
            } else {
                let zero = y_of(0.0_f64.clamp(least, most));
                for (index, (_, value)) in points.iter().enumerate() {
                    let y = y_of(*value);
                    out.push_str(&format!(
                        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{}\"/>",
                        left + step * index as f64 + step * 0.15,
                        y.min(zero),
                        step * 0.7,
                        (zero - y).abs().max(1.0),
                        PALETTE[0]
                    ));
                }
            }
            if labelled {
                for (index, (label, _)) in points.iter().enumerate() {
                    let short: String = label.chars().take(12).collect();
                    out.push_str(&format!(
                        "<text x=\"{:.1}\" y=\"{}\" font-size=\"11\" text-anchor=\"middle\" fill=\"#495057\">{}</text>",
                        left + step * (index as f64 + 0.5),
                        bottom + 16.0,
                        escape(&short)
                    ));
                }
            }
        }
    }
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboards::FigureGroup;

    fn figures(points: &[(&str, f64)]) -> Figures {
        Figures {
            view_type: "chart".to_owned(),
            view_name: "Budget".to_owned(),
            collection_name: "Plan".to_owned(),
            total: i64::try_from(points.len()).unwrap_or(0),
            value: Some(points.iter().map(|(_, value)| value).sum()),
            style: None,
            measure: None,
            groups: points
                .iter()
                .map(|(name, value)| FigureGroup {
                    key: (*name).to_owned(),
                    name: (*name).to_owned(),
                    color: None,
                    count: 1,
                    value: Some(*value),
                })
                .collect(),
            records: Vec::new(),
            columns: Vec::new(),
            rows: Vec::new(),
            ran_at: None,
            running: false,
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn every_style_draws_and_escapes_its_words() {
        let shown = figures(&[("Rent <office>", 1200.0), ("Food", 300.0)]);
        for style in ["bar", "line", "donut", "number"] {
            let picture = svg("Costs & more", style, Some(&shown));
            assert!(picture.starts_with("<svg"), "{style}");
            assert!(picture.ends_with("</svg>"), "{style}");
            assert!(picture.contains("Costs &amp; more"), "{style}");
            assert!(!picture.contains("<office>"), "{style}");
        }
        assert!(svg("Costs", "bar", Some(&shown)).contains("<rect x="));
        assert!(svg("Costs", "number", Some(&shown)).contains(">1500<"));
        assert!(svg("Costs", "bar", None).contains("No figures"));
    }
}
