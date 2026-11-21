//! Command-line flags. Shiv is a GUI and takes no options, but a flag must
//! never start the window or block: `--version` and `--help` print and
//! exit, anything else starting with `-` is an error (exit 2).

use std::ffi::OsString;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Cli {
    /// No flags: open the window.
    Run,
    /// Print this to stdout and exit 0.
    Print(String),
    /// Print this to stderr and exit 2.
    Fail(String),
}

const USAGE: &str = "Usage: shiv [--version | --help]

Shiv is a desktop mod manager for Deadlock; run it without options to open
the window.

Options:
  -V, --version  Print the version and exit
  -h, --help     Print this help and exit
";

pub(crate) fn parse(args: impl IntoIterator<Item = OsString>) -> Cli {
    for arg in args {
        match arg.to_str() {
            Some("-V" | "--version") => {
                return Cli::Print(format!("shiv {}\n", env!("CARGO_PKG_VERSION")));
            }
            Some("-h" | "--help") => return Cli::Print(USAGE.to_owned()),
            Some(flag) if flag.starts_with('-') => {
                return Cli::Fail(format!("shiv: unknown option '{flag}'\n\n{USAGE}"));
            }
            None if arg.to_string_lossy().starts_with('-') => {
                return Cli::Fail(format!("shiv: unknown option\n\n{USAGE}"));
            }
            // Positional arguments were always ignored; keep it that way.
            _ => {}
        }
    }
    Cli::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Cli {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn flags_print_or_fail_and_never_run() {
        let version = format!("shiv {}\n", env!("CARGO_PKG_VERSION"));
        assert_eq!(run(&["--version"]), Cli::Print(version.clone()));
        assert_eq!(run(&["-V"]), Cli::Print(version));
        assert!(matches!(run(&["--help"]), Cli::Print(t) if t.starts_with("Usage: shiv")));
        assert!(matches!(run(&["-h"]), Cli::Print(_)));
        assert!(matches!(run(&["--frobnicate"]), Cli::Fail(t) if t.contains("--frobnicate")));
        assert_eq!(run(&[]), Cli::Run);
        assert_eq!(run(&["some/path"]), Cli::Run);
    }
}
