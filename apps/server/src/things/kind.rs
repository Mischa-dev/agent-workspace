//! A kind's name (K3 3.1). Open-ended: M1's built and connected modules add
//! kinds at runtime, so it is a string with constants for the core kinds.
//! K1's `access::Target` and K2's `ops::Target` use this type; whichever of
//! K1.2, K2.2 or K3.10 merges first creates this file.
//!
//! Every kind name has one grammar (ruling R54): `name` for a core kind,
//! `install.name` for a kind owned by the connected install that registered
//! it (an install registers or changes only its own kinds, and only in its
//! workspace). [`valid_name`] is the Rust check and the `thing_kind_name`
//! SQL domain the database's; everything that validates a kind (the
//! registry, manifests, comment subjects, links, event declarations,
//! access and export) uses one of the two.

use std::{borrow::Cow, fmt};

use serde::{Deserialize, Serialize};

/// The grammar of a kind's name, as the `thing_kind_name` domain has it.
pub const NAME_PATTERN: &str = r"^[a-z][a-z0-9_]{1,39}(\.[a-z][a-z0-9_]{1,39})?$";

/// Whether `name` is a kind's name: one or two parts, each a lower case
/// letter then 1 to 39 lower case letters, digits or `_`.
pub fn valid_name(name: &str) -> bool {
    fn part(part: &str) -> bool {
        let mut characters = part.chars();
        characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
            && (2..=40).contains(&part.len())
            && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }
    match name.split_once('.') {
        Some((install, kind)) => part(install) && part(kind),
        None => part(name),
    }
}

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
    pub const RUN: Self = Self::named("run");
    pub const REQUEST: Self = Self::named("request");

    pub const fn named(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }

    /// A kind read from storage, which has checked its name already.
    pub fn new(name: impl Into<String>) -> Self {
        Self(Cow::Owned(name.into()))
    }

    /// A kind from a request or a manifest: None unless the name is well
    /// formed (R54).
    pub fn parse(name: &str) -> Option<Self> {
        valid_name(name).then(|| Self::new(name))
    }

    /// A core kind: an unqualified name, reserved for this server.
    pub fn is_core(&self) -> bool {
        !self.0.contains('.')
    }

    /// The install that owns a qualified kind (`acme` in `acme.build_v2`).
    pub fn install(&self) -> Option<&str> {
        self.0.split_once('.').map(|(install, _)| install)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_follow_one_grammar() {
        for good in ["task", "pull_request", "ab", "acme.build_v2", "x9.y_"] {
            assert!(valid_name(good), "{good}");
        }
        let long = "a".repeat(41);
        for bad in [
            "",
            "a",
            "Task",
            "9task",
            "_task",
            "task-x",
            "acme.",
            ".task",
            "a.b.c",
            "acme.b",
            long.as_str(),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_name(&"a".repeat(40)));
        let kind = ThingKind::parse("acme.build_v2").expect("qualified");
        assert!(!kind.is_core());
        assert_eq!(kind.install(), Some("acme"));
        assert!(ThingKind::TASK.is_core());
        assert_eq!(ThingKind::parse("Bad"), None);
    }

    #[test]
    fn the_database_domain_has_the_same_grammar() {
        let migration = include_str!("../../migrations/20261114190000_event_log.sql");
        assert!(
            migration.contains(&format!("CHECK (VALUE ~ '{NAME_PATTERN}')")),
            "thing_kind_name and NAME_PATTERN must match"
        );
    }

    #[test]
    fn every_registered_kind_is_a_core_name() {
        for spec in super::super::registry() {
            assert!(valid_name(spec.kind.as_str()), "{}", spec.kind);
            assert!(spec.kind.is_core(), "{}", spec.kind);
        }
    }
}
