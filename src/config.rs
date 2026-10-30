use crate::fixture::Personality;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
#[derive(Default)]
pub struct Config {
    pub input: Input,
    pub output: Output,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Artnet,
    Sacn,
    Usbpro,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SignalLoss {
    Blackout,
    Hold,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mapping {
    Fit,
    Stretch,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct Input {
    pub source: Source,
    /// Art-Net 15-bit port-address (QLC+ "universe 1" = 0) or sACN universe (1..63999).
    pub universe: Option<u16>,
    pub serial_port: String,
    /// DMX start address, 1-based.
    pub start_address: u16,
    pub personality: Personality,
    pub signal_loss: SignalLoss,
    pub signal_timeout_s: f64,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct Output {
    pub fullscreen: bool,
    /// Index into the monitor list.
    pub monitor: usize,
    pub mapping: Mapping,
    pub line_width_px: f32,
    pub glow: bool,
    pub glow_decay: f32,
    pub dimmer_gamma: f32,
    /// Internal render resolution relative to the window (0.5 = half).
    pub internal_scale: f32,
    pub exposure: f32,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            source: Source::Artnet,
            universe: None,
            serial_port: if cfg!(windows) {
                "COM3".into()
            } else {
                "/dev/ttyUSB0".into()
            },
            start_address: 1,
            personality: Personality::default(),
            signal_loss: SignalLoss::Blackout,
            signal_timeout_s: 3.0,
        }
    }
}

impl Default for Output {
    fn default() -> Self {
        Self {
            fullscreen: true,
            monitor: 0,
            mapping: Mapping::Fit,
            line_width_px: 2.0,
            glow: true,
            glow_decay: 0.85,
            dimmer_gamma: 2.2,
            internal_scale: 1.0,
            exposure: 1.0,
        }
    }
}

impl Input {
    pub fn universe(&self) -> u16 {
        self.universe.unwrap_or(match self.source {
            Source::Sacn => 1,
            _ => 0,
        })
    }
}

impl Config {
    fn validate(self) -> Result<Self, String> {
        let first = self.input.start_address as usize;
        let last = first + self.input.personality.footprint() - 1;
        if first == 0 || last > 512 {
            return Err(format!(
                "start_address {first} with {} channels does not fit in 1..512",
                self.input.personality.footprint()
            ));
        }
        Ok(self)
    }
}

pub fn load(path: &str) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str::<Config>(&s)
            .map_err(|e| format!("{path}: {e}"))?
            .validate()
            .map_err(|e| format!("{path}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::warn!("{path} not found, using defaults");
            Ok(Config::default())
        }
        Err(e) => Err(format!("{path}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(toml: &str) -> bool {
        toml::from_str::<Config>(toml).unwrap().validate().is_ok()
    }

    #[test]
    fn start_address_bounds() {
        assert!(!check("[input]\nstart_address = 0"));
        assert!(check("[input]\nstart_address = 500"));
        assert!(!check("[input]\nstart_address = 501"));
        assert!(check(
            "[input]\nstart_address = 496\npersonality = \"17ch\""
        ));
        assert!(!check(
            "[input]\nstart_address = 497\npersonality = \"17ch\""
        ));
    }
}
