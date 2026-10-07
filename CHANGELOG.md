# Changelog

## Unreleased

- Restructure the workspace for maintainability and future platforms: portable `fastrecorder-core` and new `fastrecorder-mp4` crates, a `fastrecorder-platform` facade, and the Windows backend in `crates/platform-windows` split into `capture`, `pipeline`, `encode`, `audio` and shell modules. The app's controller is split into per-area `studio` modules and the UI into reusable Slint components and pages. See docs/ARCHITECTURE.md. No intended behavior change.
- MP4 muxing now has structural tests (H.264/AV1 configuration, B-frame composition offsets, AAC interleaving and post-hoc attachment), and CI runs the workspace tests on Windows plus the portable crates on Linux x64/ARM64 and macOS.
- A panic inside one UI callback no longer poisons the shared studio state for later callbacks.
- NVENC Stop signals EOS before reading pending frames, and every output read waits for completion in the synchronous encoder session. Conversion/preview resources retire before encoder finalization. If flushing fails after video packets were saved, finalize the completed portion with a visible warning instead of leaving it without MP4 metadata; audio is limited to that portion. Errors name the NVENC operation/status and are retained in local diagnostics.
- Recording and native-operation errors use short notices; Details opens the in-app Info panel with the full last issue. Timestamp failures include pending-frame context in the panel and local diagnostics.
- Fix a recording abort when NVENC accepts a new buffered frame while older output is ready. Queued output retirement no longer treats the newest NEED_MORE_INPUT status as a full-queue failure; input rings reserve a larger B-frame/lookahead pipeline margin within the memory budget.
- Recording defaults now request capability-supported NVENC B-frames, hardware lookahead and B-frame references; memory/driver fallbacks retain the selected codec and quality tuning. Intel requests up to three B-frames and uses driver-reported working parameters and surface requirements.
- Bounded NVENC GPU buffer rings, delayed-output handling, stop-time flushing and MP4 composition offsets/video edits support reordered frames without shifting the audio timeline. Info reports the accepted enhancements.
- Reordered MP4 sample durations follow presentation order, keeping a captured frame visible until the next frame even when capture ticks are skipped.
- Live desktop/microphone meters before and during recording, with overload coloring and actionable permission/disconnection errors. Idle metering saves no audio and stops while minimized.
- FP16 Windows Graphics Capture, shared GPU scRGB-to-SDR conversion for preview and recording, display-specific SDR-white normalization and fixed HDR highlight compression. HDR on another monitor no longer blocks recording; output remains SDR.
- Compile and embed the color shaders during the build; no shader compiler runs when the app launches.
- Bounded audio initialization waiting, startup/driver interruption safeguards, graphics-reset checks, and bounded audio catch-up after sleep.
- Preserve Play/Show in folder access when a completed recording cannot be renamed to its destination. Startup cancellation has a normal cancelled state.
- Refresh the studio surface after unocclusion/DPI changes and keep completed worker teardown off the UI thread.
- Explicitly close captured frames when replaced, resized or stopped, returning WGC frame-pool buffers on every path.

Recording/playback, audio synchronization, HDR color appearance, and repeated restore checks remain with the user. Crash recovery is still a separate outstanding feature.

## 0.1.0-alpha.1 — 2026-10-02

Initial public Windows x64 preview. This is a prerelease, not a completed production quality certification.

- Native Rust/Slint studio, live preview and embedded display/application selection.
- NVIDIA/Intel AV1, HEVC and H.264 where supported; Windows hardware H.264 fallback and software H.264/AV1.
- Desktop/microphone WASAPI capture, selected devices, recording gain and stereo AAC audio.
- Recording-focused quality defaults, bitrate tiers, remembered settings and timestamped MP4 files.
- Native recording tray controls and configurable global start/stop shortcuts.
- Simplified studio status showing only codec and frame rate.
- Surface refresh on taskbar/tray restoration, minimized redraw suppression, embedded Windows executable icon/version/manifest, and software UI fallback.
- Open-source documentation, platform roadmap, issue templates, build checks and Windows release packaging.

Known limits: ordinary MP4 needs successful finalization; crash recovery, pause/region capture, HDR, separate audio tracks and non-x64/non-Windows releases remain planned. GPU/driver compatibility, audio synchronization and the reported restore fix require runtime validation. See [release readiness](docs/RELEASE_READINESS.md).
