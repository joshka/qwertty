//! `qdb` library: the sequence-database model, validation rules, and doc generation.
//!
//! The `qdb` binary is a thin CLI over these modules; tests exercise them directly.

pub mod capture;
pub mod escape;
pub mod generate;
pub mod matrix;
pub mod model;
#[cfg(unix)]
pub mod orchestrate;
pub mod runner;
pub mod targets;
pub mod validate;
pub mod width;
