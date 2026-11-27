use anyhow::{Context, Result, bail};
use rustix::process::{Pid, Signal, kill_process};
use std::{
    ffi::OsString,
    fs::{self, File},
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

/// Sound sources, both read through PipeWire's PulseAudio server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Audio {
    /// What the speakers play.
    pub system: bool,
    /// The default input device.
    pub microphone: bool,
}

impl Audio {
    fn sources(self) -> Vec<&'static str> {
        [
            (self.system, "@DEFAULT_MONITOR@"),
            (self.microphone, "@DEFAULT_SOURCE@"),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, s)| s)
        .collect()
    }
}

#[derive(Debug, Clone)]
pub struct RecordArgs {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio: Audio,
    pub output: PathBuf,
}

impl RecordArgs {
    pub fn to_args(&self) -> Result<Vec<OsString>> {
        // yuv420p needs even dimensions.
        let (w, h) = (self.width & !1, self.height & !1);
        if w == 0 || h == 0 {
            bail!("region is too small to record");
        }
        let mut a: Vec<OsString> = Vec::new();
        let mut push = |s: &str| a.extend(s.split_whitespace().map(OsString::from));
        push("-hide_banner -loglevel warning -nostats -y");
        // Audio inputs come first. ffmpeg 9.1 rejects -thread_queue_size before an input.
        let sources = self.audio.sources();
        for s in &sources {
            push(&format!("-f pulse -i {s}"));
        }
        push(&format!(
            "-f rawvideo -pixel_format bgr0 -video_size {}x{} -framerate {} -i pipe:0",
            self.width, self.height, self.fps
        ));
        push(match sources.len() {
            0 => "-map 0:v",
            1 => "-map 1:v -map 0:a",
            // Kept at their own levels; amix would halve each by default.
            _ => {
                "-filter_complex [0:a][1:a]amix=inputs=2:duration=longest:normalize=0[a] \
                 -map 2:v -map [a]"
            }
        });
        if !sources.is_empty() {
            push("-c:a aac -b:a 160k -shortest");
        }
        push(&format!(
            "-vf crop={w}:{h}:0:0 -c:v libx264 -preset veryfast -crf 20 -pix_fmt yuv420p"
        ));
        push("-movflags +faststart -f mp4");
        a.push(self.output.clone().into());
        Ok(a)
    }
}

pub struct Encoder {
    child: Child,
    output: PathBuf,
    log: PathBuf,
}

impl Encoder {
    pub fn spawn(args: &RecordArgs) -> Result<Self> {
        if let Some(dir) = args.output.parent() {
            fs::create_dir_all(dir)?;
        }
        let log = args.output.with_extension("log");
        let child = Command::new("ffmpeg")
            .args(args.to_args()?)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(File::create(&log)?)
            .spawn()
            .context("cannot start ffmpeg")?;
        Ok(Self {
            child,
            output: args.output.clone(),
            log,
        })
    }

    pub fn take_stdin(&mut self) -> Result<ChildStdin> {
        let stdin = self.child.stdin.take().context("ffmpeg stdin is taken")?;
        // A frame is megabytes; the default 64 KiB pipe splits it into many
        // writes, each a switch between the pacer and ffmpeg. 1 MiB is the
        // usual unprivileged limit, so failing to grow it is fine.
        let _ = rustix::pipe::fcntl_setpipe_size(&stdin, 1 << 20);
        Ok(stdin)
    }

    pub fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Call after stdin is closed: waits for EOF handling, then SIGINT, then SIGKILL.
    pub fn finish(mut self, timeout: Duration) -> Result<PathBuf> {
        let mut status = self.wait(timeout)?;
        if status.is_none() {
            kill_process(Pid::from_child(&self.child), Signal::INT)?;
            status = self.wait(timeout)?;
        }
        let log = fs::read_to_string(&self.log).unwrap_or_default();
        match status {
            None => {
                self.kill();
                bail!("ffmpeg did not exit: {}", tail(&log))
            }
            Some(s) if !s.success() => bail!("ffmpeg failed ({s}): {}", tail(&log)),
            Some(_) => {}
        }
        if fs::metadata(&self.output).map_or(0, |m| m.len()) == 0 {
            bail!("ffmpeg produced no file: {}", tail(&log));
        }
        let _ = fs::remove_file(&self.log);
        Ok(self.output.clone())
    }

    fn wait(&mut self, timeout: Duration) -> Result<Option<ExitStatus>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(s) = self.child.try_wait()? {
                return Ok(Some(s));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.has_exited() {
            self.kill();
        }
    }
}

fn tail(log: &str) -> &str {
    let start = log.trim_end().rmatch_indices('\n').nth(4);
    log[start.map_or(0, |(i, _)| i + 1)..].trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(width: u32, audio: Audio) -> Result<String> {
        let a = RecordArgs {
            width,
            height: 101,
            fps: 30,
            audio,
            output: "/tmp/a b.mp4".into(),
        }
        .to_args()?;
        assert_eq!(a.last().unwrap(), "/tmp/a b.mp4");
        Ok(a.join(" ".as_ref()).into_string().unwrap())
    }

    #[test]
    fn arguments() {
        let system = Audio {
            system: true,
            microphone: false,
        };
        let a = args(201, system).unwrap();
        assert!(a.contains("-video_size 201x101") && a.contains("crop=200:100:0:0"));
        assert!(a.find("-f pulse").unwrap() < a.find("pipe:0").unwrap());
        assert!(a.contains("-map 1:v -map 0:a") && !a.contains("amix"));
        let none = args(201, Audio::default()).unwrap();
        assert!(!none.contains("pulse") && none.contains("-map 0:v"));
        assert!(args(1, Audio::default()).is_err());
    }

    #[test]
    fn microphone_alone_and_mixed() {
        let mic = Audio {
            system: false,
            microphone: true,
        };
        let a = args(200, mic).unwrap();
        assert!(a.contains("-i @DEFAULT_SOURCE@") && !a.contains("MONITOR"));
        assert!(a.contains("-map 1:v -map 0:a"));
        let both = Audio {
            system: true,
            microphone: true,
        };
        let a = args(200, both).unwrap();
        assert!(a.find("@DEFAULT_MONITOR@").unwrap() < a.find("@DEFAULT_SOURCE@").unwrap());
        assert!(a.contains("amix=inputs=2") && a.contains("-map 2:v -map [a]"));
    }
}
