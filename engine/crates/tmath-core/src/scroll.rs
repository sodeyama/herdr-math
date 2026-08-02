//! Smooth document scrolling.
//!
//! A [`ScrollState`] tracks a target offset and an eased current position. A
//! [`ScrollProfile`] decides how position chases the target each frame; the
//! [`Smooth`] profile uses exponential easing and brakes once the input stream
//! goes quiet, mirroring how `terminal-browser` animates scrollback.

/// Decides how a scroll state advances toward its target on each frame.
pub trait ScrollProfile: std::fmt::Debug + Sync {
    /// Optional immediate reaction to a new delta.
    fn tick(&self, _state: &mut ScrollState, _delta: f32, _max: f32) {}
    /// Advances the position toward the target for one frame.
    fn step(&self, state: &mut ScrollState, dt: f32, max: f32);
}

/// Eased scroll position and velocity.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollState {
    pub position: f32,
    pub target: f32,
    pub velocity: f32,
    idle: f32,
}

impl ScrollState {
    /// Applies a new input delta, clamping the target into `[0, max]`, and lets
    /// the profile react.
    pub fn tick<P: ScrollProfile + ?Sized>(&mut self, profile: &P, delta: f32, max: f32) {
        self.idle = 0.0;
        self.target = (self.target + delta).clamp(0.0, max);
        profile.tick(self, delta, max);
    }

    /// Sets the target directly, cancelling any leftover velocity.
    pub fn set_target(&mut self, pos: f32) {
        self.target = pos.max(0.0);
        self.velocity = 0.0;
    }

    /// Whether the state has reached its target and has no velocity.
    pub fn settled(&self) -> bool {
        self.position == self.target && self.velocity == 0.0
    }

    /// Advances one frame; returns whether the position changed.
    pub fn step<P: ScrollProfile + ?Sized>(&mut self, profile: &P, dt: f32, max: f32) -> bool {
        let before = self.position;
        self.idle += dt;
        profile.step(self, dt, max);
        self.position != before
    }

    /// How long the input stream has been quiet, in seconds.
    pub fn idle(&self) -> f32 {
        self.idle
    }

    /// Eases position toward the target with a smoothing time constant `tau`.
    ///
    /// A follow target may keep the position ahead of a stale `max`; the caller
    /// is expected to refresh `max` as content grows.
    pub fn chase(&mut self, tau: f32, dt: f32) {
        let gap = self.target - self.position;
        self.position = if gap.abs() < 0.5 {
            self.target
        } else {
            self.position + gap * (1.0 - (-dt / tau).exp())
        };
    }
}

/// A smooth scroll profile with exponential easing and optional braking.
#[derive(Debug, Clone, Copy)]
pub struct Smooth {
    pub tau: f32,
    pub brake: f32,
}

impl Default for Smooth {
    fn default() -> Self {
        Self {
            tau: 0.08,
            brake: 0.025,
        }
    }
}

const CATCH_IDLE: f32 = 0.06;

impl ScrollProfile for Smooth {
    fn step(&self, state: &mut ScrollState, dt: f32, _max: f32) {
        state.velocity = 0.0;
        let tau = if state.idle() > CATCH_IDLE {
            self.brake
        } else {
            self.tau
        };
        state.chase(tau, dt);
    }
}

#[cfg(test)]
fn settle(state: &mut ScrollState, profile: &dyn ScrollProfile, max: f32) -> usize {
    let mut steps = 0;
    while !state.settled() {
        state.step(profile, 1.0 / 60.0, max);
        steps += 1;
        assert!(steps < 1000, "never settled");
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_clamp_but_follow_targets_may_lead_content_growth() {
        let smooth = Smooth {
            tau: 0.08,
            brake: 0.025,
        };
        let mut state = ScrollState::default();
        state.tick(&smooth, 300.0, 200.0);
        settle(&mut state, &smooth, 200.0);
        assert_eq!(state.position, 200.0);

        state.set_target(230.0);
        settle(&mut state, &smooth, 200.0);
        assert_eq!(state.position, 230.0, "follow may outrun a stale max");
    }

    #[test]
    fn eases_over_multiple_frames_then_settles_exactly() {
        let smooth = Smooth {
            tau: 0.08,
            brake: 0.025,
        };
        let mut state = ScrollState::default();
        state.tick(&smooth, 100.0, 500.0);
        assert!(!state.settled());
        let mut last = 0.0;
        let mut steps = 0;
        while !state.settled() {
            state.step(&smooth, 1.0 / 60.0, 500.0);
            assert!(state.position > last && state.position <= 100.0);
            last = state.position;
            steps += 1;
            assert!(steps < 1000);
        }
        assert_eq!(state.position, 100.0);
        assert!(steps > 3, "eased over {steps} frames");
    }

    #[test]
    fn brakes_once_the_stream_goes_quiet() {
        let plain = Smooth {
            tau: 0.08,
            brake: 0.08,
        };
        let braked = Smooth {
            tau: 0.08,
            brake: 0.02,
        };
        let mut a = ScrollState::default();
        let mut b = ScrollState::default();
        a.tick(&plain, 300.0, 1000.0);
        b.tick(&braked, 300.0, 1000.0);
        let plain_steps = settle(&mut a, &plain, 1000.0);
        let braked_steps = settle(&mut b, &braked, 1000.0);
        assert!(
            braked_steps < plain_steps,
            "braked settled in {braked_steps} steps vs {plain_steps}"
        );
        assert_eq!(b.position, 300.0, "braking changes speed, not distance");
    }

    #[test]
    fn set_target_cancels_velocity() {
        let smooth = Smooth::default();
        let mut state = ScrollState::default();
        state.tick(&smooth, 50.0, 500.0);
        state.velocity = 9.0;
        state.set_target(20.0);
        assert_eq!(state.velocity, 0.0);
        assert_eq!(state.target, 20.0);
    }

    #[test]
    fn step_reports_changes_and_increments_idle() {
        let smooth = Smooth::default();
        let mut state = ScrollState::default();
        state.tick(&smooth, 100.0, 500.0);
        assert!(state.step(&smooth, 1.0 / 60.0, 500.0));
        assert!(state.idle() > 0.0);
        let position = state.position;
        state.step(&smooth, 1.0 / 60.0, 500.0);
        assert_ne!(state.position, position);
    }
}
