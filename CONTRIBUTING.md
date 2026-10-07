# Contributing to Pip

Thanks for helping make a simple, friendly screen recorder for everyone. First contribution? Look for [good first issues](https://github.com/neerajsunil/pip-recorder/contribute), or say hi in [Discussions](https://github.com/neerajsunil/pip-recorder/discussions) to find a place to start.

## Before changing code

For a small fix, open a pull request. For a new backend, encoding change or significant UI change, open an issue first so the scope and platform behavior can be agreed on. Search existing issues before submitting a duplicate. Follow [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

The default studio should be understandable without technical knowledge. UI colours, radii and the product name live in `crates/app/ui/theme.slint`; use those tokens instead of literal colours. Pip (`components/pip.slint`) reacts to the studio's state, so keep its moods meaningful rather than decorative. Copy should be short, warm and plain: say what happened and what to do next. Put encoder/vendor details, rate control and diagnostics in Settings/Info. Keep recording work away from the UI thread and keep buffers bounded. A buildable UI on another platform does not establish recording support; update the README platform table only when the complete backend and release are verified.

## Where code lives

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) maps every directory and says where new code belongs. Keep logic that needs no OS API in `crates/core` or `crates/mp4`, so it is tested on every platform. The app must use `fastrecorder-platform`, not a backend crate directly.

## Setup and checks

Windows contributors need stable Rust, the MSVC x64 toolchain, Visual Studio C++ Build Tools and a Windows SDK. Clone the repository and build with the committed lockfile:

```powershell
cargo build -p fastrecorder --locked
# Optimized but quick to rebuild, for trying changes (target/fast/fastrecorder.exe):
cargo build -p fastrecorder --profile fast --locked
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
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/audio-settings.png --docs-view audio --software-ui
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/community.png --docs-view community --software-ui
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/studio-dark.png --docs-dark --software-ui
cargo run -p fastrecorder --features diagnostics -- --docs-snapshot docs/images/settings-advanced.png --docs-view video-advanced --software-ui
```

Other `--docs-view` values preview each studio state without capturing anything: `video`, `recording-settings`, `appearance`, `sources`, `recording`, `countdown`, `saved`, `napping` and `oops`. Add `--docs-dark` for dark mode, `--docs-advanced` to show Advanced options on the audio page, and `--docs-height 1750` to capture a whole settings page. Self-test recordings accept `--nvenc-max`, `--nvenc-low-latency` and `--plays-everywhere` to exercise encoder options. For performance work, `--profile-record SECONDS --profile-output FILE` records the selected display (add `--profile-no-lookahead`, `--profile-no-multipass` or `--profile-no-bframes`), `--profile-no-preview` and `--profile-no-audio` isolate idle costs, and `--profile-visible` allows screenshots of the studio. Measure with a release build (`cargo build --release --features diagnostics`). New settings belong in the Simple layer only if most people need them; everything else goes behind Advanced.

## Release process

Follow [docs/RELEASING.md](docs/RELEASING.md). Maintainers should keep preview releases explicitly marked as prereleases until the recording/device matrix is satisfied.
