//! Native Hermes Desktop window discovery and foreground/minimize control.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex, OnceLock};

    type Bool = i32;
    type Handle = isize;
    type Hwnd = isize;
    type Lparam = isize;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const GWL_EXSTYLE: i32 = -20;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_CONTROL: u8 = 0x11;
    const VK_N: u8 = 0x4E;
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
    /// How long the relaunch fallback may wait for Hermes to raise its own
    /// window through the Electron single-instance handshake.
    const RELAUNCH_FOCUS_TIMEOUT_MS: u64 = 4_000;

    const SELECT_SESSION_SCRIPT: &str = include_str!("hermes_select_session.ps1");

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

    /// Whether the last press opened the Windows dictation bar in Hermes.
    static DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);

    /// Latest-wins queue of pending session-selection titles, feeding the
    /// single sidebar-automation worker. UIA can take seconds per request,
    /// so the daemon loop never blocks on it and a burst of presses collapses
    /// into the most recent request.
    struct AutomationQueue {
        pending: Mutex<Option<String>>,
        signaled: Condvar,
        started: AtomicBool,
    }

    static AUTOMATION_QUEUE: OnceLock<AutomationQueue> = OnceLock::new();

    pub fn is_foreground() -> Result<bool, String> {
        let target = find_hermes_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    /// True while the Hermes desktop app has a top-level window. The daemon
    /// uses this to keep the Hermes task feed alive across MCP proxy blips.
    pub fn desktop_running() -> bool {
        find_hermes_window().is_ok()
    }

    /// Starts a new Hermes session. The sidebar's New session button carries
    /// the Ctrl+N accelerator, so a keyboard chord reaches it without the
    /// accessibility-tree warm-up a UIA click would need. The caller has
    /// already verified that Hermes is the foreground window; the chord is
    /// re-checked here so it can never land in another app.
    pub fn start_new_session() -> Result<(), String> {
        let target = find_hermes_window()?;
        if unsafe { GetForegroundWindow() } != target {
            return Err("the Hermes window is no longer focused".to_owned());
        }
        unsafe {
            key_down(VK_CONTROL);
            tap_key(VK_N);
            key_up(VK_CONTROL);
        }
        Ok(())
    }

    /// True between a handled press and its release, so the release reaches
    /// `set_microphone` even if the dictation bar itself took the foreground
    /// or the user switched windows while holding the key.
    pub fn microphone_active() -> bool {
        DICTATION_ACTIVE.load(Ordering::SeqCst)
    }

    /// Hermes has no voice input of its own. A press opens Windows dictation
    /// (Win+H), which transcribes into Hermes' focused composer; the release
    /// closes the dictation bar with Escape, mirroring the Codex action's
    /// hold-to-talk semantics. The caller has already verified that Hermes
    /// is the foreground window; the press re-checks because the chord must
    /// never land in another app.
    pub fn set_microphone(pressed: bool) -> Result<(), String> {
        if pressed {
            let target = find_hermes_window()?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the Hermes window is no longer focused".to_owned());
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

    fn relaunch_and_wait_for_focus(target: Hwnd, exe: String) {
        let spawned = Command::new(&exe)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(error) = spawned {
            eprintln!("Hermes focus relaunch failed: {error}");
            return;
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(RELAUNCH_FOCUS_TIMEOUT_MS);
        while std::time::Instant::now() < deadline {
            if unsafe { GetForegroundWindow() } == target {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        eprintln!("Hermes window focus failed: relaunch did not raise the window");
    }

    /// Asks the running Hermes desktop app to make the session with `title`
    /// the active one. Returns as soon as the request is queued; the UIA
    /// work happens on a worker thread.
    pub fn request_session_selection(title: &str) {
        if title.trim().is_empty() {
            return;
        }
        queue_request(title.trim().to_owned());
    }

    fn queue_request(title: String) {
        let queue: &'static AutomationQueue = AUTOMATION_QUEUE.get_or_init(|| AutomationQueue {
            pending: Mutex::new(None),
            signaled: Condvar::new(),
            started: AtomicBool::new(false),
        });
        if !queue.started.swap(true, Ordering::SeqCst) {
            let _ = std::thread::Builder::new()
                .name("hermes-sidebar-automation".to_owned())
                .spawn(move || loop {
                    let title = {
                        let mut guard = queue
                            .pending
                            .lock()
                            .expect("hermes automation queue lock poisoned");
                        while guard.is_none() {
                            guard = queue
                                .signaled
                                .wait(guard)
                                .expect("hermes automation queue lock poisoned");
                        }
                        guard.take()
                    };
                    if let Some(title) = title {
                        // A cold Chromium accessibility tree can outlive the
                        // script's own warm-up window, especially right after
                        // Hermes starts. Retry a bounded number of times so a
                        // first press eventually lands instead of dying with
                        // the tree still materializing.
                        const MAX_ATTEMPTS: u8 = 3;
                        for attempt in 1..=MAX_ATTEMPTS {
                            match run_request(&title) {
                                Ok(()) => {
                                    if attempt > 1 {
                                        crate::diaglog::log(&format!(
                                            "hermes session selection for {title:?} succeeded on attempt {attempt}"
                                        ));
                                    }
                                    break;
                                }
                                Err(error) if attempt < MAX_ATTEMPTS => {
                                    eprintln!(
                                        "Hermes session selection for {title:?} failed (attempt {attempt}/{MAX_ATTEMPTS}): {error}",
                                    );
                                    std::thread::sleep(std::time::Duration::from_millis(
                                        u64::from(attempt) * 4_000,
                                    ));
                                    // A newer press supersedes this retry loop.
                                    let superseded = queue
                                        .pending
                                        .lock()
                                        .map(|pending| pending.is_some())
                                        .unwrap_or(true);
                                    if superseded {
                                        break;
                                    }
                                }
                                Err(error) => {
                                    eprintln!(
                                        "Hermes session selection for {title:?} failed: {error}"
                                    );
                                    crate::diaglog::log(&format!(
                                        "hermes session selection for {title:?} failed after {MAX_ATTEMPTS} attempts: {error}"
                                    ));
                                }
                            }
                        }
                    }
                });
        }
        let mut pending = queue
            .pending
            .lock()
            .expect("hermes automation queue lock poisoned");
        *pending = Some(title);
        queue.signaled.notify_one();
    }

    fn run_request(title: &str) -> Result<(), String> {
        let window = find_hermes_window()?;
        let script = format!(
            "$WindowHandle = {window}\n$TargetTitle = '{}'\n{SELECT_SESSION_SCRIPT}",
            escape_powershell_single_quoted(title)
        );
        run_automation(&script, "selected")
    }

    fn run_automation(script: &str, success_marker: &str) -> Result<(), String> {
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
            .map_err(|error| format!("could not start Hermes sidebar automation: {error}"))?;

        child
            .stdin
            .take()
            .ok_or_else(|| "could not open Hermes sidebar automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send Hermes sidebar automation: {error}"))?;

        let output = child
            .wait_with_output()
            .map_err(|error| format!("Hermes sidebar automation did not finish: {error}"))?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Hermes sidebar automation failed: {}", error.trim()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.lines().any(|line| line.trim() == success_marker) {
            Ok(())
        } else {
            Err(format!(
                "Hermes sidebar automation returned an unexpected result: {}",
                stdout.trim()
            ))
        }
    }

    fn escape_powershell_single_quoted(value: &str) -> String {
        value.replace('\'', "''")
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
                // Windows routinely denies foreground activation to background
                // processes. Relaunching the executable hands the request to
                // the running instance through Electron's single-instance
                // handshake, which raises the window from inside the app.
                return match window_process_image(target) {
                    Some(exe) => {
                        std::thread::Builder::new()
                            .name("hermes-window-focus".to_owned())
                            .spawn(move || relaunch_and_wait_for_focus(target, exe))
                            .map_err(|error| format!("focus fallback failed to start: {error}"))?;
                        Ok(())
                    }
                    None => Err("Windows refused to activate the Hermes window".to_owned()),
                };
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

    fn window_process_image(window: Hwnd) -> Option<String> {
        let mut process_id = 0;
        if unsafe { GetWindowThreadProcessId(window, &mut process_id) } == 0 || process_id == 0 {
            return None;
        }
        // The window was already matched against the Hermes executable during
        // enumeration; re-check so a relaunch can never start a foreign binary.
        process_image_path(process_id).filter(|path| is_hermes_desktop_executable(path))
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

    fn process_is_hermes_desktop(process_id: u32) -> bool {
        process_image_path(process_id).is_some_and(|path| is_hermes_desktop_executable(&path))
    }

    fn is_hermes_desktop_executable(path: &str) -> bool {
        path.replace('/', "\\")
            .to_ascii_lowercase()
            .ends_with("\\hermes.exe")
    }

    #[cfg(test)]
    mod tests {
        use super::{escape_powershell_single_quoted, is_hermes_desktop_executable};

        #[test]
        fn recognizes_only_hermes_desktop_executable() {
            assert!(is_hermes_desktop_executable(
                r"C:\Users\me\AppData\Local\hermes\release\win-unpacked\Hermes.exe"
            ));
            assert!(is_hermes_desktop_executable(r"D:/apps/HERMES.EXE"));
            assert!(!is_hermes_desktop_executable(r"D:\apps\hermes-agent.exe"));
            assert!(!is_hermes_desktop_executable(r"D:\apps\Codex.exe"));
        }

        #[test]
        fn escapes_single_quotes_for_powershell_literals() {
            assert_eq!(escape_powershell_single_quoted("plain title"), "plain title");
            assert_eq!(
                escape_powershell_single_quoted("it's a task's title"),
                "it''s a task''s title"
            );
        }
    }
}

#[cfg(windows)]
pub use native::{
    desktop_running, is_foreground, microphone_active, minimize, request_session_selection,
    set_microphone, show_and_focus, start_new_session,
};

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
#[cfg(not(windows))]
pub fn start_new_session() -> Result<(), String> {
    Err("Hermes window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn request_session_selection(_title: &str) {}
#[cfg(not(windows))]
pub fn microphone_active() -> bool {
    false
}
#[cfg(not(windows))]
pub fn set_microphone(_pressed: bool) -> Result<(), String> {
    Err("Hermes window control is only available on Windows".to_owned())
}
#[cfg(not(windows))]
pub fn desktop_running() -> bool {
    false
}
