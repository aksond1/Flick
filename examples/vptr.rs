//! Test helper: drives a virtual pointer. Use only inside a nested compositor.
//! `VPTR_STEP` is the pause after each step in ms, 40 by default.
//! `VPTR_SIZE=WxH` is the output size, 1920x1080 by default.
//! Steps: `m X Y` move, `d` press, `u` release, `s MS` sleep, `w N` wheel
//! N notches down, `W N` up.
use std::{env, thread::sleep, time::Duration};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{
        wl_pointer::{Axis, AxisSource, ButtonState},
        wl_registry,
    },
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1 as Manager,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1 as Pointer,
};

#[derive(Default)]
struct State(Option<Manager>);

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = e
            && interface == "zwlr_virtual_pointer_manager_v1"
        {
            s.0 = Some(r.bind(name, 1, qh, ()));
        }
    }
}
delegate_noop!(State: Manager);
delegate_noop!(State: Pointer);

fn main() {
    let size = env::var("VPTR_SIZE").unwrap_or("1920x1080".into());
    let (w, h) = size.split_once('x').unwrap();
    let (w, h): (u32, u32) = (w.parse().unwrap(), h.parse().unwrap());
    let step = env::var("VPTR_STEP").map_or(40, |s| s.parse().unwrap());
    let step = Duration::from_millis(step);
    let conn = Connection::connect_to_env().unwrap();
    let mut q = conn.new_event_queue();
    conn.display().get_registry(&q.handle(), ());
    let mut s = State::default();
    q.roundtrip(&mut s).unwrap();
    let p =
        s.0.as_ref()
            .unwrap()
            .create_virtual_pointer(None, &q.handle(), ());
    let mut a = env::args().skip(1);
    let mut t = 0;
    while let Some(c) = a.next() {
        let mut num = || a.next().unwrap().parse::<u32>().unwrap();
        t += 20;
        match c.as_str() {
            "m" => p.motion_absolute(t, num(), num(), w, h),
            "d" => p.button(t, 0x110, ButtonState::Pressed),
            "u" => p.button(t, 0x110, ButtonState::Released),
            "s" => sleep(Duration::from_millis(num().into())),
            "w" | "W" => {
                let n = num() as i32 * if c == "w" { 1 } else { -1 };
                p.axis_source(AxisSource::Wheel);
                p.axis_discrete(t, Axis::VerticalScroll, f64::from(15 * n), n);
            }
            _ => panic!("unknown step {c}"),
        }
        if c != "s" {
            p.frame();
        }
        q.roundtrip(&mut s).unwrap();
        sleep(step);
    }
}
