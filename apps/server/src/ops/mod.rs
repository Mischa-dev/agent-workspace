//! The operation layer (K2): every thing the app can do, written once.
//!
//! An operation has a name (`<thing>_<verb>`), an input type, the permission
//! it needs, whether it can arrive as a proposal, and one function that does
//! the work in one transaction. The web route, the agent tool, the public
//! API endpoint, the automation action and the audit row all come from that
//! one definition (`ops::web`, `ops::mcp`, `ops::automation`), so a person
//! and an agent asking for the same thing run the same code and get the
//! same answer, with the same errors, retry safety and audit row.
//!
//! A module owns `src/<module>/ops.rs` with its operation structs and a
//! `register_ops(&mut Registry)`; `registry::registry()` collects them. The
//! `add-operation` skill walks through adding one.
//!
//! The pieces:
//! - `Operation` (this file): the trait an operation implements.
//! - `context`: who is calling (`Caller`), through which edge (`Edge`), and
//!   what `run` sees (`OpContext`, `OpCall`).
//! - `target`: what an input acts on, and where that lives.
//! - `error` and `envelope`: one error list and one result shape for every
//!   edge.
//! - `access_shim`: today's role and membership checks behind the shape K1's
//!   access module will have (deleted by K2.17).
//! - `dispatch`: the pipeline every call goes through.
//! - `registry`: the list of operations, built once.

use std::future::Future;

use rmcp::schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};

pub mod access_shim;
pub mod context;
pub mod dispatch;
pub mod envelope;
pub mod error;
pub mod refs;
pub mod registry;
pub mod target;

pub use context::{Caller, Edge, OpCall, OpContext};
pub use dispatch::{Answer, DispatchRequest, dispatch};
pub use envelope::Envelope;
pub use error::{ConflictReason, DenyReason, OpError, RetryCategory};
pub use registry::{OpEntry, OpMeta, Registry, registry};
pub use target::{Located, Target, Targeted};

/// The permission an operation needs: K1's catalogue key. Until K1.2 lands
/// this is today's `policy::Permission`; K1.2 changes the alias.
pub type Perm = crate::policy::Permission;

/// The five kinds of operation (END-STATE section 5). Agents learn a new
/// module from an old one by them; the verbs each kind may use are in
/// `conventions`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    /// One thing, with what it links to.
    Describe,
    /// Many things, paged.
    Query,
    /// A new thing.
    Create,
    /// An existing thing changes.
    Change,
    /// Something to accept (a draft, an analysis); never changes the thing.
    Suggest,
}

impl OpKind {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Describe => "describe",
            Self::Query => "query",
            Self::Create => "create",
            Self::Change => "change",
            Self::Suggest => "suggest",
        }
    }

    /// Whether operations of this kind change anything.
    pub const fn writes(self) -> bool {
        matches!(self, Self::Create | Self::Change | Self::Suggest)
    }
}

/// Who makes a retried write safe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Idempotency {
    /// The dispatcher reserves the request id in its transaction before
    /// `run`, and replays the stored envelope on a retry.
    Dispatcher,
    /// The wrapped command reserves it itself (message send); the dispatcher
    /// passes `ctx.request_id` through and never reserves twice.
    Command,
    /// Reads.
    None,
}

/// One operation. A unit struct implements it; `registry::entry::<O>()`
/// erases it into the registry.
pub trait Operation: Send + Sync + 'static {
    /// `<thing>_<verb>`, such as `decision_record`.
    const NAME: &'static str;
    const KIND: OpKind;
    /// The module it belongs to (a manifest id), which must be on where the
    /// call acts.
    const MODULE: &'static str;
    /// The permission it needs where it acts; `None` means anyone who can
    /// see the target.
    const PERMISSION: Option<Perm>;
    /// May arrive as a proposal (C1) when access says "suggest only".
    const PROPOSABLE: bool = false;
    const IDEMPOTENCY: Idempotency;
    /// May run as a job (MCP Tasks extension, K2.21).
    const LONG: bool = false;
    /// Accepts `preview: true`: runs and rolls back.
    const PREVIEW: bool = false;
    /// Removes or overwrites something a person would miss.
    const DESTRUCTIVE: bool = false;
    /// Offered as an automation action. Writes are by default.
    const AUTOMATION: bool = Self::KIND.writes();
    /// "Record a decision".
    const TITLE: &'static str;
    /// For agents and docs. Vendor-free and kind-free (K2.1).
    const DESCRIPTION: &'static str;
    /// Event kinds a write emits (K3), for triggers and docs.
    const EMITS: &'static [&'static str] = &[];

    type Input: DeserializeOwned + Serialize + JsonSchema + Targeted + Send + 'static;
    type Output: Serialize + JsonSchema + Send + 'static;

    /// Does the work. Writes run inside the dispatcher's transaction
    /// (`call.tx()`); the dispatcher commits after the audit row.
    fn run(
        call: &mut OpCall<'_>,
        input: Self::Input,
    ) -> impl Future<Output = Result<Self::Output, OpError>> + Send;
}
