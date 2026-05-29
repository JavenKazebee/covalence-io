pub mod nodes;

use async_trait::async_trait;
use shared::{Event, Message, Value};
use tokio::sync::broadcast;
use tokio::time::{Duration, Instant, MissedTickBehavior};
use tracing::info;

use crate::drivers::{Driver, DriverContext};
use crate::nodes::Node;

pub enum TimerMode {
    /// Counts up from zero, emitting `elapsed_ms` on each tick.
    CountUp,
    /// Counts down from `from_ms`, emitting `remaining_ms` and `finished`.
    CountDown { from_ms: u64 },
    /// Fires `tick_count` every `every_ms`, regardless of elapsed time tracking.
    Interval { every_ms: u64 },
}

/// Internal mutable state for a running timer.
struct TimerState {
    running: bool,
    /// Accumulated ms before the last pause.
    accumulated_ms: u64,
    /// When the timer was last resumed (None if stopped).
    last_resume: Option<Instant>,
}

impl TimerState {
    fn new(auto_start: bool) -> Self {
        Self {
            running: auto_start,
            accumulated_ms: 0,
            last_resume: if auto_start {
                Some(Instant::now())
            } else {
                None
            },
        }
    }

    fn elapsed_ms(&self) -> u64 {
        let since_resume = self
            .last_resume
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        self.accumulated_ms + since_resume
    }

    fn start(&mut self) {
        if !self.running {
            self.last_resume = Some(Instant::now());
            self.running = true;
        }
    }

    fn stop(&mut self) {
        if self.running {
            self.accumulated_ms = self.elapsed_ms();
            self.last_resume = None;
            self.running = false;
        }
    }

    fn reset(&mut self) {
        self.accumulated_ms = 0;
        self.last_resume = None;
        self.running = false;
    }

    fn set_elapsed(&mut self, ms: u64) {
        self.accumulated_ms = ms;
        if self.running {
            // Reset the resume anchor so elapsed continues from the new value
            self.last_resume = Some(Instant::now());
        }
    }
}

pub struct TimerDriver {
    id: String,
    mode: TimerMode,
    /// How often the internal loop ticks (cache update resolution).
    /// For CountUp/CountDown: controls smoothness of updates (default 100ms).
    /// For Interval: determines the minimum latency of tick detection.
    resolution_ms: u64,
    auto_start: bool,
}

impl TimerDriver {
    pub fn new(id: &str, mode: TimerMode, auto_start: bool) -> Self {
        Self {
            id: id.to_string(),
            mode,
            resolution_ms: 100,
            auto_start,
        }
    }

    pub fn with_resolution(mut self, resolution_ms: u64) -> Self {
        self.resolution_ms = resolution_ms;
        self
    }
}

#[async_trait]
impl Driver for TimerDriver {
    fn id(&self) -> &str {
        &self.id
    }

    fn nodes(&self) -> Vec<Box<dyn Node>> {
        vec![Box::new(nodes::TimerControlNode)]
    }

    async fn start(
        &self,
        context: DriverContext,
        rx: broadcast::Receiver<Message>,
    ) -> Result<(), String> {
        let mut rx = rx;
        info!(
            driver = self.id,
            mode = match &self.mode {
                TimerMode::CountUp => "countup",
                TimerMode::CountDown { .. } => "countdown",
                TimerMode::Interval { .. } => "interval",
            },
            "Timer driver starting"
        );

        let mut state = TimerState::new(self.auto_start);
        let mut tick = tokio::time::interval(Duration::from_millis(self.resolution_ms));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        // Tracks the last whole-interval boundary emitted (Interval mode only)
        let mut last_tick_count: u64 = 0;

        // Emit initial state
        context.update("running", Value::Bool(state.running));
        match &self.mode {
            TimerMode::CountUp => {
                context.update("elapsed_ms", Value::Float(0.0));
            }
            TimerMode::CountDown { from_ms } => {
                context.update("elapsed_ms", Value::Float(0.0));
                context.update("remaining_ms", Value::Float(*from_ms as f64));
                context.update("finished", Value::Bool(false));
            }
            TimerMode::Interval { .. } => {
                context.update("tick_count", Value::Float(0.0));
            }
        }

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if !state.running {
                        continue;
                    }

                    let elapsed = state.elapsed_ms();

                    match &self.mode {
                        TimerMode::CountUp => {
                            context.update("elapsed_ms", Value::Float(elapsed as f64));
                        }
                        TimerMode::CountDown { from_ms } => {
                            let remaining = from_ms.saturating_sub(elapsed);
                            context.update("elapsed_ms", Value::Float(elapsed as f64));
                            context.update("remaining_ms", Value::Float(remaining as f64));
                            if remaining == 0 {
                                context.update("finished", Value::Bool(true));
                                context.update("running", Value::Bool(false));
                                state.stop();
                            }
                        }
                        TimerMode::Interval { every_ms } => {
                            // Compute how many full intervals have elapsed; emit
                            // once for each new boundary crossed since the last emit.
                            let current_tick_count = elapsed / every_ms;
                            if current_tick_count > last_tick_count {
                                last_tick_count = current_tick_count;
                                context.update("tick_count", Value::Float(last_tick_count as f64));
                            }
                        }
                    }
                }

                result = rx.recv() => {
                    match result {
                        Ok(msg) => {
                            if let Event::Command { target, name, params } = msg.payload {
                                if target == self.id || target == "all" {
                                    match name.as_str() {
                                        "start" => {
                                            state.start();
                                            context.update("running", Value::Bool(true));
                                            // Re-arm finished for countdown if restarted
                                            if let TimerMode::CountDown { .. } = &self.mode {
                                                context.update("finished", Value::Bool(false));
                                            }
                                        }
                                        "stop" => {
                                            state.stop();
                                            context.update("running", Value::Bool(false));
                                        }
                                        "reset" => {
                                            state.reset();
                                            last_tick_count = 0;
                                            context.update("running", Value::Bool(false));
                                            match &self.mode {
                                                TimerMode::CountUp => {
                                                    context.update("elapsed_ms", Value::Float(0.0));
                                                }
                                                TimerMode::CountDown { from_ms } => {
                                                    context.update("elapsed_ms", Value::Float(0.0));
                                                    context.update("remaining_ms", Value::Float(*from_ms as f64));
                                                    context.update("finished", Value::Bool(false));
                                                }
                                                TimerMode::Interval { .. } => {
                                                    context.update("tick_count", Value::Float(0.0));
                                                }
                                            }
                                        }
                                        "set_elapsed" => {
                                            if let Some(Value::Float(ms)) = params.get("ms") {
                                                state.set_elapsed(*ms as u64);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        Ok(())
    }
}
