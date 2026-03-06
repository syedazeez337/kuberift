//! kuberift library — exposes all internal modules so integration tests in tests/ can import them.
//! This is a CLI tool; the lib target exists solely to give the test suite access to internal
//! types. `must_use_candidate` and `missing_errors_doc` are suppressed because these are
//! implementation details, not a published library API.
#![warn(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::too_many_lines,
    clippy::must_use_candidate,   // internal helpers; callers are tests, not library consumers
    clippy::missing_errors_doc,   // action fns return I/O errors; docs belong in README/wiki
    clippy::missing_panics_doc,   // Mutex::lock().unwrap() in coordinator is local and can't deadlock
)]

pub mod actions;
pub mod cli;
pub mod config;
pub mod cost;
pub mod items;
pub mod k8s;
