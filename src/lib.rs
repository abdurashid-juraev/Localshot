#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod capture;
pub mod config;
pub mod coord;
pub mod daemon;
pub mod draw;
pub mod interaction;
pub mod linux_desktop;
pub mod state;
pub mod updater;
