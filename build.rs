// Compile and link the Windows resource file (app icon, version info) into the
// binary. On non-Windows targets this is a no-op — the rc toolchain is
// Windows-only.
fn main() {
    #[cfg(target_os = "windows")]
    embed_resource::compile("resources/windows.rc", embed_resource::NONE)
        .manifest_required()
        .expect("compile windows resources");
}
