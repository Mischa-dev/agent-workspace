//! The Dashboards module's registration (T1.32). Its feature code lives in
//! `crate::dashboards`; this folder is how the server finds the module.

use crate::modules::registry::{CoreModule, Registrar};

pub struct Module;

pub static MODULE: Module = Module;

impl CoreModule for Module {
    fn manifest_source(&self) -> &'static str {
        include_str!("../../../../../../modules/dashboards.json")
    }

    fn register(&self, module: &mut Registrar<'_>) {
        module.operations(crate::dashboards::ops::register_ops);
    }
}
