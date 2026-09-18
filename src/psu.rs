//! Single-channel SPE / Kiprim DC PSU dialect (`VOLT` / `CURR` / `OUTP` / `MEAS:ALL:INFO?`).
//!
//! Models share one SCPI dialect; they differ in voltage, current, and power ratings.

/// Operating mode from `MEAS:ALL:INFO?` (NR4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PsuRunMode {
    #[default]
    Standby,
    Cv,
    Cc,
    Fault,
}

impl PsuRunMode {
    pub fn from_token(s: &str) -> Option<Self> {
        match s.trim() {
            "0" => Some(Self::Standby),
            "1" => Some(Self::Cv),
            "2" => Some(Self::Cc),
            "3" => Some(Self::Fault),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Standby => "STANDBY",
            Self::Cv => "CV",
            Self::Cc => "CC",
            Self::Fault => "FAULT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PsuModel {
    pub idn_model: &'static str,
    pub brand: &'static str,
    pub v_max: f32,
    pub i_max: f32,
    pub p_max: f32,
}

/// OVP/OCP on SPE/Kiprim boxes sit ~5% above rated V/I (DC620S: 63 V / 21 A).
pub const PROT_HEADROOM: f32 = 1.05;

impl PsuModel {
    pub fn display_name(self) -> String {
        format!("{} {}", self.brand, self.idn_model)
    }

    /// Upper bound for the OVP slider (rated V × headroom).
    pub fn ovp_max(self) -> f32 {
        self.v_max * PROT_HEADROOM
    }

    /// Upper bound for the OCP slider (rated I × headroom).
    pub fn ocp_max(self) -> f32 {
        self.i_max * PROT_HEADROOM
    }

    /// Used when IDN matches SPE/Kiprim but the exact SKU is not in the table yet.
    pub const UNKNOWN: Self = Self {
        idn_model: "PSU",
        brand: "SPE/Kiprim",
        v_max: 60.0,
        i_max: 20.0,
        p_max: 400.0,
    };
}

impl Default for PsuModel {
    fn default() -> Self {
        Self::UNKNOWN
    }
}

/// Known SPE / Kiprim single-channel boxes. Ratings can be refined on the bench.
pub const PSU_MODELS: &[PsuModel] = &[
    PsuModel {
        idn_model: "DC620S",
        brand: "KIPRIM",
        v_max: 60.0,
        i_max: 20.0,
        p_max: 400.0,
    },
    PsuModel {
        idn_model: "DC610S",
        brand: "KIPRIM",
        v_max: 60.0,
        i_max: 10.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "DC605S",
        brand: "KIPRIM",
        v_max: 60.0,
        i_max: 5.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "DC310S",
        brand: "KIPRIM",
        v_max: 30.0,
        i_max: 10.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "SPE6103",
        brand: "OWON",
        v_max: 60.0,
        i_max: 10.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "SPE6053",
        brand: "OWON",
        v_max: 60.0,
        i_max: 5.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "SPE3103",
        brand: "OWON",
        v_max: 30.0,
        i_max: 10.0,
        p_max: 300.0,
    },
    PsuModel {
        idn_model: "SPE3102",
        brand: "OWON",
        v_max: 30.0,
        i_max: 10.0,
        p_max: 150.0,
    },
];

pub fn lookup_model(idn_model: &str) -> Option<PsuModel> {
    let m = idn_model.trim();
    PSU_MODELS
        .iter()
        .copied()
        .find(|row| row.idn_model.eq_ignore_ascii_case(m))
}

pub fn model_from_idn(idn: &str) -> PsuModel {
    let model = crate::scpi_macro::idn_model(idn);
    lookup_model(&model).unwrap_or(PsuModel::UNKNOWN)
}

pub fn meas_query() -> &'static str {
    "MEAS:ALL:INFO?\n"
}

pub fn set_volt(v: f32) -> String {
    format!("VOLT {v:.3}\n")
}

pub fn set_curr(i: f32) -> String {
    format!("CURR {i:.3}\n")
}

pub fn set_ovp(v: f32) -> String {
    format!("VOLT:LIM {v:.3}\n")
}

pub fn set_ocp(i: f32) -> String {
    format!("CURR:LIM {i:.3}\n")
}

pub fn set_output(on: bool) -> String {
    if on {
        "OUTP ON\n".to_owned()
    } else {
        "OUTP OFF\n".to_owned()
    }
}

/// One `MEAS:ALL:INFO?` (or `MEAS:ALL?`) line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PsuSample {
    pub volt: f64,
    pub curr: f64,
    pub power: f64,
    pub ovp_fault: bool,
    pub ocp_fault: bool,
    pub otp_fault: bool,
    pub run: PsuRunMode,
}

/// Setpoints from OUTP?/VOLT?/CURR?/LIM? in one GUI sync.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PsuStatus {
    pub output_on: Option<bool>,
    pub set_v: Option<f64>,
    pub set_i: Option<f64>,
    pub ovp: Option<f64>,
    pub ocp: Option<f64>,
}

/// Slider/text field has focus or is being dragged; skip that field on status.
#[derive(Clone, Copy, Debug, Default)]
pub struct PsuEditMask {
    pub set_v: bool,
    pub set_i: bool,
    pub ovp: bool,
    pub ocp: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PsuUpdate {
    Sample(PsuSample),
    Status(PsuStatus),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PsuLive {
    pub meas_v: f64,
    pub meas_i: f64,
    pub meas_p: f64,
    pub set_v: f32,
    pub set_i: f32,
    pub ovp: f32,
    pub ocp: f32,
    pub output_on: bool,
    pub run: PsuRunMode,
    pub ovp_fault: bool,
    pub ocp_fault: bool,
    pub otp_fault: bool,
}

impl Default for PsuLive {
    fn default() -> Self {
        Self {
            meas_v: f64::NAN,
            meas_i: f64::NAN,
            meas_p: f64::NAN,
            set_v: 0.0,
            set_i: 0.0,
            ovp: 0.0,
            ocp: 0.0,
            output_on: false,
            run: PsuRunMode::Standby,
            ovp_fault: false,
            ocp_fault: false,
            otp_fault: false,
        }
    }
}

impl PsuLive {
    pub fn apply_sample(&mut self, s: PsuSample) {
        self.meas_v = s.volt;
        self.meas_i = s.curr;
        self.meas_p = s.power;
        self.ovp_fault = s.ovp_fault;
        self.ocp_fault = s.ocp_fault;
        self.otp_fault = s.otp_fault;
        self.run = s.run;
    }

    pub fn apply_status(&mut self, st: PsuStatus, editing: PsuEditMask) {
        if let Some(on) = st.output_on {
            self.output_on = on;
        }
        if let Some(v) = st.set_v
            && !editing.set_v
        {
            self.set_v = v as f32;
        }
        if let Some(i) = st.set_i
            && !editing.set_i
        {
            self.set_i = i as f32;
        }
        if let Some(v) = st.ovp
            && !editing.ovp
        {
            self.ovp = v as f32;
        }
        if let Some(i) = st.ocp
            && !editing.ocp
        {
            self.ocp = i as f32;
        }
    }
}

fn parse_bool_token(s: &str) -> Option<bool> {
    match s.trim() {
        "1" | "ON" | "on" => Some(true),
        "0" | "OFF" | "off" => Some(false),
        _ => None,
    }
}

/// `MEAS:ALL:INFO?` → `V I P ovp ocp otp mode`. Also accepts `MEAS:ALL?` (`V I P`).
///
/// The programming manual shows space-separated fields; live DC620S firmware
/// replies comma-separated (`0.001,0.000,0.000,OFF,OFF,OFF,0`).
pub fn parse_meas_line(raw: &str) -> Option<PsuSample> {
    let t = raw.trim().trim_matches('"');
    let normalized = t.replace(',', " ");
    let mut parts = normalized.split_whitespace();
    let volt: f64 = parts.next()?.parse().ok()?;
    let curr: f64 = parts.next()?.parse().ok()?;
    let power: f64 = parts.next()?.parse().ok()?;
    let rest: Vec<&str> = parts.collect();
    if rest.is_empty() {
        return Some(PsuSample {
            volt,
            curr,
            power,
            ovp_fault: false,
            ocp_fault: false,
            otp_fault: false,
            run: PsuRunMode::Standby,
        });
    }
    if rest.len() < 4 {
        return None;
    }
    Some(PsuSample {
        volt,
        curr,
        power,
        ovp_fault: parse_bool_token(rest[0])?,
        ocp_fault: parse_bool_token(rest[1])?,
        otp_fault: parse_bool_token(rest[2])?,
        run: PsuRunMode::from_token(rest[3])?,
    })
}

pub fn parse_outp_reply(raw: &str) -> Option<bool> {
    parse_bool_token(raw.trim().trim_matches('"'))
}

pub fn parse_setpoint_reply(raw: &str) -> Option<f64> {
    raw.trim().trim_matches('"').parse().ok()
}

pub fn format_qty(value: f64, unit: &str) -> (String, String) {
    if !value.is_finite() {
        return ("    ---".to_owned(), unit.to_owned());
    }
    let abs = value.abs();
    if unit == "V" && abs < 1.0 {
        return (format!("{:>8.3}", value * 1000.0), "mV".to_owned());
    }
    if unit == "A" && abs < 1.0 {
        return (format!("{:>8.3}", value * 1000.0), "mA".to_owned());
    }
    if unit == "W" && abs < 1.0 {
        return (format!("{:>8.3}", value * 1000.0), "mW".to_owned());
    }
    (format!("{:>8.3}", value), unit.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_info() {
        let s = parse_meas_line("2.000 5.000 10.000 0 0 0 1").unwrap();
        assert_eq!(s.volt, 2.0);
        assert_eq!(s.curr, 5.0);
        assert_eq!(s.power, 10.0);
        assert!(!s.ovp_fault);
        assert_eq!(s.run, PsuRunMode::Cv);
    }

    #[test]
    fn parses_all_info_on_off_cc() {
        let s = parse_meas_line("12.0 0.5 6.0 OFF ON 0 2").unwrap();
        assert!(!s.ovp_fault);
        assert!(s.ocp_fault);
        assert_eq!(s.run, PsuRunMode::Cc);
    }

    #[test]
    fn parses_all_without_flags() {
        let s = parse_meas_line("1.000 2.000 2.000").unwrap();
        assert_eq!(s.curr, 2.0);
        assert_eq!(s.run, PsuRunMode::Standby);
    }

    #[test]
    fn parses_comma_separated_info() {
        let s = parse_meas_line("0.001,0.000,0.000,OFF,OFF,OFF,0").unwrap();
        assert_eq!(s.volt, 0.001);
        assert_eq!(s.curr, 0.0);
        assert_eq!(s.power, 0.0);
        assert!(!s.ovp_fault);
        assert!(!s.ocp_fault);
        assert!(!s.otp_fault);
        assert_eq!(s.run, PsuRunMode::Standby);
    }

    #[test]
    fn parses_comma_separated_all_without_flags() {
        let s = parse_meas_line("1.000,2.000,2.000").unwrap();
        assert_eq!(s.volt, 1.0);
        assert_eq!(s.curr, 2.0);
        assert_eq!(s.power, 2.0);
    }

    #[test]
    fn rejects_dmm_meas() {
        assert!(parse_meas_line("5.524573E-01").is_none());
        assert!(parse_meas_line("VOLT").is_none());
    }

    #[test]
    fn lookup_dc620s() {
        let m = lookup_model("DC620S").unwrap();
        assert_eq!(m.v_max, 60.0);
        assert_eq!(m.i_max, 20.0);
        assert_eq!(m.ovp_max(), 60.0 * PROT_HEADROOM);
        assert_eq!(m.ocp_max(), 20.0 * PROT_HEADROOM);
    }

    #[test]
    fn set_cmds_use_milli_resolution() {
        assert_eq!(set_volt(5.0), "VOLT 5.000\n");
        assert_eq!(set_curr(0.3), "CURR 0.300\n");
        assert_eq!(set_ovp(63.0), "VOLT:LIM 63.000\n");
        assert_eq!(set_ocp(21.0), "CURR:LIM 21.000\n");
    }

    #[test]
    fn status_skips_field_being_edited() {
        let mut live = PsuLive {
            set_i: 0.3,
            ..PsuLive::default()
        };
        live.apply_status(
            PsuStatus {
                set_i: Some(1.0),
                set_v: Some(12.0),
                ..PsuStatus::default()
            },
            PsuEditMask {
                set_i: true,
                ..PsuEditMask::default()
            },
        );
        assert!((live.set_i - 0.3).abs() < f32::EPSILON);
        assert!((live.set_v - 12.0).abs() < f32::EPSILON);
    }
}
