//! Live updates (K5 3.4). `hub` is the in-process fan-out to browsers'
//! sockets; `wake` is the one listening connection that says when to look;
//! `tail` follows the event log on every instance and feeds the hub (K5.5);
//! `ephemeral` carries what is never stored between instances.

pub mod ephemeral;
mod hub;
pub mod tail;
pub mod wake;

pub use hub::*;
