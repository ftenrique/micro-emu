//! Native Codex desktop window discovery and foreground/minimize control.

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
    const VK_CONTROL: u8 = 0x11;
    const VK_SHIFT: u8 = 0x10;
    const VK_G: u8 = 0x47;
    const VK_OEM_3: u8 = 0xc0;
    const VK_M: u8 = 0x4d;
    const VK_HOME: u8 = 0x24;
    const VK_DOWN: u8 = 0x28;
    const VK_RETURN: u8 = 0x0d;
    const KEYEVENTF_KEYUP: u32 = 0x0002;

    const FEATURED_MODELS: [&str; 3] = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];

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
        fn keybd_event(virtual_key: u8, scan_code: u8, flags: u32, extra_info: usize);
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
        let target = find_codex_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    pub fn minimize() -> Result<(), String> {
        let target = find_codex_window()?;
        unsafe {
            ShowWindow(target, SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn show_and_focus() -> Result<(), String> {
        let target = find_codex_window()?;
        unsafe {
            if IsIconic(target) != 0 {
                ShowWindow(target, SW_RESTORE);
            } else {
                ShowWindow(target, SW_SHOW);
            }

            // A loopback event is not classified as direct user input by
            // Windows. Temporarily joining the foreground input queue plus a
            // topmost pulse makes activation reliable without leaving Codex
            // pinned above other applications.
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
                return Err("Windows refused to activate the Codex window".to_owned());
            }
        }
        Ok(())
    }

    /// Opens Codex's model picker and selects the next featured model.
    pub fn cycle_model(current_model: Option<&str>) -> Result<&'static str, String> {
        show_and_focus()?;
        let target = next_model_index(current_model);

        unsafe {
            key_down(VK_CONTROL);
            key_down(VK_SHIFT);
            tap_key(VK_M);
            key_up(VK_SHIFT);
            key_up(VK_CONTROL);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));

        unsafe {
            tap_key(VK_HOME);
            for _ in 0..target {
                tap_key(VK_DOWN);
            }
            tap_key(VK_RETURN);
        }
        Ok(FEATURED_MODELS[target])
    }

    /// Focuses Codex and opens task search (Ctrl+G).
    pub fn search_tasks() -> Result<(), String> {
        send_control_shortcut(VK_G)
    }

    /// Focuses Codex and toggles its bottom terminal (Ctrl+backtick).
    pub fn toggle_terminal() -> Result<(), String> {
        send_control_shortcut(VK_OEM_3)
    }

    fn send_control_shortcut(key: u8) -> Result<(), String> {
        show_and_focus()?;
        unsafe {
            key_down(VK_CONTROL);
            tap_key(key);
            key_up(VK_CONTROL);
        }
        Ok(())
    }
    fn next_model_index(current_model: Option<&str>) -> usize {
        let current = current_model
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .replace(' ', "-")
            .replace('_', "-");
        let current_index = ["sol", "terra", "luna"].iter().position(|name| {
            current == *name
                || current
                    .strip_suffix(name)
                    .is_some_and(|prefix| prefix.ends_with('-'))
        });
        current_index
            .map(|index| (index + 1) % FEATURED_MODELS.len())
            .unwrap_or(0)
    }

    unsafe fn key_down(key: u8) {
        unsafe { keybd_event(key, 0, 0, 0) };
    }

    unsafe fn key_up(key: u8) {
        unsafe { keybd_event(key, 0, KEYEVENTF_KEYUP, 0) };
    }

    unsafe fn tap_key(key: u8) {
        unsafe {
            key_down(key);
            key_up(key);
        }
    }

    fn find_codex_window() -> Result<Hwnd, String> {
        let mut search = WindowSearch::default();
        unsafe {
            EnumWindows(
                Some(collect_codex_window),
                (&mut search as *mut WindowSearch).cast::<c_void>() as Lparam,
            );
        }
        (search.window != 0)
            .then_some(search.window)
            .ok_or_else(|| "no Codex desktop window was found".to_owned())
    }

    unsafe extern "system" fn collect_codex_window(window: Hwnd, lparam: Lparam) -> Bool {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == 0 || !process_is_codex_desktop(process_id) {
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

    fn process_is_codex_desktop(process_id: u32) -> bool {
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
        if ok == 0 {
            return false;
        }
        is_codex_desktop_executable(&String::from_utf16_lossy(&name[..size as usize]))
    }

    fn is_codex_desktop_executable(path: &str) -> bool {
        let normalized = path.replace('/', "\\").to_ascii_lowercase();
        normalized.ends_with("\\codex.exe")
            || (normalized.ends_with("\\chatgpt.exe") && normalized.contains("\\openai.codex_"))
    }

    #[cfg(test)]
    mod tests {
        use super::{is_codex_desktop_executable, next_model_index};

        #[test]
        fn recognizes_packaged_codex_ui_and_helper() {
            assert!(is_codex_desktop_executable(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_1_x64\app\ChatGPT.exe"
            ));
            assert!(is_codex_desktop_executable(r"C:\apps\codex.exe"));
            assert!(!is_codex_desktop_executable(r"C:\apps\ChatGPT.exe"));
        }

        #[test]
        fn featured_models_cycle_and_unknown_models_start_at_sol() {
            assert_eq!(next_model_index(Some("gpt-5.6-sol")), 1);
            assert_eq!(next_model_index(Some("GPT-5.6-TERRA")), 2);
            assert_eq!(next_model_index(Some("gpt-5.6-luna")), 0);
            assert_eq!(next_model_index(Some("GPT 5.6 Sol")), 1);
            assert_eq!(next_model_index(Some("Terra")), 2);
            assert_eq!(next_model_index(Some("gpt-5.5")), 0);
            assert_eq!(next_model_index(None), 0);
        }
    }
}

#[cfg(windows)]
pub use native::{
    cycle_model, is_foreground, minimize, search_tasks, show_and_focus, toggle_terminal,
};

#[cfg(not(windows))]
pub fn is_foreground() -> Result<bool, String> {
    Err("Codex window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn minimize() -> Result<(), String> {
    Err("Codex window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn show_and_focus() -> Result<(), String> {
    Err("Codex window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn cycle_model(_current_model: Option<&str>) -> Result<&'static str, String> {
    Err("Codex model control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn search_tasks() -> Result<(), String> {
    Err("Codex task search control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn toggle_terminal() -> Result<(), String> {
    Err("Codex terminal control is only available on Windows".to_owned())
}
