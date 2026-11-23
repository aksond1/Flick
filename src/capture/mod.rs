pub mod wlr;

use crate::geometry::Rect;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    /// Physical mode size.
    pub width: i32,
    pub height: i32,
    pub scale: i32,
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub output: String,
    /// Output-local logical region; `None` is the whole output.
    pub region: Option<Rect>,
    pub cursor: bool,
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

/// Holds only the newest frame, so a slow consumer never builds up a queue.
#[derive(Clone, Default)]
pub struct FrameSlot(Arc<Mutex<Option<Arc<Frame>>>>);

impl FrameSlot {
    pub fn put(&self, frame: Frame) {
        *self.0.lock().unwrap() = Some(Arc::new(frame));
    }

    pub fn latest(&self) -> Option<Arc<Frame>> {
        self.0.lock().unwrap().clone()
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
    fn stream(self: Box<Self>, req: CaptureRequest, slot: FrameSlot) -> Result<StreamHandle>;
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
}
