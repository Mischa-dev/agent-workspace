//! Pages as live documents (AWD-53 slice 3).
//!
//! A page's content is a Yjs document that the web editor and the server
//! both edit; its Markdown body is derived from it. `markdown` reads
//! Markdown exactly as the web app does, so a page keeps its look when it
//! turns into a document.

pub mod edits;
pub mod http;
pub mod markdown;
pub mod store;
#[cfg(test)]
mod testing;
pub mod tree;
pub mod write;
pub mod ydoc;
