# FastRecorder implementation review — 2026-10-02

This is a source review and implementation pass, not a recording-quality benchmark. Recording/playback, GPU/driver compatibility, audio synchronization, tray restoration and failure-path tests remain with the user. The app now implements SDR video with optional desktop/microphone audio; release readiness still requires runtime validation.

## Implemented in this pass

- NVIDIA: high-quality tuning, P5 default, spatial AQ and quarter-resolution two-pass bitrate encoding. Added optional CQP 20 quality mode (single-pass, variable file size); VBR and existing High/Recommended/Low/Custom bitrate choices remain. Driver rejection of optional AQ/multipass retries the same codec and HQ tuning without those features, and reports the adjustment.
- Intel: best-quality TU1 request instead of balanced TU4. Reject a driver query that silently enables frame reordering, which the muxer cannot represent correctly.
- Software AV1: speed 8 instead of speed 10, with eight-frame RDO lookahead. This costs CPU time and may skip frames at demanding resolutions; it remains an explicit choice rather than automatic AV1 fallback.
- Preferences and destination folder survive restart. Frame rate deliberately resets to 30 fps. GPU preferences use a name rather than a stale adapter ordinal, and unsupported restored encoder choices reset to Automatic. The last saved recording is remembered if its file still exists. Diagnostic snapshots/recordings do not write user preferences.
- Configurable native global shortcuts: Ctrl+Shift+F9 starts and Ctrl+Shift+F10 stops by default. Assign identical bindings to toggle; blank disables an action. Apply validates before replacing registrations, suppresses repeats and restores prior bindings if registration fails. No keyboard hook or background service.
- Studio desktop/microphone toggles, remembered device IDs and 0–200% recording gains, native WASAPI loopback/input, bounded stereo mixing/resampling and Media Foundation AAC-LC 48 kHz/192 kbps. Audio is added to each supported encoder's MP4 without video re-encoding. An explicit disconnected device blocks recording until changed or disabled; device/encoding failures are reported and captured video is finalized where possible. Desktop audio defaults on; microphone defaults off. Sources are mixed into one track, not separate editable tracks.
- Advanced settings use Video, Audio, Shortcuts and General sidebar pages, with smaller subsections and a separate Info page for technical details.
- Include/exclude cursor setting affects preview and recording. Play video and Show in folder use native Windows shell APIs rather than constructing a command string. Recording state/time appear in the window/taskbar title.
- Main recording status is codec/FPS only; vendor/preset/rate-control and destination details stay in settings. Windows restore/focus handling refreshes the graphics surface through taskbar and tray, suppresses minimized redraws and retries a post-restore frame. The user-reported disappearing UI needs runtime confirmation on the affected GPU.
- Embedded multi-resolution executable icon, version metadata and per-monitor-DPI manifest; software fallback exits the original launcher so only the replacement process remains. Local fatal diagnostics, pinned CI actions, packaging/checksums, dependency notices and community/security documentation are included.
- Recording/preview workers catch Rust panics and report a terminal/error state, rather than leaving the studio stuck indefinitely. Panic diagnostics are written locally; incomplete media remains subject to the existing MP4 recovery limitation.

## Bugs and reliability issues addressed

| Issue | Change |
|---|---|
| Studio minimized and reported recording before a capture frame existed | Emit Started after capture supplies a usable frame and encoding accepts it. |
| Starting could not be cancelled from the main button | Enable Cancel recording during Starting, alongside tray/shortcut cancellation. |
| Switching sources blocked the UI while joining a capture worker | Stop the previous preview through Drop; its separate channel isolates late frames. |
| Refresh removed a source but left its preview active | Stop and clear preview/source state when the selected target disappears. |
| Hidden preview could use up its initial timeout | Suspend the first-frame deadline while hidden; accept only usable frames as seen. |
| Starting a recording could display a late frame from the old preview | Allocate a separate recording preview channel and clear readiness. |
| A source closed during recording but UI reported an ordinary stop | Finalize captured frames and surface the source-closure warning; invalidate unavailable source after finishing. |
| Window handles can be reused for another process | Store/check the process ID along with each application window handle. |
| Intel bridge could read beyond truncated NV12 input | Validate the buffer length before entering the C++ bridge. |
| Empty NVENC output could produce an invalid raw slice | Check pointer/length, then unlock/unmap on error. |
| Backend-specific keyframe controls appeared usable when ignored | Disable them for Media Foundation. |
| CQP would have displayed the computed bitrate as its actual rate | Show CQP/variable file size instead; verified recording info identifies the applied mode. |
| Low disk space was discovered only through a failed write | Require 256 MB initially; poll every two seconds and attempt finalization below 64 MB. |
| Failed startup left an empty partial file | Remove only zero-length temporary files; retain potentially useful incomplete media. |
| Closing after saving briefly reopened/restarted preview | Skip restoration and preview startup when closing. |
| Release startup failures could disappear without an explanation | Show a native fatal-error message and return a failed exit status. |

## Remaining features, in priority order

| Priority | Gap | Implementation direction |
|---|---|---|
| Before public release | Validate native audio pipeline | Test each video encoder with desktop-only, microphone-only, both and silence; check AAC priming/end padding, long-session sync, device changes and recovery. Shared QPC alignment and linear resampling are implemented; driver behavior is not verified yet. |
| Before public release | Crash-resilient recordings | Fragmented MP4 or a journaled muxer, then a recovery/remux flow for interrupted recordings. Current MP4 metadata is finalized on Stop; retained partial files are not guaranteed playable after a crash/power loss. |
| Before public release | Reordered video output | Bounded NVENC input/output rings, NEED_MORE_INPUT handling, flushing, and proper decode/presentation timestamps and MP4 composition offsets. This unlocks B-frames/lookahead; changing flags alone is unsafe. |
| High | Pause/resume | Exclude paused duration from the shared audio/video clock; controls in both studio and tray. A UI-only pause that repeats frames would be incorrect. |
| High | Region capture / output size | In-app crop selection and a preview of the recorded canvas; matching bitrate calculations and an explicit scaling policy. |
| High | AMD AV1/HEVC and improved Intel path | Capability-driven AMF/D3D12 support plus GPU-resident Intel surfaces. AMD currently has only the generic Media Foundation hardware H.264 fallback. |
| High | HDR / mixed HDR displays | Capture FP16, tone-map intentionally to SDR or encode supported 10-bit HDR with correct metadata. Current conservative HDR block is broader than necessary. |
| Useful | Countdown / additional audio controls | Cancellable startup countdown, live mute/meters, optional separate audio tracks and per-application sound. Extend configurable shortcuts for future pause/resume. |
| Useful | Recording library | Small recent-recordings view with duration/size, play/reveal/delete, and interrupted-file recovery. Current UI has the last recording only. |
| Distribution | Installer, signed binary, release builds | Reproducible Windows packaging and dependency/license notices. Signing reduces Windows trust friction; no updater/account system is required for a first release. |

## Manual checks for the user

1. Record NVIDIA AV1, HEVC and H.264 with VBR and CQP; verify codec, duration, colors, scrolling/text detail, stopping and playback. Compare OBS using the same source resolution, frame rate, GPU, codec and comparable size/quality settings.
2. Repeat with Intel H.264/HEVC/AV1 where supported, Windows software H.264 and rav1e AV1. Check skipped-frame counts at 1080p, 1440p and 4K, 30/60 fps.
3. Stop from tray and shortcut while minimized; restore repeatedly; cancel during Starting; close the studio while recording/saving. Verify one final file and no transparent studio.
4. Switch sources quickly; open settings before preview starts; leave it hidden longer than eight seconds; show preview again; resize/minimize/close a captured application; unplug a selected display.
5. Toggle cursor, choose a folder with spaces/Unicode, restart, and verify preferences and last-recording Play/Show in folder. Confirm 30 fps on each launch and timestamp filenames on each new recording.
6. Exercise a removed/unwritable save directory, destination created after validation, full/low-space volume and device loss. Ensure existing files are not overwritten and error details identify retained incomplete media.
7. Run two copies to check hotkey conflicts; deny borderless permission; use missing encoder/player components; verify clear errors and that recording controls remain reachable.
8. Record desktop only, mic only, both and both disabled with NVIDIA, Intel and software encoders. Play a sound and speak/produce a visible sync cue near the beginning and end; check silence, mono inputs, stereo balance, 44.1/48/96 kHz devices and long-session synchronization. Audio volume changes must not change Windows device volume.
9. Choose explicit audio devices, restart and reconnect them, refresh devices, change system defaults before recording and unplug an enabled source during recording. Check that startup failures are visible and interruptions save media with a warning. Microphone must remain off on a fresh install until enabled.
10. Apply separate, identical, blank, invalid and already-used shortcuts. Verify start does not stop a current recording, stop cancels Starting or stops while minimized, identical bindings toggle, rejected changes restore prior registrations and applied bindings survive restart. Inspect each settings page at minimum window size and high DPI.

Build checks do not substitute for these runtime checks. No recording, playback or test suites were executed in this pass.

## Dependency audit — 2026-10-02

`cargo-audit 0.22.2` reported **zero known vulnerability advisories** for the committed lockfile. It reported maintenance warnings for `bincode 2.0.1`, `paste 1.0.15`, and `ttf-parser 0.25.1`. `paste` is a rav1e build dependency; `ttf-parser` appears through Linux window decorations. `bincode` is in the complete lockfile but is not active in the current default Windows dependency graph. These warnings are tracked for upstream dependency updates, not suppressed. A clean advisory scan is not a guarantee that the application or dependencies are vulnerability-free. The scheduled security workflow checks for newly published advisories.

## Primary references

- [NVIDIA encoding configuration and rate control](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/nvenc-video-encoder-api-prog-guide/index.html)
- [OBS recording baseline](https://obsproject.com/kb/advanced-recording-settings-guide)
- [Microsoft WASAPI loopback recording](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording)
- [Microsoft AAC encoder media types and operation](https://learn.microsoft.com/en-us/windows/win32/medfound/aac-encoder)
- [WASAPI packet timestamp and buffer contract](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudiocaptureclient-getbuffer)
