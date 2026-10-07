# Releasing Pip

Preview releases must remain marked as GitHub prereleases until the recording/device matrix and known release blockers in [RELEASE_READINESS.md](RELEASE_READINESS.md) are resolved. A successful CI build is not recording/playback validation.

## Prepare

1. Update CHANGELOG.md, Cargo workspace version, Windows manifest identity and README support matrix. Do not mark ARM/macOS/Linux available merely because code compiles.
2. Review dependency licenses and security advisories, and record the build toolchain.
3. Run formatting and Clippy with the lockfile. Complete the relevant runtime checks with supported GPUs/audio devices, including repeated minimized restore through taskbar and tray, audio synchronization, cancellation and finalization. Maintainer tests are separate from automated compile checks.
4. Regenerate documentation images if the studio changed. Verify that the source/package contains no private recordings, snapshots, preferences or credentials.

## Package

```powershell
cargo build -p fastrecorder --release --locked
.\scripts\package-windows.ps1 -Version 0.2.0-alpha.1 -Binary .\target\release\fastrecorder.exe
```

The script stages the binary, release notes, license and third-party notices, generates dependency/license metadata from the lockfile and includes available license files. It produces an x64 ZIP and SHA-256 checksums in dist/. Install current Visual C++ runtime if the native bridge requires it; no driver DLLs or FFmpeg are shipped. Signing is not implemented yet; do not imply a signed build.

The **Windows package** workflow can be dispatched to create the same package as an Actions artifact. Artifacts are build outputs, not supported release declarations. ARM64 packaging is deliberately deferred until its driver/fallback behavior is validated.

## Publish

Create an annotated tag on the reviewed commit and a GitHub release. Upload the ZIP and checksum file; include limitations and a link to the exact source tag. Mark preview versions as prereleases. Verify files/metadata after publishing. A release signing certificate or automated update service is not required to publish source; both remain separate distribution work.
