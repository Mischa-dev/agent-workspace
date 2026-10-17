//! W19: email. A workspace owner sets how the workspace sends mail (SMTP
//! first) in Settings › Workspace › Email; the server then emails password
//! reset links, address confirmations and notifications.
//!
//! The rest of the server only sees [`Transport`] and [`Message`]: which
//! provider sends is a detail of its module (`smtp`), so a hosted version
//! can add an API provider without touching the callers. The provider's
//! password is an encrypted workspace secret, read only to send.

pub mod http;
pub mod smtp;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgExecutor};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    secrets::{self, SecretOwner, SecretValue},
};

/// The workspace secret that holds the SMTP password.
pub const SMTP_PASSWORD_SECRET: &str = "mail.smtp_password";

/// Why a send didn't happen, in words an owner can act on. Never holds a
/// credential.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum MailError {
    /// Worth trying again later (the server is down or busy).
    Retry(String),
    /// Trying again won't help until the settings change.
    Permanent(String),
}

impl MailError {
    pub fn message(&self) -> &str {
        match self {
            Self::Retry(message) | Self::Permanent(message) => message,
        }
    }
}

impl From<MailError> for crate::jobs::JobError {
    fn from(error: MailError) -> Self {
        match error {
            MailError::Retry(message) => Self::retry(message),
            MailError::Permanent(message) => Self::permanent(message),
        }
    }
}

// ---------------------------------------------------------------------------
// Addresses and messages

/// An email address, checked enough that it can't break a header or an
/// SMTP command: one `@`, no spaces, controls, `<>`, quotes or commas.
pub fn normalize_address(value: &str) -> Option<String> {
    let value = value.trim();
    let (local, domain) = value.rsplit_once('@')?;
    let allowed = |c: char| {
        c.is_ascii_graphic()
            && !matches!(
                c,
                '<' | '>' | '"' | ',' | ';' | '(' | ')' | '[' | ']' | '\\' | '@'
            )
    };
    let valid = (3..=254).contains(&value.len())
        && (1..=64).contains(&local.len())
        && local.chars().all(allowed)
        && !local.starts_with('.')
        && !local.ends_with('.')
        && (1..=253).contains(&domain.len())
        && domain.contains('.')
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-']);
    valid.then(|| format!("{local}@{}", domain.to_ascii_lowercase()))
}

/// Mail to one person. Plain text: links work, nothing to render or track.
#[derive(Clone, Debug)]
pub struct Message {
    pub to: String,
    pub subject: String,
    pub text: String,
}

/// A header value: as is when it's plain ASCII, else RFC 2047 base64
/// words. Line breaks are removed so a value can't add a header.
fn header_text(value: &str) -> String {
    let value: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if value.chars().all(|c| c.is_ascii() && !c.is_control()) && !value.contains("=?") {
        return value;
    }
    // Each encoded word holds at most 45 bytes, cut on character edges.
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in value.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(chunk.as_bytes())));
    }
    words.join("\r\n ")
}

fn display_name(name: &str) -> String {
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | '\''))
    {
        format!("\"{name}\"")
    } else {
        header_text(name)
    }
}

/// The whole message as RFC 5322 text, body in base64 so any text and
/// line length survives every server.
pub fn render(
    from: &str,
    from_name: Option<&str>,
    message: &Message,
    now: DateTime<Utc>,
) -> Vec<u8> {
    let domain = from
        .rsplit_once('@')
        .map_or("localhost", |(_, domain)| domain);
    let from_header = match from_name {
        Some(name) => format!("{} <{from}>", display_name(name)),
        None => format!("<{from}>"),
    };
    let text = message.text.replace("\r\n", "\n").replace('\n', "\r\n");
    let encoded = STANDARD.encode(text.as_bytes());
    let mut body = String::with_capacity(encoded.len() + encoded.len() / 76 * 2 + 2);
    for line in encoded.as_bytes().chunks(76) {
        body.push_str(std::str::from_utf8(line).unwrap_or_default());
        body.push_str("\r\n");
    }
    let headers = [
        format!("From: {from_header}"),
        format!("To: <{}>", message.to),
        format!("Subject: {}", header_text(&message.subject)),
        format!("Date: {}", now.to_rfc2822()),
        format!("Message-ID: <{}@{domain}>", Uuid::now_v7().simple()),
        "MIME-Version: 1.0".to_owned(),
        "Content-Type: text/plain; charset=utf-8".to_owned(),
        "Content-Transfer-Encoding: base64".to_owned(),
        "Auto-Submitted: auto-generated".to_owned(),
    ];
    let mut output = headers.join("\r\n");
    output.push_str("\r\n\r\n");
    output.push_str(&body);
    output.into_bytes()
}

// ---------------------------------------------------------------------------
// A workspace's transport

#[derive(Clone, Debug)]
pub enum Provider {
    Smtp(smtp::Settings),
}

impl Provider {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Smtp(_) => "smtp",
        }
    }

    fn parse(provider: &str, config: &Value) -> Option<Self> {
        match provider {
            "smtp" => serde_json::from_value(config.clone()).ok().map(Self::Smtp),
            _ => None,
        }
    }
}

/// How one workspace sends mail.
pub struct Transport {
    pub organization_id: Uuid,
    pub from_address: String,
    pub from_name: Option<String>,
    pub provider: Provider,
    password: Option<SecretValue>,
}

#[derive(FromRow)]
pub(crate) struct SettingsRow {
    pub organization_id: Uuid,
    pub provider: String,
    pub provider_config: Value,
    pub from_address: String,
    pub from_name: Option<String>,
    pub enabled: bool,
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test_ok: Option<bool>,
    pub last_test_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn settings_row<'e, E: PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<Option<SettingsRow>, AppError> {
    Ok(sqlx::query_as::<_, SettingsRow>(
        r#"
        SELECT organization_id, provider, provider_config, from_address, from_name, enabled,
               last_test_at, last_test_ok, last_test_error, updated_at
        FROM workspace_mail_settings WHERE organization_id = $1
        "#,
    )
    .bind(organization_id)
    .fetch_optional(executor)
    .await?)
}

/// Whether the workspace can send mail now (set up and turned on).
pub async fn configured<'e, E: PgExecutor<'e>>(
    executor: E,
    organization_id: Uuid,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM workspace_mail_settings WHERE organization_id = $1 AND enabled)",
    )
    .bind(organization_id)
    .fetch_one(executor)
    .await?)
}

/// Whether any workspace on this server can send mail, so sign-in can
/// offer to email a reset link.
pub async fn any_configured<'e, E: PgExecutor<'e>>(executor: E) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM workspace_mail_settings WHERE enabled)",
    )
    .fetch_one(executor)
    .await?)
}

/// The workspace's transport, with its password, or None when mail isn't
/// set up or is turned off.
pub async fn transport(
    state: &AppState,
    organization_id: Uuid,
) -> Result<Option<Transport>, AppError> {
    let Some(row) = settings_row(&state.pool, organization_id).await? else {
        return Ok(None);
    };
    if !row.enabled {
        return Ok(None);
    }
    load(state, row).await.map(Some)
}

pub(crate) async fn load(state: &AppState, row: SettingsRow) -> Result<Transport, AppError> {
    let provider = Provider::parse(&row.provider, &row.provider_config)
        .ok_or(AppError::Internal("stored mail settings"))?;
    let password = match secrets::find(
        &state.pool,
        SecretOwner::Workspace {
            organization_id: row.organization_id,
        },
        SMTP_PASSWORD_SECRET,
    )
    .await?
    {
        Some(id) => {
            let keys =
                secrets::keyring(&state.config).map_err(|_| AppError::SecretsNotConfigured)?;
            Some(secrets::reveal(&state.pool, keys.as_ref(), id, "mail.send").await?)
        }
        None => None,
    };
    Ok(Transport {
        organization_id: row.organization_id,
        from_address: row.from_address,
        from_name: row.from_name,
        provider,
        password,
    })
}

/// The transport for mail to a person outside any one workspace (password
/// resets, address confirmations): their default workspace's, else the
/// first workspace they belong to that sends mail.
pub async fn transport_for_actor(
    state: &AppState,
    actor_id: Uuid,
) -> Result<Option<Transport>, AppError> {
    let default = crate::workspace::default_workspace(&state.pool, actor_id)
        .await?
        .map(|(organization_id, _)| organization_id);
    let organization_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT membership.organization_id
        FROM organization_memberships membership
        JOIN workspace_mail_settings mail
          ON mail.organization_id = membership.organization_id AND mail.enabled
        WHERE membership.actor_id = $1 AND membership.enabled
        ORDER BY (membership.organization_id = $2) DESC, membership.created_at, membership.organization_id
        LIMIT 1
        "#,
    )
    .bind(actor_id)
    .bind(default)
    .fetch_optional(&state.pool)
    .await?;
    match organization_id {
        Some(organization_id) => transport(state, organization_id).await,
        None => Ok(None),
    }
}

/// The name we greet mail servers with: this server's public host.
fn hello_name(state: &AppState) -> String {
    url::Url::parse(&crate::events::public_origin(state))
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .filter(|host| {
            host.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
        })
        .unwrap_or_else(|| "localhost".to_owned())
}

/// Send one message.
pub async fn send(
    state: &AppState,
    transport: &Transport,
    message: &Message,
) -> Result<(), MailError> {
    let to = normalize_address(&message.to)
        .ok_or_else(|| MailError::Permanent("that email address isn't valid".into()))?;
    let message = Message {
        to,
        ..message.clone()
    };
    let data = render(
        &transport.from_address,
        transport.from_name.as_deref(),
        &message,
        Utc::now(),
    );
    match &transport.provider {
        Provider::Smtp(settings) => {
            smtp::send(
                settings,
                transport.password.as_ref().map(SecretValue::expose),
                &smtp::Envelope {
                    from: &transport.from_address,
                    to: &message.to,
                    data: &data,
                    hello: &hello_name(state),
                },
            )
            .await
        }
    }
}

/// A person's confirmed email address, if they have one.
pub async fn verified_address_for_actor<'e, E: PgExecutor<'e>>(
    executor: E,
    actor_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT address.address
        FROM user_email_addresses address
        JOIN users user_account ON user_account.id = address.user_id
        WHERE user_account.actor_id = $1 AND user_account.status = 'active'
          AND address.verified_at IS NOT NULL
        "#,
    )
    .bind(actor_id)
    .fetch_optional(executor)
    .await?)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn addresses_that_could_break_a_header_are_refused() {
        assert_eq!(
            normalize_address(" Mischa@Example.COM "),
            Some("Mischa@example.com".into())
        );
        for bad in [
            "no-at-sign",
            "a@b",
            "a b@example.com",
            "a@example.com\r\nBcc: x@y.z",
            "<a@example.com>",
            "a@exa mple.com",
            "\"quoted\"@example.com",
            "a,b@example.com",
            ".a@example.com",
            "a@-example.com",
        ] {
            assert_eq!(normalize_address(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn rendered_messages_keep_headers_on_one_line_and_encode_the_body() {
        let message = Message {
            to: "you@example.com".into(),
            subject: "Reset\r\nBcc: evil@example.com".into(),
            text: "Hi\n.\nGrüße".into(),
        };
        let at = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let rendered = String::from_utf8(render(
            "me@example.com",
            Some("Agent Workspace"),
            &message,
            at,
        ))
        .expect("ascii");
        assert!(rendered.starts_with("From: \"Agent Workspace\" <me@example.com>\r\n"));
        assert!(rendered.contains("Subject: Reset  Bcc: evil@example.com\r\n"));
        assert!(!rendered.contains("\r\nBcc:"));
        let body = rendered
            .split("\r\n\r\n")
            .nth(1)
            .expect("body")
            .replace("\r\n", "");
        assert_eq!(
            STANDARD.decode(body).expect("base64"),
            "Hi\r\n.\r\nGrüße".as_bytes()
        );
    }

    #[test]
    fn non_ascii_subjects_become_encoded_words() {
        let encoded = header_text("Grüße aus Berlin");
        assert!(encoded.starts_with("=?UTF-8?B?"));
        assert!(!encoded.contains('ü'));
    }
}
