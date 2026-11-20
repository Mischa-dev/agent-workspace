//! Rollup fields (T1.18): a value worked out from the records a relation
//! field names, such as the sum of their Hours or the latest of their Due
//! dates. Results are kept in `collection_records.computed_values` as a
//! formula's are, so views filter, sort and group by them; they're worked
//! out with the record (`collections::computed`) and again, in a job, when
//! a related record changes.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{FieldDefinition, FieldType, formula::ResultType};

/// The most related records a rollup reads; past it the value is an error.
pub const MAX_ROLLUP_RECORDS: usize = 1_000;

/// How a rollup puts the related records' values together.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    /// How many related records there are.
    Count,
    /// How many have a value.
    CountValues,
    /// How many different values they have.
    CountUnique,
    Sum,
    Average,
    Min,
    Max,
    /// The earliest date.
    Earliest,
    /// The latest date.
    Latest,
    /// The share that are ticked, 0 to 100.
    PercentChecked,
    /// The share with no value, 0 to 100.
    PercentEmpty,
    /// Every value, as a list (a lookup).
    List,
}

impl Aggregate {
    pub const ALL: [Self; 12] = [
        Self::Count,
        Self::CountValues,
        Self::CountUnique,
        Self::Sum,
        Self::Average,
        Self::Min,
        Self::Max,
        Self::Earliest,
        Self::Latest,
        Self::PercentChecked,
        Self::PercentEmpty,
        Self::List,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::CountValues => "count_values",
            Self::CountUnique => "count_unique",
            Self::Sum => "sum",
            Self::Average => "average",
            Self::Min => "min",
            Self::Max => "max",
            Self::Earliest => "earliest",
            Self::Latest => "latest",
            Self::PercentChecked => "percent_checked",
            Self::PercentEmpty => "percent_empty",
            Self::List => "list",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let squashed = value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-', '_'], "");
        Some(match squashed.as_str() {
            "count" | "countall" | "records" => Self::Count,
            "countvalues" | "filled" => Self::CountValues,
            "countunique" | "unique" | "distinct" => Self::CountUnique,
            "sum" | "total" => Self::Sum,
            "average" | "avg" | "mean" => Self::Average,
            "min" | "minimum" | "lowest" => Self::Min,
            "max" | "maximum" | "highest" => Self::Max,
            "earliest" | "first" => Self::Earliest,
            "latest" | "last" => Self::Latest,
            "percentchecked" | "checked" => Self::PercentChecked,
            "percentempty" | "empty" => Self::PercentEmpty,
            "list" | "lookup" | "show" | "values" => Self::List,
            _ => return None,
        })
    }

    /// Whether it reads no field: it counts the records themselves.
    fn reads_records(self) -> bool {
        self == Self::Count
    }
}

/// A rollup field's settings, in `FieldConfig::rollup`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollupConfig {
    /// A relation field of the same collection: whose records it reads.
    pub relation: Uuid,
    /// A field of the related collection; None counts the records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Uuid>,
    pub aggregate: Aggregate,
    /// What its value is, from the aggregate and the field it reads.
    pub result: ResultType,
}

/// A rollup as someone writes it: fields by name or ID.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollupInput {
    /// The relation field (on this collection) whose records it reads.
    pub relation: String,
    /// The field of the related records it reads. Leave it out to count
    /// the records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// count, count_values, count_unique, sum, average, min, max, earliest,
    /// latest, percent_checked, percent_empty or list.
    pub aggregate: String,
}

/// A rollup as a field's view shows it.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RollupView {
    /// The relation field's ID.
    pub relation: String,
    /// The related collection's field it reads; absent when it counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub aggregate: Aggregate,
    /// number, text, boolean, date or list: how it filters and sorts.
    pub result: ResultType,
}

impl From<&RollupConfig> for RollupView {
    fn from(config: &RollupConfig) -> Self {
        Self {
            relation: config.relation.to_string(),
            target: config.target.map(|id| id.to_string()),
            aggregate: config.aggregate,
            result: config.result,
        }
    }
}

/// What a rollup is checked against: its own collection's fields (for the
/// relation) and the related collection's (for the field it reads).
pub struct Scope<'a> {
    pub fields: &'a [FieldDefinition],
    pub related: &'a [FieldDefinition],
}

/// The relation a rollup names among `fields`, by name or ID.
pub fn find_relation<'a>(
    fields: &'a [FieldDefinition],
    reference: &str,
) -> Result<&'a FieldDefinition, String> {
    let relations = || {
        fields
            .iter()
            .filter(|field| field.field_type == FieldType::Relation && !field.archived)
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match super::find(fields, reference) {
        Some(field) if field.field_type == FieldType::Relation && !field.archived => Ok(field),
        Some(field) => Err(format!(
            "\"{}\" isn't a relation. A rollup reads the records a relation field names{}.",
            field.name,
            match relations() {
                names if names.is_empty() => String::new(),
                names => format!(": {names}"),
            }
        )),
        None => Err(match relations() {
            names if names.is_empty() => {
                "Add a relation field first: a rollup reads the records it names.".to_owned()
            }
            names => format!("Name the relation it reads: {names}."),
        }),
    }
}

/// What a field's values read as inside a rollup, or why it can't be read.
fn target_kind(field: &FieldDefinition) -> Result<ResultType, String> {
    Ok(match field.behaves_as() {
        FieldType::Number => ResultType::Number,
        FieldType::Date => ResultType::Date,
        FieldType::Checkbox => ResultType::Boolean,
        FieldType::MultipleChoice | FieldType::People | FieldType::Labels | FieldType::Relation => {
            ResultType::List
        }
        FieldType::Button => {
            return Err(format!(
                "\"{}\" is a button, which has no value to roll up.",
                field.name
            ));
        }
        _ => match (
            field.field_type,
            &field.config.formula,
            &field.config.rollup,
        ) {
            (FieldType::Formula, Some(formula), _) => formula.result,
            (FieldType::Rollup, _, Some(rollup)) => rollup.result,
            _ => ResultType::Text,
        },
    })
}

/// Checks a rollup as written and gives its settings, or what to fix.
pub fn compile(input: &RollupInput, scope: &Scope<'_>) -> Result<RollupConfig, String> {
    let relation = find_relation(scope.fields, &input.relation)?;
    let aggregate = Aggregate::parse(&input.aggregate).ok_or_else(|| {
        format!(
            "\"{}\" isn't a way to roll up. Use one of {}.",
            crate::work_item_labels::echo(&input.aggregate),
            Aggregate::ALL
                .iter()
                .map(|aggregate| aggregate.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let target = match input.field.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(reference) => {
            let Some(field) = super::find(scope.related, reference).filter(|field| !field.archived)
            else {
                return Err(format!(
                    "The related records have no field called \"{}\". Their fields: {}.",
                    crate::work_item_labels::echo(reference),
                    super::active_names(scope.related)
                ));
            };
            Some(field)
        }
    };
    if target.is_none() && !aggregate.reads_records() {
        return Err(format!(
            "Name the field of the related records to {} (only count needs none).",
            aggregate.as_str().replace('_', " ")
        ));
    }
    let kind = target.map(target_kind).transpose()?;
    let result = match aggregate {
        Aggregate::Count
        | Aggregate::CountValues
        | Aggregate::CountUnique
        | Aggregate::PercentEmpty => ResultType::Number,
        Aggregate::Sum | Aggregate::Average | Aggregate::Min | Aggregate::Max => {
            if kind != Some(ResultType::Number) {
                return Err(format!(
                    "{} adds up numbers: pick a number field of the related records.",
                    aggregate.as_str()
                ));
            }
            ResultType::Number
        }
        Aggregate::Earliest | Aggregate::Latest => {
            if kind != Some(ResultType::Date) {
                return Err(format!(
                    "{} reads dates: pick a date field of the related records.",
                    aggregate.as_str()
                ));
            }
            ResultType::Date
        }
        Aggregate::PercentChecked => {
            if kind != Some(ResultType::Boolean) {
                return Err(
                    "percent_checked reads a checkbox: pick one of the related records'."
                        .to_owned(),
                );
            }
            ResultType::Number
        }
        Aggregate::List => ResultType::List,
    };
    Ok(RollupConfig {
        relation: relation.id,
        target: target.map(|field| field.id),
        aggregate,
        result,
    })
}

#[cfg(test)]
mod tests {
    use super::Aggregate;

    #[test]
    fn aggregates_read_back_as_written() {
        for aggregate in Aggregate::ALL {
            assert_eq!(Aggregate::parse(aggregate.as_str()), Some(aggregate));
        }
        assert_eq!(Aggregate::parse("Avg"), Some(Aggregate::Average));
        assert_eq!(Aggregate::parse("lookup"), Some(Aggregate::List));
        assert_eq!(Aggregate::parse("median"), None);
    }
}
