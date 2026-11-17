//! The Workspace module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/workspace.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.operations(crate::packages::ops::register_ops);
        module.operations(crate::packages::directory_ops::register_ops);
        module.operations(crate::statuses::ops::register_ops);
        module.operations(crate::custom_emoji::ops::register_ops);
        module.routes(crate::custom_emoji::router);
        module.operations(crate::themes::ops::register_ops);
        module.operations(crate::modules::built::ops::register_ops);
        module.operations(crate::history::ops::register_ops);
        module.operations(crate::accounts::claims::register_ops);
        module.routes(crate::accounts::claims::router);
        module.operations(crate::spaces::invite_links::register_ops);
        module.routes(crate::spaces::invite_links::router);
        module.operations(crate::workspace::ops::register_ops);
        module.operations(crate::workspace::reads::register_ops);
        module.operations(crate::ops::long::register_ops);
    }
}
