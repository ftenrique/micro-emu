//! Native ZCode desktop window discovery and foreground/minimize control.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::{Condvar, Mutex, OnceLock};

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
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// How long the relaunch fallback may wait for ZCode to raise its own
    /// window through the Electron single-instance handshake.
    const RELAUNCH_FOCUS_TIMEOUT_MS: u64 = 4_000;

    const SELECT_SESSION_SCRIPT: &str = include_str!("select_zcode_session.ps1");

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

    /// Latest-wins queue feeding the single session-selection worker. UIA can
    /// take seconds per request, so the daemon loop never blocks on it and a
    /// burst of presses collapses into the most recent request.
    struct SelectionQueue {
        pending: Mutex<Option<String>>,
        signaled: Condvar,
        started: std::sync::atomic::AtomicBool,
    }

    static SELECTION_QUEUE: OnceLock<SelectionQueue> = OnceLock::new();

    pub fn is_foreground() -> Result<bool, String> {
        let target = find_zcode_window()?;
        Ok(unsafe { GetForegroundWindow() } == target)
    }

    /// True while the ZCode desktop app has a top-level window. The daemon
    /// uses this to keep the ZCode task feed alive across MCP proxy blips.
    pub fn desktop_running() -> bool {
        find_zcode_window().is_ok()
    }

    pub fn minimize() -> Result<(), String> {
        let target = find_zcode_window()?;
        unsafe {
            ShowWindow(target, SW_MINIMIZE);
        }
        Ok(())
    }

    pub fn show_and_focus() -> Result<(), String> {
        let target = find_zcode_window()?;
        unsafe {
            if IsIconic(target) != 0 {
                ShowWindow(target, SW_RESTORE);
            } else {
                ShowWindow(target, SW_SHOW);
            }

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
                            .name("zcode-window-focus".to_owned())
                            .spawn(move || relaunch_and_wait_for_focus(target, exe))
                            .map_err(|error| format!("focus fallback failed to start: {error}"))?;
                        Ok(())
                    }
                    None => Err("Windows refused to activate the ZCode window".to_owned()),
                };
            }
        }
        Ok(())
    }

    fn relaunch_and_wait_for_focus(target: Hwnd, exe: String) {
        let spawned = Command::new(&exe)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(error) = spawned {
            eprintln!("ZCode focus relaunch failed: {error}");
            return;
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_millis(RELAUNCH_FOCUS_TIMEOUT_MS);
        while std::time::Instant::now() < deadline {
            if unsafe { GetForegroundWindow() } == target {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        eprintln!("ZCode window focus failed: relaunch did not raise the window");
    }

    /// Asks the running ZCode desktop app to make the session with `title`
    /// the active one. Returns as soon as the request is queued; the UIA
    /// work happens on a worker thread.
    pub fn request_session_selection(title: &str) {
        if title.trim().is_empty() {
            return;
        }
        let queue: &'static SelectionQueue = SELECTION_QUEUE.get_or_init(|| SelectionQueue {
            pending: Mutex::new(None),
            signaled: Condvar::new(),
            started: std::sync::atomic::AtomicBool::new(false),
        });
        if !queue
            .started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let _ = std::thread::Builder::new()
                .name("zcode-session-select".to_owned())
                .spawn(move || loop {
                    let title = {
                        let mut guard = queue
                            .pending
                            .lock()
                            .expect("zcode selection queue lock poisoned");
                        while guard.is_none() {
                            guard = queue
                                .signaled
                                .wait(guard)
                                .expect("zcode selection queue lock poisoned");
                        }
                        guard.take()
                    };
                    if let Some(title) = title {
                        // A cold Chromium accessibility tree can outlive the
                        // script's own warm-up window, especially right after
                        // ZCode starts. Retry a bounded number of times so a
                        // first press eventually lands instead of dying with
                        // the tree still materializing.
                        const MAX_ATTEMPTS: u8 = 3;
                        for attempt in 1..=MAX_ATTEMPTS {
                            match select_session_sync(&title) {
                                Ok(()) => {
                                    if attempt > 1 {
                                        crate::diaglog::log(&format!(
                                            "zcode session selection for {title:?} succeeded on attempt {attempt}"
                                        ));
                                    }
                                    break;
                                }
                                Err(error) if attempt < MAX_ATTEMPTS => {
                                    eprintln!(
                                        "ZCode session selection failed (attempt {attempt}/{MAX_ATTEMPTS}): {error}"
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
                                    eprintln!("ZCode session selection failed: {error}");
                                    crate::diaglog::log(&format!(
                                        "zcode session selection for {title:?} failed after {MAX_ATTEMPTS} attempts: {error}"
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
            .expect("zcode selection queue lock poisoned");
        *pending = Some(title.to_owned());
        queue.signaled.notify_one();
    }

    fn select_session_sync(title: &str) -> Result<(), String> {
        let window = find_zcode_window()?;
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
            .map_err(|error| format!("could not start ZCode session automation: {error}"))?;

        let script = format!(
            "$WindowHandle = {window}\n$TargetTitle = '{}'\n{SELECT_SESSION_SCRIPT}",
            escape_powershell_single_quoted(title)
        );
        child
            .stdin
            .take()
            .ok_or_else(|| "could not open ZCode session automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send ZCode session automation: {error}"))?;

        let output = child
            .wait_with_output()
            .map_err(|error| format!("ZCode session automation did not finish: {error}"))?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "ZCode session automation failed: {}",
                error.trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.lines().any(|line| line.trim() == "selected") {
            Ok(())
        } else {
            Err(format!(
                "ZCode session automation returned an unexpected result: {}",
                stdout.trim()
            ))
        }
    }

    fn escape_powershell_single_quoted(value: &str) -> String {
        value.replace('\'', "''")
    }

    fn find_zcode_window() -> Result<Hwnd, String> {
        let mut search = WindowSearch::default();
        unsafe {
            EnumWindows(
                Some(collect_zcode_window),
                (&mut search as *mut WindowSearch).cast::<c_void>() as Lparam,
            );
        }
        (search.window != 0)
            .then_some(search.window)
            .ok_or_else(|| "no ZCode desktop window was found".to_owned())
    }

    unsafe extern "system" fn collect_zcode_window(window: Hwnd, lparam: Lparam) -> Bool {
        let mut process_id = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut process_id);
        }
        if process_id == 0 || !process_is_zcode_desktop(process_id) {
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
        // The window was already matched against the ZCode executable during
        // enumeration; re-check so a relaunch can never start a foreign binary.
        process_image_path(process_id).filter(|path| is_zcode_desktop_executable(path))
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

    fn process_is_zcode_desktop(process_id: u32) -> bool {
        process_image_path(process_id).is_some_and(|path| is_zcode_desktop_executable(&path))
    }

    fn is_zcode_desktop_executable(path: &str) -> bool {
        path.replace('/', "\\")
            .to_ascii_lowercase()
            .ends_with("\\zcode.exe")
    }

    #[cfg(test)]
    mod tests {
        use super::{escape_powershell_single_quoted, is_zcode_desktop_executable};

        #[test]
        fn recognizes_only_zcode_desktop_executable() {
            assert!(is_zcode_desktop_executable(r"D:\CODE\ZCode\ZCode.exe"));
            assert!(is_zcode_desktop_executable(r"D:/CODE/ZCode/ZCODE.EXE"));
            assert!(!is_zcode_desktop_executable(r"D:\CODE\ZCode\zcode.cjs"));
            assert!(!is_zcode_desktop_executable(r"D:\apps\Codex.exe"));
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
pub use native::{desktop_running, is_foreground, minimize, request_session_selection, show_and_focus};

#[cfg(not(windows))]
pub fn is_foreground() -> Result<bool, String> {
    Err("ZCode window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn minimize() -> Result<(), String> {
    Err("ZCode window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn show_and_focus() -> Result<(), String> {
    Err("ZCode window control is only available on Windows".to_owned())
}

#[cfg(not(windows))]
pub fn request_session_selection(_title: &str) {}

#[cfg(not(windows))]
pub fn desktop_running() -> bool {
    false
}
