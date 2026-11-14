//! The Tasks module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/tasks.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::work_items::router);
        module.routes(crate::work_items::comments::router);
        module.routes(crate::bug_reports::router);
        module.export(ExportPart {
            name: "tasks",
            describe: "Tasks, their comments and history.",
        });
    }
}
