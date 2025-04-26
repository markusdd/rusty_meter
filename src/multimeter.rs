use phf::{phf_ordered_map, OrderedMap};

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

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum MeterMode {
    Vdc,
    Vac,
    Adc,
    Aac,
    Res,
    Cap,
    Freq,
    Per,
    Diod,
    Cont,
    Temp,
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
    pub fn new(meter: &str, mode: &str) -> Option<Self> {
        match (meter, mode) {
            ("OWON XDM1041", "VDC") => Some(Self::default()),
            ("OWON XDM1041", "VAC") => Some(Self::owon_xdm1041_vac()),
            ("OWON XDM1041", "ADC") => Some(Self::owon_xdm1041_adc()),
            ("OWON XDM1041", "AAC") => Some(Self::owon_xdm1041_aac()),
            ("OWON XDM1041", "RES") => Some(Self::owon_xdm1041_res()),
            ("OWON XDM1041", "CAP") => Some(Self::owon_xdm1041_cap()),
            ("OWON XDM1041", "TEMP") => Some(Self::owon_xdm1041_temp()),
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