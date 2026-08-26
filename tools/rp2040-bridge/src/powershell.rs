//! Helpers for passing values into PowerShell scripts driven through stdin.
//!
//! `powershell.exe -Command -` decodes piped input with the legacy ANSI
//! codepage, not UTF-8, so non-ASCII text (task titles routinely carry
//! accents and ellipses) would reach the script mojibake'd and never match
//! the Unicode names the UI automation tree reports. Such values are passed
//! as base64 of their UTF-8 bytes instead, which stays ASCII on the wire,
//! and the script decodes them on the first line.

/// Builds a PowerShell expression that evaluates to `value` without relying
/// on the console codepage.
pub fn unicode_literal(value: &str) -> String {
    format!(
        "[System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String('{}'))",
        base64_encode(value.as_bytes())
    )
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let group = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        encoded.push(ALPHABET[(group >> 18 & 0x3f) as usize] as char);
        encoded.push(ALPHABET[(group >> 12 & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(group >> 6 & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(group & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{base64_encode, unicode_literal};

    #[test]
    fn encodes_standard_base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode("…".as_bytes()), "4oCm");
    }

    #[test]
    fn unicode_literals_stay_ascii_regardless_of_input() {
        let literal = unicode_literal("Depurar exportación WebP …");
        assert!(literal.is_ascii());
        assert!(literal.starts_with("[System.Text.Encoding]::UTF8.GetString"));
        assert!(literal.contains("FromBase64String('RGVw"));
    }
}
