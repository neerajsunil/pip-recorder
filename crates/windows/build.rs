fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag_if_supported("/EHsc")
            .include("vendor")
            .file("src/intel_bridge.cpp")
            .compile("fastrecorder_intel");
        println!("cargo:rerun-if-changed=src/intel_bridge.cpp");
        println!("cargo:rerun-if-changed=vendor/vpl");
    }
}
