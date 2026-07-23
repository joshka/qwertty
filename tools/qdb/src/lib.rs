//! `qdb` library: the sequence-database model, validation rules, and doc generation.
//!
//! The `qdb` binary is a thin CLI over these modules; tests exercise them directly.

pub mod capture;
pub mod escape;
pub mod generate;
pub mod matrix;
pub mod model;
// Compiled on Windows too so the `#[cfg(windows)]` ConPTY target registration in `TargetKind` is
// real and type-checked by the msvc build. The Unix compilation is byte-for-byte unchanged; on
// Windows the concrete target roster is just the ConPTY adapter.
#[cfg(any(unix, windows))]
pub mod orchestrate;
pub mod runner;
pub mod targets;
pub mod validate;
pub mod width;
