pub mod ffmpeg;
pub mod fit;
pub mod pacer;

use crate::{
    capture::{CaptureBackend, CaptureRequest, FrameSlot, LiveRegion, StreamHandle},
    geometry::Rect,
};
use anyhow::{Context, Result, bail};
pub use ffmpeg::Audio;
use ffmpeg::{Encoder, RecordArgs};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::JoinHandle,
    time::Duration,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// One ffmpeg run, from a start or resume to the next pause or stop.
/// Counters over all segments; see [`pacer::Stats`].
#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub captured: u64,
    pub written: u64,
    pub repeated: u64,
    pub worst_lag: u64,
}

struct Segment {
    encoder: Encoder,
    stop: Arc<AtomicBool>,
    pacer_done: Receiver<Result<u64>>,
}

impl Segment {
    fn start(slot: &FrameSlot, args: &RecordArgs, stats: &Arc<pacer::Stats>) -> Result<Self> {
        let mut encoder = Encoder::spawn(args)?;
        let stdin = encoder.take_stdin()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, slot, fps) = (stop.clone(), slot.clone(), args.fps);
        let size = (args.width, args.height);
        let stats = stats.clone();
        let (tx, pacer_done) = mpsc::channel();
        std::thread::Builder::new()
            .name("pacer".into())
            .spawn(move || {
                let _ = tx.send(pacer::run(stdin, &slot, fps, size, &flag, &stats));
            })?;
        Ok(Self {
            encoder,
            stop,
            pacer_done,
        })
    }

    fn finish(mut self) -> Result<PathBuf> {
        self.stop.store(true, Ordering::Relaxed);
        // The pacer may be stuck writing to a hung ffmpeg, so never join blindly.
        let Ok(paced) = self.pacer_done.recv_timeout(STOP_TIMEOUT) else {
            self.encoder.kill();
            bail!("ffmpeg stopped accepting frames");
        };
        let path = self.encoder.finish(STOP_TIMEOUT)?;
        paced.context("writing frames to ffmpeg")?;
        Ok(path)
    }
}

/// Records into segments: a pause ends one, resuming starts the next, and
/// stopping joins them. ffmpeg reads the audio itself, so this is the only
/// way to leave the paused time out of the sound as well.
pub struct Recorder {
    capture: StreamHandle,
    region: LiveRegion,
    slot: FrameSlot,
    /// The first segment's settings; later ones differ only in the file.
    args: RecordArgs,
    current: Option<Segment>,
    /// Paused segments, still finishing in the background.
    finished: Vec<JoinHandle<Result<PathBuf>>>,
    segments: u32,
    stats: Arc<pacer::Stats>,
}

impl Recorder {
    pub fn start(
        backend: Box<dyn CaptureBackend>,
        mut req: CaptureRequest,
        fps: u32,
        audio: Audio,
        output: PathBuf,
    ) -> Result<Self> {
        if !(1..=240).contains(&fps) {
            bail!("fps must be between 1 and 240");
        }
        // A little faster than the video, so its frames are never late.
        req.interval = Duration::from_secs(1) * 4 / (5 * fps);
        let slot = FrameSlot::default();
        let region = LiveRegion::new(req.region);
        let capture = backend.stream(req, region.clone(), slot.clone())?;
        let first = slot.latest().context("capture produced no first frame")?;
        let args = RecordArgs {
            width: first.width,
            height: first.height,
            fps,
            audio,
            output,
        };
        let mut r = Self {
            capture,
            region,
            slot,
            args,
            current: None,
            finished: Vec::new(),
            segments: 0,
            stats: Arc::default(),
        };
        r.resume()?;
        Ok(r)
    }

    fn part(&self, n: u32) -> PathBuf {
        part(&self.args.output, n)
    }

    /// Follows the border on `screen` until `resume_video`.
    pub fn track_video(&self, screen: Rect) {
        self.region.track(screen);
    }

    /// Must be told every border position that is drawn.
    pub fn border_drawn(&self, region: Rect) {
        self.region.drawn(region);
    }

    /// Records `region`, of the same size as before, from now on.
    pub fn resume_video(&self, region: Rect) {
        self.region.resume(region);
    }

    pub fn paused(&self) -> bool {
        self.current.is_none()
    }

    /// Stops writing; the segment is finalised in the background.
    pub fn pause(&mut self) {
        if let Some(s) = self.current.take() {
            self.finished.push(std::thread::spawn(move || s.finish()));
        }
    }

    /// Starts a new segment after `pause`.
    pub fn resume(&mut self) -> Result<()> {
        if self.current.is_some() {
            return Ok(());
        }
        let args = RecordArgs {
            output: self.part(self.segments),
            ..self.args.clone()
        };
        self.current = Some(Segment::start(&self.slot, &args, &self.stats)?);
        self.segments += 1;
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        let get = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed);
        Stats {
            captured: self.slot.count(),
            written: get(&self.stats.written),
            repeated: get(&self.stats.repeated),
            worst_lag: get(&self.stats.worst_lag),
        }
    }

    /// True once the recording can no longer continue; `stop` reports the reason.
    pub fn has_failed(&mut self) -> bool {
        self.current
            .as_mut()
            .is_some_and(|s| s.encoder.has_exited())
            || self.capture.is_finished()
    }

    /// Finalises the file and returns its path.
    pub fn stop(mut self) -> Result<PathBuf> {
        self.pause();
        let finished: Vec<_> = self
            .finished
            .drain(..)
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("panicked")))
            })
            .collect();
        let captured = self.capture.stop();
        let result = finished
            .into_iter()
            .collect::<Result<Vec<_>>>()
            .and_then(|parts| {
                captured.context("screen capture ended early")?;
                join(&parts, &self.args.output)
            });
        for n in 0..self.segments {
            let _ = std::fs::remove_file(part(&self.args.output, n));
        }
        result.map(|()| self.args.output.clone())
    }
}

/// `a.mp4` → `a.part0.mp4`.
fn part(output: &Path, n: u32) -> PathBuf {
    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
    output.with_file_name(format!("{stem}.part{n}.mp4"))
}

/// Concatenates segments of equal format without re-encoding.
fn join(parts: &[PathBuf], output: &Path) -> Result<()> {
    if let [one] = parts {
        return std::fs::rename(one, output).context("cannot move the recording");
    }
    let list = output.with_extension("txt");
    let mut text = String::new();
    for p in parts {
        let p = p.to_str().context("recording path is not UTF-8")?;
        text += &format!("file '{}'\n", p.replace('\'', r"'\''"));
    }
    std::fs::write(&list, text)?;
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args(["-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .args(["-c", "copy", "-movflags", "+faststart", "-f", "mp4"])
        .arg(output)
        .output();
    let _ = std::fs::remove_file(&list);
    let out = out.context("cannot start ffmpeg")?;
    if !out.status.success() {
        bail!(
            "joining the recording failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_names() {
        assert_eq!(
            part(Path::new("/c/rec_1.mp4"), 2),
            Path::new("/c/rec_1.part2.mp4")
        );
    }
}
