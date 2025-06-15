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
    pub fn profiles(&self) -> Vec<&String> {
        self.profiles.iter().collect()
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
#[derive(Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct OutputConfig {
    pub duration: Option<u64>,
    pub path: PathBuf,
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
}

fn default_profile() -> String {
    String::from("default")
}

/// Serializable reader struct for output config
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
struct ConfigReader {
    #[serde(default = "default_profile")]
    default_profile: String,
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
                    profiles.insert(Some("default"), iden, output_config.clone());
                }
            }
        }

        config.profiles = profiles;
        config.default_profile = self.default_profile;
    }
}

/// Serializable struct to support 2 different config forms
#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deserialize_config() {
        let conf_str = r#"
            [eDP-1]
            duration = 10
            path = "/home/user/wallpapers/personal"

            [HDMI-A-1]
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

        assert_eq!(
            config.get_output_config("default", "eDP-1").unwrap(),
            OutputConfig {
                duration: Some(10),
                path: "/home/user/wallpapers/personal".into(),
            },
        );
        assert_eq!(
            config.get_output_config("work", "eDP-1").unwrap(),
            OutputConfig {
                duration: Some(10),
                path: "/home/user/wallpapers/work".into()
            }
        );

        let value: toml::Table = toml::from_str(conf_str).unwrap();
        dbg!(value);
    }
}
