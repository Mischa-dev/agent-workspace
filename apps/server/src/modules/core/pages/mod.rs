//! The Pages module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/pages.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::memory::router);
        module.operations(crate::memory::ops::register_ops);
        module.export(ExportPart {
            name: "pages",
            describe: "Pages with every version, and decisions.",
        });
    }
}
