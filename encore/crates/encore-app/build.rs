fn main() {
    // Force recompile when the embedded frontend files change.
    // cargo:rerun-if-changed on a directory only detects add/remove, not content changes.
    // So we also track the key files that change on each WASM rebuild.
    println!("cargo:rerun-if-changed=../../../build/app-dist");
    println!("cargo:rerun-if-changed=../../../build/app-dist/pkg/encore_wasm_bg.wasm");
    println!("cargo:rerun-if-changed=../../../build/app-dist/pkg/encore_wasm.js");
    println!("cargo:rerun-if-changed=../../../build/app-dist/index.html");
    tauri_build::build();
}
