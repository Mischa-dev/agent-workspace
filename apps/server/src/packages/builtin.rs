//! The packages that come with the server (M1.11, M1.17): the six
//! workspace templates that were `modules/templates/*.json` and the five
//! shapes a workspace starts from, as files under `modules/packages/`.
//! They are compiled in, always listed in the directory as "built in", and
//! installed like any other package.

use std::sync::OnceLock;

use serde_json::Value;

use super::format::{Package, to_template};
use crate::workspace_templates::TemplateDefinition;

/// One built-in package.
#[derive(Debug)]
pub struct Builtin {
    /// Its folder under `modules/packages/` (`shapes/friends`).
    pub folder: &'static str,
    pub package: Package,
    /// The package sealed (with its digest), as an install stores it.
    pub document: Value,
    /// A shape: what a workspace is for, chosen at first run (M1.17).
    pub shape: bool,
}

impl Builtin {
    /// The template this package was, for the template list and routes.
    pub fn template(&self) -> Option<TemplateDefinition> {
        (!self.shape)
            .then(|| to_template(&self.package))
            .flatten()
    }
}

/// Every built-in package's folder and file, templates first (Everything
/// first, as the template list had them), then the shapes in the order
/// first run asks.
const SOURCES: &[(&str, &str)] = &[
    (
        "everything",
        include_str!("../../../../modules/packages/everything/package.json"),
    ),
    (
        "chat_with_agents",
        include_str!("../../../../modules/packages/chat_with_agents/package.json"),
    ),
    (
        "personal",
        include_str!("../../../../modules/packages/personal/package.json"),
    ),
    (
        "small_team",
        include_str!("../../../../modules/packages/small_team/package.json"),
    ),
    (
        "software_team",
        include_str!("../../../../modules/packages/software_team/package.json"),
    ),
    (
        "company",
        include_str!("../../../../modules/packages/company/package.json"),
    ),
    (
        "shapes/team",
        include_str!("../../../../modules/packages/shapes/team/package.json"),
    ),
    (
        "shapes/friends",
        include_str!("../../../../modules/packages/shapes/friends/package.json"),
    ),
    (
        "shapes/just_me",
        include_str!("../../../../modules/packages/shapes/just_me/package.json"),
    ),
    (
        "shapes/documents",
        include_str!("../../../../modules/packages/shapes/documents/package.json"),
    ),
    (
        "shapes/blank",
        include_str!("../../../../modules/packages/shapes/blank/package.json"),
    ),
];

/// Every built-in package, in order. A file that doesn't read is left out
/// here; the tests below refuse it.
pub fn all() -> &'static [Builtin] {
    static ALL: OnceLock<Vec<Builtin>> = OnceLock::new();
    ALL.get_or_init(|| {
        SOURCES
            .iter()
            .filter_map(|(folder, source)| {
                let package = serde_json::from_str::<Package>(source).ok()?.sealed();
                let document = serde_json::to_value(&package).ok()?;
                Some(Builtin {
                    folder,
                    shape: folder.starts_with("shapes/"),
                    package,
                    document,
                })
            })
            .collect()
    })
}

/// The built-in packages that were workspace templates, Everything first.
pub fn templates() -> impl Iterator<Item = &'static Builtin> {
    all().iter().filter(|builtin| !builtin.shape)
}

/// The shapes, in the order first run asks.
pub fn shapes() -> impl Iterator<Item = &'static Builtin> {
    all().iter().filter(|builtin| builtin.shape)
}

/// A built-in package by its id (`software_team`, `shapes.friends`).
pub fn find(id: &str) -> Option<&'static Builtin> {
    all().iter().find(|builtin| builtin.package.id == id.trim())
}

/// A shape by its id or its short name (`friends` or `shapes.friends`).
pub fn shape(name: &str) -> Option<&'static Builtin> {
    let name = name.trim().to_lowercase();
    let short = name.strip_prefix("shapes.").unwrap_or(&name);
    shapes().find(|builtin| {
        builtin.package.id.strip_prefix("shapes.") == Some(short)
            || builtin.package.name.eq_ignore_ascii_case(&name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_package_reads_and_validates() {
        assert_eq!(all().len(), SOURCES.len(), "a built-in package didn't read");
        for builtin in all() {
            builtin
                .package
                .validate()
                .unwrap_or_else(|problems| panic!("{}: {problems:?}", builtin.folder));
            assert!(
                super::super::secrets_check::check(&builtin.document).is_ok(),
                "{}",
                builtin.folder
            );
        }
    }

    #[test]
    fn every_folder_in_modules_packages_is_built_in() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../modules/packages");
        let mut found = Vec::new();
        for group in ["", "shapes"] {
            let Ok(entries) = std::fs::read_dir(root.join(group)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.join("package.json").is_file() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    found.push(if group.is_empty() {
                        name.into_owned()
                    } else {
                        format!("{group}/{name}")
                    });
                }
            }
        }
        found.sort();
        let mut listed = SOURCES
            .iter()
            .map(|(folder, _)| (*folder).to_owned())
            .collect::<Vec<_>>();
        listed.sort();
        assert_eq!(found, listed, "list every modules/packages folder in SOURCES");
    }

    #[test]
    fn the_templates_read_back_as_templates() {
        let ids = templates()
            .map(|builtin| builtin.package.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "everything",
                "chat_with_agents",
                "personal",
                "small_team",
                "software_team",
                "company"
            ]
        );
        for builtin in templates() {
            let definition = builtin.template().expect("a template");
            assert!(definition.modules.is_some(), "{}", builtin.folder);
            // Through the converter and back: the same package.
            let again = super::super::format::from_template(
                &builtin.package.id,
                &builtin.package.name,
                &builtin.package.description,
                &definition,
            );
            assert_eq!(again.contents, builtin.package.contents, "{}", builtin.folder);
        }
    }
}
