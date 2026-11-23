use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoFormat {
    Mp4,
    WebM,
    Gif,
}

impl VideoFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::WebM => "webm",
            Self::Gif => "gif",
        }
    }
}

/// Converts the temporary MP4. Blocking: call it off the UI thread.
pub fn export(input: &Path, output: &Path, format: VideoFormat) -> Result<()> {
    let args: &[&str] = match format {
        VideoFormat::Mp4 => {
            std::fs::copy(input, output)?;
            return Ok(());
        }
        VideoFormat::WebM => &[
            "-c:v",
            "libvpx-vp9",
            "-crf",
            "32",
            "-b:v",
            "0",
            "-row-mt",
            "1",
            "-cpu-used",
            "4",
            "-c:a",
            "libopus",
            "-f",
            "webm",
        ],
        VideoFormat::Gif => &[
            "-an",
            "-vf",
            "fps=15,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:diff_mode=rectangle",
            "-f",
            "gif",
        ],
    };
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-i"])
        .arg(input)
        .args(args)
        .arg(output)
        .stdout(Stdio::null())
        .output()
        .context("cannot start ffmpeg")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(output);
        bail!(
            "export failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
