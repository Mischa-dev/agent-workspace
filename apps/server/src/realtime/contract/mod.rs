//! The live role as a published contract (K5.19, ruling R24 rule 5;
//! `docs/contracts/live.md`): the database side (`api`, `work`) and the
//! live side meet at a ticket, a publish and a resume, so a live service
//! that sleeps idle connections can replace this server's own without the
//! server changing.
//!
//! - `ticket`: what a connection may hear, signed (Ed25519, from the
//!   keyring) so the live side trusts it with no database.
//! - `live`: the live side as this server runs it (`/live/v1/socket`,
//!   `/live/v1/publish`).
//! - `publish`: what the database side publishes, to which audiences, and
//!   the publisher that sends the log to an outside live service.
//! - `http`: tickets, resume and the public key, on `api`.

pub mod http;
pub mod live;
pub mod publish;
pub mod ticket;

pub use live::{LiveRouter, Publish};
pub use ticket::Audience;
