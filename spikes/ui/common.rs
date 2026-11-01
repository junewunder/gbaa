//! What both UI spikes share: a stand-in for the game (a thread making a
//! moving 240×160 test picture at the GBA's 59.73 Hz, paced by its own
//! clock as the audio paces the real one), the pad read through gilrs, the
//! menu's model, and timing statistics. Included by each spike with
//! `#[path]`; not part of gbaa.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const W: usize = 240;
pub const H: usize = 160;
/// The GBA's frame period: 280,896 cycles at 2^24 Hz.
pub const FRAME: Duration = Duration::from_nanos(16_742_706);

/// Between the game thread and the window's.
pub struct Shared {
    /// The newest frame, as `0RGB`, and its number.
    pub frame: Mutex<(u64, Vec<u32>)>,
    /// The menu is open: the game stands still.
    pub paused: AtomicBool,
    pub quit: AtomicBool,
    /// Frames the game made.
    pub made: AtomicU64,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            frame: Mutex::new((0, vec![0; W * H])),
            paused: AtomicBool::new(false),
            quit: AtomicBool::new(false),
            made: AtomicU64::new(0),
        })
    }
}

/// Run the stand-in game until `quit`, calling `wake` after each frame
/// (an event-loop proxy, so the window presents at once).
pub fn spawn_game(shared: Arc<Shared>, wake: impl Fn() + Send + 'static) {
    std::thread::spawn(move || {
        let mut buf = vec![0u32; W * H];
        let mut n = 0u64;
        let mut next = Instant::now();
        while !shared.quit.load(Ordering::Relaxed) {
            if !shared.paused.load(Ordering::Relaxed) {
                n += 1;
                pattern(n, &mut buf);
                let mut f = shared.frame.lock().unwrap();
                f.0 = n;
                f.1.copy_from_slice(&buf);
                drop(f);
                shared.made.fetch_add(1, Ordering::Relaxed);
                wake();
            }
            next += FRAME;
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
    });
}

/// A picture that shows dropped or doubled frames: a bar sweeping one
/// pixel a frame over a checkerboard, and the frame number in binary.
pub fn pattern(n: u64, buf: &mut [u32]) {
    let bar = (n as usize) % W;
    for y in 0..H {
        for x in 0..W {
            let c = if x == bar || x + 1 == bar {
                0xffffff
            } else if (x / 8 + y / 8) % 2 == 0 {
                0x3050a0
            } else {
                0x204070
            };
            buf[y * W + x] = c;
        }
    }
    for bit in 0..16 {
        let c = if n >> bit & 1 != 0 {
            0xf0c020
        } else {
            0x401010
        };
        for y in 4..10 {
            for x in 0..6 {
                buf[y * W + 4 + (15 - bit) * 8 + x] = c;
            }
        }
    }
}

/// The largest whole scale of 240×160 that fits `w`×`h`, and the margins
/// that centre it (as gbaa-desktop's `fit`).
pub fn fit(w: usize, h: usize) -> (usize, usize, usize) {
    let s = (w / W).min(h / H).max(1);
    (s, w.saturating_sub(W * s) / 2, h.saturating_sub(H * s) / 2)
}

/// A menu control, from a pad, a keyboard or a mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    /// Open or close the menu.
    Menu,
    /// Toggle fullscreen (F11).
    Fullscreen,
    /// End the run (scripts).
    Quit,
}

/// Send the controls `SPIKE_NAV` lists (space-separated: up, down, left,
/// right, ok, back, menu, full, quit, or a number of milliseconds to
/// wait; 250 ms between the others), for runs nobody watches.
pub fn spawn_script(send: impl Fn(Nav) + Send + 'static) {
    let Ok(script) = std::env::var("SPIKE_NAV") else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        for w in script.split_whitespace() {
            if let Ok(ms) = w.parse() {
                std::thread::sleep(Duration::from_millis(ms));
                continue;
            }
            send(match w {
                "up" => Nav::Up,
                "down" => Nav::Down,
                "left" => Nav::Left,
                "right" => Nav::Right,
                "ok" => Nav::Ok,
                "back" => Nav::Back,
                "menu" => Nav::Menu,
                "full" => Nav::Fullscreen,
                "quit" => Nav::Quit,
                _ => panic!("SPIKE_NAV: {w}?"),
            });
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

/// Read the pads on their own thread, as gilrs events turned into [`Nav`]s
/// for `send`. The menu opens with Start+Select (or the Guide button).
pub fn spawn_pad(send: impl Fn(Nav) + Send + 'static) {
    std::thread::spawn(move || {
        use gilrs::{Button, EventType, Gilrs};
        let Ok(mut g) = Gilrs::new() else {
            eprintln!("spike: no gilrs");
            return;
        };
        for (_, pad) in g.gamepads() {
            eprintln!("spike: pad {}", pad.name());
        }
        let (mut start, mut select) = (false, false);
        loop {
            while let Some(e) = g.next_event_blocking(Some(Duration::from_millis(100))) {
                let (b, down) = match e.event {
                    EventType::ButtonPressed(b, _) => (b, true),
                    EventType::ButtonReleased(b, _) => (b, false),
                    _ => continue,
                };
                match b {
                    Button::Start => start = down,
                    Button::Select => select = down,
                    _ => {}
                }
                if !down {
                    continue;
                }
                let nav = match b {
                    Button::DPadUp => Nav::Up,
                    Button::DPadDown => Nav::Down,
                    Button::DPadLeft => Nav::Left,
                    Button::DPadRight => Nav::Right,
                    Button::South => Nav::Ok,
                    Button::East => Nav::Back,
                    Button::Mode => Nav::Menu,
                    Button::Start | Button::Select if start && select => Nav::Menu,
                    _ => continue,
                };
                send(nav);
            }
        }
    });
}

pub const SPEEDS: [f32; 7] = [1.5, 2.0, 2.5, 3.0, 4.0, 8.0, 0.0];

/// The menu's state, kept by the spike whatever draws it.
#[derive(Default)]
pub struct Settings {
    pub speed: usize,
    pub slot: u8,
    pub fullscreen: bool,
    pub status: String,
}

impl Settings {
    pub fn speed_label(&self) -> String {
        match SPEEDS[self.speed] {
            0.0 => "unlimited".into(),
            s => format!("{s}x"),
        }
    }
}

/// Intervals between presents, to show hitching.
#[derive(Default)]
pub struct Stats {
    last: Option<Instant>,
    gaps: Vec<f64>,
}

impl Stats {
    pub fn present(&mut self) {
        let now = Instant::now();
        if let Some(t) = self.last.replace(now) {
            self.gaps.push((now - t).as_secs_f64() * 1000.0);
        }
    }

    /// Forget the gaps so far (the menu opening and closing).
    pub fn skip(&mut self) {
        self.last = None;
    }

    pub fn report(&self, what: &str) {
        let mut g = self.gaps.clone();
        if g.is_empty() {
            return;
        }
        g.sort_by(f64::total_cmp);
        let mean = g.iter().sum::<f64>() / g.len() as f64;
        let p = |q: f64| g[((g.len() - 1) as f64 * q) as usize];
        let late = g.iter().filter(|&&x| x > 25.0).count();
        eprintln!(
            "spike {what}: {} presents, gap mean {mean:.2} ms, p50 {:.2}, p99 {:.2}, max {:.2}, over 25 ms: {late}",
            g.len() + 1,
            p(0.5),
            p(0.99),
            g[g.len() - 1],
        );
    }
}

/// How long drawing one present took, in microseconds, to compare costs.
#[derive(Default)]
pub struct Cost {
    us: Vec<u128>,
}

impl Cost {
    pub fn add(&mut self, t: Instant) {
        self.us.push(t.elapsed().as_micros());
    }

    pub fn report(&self, what: &str) {
        let mut u = self.us.clone();
        if u.is_empty() {
            return;
        }
        u.sort();
        eprintln!(
            "spike {what}: draw p50 {} us, p99 {} us, max {} us",
            u[u.len() / 2],
            u[(u.len() - 1) * 99 / 100],
            u[u.len() - 1]
        );
    }
}
