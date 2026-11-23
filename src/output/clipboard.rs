use anyhow::{Context, Result, bail};
use std::{
    fmt::Write as _,
    io::Write,
    os::unix::ffi::OsStrExt,
    path::Path,
    process::{Command, Stdio},
};

pub fn copy_png(png: &[u8]) -> Result<()> {
    wl_copy("image/png", png)
}

pub fn copy_file(path: &Path) -> Result<()> {
    let uri = file_uri(&path.canonicalize()?);
    wl_copy("text/uri-list", format!("{uri}\r\n").as_bytes())
}

/// wl-copy forks, so the content outlives Flick.
fn wl_copy(mime: &str, data: &[u8]) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", mime])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot start wl-copy")?;
    child.stdin.take().unwrap().write_all(data)?;
    let status = child.wait()?;
    if !status.success() {
        bail!("wl-copy failed ({status})");
    }
    Ok(())
}

fn file_uri(path: &Path) -> String {
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            let _ = write!(s, "%{b:02X}");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_is_percent_encoded() {
        assert_eq!(
            file_uri(Path::new("/a b/ї#.mp4")),
            "file:///a%20b/%D1%97%23.mp4"
        );
    }
}
