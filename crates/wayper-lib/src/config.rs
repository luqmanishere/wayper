use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use color_eyre::{Result, eyre::eyre};
use serde::Deserialize;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub default_profile: String,
    pub profiles: Profiles,
    pub reloaded: bool,
    pub path: Option<PathBuf>,
    pub transition: Option<TransitionConfig>,
    pub transitions_enabled: Option<bool>,
}

impl Config {
    pub fn new(config_str: &str) -> Result<Self> {
        let mut config = Self::default();

        let config_reader = ConfigReader::new(config_str)?;
        config_reader.merge_config(&mut config);
        Ok(config)
    }

    pub fn load_file(path: &Path) -> Result<Self> {
        let mut config: Self = Self::new(&std::fs::read_to_string(path)?)?;
        config.path = Some(path.into());
        Ok(config)
    }

    pub fn update(&mut self) -> Result<()> {
        ConfigReader::new(&std::fs::read_to_string(
            self.path.clone().expect("path exists"),
        )?)?
        .merge_config(self);

        self.reloaded = true;

        Ok(())
    }

    pub fn get_output_config<'a, P, O>(&self, profile: P, output_name: O) -> Result<OutputConfig>
    where
        P: Into<Option<&'a str>>,
        O: Into<&'a str>,
    {
        Ok(self
            .profiles
            .get_config(profile, output_name)
            .ok_or_else(|| eyre!("Can't find config for that output"))?
            .clone())
    }
}

/// Holds a map of output configs. Internally, it uses a hashmap within a hashmap.
#[derive(Default, Clone, Debug)]
pub struct Profiles {
    map: HashMap<String, HashMap<String, OutputConfig>>,
    profiles: HashSet<String>,
}

impl Profiles {
    /// Gets the config for the given profile and output. If profile is None, the default profile
    /// is used.
    pub fn get_config<'a, S, O>(&self, profile: S, output_name: O) -> Option<&OutputConfig>
    where
        S: Into<Option<&'a str>>,
        O: Into<&'a str>,
    {
        let profile: Option<&str> = profile.into();
        let output_name: &str = output_name.into();
        self.map
            .get(profile.unwrap_or("default"))
            .and_then(|e| e.get(output_name))
    }

    /// Returns a list of current profiles
    pub fn profiles(&self) -> Vec<String> {
        self.profiles.iter().cloned().collect()
    }

    /// Insert an output config for a profile
    fn insert(&mut self, profile: Option<&str>, output_name: &str, output_config: OutputConfig) {
        let profile = profile.unwrap_or("default");
        self.map
            .entry(profile.to_string())
            .or_default()
            .insert(output_name.to_string(), output_config);
        self.profiles.insert(profile.to_string());
    }
}

/// Serializable output config
#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub struct OutputConfig {
    pub duration: Option<u64>,
    pub path: PathBuf,
    pub run_command: Option<String>,
    pub transition: Option<TransitionConfig>,
    pub transitions_enabled: Option<bool>,
}

impl OutputConfig {
    #[allow(dead_code)]
    pub fn load<P>(path: P) -> Result<HashMap<String, Self>>
    where
        P: AsRef<Path>,
    {
        let vecconf: HashMap<String, Self> = toml::from_str(&std::fs::read_to_string(path)?)?;
        dbg!(&vecconf);
        Ok(vecconf)
    }

    pub fn get_transition_config<'a>(&'a self, global_config: &'a Config) -> Option<&'a TransitionConfig> {
        self.transition.as_ref().or(global_config.transition.as_ref())
    }

    pub fn is_transitions_enabled(&self, global_config: &Config) -> bool {
        self.transitions_enabled
            .or(global_config.transitions_enabled)
            .unwrap_or(true)
    }
}

fn default_profile() -> String {
    String::from("default")
}

/// Serializable reader struct for output config
#[derive(Deserialize, Clone, Debug, PartialEq)]
struct ConfigReader {
    #[serde(default = "default_profile")]
    default_profile: String,
    pub transition: Option<TransitionConfig>,
    pub transitions_enabled: Option<bool>,
    #[serde(flatten)]
    pub outputs: HashMap<String, ProfileReader>,
}

impl ConfigReader {
    pub fn new(config_str: &str) -> Result<Self> {
        Ok(toml::from_str(config_str)?)
    }

    pub fn merge_config(self, config: &mut Config) {
        let mut profiles = Profiles::default();
        for (iden, config) in self.outputs.iter() {
            match config {
                ProfileReader::Profile(hash_map) => {
                    hash_map
                        .iter()
                        .for_each(|e| profiles.insert(Some(iden), e.0, e.1.clone()));
                }
                ProfileReader::Default(output_config) => {
                    profiles.insert(Some(&self.default_profile), iden, output_config.clone());
                }
            }
        }

        config.profiles = profiles;
        config.default_profile = self.default_profile;
        config.transition = self.transition;
        config.transitions_enabled = self.transitions_enabled;
    }
}

/// Serializable struct to support 2 different config forms
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
enum ProfileReader {
    Profile(HashMap<String, OutputConfig>),
    Default(OutputConfig),
}
impl Default for ProfileReader {
    fn default() -> Self {
        Self::Default(OutputConfig::default())
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct TransitionConfig {
    #[serde(rename = "type")]
    pub transition_type: TransitionSelection,

    #[serde(default = "default_duration")]
    pub duration_ms: u32,

    #[serde(default = "default_fps")]
    pub fps: u16,

    #[serde(default)]
    pub sweep: SweepConfig,
}

impl TransitionConfig {
    pub fn pick_random_type(&self) -> TransitionTypeEnum {
        use rand::seq::SliceRandom;
        match &self.transition_type {
            TransitionSelection::Single(t) => *t,
            TransitionSelection::Random(types) => {
                let mut rng = rand::thread_rng();
                *types.choose(&mut rng).unwrap_or(&TransitionTypeEnum::Crossfade)
            }
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum TransitionSelection {
    Single(TransitionTypeEnum),
    Random(Vec<TransitionTypeEnum>),
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransitionTypeEnum {
    Crossfade,
    Sweep,
}

impl TransitionTypeEnum {
    pub fn to_u32(&self) -> u32 {
        match self {
            TransitionTypeEnum::Crossfade => 0,
            TransitionTypeEnum::Sweep => 1,
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct SweepConfig {
    #[serde(default)]
    pub direction: Direction,

    #[serde(default = "default_edge_width")]
    pub edge_width: f32,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            direction: Direction::default(),
            edge_width: default_edge_width(),
        }
    }
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    #[default]
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
    TopLeftToBottomRight,
    TopRightToBottomLeft,
    BottomLeftToTopRight,
    BottomRightToTopLeft,
}

impl Direction {
    pub fn as_vec2(&self) -> [f32; 2] {
        match self {
            Direction::LeftToRight => [1.0, 0.0],
            Direction::RightToLeft => [-1.0, 0.0],
            Direction::TopToBottom => [0.0, 1.0],
            Direction::BottomToTop => [0.0, -1.0],
            Direction::TopLeftToBottomRight => [1.0, 1.0],
            Direction::TopRightToBottomLeft => [-1.0, 1.0],
            Direction::BottomLeftToTopRight => [1.0, -1.0],
            Direction::BottomRightToTopLeft => [-1.0, -1.0],
        }
    }
}

fn default_duration() -> u32 { 2000 }
fn default_fps() -> u16 { 30 }
fn default_edge_width() -> f32 { 0.05 }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_config() {
        let conf_str = r#"
            default_profile = "home"

            [home.eDP-1]
            duration = 10
            path = "/home/user/wallpapers/personal"
            run_command = "matugen image {image}"

            [home.HDMI-A-1]
            duration = 20
            path = "/home/user/wallpapers"

            [work.eDP-1]
            duration = 10
            path = "/home/user/wallpapers/work"

            [school.eDP-1]
            duration = 10
            path = "/home/user/wallpapers/school"
        "#;

        let config_reader: ConfigReader = toml::from_str(conf_str).unwrap();
        dbg!(&config_reader);
        let config = Config::new(conf_str).unwrap();

        assert_eq!(config.default_profile, "home");

        assert_eq!(
            config.get_output_config("home", "eDP-1").unwrap(),
            OutputConfig {
                duration: Some(10),
                path: "/home/user/wallpapers/personal".into(),
                run_command: Some(String::from("matugen image {image}")),
                transition: None,
                transitions_enabled: None,
            },
        );
        assert_eq!(
            config.get_output_config("work", "eDP-1").unwrap(),
            OutputConfig {
                duration: Some(10),
                path: "/home/user/wallpapers/work".into(),
                run_command: None,
                transition: None,
                transitions_enabled: None,
            }
        );

        let value: toml::Table = toml::from_str(conf_str).unwrap();
        dbg!(value);
    }

    #[test]
    fn test_deserialize_transition_config() {
        let conf_str = r#"
            [transition]
            type = "sweep"
            duration_ms = 1500
            fps = 60

            [transition.sweep]
            direction = "left-to-right"
            edge_width = 0.08

            [eDP-1]
            duration = 10
            path = "/home/user/wallpapers"

            [eDP-1.transition]
            type = "crossfade"
            duration_ms = 2000

            [HDMI-A-1]
            duration = 20
            path = "/home/user/wallpapers/hdmi"

            [HDMI-A-1.transition]
            type = ["crossfade", "sweep"]
            duration_ms = 1000
            fps = 30

            [HDMI-A-1.transition.sweep]
            direction = "top-left-to-bottom-right"
        "#;

        let config = Config::new(conf_str).unwrap();

        let global_transition = config.transition.as_ref().unwrap();
        assert_eq!(global_transition.duration_ms, 1500);
        assert_eq!(global_transition.fps, 60);
        assert_eq!(global_transition.sweep.edge_width, 0.08);
        assert!(matches!(
            global_transition.sweep.direction,
            Direction::LeftToRight
        ));

        let edp_config = config.get_output_config("default", "eDP-1").unwrap();
        let edp_transition = edp_config.transition.as_ref().unwrap();
        assert_eq!(edp_transition.duration_ms, 2000);
        assert!(matches!(
            edp_transition.transition_type,
            TransitionSelection::Single(TransitionTypeEnum::Crossfade)
        ));

        let hdmi_config = config.get_output_config("default", "HDMI-A-1").unwrap();
        let hdmi_transition = hdmi_config.transition.as_ref().unwrap();
        assert_eq!(hdmi_transition.duration_ms, 1000);
        assert_eq!(hdmi_transition.fps, 30);
        if let TransitionSelection::Random(types) = &hdmi_transition.transition_type {
            assert_eq!(types.len(), 2);
            assert!(matches!(types[0], TransitionTypeEnum::Crossfade));
            assert!(matches!(types[1], TransitionTypeEnum::Sweep));
        } else {
            panic!("Expected Random transition type");
        }
        assert!(matches!(
            hdmi_transition.sweep.direction,
            Direction::TopLeftToBottomRight
        ));
    }
}
