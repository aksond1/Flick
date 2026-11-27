use crate::capture::Frame;
use anyhow::{Context, Result, ensure};
use gtk::glib;
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

/// Puts `from` at `to`, replacing it. A hard link is instant, so a copy is made
/// only across file systems.
pub fn link_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    let mut name = to.as_os_str().to_owned();
    name.push(".flick-link");
    let tmp = PathBuf::from(name);
    let _ = fs::remove_file(&tmp);
    if fs::hard_link(from, &tmp).is_ok() {
        return fs::rename(&tmp, to).inspect_err(|_| drop(fs::remove_file(&tmp)));
    }
    fs::copy(from, to).map(drop)
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

/// A file name, without extension, from a `strftime`-style pattern and the time now.
pub fn stem(pattern: &str) -> Result<String> {
    let now = glib::DateTime::now_local()?;
    let s = now
        .format(pattern)
        .ok()
        .filter(|s| !s.trim().is_empty() && !s.contains('/'));
    ensure!(s.is_some(), "invalid file name pattern {pattern:?}");
    Ok(s.unwrap().into())
}

/// Writes `data` to a new file in `dir`, named after `pattern`.
pub fn write_new(dir: &Path, pattern: &str, ext: &str, data: &[u8]) -> Result<PathBuf> {
    let path = unique_path(dir, &stem(pattern)?, ext)?;
    write(&path, data)?;
    Ok(path)
}

/// Replaces the content of `path`.
pub fn write(path: &Path, data: &[u8]) -> Result<()> {
    fs::write(path, data).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_or_copy_replaces_the_target() {
        let d = std::env::temp_dir().join(format!("flick-link-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let (from, to) = (d.join("a.mp4"), d.join("b.mp4"));
        fs::write(&from, "new").unwrap();
        fs::write(&to, "").unwrap();
        link_or_copy(&from, &to).unwrap();
        fs::remove_file(&from).unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "new");
        assert_eq!(fs::read_dir(&d).unwrap().count(), 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn stems_follow_the_pattern() {
        assert_eq!(stem("shot").unwrap(), "shot");
        assert_eq!(stem("%Y").unwrap().len(), 4);
        assert!(stem("a/%Y").is_err());
        assert!(stem("").is_err());
    }

    #[test]
    fn new_files_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("flick-save-{}", std::process::id()));
        let a = write_new(&dir, "x", "png", b"1").unwrap();
        let b = write_new(&dir, "x", "png", b"2").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read(&a).unwrap(), b"1");
        fs::remove_dir_all(dir).unwrap();
    }
}
