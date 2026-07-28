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

    warn_if_release_sidecar_cannot_bake_fonts(src);
}

/// A **release** build embeds whatever sidecar happens to be sitting in `resources/`.
/// A missing one, or one frozen with `-Lean`, cannot run `bake-font` — so a shipped
/// app translates `unity-textbl` games but renders the result as tofu boxes, which is
/// exactly how it shipped once. The freeze script records its profile beside the exe;
/// shout at build time when that profile isn't shippable. A warning, not an error:
/// a release build of the other engines is still perfectly valid.
fn warn_if_release_sidecar_cannot_bake_fonts(src: &Path) {
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return;
    }
    let marker = src.with_file_name("rpgtl-unity.profile");
    println!("cargo:rerun-if-changed=resources/unity/rpgtl-unity.profile");
    let profile = std::fs::read_to_string(&marker).unwrap_or_default();
    let why = if !src.is_file() {
        "no frozen Unity sidecar is present"
    } else if profile.trim() != "full" {
        "the frozen Unity sidecar was built with -Lean (no numpy/scipy/PIL/freetype)"
    } else {
        return;
    };
    println!(
        "cargo:warning=RELEASE BUILD: {why} — `bake-font` will fail, so Unity (TextTable) \
         games will show Thai as tofu boxes. Run `pwsh scripts/freeze-unity-sidecar.ps1` \
         (default profile) and rebuild before shipping."
    );
}
