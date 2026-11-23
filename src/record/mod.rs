pub mod export;
pub mod ffmpeg;
pub mod pacer;

use crate::capture::{CaptureBackend, CaptureRequest, FrameSlot, StreamHandle};
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
    stop: Arc<AtomicBool>,
    pacer_done: Receiver<Result<u64>>,
}

impl Recorder {
    pub fn start(
        backend: Box<dyn CaptureBackend>,
        req: CaptureRequest,
        fps: u32,
        audio: bool,
        output: PathBuf,
    ) -> Result<Self> {
        if !(1..=240).contains(&fps) {
            bail!("fps must be between 1 and 240");
        }
        let slot = FrameSlot::default();
        let capture = backend.stream(req, slot.clone())?;
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
            stop,
            pacer_done,
        })
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
