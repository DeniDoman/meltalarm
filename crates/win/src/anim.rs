//! Motion (DESIGN.md "Motion"): short transitions on a change of state, nothing at rest. Entries
//! decelerate, exits accelerate, nothing overshoots. Windows' "Animation effects" setting turns
//! every transition into an instant change.

use std::time::{Duration, Instant};

use windows::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};

/// The alert surfaces drop from the top edge in this long, and retract in `RETRACT`.
pub const DROP: Duration = Duration::from_millis(200);
pub const RETRACT: Duration = Duration::from_millis(150);
/// The tray window rises and fades in, and fades out.
pub const RISE: Duration = Duration::from_millis(150);
pub const FADE: Duration = Duration::from_millis(100);
/// How far the tray window rises, in DIP.
pub const RISE_DIP: f32 = 8.0;
/// The frame interval while something moves (the timer's real resolution is about 15.6 ms).
pub const FRAME_MS: u32 = 15;

/// Windows' "Animation effects" (Settings → Accessibility → Visual effects). Read at the start of
/// each transition, so a change applies at once.
pub fn enabled() -> bool {
    // Dev only: a simulated build can force either path, whatever the machine's setting.
    if cfg!(feature = "simulate") {
        match std::env::var("MELTALARM_ANIMATIONS").as_deref() {
            Ok("on") => return true,
            Ok("off") => return false,
            _ => {}
        }
    }
    let mut on = windows::core::BOOL(1);
    // SAFETY: a 4-byte BOOL out parameter, as the action requires.
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
    };
    ok.is_err() || on.as_bool()
}

/// Dev only: `MELTALARM_SLOWMO=10` in a simulated build makes every transition 10 times longer,
/// to check it frame by frame. Always 1 in a release.
fn slow_motion() -> f32 {
    if !cfg!(feature = "simulate") {
        return 1.0;
    }
    std::env::var("MELTALARM_SLOWMO").ok().and_then(|v| v.parse::<f32>().ok()).filter(|k| (1.0..=100.0).contains(k)).unwrap_or(1.0)
}

/// One transition in time.
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    start: Instant,
    length: Duration,
}

impl Transition {
    pub fn new(length: Duration) -> Self {
        Transition { start: Instant::now(), length: length.mul_f32(slow_motion()) }
    }

    /// Elapsed share, 0‥1.
    fn t(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() / self.length.as_secs_f32().max(0.001)).clamp(0.0, 1.0)
    }

    pub fn done(&self, now: Instant) -> bool {
        self.t(now) >= 1.0
    }

    /// Decelerating 0‥1, for entries (cubic ease-out).
    pub fn ease_out(&self, now: Instant) -> f32 {
        1.0 - (1.0 - self.t(now)).powi(3)
    }

    /// Accelerating 0‥1, for exits (quadratic ease-in).
    pub fn ease_in(&self, now: Instant) -> f32 {
        self.t(now).powi(2)
    }
}

/// A surface's motion state.
#[derive(Clone, Copy, Debug, Default)]
pub enum Motion {
    /// At rest (shown or gone): nothing to draw per frame.
    #[default]
    Still,
    In(Transition),
    Out(Transition),
}

impl Motion {
    pub fn moving(&self) -> bool {
        !matches!(self, Motion::Still)
    }

    pub fn leaving(&self) -> bool {
        matches!(self, Motion::Out(_))
    }

    /// How much of the surface shows, 0‥1: 1 at rest.
    pub fn shown(&self, now: Instant) -> f32 {
        match self {
            Motion::Still => 1.0,
            Motion::In(t) => t.ease_out(now),
            Motion::Out(t) => 1.0 - t.ease_in(now),
        }
    }

    /// The transition ended at `now`: entries come to rest. Returns `true` for an exit that ended
    /// (the surface should now go).
    pub fn settle(&mut self, now: Instant) -> bool {
        match *self {
            Motion::In(t) if t.done(now) => {
                *self = Motion::Still;
                false
            }
            Motion::Out(t) if t.done(now) => {
                *self = Motion::Still;
                true
            }
            _ => false,
        }
    }
}
