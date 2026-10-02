fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/fastrecorder.ico");
        println!("cargo:rerun-if-changed=assets/windows.manifest");
        winresource::WindowsResource::new()
            .set_icon("assets/fastrecorder.ico")
            .set_manifest_file("assets/windows.manifest")
            .set("ProductName", "FastRecorder")
            .set("FileDescription", "FastRecorder screen recorder")
            .set("OriginalFilename", "fastrecorder.exe")
            .set(
                "LegalCopyright",
                "Copyright © 2026 FastRecorder contributors. GPL-3.0-only.",
            )
            .compile()
            .expect("Windows icon, manifest and version resources must compile");
    }
    slint_build::compile_with_config(
        "ui/main.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .expect("Slint UI must compile");
}
