//! The Inbox module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/inbox.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::notifications::router);
        module.routes(crate::later::router);
        module.routes(crate::questions::router);
    }
}
