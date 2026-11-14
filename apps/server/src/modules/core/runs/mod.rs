//! The Runs module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/runs.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::account::router);
        module.routes(crate::usage::router);
        module.routes(crate::environments::router);
        module.routes(crate::harnesses::router);
        module.routes(crate::launch::router);
        module.routes(crate::run_views::router);
    }
}
