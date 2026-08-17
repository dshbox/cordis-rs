//! Fixture compiled to a `cdylib` by the dynamic tests: exports the
//! protocol symbols but reports a foreign build fingerprint, so the loader
//! must reject it. Hand-written to avoid depending on the workspace crates.

#[unsafe(no_mangle)]
pub extern "C" fn cordis_plugin_abi() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn cordis_plugin_fingerprint() -> *const std::ffi::c_char {
    b"cordis-dynamic/1 cordis-rs/0.0.0 rustc/0.0.0-none target/none panic/none\0"
        .as_ptr()
        .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn cordis_plugin_create() -> *mut std::ffi::c_void {
    std::ptr::null_mut()
}
