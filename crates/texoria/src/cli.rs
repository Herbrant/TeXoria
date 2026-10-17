//! The command line: `texoria [FILE | FOLDER]`.

use std::ffi::OsString;
use std::path::PathBuf;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch {
    /// Started by a coding agent: relay its MCP connection to the app's tools.
    Bridge,
    Help,
    Version,
    /// Open the window, on a file or a folder if one is given.
    Open(Option<PathBuf>),
    /// An option TeXoria does not know.
    Unknown(String),
}

pub const USAGE: &str = "\
Usage: texoria [FILE | FOLDER]

Opens a .tex file, or the main file of a folder (main.tex, or the first .tex
file with \\documentclass).

Options:
  -h, --help     Print this help
  -V, --version  Print the version";

/// What `args` (without the program's name) ask for.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Launch {
    let mut args = args.into_iter().peekable();
    if args
        .peek()
        .is_some_and(|a| *a == *tex_agent::mcp::BRIDGE_FLAG)
    {
        return Launch::Bridge;
    }
    let mut path = None;
    let mut options_done = false;
    for arg in args {
        let text = arg.to_string_lossy();
        if !options_done && text.starts_with('-') && text.len() > 1 {
            match &*text {
                "--" => options_done = true,
                "-h" | "--help" => return Launch::Help,
                "-V" | "--version" => return Launch::Version,
                // macOS adds a process serial number to apps started from the Finder.
                _ if text.starts_with("-psn_") => {}
                _ => return Launch::Unknown(text.into_owned()),
            }
        } else if path.is_none() {
            path = Some(PathBuf::from(arg));
        }
    }
    Launch::Open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(args: &[&str]) -> Launch {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn opens_the_first_path() {
        assert_eq!(launch(&[]), Launch::Open(None));
        assert_eq!(
            launch(&["paper/main.tex"]),
            Launch::Open(Some("paper/main.tex".into()))
        );
        // Several files from a file manager: the first one.
        assert_eq!(
            launch(&["a.tex", "b.tex"]),
            Launch::Open(Some("a.tex".into()))
        );
        assert_eq!(launch(&["-"]), Launch::Open(Some("-".into())));
        assert_eq!(
            launch(&["--", "-draft.tex"]),
            Launch::Open(Some("-draft.tex".into()))
        );
    }

    #[test]
    fn reads_options() {
        assert_eq!(launch(&["--mcp-bridge"]), Launch::Bridge);
        // Only as the first argument, the way agents start it.
        assert_eq!(
            launch(&["a.tex", "--mcp-bridge"]),
            Launch::Unknown("--mcp-bridge".into())
        );
        assert_eq!(launch(&["--help"]), Launch::Help);
        assert_eq!(launch(&["a.tex", "-h"]), Launch::Help);
        assert_eq!(launch(&["-V"]), Launch::Version);
        assert_eq!(launch(&["--verbose"]), Launch::Unknown("--verbose".into()));
        assert_eq!(launch(&["-psn_0_12345"]), Launch::Open(None));
        assert_eq!(
            launch(&["-psn_0_1", "x.tex"]),
            Launch::Open(Some("x.tex".into()))
        );
    }
}
