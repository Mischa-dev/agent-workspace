//! MCP resources and skills (K2.20). A describe operation that names a
//! resource address (`Operation::RESOURCE`, such as a page's) reads as a
//! resource; each space's guide page is listed; each space's skills are
//! served through the skills extension (`skills/list`, `skills/get`, and
//! their SKILL.md over `resources/read`). Every list and read runs the
//! operations as the caller, so access, modules and roles decide exactly as
//! the same tools/call would, and a refusal reads as not found.

use rmcp::model::{
    ErrorData, Resource, ResourceContents, ResourceTemplate, Skill, SkillFrontmatter,
    SkillResource, SkillResources,
};
use serde_json::{Map, Value, json};

use crate::{AppState, mcp::McpPrincipal, ops::Edge};

/// The scheme of this server's resource addresses.
pub const SCHEME: &str = "workspace://";

/// Where a skill's SKILL.md reads: `workspace://skills/{pageId}/{name}/SKILL.md`.
const SKILLS: &str = "skills/";
const SKILL_MD: &str = "SKILL.md";

/// At most this many spaces are walked for a list, and this many skills
/// listed; a caller with more names them by address.
const MAX_SPACES: usize = 50;
const MAX_SKILLS: usize = 100;

/// One operation's value, run as the caller; the refusal otherwise.
async fn value_of(
    state: &AppState,
    principal: &McpPrincipal,
    edge: &Edge,
    operation: &str,
    arguments: Value,
) -> Result<Value, Value> {
    let arguments = match arguments {
        Value::Object(arguments) => arguments,
        _ => Map::new(),
    };
    let result = crate::ops::mcp::call(
        state,
        principal,
        None,
        edge.clone(),
        None,
        operation,
        Some(arguments),
    )
    .await;
    let mut envelope = result.structured_content.unwrap_or(Value::Null);
    if envelope["outcome"] == "ok" {
        Ok(envelope["value"].take())
    } else {
        Err(envelope)
    }
}

/// A refusal or failure, as the read's error. Not found and refused read
/// the same, so an address says nothing of what the caller can't see.
fn read_error(uri: &str, envelope: &Value) -> ErrorData {
    match envelope["outcome"].as_str() {
        Some("invalid_request") => ErrorData::invalid_params(
            envelope["message"]
                .as_str()
                .unwrap_or("That address isn't valid.")
                .to_owned(),
            None,
        ),
        Some("unavailable") | Some("internal_error") => {
            ErrorData::internal_error("That can't be read right now.", None)
        }
        _ => ErrorData::resource_not_found(
            "There's no resource at that address you can read.",
            Some(json!({ "uri": uri })),
        ),
    }
}

/// The resource templates: each operation's address, and skills'.
pub fn templates() -> Vec<ResourceTemplate> {
    crate::ops::registry()
        .entries()
        .filter_map(|entry| {
            let address = entry.meta.resource?;
            Some(
                ResourceTemplate::new(format!("{SCHEME}{}", address.path), entry.meta.name)
                    .with_title(entry.meta.title)
                    .with_mime_type(address.mime_type),
            )
        })
        .chain([ResourceTemplate::new(
            format!("{SCHEME}{SKILLS}{{pageId}}/{{name}}/{SKILL_MD}"),
            "skill",
        )
        .with_title("Skill")
        .with_description("A space's skill as a SKILL.md file (skills/list lists them).")
        .with_mime_type("text/markdown")])
        .collect()
}

/// A page's address.
fn page_uri(page_id: &str) -> String {
    format!("{SCHEME}pages/{page_id}")
}

/// A skill's SKILL.md address.
fn skill_uri(page_id: &str, name: &str) -> String {
    format!("{SCHEME}{SKILLS}{page_id}/{name}/{SKILL_MD}")
}

/// The page and name a SKILL.md address names.
fn skill_of(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix(SCHEME)?.strip_prefix(SKILLS)?;
    let rest = rest.strip_suffix(SKILL_MD)?.strip_suffix('/')?;
    let (page, name) = rest.split_once('/')?;
    (!page.is_empty() && !name.is_empty() && !name.contains('/')).then_some((page, name))
}

/// The operation and its input an address names, from the operations'
/// templates (`pages/{pageId}`).
fn operation_of(uri: &str) -> Option<(&'static str, crate::ops::ResourceAddress, Value)> {
    let path = uri.strip_prefix(SCHEME)?;
    crate::ops::registry().entries().find_map(|entry| {
        let address = entry.meta.resource?;
        let (prefix, rest) = address.path.split_once('{')?;
        let (field, suffix) = rest.split_once('}')?;
        let id = path.strip_prefix(prefix)?.strip_suffix(suffix)?;
        if id.is_empty() || id.contains(['/', '?', '#']) {
            return None;
        }
        let mut input = Map::new();
        input.insert(field.to_owned(), Value::String(id.to_owned()));
        Some((entry.meta.name, address, Value::Object(input)))
    })
}

/// A space's pages, as the caller may list them.
struct SpacePages {
    space_name: String,
    pages: Vec<Value>,
}

/// The spaces the caller works in (as `project_list` lists them, without
/// the Messages space), each with its pages as `page_list` lists them. A
/// space whose pages are off or hidden to the caller is left out.
async fn spaces_with_pages(
    state: &AppState,
    principal: &McpPrincipal,
    edge: &Edge,
) -> Vec<SpacePages> {
    let Ok(spaces) = value_of(state, principal, edge, "project_list", json!({})).await else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for space in spaces["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|space| space["messagesSpace"] != true)
        .take(MAX_SPACES)
    {
        let Some(id) = space["id"].as_str() else {
            continue;
        };
        if let Ok(pages) = value_of(
            state,
            principal,
            edge,
            "page_list",
            json!({ "projectId": id }),
        )
        .await
        {
            found.push(SpacePages {
                space_name: space["name"].as_str().unwrap_or_default().to_owned(),
                pages: pages["pages"].as_array().cloned().unwrap_or_default(),
            });
        }
    }
    found
}

/// `resources/list`: each space's guide page.
pub async fn list(state: &AppState, principal: &McpPrincipal, edge: &Edge) -> Vec<Resource> {
    spaces_with_pages(state, principal, edge)
        .await
        .into_iter()
        .flat_map(|space| {
            space
                .pages
                .into_iter()
                .filter(|page| page["isGuide"] == true)
                .filter_map(move |page| {
                    let id = page["id"].as_str()?;
                    let title = page["title"].as_str().unwrap_or_default();
                    Some(
                        Resource::new(page_uri(id), title)
                            .with_title(title)
                            .with_description(format!("The guide to {}", space.space_name))
                            .with_mime_type("text/markdown"),
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// `resources/read`: one operation's thing, or a skill's SKILL.md.
pub async fn read(
    state: &AppState,
    principal: &McpPrincipal,
    edge: &Edge,
    uri: &str,
) -> Result<ResourceContents, ErrorData> {
    if let Some((page_id, name)) = skill_of(uri) {
        let skill_md = skill_md(state, principal, edge, uri, page_id, name).await?;
        return Ok(ResourceContents::text(skill_md, uri).with_mime_type("text/markdown"));
    }
    let Some((operation, address, input)) = operation_of(uri) else {
        return Err(ErrorData::resource_not_found(
            "There's no resource at that address you can read.",
            Some(json!({ "uri": uri })),
        ));
    };
    let value = value_of(state, principal, edge, operation, input)
        .await
        .map_err(|envelope| read_error(uri, &envelope))?;
    let text = match address.text_at.and_then(|at| value.pointer(at)) {
        Some(Value::String(text)) => text.clone(),
        _ => serde_json::to_string_pretty(&value).unwrap_or_default(),
    };
    Ok(ResourceContents::text(text, uri).with_mime_type(address.mime_type))
}

/// The SKILL.md a skill address names, read as the caller with `skill_get`.
async fn skill_md(
    state: &AppState,
    principal: &McpPrincipal,
    edge: &Edge,
    uri: &str,
    page_id: &str,
    name: &str,
) -> Result<String, ErrorData> {
    let value = value_of(
        state,
        principal,
        edge,
        "skill_get",
        json!({ "pageId": page_id }),
    )
    .await
    .map_err(|envelope| read_error(uri, &envelope))?;
    if value["name"] != name {
        return Err(read_error(uri, &Value::Null));
    }
    Ok(value["skillMd"].as_str().unwrap_or_default().to_owned())
}

/// One skill's entry: its SKILL.md with the digest of what `resources/read`
/// serves.
fn skill_entry(page_id: &str, name: &str, description: &str, skill_md: &str) -> Skill {
    let uri = skill_uri(page_id, name);
    Skill::new(
        uri.clone(),
        SkillFrontmatter::new(name, description),
        SkillResources::Files(vec![SkillResource::from_bytes(uri, skill_md.as_bytes())]),
    )
}

/// A skill's name fits an address as it is: letters, digits and hyphens.
fn addressable(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `skills/list`: every skill in the spaces the caller works in.
pub async fn skills(state: &AppState, principal: &McpPrincipal, edge: &Edge) -> Vec<Skill> {
    let mut skills = Vec::new();
    for space in spaces_with_pages(state, principal, edge).await {
        for page in space
            .pages
            .iter()
            .filter(|page| page["skillDescription"].is_string())
        {
            if skills.len() >= MAX_SKILLS {
                return skills;
            }
            let Some(page_id) = page["id"].as_str() else {
                continue;
            };
            let Ok(skill) = value_of(
                state,
                principal,
                edge,
                "skill_get",
                json!({ "pageId": page_id }),
            )
            .await
            else {
                continue;
            };
            let name = skill["name"].as_str().unwrap_or_default();
            if !addressable(name) {
                continue;
            }
            skills.push(skill_entry(
                page_id,
                name,
                skill["description"].as_str().unwrap_or_default(),
                skill["skillMd"].as_str().unwrap_or_default(),
            ));
        }
    }
    skills
}

/// `skills/get`: one skill by its SKILL.md address.
pub async fn skill(
    state: &AppState,
    principal: &McpPrincipal,
    edge: &Edge,
    uri: &str,
) -> Result<Skill, ErrorData> {
    let Some((page_id, name)) = skill_of(uri) else {
        return Err(ErrorData::invalid_params(
            "A skill's address ends in /SKILL.md (skills/list lists them).",
            None,
        ));
    };
    let value = value_of(
        state,
        principal,
        edge,
        "skill_get",
        json!({ "pageId": page_id }),
    )
    .await
    .map_err(|envelope| read_error(uri, &envelope))?;
    if value["name"] != name {
        return Err(read_error(uri, &Value::Null));
    }
    Ok(skill_entry(
        page_id,
        name,
        value["description"].as_str().unwrap_or_default(),
        value["skillMd"].as_str().unwrap_or_default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_read_back() {
        let uri = skill_uri("abc", "deploy-steps");
        assert_eq!(skill_of(&uri), Some(("abc", "deploy-steps")));
        assert_eq!(skill_of("workspace://skills/abc/SKILL.md"), None);
        assert_eq!(skill_of("workspace://skills/abc/a/b/SKILL.md"), None);
        assert_eq!(skill_of("workspace://pages/abc"), None);
        assert_eq!(rmcp::model::skill_name_from_uri(&uri), Some("deploy-steps"));
        let (operation, _, input) = operation_of(&page_uri("abc")).expect("a page");
        assert_eq!(operation, "page_get");
        assert_eq!(input, json!({ "pageId": "abc" }));
        assert!(operation_of("workspace://pages/abc/def").is_none());
        assert!(operation_of("workspace://nothing/abc").is_none());
    }

    #[test]
    fn a_listed_skill_is_well_formed() {
        let entry = skill_entry(
            "abc",
            "deploy",
            "When deploying",
            "---\nname: deploy\n---\n",
        );
        entry.validate().expect("valid");
        assert!(addressable("deploy-2"));
        assert!(!addressable("déploy"));
        assert!(!addressable(""));
    }
}
