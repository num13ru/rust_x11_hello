//! Ratatui operator console. `app` holds the pure application state machine;
//! `view` renders it with ratatui. Neither owns the terminal, dispatch, or
//! the log buffer; the binary wires both together.

pub mod app;
pub mod view;
