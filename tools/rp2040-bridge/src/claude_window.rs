//! Native Claude Desktop window discovery and foreground/minimize control.
//!
//! The desktop app ships as `Claude.exe` (MSIX under `WindowsApps`, or the
//! Squirrel install under `%LOCALAPPDATA%\AnthropicClaude`). The Claude Code
//! processes it hosts are also named `claude.exe` but live under
//! `...\Claude\claude-code\<version>\`; they own no top-level window and are
//! excluded explicitly so a running CLI is never mistaken for the app.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io::{Read, Write};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};

    type Bool = i32;
    type Handle = isize;
    type Hwnd = isize;
    type Lparam = isize;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const GWL_EXSTYLE: i32 = -20;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_LWIN: u8 = 0x5B;
    const VK_H: u8 = 0x48;
    const VK_ESCAPE: u8 = 0x1B;
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

    /// Whether the last press opened the Windows dictation bar in Claude.
    static DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);

    pub fn is_foreground() -> Result<bool, String> {
        let target = find_claude_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    /// True while the Claude desktop app has a top-level window. The daemon
    /// uses this to keep the Claude task feed alive across MCP proxy blips.
    pub fn desktop_running() -> bool {
        find_claude_window().is_ok()
    }

    /// True between a handled press and its release, so the release reaches
    /// `set_microphone` even if the dictation bar took the foreground.
    pub fn microphone_active() -> bool {
        DICTATION_ACTIVE.load(Ordering::SeqCst)
    }

    /// Press: focus Claude's message composer, then open Windows dictation
    /// (Win+H) so speech lands in it. Release: close the dictation bar with
    /// Escape. The caller has already checked that Claude is the foreground
    /// window; the press re-checks so the chord never lands in another app.
    pub fn set_microphone(pressed: bool) -> Result<(), String> {
        if pressed {
            let target = find_claude_window()?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the Claude window is no longer focused".to_owned());
            }
            focus_composer(target)?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the Claude window lost focus while selecting its composer".to_owned());
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
            format!("$WindowHandle = {window}\n$AgentName = 'Claude'\n{FOCUS_COMPOSER_SCRIPT}");
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
            .map_err(|error| format!("could not start Claude composer automation: {error}"))?;
        child
            .stdin
            .take()
            .ok_or_else(|| "could not open Claude composer automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send Claude composer automation: {error}"))?;
        let status = child
            .wait()
            .map_err(|error| format!("Claude composer automation did not finish: {error}"))?;
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut output) = child.stdout.take() {
            let _ = output.read_to_string(&mut stdout);
        }
        if let Some(mut error) = child.stderr.take() {
            let _ = error.read_to_string(&mut stderr);
        }
        // `powershell -Command -` runs stdin statement by statement, so a
        // trapped failure can still exit 0; stderr output marks the failure.
        if !status.success() || !stderr.trim().is_empty() {
            return Err(format!(
                "Claude composer automation failed: {}",
                stderr.trim()
            ));
        }
        if stdout.lines().any(|line| line.trim() == "focused") {
            Ok(())
        } else {
            Err(format!(
                "Claude composer automation returned an unexpected result: {}",
                stdout.trim()
            ))
        }
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

    pub fn minimize() -> Result<(), String> {
        let target = find_claude_window()?;
        unsafe {
            ShowWindow(target, SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn show_and_focus() -> Result<(), String> {
        let target = find_claude_window()?;
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
                // Windows often denies foreground activation to background
                // processes, and the MSIX executable cannot be relaunched by
                // path. The app's `claude://` protocol hands the request to the
                // running instance, which raises its own window.
                return Command::new("cmd.exe")
                    .args(["/C", "start", "", "claude://"])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| format!("Claude focus fallback failed: {error}"));
            }
        }
        Ok(())
    }

    fn find_claude_window() -> Result<Hwnd, String> {
        let mut search = WindowSearch::default();
        unsafe {
            EnumWindows(
                Some(collect_claude_window),
                (&mut search as *mut WindowSearch).cast::<c_void>() as Lparam,
            );
        }
        (search.window != 0)
            .then_some(search.window)
            .ok_or_else(|| "no Claude Desktop window was found".to_owned())
    }

    unsafe extern "system" fn collect_claude_window(window: Hwnd, lparam: Lparam) -> Bool {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == 0 || !process_is_claude_desktop(process_id) {
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

    fn process_is_claude_desktop(process_id: u32) -> bool {
        process_image_path(process_id).is_some_and(|path| is_claude_desktop_executable(&path))
    }

    pub(super) fn is_claude_desktop_executable(path: &str) -> bool {
        let path = path.replace('/', "\\").to_ascii_lowercase();
        path.ends_with("\\claude.exe")
            && !path.contains("\\claude-code\\")
            && !path.contains("\\.local\\bin\\")
            && !path.contains("\\node_modules\\")
    }

    #[cfg(test)]
    mod tests {
        use super::is_claude_desktop_executable;

        #[test]
        fn recognizes_only_the_claude_desktop_executable() {
            assert!(is_claude_desktop_executable(
                r"C:\Program Files\WindowsApps\Claude_3.6556.1.0_x64__pzs8sxrjxfjjc\app\Claude.exe"
            ));
            assert!(is_claude_desktop_executable(
                r"C:\Users\me\AppData\Local\AnthropicClaude\app-0.14.0\claude.exe"
            ));
            assert!(!is_claude_desktop_executable(
                r"C:\Users\me\AppData\Roaming\Claude\claude-code\2.1.308\c17a819afb40\claude.exe"
            ));
            assert!(!is_claude_desktop_executable(r"C:\Users\me\.local\bin\claude.exe"));
            assert!(!is_claude_desktop_executable(r"D:\apps\Hermes.exe"));
        }
    }
}

#[cfg(windows)]
pub use native::{
    desktop_running, is_foreground, microphone_active, minimize, set_microphone, show_and_focus,
};

#[cfg(not(windows))]
pub fn is_foreground() -> Result<bool, String> {
    Err("Claude window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn minimize() -> Result<(), String> {
    Err("Claude window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn show_and_focus() -> Result<(), String> {
    Err("Claude window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn microphone_active() -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_microphone(_pressed: bool) -> Result<(), String> {
    Err("Claude window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn desktop_running() -> bool {
    false
}
