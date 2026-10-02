# FastRecorder development and architecture

A small, native screen recorder for Windows 11, built with Rust and Slint.

## Run

Run commands from the repository root. Install stable Rust with the `x86_64-pc-windows-msvc` toolchain and the Visual Studio C++ build tools / Windows SDK. See the [project README](../README.md) for downloads and the current platform support table.

```powershell
.\run.ps1
# Optimized build:
.\run.ps1 -Release
# Software UI renderer for GPU troubleshooting:
.\run.ps1 -SoftwareUI
```

The studio starts with a live preview of the main display. **Change source** opens the embedded display/application selector. Press **Record**; files default to a new filename in your Windows Videos/FastRecorder folder. Default filenames use local time in `YYYY-MM-DD-HH-mm-ss.mp4` format, refreshed when recording starts. A numeric suffix handles same-second collisions without overwriting a file. The folder button changes the destination, and explicitly chosen filenames are preserved for that recording. Encoder, GPU, quality settings, cursor choice, auto-minimize, save folder and last recording are remembered in `%LOCALAPPDATA%/FastRecorder/preferences.json`. GPU selection is restored by name rather than a potentially changed adapter index. Every launch starts at 30 fps. Fresh installations use automatic encoding and Recommended bitrate.

Recording minimizes the studio by default after the first frame is accepted and shows a FastRecorder icon in the Windows notification area (including the hidden-icons overflow, depending on Windows settings). Right-click for **Stop recording** or **Open FastRecorder**; double-click also restores the studio. **Ctrl+Shift+F9** starts recording globally and **Ctrl+Shift+F10** stops it. Settings → Shortcuts lets you edit both bindings and **Apply**, with conflict feedback and restoration of the previous bindings on failure. Assign the same binding to both actions for a start/stop toggle; leave a field blank to disable that action. Starting can be cancelled from the studio, tray or Stop shortcut. Stop finalizes the MP4 and restores the studio. The icon is hidden after saving. If tray controls cannot initialize, automatic minimization is disabled so Stop remains accessible. The studio is excluded from display capture to prevent preview recursion. **Play video** opens the last recording with Windows' associated player; **Show in folder** selects it in Explorer, including paths with spaces or Unicode.

The Lucide settings icon opens sidebar pages: **Video** (Encoding and Quality), **Audio** (Desktop audio and Microphone), **Shortcuts** (Recording actions), and **General** (Capture, Studio and Saving). **Info** keeps driver-query results, verified encoder/audio configuration, skipped-frame statistics, the capture/processing/rendering pipeline and fallback reasons separate from controls. Unsupported backend-specific controls are hidden or disabled.

The studio has **Desktop audio** and **Microphone** toggles, locked during recording. Fresh installations enable desktop audio and disable the microphone. Settings → Audio selects connected playback/input devices and recording volume (0–200%, independent of system volume). System default resolves the playback console device or microphone communications device when recording starts. Explicit selections use stable Windows device IDs; a disconnected saved device requires reselection or disabling that source. Refresh reloads connected devices. Choices, volumes and applied shortcuts are persisted with the other preferences.

Audio uses native shared-mode WASAPI loopback for desktop sound and WASAPI capture for the microphone. Both are timestamp-aligned on QPC, resampled/downmixed to stereo 48 kHz PCM, summed and clamped to the PCM range, and encoded by Windows Media Foundation AAC-LC at 192 kbps. Reduce recording gains if combined loud sources clip. PCM storage is bounded; only compressed AAC is temporarily spooled beside the recording and added to the MP4 at Stop without re-encoding video. Silence is preserved when a playback device has no active sound. Enabled-device initialization failures abort startup; device loss or AAC failure stops recording and attempts to save captured media with a warning. The sources share one stereo track; separate tracks, live mute, meters and per-application audio are not implemented. Long-session synchronization and device/driver compatibility require runtime checks.

Fresh-install NVENC defaults are **P5, high-quality tuning, VBR, spatial adaptive quantization, quarter-resolution two-pass encoding**, and two-second keyframes. Optional **CQP** uses a constant quantizer (default 20, lower means more detail/larger files), uncapped bitrate and single-pass encoding. Bitrate presets are disabled while CQP applies. Older GPU/drivers can retry the same codec/preset/HQ/rate control without AQ/multipass if those enhancements are rejected; the verified label and fallback details report the actual settings. Intel requests **TU1 best quality**. Hardware quality is not benchmarked against OBS. B-frames and NVENC lookahead remain disabled because the current synchronous GPU buffer and presentation-order MP4 writer do not support delayed/reordered output.

Capture requests borderless Windows Graphics Capture access once per process and disables the capture border for both preview and recording. Windows may require consent and can retain its border if access is denied or another recorder requires it. The Windows UI surface is explicitly opaque. Restore/focus handling refreshes the swapchain using the current native dimensions for both taskbar and tray restoration, skips minimized redraws and requests a new frame after restore. This implementation still needs recording/restore runtime validation. The executable embeds a multi-resolution icon, version metadata and a per-monitor-DPI manifest. A software-UI fallback starts a replacement process and exits the original wrapper. Fatal diagnostics remain locally in `%LOCALAPPDATA%/FastRecorder/diagnostics.log`, capped by rotation at 256 KB.

This foundation records SDR video at the original source resolution, with optional cursor and audio capture. The preview is GPU-downscaled to at most 960 × 540 at 12 fps, then read back for Slint. Its queue contains only the latest image. Preview processing pauses when the studio is minimized, its preview is hidden, or a settings/source panel is open. Recording stays at the selected 30/60 fps and source resolution. While recording, preview shares the recording capture instead of starting a second capture session.


## Architecture

- `fastrecorder-app`: compiled Slint UI, WGPU/Direct3D UI rendering, an embedded display/window selector, native save dialog, session commands and status. UI updates arrive through Slint's event loop. The elapsed-time timer runs only during a session; a bounded preview consumer runs at 12 Hz. GPU UI initialization errors retry with a software renderer in a fresh process; `-SoftwareUI` selects it explicitly.
- `fastrecorder-core`: platform-independent configuration, lifecycle state, and rational frame scheduling. No OS or graphics dependencies.
- `fastrecorder-windows`: Windows Graphics Capture, D3D11 video processing, a bounded Media Foundation sample allocator, direct NVIDIA NVENC and Intel Quick Sync (AV1 / HEVC / H.264), Windows software H.264, Rust rav1e software AV1, WASAPI/AAC audio, MP4 muxing and native global shortcuts. Native objects stay inside this crate. One recording thread owns the D3D immediate context; capture callbacks only replace the latest frame and wake it. A separate audio thread owns its COM endpoints, bounded mixer and AAC transform.

The capture worker copies into an owned BGRA texture, scales / letterboxes into the initial fixed canvas, and converts to NV12 using D3D11 video processing. GPU samples are leased from Media Foundation's bounded allocator, preventing texture reuse while the encoder still holds them. Static screens repeat the latest copied frame on a monotonic 30/60 fps grid. Overload skips ticks rather than accumulating an unbounded queue.

Automatic mode queries NVIDIA through NVENC and Intel through the installed oneVPL / Media SDK hardware runtime. It prefers an AV1-capable adapter, then HEVC, then H.264. On the selected adapter, startup tries AV1 -> HEVC -> H.264 before verified Media Foundation hardware H.264 and Windows software H.264. Explicit choices never silently switch codec or vendor. The encoder list contains only supported hardware choices for the selected GPU (or all detected supported hardware choices when GPU selection is Automatic), plus software H.264 and AV1. Changing GPU resets an incompatible selection to Automatic. Capability queries do not guarantee initialization for every resolution/configuration.

Intel prefers the modern driver runtime and falls back to the legacy Media SDK runtime when the modern runtime is absent. Rust owns capture, readback, packet timing and muxing; a small C++ bridge uses the vendored Intel MIT API headers to keep runtime structure layouts ABI-safe. Intel hardware encoding currently uses a bounded pool of system-memory NV12 surfaces with GPU readback. NVIDIA uses registered D3D11 NV12 surfaces without pixel readback. Software AV1 uses rav1e 0.8.1, speed 8, presentation-order frames, eight-frame RDO lookahead and up to eight CPU threads, with portable Rust kernels (assembly is not enabled in this base build). Software encoding can skip frames under load. AMD retains the Media Foundation hardware H.264 fallback.

Recommended bitrates for mostly static screen content at 30 fps:

| Resolution | AV1 | HEVC | H.264 |
|---|---:|---:|---:|
| 1080p | 4 Mbps | 5 Mbps | 8 Mbps |
| 1440p | 6 Mbps | 8 Mbps | 12 Mbps |
| 4K | 12 Mbps | 16 Mbps | 24 Mbps |

These are product starting points, not measured quality guarantees. Custom resolutions interpolate in pixel area between tiers, with gentle power-law scaling below 1080p or above 4K. At 60 fps the target increases by approximately 1.62x. High uses 1.5x Recommended; Low uses 0.65x; Custom accepts 1-100 Mbps. Targets round to whole Mbps and clamp to that range. The UI shows the computed target, codec, canvas size and frame rate. A startup codec fallback recalculates the target for the actual codec; Custom remains fixed. Resizing an application during recording preserves the initial recording canvas and bitrate.


NVENC is loaded dynamically from the installed driver in System32; no NVIDIA SDK installation, CUDA runtime, or FFmpeg is required. The vendored NVENC 12.1 ABI supports AV1 while allowing older driver compatibility; driver implementations remain current. Only compressed packets reach the CPU in the NVIDIA recording path. AV1 MP4 metadata includes its sequence header, sample timing, sizes, and keyframes; media is streamed to disk and only the sample table is held in memory. MP4 playback requires AV1 support in the player. D3D12 encoding remains future work; WGC natively supplies D3D11 textures, while the UI uses WGPU/Direct3D 12.


Free space is checked before recording and every two seconds during recording; below 64 MB, the worker stops and attempts finalization. Files are written to a uniquely named temporary sibling and renamed only after successful finalization. Existing destinations are rejected. Failures attempt to finalize captured frames; if finalization fails, the incomplete temporary file is retained and its path is reported. Standard MP4 crash recovery is not implemented.

## Validation

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets --features diagnostics -- -D warnings
cargo fmt --all --check

# Opens the actual UI, snapshots it, and exits:
cargo run -p fastrecorder --features diagnostics -- --snapshot .local\ui.png

# Records FastRecorder's own window for four seconds; use a new output filename:
cargo run -p fastrecorder --features diagnostics -- --self-test-record .local\smoke.mp4

# Decode the MP4 with Windows Media Foundation and save its first frame:
cargo run -p fastrecorder --features diagnostics -- --validate .local\smoke.mp4

# Exercise software encoding or 60 fps with new filenames:
cargo run -p fastrecorder --features diagnostics -- --self-test-record .local\software.mp4 --software-encoder
cargo run -p fastrecorder --features diagnostics -- --self-test-record .local\smooth.mp4 --60fps

# Opt in to the configured audio sources for a diagnostic recording:
cargo run -p fastrecorder --features diagnostics -- --self-test-record .local\audio.mp4 --with-audio
```

Create `.local` first. Diagnostic commands are optional development features and excluded from ordinary builds. Snapshot commands display the live selected source in the UI; recording diagnostics capture only the FastRecorder window and disable audio unless `--with-audio` is supplied. Inspect the output for correct duration, nonblank frames, color, audio synchronization and playback. Also exercise the source tabs, refresh, selection, settings, save-dialog cancellation, source closure, resize, save failure, and repeated sessions manually. No runtime tests were executed for the audio/shortcut implementation; they remain with the user.

## Current limits

- Windows 11 x64 is the initial target; macOS capture is not implemented.
- Region capture, pause/resume, webcam and editing are deferred. Audio currently mixes enabled sources into one stereo track. See [release readiness](RELEASE_READINESS.md) for prioritized release gaps and manual checks.
- HDR is blocked conservatively when **any connected desktop display** reports a non-SDR color space, because a selected application window may span displays or move between them. Color state is rechecked every two seconds while recording.
- Hardware availability, codecs, and maximum resolutions depend on the driver and GPU. Missing Media Foundation components are reported as initialization errors.
- A minimized or unavailable source may stop supplying frames; once a first frame arrives, the last frame is repeated until stop or source closure.
- Automatic mid-recording encoder switching is not supported. Device loss ends the session and attempts finalization.
- No telemetry, accounts, uploads, background service, installer, or updater.

## License

GPL-3.0-only. Slint is used under its GPL licensing option. See [LICENSE](../LICENSE).

Icons: Lucide (ISC); NVENC declarations are adapted from ViliamVadocz/nvidia-video-codec-sdk (MIT) and NVIDIA's permissively licensed header. C enum declarations use ABI-equivalent integer newtypes to allow zero-initialized reserved fields and unknown driver values safely. Intel API headers are MIT; rav1e is BSD-2-Clause. See [third-party notices](../THIRD_PARTY_NOTICES.md) for provenance and license links.
