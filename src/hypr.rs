//! Hyprland: the focused output and the keybindings that launch Flick.
use crate::config::{Config, Keybind};
use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

/// Marks Flick's own bindings, so others on the same keys can be told apart.
const DESCRIPTION: &str = "flick";

/// Name of the focused output, if Hyprland is running.
pub fn focused_output() -> Option<String> {
    let out = Command::new("hyprctl")
        .args(["-j", "activeworkspace"])
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v["monitor"].as_str().map(str::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `hyprland.lua`, Hyprland 0.55 and later.
    Lua,
    Conf,
}

/// The user's Hyprland config and the file Flick writes its bindings to.
#[derive(Debug, Clone)]
pub struct HyprConfig {
    pub syntax: Syntax,
    pub main: PathBuf,
    pub binds: PathBuf,
}

impl HyprConfig {
    /// Hyprland reads `hyprland.lua` when it exists, otherwise `hyprland.conf`.
    pub fn locate() -> Option<Self> {
        let dir = BaseDirs::new()?.config_dir().join("hypr");
        Self::in_dir(&dir)
    }

    pub fn in_dir(dir: &Path) -> Option<Self> {
        let (syntax, main, binds) = if dir.join("hyprland.lua").exists() {
            (Syntax::Lua, "hyprland.lua", "flick.lua")
        } else if dir.join("hyprland.conf").exists() {
            (Syntax::Conf, "hyprland.conf", "flick.conf")
        } else {
            return None;
        };
        Some(Self {
            syntax,
            main: dir.join(main),
            binds: dir.join(binds),
        })
    }

    /// The line that loads the bindings file.
    pub fn include_line(&self) -> String {
        match self.syntax {
            Syntax::Lua => "require(\"flick\")".into(),
            Syntax::Conf => format!("source = {}", self.binds.display()),
        }
    }

    /// Whether the main config loads the bindings file.
    pub fn is_included(&self) -> bool {
        let Ok(text) = fs::read_to_string(&self.main) else {
            return false;
        };
        text.lines().map(str::trim).any(|l| match self.syntax {
            Syntax::Lua => {
                !l.starts_with("--")
                    && [
                        "require(\"flick\")",
                        "require('flick')",
                        "require \"flick\"",
                    ]
                    .iter()
                    .any(|r| l.contains(r))
            }
            Syntax::Conf => {
                l.starts_with("source") && l.split('#').next().unwrap_or("").contains("flick.conf")
            }
        })
    }

    /// Appends the include line to the main config.
    pub fn include(&self) -> Result<()> {
        if self.is_included() {
            return Ok(());
        }
        let comment = match self.syntax {
            Syntax::Lua => "--",
            Syntax::Conf => "#",
        };
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(&self.main)
            .with_context(|| format!("cannot open {}", self.main.display()))?;
        write!(
            f,
            "\n{comment} Keybindings set in Flick's settings\n{}\n",
            self.include_line()
        )?;
        Ok(())
    }

    pub fn write_binds(&self, config: &Config, program: &str) -> Result<()> {
        let text = render(self.syntax, config, program);
        fs::write(&self.binds, text)
            .with_context(|| format!("cannot write {}", self.binds.display()))
    }
}

/// The bindings file for `config`; `program` is how to start Flick.
pub fn render(syntax: Syntax, config: &Config, program: &str) -> String {
    let mut s = String::new();
    let comment = match syntax {
        Syntax::Lua => "--",
        Syntax::Conf => "#",
    };
    s += &format!("{comment} Written by Flick's settings; changes here are overwritten.\n");
    for (key, launch) in config.keys() {
        if !key.is_bound() {
            continue;
        }
        let cmd = std::iter::once(program)
            .chain(launch.args().iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        s += &match syntax {
            Syntax::Lua => format!(
                "hl.bind({}, hl.dsp.exec_cmd({}), {{ description = {} }})\n",
                lua_string(&key.to_string()),
                lua_string(&cmd),
                lua_string(DESCRIPTION),
            ),
            Syntax::Conf => format!(
                "bindd = {}, {}, {DESCRIPTION}, exec, {cmd}\n",
                key.conf_mods(),
                key.key
            ),
        };
    }
    s
}

fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// How Hyprland should start this program: by name when `PATH` finds this very
/// binary, otherwise by its full path, quoted for the shell.
pub fn program() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "flick".into();
    };
    let on_path = std::env::var_os("PATH").is_some_and(|p| {
        std::env::split_paths(&p).any(|d| {
            d.join("flick")
                .canonicalize()
                .is_ok_and(|found| found == exe)
        })
    });
    if on_path {
        return "flick".into();
    }
    let s = exe.to_string_lossy();
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        s.into_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

pub fn reload() -> Result<()> {
    let out = Command::new("hyprctl")
        .args(["reload", "config-only"])
        .output()
        .context("cannot run hyprctl")?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !text.trim().eq_ignore_ascii_case("ok") {
        bail!("Hyprland did not reload: {}", text.trim());
    }
    Ok(())
}

/// Flick's keybindings that something else in Hyprland also uses.
pub fn conflicts(config: &Config) -> Result<Vec<Keybind>> {
    let out = Command::new("hyprctl")
        .args(["-j", "binds"])
        .output()
        .context("cannot run hyprctl")?;
    let binds: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("Hyprland is not running")?;
    Ok(find_conflicts(config, &binds))
}

fn find_conflicts(config: &Config, binds: &serde_json::Value) -> Vec<Keybind> {
    let others: Vec<(u32, &str)> = binds
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["description"].as_str() != Some(DESCRIPTION))
        .filter(|b| b["submap"].as_str().is_none_or(str::is_empty))
        .filter_map(|b| Some((b["modmask"].as_u64()? as u32, b["key"].as_str()?)))
        .collect();
    config
        .keys()
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| others.iter().any(|(m, key)| k.same(*m, key)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_syntaxes() {
        let mut c = Config::default();
        c.fastshot.key = Keybind::default();
        assert_eq!(
            render(Syntax::Lua, &c, "flick"),
            "-- Written by Flick's settings; changes here are overwritten.\n\
             hl.bind(\"Print\", hl.dsp.exec_cmd(\"flick\"), { description = \"flick\" })\n\
             hl.bind(\"ALT + Print\", hl.dsp.exec_cmd(\"flick --alt\"), { description = \"flick\" })\n"
        );
        assert_eq!(
            render(Syntax::Conf, &c, "/a b/flick").lines().nth(2),
            Some("bindd = ALT, Print, flick, exec, /a b/flick --alt")
        );
    }

    #[test]
    fn include_is_found_and_added_once() {
        let dir = std::env::temp_dir().join(format!("flick-hypr-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(HyprConfig::in_dir(&dir).is_none());
        fs::write(dir.join("hyprland.lua"), "-- require(\"flick\")\n").unwrap();
        let h = HyprConfig::in_dir(&dir).unwrap();
        assert_eq!(h.syntax, Syntax::Lua);
        assert!(!h.is_included());
        h.include().unwrap();
        h.include().unwrap();
        assert!(h.is_included());
        let text = fs::read_to_string(&h.main).unwrap();
        assert_eq!(text.matches("require(\"flick\")").count(), 2);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn other_bindings_on_the_same_keys_conflict() {
        let c = Config::default();
        let binds = serde_json::json!([
            { "modmask": 0, "key": "Print", "description": "", "submap": "" },
            { "modmask": 8, "key": "print", "description": "flick", "submap": "" },
            { "modmask": 1, "key": "Print", "description": "", "submap": "resize" },
            { "modmask": 64, "key": "Q", "description": "" },
        ]);
        assert_eq!(find_conflicts(&c, &binds), vec![c.main.key.clone()]);
    }
}
