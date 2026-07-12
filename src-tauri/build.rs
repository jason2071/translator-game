use std::path::Path;

fn main() {
    stage_unity_sidecar();
    tauri_build::build()
}

/// Stage the frozen Unity (Naninovel) helper into `OUT_DIR` so `engine::unity`
/// can `include_bytes!` it. The exe is a large, platform-specific build artifact
/// produced out-of-band by `scripts/freeze-unity-sidecar.ps1` and is **not**
/// committed (git-ignored). When it is absent — a normal `cargo build`/`cargo
/// test`, CI, or a non-Windows host — we stage a zero-byte placeholder instead;
/// the engine treats empty bytes as "no bundled helper" and falls back to system
/// Python. So the build always succeeds whether or not the exe has been frozen.
fn stage_unity_sidecar() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let dst = Path::new(&out_dir).join("rpgtl-unity.exe");
    let src = Path::new("resources/unity/rpgtl-unity.exe");

    println!("cargo:rerun-if-changed=resources/unity/rpgtl-unity.exe");

    if src.is_file() {
        std::fs::copy(src, &dst).expect("copying the frozen Unity sidecar into OUT_DIR");
    } else {
        std::fs::write(&dst, []).expect("writing the empty Unity sidecar placeholder");
    }
}
