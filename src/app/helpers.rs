use crate::app::MeterMode;

pub fn format_measurement(
    value: f64,
    max_digits: usize,
    sci_threshold_high: f64,
    sci_threshold_low: f64,
    meter_mode: &MeterMode,
) -> (String, String) {
    if value.is_nan() {
        return ("    NaN".to_string(), "".to_string());
    }

    // Check for overload/open condition (1e9) in specific modes
    if value == 1e9
        && matches!(
            meter_mode,
            MeterMode::Diod | MeterMode::Cont | MeterMode::Res
        )
    {
        return ("OVERLOAD".to_string(), "".to_string());
    }

    let abs_value = value.abs();
    let mut display_value = value;
    let mut display_unit = match meter_mode {
        MeterMode::Vdc => "VDC",
        MeterMode::Vac => "VAC",
        MeterMode::Adc => "ADC",
        MeterMode::Aac => "AAC",
        MeterMode::Res => "Ohm",
        MeterMode::Cap => "F",
        MeterMode::Freq => "Hz",
        MeterMode::Per => "s",
        MeterMode::Diod => "V",
        MeterMode::Cont => "Ohm",
        MeterMode::Temp => "°C",
    }
    .to_string();

    // Adjust value and unit based on mode and magnitude
    match meter_mode {
        MeterMode::Vdc | MeterMode::Vac => {
            if abs_value < 1.0 {
                display_value = value * 1000.0;
                display_unit = if matches!(meter_mode, MeterMode::Vdc) {
                    "mVDC"
                } else {
                    "mVAC"
                }
                .to_string();
            }
        }
        MeterMode::Adc | MeterMode::Aac => {
            if abs_value < 1.0 {
                display_value = value * 1000.0;
                display_unit = if matches!(meter_mode, MeterMode::Adc) {
                    "mADC"
                } else {
                    "mAAC"
                }
                .to_string();
            }
        }
        MeterMode::Res => {
            if abs_value >= 1_000_000.0 {
                display_value = value / 1_000_000.0;
                display_unit = "MOhm".to_string();
            } else if abs_value >= 1_000.0 {
                display_value = value / 1_000.0;
                display_unit = "kOhm".to_string();
            } else if abs_value < 1.0 {
                display_value = value * 1000.0;
                display_unit = "mOhm".to_string();
            }
        }
        MeterMode::Per => {
            if abs_value < 1.0 {
                display_value = value * 1000.0;
                display_unit = "ms".to_string();
            }
        }
        _ => {}
    }

    let abs_display_value = display_value.abs();

    // Format the value
    let formatted_value = if abs_display_value >= sci_threshold_high
        || (abs_display_value < sci_threshold_low && abs_display_value > 0.0)
    {
        format!("{:>width$.3e}", display_value, width = max_digits)
    } else {
        let precision = if abs_display_value >= 1000.0 {
            2
        } else if abs_display_value >= 100.0 {
            3
        } else if abs_display_value >= 10.0 {
            4
        } else {
            5
        };
        format!("{:>width$.*}", precision, display_value, width = max_digits)
    };

    (formatted_value, display_unit)
}

pub fn powered_by(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label("Powered by ");
        ui.hyperlink_to("egui", "https://github.com/emilk/egui");
        ui.label(", ");
        ui.hyperlink_to(
            "eframe",
            "https://github.com/emilk/egui/tree/master/crates/eframe",
        );
        ui.label(", ");
        ui.hyperlink_to("B612 Font", "https://b612-font.com/");
        ui.label(" and ");
        ui.hyperlink_to(
            "TheHWCave",
            "https://github.com/TheHWcave/OWON-XDM1041/tree/main",
        );
        ui.label(".");
    });
}
