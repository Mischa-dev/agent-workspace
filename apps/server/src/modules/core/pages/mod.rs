//! The Pages module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, IdRoute, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/pages.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::memory::router);
        module.operations(crate::memory::ops::register_ops);
        module.operations(crate::memory::page_ops::register_ops);
        module.operations(crate::access::sharing::register_ops);
        module.operations(crate::proposals::ops::register_ops);
        module.operations(crate::change_sets::ops::register_ops);
        module.id_route(IdRoute {
            prefix: "/api/v1/pages/",
            param: "id",
            lookup: "SELECT page.project_id, project.organization_id FROM project_pages page \
                     JOIN projects project ON project.id = page.project_id WHERE page.id = $1",
        });
        module.export(ExportPart {
            name: "pages",
            describe: "Pages with every version, and decisions.",
        });
    }
}
