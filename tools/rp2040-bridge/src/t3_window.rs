//! Native T3 Code Desktop window discovery, deep-link routing, dictation, and
//! keyboard shortcut control.
//!
//! T3 Code Desktop ships as `T3 Code.exe`, `T3 Code (Nightly).exe`, or
//! `T3 Code (Beta).exe` under `%LOCALAPPDATA%\Programs\t3code\`. Helper
//! processes such as `t3-resource-monitor.exe` are excluded explicitly so they
//! are never mistaken for the desktop window.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    type Bool = i32;
    type Handle = isize;
    type Hwnd = isize;
    type Lparam = isize;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const GWL_EXSTYLE: i32 = -20;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_SHIFT: u8 = 0x10;
    const VK_CONTROL: u8 = 0x11;
    const VK_MENU: u8 = 0x12;
    const VK_ESCAPE: u8 = 0x1B;
    const VK_B: u8 = 0x42;
    const VK_C: u8 = 0x43;
    const VK_F: u8 = 0x46;
    const VK_H: u8 = 0x48;
    const VK_J: u8 = 0x4A;
    const VK_K: u8 = 0x4B;
    const VK_M: u8 = 0x4D;
    const VK_P: u8 = 0x50;
    const VK_S: u8 = 0x53;
    const VK_U: u8 = 0x55;
    const VK_LWIN: u8 = 0x5B;
    const WS_EX_TOOLWINDOW: i32 = 0x0000_0080;
    const SW_MINIMIZE: i32 = 6;
    const SW_RESTORE: i32 = 9;
    const SW_SHOW: i32 = 5;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_SHOWWINDOW: u32 = 0x0040;
    const HWND_TOPMOST: Hwnd = -1;
    const HWND_NOTOPMOST: Hwnd = -2;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    const FOCUS_COMPOSER_SCRIPT: &str = include_str!("focus_agent_composer.ps1");

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
        fn keybd_event(virtual_key: u8, scan_code: u8, flags: u32, extra_info: usize);
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

    /// Whether the last press opened Windows dictation in T3 Code.
    static DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn is_foreground() -> Result<bool, String> {
        let target = find_t3_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    /// True while T3 Code Desktop has a top-level window.
    pub fn desktop_running() -> bool {
        find_t3_window().is_ok()
    }

    pub fn microphone_active() -> bool {
        DICTATION_ACTIVE.load(Ordering::SeqCst)
    }

    pub fn set_microphone(pressed: bool) -> Result<(), String> {
        if pressed {
            let target = find_t3_window()?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the T3 Code window is no longer focused".to_owned());
            }
            focus_composer(target)?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err(
                    "the T3 Code window lost focus while selecting its composer".to_owned(),
                );
            }
            unsafe {
                key_down(VK_LWIN);
                tap_key(VK_H);
                key_up(VK_LWIN);
            }
            DICTATION_ACTIVE.store(true, Ordering::SeqCst);
        } else if DICTATION_ACTIVE.swap(false, Ordering::SeqCst) {
            unsafe {
                tap_key(VK_ESCAPE);
            }
        }
        Ok(())
    }

    fn focus_composer(window: Hwnd) -> Result<(), String> {
        let script =
            format!("$WindowHandle = {window}\n$AgentName = 'T3 Code'\n{FOCUS_COMPOSER_SCRIPT}");
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("could not start T3 Code composer automation: {error}"))?;
        child
            .stdin
            .take()
            .ok_or_else(|| "could not open T3 Code composer automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send T3 Code composer automation: {error}"))?;
        let status = child
            .wait()
            .map_err(|error| format!("T3 Code composer automation did not finish: {error}"))?;
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut output) = child.stdout.take() {
            let _ = output.read_to_string(&mut stdout);
        }
        if let Some(mut error) = child.stderr.take() {
            let _ = error.read_to_string(&mut stderr);
        }
        if !status.success() || !stderr.trim().is_empty() {
            return Err(format!(
                "T3 Code composer automation failed: {}",
                stderr.trim()
            ));
        }
        if stdout.lines().any(|line| line.trim() == "focused") {
            Ok(())
        } else {
            Err(format!(
                "T3 Code composer automation returned an unexpected result: {}",
                stdout.trim()
            ))
        }
    }

    /// Switches T3 Code Desktop to the thread behind a `t3:<threadId>` card
    /// via `t3code://threads/<environmentId>/<threadId>`.
    pub fn request_session_selection(task_id: &str) {
        let Some(thread_id) = super::desktop_thread_id(task_id) else {
            return;
        };
        let Some(env_id) = crate::t3_state::t3_environment_id() else {
            let _ = show_and_focus();
            return;
        };
        let url = format!("t3code://threads/{env_id}/{thread_id}");
        if let Err(error) = open_deep_link(&url) {
            eprintln!("T3 Code thread selection failed: {error}");
            crate::diaglog::log(&format!("t3 thread selection failed: {error}"));
        }
    }

    /// Opens a new thread in T3 Code Desktop via `t3code://new`.
    pub fn start_new_session() -> Result<(), String> {
        open_deep_link("t3code://new")
    }

    pub fn toggle_command_palette() -> Result<(), String> {
        send_shortcut(true, false, false, VK_K)
    }

    pub fn search_projects() -> Result<(), String> {
        send_shortcut(true, true, false, VK_F)
    }

    pub fn toggle_terminal() -> Result<(), String> {
        send_shortcut(true, false, false, VK_J)
    }

    pub fn toggle_preview() -> Result<(), String> {
        send_shortcut(true, true, false, VK_J)
    }

    pub fn toggle_sidebar() -> Result<(), String> {
        send_shortcut(true, false, false, VK_B)
    }

    pub fn toggle_model_picker() -> Result<(), String> {
        send_shortcut(true, true, false, VK_M)
    }

    pub fn settle_thread() -> Result<(), String> {
        send_shortcut(true, true, false, VK_S)
    }

    pub fn pin_thread() -> Result<(), String> {
        send_shortcut(true, true, false, VK_P)
    }

    pub fn copy_thread_reference() -> Result<(), String> {
        send_shortcut(true, true, false, VK_C)
    }

    pub fn open_usage() -> Result<(), String> {
        send_shortcut(true, false, false, VK_U)
    }

    fn send_shortcut(ctrl: bool, shift: bool, alt: bool, key: u8) -> Result<(), String> {
        let was_foreground = is_foreground().unwrap_or(false);
        show_and_focus()?;
        if !was_foreground {
            thread::sleep(Duration::from_millis(40));
        }
        unsafe {
            if ctrl {
                key_down(VK_CONTROL);
            }
            if shift {
                key_down(VK_SHIFT);
            }
            if alt {
                key_down(VK_MENU);
            }
            tap_key(key);
            if alt {
                key_up(VK_MENU);
            }
            if shift {
                key_up(VK_SHIFT);
            }
            if ctrl {
                key_up(VK_CONTROL);
            }
        }
        Ok(())
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

    fn open_deep_link(url: &str) -> Result<(), String> {
        Command::new("cmd.exe")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(drop)
            .map_err(|error| error.to_string())
    }

    pub fn minimize() -> Result<(), String> {
        let target = find_t3_window()?;
        unsafe {
            ShowWindow(target, SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn show_and_focus() -> Result<(), String> {
        let target = find_t3_window()?;
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
                return open_deep_link("t3code://")
                    .map_err(|error| format!("T3 Code focus fallback failed: {error}"));
            }
        }
        Ok(())
    }

    fn find_t3_window() -> Result<Hwnd, String> {
        let mut search = WindowSearch::default();
        unsafe {
            EnumWindows(
                Some(collect_t3_window),
                (&mut search as *mut WindowSearch).cast::<c_void>() as Lparam,
            );
        }
        (search.window != 0)
            .then_some(search.window)
            .ok_or_else(|| "no T3 Code Desktop window was found".to_owned())
    }

    unsafe extern "system" fn collect_t3_window(window: Hwnd, lparam: Lparam) -> Bool {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == 0 || !process_is_t3_desktop(process_id) {
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

    fn process_image_path(process_id: u32) -> Option<String> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process == 0 {
            return None;
        }
        let mut name = [0_u16; 32_768];
        let mut size = name.len() as u32;
        let ok = unsafe { QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut size) };
        unsafe {
            CloseHandle(process);
        }
        (ok != 0).then(|| String::from_utf16_lossy(&name[..size as usize]))
    }

    fn process_is_t3_desktop(process_id: u32) -> bool {
        crate::proc_cache::cached("t3", process_id, |pid| {
            process_image_path(pid).is_some_and(|path| is_t3_desktop_executable(&path))
        })
    }

    pub(super) fn is_t3_desktop_executable(path: &str) -> bool {
        let normalized = path.replace('/', "\\").to_ascii_lowercase();
        if normalized.contains("\\node_modules\\") {
            return false;
        }
        let Some(file_name) = normalized.rsplit('\\').next() else {
            return false;
        };
        file_name.starts_with("t3 code") && file_name.ends_with(".exe")
    }

    #[cfg(test)]
    mod tests {
        use super::is_t3_desktop_executable;

        #[test]
        fn recognizes_only_t3_code_desktop_executables() {
            assert!(is_t3_desktop_executable(
                r"C:\Users\admin\AppData\Local\Programs\t3code\T3 Code (Nightly).exe"
            ));
            assert!(is_t3_desktop_executable(
                r"C:\Users\admin\AppData\Local\Programs\t3code\T3 Code.exe"
            ));
            assert!(is_t3_desktop_executable(
                r"C:\Users\admin\AppData\Local\Programs\t3code\T3 Code (Beta).exe"
            ));
            assert!(!is_t3_desktop_executable(
                r"C:\Users\admin\AppData\Local\Programs\t3code\resources\t3-resource-monitor.exe"
            ));
            assert!(!is_t3_desktop_executable(r"C:\Program Files\nodejs\node.exe"));
        }
    }
}

#[cfg(windows)]
pub use native::{
    copy_thread_reference, desktop_running, is_foreground, microphone_active, minimize,
    open_usage, pin_thread, request_session_selection, search_projects, set_microphone,
    settle_thread, show_and_focus, start_new_session, toggle_command_palette, toggle_model_picker,
    toggle_preview, toggle_sidebar, toggle_terminal,
};

/// Extracts and validates the bare thread UUID from a `t3:<threadId>` task id.
fn desktop_thread_id(task_id: &str) -> Option<&str> {
    let id = task_id.trim().strip_prefix("t3:")?;
    crate::t3_state::is_valid_t3_identifier(id).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::desktop_thread_id;

    #[test]
    fn validates_t3_thread_ids_strictly() {
        assert_eq!(
            desktop_thread_id("t3:013c7e24-7030-46f3-ae38-282c89051064"),
            Some("013c7e24-7030-46f3-ae38-282c89051064")
        );
        assert_eq!(desktop_thread_id("t3:"), None);
        assert_eq!(desktop_thread_id("t3:abc&calc"), None);
        assert_eq!(desktop_thread_id("t3:abc/def"), None);
        assert_eq!(desktop_thread_id("t3:abc def"), None);
        assert_eq!(desktop_thread_id("claude:local_abc"), None);
    }
}

#[cfg(not(windows))]
pub fn is_foreground() -> Result<bool, String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn minimize() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn show_and_focus() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn request_session_selection(_task_id: &str) {}
#[cfg(not(windows))]
pub fn start_new_session() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn microphone_active() -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_microphone(_pressed: bool) -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn desktop_running() -> bool {
    false
}
#[cfg(not(windows))]
pub fn toggle_command_palette() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn search_projects() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn toggle_terminal() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn toggle_preview() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn toggle_sidebar() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn toggle_model_picker() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn settle_thread() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn pin_thread() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn copy_thread_reference() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn open_usage() -> Result<(), String> {
    Err("T3 Code window control is only available on Windows".to_owned())
}
