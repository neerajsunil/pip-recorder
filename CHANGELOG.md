# Changelog

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
