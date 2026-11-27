pub mod track;
pub mod wlr;

use crate::geometry::Rect;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    /// Physical mode size.
    pub width: i32,
    pub height: i32,
    /// The integer `wl_output` scale: 2 for any fractional scale above 1.
    pub scale: i32,
    /// Logical size, which tells fractional scales apart. Zero when unknown,
    /// as in lock files of older versions, so those no longer match.
    #[serde(default)]
    pub logical: (i32, i32),
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub output: String,
    /// Output-local logical region; `None` is the whole output.
    pub region: Option<Rect>,
    pub cursor: bool,
    /// Shortest time between streamed frames; unused for single frames.
    pub interval: Duration,
}

/// Tightly packed BGRX pixels, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Frame {
    /// Cuts out a pixel rectangle, limited to the frame.
    pub fn crop(&self, r: Rect) -> Result<Frame> {
        let clamp = |v: i32, max: u32| v.clamp(0, max as i32) as usize;
        let (x0, x1) = (clamp(r.x, self.width), clamp(r.right(), self.width));
        let (y0, y1) = (clamp(r.y, self.height), clamp(r.bottom(), self.height));
        anyhow::ensure!(x1 > x0 && y1 > y0, "region is outside the screen");
        let row = self.width as usize * 4;
        let data = (y0..y1)
            .flat_map(|y| &self.data[y * row + x0 * 4..y * row + x1 * 4])
            .copied()
            .collect();
        Ok(Frame {
            width: (x1 - x0) as u32,
            height: (y1 - y0) as u32,
            data,
        })
    }

    pub fn to_rgb(&self) -> Vec<u8> {
        self.data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0]])
            .collect()
    }
}

/// Frames a `FrameSlot` keeps: enough to ride out a late frame arriving
/// together with the next one, few enough to never build up a queue.
const KEPT: usize = 4;

/// Kept frames with their numbers, and the number of the last one put.
type Numbered = (VecDeque<(u64, Arc<Frame>)>, u64);

/// The newest few frames, numbered from 1 in the order they arrived.
#[derive(Clone, Default)]
pub struct FrameSlot(Arc<Mutex<Numbered>>);

impl FrameSlot {
    pub fn put(&self, frame: Frame) {
        let mut g = self.0.lock().unwrap();
        g.1 += 1;
        let n = g.1;
        g.0.push_back((n, Arc::new(frame)));
        while g.0.len() > KEPT {
            g.0.pop_front();
        }
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.newest().map(|(_, f)| f)
    }

    /// The newest frame with its number.
    pub fn newest(&self) -> Option<(u64, Arc<Frame>)> {
        self.0.lock().unwrap().0.back().cloned()
    }

    /// The oldest frame after number `seen`, skipping older ones while more
    /// than `backlog` wait, so the delay stays bounded.
    pub fn next(&self, seen: u64, backlog: usize) -> Option<(u64, Arc<Frame>)> {
        let g = self.0.lock().unwrap();
        let waiting = g.0.iter().filter(|(n, _)| *n > seen).count();
        g.0.iter()
            .filter(|(n, _)| *n > seen)
            .nth(waiting.saturating_sub(backlog.max(1)))
            .cloned()
    }

    /// Frames put so far.
    pub fn count(&self) -> u64 {
        self.0.lock().unwrap().1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    On(Option<Rect>),
    /// The region is being dragged across this screen: follow its border.
    Track(Rect),
}

/// Border positions remembered while tracking.
const DRAWN: usize = 20;

/// What a running stream captures; may change at any time.
#[derive(Clone)]
pub struct LiveRegion(Arc<Mutex<(Live, VecDeque<Rect>)>>);

impl LiveRegion {
    pub fn new(region: Option<Rect>) -> Self {
        let drawn = region.into_iter().collect();
        Self(Arc::new(Mutex::new((Live::On(region), drawn))))
    }

    pub fn get(&self) -> Live {
        self.0.lock().unwrap().0
    }

    pub fn track(&self, screen: Rect) {
        self.0.lock().unwrap().0 = Live::Track(screen);
    }

    /// Captures `region` from now on.
    pub fn resume(&self, region: Rect) {
        self.0.lock().unwrap().0 = Live::On(Some(region));
    }

    /// Notes that the border was drawn around `region`.
    pub fn drawn(&self, region: Rect) {
        let d = &mut self.0.lock().unwrap().1;
        d.retain(|r| *r != region);
        d.push_front(region);
        d.truncate(DRAWN);
    }

    /// Recent border positions, newest first.
    pub fn recent(&self) -> Vec<Rect> {
        self.0.lock().unwrap().1.iter().copied().collect()
    }
}

/// Stops the capture thread when dropped.
pub struct StreamHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl StreamHandle {
    pub fn new(stop: Arc<AtomicBool>, thread: JoinHandle<Result<()>>) -> Self {
        Self {
            stop,
            thread: Some(thread),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            Some(Err(_)) => anyhow::bail!("capture thread panicked"),
            None => Ok(()),
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub trait CaptureBackend: Send {
    fn outputs(&mut self) -> Result<Vec<OutputInfo>>;
    fn frame(&mut self, req: &CaptureRequest) -> Result<Frame>;
    /// Starts with `req.region`, then follows `region`.
    fn stream(
        self: Box<Self>,
        req: CaptureRequest,
        region: LiveRegion,
        slot: FrameSlot,
    ) -> Result<StreamHandle>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_takes_the_right_pixels() {
        let f = Frame {
            width: 3,
            height: 2,
            data: (0..24).collect(),
        };
        let c = f.crop(Rect::new(1, 1, 9, 9)).unwrap();
        assert_eq!((c.width, c.height), (2, 1));
        assert_eq!(c.data, (16..24).collect::<Vec<u8>>());
        assert!(f.crop(Rect::new(3, 0, 1, 1)).is_err());
    }

    #[test]
    fn slot_hands_out_frames_in_order() {
        let slot = FrameSlot::default();
        let put = |w| {
            slot.put(Frame {
                width: w,
                height: 1,
                data: vec![],
            })
        };
        let next = |seen, backlog| slot.next(seen, backlog).map(|(n, f)| (n, f.width));
        assert!(next(0, 2).is_none());
        put(10);
        put(11);
        // Both are handed out in turn, then nothing is new.
        assert_eq!(next(0, 2), Some((1, 10)));
        assert_eq!(next(1, 2), Some((2, 11)));
        assert_eq!(next(2, 2), None);
        // Behind by three: the oldest is skipped to keep two waiting at most.
        put(12);
        put(13);
        put(14);
        assert_eq!(next(2, 2), Some((4, 13)));
        // Only four are kept.
        assert_eq!(next(0, 9), Some((2, 11)));
        assert_eq!((slot.newest().map(|(n, _)| n), slot.count()), (Some(5), 5));
    }
}
