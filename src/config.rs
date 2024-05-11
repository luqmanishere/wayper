use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use eyre::{eyre, Result};
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug, Default)]
pub struct WayperConfig {
    #[serde(flatten)]
    pub outputs: HashMap<String, OutputConfig>,
    #[serde(skip, default)]
    pub reload: bool,
    #[serde(skip, default)]
    pub path: Option<PathBuf>,
}

impl WayperConfig {
    /// Load config from the specified path. Returns `Self`
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(&std::fs::read_to_string(path)?)?;
        config.path = Some(path.into());
        Ok(config)
    }

    pub fn update(&mut self) -> Result<()> {
        let new_config: Self =
            toml::from_str(&std::fs::read_to_string(self.path.as_ref().unwrap())?)?;

        self.outputs = new_config.outputs;
        self.reload = true;

        Ok(())
    }

    pub fn get_output_config(&mut self, name: &str) -> Result<OutputConfig> {
        Ok(self
            .outputs
            .get(name)
            .ok_or_else(|| eyre!("Can't find config for that output"))?
            .clone())
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct OutputConfig {
    /// the name of the output, even though its redundant
    pub name: Option<String>,
    /// duration in between image switching
    pub duration: Option<u64>,
    /// path to files
    pub path: Option<PathBuf>,
    /// play videos found in folder
    pub animated: bool,
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
