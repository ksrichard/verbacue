//! Windows: hides the empty console window when the app starts from Explorer.
#![allow(unsafe_code)]

use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

/// Detaches from the console when this process is the only one attached to it. That
/// is the case when Explorer started `verbacue.exe`, not a terminal.
pub fn detach_if_alone() {
    let mut ids = [0u32; 2];
    // SAFETY: the buffer is valid for `ids.len()` entries.
    let count = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
    if count == 1 {
        // SAFETY: FreeConsole has no preconditions.
        unsafe { FreeConsole() };
    }
}
