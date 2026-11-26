use crate::{capture::OutputInfo, geometry::Rect};
use anyhow::{Context, Result, bail};
use directories::{ProjectDirs, UserDirs};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

/// How Flick was launched; each way has its own keybinding and behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    Main,
    Alternative,
    Fastshot,
    Settings,
}

impl Launch {
    /// Arguments after the program name.
    pub fn args(self) -> &'static [&'static str] {
        match self {
            Self::Main => &[],
            Self::Alternative => &["--alt"],
            Self::Fastshot => &["fastshot"],
            Self::Settings => &["settings"],
        }
    }
}

/// What happens to the picture or the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub copy: bool,
    pub save: bool,
}

impl Action {
    pub const NONE: Self = Self::new(false, false);

    pub const fn new(copy: bool, save: bool) -> Self {
        Self { copy, save }
    }

    pub fn any(self) -> bool {
        self.copy || self.save
    }

    /// Choices for fastshots, which must do something.
    pub const FASTSHOT: [(Self, &str); 3] = [
        (Self::new(true, false), "Copy"),
        (Self::new(true, true), "Copy and save"),
        (Self::new(false, true), "Save"),
    ];

    /// Choices for closing Flick.
    pub const QUIT: [(Self, &str); 4] = [
        (Self::NONE, "Just close"),
        (Self::new(true, false), "Copy, then close"),
        (Self::new(true, true), "Copy and save, then close"),
        (Self::new(false, true), "Save, then close"),
    ];
}

/// What happens once a region is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Behavior {
    pub action: Action,
    /// Shows the toolbar; otherwise Flick closes right after `action`.
    pub open: bool,
}

impl Behavior {
    pub const fn new(copy: bool, save: bool, open: bool) -> Self {
        Self {
            action: Action::new(copy, save),
            open,
        }
    }

    pub const CHOICES: [(Self, &str); 7] = [
        (Self::new(true, false, true), "Copy, then show the toolbar"),
        (Self::new(true, false, false), "Copy and close"),
        (
            Self::new(true, true, true),
            "Copy and save, then show the toolbar",
        ),
        (Self::new(true, true, false), "Copy and save, then close"),
        (Self::new(false, false, true), "Show the toolbar"),
        (Self::new(false, true, true), "Save, then show the toolbar"),
        (Self::new(false, true, false), "Save and close"),
    ];
}

/// A keybinding in Hyprland's notation, such as `ALT + Print`; no key means unbound.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Keybind {
    pub mods: u32,
    pub key: String,
}

impl Keybind {
    // Hyprland's modmask bits.
    pub const SHIFT: u32 = 1;
    pub const CTRL: u32 = 4;
    pub const ALT: u32 = 8;
    pub const SUPER: u32 = 64;
    /// In the order they are written.
    const MODS: [(u32, &'static str, &'static str); 4] = [
        (Self::SUPER, "SUPER", "Super"),
        (Self::CTRL, "CTRL", "Ctrl"),
        (Self::ALT, "ALT", "Alt"),
        (Self::SHIFT, "SHIFT", "Shift"),
    ];

    pub fn new(mods: u32, key: &str) -> Self {
        // Hyprland's own examples write letters in capitals.
        let key = match key.len() {
            1 => key.to_ascii_uppercase(),
            _ => key.to_owned(),
        };
        Self { mods, key }
    }

    pub fn is_bound(&self) -> bool {
        !self.key.is_empty()
    }

    fn join(&self, names: impl Fn(&(u32, &'static str, &'static str)) -> &'static str) -> String {
        let mut parts: Vec<&str> = Self::MODS
            .iter()
            .filter(|m| self.mods & m.0 != 0)
            .map(names)
            .collect();
        parts.push(&self.key);
        parts.join(" + ")
    }

    /// For people, e.g. `Alt + Print`.
    pub fn label(&self) -> String {
        match self.is_bound() {
            true => self.join(|m| m.2),
            false => "Not set".into(),
        }
    }

    /// Modifiers for `hyprland.conf`, e.g. `ALT SHIFT`.
    pub fn conf_mods(&self) -> String {
        Self::MODS
            .iter()
            .filter(|m| self.mods & m.0 != 0)
            .map(|m| m.1)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The same key and modifiers, ignoring letter case.
    pub fn same(&self, mods: u32, key: &str) -> bool {
        self.is_bound() && self.mods == mods && self.key.eq_ignore_ascii_case(key)
    }
}

impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.is_bound() {
            true => f.write_str(&self.join(|m| m.1)),
            false => Ok(()),
        }
    }
}

impl From<Keybind> for String {
    fn from(k: Keybind) -> Self {
        k.to_string()
    }
}

impl TryFrom<String> for Keybind {
    type Error = anyhow::Error;

    fn try_from(s: String) -> Result<Self> {
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let Some((key, mods)) = parts.split_last() else {
            unreachable!("split yields at least one part")
        };
        if key.is_empty() {
            if parts.len() > 1 {
                bail!("keybinding {s:?} has no key");
            }
            return Ok(Self::default());
        }
        let mut mask = 0;
        for m in mods {
            mask |= match m.to_ascii_uppercase().as_str() {
                "SUPER" | "WIN" | "LOGO" | "MOD4" => Self::SUPER,
                "CTRL" | "CONTROL" => Self::CTRL,
                "ALT" | "MOD1" => Self::ALT,
                "SHIFT" => Self::SHIFT,
                _ => bail!("unknown modifier {m:?} in {s:?}"),
            };
        }
        Ok(Self::new(mask, key))
    }
}

/// A keybinding and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trigger {
    pub key: Keybind,
    pub copy: bool,
    pub save: bool,
    pub open: bool,
}

impl Trigger {
    pub fn behavior(&self) -> Behavior {
        Behavior::new(self.copy, self.save, self.open)
    }

    pub fn set_behavior(&mut self, b: Behavior) {
        (self.copy, self.save, self.open) = (b.action.copy, b.action.save, b.open);
    }
}

/// Fastshots work only on a locked region and never show the toolbar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fastshot {
    pub key: Keybind,
    pub copy: bool,
    pub save: bool,
}

impl Fastshot {
    pub fn action(&self) -> Action {
        Action::new(self.copy, self.save)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Saving {
    /// `None` is Flick in the user's pictures or videos directory.
    pub dir: Option<PathBuf>,
    /// Asks for a file name on every save instead of using `dir`.
    pub ask: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // Plain values come first: TOML cannot put them after a table.
    /// A `strftime`-style pattern, without the extension.
    pub file_name: String,
    pub fps: u32,
    /// System audio in recordings.
    pub audio: bool,
    /// The default input device in recordings.
    pub microphone: bool,
    /// The pointer in recordings.
    pub cursor: bool,
    /// Asks before closing a recording that was not saved.
    pub confirm_discard: bool,
    /// Dashed lines from the handles to the screen edges while selecting.
    pub guides: bool,
    /// Done by Esc and the close button.
    pub quit: Action,
    pub main: Trigger,
    pub alternative: Trigger,
    pub fastshot: Fastshot,
    pub screenshots: Saving,
    pub recordings: Saving,
    /// The screenshot directory of older versions.
    #[serde(skip_serializing)]
    save_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        let trigger = |key: &str, b: Behavior| {
            let mut t = Trigger {
                key: Keybind::try_from(key.to_owned()).expect("valid default"),
                copy: false,
                save: false,
                open: false,
            };
            t.set_behavior(b);
            t
        };
        Self {
            file_name: "Flick_%Y-%m-%d_%H-%M-%S".into(),
            fps: 30,
            audio: true,
            microphone: false,
            cursor: true,
            confirm_discard: true,
            guides: true,
            quit: Action::NONE,
            main: trigger("Print", Behavior::new(true, true, false)),
            alternative: trigger("ALT + Print", Behavior::new(true, true, true)),
            fastshot: Fastshot {
                key: Keybind::new(Keybind::SHIFT, "Print"),
                copy: true,
                save: true,
            },
            screenshots: Saving::default(),
            recordings: Saving::default(),
            save_dir: None,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut c: Self = read_toml(path)?.unwrap_or_default();
        if let Some(dir) = c.save_dir.take() {
            c.screenshots.dir.get_or_insert(dir);
        }
        Ok(c)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        write_toml(path, self)
    }

    /// The behavior of a launch; fastshots never show the toolbar.
    pub fn behavior(&self, launch: Launch) -> Behavior {
        match launch {
            Launch::Alternative => self.alternative.behavior(),
            Launch::Fastshot => Behavior {
                action: self.fastshot.action(),
                open: false,
            },
            Launch::Main | Launch::Settings => self.main.behavior(),
        }
    }

    pub fn keys(&self) -> [(&Keybind, Launch); 3] {
        [
            (&self.main.key, Launch::Main),
            (&self.alternative.key, Launch::Alternative),
            (&self.fastshot.key, Launch::Fastshot),
        ]
    }

    /// The first rule the settings break, in words for the settings window.
    pub fn problem(&self) -> Option<String> {
        let (main, alt) = (self.main.behavior(), self.alternative.behavior());
        if main == alt {
            return Some("The main and the alternative behavior must differ.".into());
        }
        if !main.open && !alt.open {
            return Some(
                "The main or the alternative behavior must show the toolbar, \
                 or you could never edit, record or unlock."
                    .into(),
            );
        }
        if !main.action.any() && !main.open || !alt.action.any() && !alt.open {
            return Some("A behavior must do something.".into());
        }
        if !self.fastshot.action().any() {
            return Some("A fastshot must copy or save.".into());
        }
        let keys = self.keys();
        for (i, (a, _)) in keys.iter().enumerate() {
            if keys[i + 1..].iter().any(|(b, _)| b.same(a.mods, &a.key)) {
                return Some(format!(
                    "{} is used twice; keybindings must differ.",
                    a.label()
                ));
            }
        }
        if self.file_name.trim().is_empty() || self.file_name.contains('/') {
            return Some("The file name must not be empty or contain “/”.".into());
        }
        if !(1..=240).contains(&self.fps) {
            return Some("The frame rate must be between 1 and 240.".into());
        }
        None
    }

    pub fn screenshot_dir(&self) -> Result<PathBuf> {
        default_dir(&self.screenshots, |u| u.picture_dir(), "Pictures")
    }

    pub fn recording_dir(&self) -> Result<PathBuf> {
        default_dir(&self.recordings, |u| u.video_dir(), "Videos")
    }
}

fn default_dir(
    s: &Saving,
    pick: impl Fn(&UserDirs) -> Option<&Path>,
    fallback: &str,
) -> Result<PathBuf> {
    if let Some(dir) = &s.dir {
        return Ok(dir.clone());
    }
    let user = UserDirs::new().context("cannot find the home directory")?;
    let base = pick(&user)
        .map(Path::to_owned)
        .unwrap_or_else(|| user.home_dir().join(fallback));
    Ok(base.join("Flick"))
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

    #[test]
    fn config_round_trips_and_migrates() {
        let dir = std::env::temp_dir().join(format!("flick-config-{}", std::process::id()));
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.alternative.key = Keybind::default();
        c.recordings.ask = true;
        c.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), c);
        // Older versions had one directory and a video format.
        fs::write(&path, "save_dir = \"/a\"\nvideo_format = \"gif\"\n").unwrap();
        let c = Config::load(&path).unwrap();
        assert_eq!(c.screenshots.dir.as_deref(), Some(Path::new("/a")));
        assert_eq!(c.screenshot_dir().unwrap(), Path::new("/a"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keybinds_parse_and_print() {
        let k = Keybind::try_from("alt + shift+ Print".to_owned()).unwrap();
        assert_eq!(k.mods, Keybind::ALT | Keybind::SHIFT);
        assert_eq!(k.to_string(), "ALT + SHIFT + Print");
        assert_eq!(k.label(), "Alt + Shift + Print");
        assert_eq!(k.conf_mods(), "ALT SHIFT");
        assert_eq!(Keybind::new(Keybind::SUPER, "a").to_string(), "SUPER + A");
        assert!(!Keybind::try_from(String::new()).unwrap().is_bound());
        assert!(Keybind::try_from("ALT +".to_owned()).is_err());
        assert!(Keybind::try_from("HYPER + A".to_owned()).is_err());
        assert!(k.same(9, "print"));
        assert!(!Keybind::default().same(0, ""));
    }

    #[test]
    fn defaults_are_valid_and_rules_hold() {
        let mut c = Config::default();
        assert_eq!(c.problem(), None);
        c.alternative.set_behavior(c.main.behavior());
        assert!(c.problem().unwrap().contains("differ"));
        c.alternative
            .set_behavior(Behavior::new(false, true, false));
        assert!(c.problem().unwrap().contains("toolbar"));
        c.alternative
            .set_behavior(Behavior::new(false, false, true));
        assert_eq!(c.problem(), None);
        c.fastshot.key = c.main.key.clone();
        assert!(c.problem().unwrap().contains("twice"));
        // Unbound keys never clash.
        c.fastshot.key = Keybind::default();
        c.main.key = Keybind::default();
        assert_eq!(c.problem(), None);
        c.file_name = "a/b".into();
        assert!(c.problem().is_some());
    }

    #[test]
    fn every_choice_is_distinct_and_valid() {
        for (i, (b, _)) in Behavior::CHOICES.iter().enumerate() {
            assert!(b.open || b.action.any());
            assert!(!Behavior::CHOICES[i + 1..].iter().any(|(c, _)| c == b));
        }
        assert!(Action::FASTSHOT.iter().all(|(a, _)| a.any()));
    }
}
