//! The Messages module's registration. Its feature code lives where it
//! always has; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, ExportPart, ModuleFilter, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/messages.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.routes(crate::direct::router);
        module.routes(crate::replies::router);
        module.routes(crate::message_edits::router);
        module.routes(crate::reactions::router);
        module.routes(crate::scheduled_messages::router);
        module.routes(crate::forwarding::router);
        module.operations(crate::read_state::ops::register_ops);
        module.operations(crate::drafts::ops::register_ops);
        module.operations(crate::keywords::ops::register_ops);
        module.operations(crate::discussion::ops::register_ops);
        module.operations(crate::conversations::followed::register_ops);
        module.operations(crate::conversations::ops::register_ops);
        module.operations(crate::conversations::thread_ops::register_conversation_ops);
        module.operations(crate::replies::ops::register_ops);
        module.filter(ModuleFilter::Conversations { direct: true });
        // Threads belong to Channels, and work in DMs and groups too.
        module.filter(ModuleFilter::AlsoServes(&["thread_start", "thread_update"]));
        module.export(ExportPart {
            name: "messages",
            describe: "Direct messages and groups the exporting person is in.",
        });
    }
}
