//! The Collections module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, ModuleFilter, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/collections.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::collections::http::router);
        module.operations(crate::collections::ops::register_ops);
        module.operations(crate::forms::register_ops);
        module.operations(crate::collections::record_templates::register_ops);
        module.operations(crate::views::ops::register_ops);
        // Forms are bug report links too: these work where Collections is on.
        module.filter(ModuleFilter::AlsoServes(&[
            "bug_report_link_list",
            "bug_report_link_create",
            "bug_report_link_update",
        ]));
        module.export(ExportPart {
            name: "collections",
            describe: "Collections and their records.",
        });
    }
}
