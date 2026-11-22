//! The Workspace module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ModuleFilter, Registrar};

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
        module.operations(crate::follows::ops::register_ops);
        module.operations(crate::profiles::ops::register_ops);
        module.operations(crate::groups::ops::register_ops);
        module.operations(crate::search::ops::register_ops);
        module.operations(crate::custom_emoji::ops::register_ops);
        module.routes(crate::custom_emoji::router);
        module.operations(crate::themes::ops::register_ops);
        module.operations(crate::roles::ops::register_ops);
        module.operations(crate::settings::ops::register_ops);
        module.operations(crate::workspace::admin_ops::register_ops);
        module.operations(crate::notifications::ops::register_ops);
        module.routes(crate::profiles::router);
        module.operations(crate::modules::built::ops::register_ops);
        module.operations(crate::modules::connected::ops::register_ops);
        module.operations(crate::modules::connected::panels::register_ops);
        module.operations(crate::modules::connected::files::register_ops);
        module.routes(crate::modules::connected::http::router);
        module.operations(crate::history::ops::register_ops);
        module.operations(crate::events::activity::register_ops);
        module.operations(crate::links::ops::register_ops);
        module.operations(crate::files::ops::register_ops);
        module.operations(crate::files::tool_ops::register_ops);
        module.operations(crate::export::ops::register_ops);
        module.operations(crate::metering::ops::register_ops);
        module.operations(crate::budgets::ops::register_ops);
        module.routes(crate::export::transfer::router);
        module.operations(crate::accounts::claims::register_ops);
        module.routes(crate::accounts::claims::router);
        module.operations(crate::accounts::credential_ops::register_ops);
        module.operations(crate::spaces::invite_links::register_ops);
        module.operations(crate::spaces::ops::register_ops);
        module.operations(crate::spaces::members::register_ops);
        module.operations(crate::memberships::expiry::register_ops);
        module.routes(crate::spaces::invite_links::router);
        module.operations(crate::workspace::ops::register_ops);
        module.operations(crate::workspace::reads::register_ops);
        module.operations(crate::ops::long::register_ops);
        module.operations(crate::files::link::register_ops);
        module.routes(crate::files::link::router);
        // The workspace's own settings (its invitations, default layout and
        // layout presets) are set in its own space, where Projects isn't on.
        module.filter(ModuleFilter::AlsoServes(&[
            "invitation_list",
            "invitation_create",
            "invitation_update",
            "layout_update",
            "layout_preset_create",
            "layout_preset_update",
        ]));
        // Its project templates and model lists are too, but only while
        // Projects or Runs is on for the workspace.
        module.filter(ModuleFilter::HomeFor(&[
            "project_template_update",
            "model_catalog_set",
        ]));
    }
}
