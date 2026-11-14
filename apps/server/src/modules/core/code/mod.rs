//! The Code module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/code.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::repositories::router);
        module.routes(crate::github::router);
    }
}
