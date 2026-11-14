//! The module registry: the one place that answers which module owns what.
//!
//! Each core module has a folder, `modules/core/<id>/mod.rs`, with a
//! `CoreModule` registration: its manifest (`modules/<id>.json` at the root
//! of the repository) and a `register` that adds what it brings. `build.rs`
//! lists the folders into `core::ALL`, so adding a module edits no central
//! list. `Registry::build` orders the modules (manifest `order`, then id,
//! with every module after the ones it needs) and refuses a set where two
//! modules claim one tool, a `needs` names an unknown module or goes round
//! in a circle, or an id appears twice.
//!
//! `manifest::core()`, `get` and `of_*` read the registry.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    sync::OnceLock,
};

use super::manifest::Manifest;

/// What every core module implements. The only privilege a core module has
/// is that these functions run in this process.
pub trait CoreModule: Send + Sync + 'static {
    /// The manifest, `include_str!` of `modules/<id>.json`.
    fn manifest_source(&self) -> &'static str;
    /// Adds everything the module brings. Called once, when the registry is
    /// built.
    fn register(&self, module: &mut Registrar<'_>);
}

/// What one module adds, as `register` fills it in. Every contribution is
/// recorded beside the module's id, so nothing is unowned.
#[derive(Default)]
pub struct Contributions {
    /// Adds the module's operations (K2): its `ops::register_ops`.
    pub operations: Vec<fn(&mut crate::ops::Registry)>,
}

/// One module's view of the registry while it registers.
pub struct Registrar<'a> {
    module: &'a str,
    into: &'a mut Contributions,
}

impl Registrar<'_> {
    /// The id of the module registering.
    pub fn id(&self) -> &str {
        self.module
    }

    /// The module's operations: `module.operations(crate::<module>::ops::register_ops)`.
    pub fn operations(&mut self, add: fn(&mut crate::ops::Registry)) {
        self.into.operations.push(add);
    }
}

/// One module in the registry.
pub struct RegisteredModule {
    pub id: String,
    pub contributions: Contributions,
}

/// Why a set of modules can't be put together. The server refuses to start
/// on one; `registry_builds` fails first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryError {
    /// A manifest that doesn't parse.
    Manifest {
        source_index: usize,
        error: String,
    },
    /// A core id must be lower case letters, digits and `_`, no dot.
    BadId {
        module: String,
    },
    Duplicate {
        module: String,
    },
    ToolClaimedTwice {
        tool: String,
        modules: [String; 2],
    },
    UnknownNeed {
        module: String,
        need: String,
    },
    Cycle {
        module: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest {
                source_index,
                error,
            } => write!(formatter, "module {source_index}'s manifest: {error}"),
            Self::BadId { module } => write!(formatter, "{module:?} isn't a core module id"),
            Self::Duplicate { module } => write!(formatter, "two modules are called {module}"),
            Self::ToolClaimedTwice { tool, modules } => write!(
                formatter,
                "{} and {} both claim the tool {tool}",
                modules[0], modules[1]
            ),
            Self::UnknownNeed { module, need } => {
                write!(formatter, "{module} needs {need}, which isn't a module")
            }
            Self::Cycle { module } => write!(formatter, "{module}'s needs go round in a circle"),
        }
    }
}

/// Every module, ordered, with what each owns.
pub struct Registry {
    modules: Vec<RegisteredModule>,
    descriptions: Vec<Manifest>,
    by_id: HashMap<String, usize>,
    tools: HashMap<String, usize>,
}

fn valid_core_id(id: &str) -> bool {
    let mut characters = id.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && id.len() <= 40
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

impl Registry {
    /// Put the modules together. `extra` is for tests: modules from the test
    /// tree, registered the same way.
    pub fn build(
        core: &[&'static dyn CoreModule],
        extra: &[&'static dyn CoreModule],
    ) -> Result<Self, RegistryError> {
        let mut parsed = Vec::new();
        for (source_index, module) in core.iter().chain(extra).enumerate() {
            let manifest =
                serde_json::from_str::<Manifest>(module.manifest_source()).map_err(|error| {
                    RegistryError::Manifest {
                        source_index,
                        error: error.to_string(),
                    }
                })?;
            if !valid_core_id(&manifest.id) {
                return Err(RegistryError::BadId {
                    module: manifest.id,
                });
            }
            parsed.push((manifest, *module));
        }
        let ids = parsed
            .iter()
            .map(|(manifest, _)| manifest.id.clone())
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        for id in &ids {
            if !seen.insert(id.as_str()) {
                return Err(RegistryError::Duplicate { module: id.clone() });
            }
        }
        for (manifest, _) in &parsed {
            if let Some(need) = manifest
                .needs
                .iter()
                .find(|need| !seen.contains(need.as_str()))
            {
                return Err(RegistryError::UnknownNeed {
                    module: manifest.id.clone(),
                    need: need.clone(),
                });
            }
        }

        // Order: by `order`, then id, each module after what it needs.
        parsed.sort_by(|(left, _), (right, _)| {
            (left.order.unwrap_or(u32::MAX), &left.id)
                .cmp(&(right.order.unwrap_or(u32::MAX), &right.id))
        });
        let mut ordered = Vec::with_capacity(parsed.len());
        let mut placed = BTreeSet::<String>::new();
        while !parsed.is_empty() {
            let Some(next) = parsed
                .iter()
                .position(|(manifest, _)| manifest.needs.iter().all(|need| placed.contains(need)))
            else {
                return Err(RegistryError::Cycle {
                    module: parsed[0].0.id.clone(),
                });
            };
            let (manifest, module) = parsed.remove(next);
            placed.insert(manifest.id.clone());
            ordered.push((manifest, module));
        }

        let mut registry = Self {
            modules: Vec::with_capacity(ordered.len()),
            descriptions: Vec::with_capacity(ordered.len()),
            by_id: HashMap::new(),
            tools: HashMap::new(),
        };
        for (index, (manifest, module)) in ordered.into_iter().enumerate() {
            for tool in manifest.tools.keys() {
                if let Some(first) = registry.tools.insert(tool.clone(), index) {
                    return Err(RegistryError::ToolClaimedTwice {
                        tool: tool.clone(),
                        modules: [registry.descriptions[first].id.clone(), manifest.id.clone()],
                    });
                }
            }
            let mut contributions = Contributions::default();
            module.register(&mut Registrar {
                module: &manifest.id,
                into: &mut contributions,
            });
            registry.by_id.insert(manifest.id.clone(), index);
            registry.modules.push(RegisteredModule {
                id: manifest.id.clone(),
                contributions,
            });
            registry.descriptions.push(manifest);
        }
        Ok(registry)
    }

    /// The registry of this server's core modules, built on first use. A set
    /// that doesn't build is a programming error: the server refuses to
    /// start (`serve` reads this first) and `registry_builds` fails before
    /// that.
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<Registry> = OnceLock::new();
        GLOBAL.get_or_init(|| match Self::build(super::core::ALL, &[]) {
            Ok(registry) => registry,
            Err(error) => panic!("the core modules don't fit together: {error}"),
        })
    }

    /// Every module's description, in order.
    pub fn descriptions(&self) -> &[Manifest] {
        &self.descriptions
    }

    pub fn modules(&self) -> &[RegisteredModule] {
        &self.modules
    }

    pub fn describe(&self, id: &str) -> Option<&Manifest> {
        self.index_of(id).map(|index| &self.descriptions[index])
    }

    /// A module's place in the order (its bit in a `ModuleSet`).
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    pub fn module_of_tool(&self, tool: &str) -> Option<&Manifest> {
        self.tools.get(tool).map(|index| &self.descriptions[*index])
    }

    /// Every module's operation registrations, with the module's id.
    pub fn operation_registrations(
        &self,
    ) -> impl Iterator<Item = (&str, fn(&mut crate::ops::Registry))> {
        self.modules.iter().flat_map(|module| {
            module
                .contributions
                .operations
                .iter()
                .map(move |add| (module.id.as_str(), *add))
        })
    }

    /// Module id to the ids it needs, for docs and tests.
    pub fn needs(&self) -> BTreeMap<&str, Vec<&str>> {
        self.descriptions
            .iter()
            .map(|manifest| {
                (
                    manifest.id.as_str(),
                    manifest.needs.iter().map(String::as_str).collect(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::state::Bits;

    struct Synthetic(&'static str);

    impl CoreModule for Synthetic {
        fn manifest_source(&self) -> &'static str {
            self.0
        }

        fn register(&self, _module: &mut Registrar<'_>) {}
    }

    fn synthetic(id: &str, needs: &[&str], tools: &[&str]) -> &'static dyn CoreModule {
        let source = serde_json::json!({
            "id": id,
            "name": id,
            "description": "A test module.",
            "version": 1,
            "kind": "core",
            "switchable": true,
            "needs": needs,
            "tools": tools.iter().map(|tool| (tool.to_string(), "query")).collect::<BTreeMap<_, _>>(),
        })
        .to_string();
        Box::leak(Box::new(Synthetic(Box::leak(source.into_boxed_str()))))
    }

    #[test]
    fn registry_builds() {
        let registry = Registry::build(super::super::core::ALL, &[]).expect("core modules");
        assert_eq!(registry.descriptions().len(), super::super::core::ALL.len());
        // Every module comes after what it needs.
        for (index, module) in registry.descriptions().iter().enumerate() {
            for need in &module.needs {
                assert!(registry.index_of(need).is_some_and(|need| need < index));
            }
        }
        assert_eq!(
            registry
                .descriptions()
                .first()
                .map(|module| module.id.as_str()),
            Some("workspace")
        );
    }

    #[test]
    fn two_modules_claiming_one_tool_fail() {
        let error = Registry::build(
            &[synthetic("alpha", &[], &["thing_get"])],
            &[synthetic("beta", &[], &["thing_get"])],
        )
        .err()
        .expect("refused");
        assert!(
            matches!(error, RegistryError::ToolClaimedTwice { .. }),
            "{error}"
        );
    }

    #[test]
    fn needs_must_be_known_and_acyclic() {
        let unknown = Registry::build(&[synthetic("alpha", &["nowhere"], &[])], &[])
            .err()
            .expect("refused");
        assert!(
            matches!(unknown, RegistryError::UnknownNeed { .. }),
            "{unknown}"
        );
        let cycle = Registry::build(
            &[
                synthetic("alpha", &["beta"], &[]),
                synthetic("beta", &["alpha"], &[]),
            ],
            &[],
        )
        .err()
        .expect("refused");
        assert!(matches!(cycle, RegistryError::Cycle { .. }), "{cycle}");
        let twice = Registry::build(
            &[synthetic("alpha", &[], &[]), synthetic("alpha", &[], &[])],
            &[],
        )
        .err()
        .expect("refused");
        assert!(matches!(twice, RegistryError::Duplicate { .. }), "{twice}");
        let dotted = Registry::build(&[synthetic("local.alpha", &[], &[])], &[])
            .err()
            .expect("refused");
        assert!(matches!(dotted, RegistryError::BadId { .. }), "{dotted}");
    }

    #[test]
    fn seventy_modules_fit() {
        let modules = (0..70)
            .map(|index| {
                let needs = if index > 0 {
                    vec![format!("module_{}", index - 1)]
                } else {
                    Vec::new()
                };
                let needs = needs.iter().map(String::as_str).collect::<Vec<_>>();
                synthetic(&format!("module_{index}"), &needs, &[])
            })
            .collect::<Vec<_>>();
        let registry = Registry::build(&modules, &[]).expect("seventy modules");
        assert_eq!(registry.index_of("module_69"), Some(69));
        // A set over seventy modules needs two words, and holds the last.
        let all = Bits::<2>::filled(70);
        assert!(all.has(69));
        assert!(!all.has(70));
        assert!(!all.without(69).has(69));
    }
}
