//! Offline analysis of Victor 86B/C/D DM1107 serial captures.
//!
//! ```text
//! cargo run --bin victor-86bcd-explore
//! ```

use std::env;

use rusty_meter::victor_86bcd_capture;
use rusty_meter::victor_dm1107::{self, MeterFunction, VictorFrame};

struct Sample {
    label: String,
    display: rusty_meter::victor_86bcd_capture::LcdDisplay,
    raw: Vec<u8>,
}

fn load_latest(path: &str) -> Vec<Sample> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    victor_86bcd_capture::latest_samples_by_key(&text)
        .into_iter()
        .map(|row| Sample {
            label: row.context.summary(),
            display: row.context.display,
            raw: row.raw,
        })
        .collect()
}

fn function_label(f: MeterFunction) -> &'static str {
    match f {
        MeterFunction::Vdc => "VDC",
        MeterFunction::Vac => "VAC",
        MeterFunction::Adc => "ADC",
        MeterFunction::Aac => "AAC",
        MeterFunction::Res => "RES",
        MeterFunction::Cap => "CAP",
        MeterFunction::Freq => "FREQ",
        MeterFunction::Duty => "DUTY",
        MeterFunction::Diod => "DIOD",
        MeterFunction::Cont => "CONT",
        MeterFunction::Temp => "TEMP",
        MeterFunction::Unknown => "?",
    }
}

fn digit_hex(frame: &VictorFrame) -> String {
    frame
        .digits
        .iter()
        .map(|d| format!("{:02x}", d.raw))
        .collect::<Vec<_>>()
        .join(" ")
}

fn main() {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "data/victor_serial/victor_serial_samples.csv".to_owned());

    let samples = load_latest(&path);

    println!("=== Victor 86B/C/D DM1107 serial, 20-byte `a5 12` frames ===");
    println!("File: {path} ({} unique captures)\n", samples.len());

    println!("--- Frame decode ---");
    println!(
        "{:<32}  {:>6}   mode        decode (labeled)",
        "context", "labeled"
    );

    let mut framed = 0usize;
    let mut decoded_ok = 0usize;
    for sample in &samples {
        let Some(frame) = victor_dm1107::find_frame(&sample.raw) else {
            println!("{:<32}  (no a5 12 frame)", sample.label);
            continue;
        };
        framed += 1;
        let reading = victor_dm1107::decode_frame(&frame);
        let mode = format!(
            "{} {:?} {:?}",
            function_label(frame.decoded_mode.function),
            frame.decoded_mode.unit,
            frame.decoded_mode.dp_mode,
        );
        let labeled = sample.display.format();
        let mark = if reading.text == labeled
            || (reading.text.replace('_', "") == labeled.replace('_', ""))
        {
            decoded_ok += 1;
            "ok"
        } else if reading.text.is_empty() {
            "??"
        } else {
            "~~"
        };
        println!(
            "{:<32}  {:>6}   {:<12}  {} ({mark}, conf={})  wire={}",
            sample.label,
            labeled,
            mode,
            reading.text,
            reading.confidence,
            digit_hex(&frame),
        );
    }
    println!(
        "({framed}/{} framed, {decoded_ok}/{} exact text match)\n",
        samples.len(),
        samples.len()
    );

    println!("--- Protocol ---");
    println!("  • DM1107 meter IC → opto isolation → CP2102 USB serial (9600 8N1).");
    println!("  • 20-byte repeating frames: `a5 12` sync, 3-byte mode header, 4 digit bytes, tail `.. 04 ..`.");
    println!("  • Digit bytes: low 7 bits = segment map, bit 7 = decimal point (positions 0–2).");
    println!("  • `0x5f` = lit zero; `0x00` = off; `0x80` = DP only; `0xdf` = zero + DP.");
    println!("  • Mode annunciators are single bits across bytes 2–4 and 9 (V/m/DC/−/AUTO/…).");
}