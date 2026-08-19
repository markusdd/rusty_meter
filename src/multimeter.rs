use phf::{OrderedMap, phf_ordered_map};
use serde::{Deserialize, Serialize};

/// A trait that must be implemented for all SCPI command structs.
/// Gets passed the struct instance itself and the selected option name
/// and must return a complete SCPI command string (including newline)
/// that can be sent via serial or LXI to the target device.
pub trait GenScpi {
    fn gen_scpi(&self, opt_name: &str) -> String;
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ScpiMode {
    Idn,
    Meas,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash, Serialize, Deserialize)]
pub enum MeterMode {
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
}

impl MeterMode {
    pub const ALL: [Self; 12] = [
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
    ];

    pub fn default_unit(self) -> &'static str {
        match self {
            Self::Vdc => "VDC",
            Self::Vac => "VAC",
            Self::Adc => "ADC",
            Self::Aac => "AAC",
            Self::Res => "Ohm",
            Self::Cap => "F",
            Self::Freq => "Hz",
            Self::Per => "s",
            Self::Duty => "%",
            Self::Diod => "V",
            Self::Cont => "Ohm",
            Self::Temp => "°C",
        }
    }

    pub fn button_label(self) -> &'static str {
        match self {
            Self::Vdc => "VDC",
            Self::Vac => "VAC",
            Self::Adc => "ADC",
            Self::Aac => "AAC",
            Self::Res => "Ohm",
            Self::Cap => "C",
            Self::Freq => "Freq",
            Self::Per => "Period",
            Self::Duty => "Duty",
            Self::Diod => "Diode",
            Self::Cont => "Cont",
            Self::Temp => "Temp",
        }
    }

    /// Compact-Owon `CONF:` command used by the mode buttons.
    pub fn default_conf(self) -> &'static str {
        match self {
            Self::Vdc => "CONF:VOLT:DC AUTO\n",
            Self::Vac => "CONF:VOLT:AC AUTO\n",
            Self::Adc => "CONF:CURR:DC AUTO\n",
            Self::Aac => "CONF:CURR:AC AUTO\n",
            Self::Res => "CONF:RES AUTO\n",
            Self::Cap => "CONF:CAP AUTO\n",
            Self::Freq => "CONF:FREQ\n",
            Self::Per => "CONF:PER\n",
            Self::Duty => "",
            Self::Diod => "CONF:DIOD\n",
            Self::Cont => "CONF:CONT\n",
            Self::Temp => "CONF:TEMP:RTD PT100\n",
        }
    }

    pub fn with_beeper_threshold(self) -> bool {
        matches!(self, Self::Cont | Self::Diod)
    }

    /// Compact Owon has no `RANGE?` reply in CONT/DIOD/FREQ/PER.
    pub fn has_manual_range(self) -> bool {
        matches!(
            self,
            Self::Vdc | Self::Vac | Self::Adc | Self::Aac | Self::Res | Self::Cap | Self::Temp
        )
    }

    /// `FUNC?` tokens from MEAS-era Owons. DIOD/CONT swap is applied by the caller.
    pub fn from_func_reply(s: &str) -> Option<Self> {
        match s.trim().trim_matches('"') {
            "VOLT" => Some(Self::Vdc),
            "VOLT AC" => Some(Self::Vac),
            "CURR" => Some(Self::Adc),
            "CURR AC" => Some(Self::Aac),
            "RES" => Some(Self::Res),
            "CAP" => Some(Self::Cap),
            "FREQ" => Some(Self::Freq),
            "PER" => Some(Self::Per),
            "TEMP" => Some(Self::Temp),
            "DIOD" => Some(Self::Diod),
            "CONT" => Some(Self::Cont),
            _ => None,
        }
    }

    /// `CONF:` / `CONFigure:` suffixes, longest first, for parsing macros.
    pub fn conf_prefixes(self) -> &'static [&'static str] {
        match self {
            Self::Vdc => &["VOLTAGE:DC", "VOLT:DC"],
            Self::Vac => &["VOLTAGE:AC", "VOLT:AC"],
            Self::Adc => &["CURRENT:DC", "CURR:DC"],
            Self::Aac => &["CURRENT:AC", "CURR:AC"],
            Self::Res => &["RESISTANCE", "RES"],
            Self::Cap => &["CAPACITANCE", "CAP"],
            Self::Freq => &["FREQUENCY", "FREQ"],
            Self::Per => &["PERIOD", "PER"],
            Self::Temp => &["TEMPERATURE:RTD", "TEMP:RTD", "TEMP"],
            Self::Diod => &["DIODE", "DIOD"],
            Self::Cont => &["CONTINUITY", "CONT"],
            Self::Duty => &[],
        }
    }
}

pub struct RateCmd {
    scpi: &'static str,
    pub opts: OrderedMap<&'static str, &'static str>,
}

impl Default for RateCmd {
    // this corresponds to OWON XDM1041
    fn default() -> Self {
        Self {
            scpi: "RATE ",
            opts: phf_ordered_map! {
                "Slow" => "S",
                "Medium" => "M",
                "Fast" => "F",
            },
        }
    }
}

impl GenScpi for RateCmd {
    fn gen_scpi(&self, opt_name: &str) -> String {
        format!("{}{}\n", self.scpi, self.opts[opt_name])
    }
}

impl RateCmd {
    pub fn get_opt(&self, index: usize) -> (&'static str, &'static str) {
        let (key, value) = self.opts.index(index).unwrap();
        (*key, *value)
    }

    pub fn len(&self) -> usize {
        self.opts.len()
    }

    pub fn index_of_scpi(&self, raw: &str) -> Option<usize> {
        let r = raw.trim().trim_matches('"').to_ascii_uppercase();
        (0..self.len()).find(|&i| self.get_opt(i).1.eq_ignore_ascii_case(&r))
    }
}

pub struct RangeCmd {
    scpi: &'static str,
    pub opts: OrderedMap<&'static str, &'static str>,
}

impl Default for RangeCmd {
    // this corresponds to OWON XDM1041 VDC ranges
    fn default() -> Self {
        Self {
            scpi: "CONF:VOLT:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "50mV" => "50E-3",
                "500mV" => "500E-3",
                "5V" => "5",
                "50V" => "50",
                "500V" => "500",
                "1000V" => "1000",
            },
        }
    }
}

impl GenScpi for RangeCmd {
    fn gen_scpi(&self, opt_name: &str) -> String {
        format!("{}{}\n", self.scpi, self.opts[opt_name])
    }
}

impl RangeCmd {
    pub fn new(meter: &str, mode: MeterMode) -> Option<Self> {
        match (meter, mode) {
            ("OWON XDM1041", MeterMode::Vdc) => Some(Self::default()),
            ("OWON XDM1041", MeterMode::Vac) => Some(Self::owon_xdm1041_vac()),
            ("OWON XDM1041", MeterMode::Adc) => Some(Self::owon_xdm1041_adc()),
            ("OWON XDM1041", MeterMode::Aac) => Some(Self::owon_xdm1041_aac()),
            ("OWON XDM1041", MeterMode::Res) => Some(Self::owon_xdm1041_res()),
            ("OWON XDM1041", MeterMode::Cap) => Some(Self::owon_xdm1041_cap()),
            ("OWON XDM1041", MeterMode::Temp) => Some(Self::owon_xdm1041_temp()),
            _ => None,
        }
    }

    pub fn get_opt(&self, index: usize) -> (&'static str, &'static str) {
        let (key, value) = self.opts.index(index).unwrap();
        (*key, *value)
    }

    pub fn len(&self) -> usize {
        self.opts.len()
    }

    pub fn index_of_param(&self, raw: &str) -> Option<usize> {
        let raw = raw.trim().trim_matches('"');
        if raw.is_empty() {
            return None;
        }
        let upper = raw.to_ascii_uppercase();
        if upper == "AUTO" {
            return (0..self.len()).find(|&i| self.get_opt(i).1.eq_ignore_ascii_case("AUTO"));
        }
        let stripped = upper
            .trim_end_matches("OHM")
            .trim_end_matches('Ω')
            .trim_end_matches('V')
            .trim_end_matches('A')
            .trim_end_matches('F')
            .trim();
        let compact = upper.replace([' ', '_'], "");
        let as_f = parse_eng(raw)
            .or_else(|| parse_eng(stripped))
            .or_else(|| parse_eng(&compact));
        (0..self.len()).find(|&i| {
            let (key, val) = self.get_opt(i);
            let key_c = key.replace([' ', '_'], "");
            key.eq_ignore_ascii_case(raw)
                || key_c.eq_ignore_ascii_case(&compact)
                || val.eq_ignore_ascii_case(raw)
                || val.eq_ignore_ascii_case(&upper)
                || val.eq_ignore_ascii_case(&compact)
                || as_f.is_some_and(|a| {
                    parse_eng(val)
                        .is_some_and(|b| (a - b).abs() <= 1e-15 * a.abs().max(b.abs()).max(1.0))
                })
        })
    }

    fn owon_xdm1041_vac() -> Self {
        Self {
            scpi: "CONF:VOLT:AC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500mV" => "500E-3",
                "5V" => "5",
                "50V" => "50",
                "500V" => "500",
                "750V" => "750",
            },
        }
    }

    fn owon_xdm1041_adc() -> Self {
        Self {
            scpi: "CONF:CURR:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500uA" => "500E-6",
                "5mA" => "5E-3",
                "50mA" => "50E-3",
                "500mA" => "500E-3",
                "5A" => "5",
                "10A" => "10",
            },
        }
    }

    fn owon_xdm1041_aac() -> Self {
        Self {
            scpi: "CONF:CURR:AC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500uA" => "500E-6",
                "5mA" => "5E-3",
                "50mA" => "50E-3",
                "500mA" => "500E-3",
                "5A" => "5",
                "10A" => "10",
            },
        }
    }

    fn owon_xdm1041_res() -> Self {
        Self {
            scpi: "CONF:RES ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500Ohm" => "500",
                "5kOhm" => "5E3",
                "50kOhm" => "50E3",
                "500kOhm" => "500E3",
                "5MOhm" => "5E6",
                "50MOhm" => "50E6",
            },
        }
    }

    fn owon_xdm1041_cap() -> Self {
        Self {
            scpi: "CONF:CAP ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "50nF" => "50E-9",
                "500nF" => "500E-9",
                "5uF" => "5E-6",
                "50uF" => "50E-6",
                "500uF" => "500E-6",
                "5mF" => "5E-3",
                "50mF" => "50E-3",
            },
        }
    }

    fn owon_xdm1041_temp() -> Self {
        Self {
            scpi: "CONF:TEMP:RTD ",
            opts: phf_ordered_map! {
                "PT100" => "PT100",
                "K-type (KITS90)" => "KITS90",
            },
        }
    }
}

fn parse_eng(raw: &str) -> Option<f64> {
    let t = raw.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}
