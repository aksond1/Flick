pub mod export;
pub mod ffmpeg;
pub mod fit;
pub mod pacer;

use crate::{
    capture::{CaptureBackend, CaptureRequest, FrameSlot, LiveRegion, StreamHandle},
    geometry::Rect,
};
use anyhow::{Context, Result, bail};
use ffmpeg::{Encoder, RecordArgs};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Duration,
};

const STOP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Recorder {
    encoder: Encoder,
    capture: StreamHandle,
    region: LiveRegion,
    stop: Arc<AtomicBool>,
    pacer_done: Receiver<Result<u64>>,
}

impl Recorder {
    pub fn start(
        backend: Box<dyn CaptureBackend>,
        mut req: CaptureRequest,
        fps: u32,
        audio: bool,
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
        let mut encoder = Encoder::spawn(&RecordArgs {
            width: first.width,
            height: first.height,
            fps,
            audio,
            output,
        })?;
        let stdin = encoder.take_stdin()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, pacer_done) = mpsc::channel();
        std::thread::Builder::new()
            .name("pacer".into())
            .spawn(move || {
                let _ = tx.send(pacer::run(stdin, &slot, fps, &flag));
            })?;
        Ok(Self {
            encoder,
            capture,
            region,
            stop,
            pacer_done,
        })
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

    /// True once the recording can no longer continue; `stop` reports the reason.
    pub fn has_failed(&mut self) -> bool {
        self.encoder.has_exited() || self.capture.is_finished()
    }

    /// Finalises the file and returns its path.
    pub fn stop(mut self) -> Result<PathBuf> {
        self.stop.store(true, Ordering::Relaxed);
        // The pacer may be stuck writing to a hung ffmpeg, so never join blindly.
        let paced = self.pacer_done.recv_timeout(STOP_TIMEOUT);
        let captured = self.capture.stop();
        if paced.is_err() {
            self.encoder.kill();
            bail!("ffmpeg stopped accepting frames");
        }
        let path = self.encoder.finish(STOP_TIMEOUT)?;
        paced?.context("writing frames to ffmpeg")?;
        captured.context("screen capture ended early")?;
        Ok(path)
    }
}
