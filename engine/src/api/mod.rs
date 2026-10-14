//! The engine's face to the browser: what the UI reads, and the server that
//! sends it.
//!
//! The UI is a separate program in another language, so the JSON here is a
//! contract. `view` decides what a link looks like on the wire and is pure,
//! like `links::render` is for the terminal.

pub mod view;

pub use view::{LinkView, LiveMessage, TableView, change_message, table_view};
