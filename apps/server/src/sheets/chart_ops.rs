//! Chart operations (C1.18): `sheet_chart_set` adds, changes or removes a
//! sheet's chart, and `sheet_chart_get` reads one as figures with an SVG
//! picture, worked out from the cells as they are now.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::charts::{self, Chart, MAX_CHARTS};
use crate::{
    events::{self, NewEvent, catalog},
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
    views::definition::ChartStyle,
};

const SHEET: &str = "That sheet";
const CHART: &str = "That chart";

fn db(error: sqlx::Error) -> OpError {
    OpError::logged("sheet_chart", &error.into())
}

/// Add, change or remove a chart.
pub struct SheetChartSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetChartSetInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// The chart to change or remove; leave out to add one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chart_id: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// bar (the default), line, donut or number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ChartStyle>,
    /// What it draws: a named range, or a block such as Summary!A1:B12
    /// (labels in its first column, numbers in its second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove: bool,
}

impl Targeted for SheetChartSetInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetChartsOutput {
    /// The chart added or changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub chart_id: Option<Uuid>,
    /// The sheet's charts now, in order.
    pub charts: Vec<Chart>,
}

fn title_of(title: &str) -> Result<String, OpError> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > 120 {
        return Err(OpError::invalid(
            "title",
            "A chart's title is 1 to 120 characters.",
        ));
    }
    Ok(title)
}

impl Operation for SheetChartSet {
    const NAME: &'static str = "sheet_chart_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = Some(Perm::SheetsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Chart a sheet range";
    const DESCRIPTION: &'static str = "Add a chart of a block of a sheet (a named range keeps \
        it in place as rows move), or change or remove one. It shows the cells as they are; \
        a page embeds it.";
    const EMITS: &'static [&'static str] = &["sheet.charts_changed"];

    type Input = SheetChartSetInput;
    type Output = SheetChartsOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetChartSetInput,
    ) -> Result<SheetChartsOutput, OpError> {
        let sheet_id = input.sheet_id.id();
        let actor = call.account_id();
        let tx = call.tx().await?;
        // The sheet's row orders writes to its charts.
        let found = sqlx::query_scalar::<_, Uuid>("SELECT id FROM sheets WHERE id = $1 FOR UPDATE")
            .bind(sheet_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(db)?;
        if found.is_none() {
            return Err(OpError::not_found(SHEET));
        }
        let source = match input.source.as_deref().map(str::trim) {
            Some(source) => {
                if source.is_empty()
                    || source.chars().count() > 200
                    || super::ranges::resolve(&mut **tx, sheet_id, source)
                        .await?
                        .is_none()
                {
                    return Err(OpError::invalid(
                        "source",
                        "Name a named range of the sheet or a block such as Summary!A1:B12.",
                    ));
                }
                Some(source.to_owned())
            }
            None => None,
        };
        let title = input.title.as_deref().map(title_of).transpose()?;
        let style = input.style.map(charts::style_name);
        let chart_id = match (input.chart_id, input.remove) {
            (Some(chart), true) => {
                sqlx::query("DELETE FROM sheet_charts WHERE id = $1 AND sheet_id = $2")
                    .bind(chart.id())
                    .bind(sheet_id)
                    .execute(&mut **tx)
                    .await
                    .map_err(db)?
                    .rows_affected()
                    .eq(&1)
                    .then_some(())
                    .ok_or(OpError::not_found(CHART))?;
                None
            }
            (None, true) => {
                return Err(OpError::invalid("chartId", "Say which chart to remove."));
            }
            (Some(chart), false) => Some(
                sqlx::query_scalar::<_, Uuid>(
                    "UPDATE sheet_charts SET title = COALESCE($3, title), \
                       style = COALESCE($4, style), source = COALESCE($5, source), \
                       updated_at = now() \
                     WHERE id = $1 AND sheet_id = $2 RETURNING id",
                )
                .bind(chart.id())
                .bind(sheet_id)
                .bind(&title)
                .bind(style)
                .bind(&source)
                .fetch_optional(&mut **tx)
                .await
                .map_err(db)?
                .ok_or(OpError::not_found(CHART))?,
            ),
            (None, false) => {
                let source = source.ok_or(OpError::invalid(
                    "source",
                    "Say what the chart draws: a named range or a block such as A1:B12.",
                ))?;
                let count = sqlx::query_scalar::<_, i64>(
                    "SELECT count(*) FROM sheet_charts WHERE sheet_id = $1",
                )
                .bind(sheet_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(db)?;
                if count >= MAX_CHARTS {
                    return Err(OpError::invalid(
                        "sheetId",
                        "A sheet keeps up to 50 charts. Remove one first.",
                    ));
                }
                Some(
                    sqlx::query_scalar::<_, Uuid>(
                        "INSERT INTO sheet_charts (id, workspace_id, space_id, sheet_id, title, \
                           style, source, position, created_by_actor_id) \
                         SELECT $1, project.organization_id, sheet.project_id, sheet.id, $3, $4, \
                                $5, $6, $7 \
                         FROM sheets sheet JOIN projects project ON project.id = sheet.project_id \
                         WHERE sheet.id = $2 RETURNING id",
                    )
                    .bind(Uuid::now_v7())
                    .bind(sheet_id)
                    .bind(title.unwrap_or_else(|| source.clone()))
                    .bind(style.unwrap_or("bar"))
                    .bind(&source)
                    .bind(i32::try_from(count).unwrap_or(0))
                    .bind(actor)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(db)?,
                )
            }
        };
        let charts = charts::list(&mut **tx, sheet_id).await?;
        let space = call.ctx.space_id.ok_or(OpError::Unavailable)?;
        let workspace = call.ctx.workspace_id;
        let event = NewEvent::thing(&catalog::SHEET_CHARTS_CHANGED, space, "sheet", sheet_id)
            .payload(json!({"sheetId": sheet_id}))
            .actor(actor)
            .workspace(workspace);
        let tx = call.tx().await?;
        events::emit(tx, event).await?;
        call.audit_target(
            "sheet",
            sheet_id,
            json!({"chart": chart_id, "removed": input.remove}),
        );
        Ok(SheetChartsOutput { chart_id, charts })
    }
}

/// Read a chart.
pub struct SheetChartGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetChartGetInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    pub chart_id: IdRef,
    /// Leave out the SVG picture.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub without_picture: bool,
}

impl Targeted for SheetChartGetInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChartPoint {
    pub label: String,
    pub value: f64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetChartOutput {
    #[serde(flatten)]
    pub chart: Chart,
    /// What its numbers are, from the header row (such as "sum of Cost").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure: Option<String>,
    /// Its labels and numbers in order; empty when the source names nothing.
    pub points: Vec<ChartPoint>,
    /// Every number added up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// False when its source names nothing now (a named range removed).
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub svg: Option<String>,
}

impl Operation for SheetChartGet {
    const NAME: &'static str = "sheet_chart_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Read a sheet chart";
    const DESCRIPTION: &'static str = "A sheet's chart as its labels and numbers now, with an \
        SVG picture of it.";

    type Input = SheetChartGetInput;
    type Output = SheetChartOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: SheetChartGetInput,
    ) -> Result<SheetChartOutput, OpError> {
        let reader = call.account_id();
        let tx = call.tx().await?;
        let (chart, sheet_id, sheet_name) = charts::find(&mut **tx, input.chart_id.id(), reader)
            .await?
            .filter(|(_, sheet, _)| *sheet == input.sheet_id.id())
            .ok_or(OpError::not_found(CHART))?;
        let figures = charts::figures(&mut **tx, &chart, sheet_id, sheet_name).await?;
        let svg = (!input.without_picture)
            .then(|| charts::svg(&chart.title, &chart.style, figures.as_ref()));
        Ok(SheetChartOutput {
            found: figures.is_some(),
            measure: figures.as_ref().and_then(|figures| figures.measure.clone()),
            total: figures.as_ref().and_then(|figures| figures.value),
            points: figures
                .map(|figures| {
                    figures
                        .groups
                        .into_iter()
                        .map(|group| ChartPoint {
                            label: group.name,
                            value: group.value.unwrap_or(0.0),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            chart,
            svg,
        })
    }
}
