//! The Whiteboards module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, IdRoute, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/whiteboards.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::whiteboards::router);
        module.operations(crate::whiteboards::ops::register_ops);
        module.id_route(IdRoute {
            prefix: "/api/v1/whiteboards/",
            param: "id",
            lookup: "SELECT project_id, organization_id FROM whiteboards WHERE id = $1",
        });
        module.export(ExportPart {
            name: "whiteboards",
            describe: "Whiteboards as documents and images.",
        });
    }
}
