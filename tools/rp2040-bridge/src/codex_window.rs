//! Native Codex desktop window discovery and foreground/minimize control.

#[cfg(windows)]
mod native {
    use std::ffi::c_void;
    use std::io::Write;
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
    const VK_CONTROL: u8 = 0x11;
    const VK_G: u8 = 0x47;
    const VK_OEM_3: u8 = 0xc0;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    const FEATURED_MODELS: [&str; 3] = ["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];
    const CYCLE_MODEL_SCRIPT: &str = include_str!("cycle_codex_model.ps1");
    const FOCUS_COMPOSER_SCRIPT: &str = include_str!("focus_agent_composer.ps1");

    static DICTATION_ACTIVE: AtomicBool = AtomicBool::new(false);

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

    /// Advances the active task through Sol, Terra, and Luna via semantic
    /// Windows UI Automation controls, then persists the selected default.
    /// This uses neither keyboard shortcuts nor screen coordinates.
    pub fn cycle_model() -> Result<&'static str, String> {
        let window = find_codex_window()?;
        let new_model = select_next_model_with_uia(window)?;
        // The active task has already changed at this point. Keeping the
        // config synchronized is useful for future tasks, but a read-only or
        // temporarily locked config must not turn a successful UI selection
        // into a reported failure.
        let _ = persist_default_model(new_model);
        Ok(new_model)
    }

    fn select_next_model_with_uia(window: Hwnd) -> Result<&'static str, String> {
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
            .map_err(|error| format!("could not start Codex model UI automation: {error}"))?;

        let script = format!("$WindowHandle = {window}\n{CYCLE_MODEL_SCRIPT}");
        child
            .stdin
            .take()
            .ok_or_else(|| "could not open Codex model automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send Codex model automation: {error}"))?;

        let output = child
            .wait_with_output()
            .map_err(|error| format!("Codex model UI automation did not finish: {error}"))?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Codex model UI automation failed: {}",
                error.trim()
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        FEATURED_MODELS
            .iter()
            .copied()
            .find(|model| stdout.lines().any(|line| line.trim() == *model))
            .ok_or_else(|| {
                format!(
                    "Codex model UI automation returned an unexpected result: {}",
                    stdout.trim()
                )
            })
    }

    fn persist_default_model(new_model: &str) -> Result<(), String> {
        let path = codex_config_path()?;
        let content = std::fs::read_to_string(&path).unwrap_or_default();

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let updated = replace_top_level_model(&content, new_model);
        std::fs::write(&path, updated)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        Ok(())
    }

    fn codex_config_path() -> Result<std::path::PathBuf, String> {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .map_err(|error| format!("could not resolve home directory: {error}"))?;
        Ok(std::path::Path::new(&home)
            .join(".codex")
            .join("config.toml"))
    }

    /// Returns the value of the top-level `model` key, ignoring any keys that
    /// appear under a `[table]` header. Mirrors the bridge's display reader,
    /// which only honors keys before the first section.
    fn read_current_model(content: &str) -> Option<String> {
        let mut in_section = false;
        let mut current = None;
        for line in content.lines() {
            if line.trim_start().starts_with('[') {
                in_section = true;
            }
            if !in_section && is_top_level_key(line, "model") {
                if let Some((_, value)) = line.split_once('=') {
                    current = Some(unquote_toml(value.trim()).to_owned());
                }
            }
        }
        current
    }

    /// Replaces the first top-level `model = ...` line with
    /// `model = "<new_model>"`, leaving every other line — including any
    /// `model` keys nested under `[sections]` — untouched. When there is no
    /// top-level model key, one is prepended.
    fn replace_top_level_model(content: &str, new_model: &str) -> String {
        let mut in_section = false;
        let mut replaced = false;
        let mut lines: Vec<String> = Vec::new();
        for line in content.lines() {
            if line.trim_start().starts_with('[') {
                in_section = true;
            }
            if !in_section && !replaced && is_top_level_key(line, "model") {
                lines.push(format!("model = \"{new_model}\""));
                replaced = true;
            } else {
                lines.push(line.to_string());
            }
        }
        let mut out = lines.join("\n");
        if !replaced {
            out = format!("model = \"{new_model}\"\n{out}");
        }
        if content.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// True when `line` is a `<key> = ...` assignment whose key is exactly
    /// `model`, so siblings such as `model_reasoning_effort` and
    /// `model_provider` are excluded.
    fn is_top_level_key(line: &str, key: &str) -> bool {
        match line.trim_start().strip_prefix(key) {
            Some(rest) => rest.trim_start().starts_with('='),
            None => false,
        }
    }

    fn unquote_toml(value: &str) -> &str {
        let bytes = value.as_bytes();
        if bytes.len() >= 2
            && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
                || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
        {
            &value[1..value.len() - 1]
        } else {
            value
        }
    }

    /// Focuses Codex and opens task search (Ctrl+G).
    pub fn search_tasks() -> Result<(), String> {
        send_control_shortcut(VK_G)
    }

    /// Focuses Codex and toggles its bottom terminal (Ctrl+backtick).
    pub fn toggle_terminal() -> Result<(), String> {
        send_control_shortcut(VK_OEM_3)
    }

    /// Uses Windows dictation as hold-to-talk input for Codex's composer.
    pub fn set_microphone(pressed: bool) -> Result<(), String> {
        if pressed {
            let target = find_codex_window()?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the Codex window is no longer focused".to_owned());
            }
            focus_composer(target)?;
            if unsafe { GetForegroundWindow() } != target {
                DICTATION_ACTIVE.store(false, Ordering::SeqCst);
                return Err("the Codex window lost focus while selecting its composer".to_owned());
            }
            unsafe {
                key_down(VK_LWIN);
                tap_key(VK_H);
                key_up(VK_LWIN);
            }
            DICTATION_ACTIVE.store(true, Ordering::SeqCst);
        } else if DICTATION_ACTIVE.swap(false, Ordering::SeqCst) {
            unsafe { tap_key(VK_ESCAPE); }
        }
        Ok(())
    }

    fn focus_composer(window: Hwnd) -> Result<(), String> {
        let script = format!("$WindowHandle = {window}\n$AgentName = 'Codex'\n{FOCUS_COMPOSER_SCRIPT}");
        run_automation(&script, "focused")
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

    fn run_automation(script: &str, success_marker: &str) -> Result<(), String> {
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("could not start Codex composer automation: {error}"))?;
        child
            .stdin
            .take()
            .ok_or_else(|| "could not open Codex composer automation input".to_owned())?
            .write_all(script.as_bytes())
            .map_err(|error| format!("could not send Codex composer automation: {error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("Codex composer automation did not finish: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "Codex composer automation failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        if String::from_utf8_lossy(&output.stdout).lines().any(|line| line.trim() == success_marker) {
            Ok(())
        } else {
            Err(format!(
                "Codex composer automation returned an unexpected result: {}",
                String::from_utf8_lossy(&output.stdout).trim()
            ))
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
        use super::{is_codex_desktop_executable, read_current_model, replace_top_level_model};

        #[test]
        fn recognizes_packaged_codex_ui_and_helper() {
            assert!(is_codex_desktop_executable(
                r"C:\Program Files\WindowsApps\OpenAI.Codex_1_x64\app\ChatGPT.exe"
            ));
            assert!(is_codex_desktop_executable(r"C:\apps\codex.exe"));
            assert!(!is_codex_desktop_executable(r"C:\apps\ChatGPT.exe"));
        }

        #[test]
        fn replace_top_level_model_updates_only_the_first_top_level_key() {
            let original = "# Codex config\n\
                model = \"gpt-5.6-sol\"\n\
                model_reasoning_effort = \"medium\"\n\n\
                [history]\n\
                persistence = \"save-all\"\n\
                model = \"leave-me-alone\"\n";
            let updated = replace_top_level_model(original, "gpt-5.6-terra");

            assert!(updated.contains("model = \"gpt-5.6-terra\""));
            // Sibling model_* keys are untouched.
            assert!(updated.contains("model_reasoning_effort = \"medium\""));
            // The in-section model line is preserved, not replaced.
            assert!(updated.contains("model = \"leave-me-alone\""));
            // Exactly one replacement happened.
            assert_eq!(updated.matches("gpt-5.6-terra").count(), 1);
            assert!(updated.ends_with('\n'));
            // Round-trip: the reader sees exactly what we wrote.
            assert_eq!(
                read_current_model(&updated),
                Some("gpt-5.6-terra".to_owned())
            );
        }

        #[test]
        fn replace_top_level_model_prepends_when_absent() {
            let original = "model_reasoning_effort = \"medium\"\n";
            let updated = replace_top_level_model(original, "gpt-5.6-luna");

            assert!(updated.starts_with("model = \"gpt-5.6-luna\"\n"));
            assert!(updated.contains("model_reasoning_effort = \"medium\""));
            assert_eq!(
                read_current_model(&updated),
                Some("gpt-5.6-luna".to_owned())
            );
        }

        #[test]
        fn replace_top_level_model_creates_a_config_when_empty() {
            assert_eq!(
                replace_top_level_model("", "gpt-5.6-sol"),
                "model = \"gpt-5.6-sol\"\n"
            );
        }

        #[test]
        fn read_current_model_skips_keys_inside_sections() {
            // The first model is top-level (before any header); the second is
            // under [profiles.fast] and must be ignored.
            let content = "model = \"real\"\n[profiles.fast]\nmodel = \"ignored\"\n";
            assert_eq!(read_current_model(content), Some("real".to_owned()));
        }
    }
}

#[cfg(windows)]
pub use native::{
    cycle_model, is_foreground, minimize, search_tasks, set_microphone, show_and_focus,
    toggle_terminal,
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
pub fn cycle_model() -> Result<&'static str, String> {
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

#[cfg(not(windows))]
pub fn set_microphone(_pressed: bool) -> Result<(), String> {
    Err("Codex microphone control is only available on Windows".to_owned())
}
