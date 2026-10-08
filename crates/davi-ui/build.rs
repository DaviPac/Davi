//! Embeds the Windows application manifest when cross-compiling.
//!
//! The manifest opts into Common Controls v6 (needed for `TaskDialogIndirect`,
//! which GPUI uses for dialogs) and per-monitor DPI awareness. GPUI embeds an
//! equivalent manifest itself, but its build script only does so when the
//! *host* is Windows; cross builds would otherwise ship without one and fail
//! to start with "entry point TaskDialogIndirect not found".

fn main() {
    println!("cargo:rerun-if-changed=resources/windows");

    let target_is_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let host_is_windows = cfg!(windows);
    if target_is_windows && !host_is_windows {
        embed_resource::compile("resources/windows/davi.rc", embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
