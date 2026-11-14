//! The conventions every operation keeps (K2 sections 3.6 and 6), checked
//! over the registry by the tests below, with no database. Adding a verb is
//! a reviewed one-line change to `VERBS`.

use serde_json::Value;

use super::{Idempotency, OpKind, OpMeta};

/// The verbs each kind may use: `<thing>_<verb>`.
pub const VERBS: &[(OpKind, &[&str])] = &[
    (OpKind::Describe, &["get", "open"]),
    (OpKind::Query, &["list", "search", "history"]),
    (
        OpKind::Create,
        &[
            "create",
            "create_many",
            "send",
            "upload",
            "import",
            "record",
            "start",
            "schedule",
            "duplicate",
            "forward",
        ],
    ),
    (
        OpKind::Change,
        &[
            "update",
            "update_many",
            "update_blocks",
            "update_elements",
            "archive",
            "restore",
            "delete",
            "move",
            "reorder",
            "claim",
            "release",
            "link",
            "unlink",
            "react",
            "pin",
            "mark_read",
            "decide",
            "respond",
            "assign",
            "set",
            "cancel",
            "retry",
        ],
    ),
    (
        OpKind::Suggest,
        &["suggest", "draft", "summarize", "analyze"],
    ),
];

/// The verb of a name: the longest listed verb it ends with, after a thing.
fn verb_of(name: &str) -> Option<(&'static str, OpKind)> {
    VERBS
        .iter()
        .flat_map(|(kind, verbs)| verbs.iter().map(move |verb| (*verb, *kind)))
        .filter(|(verb, _)| {
            name.strip_suffix(verb)
                .and_then(|thing| thing.strip_suffix('_'))
                .is_some_and(|thing| !thing.is_empty())
        })
        .max_by_key(|(verb, _)| verb.len())
}

/// Whether `name` is `<thing>_<verb>` in lower snake case with a listed
/// verb, and (given a kind) a verb of that kind.
pub fn check_name(name: &str, kind: Option<OpKind>) -> Result<(), String> {
    let snake = !name.is_empty()
        && name
            .split('_')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase()));
    if !snake {
        return Err(format!(
            "{name}: names are lower snake case, <thing>_<verb>"
        ));
    }
    let Some((verb, verb_kind)) = verb_of(name) else {
        return Err(format!(
            "{name}: its verb isn't one of the listed verbs (ops/conventions.rs VERBS)"
        ));
    };
    if let Some(kind) = kind
        && kind != verb_kind
    {
        return Err(format!(
            "{name}: `{verb}` is a {} verb, but the operation is {}",
            verb_kind.key(),
            kind.key()
        ));
    }
    Ok(())
}

/// Every convention an operation's description keeps; `Err` lists the
/// broken ones.
pub fn check(meta: &OpMeta) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    let name = meta.name;
    if let Err(problem) = check_name(name, Some(meta.kind)) {
        problems.push(problem);
    }
    let writes = meta.kind.writes();
    if writes && meta.idempotency == Idempotency::None {
        problems.push(format!("{name}: a write needs a request id (IDEMPOTENCY)"));
    }
    if !writes && meta.idempotency != Idempotency::None {
        problems.push(format!("{name}: a read keeps no request id"));
    }
    if writes && meta.emits.is_empty() {
        problems.push(format!(
            "{name}: a write declares the events it emits (EMITS)"
        ));
    }
    if meta.proposable && meta.kind != OpKind::Change {
        problems.push(format!("{name}: only a change may arrive as a proposal"));
    }
    if meta.title.trim().is_empty() {
        problems.push(format!("{name}: no title"));
    }
    let length = meta.description.chars().count();
    if !(40..=1200).contains(&length) {
        problems.push(format!(
            "{name}: the description is {length} characters; keep it to 40 to 1,200"
        ));
    }
    let input = (meta.input_schema)();
    if input.get("additionalProperties") != Some(&Value::Bool(false)) {
        problems.push(format!(
            "{name}: the input refuses unknown fields (deny_unknown_fields)"
        ));
    }
    if let Some(properties) = input.get("properties").and_then(Value::as_object) {
        for field in properties.keys() {
            if field.contains('_') || field.starts_with(|c: char| c.is_ascii_uppercase()) {
                problems.push(format!("{name}: the input field {field} isn't camelCase"));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{
        OpCall, OpError, Operation, Perm, Target, Targeted, registry, registry::entry,
    };

    #[test]
    fn every_operation_keeps_the_conventions() {
        let problems = registry()
            .entries()
            .filter_map(|entry| check(&entry.meta).err())
            .flatten()
            .collect::<Vec<_>>();
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn a_verb_outside_the_list_fails() {
        assert!(check_name("task_frobnicate", None).is_err());
        assert!(check_name("task_update", Some(OpKind::Change)).is_ok());
        assert!(check_name("task_update", Some(OpKind::Create)).is_err());
        assert!(check_name("decision_record", Some(OpKind::Create)).is_ok());
        assert!(check_name("task_create_many", Some(OpKind::Create)).is_ok());
        assert!(check_name("Task_update", None).is_err());
        assert!(check_name("update", None).is_err());
    }

    use rmcp::schemars::{self, JsonSchema};

    #[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Nothing {}

    impl Targeted for Nothing {
        fn target(&self) -> Target {
            Target::Workspace(None)
        }
    }

    struct Careless;

    impl Operation for Careless {
        const NAME: &'static str = "thing_create";
        const KIND: OpKind = OpKind::Create;
        const MODULE: &'static str = "workspace";
        const PERMISSION: Option<Perm> = None;
        const IDEMPOTENCY: Idempotency = Idempotency::None;
        const TITLE: &'static str = "Create a thing";
        const DESCRIPTION: &'static str =
            "Creates a thing, without a request id: retries would make two.";
        type Input = Nothing;
        type Output = ();

        async fn run(_call: &mut OpCall<'_>, _input: Nothing) -> Result<(), OpError> {
            Ok(())
        }
    }

    #[test]
    fn a_create_without_a_request_id_fails() {
        let problems = check(&entry::<Careless>().meta).expect_err("refused");
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("request id")),
            "{problems:?}"
        );
        assert!(
            problems.iter().any(|problem| problem.contains("EMITS")),
            "{problems:?}"
        );
    }

    #[test]
    fn operations_check_no_account_kinds() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        let mut folders = vec![root.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.file_name().is_some_and(|name| name == "ops.rs")
                    && let Ok(text) = std::fs::read_to_string(&path)
                {
                    if text.contains("ActorKind::") || text.contains("actor_kind ==") {
                        found.push(path.display().to_string());
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "An operation decides by account kind; a person and an agent with the same role \
             get the same answer: {found:?}"
        );
    }
}
