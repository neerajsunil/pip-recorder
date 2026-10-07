//! Command-line flags: development switches used by `--features diagnostics`
//! and docs tooling. `--software-ui` is still accepted and has no effect.

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
