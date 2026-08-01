use std::fmt;

pub const MAGIC: [u8; 2] = *b"CM";
pub const VERSION: u8 = 1;
pub const MAX_PAYLOAD: usize = 64;
pub const HEADER_BYTES: usize = 8;
pub const CRC_BYTES: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    CodexInputReport = 0x01,
    CodexOutputReport = 0x02,
    Ping = 0x03,
    Status = 0x04,
    Log = 0x05,
}

impl TryFrom<u8> for FrameType {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(Self::CodexInputReport),
            0x02 => Ok(Self::CodexOutputReport),
            0x03 => Ok(Self::Ping),
            0x04 => Ok(Self::Status),
            0x05 => Ok(Self::Log),
            _ => Err(WireError::UnknownFrameType(value)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub sequence: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(
        frame_type: FrameType,
        sequence: u16,
        payload: impl Into<Vec<u8>>,
    ) -> Result<Self, WireError> {
        let payload = payload.into();
        if payload.len() > MAX_PAYLOAD {
            return Err(WireError::PayloadTooLarge(payload.len()));
        }
        Ok(Self {
            frame_type,
            sequence,
            payload,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + self.payload.len() + CRC_BYTES);
        bytes.extend_from_slice(&MAGIC);
        bytes.push(VERSION);
        bytes.push(self.frame_type as u8);
        bytes.extend_from_slice(&self.sequence.to_le_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&self.payload);
        let crc = crc16_ccitt(&bytes);
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WireError {
    PayloadTooLarge(usize),
    UnsupportedVersion(u8),
    UnknownFrameType(u8),
    CrcMismatch { expected: u16, actual: u16 },
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge(length) => {
                write!(formatter, "bridge payload is {length} bytes; maximum is 64")
            }
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported bridge protocol version {version}")
            }
            Self::UnknownFrameType(frame_type) => {
                write!(formatter, "unknown bridge frame type 0x{frame_type:02X}")
            }
            Self::CrcMismatch { expected, actual } => write!(
                formatter,
                "bridge CRC mismatch: expected 0x{expected:04X}, got 0x{actual:04X}"
            ),
        }
    }
}

impl std::error::Error for WireError {}

#[derive(Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Result<Frame, WireError>> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            let Some(magic_offset) = find_magic(&self.buffer) else {
                if self.buffer.last() == Some(&MAGIC[0]) {
                    self.buffer.drain(..self.buffer.len().saturating_sub(1));
                } else {
                    self.buffer.clear();
                }
                break;
            };
            if magic_offset > 0 {
                self.buffer.drain(..magic_offset);
            }
            if self.buffer.len() < HEADER_BYTES {
                break;
            }
            if self.buffer[2] != VERSION {
                let version = self.buffer[2];
                self.buffer.drain(..2);
                frames.push(Err(WireError::UnsupportedVersion(version)));
                continue;
            }
            let payload_length = u16::from_le_bytes([self.buffer[6], self.buffer[7]]) as usize;
            if payload_length > MAX_PAYLOAD {
                self.buffer.drain(..2);
                frames.push(Err(WireError::PayloadTooLarge(payload_length)));
                continue;
            }
            let total = HEADER_BYTES + payload_length + CRC_BYTES;
            if self.buffer.len() < total {
                break;
            }
            let expected = u16::from_le_bytes([
                self.buffer[total - CRC_BYTES],
                self.buffer[total - CRC_BYTES + 1],
            ]);
            let actual = crc16_ccitt(&self.buffer[..total - CRC_BYTES]);
            if actual != expected {
                self.buffer.drain(..1);
                frames.push(Err(WireError::CrcMismatch { expected, actual }));
                continue;
            }
            let frame_type = FrameType::try_from(self.buffer[3]);
            let sequence = u16::from_le_bytes([self.buffer[4], self.buffer[5]]);
            let payload = self.buffer[HEADER_BYTES..HEADER_BYTES + payload_length].to_vec();
            self.buffer.drain(..total);
            frames.push(frame_type.map(|frame_type| Frame {
                frame_type,
                sequence,
                payload,
            }));
        }
        frames
    }
}

pub fn crc16_ccitt(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn find_magic(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|candidate| candidate == MAGIC)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_survives_fragmentation_and_noise() {
        let frame = Frame::new(FrameType::Status, 0x1234, b"ready".to_vec()).unwrap();
        let encoded = frame.encode();
        let mut decoder = FrameDecoder::default();
        assert!(decoder.feed(&[0xff, 0x43]).is_empty());
        assert!(decoder.feed(&encoded[..4]).is_empty());
        let decoded = decoder.feed(&encoded[4..]);
        assert_eq!(decoded, vec![Ok(frame)]);
    }

    #[test]
    fn corrupted_frame_is_rejected_and_next_frame_recovers() {
        let mut bad = Frame::new(FrameType::Ping, 1, Vec::new()).unwrap().encode();
        bad[3] ^= 1;
        let good = Frame::new(FrameType::Status, 2, b"ok".to_vec())
            .unwrap()
            .encode();
        bad.extend_from_slice(&good);

        let decoded = FrameDecoder::default().feed(&bad);
        assert!(decoded.iter().any(Result::is_err));
        assert_eq!(
            decoded.last(),
            Some(&Ok(
                Frame::new(FrameType::Status, 2, b"ok".to_vec()).unwrap()
            ))
        );
    }

    #[test]
    fn protocol_has_a_stable_ping_vector() {
        let encoded = Frame::new(FrameType::Ping, 1, Vec::new()).unwrap().encode();
        assert_eq!(
            encoded,
            [0x43, 0x4d, 0x01, 0x03, 0x01, 0x00, 0x00, 0x00, 0xbb, 0xe5]
        );
    }
}
