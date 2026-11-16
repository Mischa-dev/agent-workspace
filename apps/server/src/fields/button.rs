//! Button fields (T1.21): a button on every record of a collection that runs
//! one to three operations from the operation registry (K2) as whoever
//! presses it, with their arguments filled in from the record.
//!
//! Arguments are a template: any text in them may hold `{{...}}`
//! placeholders, which name a field of the collection (stored by ID as
//! `{{#<id>}}`, so a rename never breaks a button), or one of `record.id`,
//! `record.number`, `record.title` (or the collection's name for its
//! title), `collection.id`, `space.id`, `me` and `today`. A text that is
//! only a placeholder takes the value as it is (a number stays a number, a
//! list a list); inside other text it is written out.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use super::{FieldDefinition, FieldType};

/// A button runs at most this many operations, in order.
pub const MAX_STEPS: usize = 3;
pub const MAX_LABEL_CHARACTERS: usize = 40;
/// One step's arguments, as JSON.
pub const MAX_ARGUMENT_BYTES: usize = 4096;
/// The operation that presses buttons: never a step.
pub const PRESS_OPERATION: &str = "button_press";

/// The names a placeholder may use besides fields.
const BUILT_IN: [&str; 7] = [
    "record.id",
    "record.number",
    "record.title",
    "collection.id",
    "space.id",
    "me",
    "today",
];

/// A button field's settings, in `FieldConfig::button`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonConfig {
    /// What the button says.
    pub label: String,
    pub steps: Vec<ButtonStep>,
}

/// One operation a button runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ButtonStep {
    /// The operation's name, such as `decision_record`.
    pub operation: String,
    /// Its input, with placeholders stored by field ID.
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

/// A button as people and agents write it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonInput {
    /// What the button says (up to 40 characters); the field's name when
    /// left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 1 to 3 operations run in order as whoever presses it.
    pub steps: Vec<ButtonStepInput>,
}

/// One operation of a button, as written.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ButtonStepInput {
    /// An operation's name, such as `decision_record`.
    pub operation: String,
    /// Its input. Text may name the record's fields in double braces,
    /// `{{Stage}}`, or `{{record.id}}`, `{{record.number}}`,
    /// `{{record.title}}`, `{{collection.id}}`, `{{space.id}}`, `{{me}}`,
    /// `{{today}}`.
    #[serde(default)]
    pub arguments: Map<String, Value>,
}

/// A button as a field's view shows it (placeholders as stored).
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ButtonView {
    pub label: String,
    pub steps: Vec<ButtonStepView>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ButtonStepView {
    pub operation: String,
    /// The operation's title, such as "Record a decision".
    pub title: String,
    pub arguments: Map<String, Value>,
}

impl From<&ButtonConfig> for ButtonView {
    fn from(config: &ButtonConfig) -> Self {
        let registry = crate::ops::registry();
        Self {
            label: config.label.clone(),
            steps: config
                .steps
                .iter()
                .map(|step| ButtonStepView {
                    operation: step.operation.clone(),
                    title: registry.get(&step.operation).map_or_else(
                        || step.operation.clone(),
                        |entry| entry.meta.title.to_owned(),
                    ),
                    arguments: step.arguments.clone(),
                })
                .collect(),
        }
    }
}

/// What a button's placeholders are checked against.
pub struct Scope<'a> {
    /// The collection's fields, archived ones too.
    pub fields: &'a [FieldDefinition],
    pub title_name: &'a str,
    /// The button field's name: the label when none is given.
    pub field_name: &'a str,
}

/// The `{{...}}` placeholders in `text`: each one's byte range and what
/// is inside, trimmed.
fn placeholders(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = text[from..].find("{{").map(|at| at + from) {
        let Some(end) = text[start + 2..].find("}}").map(|at| at + start + 2) else {
            break;
        };
        found.push((start..end + 2, text[start + 2..end].trim()));
        from = end + 2;
    }
    found
}

/// Rewrites each placeholder in `text` with `replace`.
fn rewrite(
    text: &str,
    replace: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (range, inside) in placeholders(text) {
        out.push_str(&text[last..range.start]);
        out.push_str("{{");
        out.push_str(&replace(inside)?);
        out.push_str("}}");
        last = range.end;
    }
    out.push_str(&text[last..]);
    Ok(out)
}

/// Every text in `value`, rewritten (keys are left alone).
fn rewrite_value(
    value: &Value,
    replace: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => Value::String(rewrite(text, replace)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| rewrite_value(item, replace))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, item)| Ok((key.clone(), rewrite_value(item, replace)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

impl Scope<'_> {
    /// A placeholder as stored: a built-in name, or a field as `#<id>`.
    fn resolve(&self, inside: &str) -> Result<String, String> {
        let lower = inside.to_lowercase();
        if BUILT_IN.contains(&lower.as_str()) {
            return Ok(lower);
        }
        let field = match inside.strip_prefix('#') {
            Some(id) => id
                .parse::<Uuid>()
                .ok()
                .and_then(|id| self.fields.iter().find(|field| field.id == id)),
            None => super::find(self.fields, inside),
        };
        match field {
            Some(field) if field.archived => Err(format!(
                "\"{}\" is archived. Restore it to fill it into a button.",
                field.name
            )),
            Some(field) if field.field_type == FieldType::Button => Err(format!(
                "\"{}\" is a button, which has no value to fill in.",
                field.name
            )),
            Some(field) => Ok(format!("#{}", field.id)),
            None if inside.eq_ignore_ascii_case(self.title_name)
                || inside.eq_ignore_ascii_case("title") =>
            {
                Ok("record.title".to_owned())
            }
            None => Err(format!(
                "A button can fill in a field ({}{}) or one of {}; there's nothing called \"{}\".",
                self.title_name,
                self.fields
                    .iter()
                    .filter(|field| !field.archived && field.field_type != FieldType::Button)
                    .map(|field| format!(", {}", field.name))
                    .collect::<String>(),
                BUILT_IN.join(", "),
                inside.chars().take(60).collect::<String>(),
            )),
        }
    }
}

/// Checks a button as written and gives its settings, or what to fix.
pub fn compile(input: &ButtonInput, scope: &Scope<'_>) -> Result<ButtonConfig, String> {
    let label = match input.label.as_deref().map(str::trim) {
        Some(label) if !label.is_empty() => label.to_owned(),
        _ => scope
            .field_name
            .chars()
            .take(MAX_LABEL_CHARACTERS)
            .collect(),
    };
    if label.chars().count() > MAX_LABEL_CHARACTERS || label.chars().any(char::is_control) {
        return Err(format!(
            "A button's label is one line of up to {MAX_LABEL_CHARACTERS} characters."
        ));
    }
    if input.steps.is_empty() || input.steps.len() > MAX_STEPS {
        return Err(format!(
            "A button runs 1 to {MAX_STEPS} operations (steps), each with its arguments."
        ));
    }
    let registry = crate::ops::registry();
    let mut steps = Vec::with_capacity(input.steps.len());
    for (index, step) in input.steps.iter().enumerate() {
        let at = format!("Step {}", index + 1);
        let name = step.operation.trim();
        if name == PRESS_OPERATION {
            return Err(format!("{at}: a button can't press buttons."));
        }
        let Some(entry) = registry.get(name) else {
            return Err(format!(
                "{at}: there's no operation called \"{}\". The operations list has their names.",
                name.chars().take(60).collect::<String>()
            ));
        };
        if !entry.meta.kind.writes() {
            return Err(format!(
                "{at}: {} only reads, so a button has nothing to do with it.",
                entry.meta.name
            ));
        }
        let arguments = rewrite_value(&Value::Object(step.arguments.clone()), &mut |inside| {
            scope.resolve(inside)
        })
        .map_err(|problem| format!("{at}: {problem}"))?;
        if serde_json::to_vec(&arguments).map_or(usize::MAX, |bytes| bytes.len())
            > MAX_ARGUMENT_BYTES
        {
            return Err(format!(
                "{at}: its arguments are over {} KB.",
                MAX_ARGUMENT_BYTES / 1024
            ));
        }
        let Value::Object(arguments) = arguments else {
            return Err(format!("{at}: arguments are an object."));
        };
        steps.push(ButtonStep {
            operation: entry.meta.name.to_owned(),
            arguments,
        });
    }
    Ok(ButtonConfig { label, steps })
}

/// The button as people write it: fields by today's names.
pub fn shown(config: &ButtonConfig, fields: &[FieldDefinition]) -> ButtonInput {
    let mut name_of = |inside: &str| -> Result<String, String> {
        Ok(inside
            .strip_prefix('#')
            .and_then(|id| id.parse::<Uuid>().ok())
            .and_then(|id| fields.iter().find(|field| field.id == id))
            .filter(|field| !field.name.contains("}}"))
            .map_or_else(|| inside.to_owned(), |field| field.name.clone()))
    };
    ButtonInput {
        label: Some(config.label.clone()),
        steps: config
            .steps
            .iter()
            .map(|step| ButtonStepInput {
                operation: step.operation.clone(),
                arguments: match rewrite_value(&Value::Object(step.arguments.clone()), &mut name_of)
                {
                    Ok(Value::Object(arguments)) => arguments,
                    _ => step.arguments.clone(),
                },
            })
            .collect(),
    }
}

/// A value as text inside other text.
fn as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(as_text).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

/// A step's input for one press: each placeholder filled by `fill` (which
/// gets the stored name: a built-in one or `#<id>`).
pub fn render(arguments: &Map<String, Value>, fill: &dyn Fn(&str) -> Value) -> Value {
    fn walk(value: &Value, fill: &dyn Fn(&str) -> Value) -> Value {
        match value {
            Value::String(text) => {
                let found = placeholders(text);
                if let [(range, inside)] = found.as_slice()
                    && range.start == 0
                    && range.end == text.len()
                {
                    return fill(inside);
                }
                let mut out = String::with_capacity(text.len());
                let mut last = 0;
                for (range, inside) in found {
                    out.push_str(&text[last..range.start]);
                    out.push_str(&as_text(&fill(inside)));
                    last = range.end;
                }
                out.push_str(&text[last..]);
                Value::String(out)
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| walk(item, fill)).collect())
            }
            Value::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(key, item)| (key.clone(), walk(item, fill)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(&Value::Object(arguments.clone()), fill)
}

/// The fields a stored placeholder names: `#<id>`'s field ID.
pub fn field_of(inside: &str) -> Option<Uuid> {
    inside.strip_prefix('#').and_then(|id| id.parse().ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::fields::{Accessor, FieldConfig};

    fn field(name: &str, field_type: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            name: name.to_owned(),
            description: String::new(),
            field_type,
            config: FieldConfig::default(),
            position: 0,
            archived: false,
            version: 1,
            accessor: Accessor::Stored,
        }
    }

    fn input(operation: &str, arguments: Value) -> ButtonInput {
        ButtonInput {
            label: None,
            steps: vec![ButtonStepInput {
                operation: operation.to_owned(),
                arguments: arguments.as_object().cloned().unwrap_or_default(),
            }],
        }
    }

    #[test]
    fn placeholders_store_fields_by_id_and_show_names() {
        let stage = field("Stage", FieldType::SingleChoice);
        let fields = vec![stage.clone()];
        let scope = Scope {
            fields: &fields,
            title_name: "Order",
            field_name: "Ship it",
        };
        let config = compile(
            &input(
                "decision_record",
                json!({"title": "Shipped {{ Order }} ({{stage}})", "id": "{{record.ID}}"}),
            ),
            &scope,
        )
        .expect("compiles");
        assert_eq!(config.label, "Ship it");
        assert_eq!(
            config.steps[0].arguments["title"],
            json!(format!(
                "Shipped {{{{record.title}}}} ({{{{#{}}}}})",
                stage.id
            ))
        );
        assert_eq!(config.steps[0].arguments["id"], json!("{{record.id}}"));
        let mut renamed = stage.clone();
        renamed.name = "Phase".to_owned();
        let shown = shown(&config, &[renamed]);
        assert_eq!(
            shown.steps[0].arguments["title"],
            json!("Shipped {{record.title}} ({{Phase}})")
        );
    }

    #[test]
    fn mistakes_are_refused_with_what_to_fix() {
        let fields = vec![field("Stage", FieldType::Text)];
        let scope = Scope {
            fields: &fields,
            title_name: "Order",
            field_name: "Go",
        };
        for (button, part) in [
            (
                input("decision_record", json!({"x": "{{Missing}}"})),
                "nothing called",
            ),
            (input("nothing_here", json!({})), "no operation called"),
            (input("button_press", json!({})), "can't press buttons"),
            (input("decision_list", json!({})), "only reads"),
            (ButtonInput::default(), "1 to 3"),
        ] {
            let problem = compile(&button, &scope).expect_err("refused");
            assert!(problem.contains(part), "{problem}");
        }
    }

    #[test]
    fn a_whole_placeholder_keeps_its_value() {
        let arguments = json!({"n": "{{record.number}}", "text": "#{{record.number}} {{tags}}", "list": ["{{tags}}"]});
        let filled = render(arguments.as_object().unwrap(), &|inside| match inside {
            "record.number" => json!(7),
            "tags" => json!(["a", "b"]),
            _ => Value::Null,
        });
        assert_eq!(
            filled,
            json!({"n": 7, "text": "#7 a, b", "list": [["a", "b"]]})
        );
    }
}
