//! Turns a surface's raw input into gestures (surfaces spec §3).
//!
//! Timing between inputs comes from the host's timestamps, so jitter between
//! the device and the core doesn't change what a gesture is. `long` and
//! `hold` need a timer, because nothing arrives while a key is held: the
//! caller sleeps until [`Recogniser::deadline`] and then calls
//! [`Recogniser::tick`].

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::journal::{Gesture, InputKind};

/// Gesture timing (surfaces spec §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// A `down` this soon after the previous `up` is also a `double`.
    pub double_window: Duration,
    /// How long a control is down before `long`, and the first `hold`.
    pub long_after: Duration,
    /// How often `hold` repeats after that.
    pub hold_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            double_window: Duration::from_millis(300),
            long_after: Duration::from_millis(500),
            hold_every: Duration::from_millis(150),
        }
    }
}

/// One gesture on one control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detected {
    pub control: String,
    pub gesture: Gesture,
    /// `hold`: the repeat count, from 1.
    pub delta: Option<u32>,
}

/// Gesture state for one attached surface.
#[derive(Default)]
pub struct Recogniser {
    timing: Timing,
    controls: BTreeMap<String, Control>,
}

#[derive(Default)]
struct Control {
    down: Option<Down>,
    /// The host's `ts` of the last `up`, while a `double` is still possible.
    last_up: Option<f64>,
}

struct Down {
    /// The host's `ts`.
    ts: f64,
    /// When the core received it, for the `long` and `hold` timer.
    at: Instant,
    /// `hold`s fired so far. The first comes with `long`.
    holds: u32,
}

impl Down {
    /// When the next `hold` (and, for the first, `long`) is due.
    fn due(&self, timing: &Timing) -> Instant {
        self.at + timing.long_after + timing.hold_every * self.holds
    }

    /// Fires the next `hold`, with `long` before the first.
    fn fire(&mut self, control: &str, out: &mut Vec<Detected>) {
        if self.holds == 0 {
            out.push(detected(control, Gesture::Long, None));
        }
        self.holds += 1;
        out.push(detected(control, Gesture::Hold, Some(self.holds)));
    }
}

fn detected(control: &str, gesture: Gesture, delta: Option<u32>) -> Detected {
    Detected {
        control: control.to_owned(),
        gesture,
        delta,
    }
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

impl Recogniser {
    pub fn new(timing: Timing) -> Self {
        Recogniser {
            timing,
            controls: BTreeMap::new(),
        }
    }

    /// The gestures one raw input completes, in order. `ts` is the host's
    /// timestamp in milliseconds; `now` is when the core received it.
    pub fn input(
        &mut self,
        control: &str,
        kind: InputKind,
        ts: f64,
        now: Instant,
    ) -> Vec<Detected> {
        let timing = self.timing;
        let state = self.controls.entry(control.to_owned()).or_default();
        let mut out = Vec::new();
        match kind {
            InputKind::Down => {
                // A missed `up`: end the earlier press first.
                if state.down.take().is_some() {
                    out.push(detected(control, Gesture::Release, None));
                }
                out.push(detected(control, Gesture::Press, None));
                if state
                    .last_up
                    .take()
                    .is_some_and(|up| ts - up <= millis(timing.double_window))
                {
                    out.push(detected(control, Gesture::Double, None));
                }
                state.down = Some(Down {
                    ts,
                    at: now,
                    holds: 0,
                });
            }
            InputKind::Up => {
                let Some(mut down) = state.down.take() else {
                    return out;
                };
                // A late `up` can show the control was held longer than the
                // timer has seen: fire what was due before the `up`'s `ts`.
                let held = ts - down.ts;
                let long = millis(timing.long_after);
                if held >= long {
                    let due = 1 + ((held - long) / millis(timing.hold_every)) as u32;
                    while down.holds < due {
                        down.fire(control, &mut out);
                    }
                }
                out.push(detected(control, Gesture::Release, None));
                state.last_up = Some(ts);
            }
            // Encoders, touch strips and faders come later.
            InputKind::Turn | InputKind::Touch | InputKind::Value => {}
        }
        out
    }

    /// When [`tick`](Self::tick) next has something to fire.
    pub fn deadline(&self) -> Option<Instant> {
        self.controls
            .values()
            .filter_map(|c| c.down.as_ref())
            .map(|d| d.due(&self.timing))
            .min()
    }

    /// The `long` and `hold` gestures due by `now`.
    pub fn tick(&mut self, now: Instant) -> Vec<Detected> {
        let mut out = Vec::new();
        for (control, state) in &mut self.controls {
            if let Some(down) = &mut state.down {
                while down.due(&self.timing) <= now {
                    down.fire(control, &mut out);
                }
            }
        }
        out
    }

    /// Releases every control that's down, as when the surface detaches.
    pub fn release_all(&mut self) -> Vec<Detected> {
        let mut out = Vec::new();
        for (control, state) in &mut self.controls {
            if state.down.take().is_some() {
                out.push(detected(control, Gesture::Release, None));
            }
            state.last_up = None;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(found: &[Detected]) -> Vec<String> {
        found
            .iter()
            .map(|d| match d.delta {
                Some(n) => format!("{}:{}{n}", d.control, d.gesture.as_str()),
                None => format!("{}:{}", d.control, d.gesture.as_str()),
            })
            .collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn press_and_release_fire_straight_away() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 1000.0, t0)),
            ["k0:press"]
        );
        assert_eq!(r.deadline(), Some(t0 + ms(500)));
        assert_eq!(
            names(&r.input("k0", InputKind::Up, 1080.0, t0 + ms(80))),
            ["k0:release"]
        );
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn a_quick_second_press_is_also_a_double() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        r.input("k0", InputKind::Up, 80.0, t0);
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 300.0, t0)),
            ["k0:press", "k0:double"]
        );
        r.input("k0", InputKind::Up, 360.0, t0);
        // The third press starts over; a fourth soon after is a double again.
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 1000.0, t0)),
            ["k0:press"]
        );
        r.input("k0", InputKind::Up, 1050.0, t0);
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 1100.0, t0)),
            ["k0:press", "k0:double"]
        );
    }

    #[test]
    fn doubles_use_the_hosts_timestamps_not_arrival() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        // Both arrive together after a network stall, but were 500ms apart.
        r.input("k0", InputKind::Down, 0.0, t0);
        r.input("k0", InputKind::Up, 50.0, t0);
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 550.0, t0)),
            ["k0:press"]
        );
        // And the other way: a slow network doesn't break up a real double.
        r.input("k0", InputKind::Up, 600.0, t0);
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 700.0, t0 + ms(2_000))),
            ["k0:press", "k0:double"]
        );
    }

    #[test]
    fn doubles_are_per_control() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        r.input("k0", InputKind::Up, 50.0, t0);
        assert_eq!(
            names(&r.input("k1", InputKind::Down, 100.0, t0)),
            ["k1:press"]
        );
    }

    #[test]
    fn holding_fires_long_then_repeats_hold() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        assert!(r.tick(t0 + ms(499)).is_empty());
        assert_eq!(names(&r.tick(t0 + ms(500))), ["k0:long", "k0:hold1"]);
        assert_eq!(r.deadline(), Some(t0 + ms(650)));
        assert_eq!(names(&r.tick(t0 + ms(650))), ["k0:hold2"]);
        // A late tick catches up.
        assert_eq!(names(&r.tick(t0 + ms(960))), ["k0:hold3", "k0:hold4"]);
        assert_eq!(
            names(&r.input("k0", InputKind::Up, 1000.0, t0 + ms(1000))),
            ["k0:release"]
        );
    }

    #[test]
    fn a_late_up_fires_what_was_due_before_it() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        // The timer hasn't run, but the host says the key was held 700ms.
        assert_eq!(
            names(&r.input("k0", InputKind::Up, 700.0, t0 + ms(100))),
            ["k0:long", "k0:hold1", "k0:hold2", "k0:release"]
        );
        // A long press is still followed by a double window.
        r.input("k0", InputKind::Down, 800.0, t0 + ms(200));
        assert_eq!(r.deadline(), Some(t0 + ms(700)));
    }

    #[test]
    fn a_short_press_never_fires_long() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        r.input("k0", InputKind::Up, 499.0, t0 + ms(600));
        assert!(r.tick(t0 + ms(2_000)).is_empty());
    }

    #[test]
    fn a_missed_up_releases_before_the_next_press() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        assert_eq!(
            names(&r.input("k0", InputKind::Down, 100.0, t0)),
            ["k0:release", "k0:press"]
        );
        // An `up` with no `down` is ignored.
        r.input("k0", InputKind::Up, 150.0, t0);
        assert!(r.input("k0", InputKind::Up, 160.0, t0).is_empty());
    }

    #[test]
    fn detaching_releases_what_is_down() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.input("k0", InputKind::Down, 0.0, t0);
        r.input("k1", InputKind::Down, 0.0, t0);
        r.input("k1", InputKind::Up, 10.0, t0);
        assert_eq!(names(&r.release_all()), ["k0:release"]);
        assert_eq!(r.deadline(), None);
        // No double across a detach.
        assert_eq!(
            names(&r.input("k1", InputKind::Down, 50.0, t0)),
            ["k1:press"]
        );
    }
}
