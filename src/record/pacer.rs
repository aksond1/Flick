//! Turns captured frames into constant frame rate on a monotonic clock.
use super::fit::fit;
use crate::capture::{Frame, FrameSlot};
use anyhow::{Context, Result};
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// Frames that may wait for a tick; more would only add delay.
const BACKLOG: usize = 2;

/// What the pacer did, for judging whether a frame rate holds.
#[derive(Debug, Default)]
pub struct Stats {
    pub written: AtomicU64,
    /// Written frames that repeat the one before, as the screen had not changed
    /// or the capture fell behind.
    pub repeated: AtomicU64,
    /// The most frames ever due at once; above 1 means ffmpeg made the pacer late.
    pub worst_lag: AtomicU64,
}

/// Frames that must have been written after `elapsed`; the first is due at zero.
pub fn frames_due(elapsed: Duration, fps: u32) -> u64 {
    (elapsed.as_nanos() * fps as u128 / 1_000_000_000) as u64 + 1
}

fn due_at(index: u64, fps: u32) -> Duration {
    Duration::from_nanos((index as u128 * 1_000_000_000).div_ceil(fps as u128) as u64)
}

/// Writes frames of `size` until `stop`; returns how many. Dropping `sink` signals EOF.
pub fn run(
    mut sink: impl Write,
    slot: &FrameSlot,
    fps: u32,
    (width, height): (u32, u32),
    stop: &AtomicBool,
    stats: &Stats,
) -> Result<u64> {
    // Earlier frames, from before a pause, are not part of this segment.
    let (mut seen, first) = slot.newest().context("no frame to record")?;
    let start = Instant::now();
    let mut written = 0;
    // The last frame that needed fitting, with its result.
    let mut fitted: Option<(Arc<Frame>, Vec<u8>)> = None;
    let mut last: Option<Arc<Frame>> = None;
    while !stop.load(Ordering::Relaxed) {
        let due = frames_due(start.elapsed(), fps);
        stats.worst_lag.fetch_max(due - written, Ordering::Relaxed);
        while written < due {
            // Every captured frame gets its own tick, even one that came late
            // together with the next; with nothing new the last one repeats.
            let frame = match slot.next(seen, BACKLOG) {
                Some((n, f)) => {
                    seen = n;
                    f
                }
                None if last.is_some() => {
                    stats.repeated.fetch_add(1, Ordering::Relaxed);
                    last.clone().unwrap()
                }
                None => first.clone(),
            };
            last = Some(frame.clone());
            if (frame.width, frame.height) == (width, height) {
                sink.write_all(&frame.data)?;
            } else {
                if fitted.as_ref().is_none_or(|(f, _)| !Arc::ptr_eq(f, &frame)) {
                    let data = fit(&frame, width, height);
                    fitted = Some((frame, data));
                }
                sink.write_all(&fitted.as_ref().unwrap().1)?;
            }
            written += 1;
            stats.written.fetch_add(1, Ordering::Relaxed);
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
