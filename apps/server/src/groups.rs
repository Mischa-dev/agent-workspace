//! User groups (K1.27's data, K1 plan 3.4): a named set of accounts in a
//! workspace, mentioned as `@handle` (C2.18). A mention reaches the members
//! who can read the conversation (`notifications::message_mentions`); a
//! group as large as a crowd takes the same say as `@everyone`.
//!
//! Chat reads them (`group_list`, in `ops.rs`); K1.27 adds the writes
//! (`group_create`, `group_update`, `group_members_set`, `group_archive`),
//! the `members.groups` permission and role grants to groups.

pub mod ops;
