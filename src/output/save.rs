use crate::capture::Frame;
use anyhow::{Context, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn encode_png(frame: &Frame) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, frame.width, frame.height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&frame.to_rgb())?;
    Ok(out)
}

/// Reserves `<stem>.<ext>` (or `<stem>-N.<ext>`) without ever overwriting a file.
pub fn unique_path(dir: &Path, stem: &str, ext: &str) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    for n in 0u32.. {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let path = dir.join(format!("{stem}{suffix}.{ext}"));
        match fs::File::create_new(&path) {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("cannot write {}", path.display())),
        }
    }
    unreachable!()
}
