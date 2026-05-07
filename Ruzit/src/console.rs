/// Set up a console window for stdout/stderr.
///
/// Ruzit.exe itself ships as the default `console` subsystem, so dev-tool runs
/// (`Ruzit Test/Build/Init`) always have working stdio without any of this.
///
/// Packaged games are switched to the `windows` subsystem at Build time so they
/// launch clean from Explorer. When `--console` is passed (or we detect we're
/// already attached to a parent console), we explicitly redirect stdio to
/// `CONOUT$` / `CONIN$` — `AttachConsole` alone gives us a console but Rust's
/// stdout/stderr handles are still null until we SetStdHandle.
pub fn setup(want_console: bool) {
    #[cfg(windows)]
    win::setup(want_console);
    #[cfg(not(windows))]
    let _ = want_console;
}

#[cfg(windows)]
mod win {
    use std::ptr;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileA, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
    };

    pub fn setup(want_console: bool) {
        unsafe {
            // If we already have a working stdout (i.e. we're a console-subsystem
            // exe, like Ruzit.exe itself), the OS wired everything up. Don't touch.
            let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
            if !stdout.is_null() && stdout != INVALID_HANDLE_VALUE {
                return;
            }

            // Windows-subsystem process (packaged game). Try parent first, fall back
            // to a fresh console when explicitly requested.
            let attached = AttachConsole(ATTACH_PARENT_PROCESS) != 0;
            if !attached && want_console {
                let _ = AllocConsole();
            }
            if attached || want_console {
                // AttachConsole / AllocConsole give us a console but leave stdout
                // null — we need to point std handles at CONOUT$ / CONIN$ ourselves.
                redirect_stdio_to_console();
            }
        }
    }

    unsafe fn redirect_stdio_to_console() {
        // CONOUT$ for stdout + stderr
        let conout = unsafe {
            CreateFileA(
                b"CONOUT$\0".as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if conout != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, conout);
                let _ = SetStdHandle(STD_ERROR_HANDLE, conout);
            }
        }
        // CONIN$ for stdin
        let conin = unsafe {
            CreateFileA(
                b"CONIN$\0".as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if conin != INVALID_HANDLE_VALUE {
            unsafe {
                let _ = SetStdHandle(STD_INPUT_HANDLE, conin);
            }
        }
    }
}
