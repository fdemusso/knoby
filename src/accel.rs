use std::time::Instant;

const SLOW_MS: f32 = 120.0;
const FAST_MS: f32 = 30.0;
const MIN_STEP: f32 = 0.01;
const MAX_STEP: f32 = 0.05;
const RESET_MS: f32 = 400.0;

/// Maps the time between knob detents to a volume step: slow turns give
/// `MIN_STEP`, fast turns ramp up to `MAX_STEP`.
#[derive(Default)]
pub struct Accel {
    last: Option<Instant>,
    avg_ms: f32,
}

impl Accel {
    pub fn step(&mut self, now: Instant) -> f32 {
        let dt = self
            .last
            .map_or(f32::MAX, |l| now.duration_since(l).as_secs_f32() * 1000.0);
        self.last = Some(now);
        self.avg_ms = if dt >= RESET_MS {
            dt.min(RESET_MS)
        } else {
            0.5 * self.avg_ms + 0.5 * dt
        };
        let t = ((SLOW_MS - self.avg_ms) / (SLOW_MS - FAST_MS)).clamp(0.0, 1.0);
        MIN_STEP + (MAX_STEP - MIN_STEP) * t
    }
}
