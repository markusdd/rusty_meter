//! SCPI macros: IDN family classification, connect bootstrap, parse/match.
//!
//! User-authored macros are persisted on [`crate::app::MyApp`]. The built-in
//! dialect bootstrap is generated from current UI settings and is not stored.

use serde::{Deserialize, Serialize};

/// SCPI dialect family inferred from `*IDN?`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScpiFamily {
    /// Compact / 3000-series OWON: `RATE`, `CONF:VOLT:DC 50`, `MEAS?`.
    OwonMeas,
    /// XDM6000 Keysight-like dialect. Bootstrap is a stub until that driver exists.
    OwonXdm6000,
    Unknown,
}

/// Which meters a user macro applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MacroTarget {
    AllScpi,
    #[default]
    OwonMeas,
    OwonXdm6000,
    /// Exact IDN model field, e.g. `"XDM1041"`.
    Model(String),
    /// Case-insensitive substring of the full IDN string.
    IdnContains(String),
}

impl MacroTarget {
    pub fn matches(&self, idn: &str) -> bool {
        let idn = idn.trim();
        if idn.is_empty() {
            return false;
        }
        match self {
            Self::AllScpi => true,
            Self::OwonMeas => classify_idn(idn) == ScpiFamily::OwonMeas,
            Self::OwonXdm6000 => classify_idn(idn) == ScpiFamily::OwonXdm6000,
            Self::Model(model) => idn_model(idn).eq_ignore_ascii_case(model.trim()),
            Self::IdnContains(needle) => {
                !needle.is_empty()
                    && idn
                        .to_ascii_lowercase()
                        .contains(&needle.to_ascii_lowercase())
            }
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::AllScpi => "All SCPI meters".to_owned(),
            Self::OwonMeas => "Owon XDM (MEAS-era)".to_owned(),
            Self::OwonXdm6000 => "Owon XDM 6000".to_owned(),
            Self::Model(m) if m.is_empty() => "This meter".to_owned(),
            Self::Model(m) => format!("This meter ({m})"),
            Self::IdnContains(_) => "Custom IDN substring".to_owned(),
        }
    }
}

/// A named, user-editable SCPI sequence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScpiMacro {
    pub id: String,
    pub name: String,
    pub body: String,
    #[serde(default)]
    pub applies_to: MacroTarget,
    #[serde(default)]
    pub run_on_connect: bool,
    #[serde(default)]
    pub show_as_button: bool,
}

impl ScpiMacro {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_macro_id(),
            name: name.into(),
            body: String::new(),
            applies_to: MacroTarget::default(),
            run_on_connect: false,
            show_as_button: true,
        }
    }
}

pub fn new_macro_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}")
}

/// UI settings replayed as the built-in connect bootstrap.
#[derive(Clone, Debug, PartialEq)]
pub struct BootstrapSettings {
    pub rate_opt: String,
    pub beeper_enabled: bool,
    pub cont_threshold: u32,
    pub diod_threshold: f32,
    pub lock_remote: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedMacro {
    pub commands: Vec<String>,
    pub skipped_queries: Vec<String>,
}

/// Second comma field of a standard `*IDN?` reply, or the whole string if there is no comma.
pub fn idn_model(idn: &str) -> String {
    let trimmed = idn.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut parts = trimmed.split(',');
    let first = parts.next().unwrap_or("").trim();
    match parts.next() {
        Some(model) => model.trim().to_owned(),
        None => first.to_owned(),
    }
}

pub fn classify_idn(idn: &str) -> ScpiFamily {
    classify_model(&idn_model(idn))
}

fn classify_model(model: &str) -> ScpiFamily {
    let m = model.trim().to_ascii_uppercase();
    if m.is_empty() {
        return ScpiFamily::Unknown;
    }
    if m.starts_with("XDM6") {
        return ScpiFamily::OwonXdm6000;
    }
    // 1041/1241/2041 plus the 3000 series still use MEAS?/RATE S|M|F.
    if m.starts_with("XDM1") || m.starts_with("XDM2") || m.starts_with("XDM3") {
        return ScpiFamily::OwonMeas;
    }
    ScpiFamily::Unknown
}

/// Meter key consumed by [`crate::multimeter::RangeCmd::new`]. Compact XDMs share the 1041 tables.
pub fn range_table_meter(idn: &str) -> String {
    match classify_idn(idn) {
        ScpiFamily::OwonMeas => "OWON XDM1041".to_owned(),
        ScpiFamily::OwonXdm6000 | ScpiFamily::Unknown => {
            let model = idn_model(idn);
            if model.is_empty() {
                "OWON XDM1041".to_owned()
            } else if model.to_ascii_uppercase().starts_with("XDM") {
                format!("OWON {model}")
            } else {
                model
            }
        }
    }
}

pub fn bootstrap_commands(family: ScpiFamily, s: &BootstrapSettings) -> Vec<String> {
    match family {
        ScpiFamily::OwonMeas => {
            let mut cmds = vec![
                format!("RATE {}\n", s.rate_opt),
                format!(
                    "SYST:BEEP:STATe {}\n",
                    if s.beeper_enabled { "ON" } else { "OFF" }
                ),
                format!("CONT:THREshold {}\n", s.cont_threshold),
                format!("DIOD:THREshold {}\n", s.diod_threshold),
            ];
            if s.lock_remote {
                cmds.push("SYST:REM\n".to_owned());
            }
            cmds
        }
        // Filled in when the XDM6000 driver lands. Empty so we never send RATE S/M/F.
        ScpiFamily::OwonXdm6000 | ScpiFamily::Unknown => Vec::new(),
    }
}

/// Split a user-edited body into wire commands. Queries are dropped so `MEAS?` / `FUNC?`
/// cannot desync the serial task's `ScpiMode`.
pub fn parse_macro_body(body: &str) -> ParsedMacro {
    let mut parsed = ParsedMacro::default();
    for raw_line in body.lines() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        for part in line.split(';') {
            let cmd = part.trim();
            if cmd.is_empty() {
                continue;
            }
            if is_query(cmd) {
                parsed.skipped_queries.push(cmd.to_owned());
                continue;
            }
            parsed.commands.push(ensure_newline(cmd));
        }
    }
    parsed
}

pub fn ensure_newline(cmd: &str) -> String {
    let t = cmd.trim();
    if t.ends_with('\n') {
        t.to_owned()
    } else {
        format!("{t}\n")
    }
}

pub fn is_query(cmd: &str) -> bool {
    cmd.trim().trim_end_matches(['\r', '\n']).ends_with('?')
}

/// User-facing SCPI that the recorder should keep. Session/poll traffic is excluded.
pub fn is_recordable_scpi(cmd: &str) -> bool {
    let t = cmd
        .trim()
        .trim_end_matches(['\r', '\n'])
        .trim()
        .to_ascii_uppercase()
        .replace(' ', "");
    if t.is_empty() || t.ends_with('?') {
        return false;
    }
    if t == "*RST" || t == "*CLS" {
        return false;
    }
    if t.starts_with("SYST:REM") || t.starts_with("SYST:LOC") {
        return false;
    }
    true
}

fn strip_comment(line: &str) -> &str {
    let slash = line.find("//");
    let hash = line.find('#');
    match (slash, hash) {
        (Some(a), Some(b)) => &line[..a.min(b)],
        (Some(a), None) => &line[..a],
        (None, Some(b)) => &line[..b],
        (None, None) => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idn_model_from_standard_reply() {
        assert_eq!(idn_model("OWON,XDM1041,12345,V4.8.0"), "XDM1041");
        assert_eq!(idn_model("  OWON , XDM1241 , x , y  "), "XDM1241");
        assert_eq!(idn_model("XDM1041"), "XDM1041");
        assert_eq!(idn_model(""), "");
    }

    #[test]
    fn classify_meas_era_and_6000() {
        assert_eq!(classify_idn("OWON,XDM1041,s,V4.8.0"), ScpiFamily::OwonMeas);
        assert_eq!(classify_idn("OWON,XDM1241,s,v"), ScpiFamily::OwonMeas);
        assert_eq!(classify_idn("OWON,XDM2041,s,v"), ScpiFamily::OwonMeas);
        assert_eq!(classify_idn("OWON,XDM3051,s,v"), ScpiFamily::OwonMeas);
        assert_eq!(classify_idn("OWON,XDM6000,s,v"), ScpiFamily::OwonXdm6000);
        assert_eq!(classify_idn("OWON,XDM6241,s,v"), ScpiFamily::OwonXdm6000);
        assert_eq!(classify_idn("KEYSIGHT,34465A,s,v"), ScpiFamily::Unknown);
        assert_eq!(classify_idn(""), ScpiFamily::Unknown);
    }

    #[test]
    fn range_table_compact_maps_to_1041() {
        assert_eq!(range_table_meter("OWON,XDM1041,s,v"), "OWON XDM1041");
        assert_eq!(range_table_meter("OWON,XDM2041,s,v"), "OWON XDM1041");
    }

    fn settings() -> BootstrapSettings {
        BootstrapSettings {
            rate_opt: "F".into(),
            beeper_enabled: true,
            cont_threshold: 50,
            diod_threshold: 2.0,
            lock_remote: true,
        }
    }

    #[test]
    fn bootstrap_meas_replays_ui_settings() {
        let cmds = bootstrap_commands(ScpiFamily::OwonMeas, &settings());
        assert_eq!(
            cmds,
            vec![
                "RATE F\n".to_owned(),
                "SYST:BEEP:STATe ON\n".to_owned(),
                "CONT:THREshold 50\n".to_owned(),
                "DIOD:THREshold 2\n".to_owned(),
                "SYST:REM\n".to_owned(),
            ]
        );
    }

    #[test]
    fn bootstrap_meas_omits_rem_when_unlocked() {
        let mut s = settings();
        s.lock_remote = false;
        s.beeper_enabled = false;
        s.rate_opt = "S".into();
        let cmds = bootstrap_commands(ScpiFamily::OwonMeas, &s);
        assert_eq!(cmds[0], "RATE S\n");
        assert_eq!(cmds[1], "SYST:BEEP:STATe OFF\n");
        assert!(!cmds.iter().any(|c| c.contains("SYST:REM")));
    }

    #[test]
    fn bootstrap_6000_and_unknown_are_empty() {
        let s = settings();
        assert!(bootstrap_commands(ScpiFamily::OwonXdm6000, &s).is_empty());
        assert!(bootstrap_commands(ScpiFamily::Unknown, &s).is_empty());
    }

    #[test]
    fn parse_newlines_semicolons_comments() {
        let body = "\
CONF:VOLT:DC 50;RATE F
# a comment
CONF:RES 5E6 // trailing
CONF:VOLT:AC 500V
";
        let p = parse_macro_body(body);
        assert_eq!(
            p.commands,
            vec![
                "CONF:VOLT:DC 50\n",
                "RATE F\n",
                "CONF:RES 5E6\n",
                "CONF:VOLT:AC 500V\n",
            ]
        );
        assert!(p.skipped_queries.is_empty());
    }

    #[test]
    fn parse_drops_queries() {
        let p = parse_macro_body("CONF:VOLT:DC AUTO\nMEAS?\nFUNC?\nRATE S");
        assert_eq!(p.commands, vec!["CONF:VOLT:DC AUTO\n", "RATE S\n"]);
        assert_eq!(p.skipped_queries, vec!["MEAS?", "FUNC?"]);
    }

    #[test]
    fn parse_issue18_one_liner() {
        let p = parse_macro_body("CONFigure:VOLT:DC 50;RATE F;CONF:RES 5E6;CONFigure:VOLT:AC 500V");
        assert_eq!(p.commands.len(), 4);
        assert_eq!(p.commands[0], "CONFigure:VOLT:DC 50\n");
        assert_eq!(p.commands[3], "CONFigure:VOLT:AC 500V\n");
    }

    #[test]
    fn recordable_filters_session_and_poll() {
        assert!(is_recordable_scpi("CONF:VOLT:DC 50\n"));
        assert!(is_recordable_scpi("RATE F"));
        assert!(!is_recordable_scpi("MEAS?\n"));
        assert!(!is_recordable_scpi("*IDN?\n"));
        assert!(!is_recordable_scpi("FUNC?"));
        assert!(!is_recordable_scpi("SYST:REM\n"));
        assert!(!is_recordable_scpi("SYST:LOC\n"));
        assert!(!is_recordable_scpi("*RST\n"));
    }

    #[test]
    fn target_matching() {
        let idn = "OWON,XDM1041,abc,V4.8.0";
        assert!(MacroTarget::AllScpi.matches(idn));
        assert!(MacroTarget::OwonMeas.matches(idn));
        assert!(!MacroTarget::OwonXdm6000.matches(idn));
        assert!(MacroTarget::Model("XDM1041".into()).matches(idn));
        assert!(!MacroTarget::Model("XDM6000".into()).matches(idn));
        assert!(MacroTarget::IdnContains("xdm1041".into()).matches(idn));
        assert!(!MacroTarget::IdnContains("xdm6".into()).matches(idn));
        assert!(!MacroTarget::AllScpi.matches(""));
        assert!(MacroTarget::OwonXdm6000.matches("OWON,XDM6241,s,v"));
    }
}
