// The exe's own icon (Explorer, the taskbar, shortcuts), from assets/icon.ico.
fn main() {
    println!("cargo:rerun-if-changed=assets/launcher.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    let _ = embed_resource::compile("assets/launcher.rc", embed_resource::NONE);
}
