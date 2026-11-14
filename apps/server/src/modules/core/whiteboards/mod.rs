//! The Whiteboards module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/whiteboards.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::whiteboards::router);
        module.export(ExportPart {
            name: "whiteboards",
            describe: "Whiteboards as documents and images.",
        });
    }
}
