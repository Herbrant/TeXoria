//! What TeXoria's crates share about the system they run on: child processes that run in a
//! group of their own ([`process`]), and paths, the home folder and the `PATH` ([`paths`]).

pub mod paths;
pub mod process;
