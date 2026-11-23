//! Turns "newest frame" into constant frame rate on a monotonic clock.
use super::fit::fit;
use crate::capture::{Frame, FrameSlot};
use anyhow::{Context, Result};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Frames that must have been written after `elapsed`; the first is due at zero.
pub fn frames_due(elapsed: Duration, fps: u32) -> u64 {
    (elapsed.as_nanos() * fps as u128 / 1_000_000_000) as u64 + 1
}

fn due_at(index: u64, fps: u32) -> Duration {
    Duration::from_nanos((index as u128 * 1_000_000_000).div_ceil(fps as u128) as u64)
}

/// Writes frames until `stop`; returns how many. Dropping `sink` signals EOF.
pub fn run(mut sink: impl Write, slot: &FrameSlot, fps: u32, stop: &AtomicBool) -> Result<u64> {
    let first = slot.latest().context("no frame to record")?;
    let start = Instant::now();
    let mut written = 0;
    // The last frame that needed fitting, with its result.
    let mut fitted: Option<(Arc<Frame>, Vec<u8>)> = None;
    while !stop.load(Ordering::Relaxed) {
        let due = frames_due(start.elapsed(), fps);
        while written < due {
            let frame = slot.latest().unwrap_or_else(|| first.clone());
            if (frame.width, frame.height) == (first.width, first.height) {
                sink.write_all(&frame.data)?;
            } else {
                if fitted.as_ref().is_none_or(|(f, _)| !Arc::ptr_eq(f, &frame)) {
                    let data = fit(&frame, first.width, first.height);
                    fitted = Some((frame, data));
                }
                sink.write_all(&fitted.as_ref().unwrap().1)?;
            }
            written += 1;
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        std::thread::sleep(due_at(written, fps).saturating_sub(start.elapsed()));
    }
    sink.flush()?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule() {
        assert_eq!(frames_due(Duration::ZERO, 30), 1);
        assert_eq!(frames_due(Duration::from_millis(33), 30), 1);
        assert_eq!(frames_due(Duration::from_millis(34), 30), 2);
        assert_eq!(frames_due(Duration::from_secs(3600), 60), 216_001);
        for fps in [24, 30, 60, 144] {
            for i in 0..1000 {
                assert_eq!(frames_due(due_at(i, fps), fps), i + 1);
            }
        }
    }
}
