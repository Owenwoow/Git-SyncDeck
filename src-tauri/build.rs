fn main() {
    // 测试程序也要嵌入 Common Controls v6 清单，否则用到 Tauri 模拟运行时的测试在 Windows 上
    // 会以 STATUS_ENTRYPOINT_NOT_FOUND 启动失败（正式程序的清单由 tauri-build 负责）
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("windows-test.manifest");
        println!("cargo:rustc-link-arg-tests=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-tests=/MANIFESTINPUT:{}", manifest.display());
        println!("cargo:rerun-if-changed=windows-test.manifest");
    }
    tauri_build::build()
}
