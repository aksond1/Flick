use crate::{capture::OutputInfo, geometry::Rect, record::export::VideoFormat};
use anyhow::{Context, Result};
use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Overrides the default `~/Pictures/Flick`.
    pub save_dir: Option<PathBuf>,
    pub fps: u32,
    pub audio: bool,
    pub cursor: bool,
    pub video_format: VideoFormat,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            save_dir: None,
            fps: 30,
            audio: true,
            cursor: true,
            video_format: VideoFormat::Mp4,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        Ok(read_toml(path)?.unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_toml(path, self)
    }

    pub fn save_dir(&self) -> Result<PathBuf> {
        if let Some(dir) = &self.save_dir {
            return Ok(dir.clone());
        }
        let user = UserDirs::new().context("cannot find the home directory")?;
        let pictures = user
            .picture_dir()
            .map(Path::to_owned)
            .unwrap_or_else(|| user.home_dir().join("Pictures"));
        Ok(pictures.join("Flick"))
    }
}

/// A pinned region; valid only while its output keeps the same size and scale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub output: OutputInfo,
    pub region: Rect,
}

impl Lock {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        read_toml(path)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_toml(path, self)
    }

    pub fn clear(path: &Path) -> Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn is_valid(&self, outputs: &[OutputInfo]) -> bool {
        self.region.w > 0 && self.region.h > 0 && outputs.contains(&self.output)
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub lock: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn new() -> Result<Self> {
        let d = ProjectDirs::from("", "", "flick").context("cannot find the home directory")?;
        Ok(Self {
            config: d.config_dir().join("config.toml"),
            lock: d
                .state_dir()
                .context("no state directory")?
                .join("lock.toml"),
            cache: d.cache_dir().to_owned(),
        })
    }
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s)
            .map(Some)
            .with_context(|| format!("invalid file {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Rename, so a crash cannot leave a half-written file.
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, toml::to_string_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_round_trip_and_staleness() {
        let dir = std::env::temp_dir().join(format!("flick-lock-{}", std::process::id()));
        let path = dir.join("lock.toml");
        let out = OutputInfo {
            name: "HDMI-A-1".into(),
            width: 1920,
            height: 1080,
            scale: 1,
        };
        let lock = Lock {
            output: out.clone(),
            region: Rect::new(1, 2, 3, 4),
        };
        assert_eq!(Lock::load(&path).unwrap(), None);
        lock.save(&path).unwrap();
        assert_eq!(Lock::load(&path).unwrap().unwrap(), lock);
        assert!(lock.is_valid(std::slice::from_ref(&out)));
        assert!(!lock.is_valid(&[OutputInfo { width: 2560, ..out }]));
        assert!(!lock.is_valid(&[]));
        Lock::clear(&path).unwrap();
        Lock::clear(&path).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let c: Config = toml::from_str("fps = 60").unwrap();
        assert_eq!(
            c,
            Config {
                fps: 60,
                ..Config::default()
            }
        );
    }
}
