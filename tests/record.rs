//! Fake backend -> real ffmpeg -> ffprobe. Needs no display or audio.
use anyhow::Result;
use flick::{capture::*, geometry::Rect, record::Recorder};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

struct Fake;

impl CaptureBackend for Fake {
    fn outputs(&mut self) -> Result<Vec<OutputInfo>> {
        Ok(vec![])
    }
    fn frame(&mut self, _: &CaptureRequest) -> Result<Frame> {
        Ok(Frame {
            width: 201,
            height: 101,
            data: vec![90; 201 * 101 * 4],
        })
    }
    /// A static screen: one frame, then silence until stopped.
    /// A changed region yields one frame of that size.
    fn stream(
        mut self: Box<Self>,
        r: CaptureRequest,
        region: LiveRegion,
        slot: FrameSlot,
    ) -> Result<StreamHandle> {
        slot.put(self.frame(&r)?);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let t = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
                if let Live::On(Some(r)) = region.get() {
                    slot.put(Frame {
                        width: r.w as u32,
                        height: r.h as u32,
                        data: vec![200; (r.w * r.h * 4) as usize],
                    });
                }
            }
            Ok(())
        });
        Ok(StreamHandle::new(stop, t))
    }
}

fn start(file: PathBuf) -> Result<Recorder> {
    let req = CaptureRequest {
        output: "x".into(),
        region: None,
        cursor: false,
        interval: Duration::ZERO,
    };
    Recorder::start(Box::new(Fake), req, 30, false, file)
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("flick-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn probe(f: &Path, entries: &str) -> String {
    let o = Command::new("ffprobe")
        .args(["-v", "error", "-of", "csv=p=0", "-show_entries", entries])
        .arg(f)
        .output()
        .unwrap();
    String::from_utf8(o.stdout).unwrap().trim().to_owned()
}

#[test]
fn static_screen_gives_constant_rate_video() {
    let d = dir("rec");
    let r = start(d.join("a.mp4")).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    let f = r.stop().unwrap();
    assert_eq!(
        probe(&f, "stream=codec_name,width,height,r_frame_rate"),
        "h264,200,100,30/1"
    );
    let secs: f64 = probe(&f, "format=duration").parse().unwrap();
    assert!((secs - 2.0).abs() < 0.2, "{secs}");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn frame_size_changes_keep_the_video_size() {
    let d = dir("resize");
    let r = start(d.join("a.mp4")).unwrap();
    for (w, h) in [(202, 101), (200, 100), (201, 102)] {
        std::thread::sleep(Duration::from_millis(300));
        r.track_video(Rect::new(0, 0, 500, 500));
        std::thread::sleep(Duration::from_millis(100));
        r.resume_video(Rect::new(0, 0, w, h));
    }
    std::thread::sleep(Duration::from_millis(300));
    let f = r.stop().unwrap();
    assert_eq!(probe(&f, "stream=width,height"), "200,100");
    let secs: f64 = probe(&f, "format=duration").parse().unwrap();
    assert!((secs - 1.5).abs() < 0.2, "{secs}");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn encoder_failure_is_reported() {
    let d = dir("fail");
    // The output path is a directory, so ffmpeg exits at once.
    std::fs::create_dir_all(d.join("a.mp4")).unwrap();
    let mut r = start(d.join("a.mp4")).unwrap();
    for _ in 0..100 {
        if r.has_failed() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(r.has_failed());
    let e = r.stop().unwrap_err().to_string();
    assert!(e.contains("ffmpeg failed"), "{e}");
    std::fs::remove_dir_all(d).unwrap();
}
