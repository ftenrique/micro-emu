use crate::codex::PhysicalEvent;
use crate::controller::{ControllerKind, DisplayContext, PhysicalController};
use hidapi::{HidApi, HidDevice};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, RgbImage, imageops};
use serde_json::Value;
use std::io::Cursor;

const VENDOR_ID: u16 = 0x0fd9;
const PLUS_PID: u16 = 0x0084;
const XL_PID: u16 = 0x006c;
const OUTPUT_REPORT_BYTES: usize = 1024;
const FEATURE_REPORT_BYTES: usize = 32;
const INPUT_REPORT_BYTES: usize = 512;
const STATUS_SLOTS: usize = 6;
const DIGITS: [[u8; 15]; STATUS_SLOTS] = [
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0],
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0],
];

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
    rotate_180: bool,
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
    rotate_180: false,
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
    rotate_180: true,
};

pub fn connect(
    kind: ControllerKind,
    requested_serial: Option<&str>,
) -> Result<Box<dyn PhysicalController>, String> {
    let profile = match kind {
        ControllerKind::StreamDeckPlus => PLUS,
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
    let serial = info.serial_number().map(str::to_owned);
    let device = api.open_path(info.path()).map_err(|error| {
        format!(
            "could not open Stream Deck {}; close the official app first: {error}",
            profile.model
        )
    })?;
    validate_unit_info(&device, profile)?;
    let mut result = StreamDeckDevice {
        device: Some(device),
        profile,
        serial,
        pressed_keys: vec![false; profile.key_count],
        pressed_encoders: vec![false; profile.encoder_count],
        last_colors: [None; STATUS_SLOTS],
        last_display_context: None,
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
    profile: Profile,
    serial: Option<String>,
    pressed_keys: Vec<bool>,
    pressed_encoders: Vec<bool>,
    last_colors: [Option<(u32, u8)>; STATUS_SLOTS],
    last_display_context: Option<DisplayContext>,
}

impl StreamDeckDevice {
    fn initialize(&mut self) -> Result<(), String> {
        self.set_brightness(100)?;
        self.fill_screen([0, 0, 0])?;
        for key in 0..self.profile.key_count {
            self.fill_key(key as u8, [0, 0, 0])?;
        }
        if self.profile.window.is_some() {
            let image = encode_image(800, 100, [0, 0, 0], None, false, false)?;
            self.send_image(0x0b, 0, &image, 0, 8)?;
        }
        Ok(())
    }

    fn set_brightness(&self, brightness: u8) -> Result<(), String> {
        self.send_feature(&[0x03, 0x08, brightness.min(100)])
    }

    fn fill_screen(&self, rgb: [u8; 3]) -> Result<(), String> {
        self.send_feature(&[0x03, 0x05, rgb[0], rgb[1], rgb[2]])
    }

    fn fill_key(&self, key: u8, rgb: [u8; 3]) -> Result<(), String> {
        self.send_feature(&[0x03, 0x06, key, rgb[0], rgb[1], rgb[2]])
    }

    fn send_feature(&self, payload: &[u8]) -> Result<(), String> {
        let mut report = [0_u8; FEATURE_REPORT_BYTES];
        report[..payload.len()].copy_from_slice(payload);
        self.device
            .as_ref()
            .expect("Stream Deck HID handle is present")
            .send_feature_report(&report)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    fn send_image(
        &self,
        command: u8,
        target: u8,
        image: &[u8],
        extra: u8,
        header_len: usize,
    ) -> Result<(), String> {
        let capacity = OUTPUT_REPORT_BYTES - header_len;
        for (index, chunk) in image.chunks(capacity).enumerate() {
            let mut report = vec![0_u8; OUTPUT_REPORT_BYTES];
            report[0] = 0x02;
            report[1] = command;
            if command == 0x07 {
                report[2] = target;
                report[3] = u8::from(index + 1 == image.chunks(capacity).len());
                report[4..6].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
                report[6..8].copy_from_slice(&(index as u16).to_le_bytes());
                report[8..8 + chunk.len()].copy_from_slice(chunk);
            } else if command == 0x0b {
                report[2] = 0;
                report[3] = u8::from(index + 1 == image.chunks(capacity).len());
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
            let written = self
                .device
                .as_ref()
                .expect("Stream Deck HID handle is present")
                .write(&report)
                .map_err(|error| error.to_string())?;
            if written != report.len() {
                return Err(format!(
                    "short Stream Deck image write: {written}/{}",
                    report.len()
                ));
            }
        }
        Ok(())
    }

    fn write_key_image(&self, key: usize, image: &[u8]) -> Result<(), String> {
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
            0x02 => Vec::new(),
            0x03 if self.profile.encoder_count > 0 => self.encoder_events(payload),
            _ => Vec::new(),
        }
    }

    fn key_events(&mut self, payload: &[u8]) -> Vec<PhysicalEvent> {
        let mut events = Vec::new();
        for (index, &state) in payload.iter().take(self.profile.key_count).enumerate() {
            let pressed = state != 0;
            if pressed != self.pressed_keys[index] {
                self.pressed_keys[index] = pressed;
                events.push(PhysicalEvent::Button {
                    index: index as u8,
                    pressed,
                });
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
        let mut report = vec![0_u8; INPUT_REPORT_BYTES];
        let read = self
            .device
            .as_ref()
            .expect("Stream Deck HID handle is present")
            .read_timeout(&mut report, timeout_ms)
            .map_err(|error| error.to_string())?;
        Ok(self.events_from_report(&report[..read]))
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
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if id >= STATUS_SLOTS as u64 {
                continue;
            }
            let color = entry.get("c").and_then(Value::as_u64).unwrap_or(0) as u32;
            let brightness = entry
                .get("b")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            if entry.get("e").and_then(Value::as_u64) == Some(0) || brightness <= f64::EPSILON {
                colors[id as usize] = None;
            } else {
                colors[id as usize] = Some((color & 0x00ff_ffff, (brightness * 100.0) as u8));
            }
        }
        for (index, color) in colors.into_iter().enumerate() {
            if self.last_colors[index] == color {
                continue;
            }
            let image = match color {
                Some((rgb, brightness)) => encode_image(
                    self.profile.key_size,
                    self.profile.key_size,
                    rgb_bytes(rgb, brightness),
                    Some(index),
                    true,
                    self.profile.rotate_180,
                )?,
                None => encode_image(
                    self.profile.key_size,
                    self.profile.key_size,
                    [0, 0, 0],
                    None,
                    false,
                    self.profile.rotate_180,
                )?,
            };
            self.write_key_image(index, &image)?;
            self.last_colors[index] = color;
        }
        Ok(())
    }
    fn apply_display_context(&mut self, context: &DisplayContext) -> Result<(), String> {
        if self.last_display_context.as_ref() == Some(context) {
            return Ok(());
        }
        if let Some((width, height)) = self.profile.window {
            let image = render_window_image(context, width, height)?;
            self.send_image(0x0b, 0, &image, 0, 8)?;
        }
        self.last_display_context = Some(context.clone());
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.send_feature(&[0x03, 0x02]);
    }
}

fn render_window_image(
    context: &DisplayContext,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, height, Rgb([5, 8, 15]));
    let project = context.project.as_deref().unwrap_or("CODEX");
    let task = context.task.as_deref().unwrap_or("BRIDGE");
    let model = context.model.as_deref().unwrap_or("MODEL");
    let effort = context.effort.as_deref().unwrap_or("EFFORT");
    let status = context.status.as_deref().unwrap_or("READY");

    draw_text(
        &mut image,
        &format!("{project}  /  {task}"),
        16,
        10,
        2,
        [230, 235, 245],
    );
    draw_text(
        &mut image,
        &format!("{model}  /  {effort}  /  {status}"),
        16,
        48,
        2,
        [112, 205, 255],
    );

    if let Some(progress) = context.progress {
        let x0 = 16_u32;
        let y0 = height.saturating_sub(10);
        let bar_width = width.saturating_sub(32);
        for y in y0..height.saturating_sub(4) {
            for x in x0..x0.saturating_add(bar_width) {
                image.put_pixel(x, y, Rgb([20, 32, 48]));
            }
        }
        let fill_width = bar_width * u32::from(progress) / 100;
        for y in y0..height.saturating_sub(4) {
            for x in x0..x0.saturating_add(fill_width) {
                image.put_pixel(x, y, Rgb([65, 190, 125]));
            }
        }
    }

    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
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
fn rgb_bytes(color: u32, brightness: u8) -> [u8; 3] {
    let scale = u32::from(brightness.min(100));
    [
        (((color >> 16) & 0xff) * scale / 100) as u8,
        (((color >> 8) & 0xff) * scale / 100) as u8,
        ((color & 0xff) * scale / 100) as u8,
    ]
}

fn encode_image(
    width: u32,
    height: u32,
    rgb: [u8; 3],
    digit: Option<usize>,
    draw_digits: bool,
    rotate_180: bool,
) -> Result<Vec<u8>, String> {
    let mut image: RgbImage = ImageBuffer::from_pixel(width, height, Rgb(rgb));
    if draw_digits {
        let cell = (width.min(height) / 6).max(8);
        let origin_x = (width - 3 * cell) / 2;
        let origin_y = (height - 5 * cell) / 2;
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
    let image = if rotate_180 {
        imageops::rotate180(&image)
    } else {
        image
    };
    let mut bytes = Vec::new();
    DynamicImage::ImageRgb8(image)
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
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
            profile: PLUS,
            serial: None,
            pressed_keys: vec![false; 8],
            pressed_encoders: vec![false; 4],
            last_colors: [None; 6],
            last_display_context: None,
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
            profile: PLUS,
            serial: None,
            pressed_keys: vec![false; 8],
            pressed_encoders: vec![false; 4],
            last_colors: [None; 6],
            last_display_context: None,
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
    fn xl_snapshot_handles_multiple_keys_and_release() {
        let mut device = StreamDeckDevice {
            device: None,
            profile: XL,
            serial: None,
            pressed_keys: vec![false; 32],
            pressed_encoders: Vec::new(),
            last_colors: [None; 6],
            last_display_context: None,
        };
        let mut pressed = vec![0_u8; 32];
        pressed[0] = 1;
        pressed[6] = 1;
        pressed[31] = 1;
        let events = device.events_from_report(&report(0x00, &pressed));
        assert_eq!(events.len(), 3);
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
        assert_eq!(
            events[2],
            PhysicalEvent::Button {
                index: 31,
                pressed: true
            }
        );
        let released = device.events_from_report(&report(0x00, &[0; 32]));
        assert_eq!(released.len(), 3);
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
        };
        let image = render_window_image(&context, 800, 100).unwrap();
        assert_eq!(
            image::load_from_memory(&image).unwrap().dimensions(),
            (800, 100)
        );
        assert!(image.len() > 100);
        let blank = render_window_image(&DisplayContext::default(), 800, 100).unwrap();
        assert_ne!(image, blank);
    }

    #[test]
    fn profile_constants_target_only_requested_models() {
        assert_eq!(
            (PLUS.pid, PLUS.key_count, PLUS.encoder_count),
            (0x0084, 8, 4)
        );
        assert_eq!((XL.pid, XL.key_count, XL.encoder_count), (0x006c, 32, 0));
        let plus = encode_image(120, 120, [0, 0, 0], Some(0), true, false).unwrap();
        let xl = encode_image(96, 96, [0, 0, 0], Some(0), true, true).unwrap();
        assert_eq!(
            image::load_from_memory(&plus).unwrap().dimensions(),
            (120, 120)
        );
        assert_eq!(image::load_from_memory(&xl).unwrap().dimensions(), (96, 96));
    }
}
