//! Themes (Lane C): one document of design tokens for light and dark, each
//! starting from a built-in theme, plus optional CSS. The theme contract,
//! `apps/web/src/theme/contract.json`, says which tokens a theme may set,
//! which built-in themes it may start from and what its CSS may use; the
//! web app reads the same file, and `docs/theming.md` explains it.
//!
//! Everything here is checked before it is stored, because a workspace can
//! offer a theme to everyone in it: a token value or a stylesheet must not
//! load anything from elsewhere (no `url()`, `@import` or `@font-face`), so
//! a theme can't call out of the browser or leak what's on screen.
//!
//! `theme_list`, `theme_get`, `theme_create`, `theme_update`,
//! `theme_delete` and `theme_offer` (in `ops.rs`) are the ways in.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::AppError, ops::OpError};

pub mod ops;

/// The theme contract, as `contract.json` writes it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContractFile {
    version: u32,
    presets: Vec<ContractPreset>,
    groups: Vec<ContractGroup>,
    value_functions: Vec<String>,
    css_functions: Vec<String>,
    css_at_rules: Vec<String>,
    limits: ContractLimits,
}

#[derive(Debug, Deserialize)]
struct ContractPreset {
    id: String,
    scheme: String,
}

#[derive(Debug, Deserialize)]
struct ContractGroup {
    tokens: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ContractLimits {
    name: usize,
    value: usize,
    css: usize,
    document: usize,
}

/// What the contract allows, read once.
pub struct Contract {
    pub version: u32,
    /// Built-in theme id to its scheme (`light` or `dark`).
    presets: BTreeMap<String, String>,
    preset_order: Vec<String>,
    tokens: BTreeSet<String>,
    value_functions: BTreeSet<String>,
    css_functions: BTreeSet<String>,
    css_at_rules: BTreeSet<String>,
    pub name_limit: usize,
    value_limit: usize,
    css_limit: usize,
    pub document_limit: usize,
}

pub static CONTRACT: LazyLock<Contract> = LazyLock::new(|| {
    let file: ContractFile =
        serde_json::from_str(include_str!("../../web/src/theme/contract.json"))
            .expect("the theme contract is valid JSON");
    Contract {
        version: file.version,
        preset_order: file
            .presets
            .iter()
            .map(|preset| preset.id.clone())
            .collect(),
        presets: file
            .presets
            .into_iter()
            .map(|preset| (preset.id, preset.scheme))
            .collect(),
        tokens: file
            .groups
            .into_iter()
            .flat_map(|group| group.tokens)
            .collect(),
        value_functions: lower(file.value_functions),
        css_functions: lower(file.css_functions),
        css_at_rules: lower(file.css_at_rules),
        name_limit: file.limits.name,
        value_limit: file.limits.value,
        css_limit: file.limits.css,
        document_limit: file.limits.document,
    }
});

impl Contract {
    /// The built-in themes' ids, in the contract's order.
    pub fn preset_ids(&self) -> Vec<String> {
        self.preset_order.clone()
    }
}

/// A refusal naming the field and what to change.
pub(crate) fn invalid(field: impl Into<String>, message: impl Into<String>) -> OpError {
    OpError::Invalid {
        field: std::borrow::Cow::Owned(field.into()),
        message: std::borrow::Cow::Owned(message.into()),
    }
}

fn lower(values: Vec<String>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.to_lowercase())
        .collect()
}

/// A theme: its light and dark modes, and CSS for both.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeDocument {
    /// The contract version it was written for: 1.
    pub format: u32,
    /// The theme in light mode.
    pub light: ThemeMode,
    /// The theme in dark mode.
    pub dark: ThemeMode,
    /// Extra CSS, applied in both modes. It may target the published part
    /// names (classes such as `aw-card`, see the contract) and use the
    /// tokens as `var(--name)`; it can't load anything (no `url()`,
    /// `@import` or `@font-face`) or use backslash escapes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub css: String,
}

/// One mode of a theme.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeMode {
    /// The built-in theme it starts from, such as `quiet` (light) or
    /// `quiet-dark` (dark); see the contract's presets.
    pub base: String,
    /// Token name (without the leading `--`, such as `accent` or
    /// `radius-md`) to its CSS value, such as `oklch(0.5 0.19 277)` or
    /// `#3355ff` or `6px`. Only the contract's tokens; values may use
    /// `var(--token)` and colour and math functions, nothing that loads.
    #[serde(default)]
    pub tokens: BTreeMap<String, String>,
}

impl ThemeDocument {
    /// Checks the document against the contract; an error names the field
    /// and says what to change.
    pub fn check(&self) -> Result<(), OpError> {
        let contract = &*CONTRACT;
        if self.format != contract.version {
            return Err(invalid(
                "document.format",
                format!("Write the theme for format {}.", contract.version),
            ));
        }
        for (field, mode, scheme) in [
            ("light", &self.light, "light"),
            ("dark", &self.dark, "dark"),
        ] {
            match contract.presets.get(&mode.base) {
                None => {
                    return Err(invalid(
                        format!("document.{field}.base"),
                        format!(
                            "Start from a built-in theme: one of {}.",
                            contract
                                .presets
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                Some(found) if found != scheme => {
                    return Err(invalid(
                        format!("document.{field}.base"),
                        format!("Start the {field} mode from a {scheme} built-in theme."),
                    ));
                }
                Some(_) => {}
            }
            for (name, value) in &mode.tokens {
                if !contract.tokens.contains(name) {
                    return Err(invalid(
                        format!("document.{field}.tokens.{name}"),
                        "That isn't a theme token: use one the contract lists, without the --.",
                    ));
                }
                check_value(value).map_err(|problem| {
                    invalid(format!("document.{field}.tokens.{name}"), problem)
                })?;
            }
        }
        check_css(&self.css).map_err(|problem| invalid("document.css", problem))?;
        let size = serde_json::to_vec(self).map_or(usize::MAX, |bytes| bytes.len());
        if size > contract.document_limit {
            return Err(OpError::TooLarge {
                limit_bytes: contract.document_limit,
            });
        }
        Ok(())
    }
}

/// The function names a piece of CSS calls, lowercased: every name right
/// before a `(`.
fn functions(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            current.push(character);
        } else {
            // A bare ( is grouping, as in @media (…) or calc((…)).
            if character == '(' && !current.is_empty() {
                names.push(current.to_lowercase());
            }
            current.clear();
        }
    }
    names
}

/// Quotes that open and close, and brackets that balance.
fn balanced(text: &str) -> Result<(), &'static str> {
    let mut quote: Option<char> = None;
    let mut stack = Vec::new();
    for character in text.chars() {
        match (quote, character) {
            (Some(open), close) if close == open => quote = None,
            (Some(_), '\n') => return Err("Close every quote on the line it opens."),
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(character),
            (None, '(' | '{' | '[') => stack.push(character),
            (None, ')') if stack.pop() != Some('(') => return Err("A ) has no ( before it."),
            (None, '}') if stack.pop() != Some('{') => return Err("A } has no { before it."),
            (None, ']') if stack.pop() != Some('[') => return Err("A ] has no [ before it."),
            (None, _) => {}
        }
    }
    if quote.is_some() {
        return Err("Close every quote.");
    }
    if !stack.is_empty() {
        return Err("Close every bracket.");
    }
    Ok(())
}

/// One token's value: short, one declaration, only the contract's
/// functions, and `var()` only of theme tokens.
pub fn check_value(value: &str) -> Result<(), String> {
    let contract = &*CONTRACT;
    let value = value.trim();
    if value.is_empty() {
        return Err("Give it a value, or leave the token out.".into());
    }
    if value.chars().count() > contract.value_limit {
        return Err(format!(
            "Keep a value to {} characters.",
            contract.value_limit
        ));
    }
    if let Some(bad) = value.chars().find(|character| {
        matches!(character, ';' | '{' | '}' | '<' | '>' | '\\' | '@' | '!')
            || character.is_control()
    }) {
        return Err(format!("A value can't contain {bad:?}."));
    }
    if value.contains("/*") {
        return Err("A value can't contain a comment.".into());
    }
    balanced(value)?;
    for function in functions(value) {
        if !contract.value_functions.contains(&function) {
            return Err(format!(
                "A value can't call {function}(): use colors, lengths and math (calc, color-mix, var)."
            ));
        }
    }
    let mut rest = value;
    while let Some(start) = rest.find("--") {
        let name: String = rest[start + 2..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect();
        if !contract.tokens.contains(&name) && name != "text-scale" {
            return Err(format!("var(--{name}) isn't a theme token."));
        }
        rest = &rest[start + 2 + name.len()..];
    }
    Ok(())
}

/// Removes `/* … */` comments (outside strings), so the checks see what the
/// browser will read.
fn strip_comments(css: &str) -> Result<String, &'static str> {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(character) = chars.next() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
                out.push(character);
            }
            None if character == '/' && chars.peek() == Some(&'*') => {
                chars.next();
                let mut closed = false;
                while let Some(inner) = chars.next() {
                    if inner == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err("Close every comment.");
                }
                out.push(' ');
            }
            None => {
                if character == '"' || character == '\'' {
                    quote = Some(character);
                }
                out.push(character);
            }
        }
    }
    Ok(out)
}

/// A theme's CSS: within the size limit, no backslash escapes or markup, only
/// the contract's at-rules and functions (none that load anything).
pub fn check_css(css: &str) -> Result<(), String> {
    let contract = &*CONTRACT;
    if css.is_empty() {
        return Ok(());
    }
    if css.len() > contract.css_limit {
        return Err(format!("Keep the CSS to {} KB.", contract.css_limit / 1024));
    }
    if css.contains('\\') {
        return Err("The CSS can't use backslash escapes.".into());
    }
    if css.contains('<') {
        return Err("The CSS can't contain <: write > and + for selectors, and no markup.".into());
    }
    if let Some(bad) = css
        .chars()
        .find(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(format!("The CSS can't contain {bad:?}."));
    }
    let css = strip_comments(css)?;
    balanced(&css)?;
    let mut rest = css.as_str();
    while let Some(start) = rest.find('@') {
        let name: String = rest[start + 1..]
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect::<String>()
            .to_lowercase();
        if !contract.css_at_rules.contains(&name) {
            return Err(format!(
                "The CSS can't use @{name}: only @media, @supports, @container, @layer and @keyframes."
            ));
        }
        rest = &rest[start + 1 + name.len()..];
    }
    for function in functions(&css) {
        if !contract.css_functions.contains(&function) {
            return Err(format!(
                "The CSS can't call {function}(): a theme can't load anything (no url())."
            ));
        }
    }
    Ok(())
}

/// A theme as lists show it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummary {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// Offered to everyone in the workspace.
    pub offered: bool,
    /// You made it, so you can change it.
    pub mine: bool,
    #[schemars(with = "String")]
    pub owner_account_id: Uuid,
    /// The built-in themes its light and dark modes start from.
    pub light_base: String,
    pub dark_base: String,
    /// Pass it as expectedVersion to theme_update.
    pub version: i64,
    #[schemars(with = "String")]
    pub updated_at: DateTime<Utc>,
}

/// A theme with its document.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThemeView {
    #[serde(flatten)]
    pub summary: ThemeSummary,
    pub document: ThemeDocument,
}

#[derive(sqlx::FromRow)]
struct ThemeRow {
    id: Uuid,
    name: String,
    offered: bool,
    owner_account_id: Uuid,
    document: serde_json::Value,
    version: i64,
    updated_at: DateTime<Utc>,
}

pub(crate) const ROW_COLUMNS: &str =
    "id, name, offered, owner_account_id, document, version, updated_at";

impl ThemeRow {
    fn view(self, viewer: Uuid) -> Result<ThemeView, AppError> {
        let document: ThemeDocument = serde_json::from_value(self.document)
            .map_err(|_| AppError::Internal("a stored theme doesn't read back"))?;
        Ok(ThemeView {
            summary: ThemeSummary {
                id: self.id,
                name: self.name,
                offered: self.offered,
                mine: self.owner_account_id == viewer,
                owner_account_id: self.owner_account_id,
                light_base: document.light.base.clone(),
                dark_base: document.dark.base.clone(),
                version: self.version,
                updated_at: self.updated_at,
            },
            document,
        })
    }
}

/// A theme the viewer can see in `workspace_id`: their own, or one the
/// workspace offers. `lock` takes it for a change.
pub(crate) async fn visible(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    viewer: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Option<ThemeView>, AppError> {
    let row = sqlx::query_as::<_, ThemeRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM themes \
         WHERE id = $1 AND workspace_id = $2 AND (owner_account_id = $3 OR offered) {}",
        if lock { "FOR UPDATE" } else { "" }
    ))
    .bind(id)
    .bind(workspace_id)
    .bind(viewer)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|row| row.view(viewer)).transpose()
}

/// A theme by id in `workspace_id`, whoever's it is: only after an access
/// decision that lets the viewer act on it.
pub(crate) async fn by_id(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    viewer: Uuid,
    id: Uuid,
) -> Result<Option<ThemeView>, AppError> {
    let row = sqlx::query_as::<_, ThemeRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM themes WHERE id = $1 AND workspace_id = $2"
    ))
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(|row| row.view(viewer)).transpose()
}

/// The most themes one list shows.
pub const MAX_LISTED: i64 = 200;

/// The viewer's own themes and the ones the workspace offers, by name.
pub(crate) async fn list(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    viewer: Uuid,
) -> Result<Vec<ThemeSummary>, AppError> {
    let rows = sqlx::query_as::<_, ThemeRow>(&format!(
        "SELECT {ROW_COLUMNS} FROM themes \
         WHERE workspace_id = $1 AND (owner_account_id = $2 OR offered) \
         ORDER BY lower(name), id LIMIT $3"
    ))
    .bind(workspace_id)
    .bind(viewer)
    .bind(MAX_LISTED)
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter()
        .map(|row| row.view(viewer).map(|view| view.summary))
        .collect()
}

/// Tells whoever can see the theme that it changed, in the change's
/// transaction: its owner's other devices always, and everyone in the
/// workspace when it is (or just stopped being) offered. Ids only.
pub(crate) async fn announce(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    owner_account_id: Uuid,
    theme_id: Uuid,
    everyone: bool,
) -> Result<(), AppError> {
    let space = crate::spaces::workspace::ensure(transaction, workspace_id).await?;
    crate::outbox::enqueue_typed(
        transaction,
        space,
        crate::outbox::OutboxEventPayload::ThemeChanged(crate::outbox::ThemeChangedPayload {
            actor_id: owner_account_id,
            theme_id,
        }),
    )
    .await?;
    if everyone {
        crate::outbox::enqueue_typed(
            transaction,
            space,
            crate::outbox::OutboxEventPayload::WorkspaceThemesChanged(
                crate::outbox::WorkspaceThemesChangedPayload {
                    workspace_id,
                    project_id: space,
                    theme_id,
                },
            ),
        )
        .await?;
    }
    Ok(())
}

/// A theme made already offered to the workspace, by a package install
/// (M1.9): its installer owns it, and the install checked the workspace's
/// settings permission. The document is checked first.
pub(crate) async fn insert_offered(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    account: Uuid,
    name: &str,
    document: &ThemeDocument,
) -> Result<Uuid, AppError> {
    let id = Uuid::now_v7();
    let body = serde_json::to_value(document)
        .map_err(|_| AppError::Internal("serialize an installed theme"))?;
    sqlx::query(
        "INSERT INTO themes (id, workspace_id, owner_account_id, name, document, offered, \
         offered_by_account_id) VALUES ($1, $2, $3, $4, $5, true, $3)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(account)
    .bind(name)
    .bind(&body)
    .execute(&mut **transaction)
    .await?;
    announce(transaction, workspace_id, account, id, true).await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(css: &str, tokens: &[(&str, &str)]) -> ThemeDocument {
        ThemeDocument {
            format: 1,
            light: ThemeMode {
                base: "quiet".into(),
                tokens: tokens
                    .iter()
                    .map(|(name, value)| ((*name).into(), (*value).into()))
                    .collect(),
            },
            dark: ThemeMode {
                base: "quiet-dark".into(),
                tokens: BTreeMap::new(),
            },
            css: css.into(),
        }
    }

    #[test]
    fn tokens_take_colours_lengths_and_math() {
        for value in [
            "#3355ff",
            "oklch(0.5 0.19 277)",
            "color-mix(in oklch, var(--accent) 12%, var(--content-bg))",
            "6px",
            "0 1px 2px #1b1a3a0f",
            "\"Inter\", ui-sans-serif, system-ui",
            "600 calc(26px * var(--text-scale)) / calc(34px * var(--text-scale)) var(--font-display)",
            "cubic-bezier(0.22, 1, 0.36, 1)",
        ] {
            assert_eq!(check_value(value), Ok(()), "{value}");
        }
    }

    #[test]
    fn tokens_never_load_or_break_out() {
        for value in [
            "url(https://example.com/x.png)",
            "image-set(\"x.png\" 1x)",
            "red; background: red",
            "red } body { color: red",
            "var(--sidebar-width)",
            "\\75 rl(x)",
            "red !important",
            "attr(data-secret)",
            "",
        ] {
            assert!(check_value(value).is_err(), "{value}");
        }
    }

    #[test]
    fn css_styles_parts_without_loading_anything() {
        assert_eq!(
            check_css(
                ".aw-card { border: 0; background: color-mix(in oklch, var(--accent) 5%, transparent); }\n\
                 @media (max-width: 600px) { .aw-sidebar { width: 100%; } }\n\
                 /* a comment with url(x) inside is fine */\n\
                 .aw-btn:not(:disabled):hover { transform: translateY(-1px); }"
            ),
            Ok(())
        );
        for css in [
            "@import 'https://example.com/x.css';",
            ".aw-card { background: url(https://example.com/x.png); }",
            ".aw-card { background: URL(x); }",
            "@font-face { font-family: x; src: local(x); }",
            ".aw-card { background: u\\72l(x); }",
            ".aw-card { background: image-set('x.png' 1x); }",
            "</style><script>alert(1)</script>",
            ".aw-card { color: red;",
            "/* never closed",
            "input[value^='a'] { background: url(//example.com/a); }",
        ] {
            assert!(check_css(css).is_err(), "{css}");
        }
    }

    #[test]
    fn documents_start_from_a_built_in_theme_of_their_mode() {
        assert!(document("", &[("accent", "#123456")]).check().is_ok());
        let mut wrong = document("", &[]);
        wrong.light.base = "quiet-dark".into();
        assert!(wrong.check().is_err());
        let mut unknown = document("", &[]);
        unknown.dark.base = "nowhere".into();
        assert!(unknown.check().is_err());
        assert!(document("", &[("sidebar-width", "10px")]).check().is_err());
        let mut future = document("", &[]);
        future.format = 2;
        assert!(future.check().is_err());
    }
}
