//! Rules implementations (§10).
//!
//! `list/1` is the shared list. `chess/1` is a two-player game: shakmaty decides whether a
//! move is legal, and the rules never read a clock.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod chess;
pub mod list;
