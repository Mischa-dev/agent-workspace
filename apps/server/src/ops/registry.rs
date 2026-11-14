//! The list of operations, built once. Each module adds its operations with
//! `register_ops(&mut Registry)`; `entry::<O>()` erases an operation into an
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
    pub input_schema: fn() -> Value,
    pub output_schema: fn() -> Value,
}

type Call = for<'a> fn(&'a AppState, DispatchRequest, Value) -> BoxFuture<'a, Answer>;

/// One operation in the registry.
#[derive(Clone)]
pub struct OpEntry {
    pub meta: OpMeta,
    pub call: Call,
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
            input_schema: schema_of::<O::Input>,
            output_schema: schema_of::<O::Output>,
        },
        call: |state, request, input| Box::pin(super::dispatch::run::<O>(state, request, input)),
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
        #[allow(unused_mut)]
        let mut registry = Self::default();
        // Modules' `register_ops` are added here (M1.1 moves them into each
        // module's registration).
        for extra in extras().iter() {
            extra(&mut registry);
        }
        registry
    }
}

static CURRENT: RwLock<Option<Arc<Registry>>> = RwLock::new(None);

/// Operations added by tests outside the crate.
fn extras_cell() -> &'static RwLock<Vec<fn(&mut Registry)>> {
    static EXTRAS: OnceLock<RwLock<Vec<fn(&mut Registry)>>> = OnceLock::new();
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
