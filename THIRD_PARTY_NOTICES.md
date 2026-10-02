# Third-party notices

FastRecorder source is licensed GPL-3.0-only. Dependency and vendored-file licenses remain in effect. Cargo.lock records exact dependency versions; source distributions include the vendored notices listed below.

| Component | Use | License / source |
|---|---|---|
| Slint | UI, rendering and compiler | GPL-3.0-only option; [Slint](https://github.com/slint-ui/slint) |
| Lucide | UI icons | ISC; [local notice](crates/app/ui/icons/LICENSE) |
| NVIDIA Video Codec API declarations | NVENC ABI | NVIDIA permissive notice and MIT adaptation; [local provenance/licenses](crates/windows/src/nvenc_api/README.md) |
| Intel oneVPL / Media SDK headers | Native bridge ABI | MIT; [local license](crates/windows/vendor/INTEL-LICENSE), [provenance](crates/windows/vendor/README.md) |
| rav1e | Software AV1 | BSD-2-Clause; [rav1e](https://github.com/xiph/rav1e) |
| Rust Windows bindings | Windows API access | MIT or Apache-2.0; [windows-rs](https://github.com/microsoft/windows-rs) |
| WGPU | GPU UI rendering | MIT or Apache-2.0; [wgpu](https://github.com/gfx-rs/wgpu) |
| tray-icon / muda | Windows tray/menu integration | MIT or Apache-2.0; [tray-icon](https://github.com/tauri-apps/tray-icon) |
| winresource | Executable resources at build time | MIT; [winresource](https://github.com/BenjaminRi/winresource) |

Drivers, Windows Media Foundation, WASAPI and platform SDKs are provided by their vendors; FastRecorder does not redistribute NVIDIA or Intel driver DLLs. FFmpeg is not bundled. Pillow is optional for regenerating code-drawn documentation/icon assets and is not an application dependency.

The package script includes a generated list of dependency names, versions and SPDX license expressions plus available registry license/notice files. Those expressions are metadata, not replacements for the original license texts. Release maintainers must review licenses and include additional required notices when changing dependencies. Corresponding FastRecorder source is available in this repository and each release's source archives; retain LICENSE and notices when redistributing binaries.
