//! Stand-ins for three glibc 2.38 functions on an older glibc.
//!
//! The prebuilt ONNX Runtime for Linux calls the C23 versions of `strtol`,
//! `strtoll` and `strtoull`. glibc adds them in 2.38, but the Linux builds use
//! Ubuntu 22.04 (glibc 2.35) so that the program also runs on older systems.
//! The C23 versions only add the `0b` prefix for binary numbers, which ONNX
//! Runtime does not parse. So the old functions give the same results.
//! build.rs enables this module only when the build machine has glibc < 2.38.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_long, c_longlong, c_ulonglong};

unsafe extern "C" {
    fn strtol(s: *const c_char, end: *mut *mut c_char, base: c_int) -> c_long;
    fn strtoll(s: *const c_char, end: *mut *mut c_char, base: c_int) -> c_longlong;
    fn strtoull(s: *const c_char, end: *mut *mut c_char, base: c_int) -> c_ulonglong;
}

/// # Safety
/// The same as `strtol`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtol(
    s: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_long {
    unsafe { strtol(s, end, base) }
}

/// # Safety
/// The same as `strtoll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoll(
    s: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_longlong {
    unsafe { strtoll(s, end, base) }
}

/// # Safety
/// The same as `strtoull`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoull(
    s: *const c_char,
    end: *mut *mut c_char,
    base: c_int,
) -> c_ulonglong {
    unsafe { strtoull(s, end, base) }
}
