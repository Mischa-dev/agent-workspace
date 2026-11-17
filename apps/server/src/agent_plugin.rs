//! The agent plugin (A2.17): `packages/agent-plugin`, embedded at build
//! time. It serves as `/agent-plugin.zip` with this server's address filled
//! in and the catalogue of events agents hear, and its `recipes/` are the
//! setup steps for each client an agent can run in: the setup link and the
//! web's "Set it up in" list both read them, so adding a client is adding a
//! file there, not code.

use axum::{
    Json, Router,
    extract::State,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::AppState;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/agent_plugin.rs"));
}

/// How to connect one client, from `recipes/<file>.md`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipe {
    pub id: String,
    pub title: String,
    #[serde(skip)]
    pub order: i64,
    #[serde(skip)]
    pub body: String,
}

/// Splits `---` front matter from a file: its `key: value` lines and the
/// body after it.
fn front_matter(text: &str) -> Option<(Vec<(&str, &str)>, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let (head, body) = rest.split_once("\n---\n")?;
    let fields = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    Some((fields, body.trim_start_matches('\n')))
}

fn field<'a>(fields: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, value)| *value)
}

/// Every client recipe, in their `order`.
pub fn recipes() -> Vec<Recipe> {
    let mut recipes: Vec<Recipe> = embedded::FILES
        .iter()
        .filter(|(path, _)| path.starts_with("recipes/") && path.ends_with(".md"))
        .filter_map(|(_, text)| {
            let (fields, body) = front_matter(text)?;
            Some(Recipe {
                id: field(&fields, "id")?.to_owned(),
                title: field(&fields, "title")?.to_owned(),
                order: field(&fields, "order")
                    .and_then(|order| order.parse().ok())
                    .unwrap_or(100),
                body: body.to_owned(),
            })
        })
        .collect();
    recipes.sort_by(|left, right| left.order.cmp(&right.order).then(left.id.cmp(&right.id)));
    recipes
}

/// The recipe for one client, if there is one.
pub fn recipe(id: &str) -> Option<Recipe> {
    recipes().into_iter().find(|recipe| recipe.id == id)
}

/// A recipe's steps for `origin`, with `token` in them.
pub fn steps(recipe: &Recipe, origin: &str, token: &str) -> String {
    recipe
        .body
        .replace("{mcp}", &format!("{origin}/api/v1/mcp"))
        .replace("{origin}", origin)
        .replace("{token}", token)
}

/// The skills in the plugin: each folder's name and its text.
pub fn skills() -> impl Iterator<Item = (&'static str, &'static str)> {
    embedded::FILES.iter().filter_map(|(path, text)| {
        let name = path.strip_prefix("skills/")?.strip_suffix("/SKILL.md")?;
        Some((name, *text))
    })
}

fn version() -> String {
    format!(
        "{}+{}",
        env!("CARGO_PKG_VERSION"),
        crate::health::BUILD_COMMIT
    )
}

/// The plugin as a zip, for `origin`.
pub fn package(origin: &str) -> Vec<u8> {
    let parts = parts(origin);
    let borrowed: Vec<(&str, Vec<u8>)> = parts
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.clone()))
        .collect();
    crate::page_docs::zip::write(&borrowed)
}

/// The plugin's files for `origin`: templates filled in, recipes left out
/// (they're for setting up, which the setup link does), and the events
/// catalogue added.
fn parts(origin: &str) -> Vec<(String, Vec<u8>)> {
    let origin = origin.trim_end_matches('/');
    let fill = |text: &str| {
        text.replace("{origin}", origin)
            .replace("{version}", &version())
    };
    let mut parts: Vec<(String, Vec<u8>)> = embedded::FILES
        .iter()
        .filter(|(path, _)| !path.starts_with("recipes/"))
        .map(|(path, text)| {
            let path = path.strip_suffix(".tmpl").unwrap_or(path);
            (format!("agent-workspace/{path}"), fill(text).into_bytes())
        })
        .collect();
    let catalogue =
        serde_json::to_vec_pretty(crate::delivery::catalogue::ENTRIES).unwrap_or_default();
    parts.push((
        "agent-workspace/events/catalogue.json".to_owned(),
        catalogue,
    ));
    parts
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agent-plugin.zip", get(download))
        .route("/api/v1/agent-recipes", get(list))
}

/// Nothing in it is secret, so anyone may fetch it.
async fn download(State(state): State<AppState>) -> Response {
    let origin = crate::events::public_origin(&state);
    (
        [
            (header::CONTENT_TYPE, "application/zip"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"agent-workspace-plugin.zip\"",
            ),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        package(&origin),
    )
        .into_response()
}

/// The clients a setup link can be made for.
async fn list() -> Json<Vec<Recipe>> {
    Json(recipes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_recipe_is_well_formed_and_has_its_token() {
        let recipes = recipes();
        assert!(recipes.iter().any(|recipe| recipe.id == "other"));
        for recipe in &recipes {
            assert!(!recipe.title.is_empty(), "{} has a title", recipe.id);
            let filled = steps(recipe, "https://w.test", "aw_ext_x");
            assert!(filled.contains("aw_ext_x"), "{} uses the token", recipe.id);
            assert!(!filled.contains("{mcp}") && !filled.contains("{origin}"));
        }
    }

    #[test]
    fn every_skill_names_tools_that_exist() {
        // Current names only: an old name still answers, but skills teach
        // the one that stays.
        let known: std::collections::BTreeSet<String> = crate::mcp::tool_catalog()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let mut count = 0;
        for (name, text) in skills() {
            count += 1;
            let (fields, body) = front_matter(text).expect("front matter");
            assert_eq!(field(&fields, "name"), Some(name), "{name}'s name");
            assert!(field(&fields, "description").is_some_and(|text| !text.is_empty()));
            for word in body.split('`').skip(1).step_by(2) {
                let looks_like_tool = word.contains('_')
                    && word
                        .chars()
                        .all(|char| char.is_ascii_lowercase() || char == '_');
                if looks_like_tool {
                    assert!(
                        known.contains(word),
                        "{name} names `{word}`, which isn't a tool"
                    );
                }
            }
        }
        assert!(count >= 5);
    }

    #[test]
    fn the_package_has_the_address_and_no_template_marks() {
        let parts = parts("https://w.test/");
        let file = |name: &str| {
            parts
                .iter()
                .find(|(path, _)| path.ends_with(name))
                .map(|(_, bytes)| String::from_utf8_lossy(bytes).into_owned())
                .unwrap_or_else(|| panic!("no {name}"))
        };
        assert!(file("mcp.json").contains("https://w.test/api/v1/mcp"));
        assert!(file("catalogue.json").contains("reply.requested"));
        assert!(parts.iter().all(|(path, bytes)| !path.contains("recipes/")
            && !String::from_utf8_lossy(bytes).contains("{origin}")));
        assert!(!package("https://w.test").is_empty());
    }
}
