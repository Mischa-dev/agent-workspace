//! The Automations module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, IdRoute, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/automations.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::webhooks::router);
        module.routes(crate::automations::router);
        module.routes(crate::incoming_hooks::router);
        module.operations(crate::incoming_hooks::ops::register_ops);
        // A delivery to a hook in a space with automations off is refused.
        module.id_route(IdRoute {
            prefix: "/api/v1/incoming-hooks/",
            param: "hook_id",
            lookup: "SELECT space_id, workspace_id FROM incoming_hooks WHERE id = $1",
        });
        module.export(ExportPart {
            name: "automations",
            describe: "Automation rules and webhooks (no secrets).",
        });
    }
}
