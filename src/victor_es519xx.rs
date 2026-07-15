//! Cyrustek ES51932 (Victor / RuoShui 86E) serial protocol parser.
//!
//! The 86E uses a CP2102 USB-UART bridge and ES51932 ASCII frames (distinct from
//! legacy 86B/C/D HID meters and from newer DM1107 serial meters such as the 86D).
//! It streams read-only measurement frames; modes cannot be changed remotely.
//!
//! ## Serial settings
//!
//! - 19200 baud, 7 data bits, odd parity, 1 stop bit (7o1)
//! - Linux example: `stty -F /dev/ttyUSB0 19200 cs7 parenb parodd -cstopb raw -echo`
//!
//! ## Frame layout (14 bytes, CR/LF terminated)
//!
//! `[range][digit×5][function][status][opt1][opt2][opt3][opt4][CR][LF]`
//!
//! A line like `103560;000:3` is not human-padded text — the semicolon and colon
//! are literal protocol bytes. For example `;` (0x3b) encodes DC voltage mode.
//!
//! ## Sources
//!
//! - [rusty_meter#16](https://github.com/markusdd/rusty_meter/issues/16) — 86E
//!   captures and serial parameters (Markus Krause / MFJoyBoy)
//! - [EEVblog teardown / ES51932_log](https://www.eevblog.com/forum/testgear/teardown-ruoshui-86e-22000-count-dmm-50-eur/)
//!   — original 14-byte packet description and Perl decoder
//! - [libsigrok `es519xx.c`](https://github.com/sigrokproject/libsigrok/blob/master/src/dmm/es519xx.c)
//!   — range exponents and function-byte mapping (ES51931/ES51932, 19200/14b)

use crate::multimeter::MeterMode;
use crate::victor_fs9922::VictorReading;

pub const VICTOR_86E_BAUD: u32 = 19200;
pub const PACKET_LEN: usize = 14;

/// Exponents for 19200 baud / 14-byte / ES51931/32 packets (from libsigrok).
const EXPONENTS_VOLTAGE: [i32; 8] = [-4, -3, -2, -1, -5, 0, 0, 0];
const EXPONENTS_UA: [i32; 8] = [-8, -7, 0, 0, 0, 0, 0, 0];
const EXPONENTS_MA: [i32; 8] = [-6, -5, 0, 0, 0, 0, 0, 0];
const EXPONENTS_A: [i32; 8] = [-3, 0, 0, 0, 0, 0, 0, 0];
const EXPONENTS_MANUAL_A: [i32; 8] = [-4, -3, -2, -1, 0, 0, 0, 0];
const EXPONENTS_RES: [i32; 8] = [-2, -1, 0, 1, 2, 3, 4, 0];
const EXPONENTS_FREQ: [i32; 8] = [-2, -1, 0, 0, 1, 2, 3, 4];
const EXPONENTS_CAP: [i32; 8] = [-12, -11, -10, -9, -8, -7, -6, -5];
const EXPONENTS_DIODE: [i32; 8] = [-4, 0, 0, 0, 0, 0, 0, 0];

#[derive(Debug, Clone, Default)]
struct Es519xxFlags {
    is_sign: bool,
    is_ol: bool,
    is_ul: bool,
    is_judge: bool,
    is_dc: bool,
    is_ac: bool,
    is_auto: bool,
    is_vahz: bool,
    is_voltage: bool,
    is_current: bool,
    is_micro: bool,
    is_milli: bool,
    is_resistance: bool,
    is_continuity: bool,
    is_diode: bool,
    is_frequency: bool,
    is_duty_cycle: bool,
    is_capacitance: bool,
    is_temperature: bool,
    is_celsius: bool,
    is_fahrenheit: bool,
}

fn parse_flags(buf: &[u8; PACKET_LEN], flags: &mut Es519xxFlags) {
    flags.is_judge = buf[7] & (1 << 3) != 0;
    flags.is_sign = buf[7] & (1 << 2) != 0;
    flags.is_ol = buf[7] & (1 << 0) != 0;

    flags.is_ul = buf[9] & (1 << 3) != 0;

    flags.is_dc = buf[10] & (1 << 3) != 0;
    flags.is_ac = buf[10] & (1 << 2) != 0;
    flags.is_auto = buf[10] & (1 << 1) != 0;
    flags.is_vahz = buf[10] & (1 << 0) != 0;

    match buf[6] {
        0x3b => flags.is_voltage = true, // ';'
        0x3d => {
            flags.is_current = true;
            flags.is_micro = true;
            flags.is_auto = true;
        }
        0x3f => {
            flags.is_current = true;
            flags.is_milli = true;
            flags.is_auto = true;
        }
        0x30 => {
            flags.is_current = true;
            flags.is_auto = true;
        }
        0x39 => {
            flags.is_current = true;
            flags.is_auto = false;
        }
        0x33 => flags.is_resistance = true,
        0x35 => flags.is_continuity = true,
        0x31 => flags.is_diode = true,
        0x32 => {
            if flags.is_judge {
                flags.is_duty_cycle = true;
            } else {
                flags.is_frequency = true;
            }
        }
        0x36 => flags.is_capacitance = true,
        0x34 => {
            flags.is_temperature = true;
            if flags.is_judge {
                flags.is_celsius = true;
            } else {
                flags.is_fahrenheit = true;
            }
        }
        _ => {}
    }

    if flags.is_vahz && (flags.is_voltage || flags.is_current) {
        flags.is_voltage = false;
        flags.is_current = false;
        flags.is_micro = false;
        flags.is_milli = false;
        if flags.is_judge {
            flags.is_duty_cycle = true;
            flags.is_frequency = false;
        } else {
            flags.is_frequency = true;
            flags.is_duty_cycle = false;
        }
    }
}

fn flags_valid(flags: &Es519xxFlags) -> bool {
    let mult = [flags.is_micro, flags.is_milli]
        .into_iter()
        .filter(|&b| b)
        .count();
    if mult > 1 {
        return false;
    }

    let modes = [
        flags.is_voltage,
        flags.is_current,
        flags.is_resistance,
        flags.is_frequency,
        flags.is_duty_cycle,
        flags.is_capacitance,
        flags.is_temperature,
        flags.is_continuity,
        flags.is_diode,
    ]
    .into_iter()
    .filter(|&b| b)
    .count();
    if modes > 1 {
        return false;
    }

    !(flags.is_ac && flags.is_dc)
}

fn parse_value(buf: &[u8; PACKET_LEN], flags: &Es519xxFlags) -> Option<f64> {
    if flags.is_ol || flags.is_ul {
        return Some(f64::INFINITY);
    }

    if !buf[1].is_ascii_digit()
        || !buf[2].is_ascii_digit()
        || !buf[3].is_ascii_digit()
        || !buf[4].is_ascii_digit()
        || !buf[5].is_ascii_digit()
    {
        return None;
    }

    let mut intval = 0i64;
    for &digit in &buf[1..=5] {
        intval = intval * 10 + (digit - b'0') as i64;
    }
    if flags.is_sign {
        intval = -intval;
    }
    Some(intval as f64)
}

fn apply_range(buf: &[u8; PACKET_LEN], flags: &Es519xxFlags, value: f64) -> Option<f64> {
    let idx = (buf[0] as i32).saturating_sub(b'0' as i32);
    if !(0..=7).contains(&idx) {
        return None;
    }
    let idx = idx as usize;

    let exponent = if flags.is_duty_cycle {
        -1
    } else if flags.is_voltage {
        EXPONENTS_VOLTAGE[idx]
    } else if flags.is_current && flags.is_micro {
        EXPONENTS_UA[idx]
    } else if flags.is_current && flags.is_milli {
        EXPONENTS_MA[idx]
    } else if flags.is_current && flags.is_auto {
        EXPONENTS_A[idx]
    } else if flags.is_current {
        EXPONENTS_MANUAL_A[idx]
    } else if flags.is_resistance || flags.is_continuity {
        EXPONENTS_RES[idx]
    } else if flags.is_frequency {
        EXPONENTS_FREQ[idx]
    } else if flags.is_capacitance {
        EXPONENTS_CAP[idx]
    } else if flags.is_diode {
        EXPONENTS_DIODE[idx]
    } else if flags.is_temperature {
        EXPONENTS_RES[idx]
    } else {
        return None;
    };

    Some(value * 10f64.powi(exponent))
}

fn mode_from_flags(flags: &Es519xxFlags) -> Option<(MeterMode, &'static str)> {
    if flags.is_continuity {
        return Some((MeterMode::Cont, "Ohm"));
    }
    if flags.is_diode {
        return Some((MeterMode::Diod, "V"));
    }
    if flags.is_duty_cycle {
        return Some((MeterMode::Duty, "%"));
    }
    if flags.is_voltage {
        if flags.is_ac {
            return Some((MeterMode::Vac, "VAC"));
        }
        return Some((MeterMode::Vdc, "VDC"));
    }
    if flags.is_current {
        if flags.is_ac {
            return Some((MeterMode::Aac, "AAC"));
        }
        return Some((MeterMode::Adc, "ADC"));
    }
    if flags.is_resistance {
        return Some((MeterMode::Res, "Ohm"));
    }
    if flags.is_capacitance {
        return Some((MeterMode::Cap, "F"));
    }
    if flags.is_frequency {
        return Some((MeterMode::Freq, "Hz"));
    }
    if flags.is_temperature {
        if flags.is_fahrenheit {
            return Some((MeterMode::Temp, "°F"));
        }
        return Some((MeterMode::Temp, "°C"));
    }
    None
}

/// Parse a 14-byte Victor 86E / ES51932 serial frame.
pub fn parse_packet(buf: &[u8; PACKET_LEN]) -> Option<VictorReading> {
    if buf[12] != b'\r' || buf[13] != b'\n' {
        return None;
    }

    let mut flags = Es519xxFlags::default();
    parse_flags(buf, &mut flags);
    if !flags_valid(&flags) {
        return None;
    }

    let mut value = parse_value(buf, &flags)?;
    value = apply_range(buf, &flags, value)?;

    if flags.is_continuity {
        value = if value.is_infinite() || !(0.0..=25.0).contains(&value) {
            0.0
        } else {
            1.0
        };
    }

    let (mode, unit) = mode_from_flags(&flags)?;

    Some(VictorReading {
        value,
        mode,
        unit: unit.to_owned(),
    })
}

/// Append incoming serial bytes and return any complete parsed frames.
pub fn feed_bytes(buffer: &mut Vec<u8>, chunk: &[u8]) -> Vec<VictorReading> {
    buffer.extend_from_slice(chunk);
    let mut readings = Vec::new();

    loop {
        let Some(pos) = buffer.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        if pos + 2 < PACKET_LEN {
            buffer.drain(..pos + 2);
            continue;
        }
        let start = pos + 2 - PACKET_LEN;
        if start > pos {
            buffer.drain(..pos + 2);
            continue;
        }
        let frame: [u8; PACKET_LEN] = buffer[start..=pos + 1].try_into().ok().unwrap();
        buffer.drain(..pos + 2);
        if let Some(reading) = parse_packet(&frame) {
            readings.push(reading);
        }
    }

    if buffer.len() > PACKET_LEN * 4 {
        let keep = PACKET_LEN * 2;
        let drain_to = buffer.len().saturating_sub(keep);
        buffer.drain(..drain_to);
    }

    readings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(s: &str) -> [u8; PACKET_LEN] {
        let bytes = s.as_bytes();
        assert_eq!(bytes.len(), PACKET_LEN);
        bytes.try_into().unwrap()
    }

    #[test]
    fn parses_86e_dc_voltage_sample() {
        // From issue #16: ~3.6 V battery in DC mode.
        let reading = parse_packet(&frame("103560;000:3\r\n")).unwrap();
        assert!((reading.value - 3.56).abs() < 1e-6, "got {}", reading.value);
        assert_eq!(reading.mode, MeterMode::Vdc);
        assert_eq!(reading.unit, "VDC");
    }

    #[test]
    fn rejects_incomplete_footer() {
        let mut buf = frame("103560;000:3\r\n");
        buf[13] = b'X';
        assert!(parse_packet(&buf).is_none());
    }
}
