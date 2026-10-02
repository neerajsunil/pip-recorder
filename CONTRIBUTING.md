# Contributing to FastRecorder

Thanks for helping make a simple screen recorder accessible to everyone.

## Before changing code

For a small fix, open a pull request. For a new backend, encoding change or significant UI change, open an issue first so the scope and platform behavior can be agreed on. Search existing issues before submitting a duplicate. Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

The default studio should be understandable without technical knowledge. Put encoder/vendor details, rate control and diagnostics in Settings/Info. Keep recording work away from the UI thread and keep buffers bounded. A buildable UI on another platform does not establish recording support; update the README platform table only when the complete backend and release are verified.

## Setup and checks

Windows contributors need stable Rust, the MSVC x64 toolchain, Visual Studio C++ Build Tools and a Windows SDK. Clone the repository and build with the committed lockfile:

```powershell
cargo build -p fastrecorder --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --features diagnostics --locked -- -D warnings
```

Run relevant tests and manual checks when you are ready to validate your change. Tests can be run with `cargo test --workspace --locked`; capture diagnostics and the manual recording matrix are documented in [development notes](docs/DEVELOPMENT.md) and [release readiness](docs/RELEASE_READINESS.md). Do not claim recording/playback verification from compilation alone.

## Pull requests

Use a focused branch and describe the user-visible problem, the resulting behavior and validation. Include UI screenshots for visible changes using a safe sample preview; never include another person's desktop, audio, device IDs or recording. Describe any hardware/driver limits. Keep Cargo.lock updated when dependencies change. Preserve third-party copyright and license notices.

Contributions are licensed under GPL-3.0-only unless an existing vendored file specifies its own license. You must have permission to contribute the code/assets you submit. No CLA is required.

## Documentation images

The committed screenshots use the actual Slint UI with a deterministic sample preview. They never start screen/audio capture or save preferences.

```powershell
# Optional: requires Pillow to regenerate the icon and sample preview.
python scripts/generate-brand.py
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/studio.png --software-ui
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/audio-settings.png --docs-audio --software-ui
```

## Release process

Follow [docs/RELEASING.md](docs/RELEASING.md). Maintainers should keep preview releases explicitly marked as prereleases until the recording/device matrix is satisfied.
