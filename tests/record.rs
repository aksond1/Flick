//! Fake backend -> real ffmpeg -> ffprobe. Needs no display or audio.
use anyhow::Result;
use flick::{
    capture::*,
    record::{
        Recorder,
        export::{VideoFormat, export},
    },
};
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
    fn stream(mut self: Box<Self>, r: CaptureRequest, slot: FrameSlot) -> Result<StreamHandle> {
        slot.put(self.frame(&r)?);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let t = std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
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
fn static_screen_gives_constant_rate_video_and_exports() {
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
    for (fmt, codec) in [(VideoFormat::WebM, "vp9"), (VideoFormat::Gif, "gif")] {
        let out = d.join(format!("o.{}", fmt.extension()));
        export(&f, &out, fmt).unwrap();
        assert_eq!(probe(&out, "stream=codec_name"), codec);
    }
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
