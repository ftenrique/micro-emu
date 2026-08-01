use crate::codex::PhysicalEvent;
use ajazz_sdk::{ImageFormat, ImageMirroring, ImageMode, ImageRotation, convert_image_with_format};
use hidapi::{HidApi, HidDevice};
use image::{DynamicImage, Rgb, RgbImage, imageops::rotate90};
use serde_json::Value;
use std::collections::HashSet;

const VENDOR_ID: u16 = 0x0300;
const PRODUCT_ID: u16 = 0x3002;
const USAGE_PAGE: u16 = 0xffa0;
const USAGE: u16 = 0x0001;
const DISPLAY_KEY_COUNT: usize = 6;
const SOURCE_SIZE: u32 = 126;
const TRANSPORT_SIZE: usize = 60;
const OUTPUT_REPORT_LENGTH: usize = 1025;
const INPUT_REPORT_LENGTH: usize = 513;
const DIGIT_COLOR: [u8; 3] = [196, 194, 255];

const DIGITS: [[u8; 15]; DISPLAY_KEY_COUNT] = [
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0],
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0],
];

pub struct AjazzDevice {
    device: HidDevice,
    pressed_buttons: HashSet<u8>,
    pressed_encoders: [bool; 3],
    last_colors: [Option<(u32, u8)>; DISPLAY_KEY_COUNT],
}

impl AjazzDevice {
    pub fn connect() -> Result<Self, String> {
        let api = HidApi::new().map_err(|error| error.to_string())?;
        let info = api
            .device_list()
            .find(|device| {
                device.vendor_id() == VENDOR_ID
                    && device.product_id() == PRODUCT_ID
                    && device.usage_page() == USAGE_PAGE
                    && device.usage() == USAGE
            })
            .ok_or_else(|| "AKP03E rev. 2 vendor interface FFA0:0001 was not found".to_owned())?;
        let device = api.open_path(info.path()).map_err(|error| {
            format!("could not open AJAZZ MI_00; close the vendor app first: {error}")
        })?;
        let mut result = Self {
            device,
            pressed_buttons: HashSet::new(),
            pressed_encoders: [false; 3],
            last_colors: [None; DISPLAY_KEY_COUNT],
        };
        result.write_command(&[0x44, 0x49, 0x53, 0x00, 0x00])?;
        result.write_command(&[0x4c, 0x49, 0x47, 0x00, 0x00, 100])?;
        result.clear_displays()?;
        Ok(result)
    }

    pub fn poll(&mut self, timeout_ms: i32) -> Result<Vec<PhysicalEvent>, String> {
        let mut report = vec![0_u8; INPUT_REPORT_LENGTH];
        let read = self
            .device
            .read_timeout(&mut report, timeout_ms)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(Vec::new());
        }
        report.truncate(read);
        let header_offset = if report.starts_with(&[0x41, 0x43, 0x4b]) {
            0
        } else if report.get(1..4) == Some(&[0x41, 0x43, 0x4b]) {
            1
        } else {
            return Ok(Vec::new());
        };
        let Some(code) = report.get(header_offset + 9).copied() else {
            return Ok(Vec::new());
        };
        Ok(self.events_from_code(code))
    }

    pub fn apply_rgb_config(&mut self, parameters: &Value) -> Result<(), String> {
        if !parameters.is_object() {
            return Err("v.oai.rgbcfg parameters must be an object".to_owned());
        }
        // Keep the LCD background black. Per-slot colours are driven only by
        // v.oai.thstatus, so a global rgbcfg update cannot repaint every slot.
        Ok(())
    }
    pub fn apply_thread_status(&mut self, parameters: &Value) -> Result<(), String> {
        let Some(entries) = parameters.as_array() else {
            return Err("v.oai.thstatus parameters must be an array".to_owned());
        };
        let mut colors = self.last_colors;
        for entry in entries {
            let Some(id) = entry.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if id >= DISPLAY_KEY_COUNT as u64 {
                continue;
            }
            let color = entry.get("c").and_then(Value::as_u64).unwrap_or(0) as u32;
            let brightness = entry
                .get("b")
                .and_then(Value::as_f64)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            let effect = entry.get("e").and_then(Value::as_u64);
            // Codex Micro uses effect=0 (OFF), or b=0, to release an
            // Agent-Key slot. Treat that as a real clear rather than drawing
            // an indigo number over a black JPEG, which otherwise leaves a
            // visible remnant after a thread finishes.
            if effect == Some(0) || brightness <= f64::EPSILON {
                colors[id as usize] = None;
            } else {
                colors[id as usize] = Some((color & 0x00ff_ffff, (brightness * 100.0) as u8));
            }
        }
        self.apply_colors(colors)
    }

    fn clear_displays(&mut self) -> Result<(), String> {
        // First discard the device's stored frames (including any digit
        // overlay), then commit an explicit black image to every LCD.  CLE by
        // itself leaves the panel at its firmware neutral grey, so the black
        // JPEG is intentional here.
        self.write_command(&[0x43, 0x4c, 0x45, 0x00, 0x00, 0x00, 0xff])?;
        self.write_command(&[0x53, 0x54, 0x50])?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        let encoded = encode_color_image(0, 0, 0, false)?;
        for index in 0..DISPLAY_KEY_COUNT {
            self.write_image(index as u8, &encoded)?;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        self.write_command(&[0x53, 0x54, 0x50])?;
        std::thread::sleep(std::time::Duration::from_millis(50));
        Ok(())
    }

    fn clear_display(&self, key: u8) -> Result<(), String> {
        // key is zero-based in the bridge; the AJAZZ command uses one-based
        // key numbers and 0xff is reserved for all keys.
        self.write_command(&[0x43, 0x4c, 0x45, 0x00, 0x00, 0x00, key + 1])
    }

    fn apply_colors(
        &mut self,
        colors: [Option<(u32, u8)>; DISPLAY_KEY_COUNT],
    ) -> Result<(), String> {
        let mut changed = false;
        for (index, color) in colors.into_iter().enumerate() {
            if self.last_colors[index] == color {
                continue;
            }
            match color {
                Some((rgb, brightness)) => {
                    let encoded = encode_color_image(index, rgb, brightness, true)?;
                    self.write_image(index as u8, &encoded)?;
                }
                None => {
                    // Purge the old frame first, then write an explicit black
                    // image: CLE alone displays the device's neutral grey.
                    self.clear_display(index as u8)?;
                    let encoded = encode_color_image(index, 0, 0, false)?;
                    self.write_image(index as u8, &encoded)?;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            self.last_colors[index] = color;
            changed = true;
        }
        if changed {
            self.write_command(&[0x53, 0x54, 0x50])?;
        }
        Ok(())
    }

    fn events_from_code(&mut self, code: u8) -> Vec<PhysicalEvent> {
        match code {
            0 => self
                .pressed_buttons
                .drain()
                .map(|index| PhysicalEvent::Button {
                    index,
                    pressed: false,
                })
                .collect(),
            1..=6 | 0x25 | 0x30 | 0x31 => {
                let index = match code {
                    1..=6 => code - 1,
                    0x25 => 6,
                    0x30 => 7,
                    0x31 => 8,
                    _ => unreachable!(),
                };
                let pressed = if self.pressed_buttons.insert(index) {
                    true
                } else {
                    self.pressed_buttons.remove(&index);
                    false
                };
                vec![PhysicalEvent::Button { index, pressed }]
            }
            0x90 | 0x91 | 0x50 | 0x51 | 0x60 | 0x61 => {
                let (index, delta) = match code {
                    0x90 => (0, -1),
                    0x91 => (0, 1),
                    0x50 => (1, -1),
                    0x51 => (1, 1),
                    0x60 => (2, -1),
                    0x61 => (2, 1),
                    _ => unreachable!(),
                };
                vec![PhysicalEvent::EncoderTurn { index, delta }]
            }
            0x33..=0x35 => {
                let index = match code {
                    0x33 => 0,
                    0x35 => 1,
                    0x34 => 2,
                    _ => unreachable!(),
                };
                self.pressed_encoders[index as usize] = !self.pressed_encoders[index as usize];
                vec![PhysicalEvent::EncoderButton {
                    index,
                    pressed: self.pressed_encoders[index as usize],
                }]
            }
            _ => Vec::new(),
        }
    }

    fn write_image(&self, key: u8, image: &[u8]) -> Result<(), String> {
        let mut announce = vec![0x42, 0x41, 0x54, 0x00, 0x00];
        announce.push((image.len() >> 8) as u8);
        announce.push(image.len() as u8);
        announce.push(key + 1);
        self.write_command(&announce)?;

        for chunk in image.chunks(OUTPUT_REPORT_LENGTH - 1) {
            let mut packet = Vec::with_capacity(OUTPUT_REPORT_LENGTH);
            packet.push(0);
            packet.extend_from_slice(chunk);
            packet.resize(OUTPUT_REPORT_LENGTH, 0);
            self.write_packet(&packet)?;
        }
        Ok(())
    }

    fn write_command(&self, command: &[u8]) -> Result<(), String> {
        let mut packet = vec![0x00, 0x43, 0x52, 0x54, 0x00, 0x00];
        packet.extend_from_slice(command);
        packet.resize(OUTPUT_REPORT_LENGTH, 0);
        self.write_packet(&packet)
    }

    fn write_packet(&self, packet: &[u8]) -> Result<(), String> {
        let written = self
            .device
            .write(packet)
            .map_err(|error| error.to_string())?;
        if written != packet.len() {
            return Err(format!("short AJAZZ HID write: {written}/{}", packet.len()));
        }
        Ok(())
    }
}

fn encode_color_image(
    index: usize,
    color: u32,
    brightness: u8,
    draw_digits: bool,
) -> Result<Vec<u8>, String> {
    let scale = u32::from(brightness.min(100));
    let base = [
        (((color >> 16) & 0xff) * scale / 100) as u8,
        (((color >> 8) & 0xff) * scale / 100) as u8,
        ((color & 0xff) * scale / 100) as u8,
    ];
    let mut image = RgbImage::from_pixel(SOURCE_SIZE, SOURCE_SIZE, Rgb(base));
    let cell = 20_u32;
    let origin_x = (SOURCE_SIZE - 3 * cell) / 2;
    let origin_y = (SOURCE_SIZE - 5 * cell) / 2;
    if !draw_digits {
        let rotated = rotate90(&image);
        return convert_image_with_format(
            ImageFormat {
                mode: ImageMode::JPEG,
                size: (TRANSPORT_SIZE, TRANSPORT_SIZE),
                rotation: ImageRotation::Rot0,
                mirror: ImageMirroring::None,
            },
            DynamicImage::ImageRgb8(rotated),
        )
        .map_err(|error| error.to_string());
    }
    for row in 0..5 {
        for column in 0..3 {
            if DIGITS[index][row * 3 + column] == 0 {
                continue;
            }
            for y in 2..cell - 2 {
                for x in 2..cell - 2 {
                    image.put_pixel(
                        origin_x + column as u32 * cell + x,
                        origin_y + row as u32 * cell + y,
                        Rgb(DIGIT_COLOR),
                    );
                }
            }
        }
    }
    let rotated = rotate90(&image);
    convert_image_with_format(
        ImageFormat {
            mode: ImageMode::JPEG,
            size: (TRANSPORT_SIZE, TRANSPORT_SIZE),
            rotation: ImageRotation::Rot0,
            mirror: ImageMirroring::None,
        },
        DynamicImage::ImageRgb8(rotated),
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_color_tile_is_a_safe_60_by_60_jpeg() {
        let encoded = encode_color_image(0, 0x2076ff, 75, true).unwrap();
        let decoded = image::load_from_memory(&encoded).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (60, 60));
    }

    #[test]
    fn blank_tile_is_black_without_digit_pixels() {
        let encoded = encode_color_image(0, 0, 0, false).unwrap();
        let decoded = image::load_from_memory(&encoded).unwrap().to_rgb8();
        assert!(decoded.pixels().all(|pixel| pixel.0 == [0, 0, 0]));
    }
}
