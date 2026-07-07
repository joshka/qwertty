//! `qdb` library: the sequence-database model, validation rules, and doc generation.
//!
//! The `qdb` binary is a thin CLI over these modules; tests exercise them directly.

pub mod generate;
pub mod model;
pub mod validate;
