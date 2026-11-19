//! What each kind of thing tells discussion (C1.3).
//!
//! Stopgap: K3's thing registry (K3.10) hasn't landed, so the kinds that can
//! be discussed are this static table, which K3 later absorbs. Kind names
//! follow R54's ThingKind grammar, and the same kinds are the arms of
//! `subject_visible_to` and `subject_href` in the migration
//! `20261115060000_discussion.sql`.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::error::AppError;

/// A kind of thing people and agents comment on.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Page,
    Sheet,
    Whiteboard,
    Record,
    Task,
}

/// Which anchors a kind takes (`anchors::check`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnchorShape {
    /// A quote in a block of a page.
    Text,
    /// A cell or a range on a tab of a sheet.
    Cells,
    /// Elements on a board, with a point to fall back on.
    Elements,
    /// A field of a record or task.
    Field,
}

/// One row of the table.
pub struct Subject {
    pub kind: SubjectKind,
    pub anchor: AnchorShape,
    /// `$1` the thing, `$2` the viewer: its workspace, its project (none for
    /// a board outside any project) and whether it takes new comments,
    /// only when the viewer may see it. Locks the thing against archiving
    /// while a comment is posted.
    home_sql: &'static str,
    /// `$1` the thing: who hears about new comments besides mentions (its
    /// maker; a task's assignee and watchers).
    followers_sql: &'static str,
}

pub const SUBJECTS: &[Subject] = &[
    Subject {
        kind: SubjectKind::Page,
        anchor: AnchorShape::Text,
        home_sql: "SELECT project.organization_id, page.project_id, page.archived_at IS NULL \
             FROM project_pages page JOIN projects project ON project.id = page.project_id \
             WHERE page.id = $1 AND subject_visible_to('page', page.id, $2) \
             FOR SHARE OF page",
        followers_sql: "SELECT created_by_actor_id FROM project_pages \
                 WHERE id = $1 AND created_by_actor_id IS NOT NULL \
             UNION SELECT account_id FROM follows \
                 WHERE target_kind = 'page' AND target_id = $1 AND level = 'everything'",
    },
    Subject {
        kind: SubjectKind::Sheet,
        anchor: AnchorShape::Cells,
        home_sql: "SELECT project.organization_id, sheet.project_id, sheet.archived_at IS NULL \
             FROM sheets sheet JOIN projects project ON project.id = sheet.project_id \
             WHERE sheet.id = $1 AND subject_visible_to('sheet', sheet.id, $2) \
             FOR SHARE OF sheet",
        followers_sql: "SELECT created_by_actor_id FROM sheets \
                 WHERE id = $1 AND created_by_actor_id IS NOT NULL \
             UNION SELECT account_id FROM follows \
                 WHERE target_kind = 'sheet' AND target_id = $1 AND level = 'everything'",
    },
    Subject {
        kind: SubjectKind::Whiteboard,
        anchor: AnchorShape::Elements,
        home_sql: "SELECT board.organization_id, board.project_id, board.deleted_at IS NULL \
             FROM whiteboards board \
             WHERE board.id = $1 AND subject_visible_to('whiteboard', board.id, $2) \
             FOR SHARE OF board",
        followers_sql: "SELECT created_by_actor_id FROM whiteboards \
                 WHERE id = $1 AND created_by_actor_id IS NOT NULL \
             UNION SELECT account_id FROM follows \
                 WHERE target_kind = 'whiteboard' AND target_id = $1 AND level = 'everything'",
    },
    Subject {
        kind: SubjectKind::Record,
        anchor: AnchorShape::Field,
        home_sql: "SELECT project.organization_id, record.project_id, record.archived_at IS NULL \
             FROM collection_records record JOIN projects project ON project.id = record.project_id \
             WHERE record.id = $1 AND subject_visible_to('record', record.id, $2) \
             FOR SHARE OF record",
        followers_sql: "SELECT created_by_actor_id FROM collection_records \
                 WHERE id = $1 AND created_by_actor_id IS NOT NULL \
             UNION SELECT account_id FROM follows \
                 WHERE target_kind = 'record' AND target_id = $1 AND level = 'everything'",
    },
    Subject {
        kind: SubjectKind::Task,
        anchor: AnchorShape::Field,
        home_sql: "SELECT project.organization_id, item.project_id, item.archived_at IS NULL \
             FROM task_rows item JOIN projects project ON project.id = item.project_id \
             WHERE item.id = $1 AND subject_visible_to('task', item.id, $2)",
        // Those who follow everything about the task (its maker and
        // assignee do until they stop), so unfollowing quiets its comments too.
        followers_sql: "SELECT account_id FROM follows \
             WHERE target_kind = 'task' AND target_id = $1 AND level = 'everything'",
    },
];

impl SubjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Sheet => "sheet",
            Self::Whiteboard => "whiteboard",
            Self::Record => "record",
            Self::Task => "task",
        }
    }

    pub fn subject(self) -> &'static Subject {
        SUBJECTS
            .iter()
            .find(|subject| subject.kind == self)
            .expect("every kind is in the table")
    }

    pub fn parse(kind: &str) -> Option<Self> {
        SUBJECTS
            .iter()
            .map(|subject| subject.kind)
            .find(|candidate| candidate.as_str() == kind)
    }
}

/// Where a thing's comments live.
#[derive(Clone, Copy, Debug)]
pub struct SubjectHome {
    pub workspace_id: Uuid,
    pub project_id: Option<Uuid>,
    /// False for an archived or deleted thing.
    pub accepts_comments: bool,
}

impl Subject {
    /// The thing's home, when `viewer` may see it.
    pub async fn home(
        &self,
        connection: &mut PgConnection,
        id: Uuid,
        viewer: Uuid,
    ) -> Result<Option<SubjectHome>, AppError> {
        Ok(
            sqlx::query_as::<_, (Uuid, Option<Uuid>, bool)>(self.home_sql)
                .bind(id)
                .bind(viewer)
                .fetch_optional(&mut *connection)
                .await?
                .map(|(workspace_id, project_id, accepts_comments)| SubjectHome {
                    workspace_id,
                    project_id,
                    accepts_comments,
                }),
        )
    }

    /// Who hears about new comments, before anyone's access is checked.
    pub async fn followers(
        &self,
        connection: &mut PgConnection,
        id: Uuid,
    ) -> Result<Vec<Uuid>, AppError> {
        Ok(sqlx::query_scalar::<_, Uuid>(self.followers_sql)
            .bind(id)
            .fetch_all(&mut *connection)
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::{SUBJECTS, SubjectKind};

    #[test]
    fn kinds_follow_the_thing_kind_grammar_and_are_listed_once() {
        let grammar = |kind: &str| {
            let mut parts = kind.split('.');
            let part_ok = |part: &str| {
                (2..=40).contains(&part.len())
                    && part.starts_with(|first: char| first.is_ascii_lowercase())
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            };
            parts.next().is_some_and(part_ok)
                && parts.next().is_none_or(part_ok)
                && parts.next().is_none()
        };
        for (index, subject) in SUBJECTS.iter().enumerate() {
            let kind = subject.kind.as_str();
            assert!(grammar(kind), "{kind}");
            assert_eq!(SubjectKind::parse(kind), Some(subject.kind));
            assert!(
                SUBJECTS[index + 1..]
                    .iter()
                    .all(|other| other.kind != subject.kind),
                "{kind} twice"
            );
        }
        assert_eq!(SubjectKind::parse("conversation"), None);
    }

    #[test]
    fn the_migration_knows_every_kind() {
        let migration = include_str!("../../migrations/20261115060000_discussion.sql");
        for subject in SUBJECTS {
            let arm = format!("WHEN '{}' THEN", subject.kind.as_str());
            assert_eq!(
                migration.matches(&arm).count(),
                2,
                "{arm} in subject_visible_to and subject_href"
            );
        }
    }
}
