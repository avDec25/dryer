//! Physics for the line: a rope simulated as a string of nodes that sags under its
//! own weight and under each card, cards that slide along it and swing from their
//! clips as pendulums, and an occasional breeze.

use std::f32::consts::PI;

/// Fixed integration step (seconds). Small enough to keep everything stable.
pub const SUBSTEP: f32 = 1.0 / 240.0;

/// Below this distance from target (and speed) a spring counts as at rest.
const REST_EPS: f32 = 0.01;

// ---------------------------------------------------------------- rope

/// Rope segments between the two fixed ends.
pub const ROPE_SEGMENTS: usize = 64;
const ROPE_NODES: usize = ROPE_SEGMENTS + 1;
/// Tension between neighbouring nodes. With ROPE_G this gives a light sag of
/// about 12 DIPs (a taut line) and a slowest swing of about 1.2 s.
const ROPE_K: f32 = 11_230.0;
/// The rope's own weight, as an acceleration (DIP/s²).
const ROPE_G: f32 = 263.0;
const ROPE_C: f32 = 4.0;
/// Width of a kick along the rope, in nodes.
const KICK_SPREAD: f32 = 3.0;

/// Vertical displacement of each rope node below the line between the two ends.
pub struct Rope {
    y: [f32; ROPE_NODES],
    v: [f32; ROPE_NODES],
    load: [f32; ROPE_NODES],
    max_v: f32,
    max_a: f32,
}

impl Rope {
    /// A taut, flat rope: it drops into its sag (and bounces) when first shown.
    pub fn new() -> Self {
        Self {
            y: [0.0; ROPE_NODES],
            v: [0.0; ROPE_NODES],
            load: [0.0; ROPE_NODES],
            max_v: 1.0,
            max_a: ROPE_G,
        }
    }

    pub fn clear_loads(&mut self) {
        self.load = [0.0; ROPE_NODES];
    }

    /// Hang a weight (an acceleration, DIP/s²) at fraction `u` (0..1) along the rope.
    pub fn add_load(&mut self, u: f32, force: f32) {
        if !(0.0..=1.0).contains(&u) {
            return;
        }
        let p = u * ROPE_SEGMENTS as f32;
        let i = (p.floor() as usize).min(ROPE_SEGMENTS - 1);
        let t = p - i as f32;
        self.load[i] += force * (1.0 - t);
        self.load[i + 1] += force * t;
    }

    /// Push the rope downward (positive) around fraction `u`.
    pub fn kick(&mut self, u: f32, velocity: f32) {
        let p = u.clamp(0.0, 1.0) * ROPE_SEGMENTS as f32;
        for i in 1..ROPE_SEGMENTS {
            let d = (i as f32 - p) / KICK_SPREAD;
            self.v[i] += velocity * (-d * d).exp();
        }
        self.max_v = self.max_v.max(velocity.abs());
    }

    pub fn step(&mut self, dt: f32) {
        let mut acc = [0.0f32; ROPE_NODES];
        for i in 1..ROPE_SEGMENTS {
            let curve = self.y[i - 1] - 2.0 * self.y[i] + self.y[i + 1];
            acc[i] = ROPE_K * curve + ROPE_G + self.load[i] - ROPE_C * self.v[i];
        }
        let (mut max_v, mut max_a) = (0.0f32, 0.0f32);
        for i in 1..ROPE_SEGMENTS {
            self.v[i] += acc[i] * dt;
            self.y[i] += self.v[i] * dt;
            max_v = max_v.max(self.v[i].abs());
            max_a = max_a.max(acc[i].abs());
        }
        self.max_v = max_v;
        self.max_a = max_a;
    }

    /// Sag (DIPs below the ends) at fraction `u` along the rope.
    pub fn height(&self, u: f32) -> f32 {
        let p = u.clamp(0.0, 1.0) * ROPE_SEGMENTS as f32;
        let i = (p.floor() as usize).min(ROPE_SEGMENTS - 1);
        let t = p - i as f32;
        self.y[i] * (1.0 - t) + self.y[i + 1] * t
    }

    pub fn active(&self) -> bool {
        self.max_v > 0.5 || self.max_a > 5.0
    }
}

/// How hard a card pulls the rope down; taller cards are heavier.
pub fn card_load(card_h: f32) -> f32 {
    900.0 + 8.0 * card_h
}

// ---------------------------------------------------------------- breeze

const GUST_LEN: f32 = 4.0;
/// Peak sideways push of a gust on a card (DIP/s²).
const BREEZE_ACCEL: f32 = 140.0;

/// Occasional gusts of wind. Between gusts nothing moves, so the app can idle.
pub struct Breeze {
    t: f32,
    left: f32,
    len: f32,
    seed: u32,
}

impl Breeze {
    pub fn new(seed: u32) -> Self {
        Self { t: 0.0, left: 0.0, len: GUST_LEN, seed: seed | 1 }
    }

    fn rand(&mut self) -> u32 {
        // xorshift32
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        x
    }

    pub fn start(&mut self) {
        self.len = GUST_LEN + (self.rand() % 2000) as f32 / 1000.0;
        self.left = self.len;
    }

    /// Milliseconds until the next gust.
    pub fn next_delay_ms(&mut self) -> u32 {
        15_000 + self.rand() % 20_000
    }

    pub fn step(&mut self, dt: f32) {
        self.t += dt;
        self.left = (self.left - dt).max(0.0);
    }

    pub fn active(&self) -> bool {
        self.left > 0.0
    }

    /// Sideways push on one card (positive = to the right). `phase` differs per
    /// card so the cards never move in lockstep.
    pub fn force(&self, phase: f32) -> f32 {
        if self.left <= 0.0 {
            return 0.0;
        }
        let progress = 1.0 - self.left / self.len;
        let envelope = (PI * progress).sin().powi(2);
        let t = self.t;
        let wave = 0.25 + 0.6 * (t * 1.9 + phase).sin() + 0.35 * (t * 0.8 + phase * 1.7).sin();
        envelope * BREEZE_ACCEL * wave
    }
}

// ---------------------------------------------------------------- cards

/// Slides ease in through two critically damped stages, so a card starts and
/// stops gently instead of being jerked (rad/s).
const SLIDE_LEAD_W: f32 = 6.0;
const SLIDE_W: f32 = 12.0;
/// A new card rises faster than it travels sideways, so it flies up in an arc.
const RISE_LEAD_W: f32 = 8.5;
/// A new card starts slightly larger and shrinks onto the line.
const SCALE_W: f32 = 9.0;
/// The newest-card outline fades over about half a second.
const GLOW_W: f32 = 8.0;
/// Gravity for the pendulum (DIP/s²): with a card's hanging length this sets its
/// swing period, so taller cards swing more slowly.
const SWING_G: f32 = 3000.0;
/// Air drag on the swing.
const SWING_C: f32 = 1.7;
/// Share of the clip's sideways acceleration that reaches the card.
const SWING_DRIVE: f32 = 0.17;
const SWING_MAX: f32 = 0.6;

#[derive(Clone, Copy, Default)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
}

impl Spring {
    /// One semi-implicit Euler step towards `target`. Returns the acceleration applied.
    pub fn step_to(&mut self, target: f32, k: f32, c: f32, dt: f32) -> f32 {
        let a = -k * (self.x - target) - c * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
        a
    }

    pub fn active(&self, target: f32) -> bool {
        (self.x - target).abs() > REST_EPS || self.v.abs() > REST_EPS
    }

    pub fn snap(&mut self, target: f32) {
        self.x = target;
        self.v = 0.0;
    }
}

/// Everything that moves about one card.
#[derive(Clone, Copy, Default)]
pub struct Motion {
    /// Leading stage of the slide; `hx` follows it.
    lead: Spring,
    /// Horizontal offset of the clip from the card's slot, in DIPs.
    pub hx: Spring,
    pub hx_target: f32,
    /// Vertical offset below the rope while a new card flies in (DIPs).
    lead_y: Spring,
    pub hy: Spring,
    /// Size above 1.0 while a new card flies in.
    pub grow: Spring,
    /// Pendulum angle about the clip (radians, clockwise positive) and its speed.
    pub swing: Spring,
    /// How strongly the "newest card" outline shows, 0 to 1. It fades in and out.
    pub glow: Spring,
    pub glow_target: f32,
}

impl Motion {
    /// The card's slot moved by `-dx`: offset it so it stays where it is on screen,
    /// then let it slide to the new slot.
    pub fn shift(&mut self, dx: f32) {
        self.lead.x += dx;
        self.hx.x += dx;
    }

    /// Start a new card at an offset (`dx`, `dy`) from its place on the line,
    /// slightly enlarged, so it flies up and clips on.
    pub fn launch(&mut self, dx: f32, dy: f32, grow: f32) {
        self.lead.snap(dx);
        self.hx.snap(dx);
        self.lead_y.snap(dy);
        self.hy.snap(dy);
        self.grow.snap(grow);
    }

    /// Make `hx` an absolute x position (for a card leaving the line).
    pub fn rebase(&mut self, abs_x: f32) {
        self.lead.x = abs_x;
        self.hx.x = abs_x;
    }

    /// A push on the card (e.g. from the pointer), in rad/s.
    pub fn nudge(&mut self, angular_velocity: f32) {
        self.swing.v += angular_velocity;
    }

    /// Advance one substep. `hang` is the distance from the clip to the card's
    /// centre; `wind` is the sideways push of the breeze (DIP/s²).
    pub fn step(&mut self, dt: f32, hang: f32, wind: f32) {
        let w1 = SLIDE_LEAD_W;
        self.lead.step_to(self.hx_target, w1 * w1, 2.0 * w1, dt);
        let w2 = SLIDE_W;
        let ax = self.hx.step_to(self.lead.x, w2 * w2, 2.0 * w2, dt);
        let wy = RISE_LEAD_W;
        self.lead_y.step_to(0.0, wy * wy, 2.0 * wy, dt);
        self.hy.step_to(self.lead_y.x, w2 * w2, 2.0 * w2, dt);
        let ws = SCALE_W;
        self.grow.step_to(0.0, ws * ws, 2.0 * ws, dt);
        let wg = GLOW_W;
        self.glow.step_to(self.glow_target, wg * wg, 2.0 * wg, dt);

        // Pendulum about the clip. Clockwise (positive) moves the bottom left, so the
        // card lags behind a clip accelerating right, and is pushed left by wind
        // from the left being negative.
        let len = hang.max(20.0);
        let s = &mut self.swing;
        let (sin, cos) = s.x.sin_cos();
        let a = -(SWING_G / len) * sin - SWING_C * s.v + (SWING_DRIVE * ax - wind) / len * cos;
        s.v += a * dt;
        s.x += s.v * dt;
        if s.x.abs() > SWING_MAX {
            s.x = s.x.clamp(-SWING_MAX, SWING_MAX);
            s.v = 0.0;
        }
    }

    pub fn active(&self) -> bool {
        self.lead.active(self.hx_target)
            || self.hx.active(self.hx_target)
            || self.lead_y.active(0.0)
            || self.hy.active(0.0)
            || self.grow.active(0.0)
            || self.glow.active(self.glow_target)
            || self.swing.active(0.0)
    }

    pub fn settle(&mut self) {
        self.lead.snap(self.hx_target);
        self.hx.snap(self.hx_target);
        self.lead_y.snap(0.0);
        self.hy.snap(0.0);
        self.grow.snap(0.0);
        self.glow.snap(self.glow_target);
        self.swing = Spring::default();
    }
}
