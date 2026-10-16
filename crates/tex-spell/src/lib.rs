//! Spell checking for LaTeX: which words of a document are prose
//! ([`extract`]), which language it is written in ([`lang`]), and Hunspell
//! dictionaries for ~90 languages ([`catalog`]), found on the system or
//! downloaded on demand ([`store`]) and checked with spellbook ([`checker`]).

pub mod catalog;
pub mod checker;
pub mod extract;
pub mod lang;
mod repair;
pub mod store;

pub use checker::Speller;
pub use lang::{Detection, Source};
pub use store::Store;
