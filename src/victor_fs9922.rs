//! Fortune Semiconductor **FS9922-DMM4** — Victor (RuoShui) legacy 86B/C/D USB HID path.
//!
//! Those meters used a USB HID interface (VID 0x1244, PID 0xd237) with obfuscated
//! 14-byte reports from the USB cable. After deobfuscation the payload follows the
//! Fortune Semiconductor FS9922-DMM4 chip format. Read only — modes are set on the meter.
//!
//! **Newer** Victor meters (e.g. 86D, DM1107) do not use this path — they stream a
//! binary serial protocol over an opto-isolated CP2102 link. See `victor_dm1107`.
//! Victor 86E (ES51932 ASCII serial) is a separate line: `victor_es519xx`.
//!
//! ## Sources
//!
//! - <https://sigrok.org/wiki/Victor_protocol>
//! - libsigrok `serial_hid_victor.c` and `fs9922.c`

use crate::multimeter::MeterMode;

pub const VICTOR_VENDOR_ID: u16 = 0x1244;
pub const VICTOR_PRODUCT_ID: u16 = 0xd237;
pub const PACKET_LEN: usize = 14;

const OBFUSCATION: [u8; PACKET_LEN] = *b"jodenxunickxia";
const SHUFFLE: [u8; PACKET_LEN] = [6, 13, 5, 11, 2, 7, 9, 8, 3, 10, 12, 0, 4, 1];

#[derive(Debug, Clone, PartialEq)]
pub struct VictorReading {
    pub value: f64,
    pub mode: MeterMode,
    pub unit: String,
}

#[derive(Debug, Clone, Default)]
struct Fs9922Flags {
    is_nano: bool,
    is_micro: bool,
    is_milli: bool,
    is_kilo: bool,
    is_mega: bool,
    is_volt: bool,
    is_ampere: bool,
    is_ohm: bool,
    is_hertz: bool,
    is_farad: bool,
    is_celsius: bool,
    is_fahrenheit: bool,
    is_beep: bool,
    is_diode: bool,
    is_percent: bool,
    is_ac: bool,
    is_dc: bool,
}

fn bit_reverse(b: u8) -> u8 {
    const REV_NIBBLE: [u8; 16] = [
        0x00, 0x08, 0x04, 0x0c, 0x02, 0x0a, 0x06, 0x0e, 0x01, 0x09, 0x05, 0x0d, 0x03, 0x0b, 0x07,
        0x0f,
    ];
    REV_NIBBLE[(b >> 4) as usize] | (REV_NIBBLE[(b & 0xf) as usize] << 4)
}

/// Undo the Victor USB cable's obfuscation to recover the FS9922 packet.
pub fn deobfuscate(raw: &[u8]) -> Option<[u8; PACKET_LEN]> {
    if raw.len() != PACKET_LEN {
        return None;
    }
    if raw.iter().all(|&b| b == 0) {
        return None;
    }

    let mut out = [0u8; PACKET_LEN];
    for (idx, &byte) in raw.iter().enumerate() {
        let to_idx = PACKET_LEN - 1 - SHUFFLE[idx] as usize;
        out[to_idx] = bit_reverse(byte.wrapping_sub(OBFUSCATION[idx]));
    }
    Some(out)
}

/// Extract the 14-byte Victor payload from a HID read buffer.
pub fn extract_packet(buf: &[u8]) -> Option<&[u8; PACKET_LEN]> {
    match buf.len() {
        PACKET_LEN => Some(buf.try_into().ok()?),
        n if n > PACKET_LEN => {
            // Some platforms prepend a report ID byte.
            if buf[0] == 0 {
                buf.get(1..=PACKET_LEN)?.try_into().ok()
            } else {
                buf.get(..PACKET_LEN)?.try_into().ok()
            }
        }
        _ => None,
    }
}

fn flags_valid(flags: &Fs9922Flags) -> bool {
    let mult = [
        flags.is_nano,
        flags.is_micro,
        flags.is_milli,
        flags.is_kilo,
        flags.is_mega,
    ]
    .into_iter()
    .filter(|&b| b)
    .count();
    if mult > 1 {
        return false;
    }

    let modes = [
        flags.is_percent,
        flags.is_volt,
        flags.is_ampere,
        flags.is_ohm,
        flags.is_hertz,
        flags.is_farad,
        flags.is_celsius,
        flags.is_fahrenheit,
    ]
    .into_iter()
    .filter(|&b| b)
    .count();
    if modes > 1 {
        return false;
    }

    !(flags.is_ac && flags.is_dc || flags.is_celsius && flags.is_fahrenheit)
}

fn parse_flags(buf: &[u8; PACKET_LEN]) -> Fs9922Flags {
    Fs9922Flags {
        is_dc: buf[7] & (1 << 4) != 0,
        is_ac: buf[7] & (1 << 3) != 0,
        is_nano: buf[8] & (1 << 1) != 0,
        is_micro: buf[9] & (1 << 7) != 0,
        is_milli: buf[9] & (1 << 6) != 0,
        is_kilo: buf[9] & (1 << 5) != 0,
        is_mega: buf[9] & (1 << 4) != 0,
        is_beep: buf[9] & (1 << 3) != 0,
        is_diode: buf[9] & (1 << 2) != 0,
        is_percent: buf[9] & (1 << 1) != 0,
        is_volt: buf[10] & (1 << 7) != 0,
        is_ampere: buf[10] & (1 << 6) != 0,
        is_ohm: buf[10] & (1 << 5) != 0,
        is_hertz: buf[10] & (1 << 3) != 0,
        is_farad: buf[10] & (1 << 2) != 0,
        is_celsius: buf[10] & (1 << 1) != 0,
        is_fahrenheit: buf[10] & (1 << 0) != 0,
    }
}

fn parse_value(buf: &[u8; PACKET_LEN]) -> Option<(f64, i32)> {
    let sign = match buf[0] {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };

    if buf[1] == b'?' && buf[2] == b'0' && buf[3] == b':' && buf[4] == b'?' {
        return Some((f64::INFINITY, 0));
    }

    if !buf[1].is_ascii_digit()
        || !buf[2].is_ascii_digit()
        || !buf[3].is_ascii_digit()
        || !buf[4].is_ascii_digit()
    {
        return None;
    }

    let intval = (buf[1] - b'0') as i32 * 1000
        + (buf[2] - b'0') as i32 * 100
        + (buf[3] - b'0') as i32 * 10
        + (buf[4] - b'0') as i32;

    let exponent = match buf[6] {
        b'0' => 0,
        b'1' => -3,
        b'2' => -2,
        b'4' => -1,
        _ => return None,
    };

    Some((sign * intval as f64, exponent))
}

fn apply_multipliers(value: f64, exponent: i32, flags: &Fs9922Flags) -> f64 {
    let mut exp = exponent;
    if flags.is_nano {
        exp -= 9;
    }
    if flags.is_micro {
        exp -= 6;
    }
    if flags.is_milli {
        exp -= 3;
    }
    if flags.is_kilo {
        exp += 3;
    }
    if flags.is_mega {
        exp += 6;
    }
    value * 10f64.powi(exp)
}

fn mode_from_flags(flags: &Fs9922Flags) -> Option<(MeterMode, &'static str)> {
    if flags.is_beep {
        return Some((MeterMode::Cont, "Ohm"));
    }
    if flags.is_diode {
        return Some((MeterMode::Diod, "V"));
    }
    if flags.is_percent {
        return Some((MeterMode::Duty, "%"));
    }
    if flags.is_volt {
        if flags.is_ac {
            return Some((MeterMode::Vac, "VAC"));
        }
        return Some((MeterMode::Vdc, "VDC"));
    }
    if flags.is_ampere {
        if flags.is_ac {
            return Some((MeterMode::Aac, "AAC"));
        }
        return Some((MeterMode::Adc, "ADC"));
    }
    if flags.is_ohm {
        return Some((MeterMode::Res, "Ohm"));
    }
    if flags.is_farad {
        return Some((MeterMode::Cap, "F"));
    }
    if flags.is_hertz {
        return Some((MeterMode::Freq, "Hz"));
    }
    if flags.is_celsius || flags.is_fahrenheit {
        return Some((MeterMode::Temp, "°C"));
    }
    None
}

/// Parse a deobfuscated FS9922 packet into a measurement reading.
pub fn parse_packet(buf: &[u8; PACKET_LEN]) -> Option<VictorReading> {
    if buf[12] != b'\r' || buf[13] != b'\n' {
        return None;
    }

    let flags = parse_flags(buf);
    if !flags_valid(&flags) {
        return None;
    }

    let (mut value, exponent) = parse_value(buf)?;
    value = apply_multipliers(value, exponent, &flags);

    if flags.is_beep {
        value = if value.is_infinite() { 0.0 } else { 1.0 };
    }

    let (mode, unit) = mode_from_flags(&flags)?;

    Some(VictorReading {
        value,
        mode,
        unit: unit.to_owned(),
    })
}

/// Process a raw HID read buffer end-to-end.
pub fn parse_hid_buffer(buf: &[u8]) -> Option<VictorReading> {
    let packet = extract_packet(buf)?;
    let deobfuscated = deobfuscate(packet)?;
    parse_packet(&deobfuscated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_all_zero_packet() {
        assert!(deobfuscate(&[0u8; PACKET_LEN]).is_none());
    }

    #[test]
    fn rejects_invalid_fs9922_footer() {
        let mut pkt = [0u8; PACKET_LEN];
        pkt[0] = b'+';
        pkt[1] = b'1';
        pkt[2] = b'2';
        pkt[3] = b'3';
        pkt[4] = b'4';
        pkt[6] = b'4';
        pkt[12] = b'X';
        pkt[13] = b'Y';
        assert!(parse_packet(&pkt).is_none());
    }

    #[test]
    fn parses_duty_cycle_reading() {
        let mut pkt = [0u8; PACKET_LEN];
        pkt[0] = b'+';
        pkt[1] = b'5';
        pkt[2] = b'2';
        pkt[3] = b'3';
        pkt[4] = b'0';
        pkt[6] = b'2'; // two decimal places -> 52.30
        pkt[9] = 1 << 1; // duty cycle (%)
        pkt[12] = b'\r';
        pkt[13] = b'\n';

        let reading = parse_packet(&pkt).unwrap();
        assert!(
            (reading.value - 52.3).abs() < 1e-6,
            "expected 52.3, got {}",
            reading.value
        );
        assert_eq!(reading.mode, MeterMode::Duty);
        assert_eq!(reading.unit, "%");
    }

    #[test]
    fn parses_simple_voltage_reading() {
        let mut pkt = [0u8; PACKET_LEN];
        pkt[0] = b'+';
        pkt[1] = b'1';
        pkt[2] = b'2';
        pkt[3] = b'3';
        pkt[4] = b'4';
        pkt[6] = b'4'; // one decimal place -> 123.4
        pkt[7] = 1 << 4; // DC
        pkt[10] = 1 << 7; // volt
        pkt[12] = b'\r';
        pkt[13] = b'\n';

        let reading = parse_packet(&pkt).unwrap();
        assert!((reading.value - 123.4).abs() < f64::EPSILON);
        assert_eq!(reading.mode, MeterMode::Vdc);
        assert_eq!(reading.unit, "VDC");
    }
}
