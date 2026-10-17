//! Running TeX engines as isolated child processes.

mod engine;
mod scheduler;

pub use engine::{Engine, EngineKind, Flavor, Job, detect, find_tex_tool, texdoc};
pub use scheduler::{Compiler, Event, Output, Status};
pub use tex_log::{Diagnostic, Severity};
