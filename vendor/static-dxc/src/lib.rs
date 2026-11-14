//! DX12's shader compiler, linked in (#947).
//!
//! Microsoft's DirectXShaderCompiler, built as one static library from its
//! own source (tools/dxc-static), and the one function wgpu-hal's
//! `static-dxc` calls: [`DxcCreateInstance`]. With it linked, no
//! `dxcompiler.dll` or `dxil.dll` ships: since DXC 1.8.2505 the compiler
//! signs DXIL with its internal validator.
//!
//! [`NOTICES`] is the text a Windows build that carries this must ship
//! beside it (`flint dxc notices`).

use std::ffi::{c_long, c_void};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

#[allow(dead_code)]
mod pin {
    include!("../pin.rs");
}
pub use pin::{ARCHIVE_NAME, ARCHIVE_SHA256, DXC_COMMIT, DXC_VERSION, RELEASE_TAG};

/// DXC's licence and third-party notices, as the archive carries them
/// (`LICENSE.TXT`, then `ThirdPartyNotices.txt`).
pub const NOTICES: &str = concat!(
    include_str!(concat!(env!("OUT_DIR"), "/LICENSE.TXT")),
    "\n\n",
    include_str!(concat!(env!("OUT_DIR"), "/ThirdPartyNotices.txt"))
);

/// How the archive was built: source commit, patch, compiler, flags.
pub const BUILD_INFO: &str = include_str!(concat!(env!("OUT_DIR"), "/BUILD-INFO.txt"));

const E_FAIL: c_long = 0x8000_4005_u32 as c_long;

/// [`DxcCreateInstance`](https://learn.microsoft.com/en-us/windows/win32/api/dxcapi/nf-dxcapi-dxccreateinstance):
/// creates a DXC COM object. The first call runs what `dxcompiler.dll`'s
/// `DllMain` would on load (DXC's allocator, file system and option tables).
///
/// # Safety
///
/// `rclsid` and `riid` must point to valid `GUID`s, and `ppv` to a writable
/// `*mut c_void`.
#[allow(non_snake_case)]
pub unsafe extern "system" fn DxcCreateInstance(
    rclsid: *const c_void,
    riid: *const c_void,
    ppv: *mut *mut c_void,
) -> c_long {
    static INIT: Once = Once::new();
    static INIT_OK: AtomicBool = AtomicBool::new(false);
    INIT.call_once(|| {
        // SAFETY: DXC's process-attach, once, as Windows would run it for
        // the DLL. A null module handle is what a static link has.
        let ok = unsafe {
            ffi::DllMain(std::ptr::null_mut(), DLL_PROCESS_ATTACH, std::ptr::null_mut())
        };
        INIT_OK.store(ok != 0, Ordering::Release);
    });
    if !INIT_OK.load(Ordering::Acquire) {
        return E_FAIL;
    }
    // SAFETY: the caller's guarantees, passed on
    unsafe { ffi::DxcCreateInstance(rclsid, riid, ppv) }
}

const DLL_PROCESS_ATTACH: u32 = 1;

mod ffi {
    use std::ffi::{c_long, c_void};

    extern "system" {
        pub fn DxcCreateInstance(
            rclsid: *const c_void,
            riid: *const c_void,
            ppv: *mut *mut c_void,
        ) -> c_long;

        /// dxcompiler's `DllMain`, an ordinary function in a static build.
        pub fn DllMain(module: *mut c_void, reason: u32, reserved: *mut c_void) -> i32;
    }
}
