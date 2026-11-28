//! The list of operations, built once. Each module adds its operations with
//! `register_ops(&mut Registry)`, which its registration in
//! `modules/core/<id>/mod.rs` hands to the module registry
//! (`module.operations(...)`); `entry::<O>()` erases an operation into an
//! entry.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock, RwLock},
};

use futures_util::future::BoxFuture;
use rmcp::schemars::JsonSchema;
use serde_json::Value;

use super::{
    Idempotency, OpKind, Operation, Perm,
    dispatch::{Answer, DispatchRequest},
};
use crate::AppState;

/// What an operation is, without its code: what every edge and generator
/// reads.
#[derive(Clone, Debug)]
pub struct OpMeta {
    pub name: &'static str,
    pub kind: OpKind,
    pub module: &'static str,
    pub permission: Option<Perm>,
    pub proposable: bool,
    pub idempotency: Idempotency,
    pub long: bool,
    pub preview: bool,
    pub destructive: bool,
    pub automation: bool,
    pub title: &'static str,
    pub description: &'static str,
    pub emits: &'static [&'static str],
    /// Output fields shown once and never kept (`Operation::SHOWN_ONCE`).
    pub shown_once: &'static [&'static str],
    pub max_body_bytes: usize,
    /// Reads as an MCP resource (`Operation::RESOURCE`).
    pub resource: Option<super::ResourceAddress>,
    pub input_schema: fn() -> Value,
    pub output_schema: fn() -> Value,
}

impl OpMeta {
    /// Whether a rule may run it (W1.42): marked for automations, a write,
    /// not an automation's own, not one that needs the version of what it
    /// changes (a rule can't know it when it fires), and not one that takes a
    /// secret or answers with one shown once: a rule's actions and runs are
    /// stored and listed as written, and nobody is there to be shown it.
    pub fn runs_in_rules(&self) -> bool {
        let schema = (self.input_schema)();
        self.automation
            && !["query", "describe"].contains(&self.kind.key())
            && !self.name.starts_with("automation_")
            && !needs_version(&schema)
            && !takes_secret(&schema)
            && self.shown_once.is_empty()
            && !takes_secret(&(self.output_schema)())
    }
}

/// Whether an input schema requires `expectedVersion`.
fn needs_version(schema: &Value) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| required.iter().any(|field| field == "expectedVersion"))
}

/// Whether any field, at any depth, holds a secret: a token, a password, a
/// secret or private key, or an API key.
fn takes_secret(schema: &Value) -> bool {
    match schema {
        Value::Object(fields) => fields.iter().any(|(key, value)| {
            let named = key == "properties"
                && value
                    .as_object()
                    .is_some_and(|properties| properties.keys().any(|name| names_a_secret(name)));
            named || takes_secret(value)
        }),
        Value::Array(items) => items.iter().any(takes_secret),
        _ => false,
    }
}

/// Whether a field's name says it holds a secret.
pub(super) fn names_a_secret(field: &str) -> bool {
    // A token is the field's last word (`token`, `accessToken`), so token
    // counts and design tokens (`tokens`, `inputTokens`) aren't secrets.
    const SECRET: [&str; 5] = ["secret", "password", "passphrase", "privatekey", "apikey"];
    let name = field.to_ascii_lowercase().replace('_', "");
    name.ends_with("token") || SECRET.iter().any(|secret| name.contains(secret))
}

/// The fields of an answer that read as a secret in text: the answer's own
/// (which `SHOWN_ONCE` can keep out of a stored answer) and those further in
/// (which it can't reach). An answer that is one of several shapes has each
/// shape's fields as its own.
pub(super) fn secrets_in_answer(schema: &Value) -> (Vec<String>, Vec<String>) {
    struct Found<'a> {
        root: &'a Value,
        own: Vec<String>,
        inside: Vec<String>,
        seen: Vec<(*const Value, bool)>,
    }
    impl<'a> Found<'a> {
        fn resolved(&self, schema: &'a Value) -> &'a Value {
            schema
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|path| path.strip_prefix('#'))
                .and_then(|pointer| self.root.pointer(pointer))
                .unwrap_or(schema)
        }
        fn text(&self, field: &'a Value) -> bool {
            match self.resolved(field).get("type") {
                Some(Value::String(kind)) => kind == "string",
                Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "string"),
                _ => false,
            }
        }
        fn look(&mut self, schema: &'a Value, own: bool) {
            let schema = self.resolved(schema);
            if self.seen.contains(&(std::ptr::from_ref(schema), own)) {
                return;
            }
            self.seen.push((std::ptr::from_ref(schema), own));
            let fields = schema.get("properties").and_then(Value::as_object);
            for (name, field) in fields.into_iter().flatten() {
                if names_a_secret(name) && self.text(field) {
                    let list = if own { &mut self.own } else { &mut self.inside };
                    list.push(name.clone());
                }
                self.look(field, false);
            }
            for shapes in ["oneOf", "anyOf", "allOf"] {
                let shapes = schema.get(shapes).and_then(Value::as_array);
                for shape in shapes.into_iter().flatten() {
                    self.look(shape, own);
                }
            }
            for each in ["items", "additionalProperties"] {
                if let Some(each) = schema.get(each) {
                    self.look(each, false);
                }
            }
            let listed = schema.get("prefixItems").and_then(Value::as_array);
            for item in listed.into_iter().flatten() {
                self.look(item, false);
            }
        }
    }
    let mut found = Found {
        root: schema,
        own: Vec::new(),
        inside: Vec::new(),
        seen: Vec::new(),
    };
    found.look(schema, true);
    (found.own, found.inside)
}

type Call = for<'a> fn(&'a AppState, DispatchRequest, Value) -> BoxFuture<'a, Answer>;

/// Whether a caller may run it on a target, without running it
/// (`dispatch::may_run`).
type Check = for<'a> fn(
    &'a AppState,
    &'a super::Caller,
    Option<uuid::Uuid>,
    &'a super::Target,
) -> BoxFuture<'a, Result<(), super::OpError>>;

/// One operation in the registry.
#[derive(Clone)]
pub struct OpEntry {
    pub meta: OpMeta,
    pub call: Call,
    pub check: Check,
}

impl std::fmt::Debug for OpEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OpEntry")
            .field("name", &self.meta.name)
            .finish()
    }
}

/// A JSON Schema for `T`, in the dialect the MCP tools already use.
pub fn schema_of<T: JsonSchema + 'static>() -> Value {
    Value::Object((*rmcp::handler::server::common::schema_for_type::<T>()).clone())
}

/// The one generic eraser.
pub fn entry<O: Operation>() -> OpEntry {
    OpEntry {
        meta: OpMeta {
            name: O::NAME,
            kind: O::KIND,
            module: O::MODULE,
            permission: O::PERMISSION,
            proposable: O::PROPOSABLE,
            idempotency: O::IDEMPOTENCY,
            long: O::LONG,
            preview: O::PREVIEW,
            destructive: O::DESTRUCTIVE,
            automation: O::AUTOMATION,
            title: O::TITLE,
            description: O::DESCRIPTION,
            emits: O::EMITS,
            shown_once: O::SHOWN_ONCE,
            max_body_bytes: O::MAX_BODY_BYTES,
            resource: O::RESOURCE,
            input_schema: schema_of::<O::Input>,
            output_schema: schema_of::<O::Output>,
        },
        call: |state, request, input| Box::pin(super::dispatch::run::<O>(state, request, input)),
        check: |state, caller, workspace, target| {
            Box::pin(super::dispatch::check::<O>(
                state, caller, workspace, target,
            ))
        },
    }
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    by_name: BTreeMap<&'static str, OpEntry>,
}

impl Registry {
    /// Adds an operation. Two with one name is a programming error, caught
    /// by `registry_builds` before anything ships.
    pub fn add<O: Operation>(&mut self) -> &mut Self {
        let previous = self.by_name.insert(O::NAME, entry::<O>());
        assert!(previous.is_none(), "two operations named {}", O::NAME);
        self
    }

    pub fn get(&self, name: &str) -> Option<&OpEntry> {
        self.by_name.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub fn entries(&self) -> impl Iterator<Item = &OpEntry> {
        self.by_name.values()
    }

    /// Every module's operations.
    fn build() -> Self {
        let mut registry = Self::default();
        for (_module, add) in crate::modules::registry::Registry::global().operation_registrations()
        {
            add(&mut registry);
        }
        for extra in extras().iter() {
            extra(&mut registry);
        }
        registry
    }
}

static CURRENT: RwLock<Option<Arc<Registry>>> = RwLock::new(None);

/// Adds operations to a registry.
type AddOps = fn(&mut Registry);

/// Operations added by tests outside the crate.
fn extras_cell() -> &'static RwLock<Vec<AddOps>> {
    static EXTRAS: OnceLock<RwLock<Vec<AddOps>>> = OnceLock::new();
    EXTRAS.get_or_init(|| RwLock::new(Vec::new()))
}

fn extras() -> std::sync::RwLockReadGuard<'static, Vec<fn(&mut Registry)>> {
    extras_cell()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The registry, built on first use.
pub fn registry() -> Arc<Registry> {
    if let Some(current) = CURRENT
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        return Arc::clone(current);
    }
    let mut slot = CURRENT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(slot.get_or_insert_with(|| Arc::new(Registry::build())))
}

/// For tests outside the crate (the integration suite): add operations
/// that exist only in the test tree, such as the kernel's echo operation.
/// The registry is rebuilt with them; nothing in the server calls this.
pub fn register_for_tests(add: fn(&mut Registry)) {
    {
        let mut extras = extras_cell()
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if extras.iter().any(|known| std::ptr::fn_addr_eq(*known, add)) {
            return;
        }
        extras.push(add);
    }
    *CURRENT
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
