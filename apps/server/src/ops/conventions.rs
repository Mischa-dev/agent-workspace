//! The conventions every operation keeps (K2 sections 3.6 and 6), checked
//! over the registry by the tests below, with no database. Adding a verb is
//! a reviewed one-line change to `VERBS`.

use serde_json::Value;

use super::{Idempotency, OpKind, OpMeta, registry::secrets_in_answer};

/// The verbs each kind may use: `<thing>_<verb>`.
pub const VERBS: &[(OpKind, &[&str])] = &[
    (OpKind::Describe, &["get", "open", "show"]),
    (
        OpKind::Query,
        // `answer`: a written answer from what the caller can read (C2.27).
        &["list", "search", "history", "export", "preview", "answer"],
    ),
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
            // The second step of a two-step create (a direct upload).
            "finish",
            "schedule",
            "duplicate",
            "forward",
            "report",
            "install",
            // Made, or changed when it exists: a connected module's thing,
            // which the module names by its own id (M1.13).
            "upsert",
            // A page made from other things (messages, a thread, blocks).
            "make",
            // Tasks made from a thing's parts (a board's notes, C1.32).
            "to_tasks",
            // A change kept for someone who edits the thing to decide (C1.11).
            "propose",
            // A template made from what's there, such as a workspace (K2.17).
            "save",
            // Work handed to agents: runs started from tasks (K2.19).
            "launch",
            // A question about part of a thing, put to someone (C1.31).
            "ask",
        ],
    ),
    (
        OpKind::Change,
        &[
            "update",
            "update_many",
            "link_many",
            "update_blocks",
            "update_elements",
            "archive",
            "restore",
            "undo",
            "delete",
            "move",
            "reorder",
            "claim",
            // The best ready one of a queue, claimed in the same step (AWD-220).
            "claim_next",
            "release",
            "link",
            "unlink",
            "bind",
            "unbind",
            "react",
            "hide",
            "pin",
            "mark_read",
            "mark_not_started",
            "decide",
            "respond",
            "assign",
            "set",
            "cancel",
            "retry",
            // A run's work goes out to its code host (K2.19).
            "publish",
            // An install takes a newer version of its package (M1.10).
            "upgrade",
            // A group's agents take more turns (K2.8).
            "continue",
            // An agent passes on, or a person stops, a reply request (K2.9).
            "pass",
            "stop",
            // Every open one in a conversation at once (C2.35).
            "stop_all",
            // Something already sent goes out again, unchanged (a webhook delivery).
            "redeliver",
            // A saved query read and found right, or shared (T1.35).
            "review",
            "share",
            // A harmless signed ping, to check an address answers.
            "test",
            "press",
            "remove",
            // A template's modules, layout and roles put in place (K2.17).
            "apply",
            // You go into or out of something, such as a channel (C2.7).
            "join",
            "leave",
            // A link or credential stops working; the row stays (K4.26).
            "revoke",
            // A credential gets a new value; the old one lapses (K1.23).
            "rotate",
            // An environment is tried and the result recorded (K2.18).
            "check",
            // A repository's copy on the server is made or brought up to
            // date; a pull request is read again from its code host, or
            // merged there (K2.18).
            "prepare",
            "refresh",
            "merge",
            // An account stops acting and hearing until resumed (A2.10).
            "pause",
            "resume",
            // A host's published action runs (A2.19); so does a connected
            // module's declared operation (M1.13).
            "run",
            // Someone with the right grants what was asked: a connected
            // module's scopes (M1.12).
            "approve",
            // Something a space can use is turned on, such as triage (T1.23).
            "enable",
            // Someone makes their own what another owned, such as an agent
            // whose owning workspace is being deleted (K1.33).
            "take",
            // Someone joins or leaves a space or a workspace, or is added or
            // taken out (K4.8, A2.18).
            "add",
            "remove",
            "join",
            "leave",
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
    let output = (meta.output_schema)();
    if meta.idempotency == Idempotency::Dispatcher {
        let (own, inside) = secrets_in_answer(&output);
        for field in own {
            if !meta.shown_once.contains(&field.as_str()) {
                problems.push(format!(
                    "{name}: its answer is kept to replay to a retry, and {field} reads as a \
                     secret: add it to SHOWN_ONCE, so it is shown once and never kept"
                ));
            }
        }
        for field in inside {
            problems.push(format!(
                "{name}: {field}, inside its answer, reads as a secret, and SHOWN_ONCE reaches \
                 only the answer's own fields: move it to the top of the answer and add it there"
            ));
        }
    }
    // An answer without a declared shape (a wrapped tool's) can't be
    // checked here; the dispatcher keeps its own secret-named fields out.
    let own = output.get("properties");
    for field in meta.shown_once {
        if own.is_some_and(|own| own.get(*field).is_none()) {
            problems.push(format!(
                "{name}: SHOWN_ONCE names {field}, which isn't a field of its answer, so \
                 nothing is kept out: name the field as the answer spells it"
            ));
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

    #[derive(Debug, serde::Serialize, JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct Delivery {
        id: String,
        signing_secret: String,
    }

    #[derive(Debug, serde::Serialize, JsonSchema)]
    #[serde(rename_all = "camelCase")]
    struct Connected {
        id: String,
        webhook_secret: Option<String>,
        has_secret: bool,
        input_tokens: i64,
        deliveries: Vec<Delivery>,
    }

    /// A connection made as before the fix that blanked its secret: the
    /// answer carries it and nothing keeps it out of the stored answer.
    struct KeepsItsSecret;

    impl Operation for KeepsItsSecret {
        const NAME: &'static str = "connection_create";
        const KIND: OpKind = OpKind::Create;
        const MODULE: &'static str = "workspace";
        const PERMISSION: Option<Perm> = None;
        const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
        const SHOWN_ONCE: &'static [&'static str] = &["webhook_secret"];
        const EMITS: &'static [&'static str] = &["connection.created"];
        const TITLE: &'static str = "Connect something";
        const DESCRIPTION: &'static str =
            "Connects something that is told about events, and answers with its secret.";
        type Input = Nothing;
        type Output = Connected;

        async fn run(_call: &mut OpCall<'_>, _input: Nothing) -> Result<Connected, OpError> {
            Err(OpError::Unavailable)
        }
    }

    #[test]
    fn a_secret_an_answer_would_keep_for_a_retry_fails() {
        let problems = check(&entry::<KeepsItsSecret>().meta).expect_err("refused");
        let about = |field: &str| {
            problems
                .iter()
                .filter(|problem| problem.contains(field))
                .collect::<Vec<_>>()
        };
        assert!(
            about("webhookSecret")
                .iter()
                .any(|problem| problem.contains("add it to SHOWN_ONCE")),
            "{problems:?}"
        );
        assert!(
            about("webhook_secret")
                .iter()
                .any(|problem| problem.contains("isn't a field of its answer")),
            "{problems:?}"
        );
        assert!(
            about("signingSecret")
                .iter()
                .any(|problem| problem.contains("inside its answer")),
            "{problems:?}"
        );
        assert_eq!(problems.len(), 3, "{problems:?}");
    }

    /// An answer that is one of several shapes serializes each shape's
    /// fields at its top, where `SHOWN_ONCE` reaches them.
    #[test]
    fn a_secret_in_one_shape_of_an_answer_is_its_own() {
        let answer = serde_json::json!({
            "oneOf": [
                { "$ref": "#/$defs/Made" },
                { "type": "object", "properties": { "id": { "type": "string" } } }
            ],
            "$defs": {
                "Made": {
                    "type": "object",
                    "properties": {
                        "webhookSecret": { "type": ["string", "null"] },
                        "fields": { "type": "object", "additionalProperties": true },
                        "deliveries": { "type": "array", "items": { "$ref": "#/$defs/Delivery" } }
                    }
                },
                "Delivery": {
                    "type": "object",
                    "properties": { "signingSecret": { "type": "string" } }
                },
                "NotInTheAnswer": {
                    "type": "object",
                    "properties": { "token": { "type": "string" } }
                }
            }
        });
        assert_eq!(
            secrets_in_answer(&answer),
            (
                vec!["webhookSecret".to_owned()],
                vec!["signingSecret".to_owned()]
            )
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
                    && (text.contains("ActorKind::") || text.contains("actor_kind =="))
                {
                    found.push(path.display().to_string());
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
