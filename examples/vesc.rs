//! Test helper: presses keys on a virtual keyboard. Nested compositor only.
//! Argument: a compiled XKB keymap (`xkbcli compile-keymap`), then evdev codes
//! with prefixes for modifiers: `c` Ctrl, `a` Alt, `s` Shift (`c44` is Ctrl+Z,
//! `a99` Alt+Print); Escape by default.
use std::{fs::File, io::Write, os::fd::AsFd, thread::sleep, time::Duration};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_registry, wl_seat::WlSeat},
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1 as Manager,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1 as Keyboard,
};

#[derive(Default)]
struct State(Option<Manager>, Option<WlSeat>);

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
        {
            match interface.as_str() {
                "zwp_virtual_keyboard_manager_v1" => s.0 = Some(r.bind(name, 1, qh, ())),
                "wl_seat" => s.1 = Some(r.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}
delegate_noop!(State: Manager);
delegate_noop!(State: Keyboard);
delegate_noop!(State: ignore WlSeat);

fn main() {
    let mut map = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    map.push(0);
    let fd = rustix::fs::memfd_create("keymap", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = File::from(fd);
    file.write_all(&map).unwrap();
    let conn = Connection::connect_to_env().unwrap();
    let mut q = conn.new_event_queue();
    conn.display().get_registry(&q.handle(), ());
    let mut s = State::default();
    q.roundtrip(&mut s).unwrap();
    let k =
        s.0.as_ref()
            .unwrap()
            .create_virtual_keyboard(s.1.as_ref().unwrap(), &q.handle(), ());
    k.keymap(1, file.as_fd(), map.len() as u32);
    q.roundtrip(&mut s).unwrap();
    sleep(Duration::from_millis(200));
    let codes: Vec<String> = std::env::args().skip(2).collect();
    let codes = if codes.is_empty() {
        vec!["1".into()]
    } else {
        codes
    };
    for c in codes {
        let code = c.trim_start_matches(['c', 'a', 's']);
        let mods = c[..c.len() - code.len()]
            .chars()
            .map(|m| match m {
                'c' => 4,
                'a' => 8,
                _ => 1,
            })
            .fold(0, |a, b| a | b);
        k.modifiers(mods, 0, 0, 0);
        for (t, state) in [(1, 1), (60, 0)] {
            k.key(t, code.parse().unwrap(), state);
            q.roundtrip(&mut s).unwrap();
            sleep(Duration::from_millis(60));
        }
        k.modifiers(0, 0, 0, 0);
        q.roundtrip(&mut s).unwrap();
    }
}
