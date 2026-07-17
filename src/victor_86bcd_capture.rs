//! Labeled serial capture for Victor 86B/C/D meters (DM1107 USB-serial).
//!
//! On these meters, **manual range** means a fixed decimal-point position
//! (not a traditional 20 V / 200 mV knob). **Unit** (V, mV, µA, …) is a separate
//! LCD annunciator — reflected in the wire stream.
//!
//! Captures store the LCD as four explicit digit slots (`d0`–`d3`) plus `dp_after`
//! (decimal after digit 0, 1, or 2). Use `_` for a digit that is completely off;
//! `0`–`9` for a lit digit. This avoids spreadsheet coercion (`00.00` → `0`) and
//! keeps off-vs-zero distinct for protocol correlation.
//!
//! Not used for Victor 86E (ES51932 serial) or legacy HID (FS9922) meters.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::victor_dm1107;

/// Measurement function selected on the meter (user-declared).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Victor86bcdCaptureFunction {
    #[default]
    Vdc,
    Vac,
    Adc,
    Aac,
    Res,
    Cap,
    Freq,
    Per,
    Duty,
    Diod,
    Cont,
    Temp,
    Other,
}

impl Victor86bcdCaptureFunction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vdc => "VDC",
            Self::Vac => "VAC",
            Self::Adc => "ADC",
            Self::Aac => "AAC",
            Self::Res => "RES",
            Self::Cap => "CAP",
            Self::Freq => "FREQ",
            Self::Per => "PER",
            Self::Duty => "DUTY",
            Self::Diod => "DIOD",
            Self::Cont => "CONT",
            Self::Temp => "TEMP",
            Self::Other => "OTHER",
        }
    }

    pub fn default_unit(self) -> Victor86bcdCaptureUnit {
        match self {
            Self::Vdc | Self::Vac | Self::Diod => Victor86bcdCaptureUnit::V,
            Self::Adc => Victor86bcdCaptureUnit::A,
            Self::Aac => Victor86bcdCaptureUnit::A,
            Self::Res | Self::Cont => Victor86bcdCaptureUnit::Ohm,
            Self::Cap => Victor86bcdCaptureUnit::Nf,
            Self::Freq => Victor86bcdCaptureUnit::Hz,
            Self::Per | Self::Duty => Victor86bcdCaptureUnit::Percent,
            Self::Temp => Victor86bcdCaptureUnit::Celsius,
            Self::Other => Victor86bcdCaptureUnit::Unknown,
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Vdc,
            Self::Vac,
            Self::Adc,
            Self::Aac,
            Self::Res,
            Self::Cap,
            Self::Freq,
            Self::Per,
            Self::Duty,
            Self::Diod,
            Self::Cont,
            Self::Temp,
            Self::Other,
        ]
    }
}

/// Unit annunciator lit on the LCD (separate from function dial).
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Victor86bcdCaptureUnit {
    #[default]
    V,
    mV,
    A,
    mA,
    uA,
    Ohm,
    kOhm,
    MOhm,
    Nf,
    Uf,
    Hz,
    Percent,
    Celsius,
    Fahrenheit,
    Unknown,
}

impl Victor86bcdCaptureUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::V => "V",
            Self::mV => "mV",
            Self::A => "A",
            Self::mA => "mA",
            Self::uA => "uA",
            Self::Ohm => "Ω",
            Self::kOhm => "kΩ",
            Self::MOhm => "MΩ",
            Self::Nf => "nF",
            Self::Uf => "µF",
            Self::Hz => "Hz",
            Self::Percent => "%",
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
            Self::Unknown => "?",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::V,
            Self::mV,
            Self::A,
            Self::mA,
            Self::uA,
            Self::Ohm,
            Self::kOhm,
            Self::MOhm,
            Self::Nf,
            Self::Uf,
            Self::Hz,
            Self::Percent,
            Self::Celsius,
            Self::Fahrenheit,
            Self::Unknown,
        ]
    }
}

/// Fixed decimal-point position (manual) vs autorange moving the decimal (auto).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Victor86bcdCaptureDpMode {
    /// Decimal point locked by the range dial.
    #[default]
    Manual,
    /// Meter chose decimal placement.
    Auto,
}

impl Victor86bcdCaptureDpMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual DP",
            Self::Auto => "auto DP",
        }
    }

    pub fn csv_value(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Manual, Self::Auto]
    }
}

/// One LCD digit position: completely off, a lit numeral, or overload `L`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LcdDigit {
    /// Digit position blank on the LCD (`_` in CSV).
    #[default]
    Off,
    D0,
    D1,
    D2,
    D3,
    D4,
    D5,
    D6,
    D7,
    D8,
    D9,
    /// Overload / open-line `L` (usually last digit).
    L,
}

impl LcdDigit {
    pub fn csv_value(self) -> &'static str {
        match self {
            Self::Off => "_",
            Self::D0 => "0",
            Self::D1 => "1",
            Self::D2 => "2",
            Self::D3 => "3",
            Self::D4 => "4",
            Self::D5 => "5",
            Self::D6 => "6",
            Self::D7 => "7",
            Self::D8 => "8",
            Self::D9 => "9",
            Self::L => "L",
        }
    }

    pub fn ui_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::L => "L",
            d => d.csv_value(),
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::Off,
            Self::D0,
            Self::D1,
            Self::D2,
            Self::D3,
            Self::D4,
            Self::D5,
            Self::D6,
            Self::D7,
            Self::D8,
            Self::D9,
            Self::L,
        ]
    }

    pub fn is_configured(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Four-digit LCD layout with explicit decimal placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LcdDisplay {
    pub digits: [LcdDigit; 4],
    /// Decimal point after digit index 0, 1, or 2. `None` = no decimal shown.
    pub dp_after: Option<u8>,
}

impl LcdDisplay {
    pub fn is_empty(self) -> bool {
        self.digits.iter().all(|d| !d.is_configured()) && self.dp_after.is_none()
    }

    /// Human-readable preview: `00.00`, `__.1L`, `_1.23`, etc.
    pub fn format(&self) -> String {
        let mut out = String::new();
        for (i, digit) in self.digits.iter().enumerate() {
            out.push(match digit {
                LcdDigit::Off => '_',
                LcdDigit::L => 'L',
                d => d.csv_value().chars().next().unwrap_or('_'),
            });
            if self.dp_after == Some(i as u8) {
                out.push('.');
            }
        }
        out
    }

    pub fn sort_key(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.digits[0].csv_value(),
            self.digits[1].csv_value(),
            self.digits[2].csv_value(),
            self.digits[3].csv_value(),
            self.dp_after
                .map(|n| n.to_string())
                .unwrap_or_else(|| "_".to_owned())
        )
    }
}

fn parse_lcd_digit_field(field: &str) -> Option<LcdDigit> {
    match field.trim() {
        "_" | "-" | "" => Some(LcdDigit::Off),
        "0" => Some(LcdDigit::D0),
        "1" => Some(LcdDigit::D1),
        "2" => Some(LcdDigit::D2),
        "3" => Some(LcdDigit::D3),
        "4" => Some(LcdDigit::D4),
        "5" => Some(LcdDigit::D5),
        "6" => Some(LcdDigit::D6),
        "7" => Some(LcdDigit::D7),
        "8" => Some(LcdDigit::D8),
        "9" => Some(LcdDigit::D9),
        "L" | "l" => Some(LcdDigit::L),
        _ => None,
    }
}

fn parse_dp_after_field(field: &str) -> Option<Option<u8>> {
    match field.trim() {
        "_" | "-" | "" => Some(None),
        "0" => Some(Some(0)),
        "1" => Some(Some(1)),
        "2" => Some(Some(2)),
        _ => None,
    }
}

/// Full user-declared meter state for one labeled capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Victor86bcdCaptureContext {
    pub function: Victor86bcdCaptureFunction,
    /// LCD unit annunciator (V, mV, µA, …).
    pub unit: Victor86bcdCaptureUnit,
    /// Manual fixed decimal vs autorange.
    pub dp_mode: Victor86bcdCaptureDpMode,
    /// Four LCD digit slots + decimal placement.
    pub display: LcdDisplay,
    /// Optional extras: REL, MAX/MIN, hold, probe notes, etc.
    pub notes: String,
}

impl Default for Victor86bcdCaptureContext {
    fn default() -> Self {
        Self {
            function: Victor86bcdCaptureFunction::default(),
            unit: Victor86bcdCaptureFunction::default().default_unit(),
            dp_mode: Victor86bcdCaptureDpMode::default(),
            display: LcdDisplay::default(),
            notes: String::new(),
        }
    }
}

impl Victor86bcdCaptureContext {
    /// Short label for status messages and explore output.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "{} {} {}",
            self.function.label(),
            self.unit.label(),
            self.dp_mode.label()
        )];
        parts.push(self.display.format());
        if !self.notes.trim().is_empty() {
            parts.push(format!("({})", self.notes.trim()));
        }
        parts.join(" ")
    }

    /// Unique key for offline tools (same digits in different unit/DP mode stay distinct).
    pub fn sample_key(&self) -> String {
        let notes = self.notes.trim();
        if notes.is_empty() {
            format!(
                "{}|{}|{}|{}",
                self.function.label(),
                self.unit.label(),
                self.dp_mode.csv_value(),
                self.display.sort_key()
            )
        } else {
            format!(
                "{}|{}|{}|{}|{}",
                self.function.label(),
                self.unit.label(),
                self.dp_mode.csv_value(),
                self.display.sort_key(),
                notes
            )
        }
    }
}

/// Capture request: record every byte received for `duration_ms`.
#[derive(Debug, Clone)]
pub struct Victor86bcdCaptureJob {
    pub context: Victor86bcdCaptureContext,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Victor86bcdCaptureStatus {
    pub message: String,
    pub bytes_written: usize,
}

pub const SAMPLES_CSV_HEADER: &str =
    "timestamp,function,unit,dp_mode,d0,d1,d2,d3,dp_after,notes,duration_ms,byte_count,raw_hex";

pub fn default_samples_path() -> PathBuf {
    PathBuf::from("data/victor_serial/victor_serial_samples.csv")
}

fn csv_field(value: &str) -> String {
    if value.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn write_csv_row(
    file: &mut impl Write,
    timestamp: &str,
    context: &Victor86bcdCaptureContext,
    duration_ms: u64,
    raw: &[u8],
) -> io::Result<()> {
    let ts = timestamp;
    let hex = victor_dm1107::hex_encode(raw);
    let dp_after = context
        .display
        .dp_after
        .map(|n| n.to_string())
        .unwrap_or_else(|| "_".to_owned());
    writeln!(
        file,
        "{ts},{},{},{},{},{},{},{},{},{},{duration_ms},{},{}",
        context.function.label(),
        context.unit.label(),
        context.dp_mode.csv_value(),
        context.display.digits[0].csv_value(),
        context.display.digits[1].csv_value(),
        context.display.digits[2].csv_value(),
        context.display.digits[3].csv_value(),
        dp_after,
        csv_field(context.notes.trim()),
        raw.len(),
        csv_hex_field(&hex),
    )
}

fn csv_hex_field(hex: &str) -> String {
    format!("\"{}\"", hex.replace('"', "\"\""))
}

pub fn append_labeled_capture(
    path: &Path,
    context: &Victor86bcdCaptureContext,
    duration_ms: u64,
    raw: &[u8],
) -> io::Result<usize> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let needs_header = !path.exists() || path.metadata()?.len() == 0;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    if needs_header {
        writeln!(file, "{SAMPLES_CSV_HEADER}")?;
    }
    let ts = Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();
    write_csv_row(&mut file, &ts, context, duration_ms, raw)?;
    Ok(raw.len())
}

/// One parsed CSV row (current structured LCD format only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Victor86bcdCaptureRow {
    pub timestamp: String,
    pub context: Victor86bcdCaptureContext,
    pub duration_ms: u64,
    pub raw: Vec<u8>,
}

fn parse_data_line(line: &str) -> Option<Victor86bcdCaptureRow> {
    let (prefix, tail) = split_tail_fields(line, 3)?;
    let duration_ms: u64 = tail[0].parse().ok()?;
    let _byte_count: usize = tail[1].parse().ok()?;
    let raw = parse_hex_field(tail[2]);
    let mut parts: Vec<&str> = prefix.split(',').collect();
    while parts.last().is_some_and(|s| s.is_empty()) {
        parts.pop();
    }
    if parts.len() < 10 {
        return None;
    }
    let function = parse_function_label(parts[1]);
    let notes = if parts.len() >= 10 {
        parts[9].to_owned()
    } else {
        String::new()
    };
    let context = Victor86bcdCaptureContext {
        function,
        unit: parse_unit_label(parts[2]).unwrap_or_else(|| function.default_unit()),
        dp_mode: parse_dp_mode_label(parts[3]),
        display: LcdDisplay {
            digits: [
                parse_lcd_digit_field(parts[4])?,
                parse_lcd_digit_field(parts[5])?,
                parse_lcd_digit_field(parts[6])?,
                parse_lcd_digit_field(parts[7])?,
            ],
            dp_after: parse_dp_after_field(parts[8])?,
        },
        notes,
    };
    Some(Victor86bcdCaptureRow {
        timestamp: parts[0].to_owned(),
        context,
        duration_ms,
        raw,
    })
}

fn split_tail_fields(line: &str, n: usize) -> Option<(&str, Vec<&str>)> {
    let mut rest = line;
    let mut tail = Vec::with_capacity(n);
    for _ in 0..n {
        let (left, right) = rest.rsplit_once(',')?;
        tail.push(right);
        rest = left;
    }
    tail.reverse();
    Some((rest, tail))
}

pub fn parse_samples_csv(text: &str) -> Vec<Victor86bcdCaptureRow> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    if header.trim().is_empty() {
        return Vec::new();
    }

    let mut rows = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(row) = parse_data_line(line) {
            rows.push(row);
        }
    }
    rows
}

/// Latest row per [`Victor86bcdCaptureContext::sample_key`].
pub fn latest_samples_by_key(text: &str) -> Vec<Victor86bcdCaptureRow> {
    let mut latest = std::collections::HashMap::new();
    for row in parse_samples_csv(text) {
        latest.insert(row.context.sample_key(), row);
    }
    let mut out: Vec<_> = latest.into_values().collect();
    out.sort_by(|a, b| {
        a.context
            .display
            .sort_key()
            .cmp(&b.context.display.sort_key())
    });
    out
}

fn parse_hex_field(field: &str) -> Vec<u8> {
    let trimmed = field.trim().trim_matches('"');
    trimmed
        .split_whitespace()
        .filter_map(|t| u8::from_str_radix(t, 16).ok())
        .collect()
}

fn parse_function_label(label: &str) -> Victor86bcdCaptureFunction {
    match label.trim().to_ascii_uppercase().as_str() {
        "VDC" => Victor86bcdCaptureFunction::Vdc,
        "VAC" => Victor86bcdCaptureFunction::Vac,
        "ADC" => Victor86bcdCaptureFunction::Adc,
        "AAC" => Victor86bcdCaptureFunction::Aac,
        "RES" => Victor86bcdCaptureFunction::Res,
        "CAP" => Victor86bcdCaptureFunction::Cap,
        "FREQ" => Victor86bcdCaptureFunction::Freq,
        "PER" => Victor86bcdCaptureFunction::Per,
        "DUTY" => Victor86bcdCaptureFunction::Duty,
        "DIOD" => Victor86bcdCaptureFunction::Diod,
        "CONT" => Victor86bcdCaptureFunction::Cont,
        "TEMP" => Victor86bcdCaptureFunction::Temp,
        _ => Victor86bcdCaptureFunction::Other,
    }
}

fn parse_dp_mode_label(label: &str) -> Victor86bcdCaptureDpMode {
    match label.trim().to_ascii_lowercase().as_str() {
        "auto" | "auto dp" => Victor86bcdCaptureDpMode::Auto,
        _ => Victor86bcdCaptureDpMode::Manual,
    }
}

fn parse_unit_label(label: &str) -> Option<Victor86bcdCaptureUnit> {
    match label.trim() {
        "V" => Some(Victor86bcdCaptureUnit::V),
        "mV" => Some(Victor86bcdCaptureUnit::mV),
        "A" => Some(Victor86bcdCaptureUnit::A),
        "mA" => Some(Victor86bcdCaptureUnit::mA),
        "uA" | "µA" => Some(Victor86bcdCaptureUnit::uA),
        "Ω" | "Ohm" => Some(Victor86bcdCaptureUnit::Ohm),
        "kΩ" | "kOhm" => Some(Victor86bcdCaptureUnit::kOhm),
        "MΩ" | "MOhm" => Some(Victor86bcdCaptureUnit::MOhm),
        "nF" => Some(Victor86bcdCaptureUnit::Nf),
        "µF" | "uF" => Some(Victor86bcdCaptureUnit::Uf),
        "Hz" => Some(Victor86bcdCaptureUnit::Hz),
        "%" => Some(Victor86bcdCaptureUnit::Percent),
        "°C" | "C" => Some(Victor86bcdCaptureUnit::Celsius),
        "°F" | "F" => Some(Victor86bcdCaptureUnit::Fahrenheit),
        "" => None,
        _ => Some(Victor86bcdCaptureUnit::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_parses_structured_lcd_csv() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("samples.csv");
        let context = Victor86bcdCaptureContext {
            function: Victor86bcdCaptureFunction::Vdc,
            unit: Victor86bcdCaptureUnit::mV,
            dp_mode: Victor86bcdCaptureDpMode::Manual,
            display: LcdDisplay {
                digits: [LcdDigit::D0, LcdDigit::D0, LcdDigit::D0, LcdDigit::D0],
                dp_after: Some(1),
            },
            notes: "REL on".to_owned(),
        };
        append_labeled_capture(&path, &context, 500, &[0xa5, 0x12]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(",0,0,0,0,1,REL on,"));
        let rows = parse_samples_csv(&text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].context, context);
        assert_eq!(rows[0].duration_ms, 500);
        assert_eq!(rows[0].raw, vec![0xa5, 0x12]);
    }

    #[test]
    fn structured_lcd_distinguishes_off_from_zero() {
        let zero = LcdDisplay {
            digits: [LcdDigit::D0, LcdDigit::D0, LcdDigit::D0, LcdDigit::D0],
            dp_after: Some(1),
        };
        let off = LcdDisplay {
            digits: [LcdDigit::Off, LcdDigit::Off, LcdDigit::D0, LcdDigit::D0],
            dp_after: Some(1),
        };
        assert_eq!(zero.format(), "00.00");
        assert_eq!(off.format(), "__.00");
        assert_ne!(zero.sort_key(), off.sort_key());
    }

    #[test]
    fn sample_key_distinguishes_unit_and_dp() {
        let a = Victor86bcdCaptureContext {
            function: Victor86bcdCaptureFunction::Vdc,
            unit: Victor86bcdCaptureUnit::V,
            dp_mode: Victor86bcdCaptureDpMode::Manual,
            display: LcdDisplay {
                digits: [LcdDigit::D0, LcdDigit::D0, LcdDigit::D0, LcdDigit::D0],
                dp_after: Some(1),
            },
            notes: String::new(),
        };
        let b = Victor86bcdCaptureContext {
            function: Victor86bcdCaptureFunction::Vdc,
            unit: Victor86bcdCaptureUnit::mV,
            dp_mode: Victor86bcdCaptureDpMode::Manual,
            display: LcdDisplay {
                digits: [LcdDigit::D0, LcdDigit::D0, LcdDigit::D0, LcdDigit::D0],
                dp_after: Some(1),
            },
            notes: String::new(),
        };
        assert_ne!(a.sample_key(), b.sample_key());
    }

    #[test]
    fn default_samples_path_uses_victor_serial_subdir() {
        assert_eq!(
            default_samples_path(),
            PathBuf::from("data/victor_serial/victor_serial_samples.csv")
        );
    }
}
