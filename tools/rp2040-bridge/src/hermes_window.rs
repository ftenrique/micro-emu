//! Native Hermes Desktop window discovery and foreground/minimize control.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;

    type Bool = i32;
    type Handle = isize;
    type Hwnd = isize;
    type Lparam = isize;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
    const SW_MINIMIZE: i32 = 6;
    const SW_RESTORE: i32 = 9;
    const SW_SHOW: i32 = 5;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const HWND_TOPMOST: Hwnd = -1;
    const HWND_NOTOPMOST: Hwnd = -2;

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            callback: Option<unsafe extern "system" fn(Hwnd, Lparam) -> Bool>,
            lparam: Lparam,
        ) -> Bool;
        fn GetForegroundWindow() -> Hwnd;
        fn GetWindowThreadProcessId(window: Hwnd, process_id: *mut u32) -> u32;
        fn GetWindowLongW(window: Hwnd, index: i32) -> i32;
        fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> Bool;
        fn IsIconic(window: Hwnd) -> Bool;
        fn IsWindowVisible(window: Hwnd) -> Bool;
        fn ShowWindow(window: Hwnd, command: i32) -> Bool;
        fn BringWindowToTop(window: Hwnd) -> Bool;
        fn SetForegroundWindow(window: Hwnd) -> Bool;
        fn SetWindowPos(
            window: Hwnd,
            insert_after: Hwnd,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> Bool;
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, attach: Bool) -> Bool;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn OpenProcess(desired_access: u32, inherit_handle: Bool, process_id: u32) -> Handle;
        fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
    }

    #[derive(Default)]
    struct WindowSearch {
        window: Hwnd,
        score: u64,
    }

    pub fn is_foreground() -> Result<bool, String> {
        let target = find_hermes_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    pub fn minimize() -> Result<(), String> {
        let target = find_hermes_window()?;
        unsafe {
            ShowWindow(target, SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn show_and_focus() -> Result<(), String> {
        let target = find_hermes_window()?;
        unsafe {
            ShowWindow(
                target,
                if IsIconic(target) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            let foreground = GetForegroundWindow();
            let current_thread = GetCurrentThreadId();
            let foreground_thread = if foreground != 0 {
                GetWindowThreadProcessId(foreground, std::ptr::null_mut())
            } else {
                0
            };
            let attached = foreground_thread != 0
                && foreground_thread != current_thread
                && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
            BringWindowToTop(target);
            SetWindowPos(
                target,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            SetWindowPos(
                target,
                HWND_NOTOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            SetForegroundWindow(target);
            if attached {
                AttachThreadInput(current_thread, foreground_thread, 0);
            }
            if GetForegroundWindow() != target {
                return Err("Windows refused to activate the Hermes window".to_owned());
            }
        }
        Ok(())
    }

    fn find_hermes_window() -> Result<Hwnd, String> {
        let mut search = WindowSearch::default();
        unsafe {
            EnumWindows(
                Some(collect_hermes_window),
                (&mut search as *mut WindowSearch).cast::<c_void>() as Lparam,
            );
        }
        (search.window != 0)
            .then_some(search.window)
            .ok_or_else(|| "no Hermes Desktop window was found".to_owned())
    }

    unsafe extern "system" fn collect_hermes_window(window: Hwnd, lparam: Lparam) -> Bool {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == 0 || !process_is_hermes_desktop(process_id) {
            return 1;
        }
        if unsafe { GetWindowLongW(window, GWL_EXSTYLE) } & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let mut rect = Rect::default();
        unsafe {
            GetWindowRect(window, &mut rect);
        }
        let area = i64::from((rect.right - rect.left).max(0))
            .saturating_mul(i64::from((rect.bottom - rect.top).max(0))) as u64;
        let visible = unsafe { IsWindowVisible(window) != 0 };
        let restored = unsafe { IsIconic(window) == 0 };
        let score = area | (u64::from(restored) << 61) | (u64::from(visible) << 62);
        let search = unsafe { &mut *(lparam as *mut WindowSearch) };
        if score > search.score {
            search.window = window;
            search.score = score;
        }
        1
    }

    fn process_is_hermes_desktop(process_id: u32) -> bool {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process == 0 {
            return false;
        }
        let mut name = [0_u16; 32_768];
        let mut size = name.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut size) };
        unsafe {
            CloseHandle(process);
        }
        ok != 0 && is_hermes_desktop_executable(&String::from_utf16_lossy(&name[..size as usize]))
    }

    fn is_hermes_desktop_executable(path: &str) -> bool {
        path.replace('/', "\\")
            .to_ascii_lowercase()
            .ends_with("\\hermes.exe")
    }

    #[cfg(test)]
    mod tests {
        use super::is_hermes_desktop_executable;

        #[test]
        fn recognizes_only_hermes_desktop_executable() {
            assert!(is_hermes_desktop_executable(
                r"C:\Users\me\AppData\Local\hermes\release\win-unpacked\Hermes.exe"
            ));
            assert!(is_hermes_desktop_executable(r"D:/apps/HERMES.EXE"));
            assert!(!is_hermes_desktop_executable(r"D:\apps\hermes-agent.exe"));
            assert!(!is_hermes_desktop_executable(r"D:\apps\Codex.exe"));
        }
    }
}

#[cfg(windows)]
pub use native::{is_foreground, minimize, show_and_focus};

#[cfg(not(windows))]
pub fn is_foreground() -> Result<bool, String> {
    Err("Hermes window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn minimize() -> Result<(), String> {
    Err("Hermes window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn show_and_focus() -> Result<(), String> {
    Err("Hermes window control is only available on Windows".to_owned())
}
