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
    Fres,
    Cap,
    Freq,
    Per,
    Duty,
    Diod,
    Cont,
    Temp,
}

impl MeterMode {
    pub const ALL: [Self; 13] = [
        Self::Vdc,
        Self::Vac,
        Self::Adc,
        Self::Aac,
        Self::Res,
        Self::Fres,
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
            Self::Fres => "Ohm",
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
            Self::Fres => "4W Ohm",
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
            Self::Fres => "CONF:FRES AUTO\n",
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
            Self::Vdc
                | Self::Vac
                | Self::Adc
                | Self::Aac
                | Self::Res
                | Self::Fres
                | Self::Cap
                | Self::Temp
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
            "FRES" => Some(Self::Fres),
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
            Self::Fres => &["FRESISTANCE", "FRES"],
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
            ("OWON XDM1041", MeterMode::Fres) => Some(Self::owon_xdm2041_fres()),
            ("OWON XDM1041", MeterMode::Cap) => Some(Self::owon_xdm1041_cap()),
            ("OWON XDM1041", MeterMode::Temp) => Some(Self::owon_xdm1041_temp()),
            // XDM1051/1251 (5.5 digit). User manual p.46: DCV/DCI/RES differ
            // from the 1041; ACV/ACI/CAP/TEMP match.
            ("OWON XDM1051", MeterMode::Vdc) => Some(Self::owon_xdm1051_vdc()),
            ("OWON XDM1051", MeterMode::Vac) => Some(Self::owon_xdm1041_vac()),
            ("OWON XDM1051", MeterMode::Adc) => Some(Self::owon_xdm1051_adc()),
            ("OWON XDM1051", MeterMode::Aac) => Some(Self::owon_xdm1041_aac()),
            ("OWON XDM1051", MeterMode::Res) => Some(Self::owon_xdm1051_res()),
            ("OWON XDM1051", MeterMode::Cap) => Some(Self::owon_xdm1041_cap()),
            ("OWON XDM1051", MeterMode::Temp) => Some(Self::owon_xdm1041_temp()),
            ("OWON XDM3041", MeterMode::Vdc) => Some(Self::owon_xdm3041_vdc()),
            ("OWON XDM3041", MeterMode::Vac) => Some(Self::owon_xdm3041_vac()),
            ("OWON XDM3041", MeterMode::Adc) => Some(Self::owon_xdm3041_adc()),
            ("OWON XDM3041", MeterMode::Aac) => Some(Self::owon_xdm3041_aac()),
            ("OWON XDM3041", MeterMode::Res | MeterMode::Fres) => {
                Some(Self::owon_xdm3041_res(mode))
            }
            ("OWON XDM3041", MeterMode::Cap) => Some(Self::owon_xdm3000_cap()),
            ("OWON XDM3051", MeterMode::Vdc) => Some(Self::owon_xdm3051_vdc()),
            ("OWON XDM3051", MeterMode::Vac) => Some(Self::owon_xdm3051_vac()),
            ("OWON XDM3051", MeterMode::Adc) => Some(Self::owon_xdm3051_adc()),
            ("OWON XDM3051", MeterMode::Aac) => Some(Self::owon_xdm3051_aac()),
            ("OWON XDM3051", MeterMode::Res | MeterMode::Fres) => {
                Some(Self::owon_xdm3051_res(mode))
            }
            ("OWON XDM3051", MeterMode::Cap) => Some(Self::owon_xdm3000_cap()),
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

    /// XDM2041 four-wire resistance ranges. The programming manual limits
    /// FRESistance to 50 kOhm even though two-wire RESistance goes to 50 MOhm.
    fn owon_xdm2041_fres() -> Self {
        Self {
            scpi: "CONF:FRES ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "500Ohm" => "500",
                "5kOhm" => "5E3",
                "50kOhm" => "50E3",
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

    fn owon_xdm1051_vdc() -> Self {
        Self {
            scpi: "CONF:VOLT:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "100mV" => "100E-3",
                "1V" => "1",
                "10V" => "10",
                "100V" => "100",
                "1000V" => "1000",
            },
        }
    }

    fn owon_xdm1051_adc() -> Self {
        Self {
            scpi: "CONF:CURR:DC ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "100uA" => "100E-6",
                "1mA" => "1E-3",
                "10mA" => "10E-3",
                "100mA" => "100E-3",
                "1A" => "1",
                "10A" => "10",
            },
        }
    }

    fn owon_xdm1051_res() -> Self {
        Self {
            scpi: "CONF:RES ",
            opts: phf_ordered_map! {
                "auto" => "AUTO",
                "100Ohm" => "100",
                "1kOhm" => "1E3",
                "10kOhm" => "10E3",
                "100kOhm" => "100E3",
                "1MOhm" => "1E6",
                "10MOhm" => "10E6",
                "100MOhm" => "100E6",
            },
        }
    }

    fn owon_xdm3041_vdc() -> Self {
        Self::with_ranges(
            "CONF:VOLT:DC ",
            phf_ordered_map! {
                "auto" => "AUTO", "600mV" => "600E-3", "6V" => "6", "60V" => "60",
                "600V" => "600", "1000V" => "1000",
            },
        )
    }

    fn owon_xdm3041_vac() -> Self {
        Self::with_ranges(
            "CONF:VOLT:AC ",
            phf_ordered_map! {
                "auto" => "AUTO", "600mV" => "600E-3", "6V" => "6", "60V" => "60",
                "600V" => "600", "750V" => "750",
            },
        )
    }

    fn owon_xdm3041_adc() -> Self {
        Self::with_ranges(
            "CONF:CURR:DC ",
            phf_ordered_map! {
                "auto" => "AUTO", "600uA" => "600E-6", "6mA" => "6E-3",
                "60mA" => "60E-3", "600mA" => "600E-3", "6A" => "6", "10A" => "10",
            },
        )
    }

    fn owon_xdm3041_aac() -> Self {
        Self::with_ranges(
            "CONF:CURR:AC ",
            phf_ordered_map! {
                "auto" => "AUTO", "60mA" => "60E-3", "600mA" => "600E-3",
                "6A" => "6", "10A" => "10",
            },
        )
    }

    fn owon_xdm3041_res(mode: MeterMode) -> Self {
        Self::with_ranges(
            if mode == MeterMode::Fres {
                "CONF:FRES "
            } else {
                "CONF:RES "
            },
            phf_ordered_map! {
                "auto" => "AUTO", "600Ohm" => "600", "6kOhm" => "6E3",
                "60kOhm" => "60E3", "600kOhm" => "600E3", "6MOhm" => "6E6",
                "60MOhm" => "60E6", "100MOhm" => "100E6",
            },
        )
    }

    fn owon_xdm3051_vdc() -> Self {
        Self::with_ranges(
            "CONF:VOLT:DC ",
            phf_ordered_map! {
                "auto" => "AUTO", "200mV" => "200E-3", "2V" => "2", "20V" => "20",
                "200V" => "200", "1000V" => "1000",
            },
        )
    }

    fn owon_xdm3051_vac() -> Self {
        Self::with_ranges(
            "CONF:VOLT:AC ",
            phf_ordered_map! {
                "auto" => "AUTO", "200mV" => "200E-3", "2V" => "2", "20V" => "20",
                "200V" => "200", "750V" => "750",
            },
        )
    }

    fn owon_xdm3051_adc() -> Self {
        Self::with_ranges(
            "CONF:CURR:DC ",
            phf_ordered_map! {
                "auto" => "AUTO", "200uA" => "200E-6", "2mA" => "2E-3",
                "20mA" => "20E-3", "200mA" => "200E-3", "2A" => "2", "10A" => "10",
            },
        )
    }

    fn owon_xdm3051_aac() -> Self {
        Self::with_ranges(
            "CONF:CURR:AC ",
            phf_ordered_map! {
                "auto" => "AUTO", "20mA" => "20E-3", "200mA" => "200E-3",
                "2A" => "2", "10A" => "10",
            },
        )
    }

    fn owon_xdm3051_res(mode: MeterMode) -> Self {
        Self::with_ranges(
            if mode == MeterMode::Fres {
                "CONF:FRES "
            } else {
                "CONF:RES "
            },
            phf_ordered_map! {
                "auto" => "AUTO", "200Ohm" => "200", "2kOhm" => "2E3",
                "20kOhm" => "20E3", "200kOhm" => "200E3", "2MOhm" => "2E6",
                "10MOhm" => "10E6", "100MOhm" => "100E6",
            },
        )
    }

    fn owon_xdm3000_cap() -> Self {
        Self::with_ranges(
            "CONF:CAP ",
            phf_ordered_map! {
                "auto" => "AUTO", "2nF" => "2E-9", "20nF" => "20E-9",
                "200nF" => "200E-9", "2uF" => "2E-6", "20uF" => "20E-6",
                "200uF" => "200E-6", "10mF" => "10E-3",
            },
        )
    }

    fn with_ranges(scpi: &'static str, opts: OrderedMap<&'static str, &'static str>) -> Self {
        Self { scpi, opts }
    }
}

fn parse_eng(raw: &str) -> Option<f64> {
    let t = raw.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok()
}
