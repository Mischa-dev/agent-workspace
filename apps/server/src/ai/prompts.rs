//! The prompts the server's short AI jobs use. Each is a versioned pair of
//! files in `prompts/`: `<id>.v<n>.md` (the instructions) and
//! `<id>.v<n>.schema.json` (the reply's JSON Schema), plus a fixture in
//! `prompts/fixtures/` with an example input and a reply that fits.
//!
//! To change a prompt, add the next version's files and point the constant
//! at them; usage rows and suggestions record which version made them.

use std::sync::OnceLock;

use serde_json::Value;

/// One prompt, as the gateway sends it.
pub struct Prompt {
    pub id: &'static str,
    pub version: i32,
    pub system: &'static str,
    schema_text: &'static str,
    /// The most output tokens a reply may use.
    pub max_output_tokens: u32,
    schema: OnceLock<Value>,
    #[cfg_attr(not(test), allow(dead_code))]
    fixture_text: &'static str,
}

impl Prompt {
    const fn new(
        id: &'static str,
        version: i32,
        system: &'static str,
        schema_text: &'static str,
        fixture_text: &'static str,
        max_output_tokens: u32,
    ) -> Self {
        Self {
            id,
            version,
            system,
            schema_text,
            max_output_tokens,
            schema: OnceLock::new(),
            fixture_text,
        }
    }

    pub fn schema(&self) -> &Value {
        self.schema
            .get_or_init(|| serde_json::from_str(self.schema_text).unwrap_or(Value::Null))
    }

    #[cfg(test)]
    pub fn fixture(&self) -> Value {
        serde_json::from_str(self.fixture_text).unwrap_or(Value::Null)
    }
}

pub static PING: Prompt = Prompt::new(
    "ping",
    1,
    include_str!("prompts/ping.v1.md"),
    include_str!("prompts/ping.v1.schema.json"),
    include_str!("prompts/fixtures/ping.v1.json"),
    64,
);

pub static DEPENDENCY_JUDGE: Prompt = Prompt::new(
    "dependency_judge",
    1,
    include_str!("prompts/dependency_judge.v1.md"),
    include_str!("prompts/dependency_judge.v1.schema.json"),
    include_str!("prompts/fixtures/dependency_judge.v1.json"),
    8_000,
);

pub static CONVERSATION_BLOCKERS: Prompt = Prompt::new(
    "conversation_blockers",
    1,
    include_str!("prompts/conversation_blockers.v1.md"),
    include_str!("prompts/conversation_blockers.v1.schema.json"),
    include_str!("prompts/fixtures/conversation_blockers.v1.json"),
    6_000,
);

pub static TRIAGE: Prompt = Prompt::new(
    "triage",
    1,
    include_str!("prompts/triage.v1.md"),
    include_str!("prompts/triage.v1.schema.json"),
    include_str!("prompts/fixtures/triage.v1.json"),
    6_000,
);

pub static PLAN_TO_TASKS: Prompt = Prompt::new(
    "plan_to_tasks",
    1,
    include_str!("prompts/plan_to_tasks.v1.md"),
    include_str!("prompts/plan_to_tasks.v1.schema.json"),
    include_str!("prompts/fixtures/plan_to_tasks.v1.json"),
    32_000,
);

pub static CRITERIA: Prompt = Prompt::new(
    "criteria",
    1,
    include_str!("prompts/criteria.v1.md"),
    include_str!("prompts/criteria.v1.schema.json"),
    include_str!("prompts/fixtures/criteria.v1.json"),
    1_500,
);

pub static SUMMARY: Prompt = Prompt::new(
    "summary",
    1,
    include_str!("prompts/summary.v1.md"),
    include_str!("prompts/summary.v1.schema.json"),
    include_str!("prompts/fixtures/summary.v1.json"),
    1_500,
);

/// Every prompt, for the tests and the fake provider.
pub fn all() -> [&'static Prompt; 7] {
    [
        &PING,
        &DEPENDENCY_JUDGE,
        &CONVERSATION_BLOCKERS,
        &TRIAGE,
        &PLAN_TO_TASKS,
        &CRITERIA,
        &SUMMARY,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::schema;

    #[test]
    fn every_prompt_has_a_schema_the_gateway_understands_and_a_fixture_that_fits() {
        let mut ids = std::collections::HashSet::new();
        for prompt in all() {
            assert!(ids.insert(prompt.id), "{} twice", prompt.id);
            assert!(!prompt.system.trim().is_empty(), "{}", prompt.id);
            let schema = prompt.schema();
            assert!(schema.is_object(), "{} schema doesn't parse", prompt.id);
            assert_eq!(
                schema::unknown_keywords(schema),
                Vec::<String>::new(),
                "{}",
                prompt.id
            );
            assert_strict(schema, prompt.id);
            let fixture = prompt.fixture();
            assert!(fixture["input"].is_object(), "{} fixture input", prompt.id);
            assert_eq!(
                schema::validate(schema, &fixture["output"]),
                None,
                "{} fixture output",
                prompt.id
            );
        }
    }

    /// Strict structured output (some providers require it) needs every
    /// object to list all its properties as required and allow no others.
    fn assert_strict(schema: &Value, id: &str) {
        if schema["type"] == "object" {
            assert_eq!(
                schema["additionalProperties"],
                Value::Bool(false),
                "{id}: objects must set additionalProperties: false"
            );
            let properties = schema["properties"].as_object().expect("properties");
            let required = schema["required"].as_array().expect("required");
            assert_eq!(
                properties.len(),
                required.len(),
                "{id}: every property must be required"
            );
            for property in properties.values() {
                assert_strict(property, id);
            }
        }
        if let Some(items) = schema.get("items") {
            assert_strict(items, id);
        }
    }
}
