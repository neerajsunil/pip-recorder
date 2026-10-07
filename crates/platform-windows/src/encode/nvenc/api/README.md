# Vendored NVENC declarations

Source: https://github.com/ViliamVadocz/nvidia-video-codec-sdk/tree/master/src/sys
API / structures: NVIDIA Video Codec SDK 12.1, Windows x64.

Local changes: module paths and Rust 2024 unsafe extern declarations; generated
layout tests omitted; C enums represented as transparent integer newtypes so
zero-initialized reserved fields and unknown driver values remain valid Rust.
The layout and function table match the published Windows declarations.

The app loads the installed driver DLL from System32. It does not link these
standalone extern declarations or bundle a CUDA/NVIDIA runtime.

License: LICENSE covers the Rust bindings; NVIDIA-LICENSE covers the header ABI.
