use ajazz_sdk::{ImageFormat, ImageMirroring, ImageMode, ImageRotation, convert_image_with_format};
use hidapi::{HidApi, HidDevice};
use image::{DynamicImage, Rgb, RgbImage, imageops::rotate90};
use serde_json::json;
use std::{
    collections::HashSet,
    env,
    error::Error,
    thread,
    time::{Duration, Instant},
};

const DISPLAY_KEY_COUNT: u8 = 6;
const SOURCE_SIZE: u32 = 126;
const TRANSPORT_SIZE: usize = 60;
const OUTPUT_REPORT_LENGTH: usize = 1025;
const INPUT_REPORT_LENGTH: usize = 513;

const COLORS: [[u8; 3]; 6] = [
    [32, 118, 255],
    [19, 190, 145],
    [255, 173, 38],
    [239, 77, 99],
    [139, 92, 246],
    [25, 201, 230],
];

const DIGITS: [[u8; 15]; 6] = [
    [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 1, 0, 0, 1, 1, 1],
    [1, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0],
    [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1],
    [1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0],
    [0, 1, 1, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0, 1, 0],
];

fn listen_seconds() -> Result<u64, Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut seconds = 45;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                seconds = args.next().ok_or("--listen requires seconds")?.parse()?;
                if !(1..=180).contains(&seconds) {
                    return Err("--listen must be from 1 to 180 seconds".into());
                }
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    Ok(seconds)
}

fn test_image(index: usize) -> RgbImage {
    let base = COLORS[index];
    let mut image = RgbImage::from_fn(SOURCE_SIZE, SOURCE_SIZE, |x, y| {
        let shade = ((x + y) * 36 / (SOURCE_SIZE * 2)) as u8;
        Rgb([
            base[0].saturating_add(shade),
            base[1].saturating_add(shade),
            base[2].saturating_add(shade),
        ])
    });

    for y in 7..13 {
        for x in 7..119 {
            image.put_pixel(x, y, Rgb([255, 255, 255]));
        }
    }
    for y in 113..119 {
        for x in 7..119 {
            image.put_pixel(x, y, Rgb([255, 255, 255]));
        }
    }

    let scale = 22;
    let origin_x = (SOURCE_SIZE - 3 * scale) / 2;
    let origin_y = (SOURCE_SIZE - 5 * scale) / 2;
    for row in 0..5 {
        for column in 0..3 {
            if DIGITS[index][row * 3 + column] == 0 {
                continue;
            }
            for y in 2..(scale - 2) {
                for x in 2..(scale - 2) {
                    image.put_pixel(
                        origin_x + column as u32 * scale + x,
                        origin_y + row as u32 * scale + y,
                        Rgb([255, 255, 255]),
                    );
                }
            }
        }
    }
    image
}

fn encode_image(index: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let rotated = rotate90(&test_image(index));
    Ok(convert_image_with_format(
        ImageFormat {
            mode: ImageMode::JPEG,
            size: (TRANSPORT_SIZE, TRANSPORT_SIZE),
            rotation: ImageRotation::Rot0,
            mirror: ImageMirroring::None,
        },
        DynamicImage::ImageRgb8(rotated),
    )?)
}

fn command_packet(command: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x00, 0x43, 0x52, 0x54, 0x00, 0x00];
    packet.extend_from_slice(command);
    packet.resize(OUTPUT_REPORT_LENGTH, 0);
    packet
}

fn write_packet(device: &HidDevice, packet: &[u8]) -> Result<(), Box<dyn Error>> {
    let written = device.write(packet)?;
    if written != packet.len() {
        return Err(format!("short HID write: {written}/{} bytes", packet.len()).into());
    }
    Ok(())
}

fn write_image(device: &HidDevice, key: u8, image: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut announce = vec![0x42, 0x41, 0x54, 0x00, 0x00];
    announce.push((image.len() >> 8) as u8);
    announce.push(image.len() as u8);
    announce.push(key + 1);
    write_packet(device, &command_packet(&announce))?;

    for chunk in image.chunks(OUTPUT_REPORT_LENGTH - 1) {
        let mut packet = Vec::with_capacity(OUTPUT_REPORT_LENGTH);
        packet.push(0x00);
        packet.extend_from_slice(chunk);
        packet.resize(OUTPUT_REPORT_LENGTH, 0);
        write_packet(device, &packet)?;
    }
    Ok(())
}

fn event_from_code(
    code: u8,
    pressed_buttons: &mut HashSet<u8>,
    pressed_encoders: &mut [bool; 3],
) -> Vec<serde_json::Value> {
    match code {
        0 => pressed_buttons
            .drain()
            .map(|index| json!({"type": "button-up", "index": index}))
            .collect(),
        1..=6 | 0x25 | 0x30 | 0x31 => {
            let index = match code {
                1..=6 => code - 1,
                0x25 => 6,
                0x30 => 7,
                0x31 => 8,
                _ => unreachable!(),
            };
            if pressed_buttons.insert(index) {
                vec![json!({"type": "button-down", "index": index})]
            } else {
                pressed_buttons.remove(&index);
                vec![json!({"type": "button-up", "index": index})]
            }
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
            vec![json!({"type": "encoder-twist", "index": index, "delta": delta})]
        }
        0x33..=0x35 => {
            let index = match code {
                0x33 => 0,
                0x35 => 1,
                0x34 => 2,
                _ => unreachable!(),
            };
            pressed_encoders[index] = !pressed_encoders[index];
            vec![json!({
                "type": if pressed_encoders[index] { "encoder-down" } else { "encoder-up" },
                "index": index
            })]
        }
        _ => vec![],
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let seconds = listen_seconds()?;
    let api = HidApi::new()?;
    let info = api
        .device_list()
        .find(|device| {
            device.vendor_id() == 0x0300
                && device.product_id() == 0x3002
                && device.usage_page() == 0xffa0
                && device.usage() == 0x0001
        })
        .ok_or("AKP03E rev. 2 vendor interface (FFA0:0001) not found")?;
    let path = info.path().to_string_lossy().into_owned();
    let device = api.open_path(info.path())?;

    write_packet(&device, &command_packet(&[0x44, 0x49, 0x53, 0x00, 0x00]))?;
    write_packet(
        &device,
        &command_packet(&[0x4c, 0x49, 0x47, 0x00, 0x00, 75]),
    )?;
    for key in 0..DISPLAY_KEY_COUNT {
        write_image(&device, key, &encode_image(key as usize)?)?;
    }
    write_packet(&device, &command_packet(&[0x53, 0x54, 0x50]))?;

    println!(
        "{}",
        json!({
            "type": "connected",
            "vid": "0300",
            "pid": "3002",
            "model": "AKP03E rev. 2",
            "displayTest": "six numbered color tiles",
            "interface": "FFA0:0001",
            "path": path,
            "listenSeconds": seconds
        })
    );

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut pressed_buttons = HashSet::<u8>::new();
    let mut pressed_encoders = [false; 3];
    let mut event_count = 0_u64;

    while Instant::now() < deadline {
        let mut report = vec![0u8; INPUT_REPORT_LENGTH];
        let read = device.read_timeout(&mut report, 100)?;
        if read == 0 {
            continue;
        }
        report.truncate(read);
        let header_offset = if report.starts_with(&[0x41, 0x43, 0x4b]) {
            0
        } else if report.get(1..4) == Some(&[0x41, 0x43, 0x4b]) {
            1
        } else {
            let raw = report
                .iter()
                .take(24)
                .map(|byte| format!("{byte:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("{}", json!({"type": "unknown-report", "raw": raw}));
            continue;
        };
        let events = report
            .get(header_offset + 9)
            .map(|code| event_from_code(*code, &mut pressed_buttons, &mut pressed_encoders))
            .unwrap_or_default();
        for event in events {
            event_count += 1;
            println!("{event}");
        }
        thread::yield_now();
    }

    println!("{}", json!({"type": "complete", "eventCount": event_count}));
    Ok(())
}
