
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
            
            let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
            if !stdout.is_null() && stdout != INVALID_HANDLE_VALUE {
                return;
            }

            let attached = AttachConsole(ATTACH_PARENT_PROCESS) != 0;
            if !attached && want_console {
                let _ = AllocConsole();
            }
            if attached || want_console {
                
                redirect_stdio_to_console();
            }
        }
    }

    unsafe fn redirect_stdio_to_console() {
        
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
