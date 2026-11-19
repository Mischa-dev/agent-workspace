//! The kinds of space (K4 §3.4). The stored `standard` is a project until
//! K4.22 renames it. DMs and groups live in no space (K4.17).

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    /// The workspace's own space: every workspace member is in it.
    Workspace,
    /// A project (stored `standard`).
    Project,
    /// A team's space.
    Team,
    /// One account's own space in a workspace.
    Personal,
}

impl SpaceKind {
    pub const ALL: [Self; 4] = [Self::Workspace, Self::Project, Self::Team, Self::Personal];

    /// The one place a stored kind is read.
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "workspace" => Some(Self::Workspace),
            "standard" | "project" => Some(Self::Project),
            "team" => Some(Self::Team),
            "personal" => Some(Self::Personal),
            _ => None,
        }
    }

    /// The modules a space of this kind holds when it starts (K4.10), beyond
    /// the ones always on; `None` when it may hold any. The workspace's own
    /// space holds the workspace's pages, boards and channels, never tasks
    /// or the rest of a project. This is data, ready for M1's per-space
    /// switches: a space's own switches only narrow it further.
    pub const fn starting_modules(self) -> Option<&'static [&'static str]> {
        match self {
            // Messages too: channels need it, and comments on the
            // workspace's own boards and sheets belong to it. Inbox: a
            // reminder filed in the workspace is kept in its space. Agents:
            // they belong to the workspace (K1.6), and one added without a
            // project lives here (K4.6). Runs: such an agent's runs, answering
            // a DM or a channel here (A1.25), are kept here too.
            Self::Workspace => Some(&[
                "pages",
                "whiteboards",
                "channels",
                "messages",
                "inbox",
                "agents",
                "runs",
            ]),
            // Drafts and notes: pages, boards, sheets and collections; no
            // chat, since nobody else is there.
            Self::Personal => Some(&["pages", "whiteboards", "sheets", "collections"]),
            Self::Project | Self::Team => None,
        }
    }

    /// The value stored in `projects.kind` today.
    pub const fn stored(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Project => "standard",
            Self::Team => "team",
            Self::Personal => "personal",
        }
    }
}

/// Whether a place is the direct scope, where DMs and groups live outside
/// every space (K4.17), rather than a space.
pub fn holds_messages(place: uuid::Uuid) -> bool {
    place == super::DIRECT
}

/// Who may find and read a space.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpaceVisibility {
    /// Only its members.
    Members,
    /// Every workspace member may read it and join it.
    Workspace,
}

impl SpaceVisibility {
    pub fn from_stored(value: &str) -> Option<Self> {
        match value {
            "members" => Some(Self::Members),
            "workspace" => Some(Self::Workspace),
            _ => None,
        }
    }

    pub const fn stored(self) -> &'static str {
        match self {
            Self::Members => "members",
            Self::Workspace => "workspace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpaceKind;

    #[test]
    fn stored_kinds_round_trip_and_direct_is_not_a_space() {
        for kind in SpaceKind::ALL {
            assert_eq!(SpaceKind::from_stored(kind.stored()), Some(kind));
        }
        assert_eq!(SpaceKind::from_stored("standard"), Some(SpaceKind::Project));
        assert_eq!(SpaceKind::from_stored("direct"), None);
    }
}
