//! sessions-tui: terminal UIs for coding-agent sessions: the session view (transcript, prompt box, permission modal)
//! and the control room (every session in two panels, keyboard and mouse).
//!
//! Backend-agnostic: the host program implements [`model::Backend`] (session view) or [`room::RoomBackend`] (control room).
//! Threads plus `std::sync::mpsc`, no async runtime. `reduce` is a pure function over the state; `view` and `room::draw`
//! only draw; `keys` and `room::map_key` map key presses to actions.
//! Messages arrive whole, not token by token. All text from a session is untrusted: it is cleaned and drawn as plain text.
pub mod app;
pub mod keys;
pub mod logo;
pub mod model;
pub mod reduce;
pub mod room;
pub mod room_app;
pub mod term;
pub mod text;
pub mod theme;
pub mod view;

pub use app::{run, Options, Outcome};
pub use model::*;
pub use room_app::{run_room, RoomOptions};
