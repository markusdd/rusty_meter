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

use crate::helpers::METER_OVERLOAD_VALUE;
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
            // Victor 86E (user report #16): judge set → °F, clear → °C
            // (opposite of some ES519xx / libsigrok notes).
            if flags.is_judge {
                flags.is_fahrenheit = true;
            } else {
                flags.is_celsius = true;
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
    // Never return Infinity — graph/histogram binning panics on non-finite samples.
    if flags.is_ol || flags.is_ul {
        return Some(METER_OVERLOAD_VALUE);
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

fn range_exponent(buf: &[u8; PACKET_LEN], flags: &Es519xxFlags) -> Option<i32> {
    let idx = (buf[0] as i32).saturating_sub(b'0' as i32);
    if !(0..=7).contains(&idx) {
        return None;
    }
    let idx = idx as usize;

    // Duty / temperature: fixed one-decimal scale (not range tables).
    // Temperature digits are always °C on the wire (libsigrok).
    Some(if flags.is_duty_cycle || flags.is_temperature {
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
    } else {
        return None;
    })
}

fn apply_range(buf: &[u8; PACKET_LEN], flags: &Es519xxFlags, value: f64) -> Option<f64> {
    if value == METER_OVERLOAD_VALUE {
        return Some(value);
    }

    let exponent = range_exponent(buf, flags)?;
    let mut scaled = value * 10f64.powi(exponent);

    // Wire digits are always °C; convert only when unit is Fahrenheit.
    if flags.is_temperature && flags.is_fahrenheit {
        scaled = scaled * 9.0 / 5.0 + 32.0;
    }

    Some(scaled)
}

/// Mode + unit as on the meter for this range (not SI magnitude auto-pick).
fn mode_and_unit(flags: &Es519xxFlags, exp: i32) -> Option<(MeterMode, &'static str)> {
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
        // Only the finest voltage ranges are mV (exp -4 / -5); exp -3 is still volts.
        if exp <= -4 {
            return Some(if flags.is_ac {
                (MeterMode::Vac, "mVAC")
            } else {
                (MeterMode::Vdc, "mVDC")
            });
        }
        return Some(if flags.is_ac {
            (MeterMode::Vac, "VAC")
        } else {
            (MeterMode::Vdc, "VDC")
        });
    }
    if flags.is_current {
        if flags.is_micro || exp <= -6 {
            return Some(if flags.is_ac {
                (MeterMode::Aac, "uAAC")
            } else {
                (MeterMode::Adc, "uADC")
            });
        }
        if flags.is_milli || exp <= -3 {
            return Some(if flags.is_ac {
                (MeterMode::Aac, "mAAC")
            } else {
                (MeterMode::Adc, "mADC")
            });
        }
        return Some(if flags.is_ac {
            (MeterMode::Aac, "AAC")
        } else {
            (MeterMode::Adc, "ADC")
        });
    }
    if flags.is_resistance {
        // Range exponent selects the meter’s unit ladder (e.g. manual kΩ).
        let unit = if exp >= 6 {
            "MOhm"
        } else if exp >= 3 {
            "kOhm"
        } else if exp <= -3 {
            "mOhm"
        } else {
            "Ohm"
        };
        return Some((MeterMode::Res, unit));
    }
    if flags.is_capacitance {
        let unit = match exp {
            -12 | -11 => "pF",
            -10 => "nF",
            -9..=-7 => "uF",
            -6 | -5 => "mF",
            _ => "F",
        };
        return Some((MeterMode::Cap, unit));
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

/// Scale SI `value` into the unit string produced by [`mode_and_unit`].
pub fn si_to_meter_unit(value_si: f64, unit: &str) -> f64 {
    match unit {
        "kOhm" => value_si / 1_000.0,
        "MOhm" => value_si / 1_000_000.0,
        "mOhm" => value_si * 1_000.0,
        "mVDC" | "mVAC" | "mADC" | "mAAC" => value_si * 1_000.0,
        "uADC" | "uAAC" | "µADC" | "µAAC" => value_si * 1_000_000.0,
        "uF" | "µF" | "μF" => value_si * 1_000_000.0,
        "nF" => value_si * 1_000_000_000.0,
        "pF" => value_si * 1_000_000_000_000.0,
        "mF" => value_si * 1_000.0,
        _ => value_si,
    }
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
    let exp = range_exponent(buf, &flags)?;
    value = apply_range(buf, &flags, value)?;

    let (mode, unit) = mode_and_unit(&flags, exp)?;

    // Contract: never emit non-finite values to the UI / graph path.
    if !value.is_finite() {
        value = METER_OVERLOAD_VALUE;
    }

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
        let end = pos + 2;
        let Some(frame_slice) = buffer.get(start..end) else {
            buffer.drain(..end);
            continue;
        };
        let Ok(frame) = <[u8; PACKET_LEN]>::try_from(frame_slice) else {
            buffer.drain(..end);
            continue;
        };
        buffer.drain(..end);
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

    #[test]
    fn temperature_celsius_one_decimal() {
        // Digits 00316 + exp -1 → 31.6 °C. Judge clear → °C on Victor 86E.
        let mut raw = frame("0003164000:3\r\n");
        raw[7] = 0x00;
        let reading = parse_packet(&raw).unwrap();
        assert!((reading.value - 31.6).abs() < 1e-6, "got {}", reading.value);
        assert_eq!(reading.mode, MeterMode::Temp);
        assert_eq!(reading.unit, "°C");
    }

    #[test]
    fn temperature_fahrenheit_converts_from_celsius_digits() {
        // Judge set → °F on 86E; wire digits still °C → 31.6 * 9/5 + 32 = 88.88.
        let mut raw = frame("0003164000:3\r\n");
        raw[7] = 0x08;
        let reading = parse_packet(&raw).unwrap();
        assert!(
            (reading.value - 88.88).abs() < 1e-3,
            "got {}",
            reading.value
        );
        assert_eq!(reading.mode, MeterMode::Temp);
        assert_eq!(reading.unit, "°F");
    }

    #[test]
    fn capacitance_microfarad_range() {
        // 4.885 µF → SI 4.885e-6 F; unit reflects meter range (uF).
        let reading = parse_packet(&frame("3048856000:3\r\n")).unwrap();
        assert!(
            (reading.value - 4.885e-6).abs() < 1e-12,
            "got {}",
            reading.value
        );
        assert_eq!(reading.mode, MeterMode::Cap);
        assert_eq!(reading.unit, "uF");
        assert!((si_to_meter_unit(reading.value, "uF") - 4.885).abs() < 1e-9);
    }

    #[test]
    fn resistance_range_selects_kohm_unit() {
        // RES range index 5 → exp +3; digits 00047 → 47_000 Ω SI, unit kOhm.
        let reading = parse_packet(&frame("5000473000:3\r\n")).unwrap();
        assert!(
            (reading.value - 47_000.0).abs() < 1e-6,
            "got {}",
            reading.value
        );
        assert_eq!(reading.mode, MeterMode::Res);
        assert_eq!(reading.unit, "kOhm");
        assert!((si_to_meter_unit(reading.value, "kOhm") - 47.0).abs() < 1e-9);
    }

    #[test]
    fn overload_is_finite_sentinel_not_infinity() {
        // RES function '3', OL status bit 0 — must stay finite (histogram crash).
        let mut raw = frame("0000003000:3\r\n");
        raw[7] = 0x01;
        let reading = parse_packet(&raw).unwrap();
        assert!(reading.value.is_finite());
        assert_eq!(reading.value, METER_OVERLOAD_VALUE);
        assert_eq!(reading.mode, MeterMode::Res);
    }

    #[test]
    fn feed_bytes_does_not_panic_on_short_crlf() {
        let mut buf = Vec::new();
        let readings = feed_bytes(&mut buf, b"\r\n103560;000:3\r\n");
        assert_eq!(readings.len(), 1);
        assert!((readings[0].value - 3.56).abs() < 1e-6);
    }
}
