use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use color_eyre::{
    Result,
    eyre::{OptionExt, eyre},
};
use etcetera::{AppStrategy, AppStrategyArgs};
use serde::Deserialize;

pub fn app_start() -> impl AppStrategy {
    etcetera::choose_app_strategy(AppStrategyArgs {
        top_level_domain: "dev".to_string(),
        author: "solemnattic".to_string(),
        app_name: "wayper".to_string(),
    })
    .unwrap()
}

pub static CONFIG_DIR: LazyLock<PathBuf> = LazyLock::new(|| app_start().config_dir());

pub fn default_config_path() -> PathBuf {
    CONFIG_DIR.join("windows.toml")
}

#[derive(Debug, Clone)]
pub struct Config {
    pub path: Option<PathBuf>,
    pub content: ContentConfig,
}

impl Config {
    pub fn new(config_str: &str) -> Result<Self> {
        let config: ConfigFile = toml::from_str(config_str)?;
        Ok(Self {
            path: None,
            content: config.content,
        })
    }

    pub fn load_file(path: &Path) -> Result<Self> {
        let mut config = Self::new(&fs::read_to_string(path)?)?;
        config.path = Some(path.to_path_buf());
        Ok(config)
    }

    pub fn resolve_content(&self) -> Result<ResolvedContent> {
        match &self.content {
            ContentConfig::Image(config) => Ok(ResolvedContent::Image(config.resolve()?)),
            ContentConfig::Video(config) => Ok(ResolvedContent::Video(config.clone())),
            ContentConfig::Scene(config) => Ok(ResolvedContent::Scene(config.clone())),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ConfigFile {
    content: ContentConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ContentConfig {
    Image(ImageContentConfig),
    Video(VideoContentConfig),
    Scene(SceneContentConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageContentConfig {
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub folders: Vec<PathBuf>,
    #[serde(default)]
    pub fit: FitModeConfig,
}

impl ImageContentConfig {
    pub fn resolve(&self) -> Result<ResolvedImageContent> {
        let path = if let Some(path) = &self.path {
            path.clone()
        } else {
            self.resolve_from_folders()?
        };

        if !path.is_file() {
            return Err(eyre!(
                "configured image path is not a file: {}",
                path.display()
            ));
        }

        Ok(ResolvedImageContent {
            path,
            fit: self.fit.into(),
        })
    }

    fn resolve_from_folders(&self) -> Result<PathBuf> {
        let mut candidates = Vec::new();

        for folder in &self.folders {
            let entries = fs::read_dir(folder)
                .map_err(|err| eyre!("failed to read image folder {}: {err}", folder.display()))?;

            for entry in entries {
                let entry = entry?;
                let path = entry.path();
                if path.is_file() && is_supported_image(&path) {
                    candidates.push(path);
                }
            }
        }

        candidates.sort();
        candidates
            .into_iter()
            .next()
            .ok_or_eyre("no supported image files found in configured folders")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoContentConfig {
    pub path: PathBuf,
    pub fit: Option<FitModeConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SceneContentConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum ResolvedContent {
    Image(ResolvedImageContent),
    Video(VideoContentConfig),
    Scene(SceneContentConfig),
}

#[derive(Debug, Clone)]
pub struct ResolvedImageContent {
    pub path: PathBuf,
    pub fit: FitModeConfig,
}

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FitModeConfig {
    Contain,
    #[default]
    Cover,
    Stretch,
}

fn is_supported_image(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };

    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "webp" | "tiff"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_image_config() {
        let config = Config::new(
            r#"
                [content]
                type = "image"
                path = "samples/wallpapers/example.jpg"
                fit = "contain"
            "#,
        )
        .unwrap();

        let ResolvedContent::Image(image) = config.resolve_content().unwrap() else {
            panic!("expected image content");
        };

        assert_eq!(image.path, PathBuf::from("samples/wallpapers/example.jpg"));
        assert!(matches!(image.fit, FitModeConfig::Contain));
    }

    #[test]
    fn parses_future_video_content() {
        let config = Config::new(
            r#"
                [content]
                type = "video"
                path = "samples/videos/demo.mp4"
                fit = "cover"
            "#,
        )
        .unwrap();

        let ResolvedContent::Video(video) = config.resolve_content().unwrap() else {
            panic!("expected video content");
        };

        assert_eq!(video.path, PathBuf::from("samples/videos/demo.mp4"));
    }
}
