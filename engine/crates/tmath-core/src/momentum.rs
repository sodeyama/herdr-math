//! Velocity-based momentum scrolling (stage 2 of the scroll-region viewer).
//!
//! [`Momentum`] is a pure, tick-injectable state machine distinct from
//! [`crate::scroll::Smooth`]: `Smooth` EASES a position TOWARD a fixed
//! target and stops there; `Momentum` tracks a VELOCITY that keeps moving
//! the position after input stops, decelerating it each tick until it drops
//! below a stop threshold — the OS/browser "flick and it keeps going,
//! slowing down" feel, not a snap-to-target animation. The two solve
//! different problems and are not interchangeable: the agent-viewer's
//! discrete wheel/keyboard scrolling needs momentum; `tmath render`'s
//! single-image scroll-into-place (`crate::scroll_driver::ScrollDriver`)
//! needs `Smooth`'s target-chase.
//!
//! # Model and cited constants
//!
//! Real implementations differ on the exact numbers, but converge on the
//! same shape: exponential velocity decay, a stop threshold below which the
//! animation snaps rather than approaches asymptotically forever, and a
//! frame-rate-independent decay so the same parameters produce the same
//! real-world deceleration curve regardless of tick interval.
//!
//! - **Exponential decay**: Apple's only publicly documented momentum
//!   constant is `UIScrollView.DecelerationRate`, expressed as a
//!   per-millisecond velocity multiplier (`.normal` = 0.998, `.fast` =
//!   0.99) — i.e. velocity is scaled by that factor every 1ms of elapsed
//!   time, not every frame. This is the authoritative confirmation that
//!   real momentum implementations use exponential decay parameterized by
//!   REAL TIME, not by a fixed "per frame" constant tied to a specific
//!   refresh rate.
//! - **Frame-rate independence**: converting a "decay per frame at 60fps"
//!   folklore constant (commonly cited in open-source smooth-scroll
//!   libraries as roughly 0.9-0.95 per ~16.67ms frame) into a rate- and
//!   tick-independent form uses the standard technique for exponential
//!   decay (the same shape `crate::scroll::ScrollState::chase` already
//!   uses for its own `1.0 - exp(-dt / tau)` easing, and the technique
//!   Glenn Fiedler's "Fix Your Timestep" and Robert Penner's easing
//!   references both describe for decay-per-second constants): express the
//!   decay as `DECAY_PER_SECOND.powf(dt_seconds)`, so a 0.92-per-frame
//!   folklore constant at 60fps (`dt = 1/60`) implies `DECAY_PER_SECOND =
//!   0.92.powf(60) ≈ 0.0064`, and applying `DECAY_PER_SECOND.powf(dt)` at
//!   ANY tick interval (our 40ms tick, `dt = 0.04`) reproduces the same
//!   real-world deceleration curve a 60fps implementation would show. This
//!   is exactly what makes `tick(dt)` below safe to call with a non-uniform
//!   or non-60fps interval and still produce deterministic, cited-reference
//!   behavior.
//! - **Stop threshold**: commonly expressed in the same units as velocity
//!   (rows/second here); below the threshold, the animation snaps to zero
//!   velocity rather than asymptotically approaching it forever (an
//!   exponential decay never reaches exactly zero in finite time). `0.5`
//!   rows/second is chosen so momentum visibly finishes within a handful of
//!   ticks rather than leaving an imperceptible drift running indefinitely.
//! - **Gesture-end / immediate response**: while wheel events keep
//!   arriving, the caller feeds them as immediate 1:1 velocity nudges
//!   (`add_impulse`); momentum's own decay only starts being the sole
//!   driver of the position once input stops. This module does not itself
//!   infer "gesture ended" from a timeout — the caller (`agent_viewer`'s
//!   tick loop) already has a natural signal (no wheel event arrived this
//!   tick), so a separate timer is unnecessary duplication of state the
//!   caller already tracks turn-by-turn.
//!
//! `tick`'s `dt` is a pure function parameter — no wall-clock read anywhere
//! in this module — so behavior is exactly reproducible under test.

/// Rows/second below which momentum is considered stopped and its velocity
/// snaps to exactly zero. See the module doc's "stop threshold" citation.
pub const STOP_THRESHOLD_ROWS_PER_SEC: f32 = 0.5;

/// Per-second exponential decay rate, derived from a commonly-cited
/// ~0.92-per-frame constant at 60fps (see the module doc's "frame-rate
/// independence" section for the derivation: `0.92.powf(60) ≈ 0.0064`).
/// Applied as `DECAY_PER_SECOND.powf(dt)` each tick, so the same real-world
/// deceleration curve results regardless of tick interval.
pub const DECAY_PER_SECOND: f32 = 0.0064;

/// A velocity-based momentum state: a position (rows, matching
/// `viewer_viewport::Rows`'s unit) driven by a decaying velocity
/// (rows/second) rather than eased toward a fixed target.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Momentum {
    velocity_rows_per_sec: f32,
}

impl Momentum {
    /// A momentum state at rest (no velocity).
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an immediate velocity impulse (rows/second) — called once per
    /// tick with the COALESCED sum of every wheel event that arrived during
    /// that tick (see the module doc: wheel events are discrete SGR mouse
    /// notches, not continuous pixel deltas, so "coalescing" here means
    /// summing same-tick notches into one velocity nudge rather than
    /// re-triggering a fresh decay curve per notch). Adds rather than
    /// replaces, so a fast, sustained wheel spin (several notches across
    /// consecutive ticks before the previous impulse has fully decayed)
    /// accumulates speed the way a real trackpad flick does, instead of
    /// resetting to the same per-notch velocity every tick.
    pub fn add_impulse(&mut self, delta_rows_per_sec: f32) {
        self.velocity_rows_per_sec += delta_rows_per_sec;
    }

    /// Current velocity, rows/second. Positive scrolls forward (down),
    /// negative backward (up) — matching `scroll_delta`'s existing sign
    /// convention.
    pub fn velocity(&self) -> f32 {
        self.velocity_rows_per_sec
    }

    /// Whether momentum has decayed below the stop threshold (velocity is
    /// exactly zero).
    pub fn settled(&self) -> bool {
        self.velocity_rows_per_sec == 0.0
    }

    /// Advances one tick: decays velocity by `DECAY_PER_SECOND.powf(dt)`,
    /// snaps to exactly zero once below [`STOP_THRESHOLD_ROWS_PER_SEC`], and
    /// returns the row delta to apply to the scroll offset this tick.
    ///
    /// The displacement is the EXACT closed-form integral of the
    /// exponential-decay velocity curve over `[0, dt]`
    /// (`v0 * (k^dt - 1) / ln(k)`, where `k = DECAY_PER_SECOND` and `v0` is
    /// the pre-decay velocity), not a `velocity * dt` left-Riemann
    /// approximation. This distinction is what actually delivers frame-rate
    /// independence: a flat "velocity held constant for the whole tick"
    /// approximation accumulates integration error proportional to how much
    /// the velocity decays WITHIN one tick, so a coarse 40ms tick and a fine
    /// 4ms tick would silently diverge in total distance covered even
    /// though both use the same per-second decay rate — exactly the
    /// property the module doc's "frame-rate independence" citation exists
    /// to guarantee, so it must hold exactly, not approximately. `k.ln()` is
    /// always negative and nonzero for `0 < k < 1` (`DECAY_PER_SECOND` is a
    /// fixed module constant in that range), so the division is safe by
    /// construction — never user- or caller-controlled.
    ///
    /// `dt` must be `>= 0`; a negative `dt` is treated as `0` (no motion, no
    /// decay) rather than reversing time.
    pub fn tick(&mut self, dt: f32) -> f32 {
        let dt = dt.max(0.0);
        let v0 = self.velocity_rows_per_sec;
        let k: f32 = DECAY_PER_SECOND;
        let decay = k.powf(dt);
        // v0 * integral_0^dt k^t dt = v0 * (k^dt - 1) / ln(k).
        let delta = v0 * (decay - 1.0) / k.ln();
        self.velocity_rows_per_sec *= decay;
        if self.velocity_rows_per_sec.abs() < STOP_THRESHOLD_ROWS_PER_SEC {
            self.velocity_rows_per_sec = 0.0;
        }
        delta
    }

    /// Cancels momentum immediately (velocity snaps to zero without
    /// decaying) — used by `End`/`Home` and any jump-to-position input,
    /// which must not fight a still-decaying flick.
    pub fn cancel(&mut self) {
        self.velocity_rows_per_sec = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_momentum_state_is_settled_with_zero_velocity() {
        let momentum = Momentum::new();
        assert!(momentum.settled());
        assert_eq!(momentum.velocity(), 0.0);
    }

    #[test]
    fn add_impulse_accumulates_rather_than_replaces() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(10.0);
        momentum.add_impulse(5.0);
        assert_eq!(momentum.velocity(), 15.0, "same-tick impulses sum");
    }

    #[test]
    fn tick_returns_displacement_and_decays_velocity() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(100.0);
        let delta = momentum.tick(0.04);
        // Exact closed-form integral of the decay curve over [0, 0.04s],
        // not a naive `velocity * dt` — see `tick`'s doc comment for why.
        // Necessarily less than the naive 4.0 (velocity * 0.04), since the
        // velocity is decaying throughout the interval, not held constant.
        assert!(
            (delta - 3.6217806).abs() < 1e-4,
            "displacement should match the closed-form integral: got {delta}"
        );
        assert!(
            delta < 4.0,
            "the exact integral must be less than the naive velocity*dt \
             approximation, since velocity decays within the tick: {delta}"
        );
        assert!(
            momentum.velocity() < 100.0 && momentum.velocity() > 0.0,
            "velocity decayed but did not vanish in one 40ms tick: {}",
            momentum.velocity()
        );
    }

    #[test]
    fn repeated_ticks_eventually_settle_below_the_stop_threshold() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(50.0);
        let mut ticks = 0;
        while !momentum.settled() {
            momentum.tick(0.04);
            ticks += 1;
            assert!(ticks < 10_000, "momentum never settled");
        }
        assert_eq!(momentum.velocity(), 0.0);
        assert!(
            ticks > 1,
            "settled instantly, meaning decay is a no-op: {ticks} ticks"
        );
    }

    #[test]
    fn zero_impulse_stays_settled() {
        let mut momentum = Momentum::new();
        let delta = momentum.tick(0.04);
        assert_eq!(delta, 0.0);
        assert!(momentum.settled());
    }

    #[test]
    fn cancel_snaps_to_zero_without_decaying() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(1000.0);
        momentum.cancel();
        assert!(momentum.settled());
        assert_eq!(
            momentum.tick(0.04),
            0.0,
            "no residual displacement after cancel"
        );
    }

    #[test]
    fn negative_impulse_scrolls_backward_and_still_decays() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(-50.0);
        let delta = momentum.tick(0.04);
        assert!(
            delta < 0.0,
            "backward impulse produces negative displacement"
        );
        assert!(
            momentum.velocity() < 0.0,
            "velocity stays negative while decaying"
        );
    }

    #[test]
    fn negative_dt_produces_no_motion_and_no_decay() {
        let mut momentum = Momentum::new();
        momentum.add_impulse(50.0);
        let before = momentum.velocity();
        let delta = momentum.tick(-1.0);
        assert_eq!(delta, 0.0);
        assert_eq!(
            momentum.velocity(),
            before,
            "a negative dt must not decay velocity either"
        );
    }

    /// Frame-rate independence: the SAME total elapsed time, ticked at two
    /// wildly different intervals, must produce (to a tight tolerance —
    /// this uses the EXACT closed-form integral, not an approximation, so
    /// divergence here means a real bug, not acceptable float noise) the
    /// same cumulative displacement — the whole point of parameterizing
    /// decay by `dt` in seconds rather than a fixed per-tick constant. This
    /// is the property that makes the module doc's citation meaningful
    /// rather than decorative.
    #[test]
    fn cumulative_displacement_is_tick_rate_independent() {
        let total_seconds = 2.0;

        let mut fine = Momentum::new();
        fine.add_impulse(200.0);
        let fine_dt = 1.0 / 997.0; // an oddly-shaped fine tick, not a clean divisor
        let mut fine_total = 0.0;
        let mut elapsed = 0.0;
        while elapsed < total_seconds {
            fine_total += fine.tick(fine_dt);
            elapsed += fine_dt;
        }

        let mut coarse = Momentum::new();
        coarse.add_impulse(200.0);
        let coarse_dt = 0.04; // our real tick
        let mut coarse_total = 0.0;
        let mut elapsed = 0.0;
        while elapsed < total_seconds {
            coarse_total += coarse.tick(coarse_dt);
            elapsed += coarse_dt;
        }

        let relative_error = (fine_total - coarse_total).abs() / fine_total.abs().max(1.0);
        assert!(
            relative_error < 0.001,
            "fine-tick total {fine_total} vs coarse-tick total {coarse_total} \
             diverged by more than 0.1%: the closed-form integral is not \
             actually tick-rate independent — check for a left-Riemann-sum \
             regression in `tick`"
        );
    }

    #[test]
    fn settled_is_exactly_zero_velocity_not_merely_small() {
        let mut momentum = Momentum::new();
        // An impulse just above the stop threshold: after decay it should
        // cross below the threshold and snap to exactly 0.0, not linger at
        // some tiny nonzero float.
        momentum.add_impulse(STOP_THRESHOLD_ROWS_PER_SEC * 1.5);
        momentum.tick(1.0);
        assert_eq!(momentum.velocity(), 0.0);
        assert!(momentum.settled());
    }
}
