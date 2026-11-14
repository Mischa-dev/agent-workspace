//! The core modules: one folder each under `core/`, listed by `build.rs`
//! into `ALL` (and `CORE_WORDS`, the words a `ModuleSet` needs), so adding
//! a module edits no list here.

include!(concat!(env!("OUT_DIR"), "/core_modules.rs"));
