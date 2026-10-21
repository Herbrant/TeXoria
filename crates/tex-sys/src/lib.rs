//! What TeXoria's crates share about the system they run on: child processes that run in a
//! group of their own ([`process`]), paths, the home folder and the `PATH` ([`paths`]), and
//! verified downloads ([`download`]).

pub mod download;
pub mod paths;
pub mod process;
