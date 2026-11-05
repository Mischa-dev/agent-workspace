//! AWD-75: modules.
//!
//! Every feature is a module with a manifest (`modules/<id>.json` at the
//! root of the repository): what it is, what it needs, its permissions,
//! tools, routes, events, things, embeds, UI slots and settings. Core
//! modules (Messages, Channels, Tasks, Pages, Whiteboards, Runs, Code,
//! Inbox, Automations and the rest) fill in the same contract an extension
//! will; their only privilege is running in this process.
//!
//! Four layers decide what someone has, narrowest last: modules decide what
//! exists in a workspace, roles what each person may do (AWD-74), agent
//! limits what their agents may do, and layouts what each person sees.
//! Modules are switched with `modules.<id>` settings at workspace scope;
//! off means hidden, never deleted.
//!
//! - [`manifest`]: the manifest format and the core manifests.
//! - [`contract`]: the shared contract (things, links, embeds, fields,
//!   views, pieces, events and the agent tool pattern).
//! - [`state`]: which modules are on, for a workspace and for a person.
//! - [`things`]: tasks, pages, whiteboards and threads as things, and the
//!   links between them, read both ways.
//! - [`http`]: the Modules settings page's routes.
//!
//! The guard tests below fail when any route, tool, layout part, setting,
//! setup step, event, notification, search kind or automation trigger
//! doesn't name its module; the web app's own guard covers its routes,
//! navigation, palette, shortcuts, slash commands and settings pages.

pub mod contract;
pub mod http;
pub mod manifest;
pub mod state;
pub mod things;

pub use http::router;
pub use manifest::{Manifest, core};
pub use state::ModuleSet;

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::manifest::{self, Manifest, core, pattern_match};

    /// Every value claimed by `list` across the manifests, with who claims it.
    fn claims(list: fn(&Manifest) -> &[String]) -> BTreeMap<String, Vec<&'static str>> {
        let mut found: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
        for module in core() {
            for value in list(module) {
                found.entry(value.clone()).or_default().push(&module.id);
            }
        }
        found
    }

    /// Every one of `values` must be claimed by a module's `list`, and every
    /// exact entry there must be one of `values`.
    fn each_named(what: &str, values: &BTreeSet<String>, list: fn(&Manifest) -> &[String]) {
        let unnamed = values
            .iter()
            .filter(|value| manifest::owner_by(list, value).is_none())
            .collect::<Vec<_>>();
        assert!(
            unnamed.is_empty(),
            "these {what} don't name a module; add each to the manifest of the module it \
             belongs to (modules/<id>.json): {unnamed:#?}"
        );
        let claimed = claims(list);
        let twice = claimed
            .iter()
            .filter(|(_, owners)| owners.len() > 1)
            .collect::<Vec<_>>();
        assert!(twice.is_empty(), "{what} claimed twice: {twice:#?}");
        let stale = claimed
            .keys()
            .filter(|pattern| {
                !values
                    .iter()
                    .any(|value| pattern_match(pattern, value).is_some())
            })
            .collect::<Vec<_>>();
        assert!(
            stale.is_empty(),
            "{what} in manifests that don't exist: {stale:#?}"
        );
    }

    #[test]
    fn every_core_manifest_parses_and_is_listed() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules");
        let files = std::fs::read_dir(&folder)
            .expect("modules folder")
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                let name = path.file_stem()?.to_str()?.to_owned();
                (path.extension()? == "json" && name != "presets").then_some(name)
            })
            .collect::<BTreeSet<_>>();
        let parsed = core()
            .iter()
            .map(|module| module.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            parsed, files,
            "every manifest in modules/ parses and is in CORE_SOURCES"
        );
        assert!(core().len() <= manifest::MAX_CORE);
    }

    #[test]
    fn manifests_are_well_formed() {
        let ids = core()
            .iter()
            .map(|module| module.id.as_str())
            .collect::<BTreeSet<_>>();
        let things = core()
            .iter()
            .flat_map(|module| module.things.iter().map(|thing| thing.kind.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            things.len(),
            things.iter().collect::<BTreeSet<_>>().len(),
            "a kind of thing is declared twice"
        );
        let mut embeds = BTreeSet::new();
        for module in core() {
            assert!(
                module
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "{}",
                module.id
            );
            assert!(!module.name.is_empty() && module.description.ends_with('.'));
            assert_eq!(module.kind, manifest::ModuleKind::Core);
            assert_eq!(module.switchable, module.id != "workspace", "{}", module.id);
            for need in module.needs.iter().chain(&module.works_with) {
                assert!(ids.contains(need.as_str()), "{} names {need}", module.id);
                assert_ne!(need, &module.id);
            }
            for link in &module.links {
                assert!(
                    things.contains(&link.from.as_str()),
                    "{}: {}",
                    module.id,
                    link.from
                );
                assert!(
                    things.contains(&link.to.as_str()),
                    "{}: {}",
                    module.id,
                    link.to
                );
            }
            for embed in &module.embeds {
                assert!(
                    embed.id.starts_with(&format!("{}.", module.id)),
                    "embed {} isn't named after {}",
                    embed.id,
                    module.id
                );
                assert!(embeds.insert(embed.id.clone()), "embed {} twice", embed.id);
                assert_eq!(embed.thing, embed.piece.thing);
                let decl = manifest::thing_decl(&embed.thing).expect("embed's thing is declared");
                for field in &embed.piece.fields {
                    assert!(
                        ["title", "status", "owner"].contains(&field.as_str())
                            || decl.fields.iter().any(|known| &known.id == field),
                        "{}: {} has no field {field}",
                        embed.id,
                        embed.thing
                    );
                }
            }
        }
        // Needs never go round in a circle: each module's needs come before
        // it in CORE_SOURCES.
        for (index, module) in core().iter().enumerate() {
            for need in &module.needs {
                assert!(
                    manifest::index_of(need).is_some_and(|need| need < index),
                    "{} needs {need}, which must come first",
                    module.id
                );
            }
        }
        for preset in manifest::presets() {
            for module in &preset.on {
                let found = manifest::get(module).expect("preset names a module");
                assert!(found.switchable, "{} can't be switched", module);
            }
        }
        for id in ["everything", "chat_with_agents", "pages_only", "dms_only"] {
            assert!(manifest::preset(id).is_some(), "preset {id}");
        }
    }

    #[test]
    fn every_mcp_tool_names_its_module() {
        let tools = crate::mcp::tool_catalog()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<BTreeSet<_>>();
        let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for module in core() {
            for tool in module.tools.keys() {
                owners.entry(tool).or_default().push(&module.id);
            }
        }
        let unnamed = tools
            .iter()
            .filter(|tool| !owners.contains_key(tool.as_str()))
            .collect::<Vec<_>>();
        assert!(
            unnamed.is_empty(),
            "these tools don't name a module; add each to the `tools` of the module it \
             belongs to (modules/<id>.json), with what it does: {unnamed:#?}"
        );
        let twice = owners
            .iter()
            .filter(|(_, owners)| owners.len() > 1)
            .collect::<Vec<_>>();
        assert!(twice.is_empty(), "tools in two modules: {twice:#?}");
        let gone = owners
            .keys()
            .filter(|tool| !tools.contains(**tool))
            .collect::<Vec<_>>();
        assert!(
            gone.is_empty(),
            "manifests name tools that don't exist: {gone:#?}"
        );
    }

    #[test]
    fn every_route_names_its_module() {
        let routes = route_paths();
        assert!(routes.len() > 100, "found only {} routes", routes.len());
        each_named("routes", &routes, |module| &module.routes);
    }

    #[test]
    fn every_layout_part_names_its_module() {
        let registry: serde_json::Value =
            serde_json::from_str(include_str!("../../../web/src/experience/registry.json"))
                .expect("layout registry");
        let parts = registry["parts"]
            .as_array()
            .expect("parts")
            .iter()
            .filter_map(|part| part["id"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>();
        each_named("layout parts", &parts, |module| &module.slots.parts);
        let nouns = registry["nouns"]
            .as_array()
            .expect("nouns")
            .iter()
            .filter_map(|noun| noun["id"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>();
        each_named("nouns", &nouns, |module| &module.nouns);
    }

    #[test]
    fn every_permission_names_its_module() {
        let keys = crate::policy::PERMISSIONS
            .iter()
            .map(|info| info.key.to_owned())
            .collect::<BTreeSet<_>>();
        each_named("permissions", &keys, |module| &module.permissions);
        // The Roles page groups permissions by AWD-74's groups; each group is
        // one module, and members and settings are the workspace's own.
        for info in crate::policy::PERMISSIONS {
            let owner =
                manifest::owner_by(|module| &module.permissions, info.key).expect("claimed above");
            let expected = match info.module {
                "members" | "settings" => "workspace",
                group => group,
            };
            assert_eq!(
                owner.id, expected,
                "{} is in the {} group on the Roles page but in the {} manifest",
                info.key, info.module, owner.id
            );
        }
    }

    #[test]
    fn role_modules_off_never_reach_the_workspace_itself() {
        // A role narrows only modules that can be switched off; the
        // workspace stays.
        let set = super::state::narrowed(
            &super::state::Switches::default(),
            &["workspace".to_owned(), "runs".to_owned()],
        );
        assert!(set.contains("workspace"));
        assert!(!set.contains("runs"));
        assert!(set.contains("tasks"));
        // A role can't turn on what the workspace has off.
        let mut switches = super::state::Switches::default();
        switches.values.insert("tasks".to_owned(), (false, 1));
        let set = super::state::narrowed(&switches, &[]);
        assert!(!set.contains("tasks") && !set.contains("runs"));
    }

    #[test]
    fn every_setting_names_its_module() {
        let keys = crate::settings::REGISTRY
            .iter()
            .map(|definition| definition.key.to_owned())
            .collect::<BTreeSet<_>>();
        each_named("settings", &keys, |module| &module.settings);
    }

    #[test]
    fn every_module_switch_is_a_setting() {
        for module in core() {
            let key = super::state::setting_key(&module.id);
            let definition = crate::settings::definition(&key);
            assert_eq!(definition.is_some(), module.switchable, "{key}");
            if let Some(definition) = definition {
                assert_eq!(definition.scopes, &[crate::settings::Scope::Workspace]);
                assert_eq!(definition.default_value(), serde_json::json!(true));
            }
        }
    }

    #[test]
    fn every_setup_step_names_its_module() {
        let steps = crate::onboarding::STEPS
            .iter()
            .map(|step| (*step).to_owned())
            .collect::<BTreeSet<_>>();
        each_named("setup steps", &steps, |module| &module.slots.setup);
    }

    #[test]
    fn every_event_names_its_module() {
        let events = crate::events::EVENT_TYPES
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect::<BTreeSet<_>>();
        each_named("events", &events, |module| &module.events.emits);
        for module in core() {
            for event in &module.events.listens {
                assert!(events.contains(event), "{} listens to {event}", module.id);
            }
        }
    }

    #[test]
    fn every_notification_kind_names_its_module() {
        use crate::notifications::kind;
        let mut kinds = [
            kind::MENTION,
            kind::COMMENT_MENTION,
            kind::ASSIGNED,
            kind::APPROVAL_REQUESTED,
            kind::RUN_WAITING,
            kind::RUN_COMPLETED,
            kind::RUN_FAILED,
            kind::CHECKS_FAILED,
            kind::REVIEW_REQUESTED,
            kind::TASK_CHANGED,
            kind::CONVERSATION_MESSAGE,
            kind::REMINDER,
            kind::SCHEDULED_SEND_FAILED,
            crate::automations::NOTIFICATION_KIND,
            "automation_disabled",
            "ai_suggestions",
            "ai_plan_import",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        kinds.extend(
            crate::usage::alerts::kind::ALL
                .iter()
                .map(|kind| (*kind).to_owned()),
        );
        each_named("notification kinds", &kinds, |module| &module.notifications);
    }

    #[test]
    fn every_search_kind_names_its_module() {
        let kinds = crate::search::SearchKind::ALL
            .iter()
            .map(|kind| kind.name().to_owned())
            .collect::<BTreeSet<_>>();
        each_named("search kinds", &kinds, |module| &module.search);
    }

    #[test]
    fn every_automation_trigger_and_action_names_its_module() {
        let triggers = crate::automations::TRIGGER_KINDS
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect::<BTreeSet<_>>();
        each_named("automation triggers", &triggers, |module| {
            &module.automations.triggers
        });
        let actions = crate::automations::ACTION_KINDS
            .iter()
            .map(|kind| (*kind).to_owned())
            .collect::<BTreeSet<_>>();
        each_named("automation actions", &actions, |module| {
            &module.automations.actions
        });
    }

    // -----------------------------------------------------------------------
    // Reading the routes from the source, as `policy_inventory` does.

    const ROUTE_CALL: &str = concat!(".route", "(");

    fn route_paths() -> BTreeSet<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut folders = vec![root];
        let mut paths = BTreeSet::new();
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).expect("read source folder") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                let mut source = std::fs::read_to_string(&path).expect("read source file");
                // Test modules build throwaway routers.
                if let Some(tests) = source.find("\n#[cfg(test)]\nmod ") {
                    source.truncate(tests);
                }
                let mut remaining = source.as_str();
                while let Some(start) = remaining.find(ROUTE_CALL) {
                    remaining = &remaining[start + ROUTE_CALL.len()..];
                    let Some(open) = remaining.find('"') else {
                        break;
                    };
                    // The path is the call's first argument.
                    if !remaining[..open].trim().is_empty() {
                        continue;
                    }
                    let rest = &remaining[open + 1..];
                    if let Some(close) = rest.find('"') {
                        paths.insert(rest[..close].to_owned());
                    }
                }
            }
        }
        paths
    }
}
