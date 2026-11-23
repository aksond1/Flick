use anyhow::{Context, Result, bail};
use rustix::process::{Pid, Signal, kill_process};
use std::{
    ffi::OsString,
    fs::{self, File},
    path::PathBuf,
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct RecordArgs {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub audio: bool,
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
        if self.audio {
            push("-thread_queue_size 1024 -f pulse -i @DEFAULT_MONITOR@");
        }
        push(&format!(
            "-f rawvideo -pixel_format bgr0 -video_size {}x{} -framerate {} -i pipe:0",
            self.width, self.height, self.fps
        ));
        push(if self.audio {
            "-map 1:v -map 0:a -c:a aac -b:a 160k -shortest"
        } else {
            "-map 0:v"
        });
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
        self.child.stdin.take().context("ffmpeg stdin is taken")
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

    fn args(width: u32, audio: bool) -> Result<String> {
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
        let a = args(201, true).unwrap();
        assert!(a.contains("-video_size 201x101") && a.contains("crop=200:100:0:0"));
        assert!(a.find("-f pulse").unwrap() < a.find("pipe:0").unwrap());
        assert!(!args(201, false).unwrap().contains("pulse"));
        assert!(args(1, false).is_err());
    }
}
