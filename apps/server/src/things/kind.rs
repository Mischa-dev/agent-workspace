//! A kind's name (K3 3.1). Open-ended: M1's built and connected modules add
//! kinds at runtime, so it is a string with constants for the core kinds.
//! K1's `access::Target` and K2's `ops::Target` use this type; whichever of
//! K1.2, K2.2 or K3.10 merges first creates this file.

use std::{borrow::Cow, fmt};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThingKind(Cow<'static, str>);

impl ThingKind {
    pub const PROJECT: Self = Self::named("project");
    pub const CHANNEL: Self = Self::named("channel");
    pub const THREAD: Self = Self::named("thread");
    pub const CONVERSATION: Self = Self::named("conversation");
    pub const TASK: Self = Self::named("task");
    pub const PAGE: Self = Self::named("page");
    pub const DECISION: Self = Self::named("decision");
    pub const WHITEBOARD: Self = Self::named("whiteboard");
    pub const SHEET: Self = Self::named("sheet");
    pub const COLLECTION: Self = Self::named("collection");
    pub const RECORD: Self = Self::named("record");
    pub const MESSAGE: Self = Self::named("message");
    pub const FILE: Self = Self::named("file");
    pub const MILESTONE: Self = Self::named("milestone");
    pub const PULL_REQUEST: Self = Self::named("pull_request");
    pub const REPOSITORY: Self = Self::named("repository");
    pub const AUTOMATION: Self = Self::named("automation");

    pub const fn named(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// A kind read from storage or a request.
    pub fn new(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ThingKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl PartialEq<str> for ThingKind {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ThingKind {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}
