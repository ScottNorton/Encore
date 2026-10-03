//! Build script — compiles vendored C libraries for ARM cross-compilation.

fn main() {
    // When this firmware was built. web/certstore.rs treats it as the earliest the real
    // time can be, because a speaker's clock reads 1970 until NTP sets it.
    // SOURCE_DATE_EPOCH, when set, wins so reproducible builds stay possible.
    let build_unix = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    println!("cargo:rustc-env=ENCORE_BUILD_UNIX={build_unix}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // Rerun when the code changes so the stamp tracks the build (the C compile below
    // adds its own rerun rules, which would otherwise be the only ones).
    println!("cargo:rerun-if-changed=src");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        // Compile vendored Google libsbc for SBC Bluetooth audio codec decoding.
        // The cc crate uses the cross-compiler from Cargo's target config
        // (e.g., arm-linux-gnueabihf-gcc for armv7 targets).
        // Only compiled for Linux — MSVC lacks C11 alignas/VLA support.
        cc::Build::new()
            .file("csrc/sbc/sbc.c")
            .file("csrc/sbc/bits.c")
            .include("csrc/sbc")
            .warnings(false)
            // Disable _FORTIFY_SOURCE: glibc cross-compiler enables it by default,
            // which emits __memset_chk calls that don't exist in musl.
            .flag("-U_FORTIFY_SOURCE")
            .compile("sbc");

        // Compile vendored libfreeaptx for aptX / aptX HD codec decoding.
        // LGPL-2.1 — statically linked into Encore binary.
        cc::Build::new()
            .file("csrc/aptx/freeaptx.c")
            .include("csrc/aptx")
            .warnings(false)
            .flag("-U_FORTIFY_SOURCE")
            .compile("freeaptx");
    }
}
