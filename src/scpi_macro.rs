//! SCPI macros: IDN family classification, connect bootstrap, parse/match.
//!
//! User-authored macros are persisted on [`crate::app::MyApp`]. The built-in
//! dialect bootstrap is generated from current UI settings and is not stored.

use serde::{Deserialize, Serialize};

use crate::multimeter::MeterMode;

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
            Self::OwonMeas => "Owon XDM 1/2/3xxx".to_owned(),
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

/// One FUNC/RATE/BEEP/AUTO(/RANGE) poll, applied atomically.
///
/// Compact Owon `RANGE?` returns the live window even in autorange (`50 mV` on
/// a shorted VDC input). That string must not be treated as a manual range
/// unless `AUTO?` is 0 in the **same** snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterStatus {
    pub rate: Option<String>,
    pub beep: Option<bool>,
    pub auto: Option<bool>,
    pub range: Option<String>,
}

/// UI range implied by one [`MeterStatus`] snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotRange {
    Auto,
    Manual(String),
}

/// `None` = this snapshot does not know AUTO, so leave the UI range alone.
pub fn snapshot_range(auto: Option<bool>, range: Option<&str>) -> Option<SnapshotRange> {
    match auto {
        Some(true) => Some(SnapshotRange::Auto),
        Some(false) => Some(SnapshotRange::Manual(range.unwrap_or("").trim().to_owned())),
        None => None,
    }
}

/// UI changes implied by a command we just sent (optimistic, before query).
#[derive(Clone, Debug, PartialEq)]
pub enum ScpiUiHint {
    Mode {
        mode: MeterMode,
        range_param: Option<String>,
    },
    Rate(String),
    Beep(bool),
    ContThreshold(u32),
    DiodThreshold(f32),
}

/// Best-effort parse of a compact-Owon set command into a UI hint.
pub fn ui_hint_from_command(cmd: &str) -> Option<ScpiUiHint> {
    let t = cmd.trim().trim_end_matches(['\r', '\n']).trim();
    if t.is_empty() || is_query(t) {
        return None;
    }
    let compact = t.replace(' ', "").to_ascii_uppercase();

    if let Some(rest) = compact.strip_prefix("RATE") {
        if !rest.is_empty() {
            return Some(ScpiUiHint::Rate(rest.to_owned()));
        }
    }
    if let Some(rest) = compact
        .strip_prefix("SYST:BEEP:STATE")
        .or_else(|| compact.strip_prefix("SYST:BEEP"))
    {
        return parse_beep_token(rest).map(ScpiUiHint::Beep);
    }
    if let Some(rest) = compact
        .strip_prefix("CONT:THRESHOLD")
        .or_else(|| compact.strip_prefix("CONT:THRE"))
    {
        return rest.parse::<u32>().ok().map(ScpiUiHint::ContThreshold);
    }
    if let Some(rest) = compact
        .strip_prefix("DIOD:THRESHOLD")
        .or_else(|| compact.strip_prefix("DIOD:THRE"))
    {
        return rest.parse::<f32>().ok().map(ScpiUiHint::DiodThreshold);
    }

    parse_conf_hint(&compact)
}

fn parse_beep_token(rest: &str) -> Option<bool> {
    let t = rest.trim().trim_matches('"');
    if t.eq_ignore_ascii_case("ON") || t.eq_ignore_ascii_case("NO") || t == "1" {
        // Compact Owon `SYST:BEEP:STATe?` is `ON` on some firmware and `NO` on
        // others (e.g. XDM1041 V4.2.0). Both mean enabled.
        Some(true)
    } else if t.eq_ignore_ascii_case("OFF") || t == "0" {
        Some(false)
    } else {
        None
    }
}

fn parse_conf_hint(compact: &str) -> Option<ScpiUiHint> {
    let rest = compact
        .strip_prefix("CONFIGURE:")
        .or_else(|| compact.strip_prefix("CONF:"))?;
    let rest = rest.strip_prefix("SCALAR:").unwrap_or(rest);

    let mut best: Option<(MeterMode, usize)> = None;
    for mode in MeterMode::ALL {
        for prefix in mode.conf_prefixes() {
            if rest.starts_with(prefix) && best.is_none_or(|(_, n)| prefix.len() > n) {
                best = Some((mode, prefix.len()));
            }
        }
    }
    let (mode, n) = best?;
    let param = rest[n..].trim_start_matches(':');
    let range_param = if param.is_empty() {
        None
    } else {
        Some(param.to_owned())
    };
    Some(ScpiUiHint::Mode { mode, range_param })
}

/// `*IDN?` replies contain commas / vendor text. Leftover `MEAS?` values parse as floats.
pub fn looks_like_idn(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    t.parse::<f64>().is_err()
}

pub fn parse_beep_reply(raw: &str) -> Option<bool> {
    parse_beep_token(raw.trim().trim_matches('"'))
}

/// Classify a reply by content so USB-batched / reordered lines still work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyClass {
    Func,
    Rate,
    Beep,
    Auto,
    Range,
    Meas,
    Unknown,
}

pub fn classify_reply(line: &str) -> ReplyClass {
    let u = line.trim().trim_matches('"');
    if u.is_empty() {
        return ReplyClass::Unknown;
    }
    if MeterMode::from_func_reply(u).is_some() {
        return ReplyClass::Func;
    }
    if matches!(u, "S" | "M" | "F") {
        return ReplyClass::Rate;
    }
    if u.eq_ignore_ascii_case("ON") || u.eq_ignore_ascii_case("NO") || u.eq_ignore_ascii_case("OFF")
    {
        return ReplyClass::Beep;
    }
    if u == "0" || u == "1" {
        return ReplyClass::Auto;
    }
    if parse_range_reply(u).is_some() {
        return ReplyClass::Range;
    }
    if u.parse::<f64>().is_ok() {
        return ReplyClass::Meas;
    }
    ReplyClass::Unknown
}

/// Compact Owon `RANGE?` is `50 V`, `5 V`, or a small index. Reject FUNC? / `MEAS?`.
pub fn parse_range_reply(raw: &str) -> Option<String> {
    let t = raw.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    if crate::multimeter::MeterMode::from_func_reply(t).is_some() {
        return None;
    }
    let compact = t.replace(' ', "");
    if compact.contains(['E', 'e']) {
        return None;
    }
    if compact.parse::<f64>().is_ok() && compact.contains('.') {
        return None;
    }
    if !compact.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(t.to_owned())
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
    use crate::multimeter::MeterMode;

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

    #[test]
    fn hints_from_issue18_and_settings() {
        assert_eq!(
            ui_hint_from_command("CONFigure:VOLT:DC 50"),
            Some(ScpiUiHint::Mode {
                mode: MeterMode::Vdc,
                range_param: Some("50".into()),
            })
        );
        assert_eq!(
            ui_hint_from_command("CONF:VOLT:AC 500V"),
            Some(ScpiUiHint::Mode {
                mode: MeterMode::Vac,
                range_param: Some("500V".into()),
            })
        );
        assert_eq!(
            ui_hint_from_command("CONF:RES 5E6"),
            Some(ScpiUiHint::Mode {
                mode: MeterMode::Res,
                range_param: Some("5E6".into()),
            })
        );
        assert_eq!(
            ui_hint_from_command("RATE F"),
            Some(ScpiUiHint::Rate("F".into()))
        );
        assert_eq!(
            ui_hint_from_command("SYST:BEEP:STATe OFF"),
            Some(ScpiUiHint::Beep(false))
        );
        assert_eq!(
            ui_hint_from_command("CONT:THREshold 80"),
            Some(ScpiUiHint::ContThreshold(80))
        );
        assert_eq!(
            ui_hint_from_command("CONF:CONT"),
            Some(ScpiUiHint::Mode {
                mode: MeterMode::Cont,
                range_param: None,
            })
        );
    }

    #[test]
    fn range_cmd_matches_meter_range_text() {
        let vdc =
            crate::multimeter::RangeCmd::new("OWON XDM1041", crate::multimeter::MeterMode::Vdc)
                .unwrap();
        assert_eq!(vdc.index_of_param("50 V"), Some(4));
        assert_eq!(vdc.index_of_param("50V"), Some(4));
        let vac =
            crate::multimeter::RangeCmd::new("OWON XDM1041", crate::multimeter::MeterMode::Vac)
                .unwrap();
        assert_eq!(vac.index_of_param("5 V"), Some(2));
        assert_eq!(vac.get_opt(0).0, "auto");
    }

    #[test]
    fn beep_reply_accepts_on_and_no() {
        assert_eq!(parse_beep_reply("ON"), Some(true));
        assert_eq!(parse_beep_reply("NO"), Some(true));
        assert_eq!(parse_beep_reply("no"), Some(true));
        assert_eq!(parse_beep_reply("OFF"), Some(false));
        assert_eq!(parse_beep_reply("1"), Some(true));
        assert_eq!(parse_beep_reply("0"), Some(false));
        assert_eq!(parse_beep_reply("VOLT"), None);
    }

    #[test]
    fn classify_reply_uses_content_not_order() {
        assert_eq!(classify_reply("\"VOLT AC\""), ReplyClass::Func);
        assert_eq!(classify_reply("VOLT"), ReplyClass::Func);
        assert_eq!(classify_reply("F"), ReplyClass::Rate);
        assert_eq!(classify_reply("OFF"), ReplyClass::Beep);
        assert_eq!(classify_reply("ON"), ReplyClass::Beep);
        assert_eq!(classify_reply("NO"), ReplyClass::Beep);
        assert_eq!(classify_reply("1"), ReplyClass::Auto);
        assert_eq!(classify_reply("0"), ReplyClass::Auto);
        assert_eq!(classify_reply("50 mV"), ReplyClass::Range);
        assert_eq!(classify_reply("50 V"), ReplyClass::Range);
        assert_eq!(classify_reply("5.524573E-01"), ReplyClass::Meas);
        assert_eq!(classify_reply("1.0"), ReplyClass::Meas);
    }

    #[test]
    fn auto_range_snapshot_ignores_live_window() {
        assert_eq!(
            snapshot_range(Some(true), Some("50 mV")),
            Some(SnapshotRange::Auto)
        );
        assert_eq!(
            snapshot_range(Some(false), Some("50 V")),
            Some(SnapshotRange::Manual("50 V".into()))
        );
        assert_eq!(snapshot_range(None, Some("50 mV")), None);
    }

    #[test]
    fn parse_range_reply_rejects_meas() {
        assert_eq!(parse_range_reply("4"), Some("4".into()));
        assert_eq!(parse_range_reply("50 V"), Some("50 V".into()));
        assert_eq!(parse_range_reply("5 V"), Some("5 V".into()));
        assert_eq!(parse_range_reply("500 mV"), Some("500 mV".into()));
        assert_eq!(parse_range_reply("VOLT AC"), None);
        assert_eq!(parse_range_reply("F"), None);
        assert_eq!(parse_range_reply("OFF"), None);
        assert_eq!(parse_range_reply("+3.210000E-01"), None);
        assert_eq!(parse_range_reply("0.000000E+00"), None);
    }

    #[test]
    fn looks_like_idn_rejects_meas_floats() {
        assert!(looks_like_idn("OWON,XDM1041,abc,V4.8.0"));
        assert!(looks_like_idn("XDM1041"));
        assert!(!looks_like_idn("1.2345"));
        assert!(!looks_like_idn("+3.210000E-01"));
        assert!(!looks_like_idn(""));
    }
}
