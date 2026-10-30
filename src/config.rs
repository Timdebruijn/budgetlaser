use crate::fixture::Personality;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub input: Input,
    pub output: Output,
    #[serde(rename = "fixture")]
    pub fixtures: Vec<Fixture>,
}

/// One virtual fixture: a DMX footprint drawn into its own zone of the output.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields, default)]
pub struct Fixture {
    /// DMX start address, 1-based.
    pub start_address: u16,
    pub personality: Personality,
    /// x, y, width, height as fractions of the output, origin top-left.
    pub zone: [f32; 4],
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            start_address: 1,
            personality: Personality::default(),
            zone: [0.0, 0.0, 1.0, 1.0],
        }
    }
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Active sources; several at once are merged (sACN priority, then HTP).
    pub sources: Vec<Source>,
    /// Art-Net 15-bit port-address (QLC+ "universe 1" = 0).
    pub artnet_universe: u16,
    /// sACN universe, 1..63999.
    pub sacn_universe: u16,
    pub serial_port: String,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            input: Input::default(),
            output: Output::default(),
            fixtures: vec![Fixture::default()],
        }
    }
}

impl Config {
    fn validate(mut self) -> Result<Self, String> {
        let i = &mut self.input;
        i.sources.sort_unstable();
        i.sources.dedup();
        if i.sources.is_empty() {
            return Err("input.sources must name at least one source".into());
        }
        if i.artnet_universe > 0x7fff {
            return Err(format!(
                "input.artnet_universe {} exceeds 32767",
                i.artnet_universe
            ));
        }
        if !(1..=63999).contains(&i.sacn_universe) {
            return Err(format!(
                "input.sacn_universe {} must be 1..63999",
                i.sacn_universe
            ));
        }
        if self.fixtures.is_empty() {
            return Err("at least one [[fixture]] is required".into());
        }
        for (i, f) in self.fixtures.iter().enumerate() {
            let n = i + 1;
            let last = f.start_address as usize + f.personality.footprint() - 1;
            if f.start_address == 0 || last > 512 {
                return Err(format!(
                    "fixture {n}: start_address {} with {} channels does not fit in 1..512",
                    f.start_address,
                    f.personality.footprint()
                ));
            }
            let [x, y, w, h] = f.zone;
            let inside = |v: f32| (0.0..=1.0 + 1e-4).contains(&v);
            if !(w > 0.0 && h > 0.0 && inside(x) && inside(y) && inside(x + w) && inside(y + h)) {
                return Err(format!(
                    "fixture {n}: zone {:?} must lie within 0..1 with positive size",
                    f.zone
                ));
            }
        }
        Ok(self)
    }
}

impl Default for Input {
    fn default() -> Self {
        Self {
            sources: vec![Source::Artnet],
            artnet_universe: 0,
            sacn_universe: 1,
            serial_port: if cfg!(windows) {
                "COM3".into()
            } else {
                "/dev/ttyUSB0".into()
            },
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

pub fn load(path: &str) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str::<Config>(&s)
            .map_err(|e| {
                let mut e = e.to_string();
                if e.contains("unknown field `start_address`") {
                    e.push_str("\nstart_address moved to [[fixture]] blocks, see budgetlaser.toml");
                }
                if e.contains("unknown field `source`") || e.contains("unknown field `universe`") {
                    e.push_str("\nuse sources = [...] with artnet_universe / sacn_universe");
                }
                e
            })
            .and_then(Config::validate)
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

    #[test]
    fn fixtures_parse_and_validate() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.fixtures.len(), 1);
        let two = r#"
            [[fixture]]
            start_address = 1
            zone = [0.0, 0.0, 0.5, 1.0]
            [[fixture]]
            start_address = 14
            zone = [0.5, 0.0, 0.5, 1.0]
        "#;
        let c: Config = toml::from_str(two).unwrap();
        assert!(c.validate().is_ok());
        let bad_zone: Config = toml::from_str("[[fixture]]\nzone = [0.6, 0.0, 0.5, 1.0]").unwrap();
        assert!(bad_zone.validate().is_err());
        let bad_addr: Config = toml::from_str("[[fixture]]\nstart_address = 501").unwrap();
        assert!(bad_addr.validate().is_err());
    }

    #[test]
    fn start_address_bounds() {
        let check = |f: &str| {
            toml::from_str::<Config>(&format!("[[fixture]]\n{f}"))
                .unwrap()
                .validate()
                .is_ok()
        };
        assert!(!check("start_address = 0"));
        assert!(check("start_address = 500"));
        assert!(!check("start_address = 501"));
        assert!(check("start_address = 496\npersonality = \"17ch\""));
        assert!(!check("start_address = 497\npersonality = \"17ch\""));
    }

    #[test]
    fn sources_parse_and_validate() {
        let c: Config = toml::from_str("[input]\nsources = [\"sacn\", \"artnet\"]").unwrap();
        assert_eq!(c.input.sources, [Source::Sacn, Source::Artnet]);
        assert!(c.validate().is_ok());
        let none: Config = toml::from_str("[input]\nsources = []").unwrap();
        assert!(none.validate().is_err());
        let bad: Config = toml::from_str("[input]\nsacn_universe = 0").unwrap();
        assert!(bad.validate().is_err());
        assert!(toml::from_str::<Config>("[input]\nsource = \"artnet\"").is_err());
    }
}
