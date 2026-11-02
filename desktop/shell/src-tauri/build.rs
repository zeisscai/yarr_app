fn main() {
    // 侧车在开发态位于 binaries/yarr-<triple>，把编译目标三元组注入为编译期常量，
    // 避免在 Rust 里手写 aarch64-apple-darwin 之类的字符串。
    if let Ok(target) = std::env::var("TARGET") {
        println!("cargo:rustc-env=YARR_TARGET_TRIPLE={target}");
    }
    tauri_build::build()
}
