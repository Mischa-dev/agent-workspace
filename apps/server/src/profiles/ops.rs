//! Profiles as operations (C2.17): `profile_get`, `profile_update` and
//! `profile_avatar_list`. People and agents read anyone's in the workspace
//! they share and change their own (or an agent's they manage).

use base64::Engine as _;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{MAX_AVATAR_BYTES, MAX_PRONOUNS, MAX_TITLE, avatar_type, avatar_url};
use crate::{
    access,
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
    spaces::SpaceKind,
    statuses::StatusView,
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ProfileGet>()
        .add::<ProfileUpdate>()
        .add::<ProfileAvatarList>();
}

/// A space the reader and the account are both in.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedSpace {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
}

/// An account's profile, as someone who shares the workspace sees it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    /// person, agent or app.
    pub kind: String,
    pub name: String,
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// Their time zone, such as Europe/Berlin, for their local time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    /// Their status here: their own, and a line about work here.
    pub statuses: Vec<StatusView>,
    /// The spaces of this workspace you are both in.
    pub shared_spaces: Vec<SharedSpace>,
    /// Whether you may change it: your own, or an agent's you manage.
    pub can_edit: bool,
}

#[derive(sqlx::FromRow)]
struct ProfileRow {
    kind: String,
    name: Option<String>,
    handle: Option<String>,
    title: Option<String>,
    pronouns: Option<String>,
    time_zone: Option<String>,
    avatar_version: Option<i32>,
}

/// The account if it's an enabled member of `workspace`; not found
/// otherwise (an account elsewhere is none of this workspace's business).
async fn member_of(
    tx: &mut Transaction<'static, Postgres>,
    workspace: Uuid,
    account: Uuid,
) -> Result<(), OpError> {
    let member = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2 AND enabled)",
    )
    .bind(workspace)
    .bind(account)
    .fetch_one(&mut **tx)
    .await
    .map_err(AppError::from)?;
    if member {
        Ok(())
    } else {
        Err(OpError::not_found("That account"))
    }
}

async fn may_edit(call: &mut OpCall<'_>, account: Uuid) -> Result<bool, OpError> {
    if account == call.account_id() {
        return Ok(true);
    }
    let caller = call.ctx.caller.clone();
    let tx = call.tx().await?;
    Ok(access::may_manage_account(tx, &caller, account)
        .await?
        .is_ok())
}

async fn profile(call: &mut OpCall<'_>, account: Uuid) -> Result<ProfileView, OpError> {
    let workspace = call.ctx.workspace_id;
    let viewer = call.account_id();
    let can_edit = may_edit(call, account).await?;
    let tx = call.tx().await?;
    member_of(tx, workspace, account).await?;
    let row = sqlx::query_as::<_, ProfileRow>(
        r#"
        SELECT actor.account_kind AS kind,
               (SELECT min(membership.display_name) FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                WHERE membership.actor_id = actor.id AND project.organization_id = $2) AS name,
               (SELECT min(membership.handle) FROM project_memberships membership
                JOIN projects project ON project.id = membership.project_id
                WHERE membership.actor_id = actor.id AND project.organization_id = $2) AS handle,
               profile.title, profile.pronouns, profile.time_zone,
               CASE WHEN profile.avatar IS NOT NULL THEN profile.avatar_version END
                   AS avatar_version
        FROM actors actor
        LEFT JOIN account_profiles profile ON profile.account_id = actor.id
        WHERE actor.id = $1
        "#,
    )
    .bind(account)
    .bind(workspace)
    .fetch_optional(&mut **tx)
    .await
    .map_err(AppError::from)?
    .ok_or_else(|| OpError::not_found("That account"))?;
    // Spaces of this workspace only, that the viewer may read and the
    // account is in; never anyone's personal space.
    let spaces = sqlx::query_as::<_, (Uuid, String, String)>(&format!(
        r#"
        SELECT space.id, space.name, space.kind
        FROM projects space
        WHERE space.organization_id = $1
          AND space.archived_at IS NULL
          AND {readable}
          AND EXISTS (SELECT 1 FROM project_memberships membership
                      WHERE membership.project_id = space.id
                        AND membership.actor_id = $3 AND membership.enabled)
        ORDER BY lower(space.name), space.id
        LIMIT 100
        "#,
        readable = crate::spaces::access::readable_sql("space.id", "$2"),
    ))
    .bind(workspace)
    .bind(viewer)
    .bind(account)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let shared_spaces = spaces
        .into_iter()
        .filter(|(_, _, kind)| {
            SpaceKind::from_stored(kind).is_some_and(|kind| kind != SpaceKind::Personal)
        })
        .map(|(id, name, _)| SharedSpace { id, name })
        .collect();
    let statuses = crate::statuses::shown_in(tx, workspace, &[account])
        .await?
        .remove(&account)
        .unwrap_or_default();
    Ok(ProfileView {
        account_id: account,
        kind: row.kind,
        name: row.name.unwrap_or_default(),
        handle: row.handle.unwrap_or_default(),
        title: row.title,
        pronouns: row.pronouns,
        time_zone: row.time_zone,
        avatar_url: row
            .avatar_version
            .map(|version| avatar_url(account, version)),
        statuses,
        shared_spaces,
        can_edit,
    })
}

// ---------------------------------------------------------------------------
// profile_get

pub struct ProfileGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileGetInput {
    /// The account: its id. Left out, your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for ProfileGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for ProfileGet {
    const NAME: &'static str = "profile_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get profile";
    const DESCRIPTION: &'static str = "Someone's profile in this workspace (a person's or an \
        agent's): name, handle, title, pronouns, time zone, avatar, status, and the spaces here \
        you're both in. Left without an account, your own.";

    type Input = ProfileGetInput;
    type Output = ProfileView;

    async fn run(call: &mut OpCall<'_>, input: ProfileGetInput) -> Result<ProfileView, OpError> {
        let account = input.account.map_or_else(|| call.account_id(), IdRef::id);
        profile(call, account).await
    }
}

// ---------------------------------------------------------------------------
// profile_update

pub struct ProfileUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileUpdateInput {
    /// What you do, up to 80 characters; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Up to 40 characters, such as she/her; "" clears them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// A time zone such as Europe/Berlin; "" clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    /// A new avatar: a PNG, JPEG, GIF or WebP of at most 256 KB, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_base64: Option<String>,
    /// true takes the avatar away (initials show again).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_avatar: bool,
    /// An agent you manage. Left out, your own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<IdRef>,
}

impl Targeted for ProfileUpdateInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// A line of text, trimmed: `None` leaves it, `Some(None)` clears it.
fn line(
    value: Option<&str>,
    max: usize,
    field: &'static str,
    message: &'static str,
) -> Result<Option<Option<String>>, OpError> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(Some(None));
    }
    if value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(OpError::invalid(field, message));
    }
    Ok(Some(Some(value.to_owned())))
}

impl Operation for ProfileUpdate {
    const NAME: &'static str = "profile_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update profile";
    const DESCRIPTION: &'static str = "Change your profile (or an agent's you manage): title, \
        pronouns, timeZone, a new avatar (avatarBase64: PNG, JPEG, GIF or WebP, at most \
        256 KB) or removeAvatar. Fields left out stay; \"\" clears one. It shows in every \
        workspace you're in.";
    const EMITS: &'static [&'static str] = &["account.profile_changed"];

    type Input = ProfileUpdateInput;
    type Output = ProfileView;

    async fn run(call: &mut OpCall<'_>, input: ProfileUpdateInput) -> Result<ProfileView, OpError> {
        let title = line(
            input.title.as_deref(),
            MAX_TITLE,
            "title",
            "A title is up to 80 characters on one line.",
        )?;
        let pronouns = line(
            input.pronouns.as_deref(),
            MAX_PRONOUNS,
            "pronouns",
            "Pronouns are up to 40 characters on one line.",
        )?;
        let time_zone = match input.time_zone.as_deref().map(str::trim) {
            None => None,
            Some("") => Some(None),
            Some(zone) => {
                if crate::usage::schedule::parse_time_zone(zone).is_none() {
                    return Err(OpError::invalid(
                        "timeZone",
                        "Use a time zone such as Europe/Berlin.",
                    ));
                }
                Some(Some(zone.to_owned()))
            }
        };
        let avatar = match input.avatar_base64.as_deref().map(str::trim) {
            None => None,
            Some(_) if input.remove_avatar => {
                return Err(OpError::invalid(
                    "removeAvatar",
                    "Give a new avatar or remove it, not both.",
                ));
            }
            Some(encoded) => {
                if encoded.len() > MAX_AVATAR_BYTES * 4 / 3 + 8 {
                    return Err(OpError::invalid(
                        "avatarBase64",
                        "An avatar is at most 256 KB.",
                    ));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| OpError::invalid("avatarBase64", "Pass the image as base64."))?;
                if bytes.len() > MAX_AVATAR_BYTES {
                    return Err(OpError::invalid(
                        "avatarBase64",
                        "An avatar is at most 256 KB.",
                    ));
                }
                let kind = avatar_type(&bytes).ok_or_else(|| {
                    OpError::invalid("avatarBase64", "Use a PNG, JPEG, GIF or WebP image.")
                })?;
                Some((bytes, kind))
            }
        };
        let own = call.account_id();
        let account = input.account.map_or(own, IdRef::id);
        let workspace = call.ctx.workspace_id;
        {
            let tx = call.tx().await?;
            member_of(tx, workspace, account).await?;
        }
        if !may_edit(call, account).await? {
            return Err(OpError::invalid(
                "account",
                "You can change only your own profile, or that of an agent you manage.",
            ));
        }
        let tx = call.tx().await?;
        let (avatar_bytes, avatar_kind) = match &avatar {
            Some((bytes, kind)) => (Some(bytes.as_slice()), Some(*kind)),
            None => (None, None),
        };
        sqlx::query(
            r#"
            INSERT INTO account_profiles AS profile
                (account_id, title, pronouns, time_zone, avatar, avatar_type, avatar_version)
            VALUES ($1, $3, $5, $7,
                    CASE WHEN $10 THEN NULL ELSE $8 END,
                    CASE WHEN $10 THEN NULL ELSE $9 END,
                    CASE WHEN $8::bytea IS NULL THEN 0 ELSE 1 END)
            ON CONFLICT (account_id) DO UPDATE SET
                title = CASE WHEN $2 THEN $3 ELSE profile.title END,
                pronouns = CASE WHEN $4 THEN $5 ELSE profile.pronouns END,
                time_zone = CASE WHEN $6 THEN $7 ELSE profile.time_zone END,
                avatar = CASE WHEN $10 THEN NULL WHEN $8::bytea IS NOT NULL THEN $8
                              ELSE profile.avatar END,
                avatar_type = CASE WHEN $10 THEN NULL WHEN $8::bytea IS NOT NULL THEN $9
                                   ELSE profile.avatar_type END,
                avatar_version = profile.avatar_version
                    + CASE WHEN $8::bytea IS NOT NULL THEN 1 ELSE 0 END,
                updated_at = now()
            "#,
        )
        .bind(account)
        .bind(title.is_some())
        .bind(title.clone().flatten())
        .bind(pronouns.is_some())
        .bind(pronouns.clone().flatten())
        .bind(time_zone.is_some())
        .bind(time_zone.clone().flatten())
        .bind(avatar_bytes)
        .bind(avatar_kind)
        .bind(input.remove_avatar)
        .execute(&mut **tx)
        .await
        .map_err(AppError::from)?;
        // Everyone who sees it, in every workspace it's in, reads it again.
        let workspaces = sqlx::query_scalar::<_, Uuid>(
            "SELECT organization_id FROM organization_memberships \
             WHERE actor_id = $1 AND enabled ORDER BY organization_id LIMIT 50",
        )
        .bind(account)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        for workspace in workspaces {
            let space = crate::spaces::workspace::ensure(tx, workspace).await?;
            crate::outbox::enqueue_typed(
                tx,
                space,
                crate::outbox::OutboxEventPayload::AccountProfileChanged(
                    crate::outbox::AccountProfileChangedPayload {
                        account_id: account,
                    },
                ),
            )
            .await?;
        }
        let fields: Vec<&str> = [
            title.is_some().then_some("title"),
            pronouns.is_some().then_some("pronouns"),
            time_zone.is_some().then_some("timeZone"),
            (avatar.is_some() || input.remove_avatar).then_some("avatar"),
        ]
        .into_iter()
        .flatten()
        .collect();
        call.audit_target("account", account, json!({ "fields": fields }));
        profile(call, account).await
    }
}

// ---------------------------------------------------------------------------
// profile_avatar_list

pub struct ProfileAvatarList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileAvatarListInput {}

impl Targeted for ProfileAvatarListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct AvatarLine {
    #[schemars(with = "String")]
    pub account_id: Uuid,
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AvatarList {
    pub avatars: Vec<AvatarLine>,
}

impl Operation for ProfileAvatarList {
    const NAME: &'static str = "profile_avatar_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List avatars";
    const DESCRIPTION: &'static str = "The avatar image of each account in this workspace that \
        has one, by account id: the addresses to show beside their names.";

    type Input = ProfileAvatarListInput;
    type Output = AvatarList;

    async fn run(
        call: &mut OpCall<'_>,
        _input: ProfileAvatarListInput,
    ) -> Result<AvatarList, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let rows = sqlx::query_as::<_, (Uuid, i32)>(
            r#"
            SELECT profile.account_id, profile.avatar_version
            FROM account_profiles profile
            JOIN organization_memberships membership
              ON membership.actor_id = profile.account_id
             AND membership.organization_id = $1
             AND membership.enabled
            WHERE profile.avatar IS NOT NULL
            ORDER BY profile.account_id
            LIMIT 5000
            "#,
        )
        .bind(workspace)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        Ok(AvatarList {
            avatars: rows
                .into_iter()
                .map(|(account_id, version)| AvatarLine {
                    account_id,
                    url: avatar_url(account_id, version),
                })
                .collect(),
        })
    }
}
