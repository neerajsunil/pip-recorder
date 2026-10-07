fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile_color_shaders();
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag_if_supported("/EHsc")
            .include("vendor")
            .file("src/encode/intel/bridge.cpp")
            .compile("fastrecorder_intel");
        println!("cargo:rerun-if-changed=src/intel_bridge.cpp");
        println!("cargo:rerun-if-changed=vendor/vpl");
    }
}

#[cfg(windows)]
fn compile_color_shaders() {
    use windows::{
        Win32::Graphics::Direct3D::Fxc::*,
        core::{PCSTR, s},
    };
    println!("cargo:rerun-if-changed=src/capture/color.hlsl");
    let source = std::fs::read("src/capture/color.hlsl").expect("Read capture color shader");
    let directory = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    for (entry, target, filename) in [
        (s!("vertex"), s!("vs_5_0"), "color-vertex.cso"),
        (s!("pixel"), s!("ps_5_0"), "color-pixel.cso"),
    ] {
        let (mut blob, mut errors) = (None, None);
        unsafe {
            if let Err(error) = D3DCompile(
                source.as_ptr().cast(),
                source.len(),
                PCSTR::null(),
                None,
                None,
                entry,
                target,
                D3DCOMPILE_OPTIMIZATION_LEVEL3 | D3DCOMPILE_WARNINGS_ARE_ERRORS,
                0,
                &mut blob,
                Some(&mut errors),
            ) {
                let detail = errors
                    .map(|blob| {
                        String::from_utf8_lossy(std::slice::from_raw_parts(
                            blob.GetBufferPointer().cast(),
                            blob.GetBufferSize(),
                        ))
                        .into_owned()
                    })
                    .unwrap_or_else(|| error.to_string());
                panic!("Compile {filename}: {detail}");
            }
            let blob = blob.unwrap();
            std::fs::write(
                directory.join(filename),
                std::slice::from_raw_parts(
                    blob.GetBufferPointer().cast::<u8>(),
                    blob.GetBufferSize(),
                ),
            )
            .expect("Write capture shader bytecode");
        }
    }
}

#[cfg(not(windows))]
fn compile_color_shaders() {
    panic!("The Windows backend currently requires a Windows build host.");
}
