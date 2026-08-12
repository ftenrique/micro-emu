use crate::codex::PhysicalEvent;
use crate::controller::{ControllerKind, DisplayContext, PhysicalController};
use hidapi::{HidApi, HidDevice};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, RgbImage, imageops};
use serde_json::Value;
use std::ffi::CString;
use std::io::Cursor;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

const VENDOR_ID: u16 = 0x0fd9;
const PLUS_PID: u16 = 0x0084;
const XL_PID: u16 = 0x006c;
const PLUS_XL_PID: u16 = 0x00c6;
const OUTPUT_REPORT_BYTES: usize = 1024;
const FEATURE_REPORT_BYTES: usize = 32;
const INPUT_REPORT_BYTES: usize = 512;
const STATUS_SLOTS: usize = 8;
const HID_WRITE_RETRIES: usize = 4;
// HID writes are synchronous. Small pacing windows keep older firmware happy
// without turning initial state replay into a long serialized startup.
const HID_REPORT_SETTLE: Duration = Duration::from_millis(5);
const HID_IMAGE_SETTLE: Duration = Duration::from_millis(20);
const HID_DEVICE_SETTLE: Duration = Duration::from_millis(100);
const HID_WINDOW_MIN_INTERVAL: Duration = Duration::from_millis(100);
const DIGITS: [[u8; 15]; STATUS_SLOTS] = [
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0],
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0],
    [1, 1, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1],
    [0, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1, 0, 1],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rotation {
    None,
    Rotate90Ccw,
    Rotate180,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayMode {
    Context,
    Usage,
}

#[derive(Clone, Copy)]
struct Profile {
    kind: ControllerKind,
    model: &'static str,
    pid: u16,
    key_count: usize,
    encoder_count: usize,
    key_size: u32,
    screen: (u32, u32),
    window: Option<(u32, u32)>,
    rotation: Rotation,
}

const PLUS: Profile = Profile {
    kind: ControllerKind::StreamDeckPlus,
    model: "20GBD9901",
    pid: PLUS_PID,
    key_count: 8,
    encoder_count: 4,
    key_size: 120,
    screen: (800, 480),
    window: Some((800, 100)),
    rotation: Rotation::None,
};

const XL: Profile = Profile {
    kind: ControllerKind::StreamDeckXl,
    model: "20GAT9901",
    pid: XL_PID,
    key_count: 32,
    encoder_count: 0,
    key_size: 96,
    screen: (1024, 600),
    window: None,
    rotation: Rotation::Rotate180,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    RotorLeft,
    RotorCenter,
    RotorRight,
    Mic,
    Send,
}

// Original XL uses a 4x8 row-major key grid. The fixed layout leaves all other keys black.
//            00 01 02 03 04 05 06 07  (AG00-AG05, ACT06-ACT07)
//            08 09 10 11 12 13 14 15  (up at 11, Mic at 14)
//            16 17 18 19 20 21 22 23  (left/send/right)
//            24 25 26 27 28 29 30 31  (down, rotor at 29-31)
fn icon_for_key(kind: ControllerKind, index: usize) -> Option<Icon> {
    if kind != ControllerKind::StreamDeckXl {
        return None;
    }
    match index {
        11 => Some(Icon::ArrowUp),
        14 => Some(Icon::Mic),
        18 => Some(Icon::ArrowLeft),
        19 => Some(Icon::Send),
        20 => Some(Icon::ArrowRight),
        27 => Some(Icon::ArrowDown),
        29 => Some(Icon::RotorLeft),
        30 => Some(Icon::RotorCenter),
        31 => Some(Icon::RotorRight),
        _ => None,
    }
}

// Virtual controls deliberately reuse EncoderTurn/EncoderButton so Codex mappings stay unchanged.
fn virtual_key_event(kind: ControllerKind, index: usize, pressed: bool) -> Option<PhysicalEvent> {
    if kind != ControllerKind::StreamDeckXl || index < 9 {
        return None;
    }
    match index {
        11 if pressed => Some(PhysicalEvent::EncoderTurn { index: 2, delta: 1 }),
        27 if pressed => Some(PhysicalEvent::EncoderTurn {
            index: 2,
            delta: -1,
        }),
        18 if pressed => Some(PhysicalEvent::EncoderTurn {
            index: 0,
            delta: -1,
        }),
        20 if pressed => Some(PhysicalEvent::EncoderTurn { index: 0, delta: 1 }),
        29 if pressed => Some(PhysicalEvent::EncoderTurn {
            index: 1,
            delta: -1,
        }),
        31 if pressed => Some(PhysicalEvent::EncoderTurn { index: 1, delta: 1 }),
        19 => Some(PhysicalEvent::EncoderButton { index: 0, pressed }),
        14 => Some(PhysicalEvent::EncoderButton { index: 2, pressed }),
        30 => Some(PhysicalEvent::EncoderButton { index: 1, pressed }),
        _ => None,
    }
}
const PLUS_XL: Profile = Profile {
    kind: ControllerKind::StreamDeckPlusXl,
    model: "20GBD9901",
    pid: PLUS_XL_PID,
    key_count: 36,
    encoder_count: 6,
    key_size: 112,
    screen: (1280, 800),
    window: Some((1200, 100)),
    rotation: Rotation::Rotate90Ccw,
};

pub fn connect(
    kind: ControllerKind,
    requested_serial: Option<&str>,
) -> Result<Box<dyn PhysicalController>, String> {
    let profile = match kind {
        ControllerKind::StreamDeckPlus => PLUS,
        ControllerKind::StreamDeckPlusXl => PLUS_XL,
        ControllerKind::StreamDeckXl => XL,
        _ => {
            return Err(format!(
                "unsupported Stream Deck controller kind {}",
                kind.as_str()
            ));
        }
    };
    let api = HidApi::new().map_err(|error| error.to_string())?;
    let candidates = api
        .device_list()
        .filter(|info| {
            info.vendor_id() == VENDOR_ID
                && info.product_id() == profile.pid
                && requested_serial
                    .map(|serial| info.serial_number() == Some(serial))
                    .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        let visible = api
            .device_list()
            .filter(|info| info.vendor_id() == VENDOR_ID)
            .map(|info| {
                format!(
                    "{:04X}:{:04X} serial={:?}",
                    info.vendor_id(),
                    info.product_id(),
                    info.serial_number()
                )
            })
            .collect::<Vec<_>>();
        eprintln!(
            "Stream Deck HID attach failed model={} vid={:04X} pid={:04X} requested_serial={:?} visible_vendor_devices={:?}",
            profile.model, VENDOR_ID, profile.pid, requested_serial, visible
        );
        return Err(format!(
            "{} ({:04X}:{:04X}) was not found; close the Stream Deck app if it owns the device",
            profile.model, VENDOR_ID, profile.pid
        ));
    }
    if candidates.len() != 1 {
        return Err(format!(
            "multiple {} devices were found; pass --controller-serial",
            profile.model
        ));
    }
    let info = candidates[0];
    let path = info.path().to_owned();
    let serial = info.serial_number().map(str::to_owned);
    let device = api.open_path(info.path()).map_err(|error| {
        format!(
            "could not open Stream Deck {}; close the official app first: {error}",
            profile.model
        )
    })?;
    validate_unit_info(&device, profile)?;
    // Give Windows and the Stream Deck firmware a short settle window before
    // the initial feature/image burst. This avoids canceling the first image
    // transfer while the HID interface is still finishing enumeration.
    thread::sleep(HID_DEVICE_SETTLE);
    eprintln!(
        "Stream Deck HID attached model={} vid={:04X} pid={:04X} serial={:?}",
        profile.model, VENDOR_ID, profile.pid, serial
    );
    let mut result = StreamDeckDevice {
        path,
        device: Some(device),
        profile,
        serial,
        pressed_keys: vec![false; profile.key_count],
        pressed_encoders: vec![false; profile.encoder_count],
        last_colors: [None; STATUS_SLOTS],
        last_agents: std::array::from_fn(|_| None),
        last_display_context: None,
        display_context: None,
        display_mode: DisplayMode::Context,
        last_display_mode: None,
        pending_display_context: None,
        pending_display_mode: None,
        last_display_write_at: None,
        write_lock: Mutex::new(()),
    };
    result.initialize()?;
    Ok(Box::new(result))
}

fn validate_unit_info(device: &HidDevice, profile: Profile) -> Result<(), String> {
    let mut report = [0_u8; FEATURE_REPORT_BYTES];
    report[0] = 0x08;
    let read = device
        .get_feature_report(&mut report)
        .map_err(|error| error.to_string())?;
    if read >= 11 {
        let key_width = u16::from_le_bytes([report[3], report[4]]) as u32;
        let key_height = u16::from_le_bytes([report[5], report[6]]) as u32;
        let lcd_width = u16::from_le_bytes([report[7], report[8]]) as u32;
        let lcd_height = u16::from_le_bytes([report[9], report[10]]) as u32;
        if (key_width, key_height) != (profile.key_size, profile.key_size)
            || (lcd_width, lcd_height) != profile.screen
        {
            return Err(format!(
                "Stream Deck {} reported unexpected geometry {}x{} keys and {}x{} LCD",
                profile.model, key_width, key_height, lcd_width, lcd_height
            ));
        }
    }
    Ok(())
}

struct StreamDeckDevice {
    device: Option<HidDevice>,
    path: CString,
    write_lock: Mutex<()>,
    profile: Profile,
    serial: Option<String>,
    pressed_keys: Vec<bool>,
    pressed_encoders: Vec<bool>,
    last_colors: [Option<(u32, u8)>; STATUS_SLOTS],
    last_agents: [Option<String>; STATUS_SLOTS],
    last_display_context: Option<DisplayContext>,
    display_context: Option<DisplayContext>,
    display_mode: DisplayMode,
    last_display_mode: Option<DisplayMode>,
    pending_display_context: Option<DisplayContext>,
    pending_display_mode: Option<DisplayMode>,
    last_display_write_at: Option<Instant>,
}

impl StreamDeckDevice {
    fn reopen_device(&mut self) -> Result<(), String> {
        let api = HidApi::new().map_err(|error| error.to_string())?;
        let device = api
            .open_path(self.path.as_c_str())
            .map_err(|error| format!("could not reopen Stream Deck HID handle: {error}"))?;
        validate_unit_info(&device, self.profile)?;
        thread::sleep(HID_DEVICE_SETTLE);
        self.device = Some(device);
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), String> {
        self.set_brightness(100)?;
        // A full-LCD fill clears the display before normal state replay paints active keys.
        self.fill_screen([0, 0, 0])?;
        for key in 0..self.profile.key_count {
            if let Some(icon) = icon_for_key(self.profile.kind, key) {
                let image = encode_icon_image(self.profile.key_size, icon, self.profile.rotation)?;
                self.write_key_image(key, &image)?;
            }
        }
        Ok(())
    }

    fn set_brightness(&self, brightness: u8) -> Result<(), String> {
        self.send_feature(&[0x03, 0x08, brightness.min(100)])
    }

    fn fill_screen(&self, rgb: [u8; 3]) -> Result<(), String> {
        self.send_feature(&[0x03, 0x05, rgb[0], rgb[1], rgb[2]])
    }

    fn send_feature(&self, payload: &[u8]) -> Result<(), String> {
        if payload.len() > FEATURE_REPORT_BYTES {
            return Err(format!(
                "Stream Deck feature payload is too large: {} bytes",
                payload.len()
            ));
        }
        let mut report = [0_u8; FEATURE_REPORT_BYTES];
        report[..payload.len()].copy_from_slice(payload);
        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| "Stream Deck HID write lock was poisoned".to_owned())?;
        let device = self
            .device
            .as_ref()
            .expect("Stream Deck HID handle is present");
        let mut last_error = None;
        for attempt in 0..=HID_WRITE_RETRIES {
            match device.send_feature_report(&report) {
                Ok(_) => {
                    thread::sleep(HID_REPORT_SETTLE);
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    if attempt < HID_WRITE_RETRIES {
                        thread::sleep(Duration::from_millis(10 * (attempt as u64 + 1)));
                    }
                }
            }
        }
        Err(format!(
            "Stream Deck feature write failed after retries: {}",
            last_error.unwrap_or_else(|| "unknown error".to_owned())
        ))
    }

    fn send_image(
        &mut self,
        command: u8,
        target: u8,
        image: &[u8],
        extra: u8,
        header_len: usize,
    ) -> Result<(), String> {
        let capacity = OUTPUT_REPORT_BYTES - header_len;
        let total_chunks = (image.len() + capacity - 1) / capacity;
        for (index, chunk) in image.chunks(capacity).enumerate() {
            let mut report = vec![0_u8; OUTPUT_REPORT_BYTES];
            report[0] = 0x02;
            report[1] = command;
            if command == 0x07 {
                report[2] = target;
                report[3] = u8::from(index + 1 == total_chunks);
                report[4..6].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                report[6..8].copy_from_slice(&(index as u16).to_le_bytes());
                report[8..8 + chunk.len()].copy_from_slice(chunk);
            } else if command == 0x0b {
                report[2] = 0;
                report[3] = u8::from(index + 1 == total_chunks);
                report[4..6].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                report[6..8].copy_from_slice(&(index as u16).to_le_bytes());
                report[8..8 + chunk.len()].copy_from_slice(chunk);
            } else {
                report[2] = target;
                report[3] = extra;
                report[4..6].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                report[6..8].copy_from_slice(&(index as u16).to_le_bytes());
                report[8..8 + chunk.len()].copy_from_slice(chunk);
            }

            let mut last_error = None;
            let mut written_ok = false;
            for attempt in 0..=HID_WRITE_RETRIES {
                let write_result = {
                    let _guard = self
                        .write_lock
                        .lock()
                        .map_err(|_| "Stream Deck HID write lock was poisoned".to_owned())?;
                    let device = self
                        .device
                        .as_ref()
                        .expect("Stream Deck HID handle is present");
                    device.write(&report)
                };
                match write_result {
                    Ok(written) if written == report.len() => {
                        written_ok = true;
                        break;
                    }
                    Ok(written) => {
                        last_error = Some(format!("short write: {written}/{}", report.len()));
                    }
                    Err(error) => {
                        last_error = Some(error.to_string());
                    }
                }
                if attempt < HID_WRITE_RETRIES {
                    thread::sleep(Duration::from_millis(75 * (attempt as u64 + 1)));
                    if let Err(error) = self.reopen_device() {
                        last_error = Some(format!("{last_error:?}; HID reopen failed: {error}"));
                    }
                }
            }
            if !written_ok {
                let error = last_error.unwrap_or_else(|| "unknown error".to_owned());
                eprintln!(
                    "Stream Deck HID image write failed command=0x{command:02X} target={target} chunk={}/{} bytes={} error={error}",
                    index + 1,
                    total_chunks,
                    image.len()
                );
                return Err(format!(
                    "Stream Deck image report failed after retries: {error}"
                ));
            }
            thread::sleep(HID_REPORT_SETTLE);
        }
        thread::sleep(HID_IMAGE_SETTLE);
        Ok(())
    }

    fn write_key_image(&mut self, key: usize, image: &[u8]) -> Result<(), String> {
        self.send_image(0x07, key as u8, image, 0, 8)
    }

    fn events_from_report(&mut self, report: &[u8]) -> Vec<PhysicalEvent> {
        if report.len() < 4 || report[0] != 0x01 {
            return Vec::new();
        }
        let length = u16::from_le_bytes([report[2], report[3]]) as usize;
        let end = (4 + length).min(report.len());
        let payload = &report[4..end];
        match report[1] {
            0x00 => self.key_events(payload),
            0x02 => {
                self.touch_events(payload);
                Vec::new()
            }
            0x03 if self.profile.encoder_count > 0 => self.encoder_events(payload),
            _ => Vec::new(),
        }
    }

    fn touch_events(&mut self, payload: &[u8]) {
        if self.profile.window.is_none() || payload.len() < 10 || payload[0] != 0x03 {
            return;
        }
        let start_x = u16::from_le_bytes([payload[2], payload[3]]);
        let start_y = u16::from_le_bytes([payload[4], payload[5]]);
        let end_x = u16::from_le_bytes([payload[6], payload[7]]);
        let end_y = u16::from_le_bytes([payload[8], payload[9]]);
        let horizontal = end_x.abs_diff(start_x);
        let vertical = end_y.abs_diff(start_y);
        // The device has already classified 0x03 as a completed flick. Keep
        // only a generous direction check instead of rejecting short swipes.
        if horizontal == 0 || horizontal.saturating_mul(2) < vertical {
            return;
        }
        // Directional selection is idempotent, so repeating a swipe cannot
        // switch to the target screen and immediately switch back.
        let next_mode = if end_x < start_x {
            DisplayMode::Usage
        } else {
            DisplayMode::Context
        };
        if next_mode == self.display_mode {
            return;
        }
        self.display_mode = next_mode;
        let context = self.display_context.clone().unwrap_or_default();
        self.pending_display_context = Some(context);
        self.pending_display_mode = Some(self.display_mode);
        if self.device.is_some()
            && let Err(error) = self.flush_display_context(true)
        {
            eprintln!("Stream Deck display mode switch failed: {error}");
        }
    }
    fn key_events(&mut self, payload: &[u8]) -> Vec<PhysicalEvent> {
        let mut events = Vec::new();
        for (index, &state) in payload.iter().take(self.profile.key_count).enumerate() {
            let pressed = state != 0;
            if pressed != self.pressed_keys[index] {
                self.pressed_keys[index] = pressed;
                if let Some(event) = virtual_key_event(self.profile.kind, index, pressed) {
                    events.push(event);
                } else if self.profile.kind != ControllerKind::StreamDeckXl || index < 9 {
                    events.push(PhysicalEvent::Button {
                        index: index as u8,
                        pressed,
                    });
                }
            }
        }
        events
    }

    fn encoder_events(&mut self, payload: &[u8]) -> Vec<PhysicalEvent> {
        let Some((&event_type, values)) = payload.split_first() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        match event_type {
            0x00 => {
                for (index, &state) in values.iter().take(self.profile.encoder_count).enumerate() {
                    let pressed = state != 0;
                    if pressed != self.pressed_encoders[index] {
                        self.pressed_encoders[index] = pressed;
                        events.push(PhysicalEvent::EncoderButton {
                            index: index as u8,
                            pressed,
                        });
                    }
                }
            }
            0x01 => {
                for (index, &raw) in values.iter().take(self.profile.encoder_count).enumerate() {
                    let delta = raw as i8;
                    let sign = delta.signum();
                    for _ in 0..delta.unsigned_abs() {
                        events.push(PhysicalEvent::EncoderTurn {
                            index: index as u8,
                            delta: sign,
                        });
                    }
                }
            }
            _ => {}
        }
        events
    }
    fn flush_display_context(&mut self, force: bool) -> Result<(), String> {
        let Some(context) = self.pending_display_context.clone() else {
            return Ok(());
        };
        let mode = self.pending_display_mode.unwrap_or(self.display_mode);
        if self.last_display_context.as_ref() == Some(&context)
            && self.last_display_mode == Some(mode)
        {
            self.pending_display_context = None;
            self.pending_display_mode = None;
            return Ok(());
        }
        if !force
            && self
                .last_display_write_at
                .is_some_and(|written_at| written_at.elapsed() < HID_WINDOW_MIN_INTERVAL)
        {
            return Ok(());
        }
        if let Some((width, height)) = self.profile.window {
            let image = match mode {
                DisplayMode::Context => {
                    render_window_image(&context, width, height, self.profile.rotation)?
                }
                DisplayMode::Usage => {
                    render_usage_image(&context, width, height, self.profile.rotation)?
                }
            };
            self.send_image(0x0b, 0, &image, 0, 8)?;
            self.last_display_write_at = Some(Instant::now());
        }
        self.last_display_context = Some(context);
        self.last_display_mode = Some(mode);
        self.pending_display_context = None;
        self.pending_display_mode = None;
        Ok(())
    }
}

impl PhysicalController for StreamDeckDevice {
    fn kind(&self) -> ControllerKind {
        self.profile.kind
    }
    fn model(&self) -> &'static str {
        self.profile.model
    }
    fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }
    fn poll(&mut self, timeout_ms: i32) -> Result<Vec<PhysicalEvent>, String> {
        self.flush_display_context(false)?;
        let mut report = vec![0_u8; INPUT_REPORT_BYTES];
        let read = self
            .device
            .as_ref()
            .expect("Stream Deck HID handle is present")
            .read_timeout(&mut report, timeout_ms)
            .map_err(|error| error.to_string())?;
        let events = self.events_from_report(&report[..read]);
        self.flush_display_context(false)?;
        Ok(events)
    }
    fn apply_rgb_config(&mut self, parameters: &Value) -> Result<(), String> {
        if !parameters.is_object() {
            return Err("v.oai.rgbcfg parameters must be an object".to_owned());
        }
        Ok(())
    }
    fn apply_thread_status(&mut self, parameters: &Value) -> Result<(), String> {
        let Some(entries) = parameters.as_array() else {
            return Err("v.oai.thstatus parameters must be an array".to_owned());
        };
        let mut colors = self.last_colors;
        let mut agents = self.last_agents.clone();
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if id >= STATUS_SLOTS as u64 {
                continue;
            }
            let index = id as usize;
            let color = entry.get("c").and_then(Value::as_u64).unwrap_or(0) as u32;
            let brightness = entry
                .get("b")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            if entry.get("e").and_then(Value::as_u64) == Some(0) || brightness <= f64::EPSILON {
                colors[index] = None;
                agents[index] = None;
            } else {
                colors[index] = Some((color & 0x00ff_ffff, (brightness * 100.0) as u8));
                agents[index] = entry
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
        }
        for (index, color) in colors.into_iter().enumerate() {
            if self.last_colors[index] == color && self.last_agents[index] == agents[index] {
                continue;
            }
            let image = match color {
                Some((rgb, brightness)) => encode_image_with_agent(
                    self.profile.key_size,
                    self.profile.key_size,
                    rgb_bytes(rgb, brightness),
                    Some(index),
                    true,
                    self.profile.rotation,
                    agents[index].as_deref(),
                )?,
                None => encode_image(
                    self.profile.key_size,
                    self.profile.key_size,
                    [0, 0, 0],
                    None,
                    false,
                    self.profile.rotation,
                )?,
            };
            self.write_key_image(index, &image)?;
            self.last_colors[index] = color;
            self.last_agents[index] = agents[index].clone();
        }
        Ok(())
    }
    fn apply_display_context(&mut self, context: &DisplayContext) -> Result<(), String> {
        self.display_context = Some(context.clone());
        if self.last_display_context.as_ref() == Some(context)
            && self.last_display_mode == Some(self.display_mode)
            && self.pending_display_context.is_none()
        {
            return Ok(());
        }
        self.pending_display_context = Some(context.clone());
        self.pending_display_mode = Some(self.display_mode);
        self.flush_display_context(false)
    }

    fn shutdown(&mut self) {
        let _ = self.send_feature(&[0x03, 0x02]);
    }
}

fn render_window_image(
    context: &DisplayContext,
    width: u32,
    height: u32,
    rotation: Rotation,
) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, height, Rgb([5, 8, 15]));
    let project = context.project.as_deref().unwrap_or("MICRO-EMU");
    let task = context.task.as_deref().unwrap_or("WAITING FOR TASK");
    let model = context.model.as_deref().unwrap_or("CODEX");
    let effort = context.effort.as_deref().unwrap_or("DEFAULT");
    let status = context.status.as_deref().unwrap_or("READY");
    let actionable = context.prompt.as_deref();

    draw_text(&mut image, task, 16, 8, 2, [230, 235, 245]);
    draw_text(
        &mut image,
        &format!("{project}  |  {model}  |  {effort}"),
        16,
        38,
        1,
        [112, 205, 255],
    );
    if let Some(prompt) = actionable {
        draw_text(&mut image, &format!("{}: {}", context.wait_reason.as_deref().unwrap_or("WAITING"), prompt.chars().take(80).collect::<String>()), 16, 80, 1, [245, 175, 65]);
        draw_text(&mut image, &format!("TAP {}  HOLD {}", context.short_action.as_deref().unwrap_or("—"), context.long_action.as_deref().unwrap_or("—")), 16, height.saturating_sub(18), 1, [255, 210, 120]);
    }
    let progress_label = context
        .progress
        .map(|progress| format!("  {progress}%"))
        .unwrap_or_default();
    draw_text(
        &mut image,
        &format!("{status}{progress_label}"),
        16,
        62,
        1,
        [160, 225, 190],
    );

    if let Some(progress) = context.progress {
        let x0 = 16_u32;
        let y0 = height.saturating_sub(14);
        let bar_width = width.saturating_sub(32);
        for y in y0..height.saturating_sub(6) {
            for x in x0..x0.saturating_add(bar_width) {
                image.put_pixel(x, y, Rgb([20, 32, 48]));
            }
        }
        let fill_width = bar_width * u32::from(progress) / 100;
        for y in y0..height.saturating_sub(6) {
            for x in x0..x0.saturating_add(fill_width) {
                image.put_pixel(x, y, Rgb([65, 190, 125]));
            }
        }
    }

    let image = rotate_image(&image, rotation);
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn render_usage_image(
    context: &DisplayContext,
    width: u32,
    height: u32,
    rotation: Rotation,
) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, height, Rgb([5, 8, 15]));
    let task = context.task.as_deref().unwrap_or("ACTIVE TASK");
    draw_text(
        &mut image,
        &format!("LIMITS  |  {task}"),
        16,
        6,
        2,
        [230, 235, 245],
    );
    if let Some(prompt) = context.prompt.as_deref() {
        draw_text(&mut image, &format!("{}: {}", context.wait_reason.as_deref().unwrap_or("WAITING"), prompt.chars().take(80).collect::<String>()), 16, 80, 1, [245, 175, 65]);
        draw_text(&mut image, &format!("TAP {}  HOLD {}", context.short_action.as_deref().unwrap_or("—"), context.long_action.as_deref().unwrap_or("—")), 16, height.saturating_sub(18), 1, [255, 210, 120]);
    }
    if context.weekly_remaining.is_none() && context.five_hour_remaining.is_none() {
        let status = context.status.as_deref().unwrap_or("READY");
    let _actionable = context.prompt.as_deref();
        let progress = context
            .progress
            .map(|value| format!("  |  PROGRESS {value}%"))
            .unwrap_or_default();
        draw_text(
            &mut image,
            "USAGE LIMIT DATA NOT PUBLISHED",
            16,
            40,
            1,
            [245, 175, 65],
        );
        draw_text(
            &mut image,
            &format!("STATUS {status}{progress}"),
            16,
            64,
            1,
            [160, 225, 190],
        );
    } else {
        let gap = 24;
        let column_width = width.saturating_sub(32 + gap) / 2;
        draw_usage_meter(
            &mut image,
            "WEEKLY",
            context.weekly_remaining,
            16,
            38,
            column_width,
        );
        draw_usage_meter(
            &mut image,
            "5-HOUR",
            context.five_hour_remaining,
            16 + column_width + gap,
            38,
            column_width,
        );
    }
    let image = rotate_image(&image, rotation);
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn draw_usage_meter(
    image: &mut RgbImage,
    label: &str,
    remaining: Option<u8>,
    x: u32,
    y: u32,
    width: u32,
) {
    draw_text(image, label, x, y, 1, [112, 205, 255]);
    let value = remaining
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "NOT SET".to_owned());
    // Double the resource percentage size and leave a little less room for the bar.
    let value_scale = if remaining.is_some() { 4 } else { 2 };
    draw_text(image, &value, x + 78, y - 1, value_scale, [230, 235, 245]);
    let bar_x = x + 180;
    let bar_y = y + 2;
    let bar_width = width.saturating_sub(180);
    let bar_height = 14;
    for yy in bar_y..bar_y + bar_height {
        for xx in bar_x..bar_x + bar_width {
            image.put_pixel(xx, yy, Rgb([20, 32, 48]));
        }
    }
    if let Some(value) = remaining {
        let fill_width = bar_width * u32::from(value) / 100;
        let color = if value < 20 {
            [220, 85, 85]
        } else if value < 50 {
            [245, 175, 65]
        } else {
            [65, 190, 125]
        };
        for yy in bar_y..bar_y + bar_height {
            for xx in bar_x..bar_x + fill_width {
                image.put_pixel(xx, yy, Rgb(color));
            }
        }
    }
}

fn draw_text(image: &mut RgbImage, text: &str, x: u32, y: u32, scale: u32, color: [u8; 3]) {
    let scale = scale.max(1);
    let max_chars = image.width().saturating_sub(x) / (6 * scale);
    for (index, raw) in text.chars().take(max_chars as usize).enumerate() {
        let glyph = glyph(normalize_char(raw));
        let origin_x = x + index as u32 * 6 * scale;
        for (column, bits) in glyph.iter().enumerate() {
            for row in 0..7_u32 {
                if bits & (1 << row) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = origin_x + column as u32 * scale + dx;
                        let py = y + row * scale + dy;
                        if px < image.width() && py < image.height() {
                            image.put_pixel(px, py, Rgb(color));
                        }
                    }
                }
            }
        }
    }
}

fn normalize_char(value: char) -> char {
    match value {
        'á' | 'Á' => 'A',
        'é' | 'É' => 'E',
        'í' | 'Í' => 'I',
        'ó' | 'Ó' => 'O',
        'ú' | 'Ú' | 'ü' | 'Ü' => 'U',
        'ñ' | 'Ñ' => 'N',
        value if value.is_ascii() => value.to_ascii_uppercase(),
        _ => '?',
    }
}

fn glyph(value: char) -> [u8; 5] {
    match value {
        'A' => [0x7e, 0x09, 0x09, 0x09, 0x7e],
        'B' => [0x7f, 0x49, 0x49, 0x49, 0x36],
        'C' => [0x3e, 0x41, 0x41, 0x41, 0x22],
        'D' => [0x7f, 0x41, 0x41, 0x22, 0x1c],
        'E' => [0x7f, 0x49, 0x49, 0x49, 0x41],
        'F' => [0x7f, 0x09, 0x09, 0x09, 0x01],
        'G' => [0x3e, 0x41, 0x49, 0x49, 0x7a],
        'H' => [0x7f, 0x08, 0x08, 0x08, 0x7f],
        'I' => [0x00, 0x41, 0x7f, 0x41, 0x00],
        'J' => [0x20, 0x40, 0x41, 0x3f, 0x01],
        'K' => [0x7f, 0x08, 0x14, 0x22, 0x41],
        'L' => [0x7f, 0x40, 0x40, 0x40, 0x40],
        'M' => [0x7f, 0x02, 0x0c, 0x02, 0x7f],
        'N' => [0x7f, 0x04, 0x08, 0x10, 0x7f],
        'O' => [0x3e, 0x41, 0x41, 0x41, 0x3e],
        'P' => [0x7f, 0x09, 0x09, 0x09, 0x06],
        'Q' => [0x3e, 0x41, 0x51, 0x21, 0x5e],
        'R' => [0x7f, 0x09, 0x19, 0x29, 0x46],
        'S' => [0x46, 0x49, 0x49, 0x49, 0x31],
        'T' => [0x01, 0x01, 0x7f, 0x01, 0x01],
        'U' => [0x3f, 0x40, 0x40, 0x40, 0x3f],
        'V' => [0x1f, 0x20, 0x40, 0x20, 0x1f],
        'W' => [0x7f, 0x20, 0x18, 0x20, 0x7f],
        'X' => [0x63, 0x14, 0x08, 0x14, 0x63],
        'Y' => [0x07, 0x08, 0x70, 0x08, 0x07],
        'Z' => [0x61, 0x51, 0x49, 0x45, 0x43],
        '0' => [0x3e, 0x45, 0x49, 0x51, 0x3e],
        '1' => [0x00, 0x41, 0x7f, 0x40, 0x00],
        '2' => [0x42, 0x61, 0x51, 0x49, 0x46],
        '3' => [0x21, 0x41, 0x45, 0x4b, 0x31],
        '4' => [0x18, 0x14, 0x12, 0x7f, 0x10],
        '5' => [0x27, 0x45, 0x45, 0x45, 0x39],
        '6' => [0x3c, 0x4a, 0x49, 0x49, 0x30],
        '7' => [0x01, 0x71, 0x09, 0x05, 0x03],
        '8' => [0x36, 0x49, 0x49, 0x49, 0x36],
        '9' => [0x06, 0x49, 0x49, 0x29, 0x1e],
        '-' => [0x08, 0x08, 0x08, 0x08, 0x08],
        '/' => [0x20, 0x10, 0x08, 0x04, 0x02],
        ':' => [0x00, 0x36, 0x36, 0x00, 0x00],
        '.' => [0x00, 0x40, 0x60, 0x00, 0x00],
        '_' => [0x40, 0x40, 0x40, 0x40, 0x40],
        '%' => [0x63, 0x13, 0x08, 0x64, 0x63],
        '?' => [0x02, 0x01, 0x51, 0x09, 0x06],
        ' ' => [0; 5],
        _ => [0x7f, 0x41, 0x5d, 0x41, 0x7f],
    }
}
fn rotate_image(image: &RgbImage, rotation: Rotation) -> RgbImage {
    match rotation {
        Rotation::None => image.clone(),
        Rotation::Rotate90Ccw => imageops::rotate270(image),
        Rotation::Rotate180 => imageops::rotate180(image),
    }
}
fn rgb_bytes(color: u32, brightness: u8) -> [u8; 3] {
    let scale = u32::from(brightness.min(100));
    [
        (((color >> 16) & 0xff) * scale / 100) as u8,
        (((color >> 8) & 0xff) * scale / 100) as u8,
        ((color & 0xff) * scale / 100) as u8,
    ]
}

fn encode_icon_image(width: u32, icon: Icon, rotation: Rotation) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, width, Rgb([5, 8, 15]));
    let center = (width / 2) as i32;
    let radius = (width / 6).max(3) as i32;
    let primary = match icon {
        Icon::ArrowUp | Icon::ArrowDown | Icon::ArrowLeft | Icon::ArrowRight => [82, 180, 255],
        Icon::Send => [255, 210, 90],
        Icon::RotorLeft | Icon::RotorCenter | Icon::RotorRight => [255, 170, 65],
        Icon::Mic => [80, 220, 145],
    };
    match icon {
        Icon::ArrowUp => {
            draw_line(
                &mut image,
                center,
                center + radius,
                center,
                center - radius,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center,
                center - radius,
                center - radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center,
                center - radius,
                center + radius / 2,
                center - radius / 2,
                primary,
                3,
            );
        }
        Icon::ArrowDown => {
            draw_line(
                &mut image,
                center,
                center - radius,
                center,
                center + radius,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center,
                center + radius,
                center - radius / 2,
                center + radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center,
                center + radius,
                center + radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
        Icon::ArrowLeft => {
            draw_line(
                &mut image,
                center + radius,
                center,
                center - radius,
                center,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center - radius,
                center,
                center - radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center - radius,
                center,
                center - radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
        Icon::ArrowRight => {
            draw_line(
                &mut image,
                center - radius,
                center,
                center + radius,
                center,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
        Icon::RotorLeft => {
            draw_line(
                &mut image,
                center + radius,
                center,
                center - radius,
                center,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center - radius,
                center,
                center - radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center - radius,
                center,
                center - radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
        Icon::RotorRight => {
            draw_line(
                &mut image,
                center - radius,
                center,
                center + radius,
                center,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
        Icon::RotorCenter => {
            draw_circle(&mut image, center, center, radius, primary, 3);
            draw_line(
                &mut image,
                center - radius / 2,
                center,
                center + radius / 2,
                center,
                primary,
                2,
            );
        }
        Icon::Mic => {
            let top = center - radius;
            let bottom = center + radius / 2;
            draw_line(&mut image, center, top, center, bottom, primary, 5);
            draw_line(
                &mut image,
                center - radius / 2,
                top + radius / 3,
                center - radius / 2,
                bottom,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius / 2,
                top + radius / 3,
                center + radius / 2,
                bottom,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center - radius,
                bottom,
                center + radius,
                bottom,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center,
                bottom,
                center,
                bottom + radius / 2,
                primary,
                3,
            );
        }
        Icon::Send => {
            draw_line(
                &mut image,
                center - radius,
                center,
                center + radius,
                center,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center - radius / 2,
                primary,
                3,
            );
            draw_line(
                &mut image,
                center + radius,
                center,
                center + radius / 2,
                center + radius / 2,
                primary,
                3,
            );
        }
    }
    let image = rotate_image(&image, rotation);
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn draw_line(
    image: &mut RgbImage,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: [u8; 3],
    thickness: i32,
) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    let half = (thickness.max(1) / 2) as i32;
    for step in 0..=steps {
        let x = x0 + (x1 - x0) * step / steps;
        let y = y0 + (y1 - y0) * step / steps;
        for dy in -half..=half {
            for dx in -half..=half {
                let px = x + dx;
                let py = y + dy;
                if px >= 0 && py >= 0 && (px as u32) < image.width() && (py as u32) < image.height()
                {
                    image.put_pixel(px as u32, py as u32, Rgb(color));
                }
            }
        }
    }
}

fn draw_circle(
    image: &mut RgbImage,
    cx: i32,
    cy: i32,
    radius: i32,
    color: [u8; 3],
    thickness: i32,
) {
    let outer = radius.max(1);
    let inner = (outer - thickness.max(1)).max(0);
    for y in -outer..=outer {
        for x in -outer..=outer {
            let distance = x * x + y * y;
            if distance <= outer * outer && distance >= inner * inner {
                let px = cx + x;
                let py = cy + y;
                if px >= 0 && py >= 0 && (px as u32) < image.width() && (py as u32) < image.height()
                {
                    image.put_pixel(px as u32, py as u32, Rgb(color));
                }
            }
        }
    }
}
fn encode_image(
    width: u32,
    height: u32,
    rgb: [u8; 3],
    digit: Option<usize>,
    draw_digits: bool,
    rotation: Rotation,
) -> Result<Vec<u8>, String> {
    encode_image_with_agent(width, height, rgb, digit, draw_digits, rotation, None)
}

fn encode_image_with_agent(
    width: u32,
    height: u32,
    rgb: [u8; 3],
    digit: Option<usize>,
    draw_digits: bool,
    rotation: Rotation,
    agent: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, height, Rgb(rgb));
    if draw_digits {
        if let Some(agent) = agent {
            draw_agent_label(&mut image, agent);
        }
        let cell = (width.min(height) / 10).max(6);
        let origin_x = (width - 3 * cell) / 2;
        let origin_y = height.saturating_sub(5 * cell + 16);
        if let Some(digit) = digit {
            for row in 0..5 {
                for column in 0..3 {
                    if DIGITS[digit][row * 3 + column] == 0 {
                        continue;
                    }
                    for y in 2..cell.saturating_sub(2) {
                        for x in 2..cell.saturating_sub(2) {
                            image.put_pixel(
                                origin_x + column as u32 * cell + x,
                                origin_y + row as u32 * cell + y,
                                Rgb([196, 194, 255]),
                            );
                        }
                    }
                }
            }
        }
    }
    let image = rotate_image(&image, rotation);
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn draw_agent_label(image: &mut RgbImage, agent: &str) {
    let text = agent.to_ascii_lowercase();
    let scale = (image.width().min(image.height()) / 32).max(2);
    let width = text.chars().count() as u32 * (4 * scale);
    let start_x = image.width().saturating_sub(width) / 2;
    for (index, ch) in text.chars().enumerate() {
        let glyph = agent_glyph(ch);
        for row in 0..5 {
            for column in 0..3 {
                if glyph[row * 3 + column] == 0 {
                    continue;
                }
                for y in 0..scale {
                    for x in 0..scale {
                        image.put_pixel(
                            start_x + index as u32 * 4 * scale + column as u32 * scale + x,
                            4 + row as u32 * scale + y,
                            Rgb([196, 194, 255]),
                        );
                    }
                }
            }
        }
    }
}

fn agent_glyph(ch: char) -> [u8; 15] {
    match ch {
        'c' => [0, 1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 0, 1, 1],
        'd' => [1, 1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 0],
        'e' => [1, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 1],
        'h' => [1, 0, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 0, 1],
        'm' => [1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 0, 1],
        'o' => [0, 1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 1, 0, 1, 0],
        'r' => [1, 1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 1, 1, 0, 1],
        's' => [0, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 1, 1, 1, 0],
        'x' => [1, 0, 1, 0, 1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 1],
        'z' => [1, 1, 1, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1],
        _ => [0; 15],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;

    fn report(command: u8, payload: &[u8]) -> Vec<u8> {
        let mut result = vec![0, 0, 0, 0];
        result[0] = 0x01;
        result[1] = command;
        result[2..4].copy_from_slice(&(payload.len() as u16).to_le_bytes());
        result.extend_from_slice(payload);
        result
    }

    #[test]
    fn plus_button_snapshots_emit_only_transitions() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: PLUS,
            serial: None,
            pressed_keys: vec![false; 8],
            pressed_encoders: vec![false; 4],
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: None,
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: None,
        };
        let first = device.events_from_report(&report(0x00, &[1, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(
            first,
            vec![PhysicalEvent::Button {
                index: 0,
                pressed: true
            }]
        );
        assert!(
            device
                .events_from_report(&report(0x00, &[1, 0, 0, 0, 0, 0, 0, 0]))
                .is_empty()
        );
    }

    #[test]
    fn plus_encoder_ticks_expand_and_touch_is_ignored() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: PLUS,
            serial: None,
            pressed_keys: vec![false; 8],
            pressed_encoders: vec![false; 4],
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: None,
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: None,
        };
        assert_eq!(
            device.events_from_report(&report(0x03, &[1, 2, 255, 0, 0])),
            vec![
                PhysicalEvent::EncoderTurn { index: 0, delta: 1 },
                PhysicalEvent::EncoderTurn { index: 0, delta: 1 },
                PhysicalEvent::EncoderTurn {
                    index: 1,
                    delta: -1
                }
            ]
        );
        assert!(
            device
                .events_from_report(&report(0x02, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
                .is_empty()
        );
    }

    #[test]
    fn horizontal_flicks_are_directional_and_idempotent() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: PLUS,
            serial: None,
            pressed_keys: vec![false; 8],
            pressed_encoders: vec![false; 4],
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: Some(DisplayContext {
                weekly_remaining: Some(73),
                five_hour_remaining: Some(28),
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
                ..DisplayContext::default()
            }),
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: Some(Instant::now()),
        };
        let left = [0x03, 0, 138, 2, 20, 0, 100, 0, 22, 0];
        device.events_from_report(&report(0x02, &left));
        assert_eq!(device.display_mode, DisplayMode::Usage);
        device.events_from_report(&report(0x02, &left));
        assert_eq!(device.display_mode, DisplayMode::Usage);
        let updated = DisplayContext {
            weekly_remaining: Some(42),
            five_hour_remaining: Some(81),
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
            ..DisplayContext::default()
        };
        device
            .apply_display_context(&updated)
            .expect("usage refresh");
        assert_eq!(device.pending_display_context, Some(updated));

        let right = [0x03, 0, 100, 0, 20, 0, 138, 2, 22, 0];
        device.events_from_report(&report(0x02, &right));
        assert_eq!(device.display_mode, DisplayMode::Context);

        let vertical = [0x03, 0, 100, 0, 0, 0, 110, 0, 100, 0];
        device.events_from_report(&report(0x02, &vertical));
        assert_eq!(device.display_mode, DisplayMode::Context);
    }

    #[test]
    fn xl_snapshot_handles_multiple_keys_and_release() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: XL,
            serial: None,
            pressed_keys: vec![false; 32],
            pressed_encoders: Vec::new(),
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: None,
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: None,
        };
        let mut pressed = vec![0_u8; 32];
        pressed[0] = 1;
        pressed[6] = 1;
        let events = device.events_from_report(&report(0x00, &pressed));
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            PhysicalEvent::Button {
                index: 0,
                pressed: true
            }
        );
        assert_eq!(
            events[1],
            PhysicalEvent::Button {
                index: 6,
                pressed: true
            }
        );
        let released = device.events_from_report(&report(0x00, &[0; 32]));
        assert_eq!(released.len(), 2);
    }
    #[test]
    fn xl_virtual_layout_reuses_existing_encoder_events() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: XL,
            serial: None,
            pressed_keys: vec![false; 32],
            pressed_encoders: Vec::new(),
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: None,
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: None,
        };
        let mut pressed = vec![0_u8; 32];
        for index in [11, 14, 18, 19, 20, 27, 29, 30, 31] {
            pressed[index] = 1;
        }
        assert_eq!(
            device.events_from_report(&report(0x00, &pressed)),
            vec![
                PhysicalEvent::EncoderTurn { index: 2, delta: 1 },
                PhysicalEvent::EncoderButton {
                    index: 2,
                    pressed: true
                },
                PhysicalEvent::EncoderTurn {
                    index: 0,
                    delta: -1
                },
                PhysicalEvent::EncoderButton {
                    index: 0,
                    pressed: true
                },
                PhysicalEvent::EncoderTurn { index: 0, delta: 1 },
                PhysicalEvent::EncoderTurn {
                    index: 2,
                    delta: -1
                },
                PhysicalEvent::EncoderTurn {
                    index: 1,
                    delta: -1
                },
                PhysicalEvent::EncoderButton {
                    index: 1,
                    pressed: true
                },
                PhysicalEvent::EncoderTurn { index: 1, delta: 1 },
            ]
        );
        assert!(
            device
                .events_from_report(&report(0x00, &pressed))
                .is_empty()
        );
        let releases = device.events_from_report(&report(0x00, &[0; 32]));
        assert_eq!(
            releases,
            vec![
                PhysicalEvent::EncoderButton {
                    index: 2,
                    pressed: false
                },
                PhysicalEvent::EncoderButton {
                    index: 0,
                    pressed: false
                },
                PhysicalEvent::EncoderButton {
                    index: 1,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn xl_reserved_keys_are_silent_and_icons_are_fixed() {
        let mut device = StreamDeckDevice {
            device: None,
            write_lock: Mutex::new(()),
            path: CString::new("test").expect("test path"),
            profile: XL,
            serial: None,
            pressed_keys: vec![false; 32],
            pressed_encoders: Vec::new(),
            last_colors: [None; STATUS_SLOTS],
            last_agents: std::array::from_fn(|_| None),
            last_display_context: None,
            display_context: None,
            display_mode: DisplayMode::Context,
            last_display_mode: None,
            pending_display_context: None,
            pending_display_mode: None,
            last_display_write_at: None,
        };
        let mut reserved = vec![0_u8; 32];
        reserved[9] = 1;
        reserved[12] = 1;
        assert!(
            device
                .events_from_report(&report(0x00, &reserved))
                .is_empty()
        );
        assert_eq!(
            icon_for_key(ControllerKind::StreamDeckXl, 14),
            Some(Icon::Mic)
        );
        assert_eq!(
            icon_for_key(ControllerKind::StreamDeckXl, 19),
            Some(Icon::Send)
        );
        assert!(icon_for_key(ControllerKind::StreamDeckXl, 9).is_none());
        assert!(icon_for_key(ControllerKind::StreamDeckPlus, 14).is_none());
    }
    #[test]
    fn dashboard_context_renders_a_window_sized_jpeg() {
        let context = DisplayContext {
            project: Some("micro-emu".to_owned()),
            task: Some("Stream Deck".to_owned()),
            model: Some("gpt-5".to_owned()),
            effort: Some("high".to_owned()),
            status: Some("working".to_owned()),
            progress: Some(65),
            task_id: None,
            weekly_remaining: None,
            five_hour_remaining: None,
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
        };
        let image = render_window_image(&context, 800, 100, Rotation::None).unwrap();
        assert_eq!(
            image::load_from_memory(&image).unwrap().dimensions(),
            (800, 100)
        );
        assert!(image.len() > 100);
        let blank =
            render_window_image(&DisplayContext::default(), 800, 100, Rotation::None).unwrap();
        assert_ne!(image, blank);
    }

    #[test]
    fn usage_screen_renders_numeric_and_graphical_limits() {
        let context = DisplayContext {
            weekly_remaining: Some(73),
            five_hour_remaining: Some(28),
            wait_reason: None,
            prompt: None,
            interaction_id: None,
            short_action: None,
            long_action: None,
            pending_wait_count: None,
            ..DisplayContext::default()
        };
        let image = render_usage_image(&context, 800, 100, Rotation::None).unwrap();
        assert_eq!(
            image::load_from_memory(&image).unwrap().dimensions(),
            (800, 100)
        );
        assert!(image.len() > 100);
        let missing =
            render_usage_image(&DisplayContext::default(), 800, 100, Rotation::None).unwrap();
        assert_ne!(image, missing);
    }

    #[test]
    fn profile_constants_target_only_requested_models() {
        assert_eq!(
            (PLUS.pid, PLUS.key_count, PLUS.encoder_count),
            (0x0084, 8, 4)
        );
        assert_eq!((XL.pid, XL.key_count, XL.encoder_count), (0x006c, 32, 0));
        assert_eq!(
            (PLUS_XL.pid, PLUS_XL.key_count, PLUS_XL.encoder_count),
            (0x00c6, 36, 6)
        );
        let plus = encode_image(120, 120, [0, 0, 0], Some(0), true, Rotation::None).unwrap();
        let xl = encode_image(96, 96, [0, 0, 0], Some(0), true, Rotation::Rotate180).unwrap();
        let plus_xl =
            render_window_image(&DisplayContext::default(), 1200, 100, Rotation::Rotate90Ccw)
                .unwrap();
        assert_eq!(
            image::load_from_memory(&plus).unwrap().dimensions(),
            (120, 120)
        );
        assert_eq!(image::load_from_memory(&xl).unwrap().dimensions(), (96, 96));
        assert_eq!(
            image::load_from_memory(&plus_xl).unwrap().dimensions(),
            (100, 1200)
        );
    }
}
