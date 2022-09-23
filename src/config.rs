use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use eyre::{eyre, Result};
use serde::Deserialize;

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(flatten)]
    pub outputs: HashMap<String, Arc<OutputConfig>>,
    #[serde(skip, default)]
    pub reload: bool,
    #[serde(skip, default)]
    pub path: Option<PathBuf>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(&std::fs::read_to_string(&path)?)?;
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

    pub fn get_output_config(&mut self, name: &str) -> Result<Arc<OutputConfig>> {
        Ok(Arc::clone(self.outputs.get(name).ok_or_else(|| {
            eyre!("Can't find config for that output")
        })?))
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct OutputConfig {
    pub name: Option<String>,
    pub duration: Option<i32>,
    pub path: Option<PathBuf>,
}

impl OutputConfig {
    pub fn load<P>(path: P) -> Result<HashMap<String, Self>>
    where
        P: AsRef<Path>,
    {
        let vecconf: HashMap<String, Self> = toml::from_str(&std::fs::read_to_string(path)?)?;
        dbg!(&vecconf);
        Ok(vecconf)
    }
}
