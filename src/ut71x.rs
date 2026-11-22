//! UNI-T UT71A–E / UT804 serial frames.
//!
//! 2400 baud, 7 data bits, odd parity, 1 stop. 11-byte frames ending in CR LF.
//! The link is read-only. Values are SI units except where noted.

use crate::helpers::METER_OVERLOAD_VALUE;
use crate::multimeter::MeterMode;

pub const BAUD: u32 = 2400;
pub const PACKET_LEN: usize = 11;

/// Index is the function byte, then the range byte.
const EXPONENTS: [[i32; 8]; 16] = [
    [-5, 0, 0, 0, 0, 0, 0, 0],          // AC mV
    [0, -4, -3, -2, -1, 0, 0, 0],       // DC V
    [0, -4, -3, -2, -1, 0, 0, 0],       // AC V
    [-5, 0, 0, 0, 0, 0, 0, 0],          // DC mV
    [0, -2, -1, 0, 1, 2, 3, 0],         // resistance
    [0, -12, -11, -10, -9, -8, -7, -6], // capacitance
    [-1, 0, 0, 0, 0, 0, 0, 0],          // temp C
    [-8, -7, 0, 0, 0, 0, 0, 0],         // µA
    [-6, -5, 0, 0, 0, 0, 0, 0],         // mA
    [0, -3, 0, 0, 0, 0, 0, 0],          // 10 A
    [-1, 0, 0, 0, 0, 0, 0, 0],          // continuity
    [-4, 0, 0, 0, 0, 0, 0, 0],          // diode
    [-3, -2, -1, 0, 1, 2, 3, 4],        // frequency
    [-1, 0, 0, 0, 0, 0, 0, 0],          // temp F
    [0, 0, 0, 0, 0, 0, 0, 0],           // power (UT71E)
    [-2, 0, 0, 0, 0, 0, 0, 0],          // 4–20 mA loop display
];

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub value: f64,
    pub mode: MeterMode,
    pub unit: String,
}

#[derive(Default)]
struct Flags {
    voltage: bool,
    current: bool,
    resistance: bool,
    capacitance: bool,
    frequency: bool,
    temperature: bool,
    continuity: bool,
    diode: bool,
    power: bool,
    loop_current: bool,
    celsius: bool,
    fahrenheit: bool,
    ac: bool,
    dc: bool,
    auto: bool,
    manual: bool,
    sign: bool,
    duty: bool,
}

pub fn parse_packet(buf: &[u8; PACKET_LEN]) -> Option<Reading> {
    if buf[9] != b'\r' || buf[10] != b'\n' {
        return None;
    }
    let flags = parse_flags(buf)?;
    let mut value = parse_digits(buf, flags.sign)?;
    let exp = exponent(buf)?;
    value *= 10f64.powi(exp);
    if !value.is_finite() {
        value = METER_OVERLOAD_VALUE;
    }
    let (mode, unit) = mode_and_unit(&flags, value);
    // Loop-current frames carry the meter's displayed number. Show that number
    // as milliamps on the DC-current mode.
    if flags.loop_current {
        value *= 1e-3;
    }
    Some(Reading {
        value,
        mode,
        unit: unit.to_owned(),
    })
}

pub fn feed_bytes(buffer: &mut Vec<u8>, chunk: &[u8]) -> Vec<Reading> {
    buffer.extend_from_slice(chunk);
    let mut readings = Vec::new();
    while let Some(pos) = buffer.windows(2).position(|w| w == b"\r\n") {
        if pos + 2 < PACKET_LEN {
            buffer.drain(..pos + 2);
            continue;
        }
        let start = pos + 2 - PACKET_LEN;
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
        buffer.drain(..buffer.len().saturating_sub(keep));
    }
    readings
}

fn parse_digits(buf: &[u8], negative: bool) -> Option<f64> {
    if &buf[..5] == b"::0<:" || &buf[..5] == b":<0::" {
        return Some(f64::INFINITY);
    }
    let four_count = buf[4] == b':';
    let digits = if four_count { 4 } else { 5 };
    let mut intval: i32 = 0;
    for &byte in &buf[..digits] {
        if !byte.is_ascii_digit() {
            return None;
        }
        intval = intval * 10 + i32::from(byte - b'0');
    }
    if four_count {
        intval *= 10;
    }
    if negative {
        intval = -intval;
    }
    Some(f64::from(intval))
}

fn exponent(buf: &[u8]) -> Option<i32> {
    let range = buf[5].checked_sub(b'0')?;
    let mode = buf[6].checked_sub(b'0')?;
    if range > 7 || mode > 15 {
        return None;
    }
    Some(EXPONENTS[mode as usize][range as usize])
}

fn parse_flags(buf: &[u8]) -> Option<Flags> {
    let mut flags = Flags::default();
    match buf[6].checked_sub(b'0')? {
        0 => {
            flags.voltage = true;
            flags.ac = true;
        }
        1 | 3 => {
            flags.voltage = true;
            flags.dc = true;
        }
        2 => {
            flags.voltage = true;
            flags.ac = true;
        }
        4 => flags.resistance = true,
        5 => flags.capacitance = true,
        6 => {
            flags.temperature = true;
            flags.celsius = true;
        }
        7..=9 => {
            flags.current = true;
            flags.dc = true;
        }
        10 => flags.continuity = true,
        11 => flags.diode = true,
        12 => flags.frequency = true,
        13 => {
            flags.temperature = true;
            flags.fahrenheit = true;
        }
        14 => flags.power = true,
        15 => flags.loop_current = true,
        _ => return None,
    }
    flags.ac = buf[7] & 0b0000_0001 != 0;
    flags.dc = buf[7] & 0b0000_0010 != 0;
    flags.auto = buf[8] & 0b0000_0001 != 0;
    flags.manual = buf[8] & 0b0000_0010 != 0;
    flags.sign = buf[8] & 0b0000_0100 != 0;
    if flags.frequency && flags.sign {
        flags.duty = true;
        flags.frequency = false;
        flags.sign = false;
    }
    if !flags_valid(&flags) {
        return None;
    }
    Some(flags)
}

fn flags_valid(flags: &Flags) -> bool {
    let kinds = [
        flags.voltage,
        flags.current,
        flags.resistance,
        flags.capacitance,
        flags.frequency,
        flags.temperature,
        flags.continuity,
        flags.diode,
        flags.power,
        flags.loop_current,
    ]
    .into_iter()
    .filter(|on| *on)
    .count();
    kinds <= 1 && !(flags.auto && flags.manual)
}

fn mode_and_unit(flags: &Flags, _value: f64) -> (MeterMode, &'static str) {
    if flags.power {
        return (MeterMode::Power, "W");
    }
    if flags.loop_current || flags.current {
        return if flags.ac && !flags.dc {
            (MeterMode::Aac, "AAC")
        } else {
            (MeterMode::Adc, "ADC")
        };
    }
    if flags.diode {
        return (MeterMode::Diod, "V");
    }
    if flags.voltage {
        return if flags.ac && !flags.dc {
            (MeterMode::Vac, "VAC")
        } else {
            (MeterMode::Vdc, "VDC")
        };
    }
    if flags.continuity {
        return (MeterMode::Cont, "Ohm");
    }
    if flags.resistance {
        return (MeterMode::Res, "Ohm");
    }
    if flags.capacitance {
        return (MeterMode::Cap, "F");
    }
    if flags.duty {
        return (MeterMode::Duty, "%");
    }
    if flags.frequency {
        return (MeterMode::Freq, "Hz");
    }
    if flags.temperature && flags.fahrenheit {
        return (MeterMode::Temp, "°F");
    }
    (MeterMode::Temp, "°C")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bytes: &[u8]) -> [u8; PACKET_LEN] {
        let mut out = [0u8; PACKET_LEN];
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    #[test]
    fn dc_volts_use_the_range_exponent() {
        // 12345 × 10^-4 = 1.2345 V, DC, auto.
        let reading = parse_packet(&frame(b"1234511\x02\x01\r\n")).unwrap();
        assert!((reading.value - 1.2345).abs() < 1e-9);
        assert_eq!(reading.mode, MeterMode::Vdc);
        assert_eq!(reading.unit, "VDC");
    }

    #[test]
    fn sign_bit_negates_the_digits() {
        let reading = parse_packet(&frame(b"1234511\x02\x05\r\n")).unwrap();
        assert!((reading.value + 1.2345).abs() < 1e-9);
    }

    #[test]
    fn overload_token_is_the_sentinel() {
        let reading = parse_packet(&frame(b"::0<:11\x02\x01\r\n")).unwrap();
        assert_eq!(reading.value, METER_OVERLOAD_VALUE);
    }

    #[test]
    fn four_count_resistance_gains_a_digit() {
        // '1234:' is 4000-count; multiply by 10 before the exponent.
        let reading = parse_packet(&frame(b"1234:04\x00\x01\r\n")).unwrap();
        assert!((reading.value - 12_340.0).abs() < 1e-6);
        assert_eq!(reading.mode, MeterMode::Res);
    }

    #[test]
    fn power_is_its_own_mode() {
        // Function byte '0' + 14 = '>'.
        let reading = parse_packet(&frame(b"012500>\x00\x00\r\n")).unwrap();
        assert!((reading.value - 1250.0).abs() < 1e-6);
        assert_eq!(reading.mode, MeterMode::Power);
        assert_eq!(reading.unit, "W");
    }

    #[test]
    fn loop_current_is_dc_milliamps() {
        // 01250 × 10^-2 = 12.50, shown as 12.50 mA.
        let reading = parse_packet(&frame(b"012500?\x02\x00\r\n")).unwrap();
        assert!((reading.value - 0.0125).abs() < 1e-9);
        assert_eq!(reading.mode, MeterMode::Adc);
    }

    #[test]
    fn missing_crlf_is_rejected() {
        let mut raw = *b"1234511\x02\x01\r\n";
        raw[9] = b' ';
        assert!(parse_packet(&raw).is_none());
    }

    #[test]
    fn auto_and_manual_together_are_rejected() {
        assert!(parse_packet(&frame(b"1234511\x02\x03\r\n")).is_none());
    }

    #[test]
    fn feed_bytes_waits_for_a_full_frame() {
        let mut buf = Vec::new();
        assert!(feed_bytes(&mut buf, b"1234511\x02").is_empty());
        let got = feed_bytes(&mut buf, b"\x01\r\n");
        assert_eq!(got.len(), 1);
        assert!((got[0].value - 1.2345).abs() < 1e-9);
    }
}
