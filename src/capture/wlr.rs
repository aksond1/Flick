//! wlr-screencopy backend on its own Wayland connection.
use super::{
    CaptureBackend, CaptureRequest, Frame, FrameSlot, Live, LiveRegion, OutputInfo, StreamHandle,
    track,
};
use anyhow::{Context, Result, bail};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs::{MemfdFlags, memfd_create},
    mm::{MapFlags, ProtFlags, mmap, munmap},
};
use std::{
    fs::File,
    os::fd::AsFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    backend::WaylandError,
    delegate_noop,
    protocol::{
        wl_buffer::WlBuffer,
        wl_output::{self, WlOutput},
        wl_registry::{self, WlRegistry},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
    },
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1,
    zxdg_output_v1::{self, ZxdgOutputV1},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self as frame, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const FRAME_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Spec {
    format: wl_shm::Format,
    width: u32,
    height: u32,
    stride: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Negotiating,
    BufferDone,
    Ready,
    Failed,
}

struct Pending {
    /// Tells events of the current frame from those of a cancelled one.
    id: u64,
    spec: Option<Spec>,
    y_invert: bool,
    phase: Phase,
}

struct State {
    shm: Option<WlShm>,
    manager: Option<ZwlrScreencopyManagerV1>,
    xdg: Option<ZxdgOutputManagerV1>,
    outputs: Vec<(WlOutput, OutputInfo)>,
    pending: Pending,
}

struct ShmBuffer {
    spec: Spec,
    map: Mapping,
    buffer: WlBuffer,
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

/// The buffer's memory, read in place rather than copied out first.
struct Mapping {
    ptr: *mut std::ffi::c_void,
    len: usize,
}

// Only the thread that owns the backend touches it.
unsafe impl Send for Mapping {}

impl Mapping {
    fn new(file: &File, len: usize) -> Result<Self> {
        // SAFETY: a fresh shared read-only mapping of a file of at least `len` bytes.
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ,
                MapFlags::SHARED,
                file,
                0,
            )?
        };
        Ok(Self { ptr, len })
    }

    /// Valid to read between `ready` and the next copy request, while the
    /// compositor leaves the buffer alone.
    fn bytes(&self) -> &[u8] {
        // SAFETY: the mapping lives as long as `self` and is `len` bytes long.
        unsafe { std::slice::from_raw_parts(self.ptr.cast(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: mapped in `new` and no slice of it outlives `self`.
        let _ = unsafe { munmap(self.ptr, self.len) };
    }
}

pub struct WlrBackend {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    buffer: Option<ShmBuffer>,
}

impl WlrBackend {
    pub fn connect() -> Result<Self> {
        let conn = Connection::connect_to_env().context("cannot connect to Wayland")?;
        let mut queue = conn.new_event_queue();
        conn.display().get_registry(&queue.handle(), ());
        let mut state = State {
            shm: None,
            manager: None,
            xdg: None,
            outputs: Vec::new(),
            pending: Pending {
                id: 0,
                spec: None,
                y_invert: false,
                phase: Phase::Negotiating,
            },
        };
        queue.roundtrip(&mut state)?; // globals
        if let Some(xdg) = &state.xdg {
            for (i, (o, _)) in state.outputs.iter().enumerate() {
                xdg.get_xdg_output(o, &queue.handle(), i);
            }
        }
        queue.roundtrip(&mut state)?; // output properties
        if state.manager.is_none() {
            bail!("compositor does not offer zwlr_screencopy_manager_v1 version 3");
        }
        if state.shm.is_none() {
            bail!("compositor does not offer wl_shm");
        }
        Ok(Self {
            conn,
            queue,
            state,
            buffer: None,
        })
    }

    /// Dispatches until the frame leaves `phase`. Returns false if cancelled.
    fn wait(
        &mut self,
        phase: Phase,
        deadline: Option<Instant>,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<bool> {
        loop {
            self.queue.dispatch_pending(&mut self.state)?;
            if self.state.pending.phase != phase {
                return Ok(true);
            }
            if cancel.is_some_and(|c| c()) {
                return Ok(false);
            }
            if deadline.is_some_and(|d| Instant::now() > d) {
                bail!("compositor did not deliver a frame in time");
            }
            retry_later(self.conn.flush())?;
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: 15_000_000,
            };
            let n = {
                let fd = self.conn.as_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                match poll(&mut fds, Some(&timeout)) {
                    Err(rustix::io::Errno::INTR) => 0,
                    r => r?,
                }
            };
            if n > 0 {
                retry_later(guard.read().map(drop))?;
            }
        }
    }

    /// `Ok(None)` means it was cancelled while waiting.
    fn capture(
        &mut self,
        req: &CaptureRequest,
        damage: bool,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Option<Frame>> {
        let qh = self.queue.handle();
        let (output, info) = self
            .state
            .outputs
            .iter()
            .find(|(_, i)| i.name == req.output)
            .with_context(|| format!("no output named {}", req.output))?;
        let (output, info) = (output.clone(), info.clone());
        let id = self.state.pending.id + 1;
        self.state.pending = Pending {
            id,
            spec: None,
            y_invert: false,
            phase: Phase::Negotiating,
        };
        let manager = self.state.manager.as_ref().unwrap();
        let cursor = i32::from(req.cursor);
        let obj = match req.region {
            Some(r) if r.w <= 0 || r.h <= 0 => bail!("empty region"),
            Some(r) => manager.capture_output_region(cursor, &output, r.x, r.y, r.w, r.h, &qh, id),
            None => manager.capture_output(cursor, &output, &qh, id),
        };
        let result = self.finish_capture(&obj, damage, cancel);
        obj.destroy();
        result.with_context(|| format!("capturing {}", info.name))
    }

    fn finish_capture(
        &mut self,
        obj: &ZwlrScreencopyFrameV1,
        damage: bool,
        cancel: Option<&dyn Fn() -> bool>,
    ) -> Result<Option<Frame>> {
        let deadline = Instant::now() + FRAME_TIMEOUT;
        self.wait(Phase::Negotiating, Some(deadline), None)?;
        if self.state.pending.phase == Phase::Failed {
            bail!("compositor rejected the capture");
        }
        let spec = self
            .state
            .pending
            .spec
            .context("no supported shared-memory pixel format")?;
        if self.buffer.as_ref().is_none_or(|b| b.spec != spec) {
            self.buffer = Some(self.allocate(spec)?);
        }
        let buffer = &self.buffer.as_ref().unwrap().buffer;
        if damage {
            obj.copy_with_damage(buffer);
        } else {
            obj.copy(buffer);
        }
        // A static screen produces no damage, so that wait has no deadline.
        let deadline = (!damage).then_some(deadline);
        if !self.wait(Phase::BufferDone, deadline, cancel)? {
            return Ok(None);
        }
        if self.state.pending.phase != Phase::Ready {
            bail!("compositor failed to copy the frame");
        }
        let raw = self.buffer.as_ref().unwrap().map.bytes();
        Ok(Some(Frame {
            width: spec.width,
            height: spec.height,
            data: pack(raw, spec, self.state.pending.y_invert),
        }))
    }

    fn allocate(&self, spec: Spec) -> Result<ShmBuffer> {
        let qh = self.queue.handle();
        let size = spec.stride as u64 * spec.height as u64;
        let file = File::from(memfd_create("flick-frame", MemfdFlags::CLOEXEC)?);
        file.set_len(size)?;
        let map = Mapping::new(&file, usize::try_from(size)?)?;
        let shm = self.state.shm.as_ref().unwrap();
        let pool = shm.create_pool(file.as_fd(), i32::try_from(size)?, &qh, ());
        let buffer = pool.create_buffer(
            0,
            spec.width as i32,
            spec.height as i32,
            spec.stride as i32,
            spec.format,
            &qh,
            (),
        );
        pool.destroy();
        Ok(ShmBuffer { spec, map, buffer })
    }
}

impl CaptureBackend for WlrBackend {
    fn outputs(&mut self) -> Result<Vec<OutputInfo>> {
        Ok(self.state.outputs.iter().map(|(_, i)| i.clone()).collect())
    }

    fn frame(&mut self, req: &CaptureRequest) -> Result<Frame> {
        self.capture(req, false, None)?.context("capture cancelled")
    }

    fn stream(
        mut self: Box<Self>,
        mut req: CaptureRequest,
        region: LiveRegion,
        slot: FrameSlot,
    ) -> Result<StreamHandle> {
        // The first frame is taken immediately; errors surface to the caller.
        slot.put(self.frame(&req)?);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let mut shown = Some(Live::On(req.region));
                while !flag.load(Ordering::Relaxed) {
                    let want = region.get();
                    let win = match want {
                        Live::On(r) => r,
                        Live::Track(screen) => {
                            region.recent().first().map(|r| track::window(*r, screen))
                        }
                    };
                    req.region = win;
                    // A new region is copied at once; only then wait for changes.
                    let damage = shown == Some(want);
                    let cancel = || flag.load(Ordering::Relaxed) || region.get() != want;
                    let start = Instant::now();
                    // A frame finished after a change may show the moving border.
                    if let Some(f) = self.capture(&req, damage, Some(&cancel))?
                        && region.get() == want
                    {
                        shown = Some(want);
                        match (want, win) {
                            (Live::Track(_), Some(win)) => {
                                // Without a border the last frame is kept.
                                if let Some(f) = track::cut(&f, win, &region.recent()) {
                                    slot.put(f);
                                }
                            }
                            _ => slot.put(f),
                        }
                    }
                    // Faster than the video needs only burdens the compositor, except
                    // while tracking, where late frames make the motion uneven.
                    if matches!(want, Live::On(_)) {
                        std::thread::sleep(req.interval.saturating_sub(start.elapsed()));
                    }
                }
                Ok(())
            })?;
        Ok(StreamHandle::new(stop, thread))
    }
}

/// The socket being momentarily full or empty is not an error.
fn retry_later(r: Result<(), WaylandError>) -> Result<(), WaylandError> {
    match r {
        Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
        r => r,
    }
}

fn supported(f: wl_shm::Format) -> bool {
    use wl_shm::Format::*;
    matches!(f, Xrgb8888 | Argb8888 | Xbgr8888 | Abgr8888)
}

/// Removes stride padding, fixes orientation and converts to BGRX.
fn pack(raw: &[u8], spec: Spec, y_invert: bool) -> Vec<u8> {
    let row = spec.width as usize * 4;
    let rows = raw.chunks_exact(spec.stride as usize).map(|r| &r[..row]);
    let mut out = Vec::with_capacity(row * spec.height as usize);
    if y_invert {
        rows.rev().for_each(|r| out.extend_from_slice(r));
    } else {
        rows.for_each(|r| out.extend_from_slice(r));
    }
    if matches!(
        spec.format,
        wl_shm::Format::Xbgr8888 | wl_shm::Format::Abgr8888
    ) {
        out.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p.swap(0, 2));
    }
    out
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        reg: &WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = e
        {
            match interface.as_str() {
                "wl_shm" => s.shm = Some(reg.bind(name, 1, qh, ())),
                "zwlr_screencopy_manager_v1" if version >= 3 => {
                    s.manager = Some(reg.bind(name, 3, qh, ()))
                }
                "zxdg_output_manager_v1" if version >= 2 => {
                    s.xdg = Some(reg.bind(name, version.min(3), qh, ()))
                }
                "wl_output" if version >= 4 => {
                    let info = OutputInfo {
                        name: String::new(),
                        width: 0,
                        height: 0,
                        scale: 1,
                        logical: (0, 0),
                    };
                    s.outputs.push((reg.bind(name, 4, qh, ()), info));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlOutput, ()> for State {
    fn event(
        s: &mut Self,
        out: &WlOutput,
        e: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some((_, i)) = s.outputs.iter_mut().find(|(o, _)| o == out) else {
            return;
        };
        match e {
            wl_output::Event::Name { name } => i.name = name,
            wl_output::Event::Scale { factor } => i.scale = factor,
            wl_output::Event::Mode {
                flags: WEnum::Value(f),
                width,
                height,
                ..
            } if f.contains(wl_output::Mode::Current) => {
                i.width = width;
                i.height = height;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, u64> for State {
    fn event(
        s: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        e: frame::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let p = &mut s.pending;
        if p.id != *id {
            return;
        }
        match e {
            frame::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } if supported(format) => {
                p.spec.get_or_insert(Spec {
                    format,
                    width,
                    height,
                    stride,
                });
            }
            frame::Event::Flags {
                flags: WEnum::Value(f),
            } => p.y_invert = f.contains(frame::Flags::YInvert),
            frame::Event::BufferDone => p.phase = Phase::BufferDone,
            frame::Event::Ready { .. } => p.phase = Phase::Ready,
            frame::Event::Failed => p.phase = Phase::Failed,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ZwlrScreencopyManagerV1);
delegate_noop!(State: ZxdgOutputManagerV1);

/// The data is the output's index in `State::outputs`.
impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(
        s: &mut Self,
        _: &ZxdgOutputV1,
        e: zxdg_output_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let (zxdg_output_v1::Event::LogicalSize { width, height }, Some((_, info))) =
            (e, s.outputs.get_mut(*i))
        {
            info.logical = (width, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_strips_stride_flips_and_swaps() {
        let spec = Spec {
            format: wl_shm::Format::Xbgr8888,
            width: 1,
            height: 2,
            stride: 8,
        };
        let raw = [1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0];
        assert_eq!(pack(&raw, spec, false), [3, 2, 1, 4, 7, 6, 5, 8]);
        assert_eq!(pack(&raw, spec, true), [7, 6, 5, 8, 3, 2, 1, 4]);
    }
}
