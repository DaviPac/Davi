//! Embeds the app icon into the Windows executable.
//!
//! GPUI loads icon resource #1 from the running module for its windows, and
//! Explorer/the taskbar use the same resource for the `.exe`.

fn main() {
    println!("cargo:rerun-if-changed=resources/windows");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("resources/windows/davi.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
