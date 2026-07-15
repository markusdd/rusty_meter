//! Victor DM1107 meters (e.g. 86D) — CP2102 serial protocol (9600 8N1).
//!
//! Newer Victor handhelds use a **DM1107** system IC and expose **only** a serial
//! link (CP2102 USB-UART). Meter logic is galvanically isolated from USB via an
//! optocoupler path — not the legacy HID/FS9922 cable used on discontinued 86B/C/D units.
//!
//! Each burst is a fixed **20-byte** frame:
//! ```text
//! a5 12  b2 b3 b4  [d3 d2 d1 d0]  b9  pad…  flg0-2  t0 04 t2
//! ```
//!
//! The four digit bytes on the wire are **rightmost-first** (`slice[8]` = LCD d0 / MSD,
//! `slice[5]` = LCD d3 / LSD). [`VictorFrame::digits`] is stored left-to-right for display.
//!
//! Digit bytes use **7 segment bits (low) + decimal point (bit 7)** on d0–d2.
//! `0x5f` is a lit **zero**; `0x00` is the digit position **off**; `0x80` is
//! **decimal point only** (digit off, DP lit). Combined: `0xdf = 0x5f|0x80` is zero + DP.
//!
//! Mode/range/unit annunciators are individual bits across bytes 2–4 and 9 (not
//! independent byte values). See [`LcdAnnunciators`].

use crate::helpers::METER_OVERLOAD_VALUE;
use crate::multimeter::MeterMode;

pub const VICTOR_86BCD_BAUD: u32 = 9600;

pub const SYNC: [u8; 2] = [0xa5, 0x12];
pub const FRAME_LEN: usize = 20;
pub const TAIL_MARKER: u8 = 0x04;

/// Low 7 bits = segment bitmap; bit 7 = decimal point after this digit (positions 0–2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigitCell {
    pub raw: u8,
}

impl DigitCell {
    /// Lit zero segment pattern (not “off”).
    pub const ZERO_SEG: u8 = 0x5f;
    /// Digit position completely off.
    pub const HARD_OFF: u8 = 0x00;
    /// Digit off with only the decimal point lit.
    pub const DP_ONLY: u8 = 0x80;

    pub fn from_raw(raw: u8) -> Self {
        Self { raw }
    }

    pub fn segments(self) -> u8 {
        self.raw & 0x7f
    }

    pub fn decimal_point(self) -> bool {
        self.raw & 0x80 != 0
    }

    pub fn is_zero(self) -> bool {
        self.segments() == Self::ZERO_SEG
    }

    /// Digit position off: `0x00`, or `0x80` when only the DP is lit.
    pub fn is_off(self) -> bool {
        matches!(self.raw, Self::HARD_OFF | Self::DP_ONLY)
    }

    pub fn is_dp_only(self) -> bool {
        self.raw == Self::DP_ONLY
    }
}

/// Three-byte mode/range header after `a5 12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeHeader {
    pub b2: u8,
    pub b3: u8,
    pub b4: u8,
}

/// LCD annunciator bits extracted from the mode header and delimiter byte.
///
/// Each field maps to one on-screen symbol (V, m, DC, −, AUTO, …). Modes are
/// derived from which combination is lit, not from whole-byte templates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LcdAnnunciators {
    /// Byte 9 bit 1 — DC marker.
    pub dc: bool,
    /// Byte 9 bit 3 — AC marker.
    pub ac: bool,
    /// Byte 9 bit 2 — minus (−) annunciator (independent of measurement function).
    pub minus: bool,
    /// Byte 4 bit 4 — AUTO range.
    pub auto_range: bool,
    /// Byte 4 bit 6 — MANUAL range (fixed decimal point).
    pub manual_range: bool,
    /// Byte 4 bit 2 — diode test.
    pub diode: bool,
    /// Byte 4 bit 3 — continuity.
    pub continuity: bool,
    /// Byte 2 bit 2 — volt (V) unit.
    pub volt: bool,
    /// Byte 2 bit 3 — milli (m) prefix.
    pub milli: bool,
    /// Byte 2 bit 1 — ampere (A) unit.
    pub amp: bool,
    /// Byte 2 bit 5 — ohm (Ω) unit.
    pub ohm: bool,
    /// Byte 2 bit 6 — kilo (k) prefix.
    pub kilo: bool,
    /// Byte 2 bit 7 — mega (M) prefix.
    pub mega: bool,
    /// Byte 2 bit 4 — hertz (Hz).
    pub hertz: bool,
    /// Byte 2 bit 0 — farad (F) capacitance.
    pub farad: bool,
    /// Byte 3 bit 2 — duty cycle (%).
    pub duty: bool,
    /// Byte 3 bit 5 — Celsius (°C).
    pub celsius: bool,
    /// Byte 3 bit 4 — Fahrenheit (°F).
    pub fahrenheit: bool,
    /// Byte 3 bit 6 — nano (n) prefix.
    pub nano: bool,
    /// Byte 3 bit 7 — micro (µ) prefix.
    pub micro: bool,
}

impl LcdAnnunciators {
    pub fn from_mode_header(mode: ModeHeader, delimiter: u8) -> Self {
        Self {
            dc: delimiter & 0x02 != 0,
            minus: delimiter & 0x04 != 0,
            ac: delimiter & 0x08 != 0,
            auto_range: mode.b4 & 0x10 != 0,
            manual_range: mode.b4 & 0x40 != 0,
            diode: mode.b4 & 0x04 != 0,
            continuity: mode.b4 & 0x08 != 0,
            volt: mode.b2 & 0x04 != 0,
            milli: mode.b2 & 0x08 != 0,
            amp: mode.b2 & 0x02 != 0,
            ohm: mode.b2 & 0x20 != 0,
            kilo: mode.b2 & 0x40 != 0,
            mega: mode.b2 & 0x80 != 0,
            hertz: mode.b2 & 0x10 != 0,
            farad: mode.b2 & 0x01 != 0,
            duty: mode.b3 & 0x04 != 0,
            celsius: mode.b3 & 0x20 != 0,
            fahrenheit: mode.b3 & 0x10 != 0,
            nano: mode.b3 & 0x40 != 0,
            micro: mode.b3 & 0x80 != 0,
        }
    }

    pub fn function_and_unit(self) -> (MeterFunction, MeterUnit) {
        if self.diode {
            return (MeterFunction::Diod, MeterUnit::V);
        }
        if self.continuity {
            return (MeterFunction::Cont, MeterUnit::Ohm);
        }
        if self.celsius {
            return (MeterFunction::Temp, MeterUnit::Celsius);
        }
        if self.fahrenheit {
            return (MeterFunction::Temp, MeterUnit::Fahrenheit);
        }
        if self.duty {
            return (MeterFunction::Duty, MeterUnit::Percent);
        }
        if self.hertz {
            return (MeterFunction::Freq, MeterUnit::Hz);
        }
        if self.farad {
            let unit = if self.micro {
                MeterUnit::uF
            } else if self.nano {
                MeterUnit::nF
            } else {
                MeterUnit::Unknown
            };
            return (MeterFunction::Cap, unit);
        }
        if self.ohm {
            let unit = if self.mega {
                MeterUnit::MOhm
            } else if self.kilo {
                MeterUnit::kOhm
            } else {
                MeterUnit::Ohm
            };
            return (MeterFunction::Res, unit);
        }
        if self.volt {
            let unit = if self.milli { MeterUnit::mV } else { MeterUnit::V };
            return if self.ac {
                (MeterFunction::Vac, unit)
            } else {
                (MeterFunction::Vdc, unit)
            };
        }
        if self.amp {
            let unit = if self.micro {
                MeterUnit::uA
            } else if self.milli {
                MeterUnit::mA
            } else {
                MeterUnit::A
            };
            return if self.ac {
                (MeterFunction::Aac, unit)
            } else {
                (MeterFunction::Adc, unit)
            };
        }
        (MeterFunction::Unknown, MeterUnit::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterFunction {
    Vdc,
    Vac,
    Adc,
    Aac,
    Res,
    Cap,
    Freq,
    Duty,
    Diod,
    Cont,
    Temp,
    Unknown,
}

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterUnit {
    V,
    mV,
    A,
    mA,
    uA,
    Ohm,
    kOhm,
    MOhm,
    nF,
    uF,
    Hz,
    Percent,
    Celsius,
    Fahrenheit,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DpMode {
    Manual,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcDc {
    Dc,
    Ac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayKind {
    Normal,
    /// `.0L`, `0.L`, bar graph full — first/last digit off, DP lit.
    Overload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMode {
    pub function: MeterFunction,
    pub unit: MeterUnit,
    pub dp_mode: DpMode,
    pub ac_dc: AcDc,
    /// Minus (−) annunciator lit; digit bytes stay unsigned.
    pub is_negative: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VictorFrame {
    pub mode: ModeHeader,
    pub decoded_mode: DecodedMode,
    /// Four digit slots in LCD order: `[d0, d1, d2, d3]` (left to right).
    pub digits: [DigitCell; 4],
    pub delimiter: u8,
    pub flags: [u8; 3],
    pub tail: [u8; 3],
    pub display_kind: DisplayKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedReading {
    pub text: String,
    pub confidence: u8,
}

/// One live decode from the serial stream (6000-count / 4-digit LCD meters).
#[derive(Debug, Clone, PartialEq)]
pub struct Dm1107LiveUpdate {
    /// LCD text as shown on the meter (`21.40`, `00.00`, `.0L`, …).
    pub display: String,
    /// Parsed value for graph/recording when numeric.
    pub value: Option<f64>,
    pub mode: MeterMode,
    /// Unit annunciator (`VDC`, `nF`, `Hz`, …) — not auto-scaled.
    pub unit: String,
}

/// Rolling buffer for live CP2102 stream decode.
#[derive(Debug, Default)]
pub struct Dm1107Stream {
    buf: Vec<u8>,
    last_text: Option<String>,
    last_mode_key: Option<String>,
}

impl Dm1107Stream {
    pub fn new() -> Self {
        Self::default()
    }
}

const STREAM_KEEP_MAX: usize = 64;

/// Parse one aligned 20-byte frame starting at `offset`.
pub fn parse_frame_at(raw: &[u8], offset: usize) -> Option<VictorFrame> {
    let slice = raw.get(offset..offset + FRAME_LEN)?;
    if slice[0..2] != SYNC {
        return None;
    }
    let mode = ModeHeader {
        b2: slice[2],
        b3: slice[3],
        b4: slice[4],
    };
    // Wire transmits d3..d0; store d0..d3 for LCD-left-to-right decode.
    let digits = [
        DigitCell::from_raw(slice[8]),
        DigitCell::from_raw(slice[7]),
        DigitCell::from_raw(slice[6]),
        DigitCell::from_raw(slice[5]),
    ];
    let delimiter = slice[9];
    let flags = [slice[14], slice[15], slice[16]];
    let tail = [slice[17], slice[18], slice[19]];
    if tail[1] != TAIL_MARKER {
        return None;
    }
    let decoded_mode = decode_mode_header(mode, delimiter);
    let display_kind = classify_display(&digits, flags);
    Some(VictorFrame {
        mode,
        decoded_mode,
        digits,
        delimiter,
        flags,
        tail,
        display_kind,
    })
}

/// Find the most recent valid `a5 12` frame in a buffer (live stream may retain older frames).
pub fn find_frame(raw: &[u8]) -> Option<VictorFrame> {
    if raw.len() < FRAME_LEN {
        return None;
    }
    let last_start = raw.len() - FRAME_LEN;
    (0..=last_start).rev().find_map(|i| parse_frame_at(raw, i))
}

/// Decode the 3-byte mode header + delimiter into meter function/unit/DP mode.
pub fn decode_mode_header(mode: ModeHeader, delimiter: u8) -> DecodedMode {
    let ann = LcdAnnunciators::from_mode_header(mode, delimiter);
    let (function, unit) = ann.function_and_unit();
    let ac_dc = if ann.ac { AcDc::Ac } else { AcDc::Dc };
    let dp_mode = if ann.manual_range {
        DpMode::Manual
    } else {
        DpMode::Auto
    };

    DecodedMode {
        function,
        unit,
        dp_mode,
        ac_dc,
        is_negative: ann.minus,
    }
}

fn decoded_mode_key(dm: &DecodedMode) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{}",
        dm.function, dm.unit, dm.dp_mode, dm.ac_dc, dm.is_negative
    )
}

fn apply_negative_sign(text: &str, is_negative: bool) -> String {
    if is_negative
        && !text.is_empty()
        && !text.starts_with('-')
        && !is_overload_display(text)
    {
        format!("-{text}")
    } else {
        text.to_owned()
    }
}

/// Map wire mode header to UI [`MeterMode`] and unit label.
pub fn meter_mode_and_unit(dm: &DecodedMode) -> (MeterMode, String) {
    let mode = match dm.function {
        MeterFunction::Vdc => MeterMode::Vdc,
        MeterFunction::Vac => MeterMode::Vac,
        MeterFunction::Adc => MeterMode::Adc,
        MeterFunction::Aac => MeterMode::Aac,
        MeterFunction::Res => MeterMode::Res,
        MeterFunction::Cap => MeterMode::Cap,
        MeterFunction::Freq => MeterMode::Freq,
        MeterFunction::Duty => MeterMode::Duty,
        MeterFunction::Diod => MeterMode::Diod,
        MeterFunction::Cont => MeterMode::Cont,
        MeterFunction::Temp => MeterMode::Temp,
        MeterFunction::Unknown => MeterMode::Vdc,
    };
    let unit = match (dm.function, dm.unit, dm.ac_dc) {
        (MeterFunction::Vdc, MeterUnit::V, _) => "VDC".to_owned(),
        (MeterFunction::Vdc, MeterUnit::mV, _) => "mVDC".to_owned(),
        (MeterFunction::Vac, MeterUnit::V, _) => "VAC".to_owned(),
        (MeterFunction::Vac, MeterUnit::mV, _) => "mVAC".to_owned(),
        (MeterFunction::Adc, MeterUnit::A, _) => "ADC".to_owned(),
        (MeterFunction::Adc, MeterUnit::mA, _) => "mADC".to_owned(),
        (MeterFunction::Adc, MeterUnit::uA, _) => "µADC".to_owned(),
        (MeterFunction::Aac, MeterUnit::A, _) => "AAC".to_owned(),
        (MeterFunction::Aac, MeterUnit::mA, _) => "mAAC".to_owned(),
        (MeterFunction::Aac, MeterUnit::uA, _) => "µAAC".to_owned(),
        (MeterFunction::Res, MeterUnit::Ohm, _) => "Ω".to_owned(),
        (MeterFunction::Res, MeterUnit::kOhm, _) => "kΩ".to_owned(),
        (MeterFunction::Res, MeterUnit::MOhm, _) => "MΩ".to_owned(),
        (MeterFunction::Cont, MeterUnit::Ohm, _) => "Ω".to_owned(),
        (MeterFunction::Cap, MeterUnit::nF, _) => "nF".to_owned(),
        (MeterFunction::Cap, MeterUnit::uF, _) => "µF".to_owned(),
        (MeterFunction::Freq, MeterUnit::Hz, _) => "Hz".to_owned(),
        (MeterFunction::Duty, MeterUnit::Percent, _) => "%".to_owned(),
        (MeterFunction::Diod, MeterUnit::V, _) => "V".to_owned(),
        (MeterFunction::Temp, MeterUnit::Celsius, _) => "°C".to_owned(),
        (MeterFunction::Temp, MeterUnit::Fahrenheit, _) => "°F".to_owned(),
        _ => "?".to_owned(),
    };
    (mode, unit)
}

/// Open-line / overload strings — no numeric value, must not trigger Cont/Diod thresholds.
pub fn is_overload_display(display: &str) -> bool {
    matches!(display, "OL" | ".0L" | "0.L" | "OVERLOAD") || display.contains('L')
}

fn parse_display_value(display: &str, mode: MeterMode) -> Option<f64> {
    if is_overload_display(display)
        && matches!(mode, MeterMode::Diod | MeterMode::Cont | MeterMode::Res)
    {
        return Some(METER_OVERLOAD_VALUE);
    }
    let stripped = display.trim();
    if stripped.is_empty() || stripped.ends_with('.') {
        return None;
    }
    stripped.parse().ok()
}

fn classify_display(digits: &[DigitCell; 4], flags: [u8; 3]) -> DisplayKind {
    let overload_flags = flags == [0xff, 0xff, 0xff] || flags[0] == 0xff;
    let ol_digits = digits[0].is_dp_only()
        || digits[0].raw == DigitCell::HARD_OFF
        || digits[3].raw == DigitCell::HARD_OFF;
    if overload_flags || ol_digits {
        DisplayKind::Overload
    } else {
        DisplayKind::Normal
    }
}

/// Empirical segment → digit/char (not standard GFEDCBA — DM1107 wire encoding).
fn glyph_for_segments(seg7: u8) -> Option<char> {
    match seg7 {
        0x5f => Some('0'),
        0x3d => Some('2'),
        0x50 => Some('1'),
        0x72 => Some('4'),
        0x79 => Some('3'),
        0x51 => Some('7'),
        0x7b => Some('9'),
        0x7f => Some('8'),
        0x6b => Some('5'),
        0x6f => Some('6'),
        0x0e | 0x0c => Some('0'), // overload / diode zero glyph
        _ => None,
    }
}

/// Build display text from digit cells + decimal points.
pub fn format_digits(digits: &[DigitCell; 4]) -> DecodedReading {
    // DIOD `.0L`: leading DP-only, `0` in the middle, LSD hard-off.
    if digits[0].is_dp_only()
        && digits[2].segments() == 0x0e
        && digits[3].raw == DigitCell::HARD_OFF
    {
        return DecodedReading {
            text: ".0L".to_owned(),
            confidence: 90,
        };
    }
    // RES `0.L`: d1 = zero+DP (`0xdf`), d2 = overload glyph (`0x0e`).
    if digits[1].is_zero()
        && digits[1].decimal_point()
        && digits[2].segments() == 0x0e
    {
        return DecodedReading {
            text: "0.L".to_owned(),
            confidence: 90,
        };
    }
    // CONT open (`_0L._`): d1 = 0, d2 = `0x8e` (0x0e + DP), outer digits hard-off.
    if digits[0].raw == DigitCell::HARD_OFF
        && digits[1].is_zero()
        && digits[2].segments() == 0x0e
        && digits[2].decimal_point()
        && digits[3].raw == DigitCell::HARD_OFF
    {
        return DecodedReading {
            text: "OL".to_owned(),
            confidence: 90,
        };
    }

    let mut chars: Vec<char> = Vec::new();

    let mut known = 0u8;
    let mut total = 0u8;

    for (i, cell) in digits.iter().enumerate() {
        if cell.is_dp_only() {
            chars.push('.');
            continue;
        }
        if cell.raw == DigitCell::HARD_OFF {
            continue;
        }
        total += 1;
        if let Some(c) = glyph_for_segments(cell.segments()) {
            chars.push(c);
            known += 1;
        } else {
            chars.push('?');
        }
        if i < 3 && cell.decimal_point() {
            chars.push('.');
        }
    }

    let text: String = chars.into_iter().collect();
    let confidence = if total == 0 {
        0
    } else {
        ((known as u16) * 100 / total as u16).min(100) as u8
    };
    DecodedReading { text, confidence }
}

/// Full frame → human reading string (best effort).
pub fn decode_frame(frame: &VictorFrame) -> DecodedReading {
    let mut reading = format_digits(&frame.digits);
    if reading.confidence >= 50 && !reading.text.is_empty() {
        reading.text = apply_negative_sign(&reading.text, frame.decoded_mode.is_negative);
        return reading;
    }
    if frame.display_kind == DisplayKind::Overload {
        let text = match frame.decoded_mode.function {
            MeterFunction::Cont => "OL".to_owned(),
            MeterFunction::Res => "0.L".to_owned(),
            MeterFunction::Diod => ".0L".to_owned(),
            _ => ".0L".to_owned(),
        };
        return DecodedReading {
            text,
            confidence: 85,
        };
    }
    reading.text = apply_negative_sign(&reading.text, frame.decoded_mode.is_negative);
    reading
}

/// Append serial bytes; return new live readings (display + mode + optional value).
pub fn feed_bytes(state: &mut Dm1107Stream, chunk: &[u8]) -> Vec<Dm1107LiveUpdate> {
    state.buf.extend_from_slice(chunk);
    let mut out = Vec::new();

    if let Some(frame) = find_frame(&state.buf) {
        let reading = decode_frame(&frame);
        let mode_key = decoded_mode_key(&frame.decoded_mode);
        let usable = !reading.text.is_empty()
            && (reading.confidence >= 50 || frame.display_kind == DisplayKind::Overload);
        let changed = state.last_text.as_deref() != Some(reading.text.as_str())
            || state.last_mode_key.as_deref() != Some(mode_key.as_str());
        if usable && changed {
            let display = reading.text.clone();
            state.last_text = Some(display.clone());
            state.last_mode_key = Some(mode_key);
            let (mode, unit) = meter_mode_and_unit(&frame.decoded_mode);
            out.push(Dm1107LiveUpdate {
                display,
                value: parse_display_value(&reading.text, mode),
                mode,
                unit,
            });
        }
    }

    if state.buf.len() > STREAM_KEEP_MAX {
        let keep = STREAM_KEEP_MAX / 2;
        let drain = state.buf.len().saturating_sub(keep);
        state.buf.drain(..drain);
    }
    out
}

/// Format bytes as lowercase hex (debug logging, CSV).
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_hex_line(hex: &str) -> Vec<u8> {
        hex.split_whitespace()
            .filter_map(|t| u8::from_str_radix(t, 16).ok())
            .collect()
    }

    #[test]
    fn hex_encodes_bytes() {
        assert_eq!(hex_encode(&[0xa5, 0x12, 0x04]), "a5 12 04");
    }

    #[test]
    fn parses_vdc_manual_zero_frame() {
        let raw = parse_hex_line(
            "a5 12 04 00 40 5f 5f df 5f 42 00 00 00 00 00 00 00 0c 04 49",
        );
        let frame = parse_frame_at(&raw, 0).unwrap();
        assert_eq!(frame.mode.b2, 0x04);
        assert_eq!(frame.mode.b4, 0x40);
        assert_eq!(frame.decoded_mode.function, MeterFunction::Vdc);
        assert_eq!(frame.decoded_mode.unit, MeterUnit::V);
        assert_eq!(frame.decoded_mode.dp_mode, DpMode::Manual);
        assert!(frame.digits[1].decimal_point());
        assert_eq!(frame.digits[1].segments(), 0x5f);
    }

    #[test]
    fn parses_vdc_21_34_payload() {
        let raw = parse_hex_line(
            "a5 12 04 00 40 72 79 d0 3d 42 00 00 00 00 01 ff ff fc 04 34",
        );
        let frame = parse_frame_at(&raw, 0).unwrap();
        assert_eq!(frame.digits[0].segments(), 0x3d);
        assert!(frame.digits[1].decimal_point());
        assert_eq!(frame.digits[1].segments(), 0x50);
        assert_eq!(frame.digits[2].segments(), 0x79);
        assert_eq!(frame.digits[3].segments(), 0x72);
        assert_eq!(decode_frame(&frame).text, "21.34");
        assert_eq!(frame.tail[2], 0x34);
    }

    #[test]
    fn decodes_vdc_auto_21_40_capture() {
        let raw = parse_hex_line(
            "a5 12 04 00 10 5f 72 d0 3d 42 00 00 00 00 01 ff ff fc 04 ea",
        );
        let frame = parse_frame_at(&raw, 0).unwrap();
        assert_eq!(frame.digits[0].segments(), 0x3d);
        assert_eq!(frame.digits[1].segments(), 0x50);
        assert!(frame.digits[1].decimal_point());
        assert_eq!(frame.digits[2].segments(), 0x72);
        assert!(frame.digits[3].is_zero());
        assert_eq!(decode_frame(&frame).text, "21.40");
    }

    #[test]
    fn decodes_manual_vdc_zero_layouts() {
        let cases = [
            ("a5 12 04 00 40 5f 5f df 5f 42 00 00 00 00 00 00 00 0c 04 49", "00.00"),
            ("a5 12 04 00 40 5f 5f 5f df 42 00 00 00 00 00 00 00 0c 04 49", "0.000"),
            ("a5 12 04 00 40 5f df 5f 5f 42 00 00 00 00 00 00 00 0c 04 49", "000.0"),
        ];
        for (hex, want) in cases {
            let raw = parse_hex_line(hex);
            let frame = parse_frame_at(&raw, 0).unwrap();
            assert_eq!(decode_frame(&frame).text, want, "{hex}");
        }
    }

    #[test]
    fn decodes_mode_mv_and_res_ladder() {
        let mv = parse_hex_line("a5 12 0c 00 10 5f df 5f 5f 42 00 00 00 00 00 00 00 0c 04 21");
        assert_eq!(find_frame(&mv).unwrap().decoded_mode.unit, MeterUnit::mV);

        for (b2, unit) in [(0x20, MeterUnit::Ohm), (0x60, MeterUnit::kOhm), (0xa0, MeterUnit::MOhm)] {
            let mut raw = parse_hex_line(
                "a5 12 00 00 10 5f df 5f 5f 40 00 00 00 00 00 00 00 0c 04 33",
            );
            raw[2] = b2;
            assert_eq!(find_frame(&raw).unwrap().decoded_mode.unit, unit);
        }
    }

    #[test]
    fn diode_ol_overload_pattern() {
        let raw = parse_hex_line(
            "a5 12 04 00 04 00 0e 5f 80 40 ff ff ff ff ff ff ff fc 04 e5",
        );
        let frame = find_frame(&raw).unwrap();
        assert_eq!(frame.display_kind, DisplayKind::Overload);
        assert!(frame.digits[0].is_dp_only());
        assert_eq!(frame.digits[2].segments(), 0x0e);
        assert_eq!(frame.digits[3].raw, DigitCell::HARD_OFF);
        assert_eq!(decode_frame(&frame).text, ".0L");
    }

    #[test]
    fn decodes_negative_vdc_auto_21_19() {
        let raw = parse_hex_line(
            "a5 12 04 00 10 7b 50 d0 3d 46 00 00 00 00 01 ff ff fe 04 ea",
        );
        let frame = parse_frame_at(&raw, 0).unwrap();
        assert_eq!(frame.delimiter, 0x46);
        assert!(frame.decoded_mode.is_negative);
        assert_eq!(frame.decoded_mode.function, MeterFunction::Vdc);
        assert_eq!(frame.decoded_mode.unit, MeterUnit::V);
        let reading = decode_frame(&frame);
        assert_eq!(reading.text, "-21.19");
        let (mode, unit) = meter_mode_and_unit(&frame.decoded_mode);
        assert_eq!(mode, MeterMode::Vdc);
        assert_eq!(unit, "VDC");
        assert_eq!(
            parse_display_value(&reading.text, mode),
            Some(-21.19)
        );
    }

    #[test]
    fn annunciator_bits_from_captures() {
        let vdc_auto = LcdAnnunciators::from_mode_header(
            ModeHeader {
                b2: 0x04,
                b3: 0x00,
                b4: 0x10,
            },
            0x42,
        );
        assert!(vdc_auto.volt);
        assert!(vdc_auto.dc);
        assert!(vdc_auto.auto_range);
        assert!(!vdc_auto.milli);
        assert!(!vdc_auto.minus);

        let vdc_neg = LcdAnnunciators::from_mode_header(
            ModeHeader {
                b2: 0x04,
                b3: 0x00,
                b4: 0x10,
            },
            0x46,
        );
        assert!(vdc_neg.minus);
        assert_eq!(
            vdc_neg.function_and_unit(),
            (MeterFunction::Vdc, MeterUnit::V)
        );

        let mv_auto = LcdAnnunciators::from_mode_header(
            ModeHeader {
                b2: 0x0c,
                b3: 0x00,
                b4: 0x10,
            },
            0x42,
        );
        assert!(mv_auto.volt);
        assert!(mv_auto.milli);
        assert_eq!(
            mv_auto.function_and_unit(),
            (MeterFunction::Vdc, MeterUnit::mV)
        );

        let mv_neg = LcdAnnunciators::from_mode_header(
            ModeHeader {
                b2: 0x0c,
                b3: 0x00,
                b4: 0x10,
            },
            0x46,
        );
        assert!(mv_neg.minus);
        assert_eq!(
            mv_neg.function_and_unit(),
            (MeterFunction::Vdc, MeterUnit::mV)
        );
    }

    #[test]
    fn decodes_negative_mv_without_breaking_unit() {
        let mut raw = parse_hex_line("a5 12 0c 00 10 5f df 5f 5f 42 00 00 00 00 00 00 00 0c 04 21");
        raw[9] = 0x46;
        let frame = find_frame(&raw).unwrap();
        assert_eq!(frame.decoded_mode.function, MeterFunction::Vdc);
        assert_eq!(frame.decoded_mode.unit, MeterUnit::mV);
        assert!(frame.decoded_mode.is_negative);
        let (mode, unit) = meter_mode_and_unit(&frame.decoded_mode);
        assert_eq!(unit, "mVDC");
        assert_eq!(mode, MeterMode::Vdc);
    }

    #[test]
    fn decodes_digit_six() {
        // CAP µF capture: segment 0x6f = digit 6 (distinct from 0x7d seen elsewhere).
        let raw = parse_hex_line(
            "a5 12 01 80 10 6f bd 6f 72 40 00 03 ff ff ff ff ff fc 04 93",
        );
        let frame = parse_frame_at(&raw, 0).unwrap();
        assert_eq!(frame.decoded_mode.function, MeterFunction::Cap);
        assert_eq!(frame.decoded_mode.unit, MeterUnit::uF);
        assert_eq!(decode_frame(&frame).text, "462.6");
    }

    #[test]
    fn temp_shows_trailing_digits() {
        let raw = parse_hex_line(
            "a5 12 00 20 00 5f 79 5f 5f 40 00 00 00 00 00 00 00 0c 04 bd",
        );
        let frame = find_frame(&raw).unwrap();
        assert!(frame.digits[0].is_zero());
        assert!(frame.digits[1].is_zero());
        assert_eq!(frame.digits[2].segments(), 0x79);
        assert!(frame.digits[3].is_zero());
        assert_eq!(decode_frame(&frame).text, "0030");
    }

    #[test]
    fn digit_cell_dp_bit() {
        let cell = DigitCell::from_raw(0xdf);
        assert_eq!(cell.segments(), 0x5f);
        assert!(cell.decimal_point());
        assert!(!cell.is_off());
    }

    #[test]
    fn feed_bytes_emits_mode_and_display() {
        let frame = parse_hex_line(
            "a5 12 04 00 10 5f 72 d0 3d 42 00 00 00 00 01 ff ff fc 04 ea",
        );
        let mut stream = Dm1107Stream::new();
        let updates = feed_bytes(&mut stream, &frame);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].display, "21.40");
        assert_eq!(updates[0].value, Some(21.40));
        assert_eq!(updates[0].mode, MeterMode::Vdc);
        assert_eq!(updates[0].unit, "VDC");
        assert!(feed_bytes(&mut stream, &frame).is_empty());
    }

    #[test]
    fn cont_open_line_decodes_as_ol() {
        let raw = parse_hex_line(
            "a5 12 20 00 08 00 8e 5f 00 40 ff ff ff ff ff ff ff fc 04 05",
        );
        let frame = find_frame(&raw).unwrap();
        assert_eq!(frame.decoded_mode.function, MeterFunction::Cont);
        let reading = decode_frame(&frame);
        assert_eq!(reading.text, "OL");
        assert!(is_overload_display(&reading.text));
        assert_eq!(parse_display_value(&reading.text, MeterMode::Cont), Some(METER_OVERLOAD_VALUE));
    }

    #[test]
    fn feed_bytes_emits_on_mode_change() {
        let vdc = parse_hex_line(
            "a5 12 04 00 10 5f 72 d0 3d 42 00 00 00 00 01 ff ff fc 04 ea",
        );
        let cap = parse_hex_line(
            "a5 12 01 40 10 5f 5f 5f 5f 42 00 00 00 00 00 00 00 0c 04 00",
        );
        let mut stream = Dm1107Stream::new();
        assert_eq!(feed_bytes(&mut stream, &vdc).len(), 1);
        let cap_updates = feed_bytes(&mut stream, &cap);
        assert_eq!(cap_updates.len(), 1);
        assert_eq!(cap_updates[0].mode, MeterMode::Cap);
        assert_eq!(cap_updates[0].unit, "nF");
    }
}