//! Remote sessions core: driving Claude Code Remote Control sessions that run on your own machines.
//!
//! * [`ids`]: `session_<x>` (claude.ai) and `cse_<x>` (API) are the same session.
//! * [`rc`]: eligibility checks for `claude remote-control`, worker discovery from a `ps` listing, and stop.
//! * [`send`]: `claude -p "<msg>" --cloud session_<id> --output-format json` (arrives as a PEER message).
//! * [`cast`] and [`transcript`]: a session's local transcript as an asciicast v2 recording (cast is from cloud-sessions, same authors).
//! * [`control`]: pending permission prompts, approve/deny, say (a real user turn) and interrupt, from another machine.
//! * [`api`] and [`events`]: reading events (from cloud-sessions, same authors), paged.
//! * [`stream`]: live events over `events/stream` (server-sent events, message by message), with resume on drop.
//!
//! Event text comes from a run that may have read hostile content. Nothing here executes it.
pub mod api;
pub mod cast;
pub mod control;
pub mod endcard;
pub mod events;
#[doc(hidden)]
pub mod fakehttp;
pub mod ids;
pub mod rc;
pub mod send;
pub mod stream;
pub mod transcript;

pub type Result<T> = std::result::Result<T, String>;

pub use events::Event;
pub use ids::{cse_id, parse_session_id};
