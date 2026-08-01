use crate::wire::{Frame, FrameDecoder};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

pub enum SerialEvent {
    Frame(Frame),
    ProtocolError(String),
    Disconnected(String),
}

pub fn open(port: &str) -> Result<File, String> {
    let path = if port.starts_with(r"\\.\") {
        port.to_owned()
    } else {
        format!(r"\\.\{port}")
    };
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("could not open RP2040 bridge port {port}: {error}"))?;
    set_timeouts(&file)?;
    set_dtr(&file)?;
    Ok(file)
}

pub fn start_reader(mut reader: File, sender: Sender<SerialEvent>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rp2040-cdc-reader".to_owned())
        .spawn(move || {
            let mut decoder = FrameDecoder::default();
            let mut buffer = [0_u8; 256];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => continue,
                    Ok(read) => {
                        for frame in decoder.feed(&buffer[..read]) {
                            let event = match frame {
                                Ok(frame) => SerialEvent::Frame(frame),
                                Err(error) => SerialEvent::ProtocolError(error.to_string()),
                            };
                            if sender.send(event).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(SerialEvent::Disconnected(format!(
                            "RP2040 bridge read failed: {error}"
                        )));
                        break;
                    }
                }
            }
        })
        .expect("serial reader thread should start")
}

pub fn write_frame(writer: &mut File, frame: &Frame) -> Result<(), String> {
    writer
        .write_all(&frame.encode())
        .and_then(|_| writer.flush())
        .map_err(|error| format!("RP2040 bridge write failed: {error}"))
}

#[cfg(windows)]
#[repr(C)]
struct CommTimeouts {
    read_interval_timeout: u32,
    read_total_timeout_multiplier: u32,
    read_total_timeout_constant: u32,
    write_total_timeout_multiplier: u32,
    write_total_timeout_constant: u32,
}

#[cfg(windows)]
fn set_timeouts(file: &File) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetCommTimeouts(file: *mut core::ffi::c_void, timeouts: *const CommTimeouts) -> i32;
    }
    let timeouts = CommTimeouts {
        read_interval_timeout: u32::MAX,
        read_total_timeout_multiplier: u32::MAX,
        read_total_timeout_constant: 50,
        write_total_timeout_multiplier: 0,
        write_total_timeout_constant: 1000,
    };
    let result = unsafe { SetCommTimeouts(file.as_raw_handle().cast(), &raw const timeouts) };
    if result == 0 {
        return Err(format!(
            "could not set timeouts on RP2040 bridge port: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_timeouts(_file: &File) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn set_dtr(file: &File) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;

    const SETDTR: u32 = 5;
    const SETRTS: u32 = 4;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn EscapeCommFunction(file: *mut core::ffi::c_void, function: u32) -> i32;
    }
    // Assert both DTR and RTS to signal CDC connection
    let result = unsafe { EscapeCommFunction(file.as_raw_handle().cast(), SETDTR) };
    if result == 0 {
        return Err(format!(
            "could not assert DTR on RP2040 bridge port: {}",
            std::io::Error::last_os_error()
        ));
    }
    let result = unsafe { EscapeCommFunction(file.as_raw_handle().cast(), SETRTS) };
    if result == 0 {
        return Err(format!(
            "could not assert RTS on RP2040 bridge port: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn set_dtr(_file: &File) -> Result<(), String> {
    Ok(())
}
