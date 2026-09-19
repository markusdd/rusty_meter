use crate::multimeter::MeterMode;

/// Compact XDM1041/1241 `MEAS?` open-line value, and the Victor OL flag.
/// This is *not* a universal ceiling: 1 GΩ / 1 GHz are valid on other meters.
pub const METER_OVERLOAD_VALUE: f64 = 1e9;

/// Firmware sentinels such as XDM1051 `~1e31` / Keysight `9.9e37`.
/// Below this, GΩ and GHz readings must still graph as numbers.
pub const SCPI_OVERLOAD_MAGNITUDE: f64 = 1e20;

/// Open/OL: huge SCPI sentinels in any mode, or the 1041 `1e9` flag in ohms-family modes.
pub fn is_meter_overload(value: f64, mode: MeterMode) -> bool {
    if !value.is_finite() {
        return true;
    }
    let mag = value.abs();
    if mag >= SCPI_OVERLOAD_MAGNITUDE {
        return true;
    }
    mag == METER_OVERLOAD_VALUE
        && matches!(mode, MeterMode::Diod | MeterMode::Cont | MeterMode::Res)
}

pub fn format_measurement(
    value: f64,
    max_digits: usize,
    sci_threshold_high: f64,
    sci_threshold_low: f64,
    meter_mode: &MeterMode,
    auto_scale_units: bool,
    lcd_override: Option<(&str, &str)>,
) -> (String, String) {
    if value.is_nan() {
        return ("    NaN".to_string(), "".to_string());
    }

    if is_meter_overload(value, *meter_mode) {
        return ("OVERLOAD".to_string(), "".to_string());
    }

    // Victor 6000-count meters: show wire-decoded LCD text (4 digits), unit from annunciator.
    if let Some((lcd, unit)) = lcd_override {
        if !lcd.is_empty() {
            return (
                format!("{:>width$}", lcd, width = max_digits),
                unit.to_string(),
            );
        }
    }

    let abs_value = value.abs();
    let mut display_value = value;
    let mut display_unit = meter_mode.default_unit().to_string();

    // Adjust value and unit based on mode and magnitude
    if auto_scale_units {
        match meter_mode {
            MeterMode::Vdc | MeterMode::Vac if abs_value < 1.0 => {
                display_value = value * 1000.0;
                display_unit = if matches!(meter_mode, MeterMode::Vdc) {
                    "mVDC"
                } else {
                    "mVAC"
                }
                .to_string();
            }
            MeterMode::Adc | MeterMode::Aac if abs_value < 1.0 => {
                display_value = value * 1000.0;
                display_unit = if matches!(meter_mode, MeterMode::Adc) {
                    "mADC"
                } else {
                    "mAAC"
                }
                .to_string();
            }
            MeterMode::Res | MeterMode::Cont => {
                if abs_value >= 1_000_000.0 {
                    display_value = value / 1_000_000.0;
                    display_unit = "MOhm".to_string();
                } else if abs_value >= 1_000.0 {
                    display_value = value / 1_000.0;
                    display_unit = "kOhm".to_string();
                } else if abs_value < 1.0 && abs_value > 0.0 {
                    display_value = value * 1000.0;
                    display_unit = "mOhm".to_string();
                }
            }
            MeterMode::Cap => {
                if abs_value >= 0.001 {
                    display_value = value * 1000.0;
                    display_unit = "mF".to_string();
                } else if abs_value >= 0.000_001 {
                    display_value = value * 1_000_000.0;
                    display_unit = "μF".to_string();
                } else if abs_value > 0.0 {
                    display_value = value * 1_000_000_000.0;
                    display_unit = "nF".to_string();
                }
            }
            MeterMode::Per if abs_value < 1.0 => {
                display_value = value * 1000.0;
                display_unit = "ms".to_string();
            }
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multimeter::MeterMode;

    #[test]
    fn xdm1041_1e9_is_ol_only_in_ohms_family() {
        assert!(is_meter_overload(1e9, MeterMode::Res));
        assert!(is_meter_overload(1e9, MeterMode::Cont));
        assert!(is_meter_overload(1e9, MeterMode::Diod));
        assert!(!is_meter_overload(1e9, MeterMode::Freq));
        assert!(!is_meter_overload(1e9, MeterMode::Vdc));
        assert!(!is_meter_overload(1e10, MeterMode::Res));
        assert!(!is_meter_overload(50e6, MeterMode::Res));
    }

    #[test]
    fn huge_scpi_sentinels_are_ol_in_every_mode() {
        for mode in MeterMode::ALL {
            assert!(is_meter_overload(9.9e31, mode));
            assert!(is_meter_overload(-9.9e37, mode));
            assert!(is_meter_overload(f64::INFINITY, mode));
            let (text, unit) = format_measurement(9.9e31, 10, 1e6, 1e-6, &mode, true, None);
            assert_eq!(text, "OVERLOAD");
            assert_eq!(unit, "");
        }
    }

    #[test]
    fn gigahertz_is_not_overload() {
        let (text, unit) = format_measurement(1e9, 10, 1e12, 1e-6, &MeterMode::Freq, true, None);
        assert_ne!(text, "OVERLOAD");
        assert_eq!(unit, "Hz");
    }
}
