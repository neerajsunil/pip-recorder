//! Command-line flags. Release builds accept only `--software-ui`; the rest
//! are development switches used by `--features diagnostics` and docs tooling.

/// Whether `name` was passed on the command line.
pub fn flag(name: &str) -> bool {
    std::env::args().any(|arg| arg == name)
}

/// The argument following `name`, e.g. `value("--snapshot")` for `--snapshot out.png`.
#[cfg(feature = "diagnostics")]
pub fn value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip_while(|arg| arg != name);
    args.next()?;
    args.next()
}
