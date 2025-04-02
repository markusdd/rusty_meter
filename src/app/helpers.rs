use crate::app::MeterMode;

pub fn format_measurement(
    value: f64,
    max_digits: usize,
    sci_threshold_high: f64,
    sci_threshold_low: f64,
    meter_mode: &MeterMode,
) -> String {
    if value.is_nan() {
        return "    NaN".to_string();
    }

    // Check for overload/open condition (1e9) in specific modes
    if value == 1e9
        && matches!(
            meter_mode,
            MeterMode::Diod | MeterMode::Cont | MeterMode::Res
        )
    {
        return " OVERLOAD".to_string();
    }

    let abs_value = value.abs();

    if abs_value >= sci_threshold_high || (abs_value < sci_threshold_low && abs_value > 0.0) {
        format!("{:>width$.3e}", value, width = max_digits)
    } else {
        let precision = if abs_value >= 100.0 {
            1
        } else if abs_value >= 10.0 {
            2
        } else {
            3
        };
        format!("{:>width$.*}", precision, value, width = max_digits)
    }
}
