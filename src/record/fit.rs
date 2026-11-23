//! Places a frame into a fixed BGRX canvas.
use crate::capture::Frame;

/// Crops or pads at the right and bottom. Moving a region under fractional
/// scaling changes its pixel size by one, while the video size is fixed.
pub fn fit(f: &Frame, w: u32, h: u32) -> Vec<u8> {
    let (fw, cw) = (f.width as usize * 4, w as usize * 4);
    let mut out = vec![0; cw * h as usize];
    let n = fw.min(cw);
    for y in 0..f.height.min(h) as usize {
        out[y * cw..][..n].copy_from_slice(&f.data[y * fw..][..n]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_and_pads() {
        let mut f = Frame {
            width: 5,
            height: 4,
            data: vec![200; 80],
        };
        f.data[0] = 7;
        let out = fit(&f, 4, 5);
        assert_eq!((out.len(), out[0], out[4], out[4 * 4 * 4]), (80, 7, 200, 0));
    }
}
