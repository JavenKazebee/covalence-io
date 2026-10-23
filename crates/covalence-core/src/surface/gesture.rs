//! Derives events from a surface's input state (surfaces spec §4.2).
//!
//! Each change to a control's input state (`pressed`, `detents`, `value`,
//! `touched`) gives its edge events. Timing between changes comes from the
//! host's timestamps, so jitter between the device and the core doesn't
//! change what a gesture is. `long` and `hold` need a timer, because nothing
//! arrives while a key is held: the caller sleeps until
//! [`Recogniser::deadline`] and then calls [`Recogniser::tick`].

use std::collections::BTreeMap;
use std::time::Duration;

use covalence_protocol::Value;
use tokio::time::Instant;

use crate::journal::ControlEvent;

/// Gesture timing (surfaces spec §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// A press this soon after the previous release is also a `double`.
    pub double_window: Duration,
    /// How long a control is pressed before `long`, and the first `hold`.
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

/// A change to one control's input state (surfaces spec §3).
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Pressed(bool),
    /// By how many detents `detents` moved.
    Turned(i64),
    Value(Value),
    Touched(bool),
}

/// One event on one control.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub control: String,
    pub event: ControlEvent,
    /// `hold`: the repeat count, from 1. `turn`: the change in detents.
    pub delta: Option<i64>,
    /// `change`: the new value.
    pub value: Option<Value>,
}

/// Event state for one attached surface.
#[derive(Default)]
pub struct Recogniser {
    timing: Timing,
    controls: BTreeMap<String, Control>,
}

#[derive(Default)]
struct Control {
    down: Option<Down>,
    /// The host's `ts` of the last release, while a `double` is still possible.
    last_up: Option<f64>,
    touched: bool,
}

struct Down {
    /// The host's `ts`.
    ts: f64,
    /// When the core received it, for the `long` and `hold` timer.
    at: Instant,
    /// `hold`s fired so far. The first comes with `long`.
    holds: i64,
}

impl Down {
    /// When the next `hold` (and, for the first, `long`) is due.
    fn due(&self, timing: &Timing) -> Instant {
        self.at + timing.long_after + timing.hold_every * self.holds as u32
    }

    /// Fires the next `hold`, with `long` before the first.
    fn fire(&mut self, control: &str, out: &mut Vec<Detected>) {
        if self.holds == 0 {
            out.push(detected(control, ControlEvent::Long));
        }
        self.holds += 1;
        out.push(Detected {
            delta: Some(self.holds),
            ..detected(control, ControlEvent::Hold)
        });
    }
}

fn detected(control: &str, event: ControlEvent) -> Detected {
    Detected {
        control: control.to_owned(),
        event,
        delta: None,
        value: None,
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

    /// The events one change gives, in order. `ts` is the host's timestamp
    /// in milliseconds; `now` is when the core received it.
    pub fn change(
        &mut self,
        control: &str,
        change: Change,
        ts: f64,
        now: Instant,
    ) -> Vec<Detected> {
        let timing = self.timing;
        let state = self.controls.entry(control.to_owned()).or_default();
        let mut out = Vec::new();
        match change {
            Change::Pressed(true) => {
                // A missed release: end the earlier press first.
                if state.down.take().is_some() {
                    out.push(detected(control, ControlEvent::Release));
                }
                out.push(detected(control, ControlEvent::Press));
                if state
                    .last_up
                    .take()
                    .is_some_and(|up| ts - up <= millis(timing.double_window))
                {
                    out.push(detected(control, ControlEvent::Double));
                }
                state.down = Some(Down {
                    ts,
                    at: now,
                    holds: 0,
                });
            }
            Change::Pressed(false) => {
                let Some(mut down) = state.down.take() else {
                    return out;
                };
                // A late release can show the control was held longer than
                // the timer has seen: fire what was due before its `ts`.
                let held = ts - down.ts;
                let long = millis(timing.long_after);
                if held >= long {
                    let due = 1 + ((held - long) / millis(timing.hold_every)) as i64;
                    while down.holds < due {
                        down.fire(control, &mut out);
                    }
                }
                out.push(detected(control, ControlEvent::Release));
                state.last_up = Some(ts);
            }
            Change::Turned(0) => {}
            Change::Turned(n) => out.push(Detected {
                delta: Some(n),
                ..detected(control, ControlEvent::Turn)
            }),
            Change::Value(value) => out.push(Detected {
                value: Some(value),
                ..detected(control, ControlEvent::Change)
            }),
            Change::Touched(touched) if touched != state.touched => {
                state.touched = touched;
                let event = if touched {
                    ControlEvent::Touch
                } else {
                    ControlEvent::Untouch
                };
                out.push(detected(control, event));
            }
            Change::Touched(_) => {}
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

    /// The `long` and `hold` events due by `now`.
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

    /// Releases and untouches everything, as when the surface detaches.
    pub fn release_all(&mut self) -> Vec<Detected> {
        let mut out = Vec::new();
        for (control, state) in std::mem::take(&mut self.controls) {
            if state.down.is_some() {
                out.push(detected(&control, ControlEvent::Release));
            }
            if state.touched {
                out.push(detected(&control, ControlEvent::Untouch));
            }
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
                Some(n) => format!("{}:{}{n}", d.control, d.event.as_str()),
                None => format!("{}:{}", d.control, d.event.as_str()),
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
            names(&r.change("k0", Change::Pressed(true), 1000.0, t0)),
            ["k0:press"]
        );
        assert_eq!(r.deadline(), Some(t0 + ms(500)));
        assert_eq!(
            names(&r.change("k0", Change::Pressed(false), 1080.0, t0 + ms(80))),
            ["k0:release"]
        );
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn a_quick_second_press_is_also_a_double() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        r.change("k0", Change::Pressed(false), 80.0, t0);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 300.0, t0)),
            ["k0:press", "k0:double"]
        );
        r.change("k0", Change::Pressed(false), 360.0, t0);
        // The third press starts over; a fourth soon after is a double again.
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 1000.0, t0)),
            ["k0:press"]
        );
        r.change("k0", Change::Pressed(false), 1050.0, t0);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 1100.0, t0)),
            ["k0:press", "k0:double"]
        );
    }

    #[test]
    fn doubles_use_the_hosts_timestamps_not_arrival() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        // Both arrive together after a network stall, but were 500ms apart.
        r.change("k0", Change::Pressed(true), 0.0, t0);
        r.change("k0", Change::Pressed(false), 50.0, t0);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 550.0, t0)),
            ["k0:press"]
        );
        // And the other way: a slow network doesn't break up a real double.
        r.change("k0", Change::Pressed(false), 600.0, t0);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 700.0, t0 + ms(2_000))),
            ["k0:press", "k0:double"]
        );
    }

    #[test]
    fn doubles_are_per_control() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        r.change("k0", Change::Pressed(false), 50.0, t0);
        assert_eq!(
            names(&r.change("k1", Change::Pressed(true), 100.0, t0)),
            ["k1:press"]
        );
    }

    #[test]
    fn holding_fires_long_then_repeats_hold() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        assert!(r.tick(t0 + ms(499)).is_empty());
        assert_eq!(names(&r.tick(t0 + ms(500))), ["k0:long", "k0:hold1"]);
        assert_eq!(r.deadline(), Some(t0 + ms(650)));
        assert_eq!(names(&r.tick(t0 + ms(650))), ["k0:hold2"]);
        // A late tick catches up.
        assert_eq!(names(&r.tick(t0 + ms(960))), ["k0:hold3", "k0:hold4"]);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(false), 1000.0, t0 + ms(1000))),
            ["k0:release"]
        );
    }

    #[test]
    fn a_late_up_fires_what_was_due_before_it() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        // The timer hasn't run, but the host says the key was held 700ms.
        assert_eq!(
            names(&r.change("k0", Change::Pressed(false), 700.0, t0 + ms(100))),
            ["k0:long", "k0:hold1", "k0:hold2", "k0:release"]
        );
        // A long press is still followed by a double window.
        r.change("k0", Change::Pressed(true), 800.0, t0 + ms(200));
        assert_eq!(r.deadline(), Some(t0 + ms(700)));
    }

    #[test]
    fn a_short_press_never_fires_long() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        r.change("k0", Change::Pressed(false), 499.0, t0 + ms(600));
        assert!(r.tick(t0 + ms(2_000)).is_empty());
    }

    #[test]
    fn a_missed_release_ends_the_press_before_the_next() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        assert_eq!(
            names(&r.change("k0", Change::Pressed(true), 100.0, t0)),
            ["k0:release", "k0:press"]
        );
        // A release with no press is ignored.
        r.change("k0", Change::Pressed(false), 150.0, t0);
        assert!(r.change("k0", Change::Pressed(false), 160.0, t0).is_empty());
    }

    #[test]
    fn detaching_releases_what_is_down() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("k0", Change::Pressed(true), 0.0, t0);
        r.change("k1", Change::Pressed(true), 0.0, t0);
        r.change("k1", Change::Pressed(false), 10.0, t0);
        assert_eq!(names(&r.release_all()), ["k0:release"]);
        assert_eq!(r.deadline(), None);
        // No double across a detach.
        assert_eq!(
            names(&r.change("k1", Change::Pressed(true), 50.0, t0)),
            ["k1:press"]
        );
    }

    #[test]
    fn turns_values_and_touches_are_events_of_their_own() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        let turn = r.change("dial", Change::Turned(-2), 0.0, t0);
        assert_eq!(names(&turn), ["dial:turn-2"]);
        assert!(r.change("dial", Change::Turned(0), 1.0, t0).is_empty());
        let change = r.change("fader", Change::Value(0.25.into()), 2.0, t0);
        assert_eq!(names(&change), ["fader:change"]);
        assert_eq!(change[0].value, Some(0.25.into()));
        assert_eq!(
            names(&r.change("fader", Change::Touched(true), 3.0, t0)),
            ["fader:touch"]
        );
        assert!(r.change("fader", Change::Touched(true), 4.0, t0).is_empty());
        assert_eq!(
            names(&r.change("fader", Change::Touched(false), 5.0, t0)),
            ["fader:untouch"]
        );
        // None of them start the timer.
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn detaching_untouches_what_is_touched() {
        let mut r = Recogniser::default();
        let t0 = Instant::now();
        r.change("fader", Change::Touched(true), 0.0, t0);
        r.change("k0", Change::Pressed(true), 0.0, t0);
        assert_eq!(names(&r.release_all()), ["fader:untouch", "k0:release"]);
        assert!(r.release_all().is_empty());
    }
}
