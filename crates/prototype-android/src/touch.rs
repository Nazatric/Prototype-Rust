//! touch — maps Android touch events onto prototype-input's mouse-shaped
//! [`InputEvent`] stream.
//!
//! # Design (DESIGN — ours, not the engine's)
//!
//! Prototype (2009) shipped PC-only: there is no original touch behaviour
//! to match, so this follows the loader-port convention (cinnabar/horizon
//! and every similar rewrite): **single-pointer-as-mouse**.  The first
//! finger to touch the screen owns the pointer until it lifts; other
//! concurrent fingers are ignored while one is tracked.  Positions pass
//! through in WINDOW PIXELS — on Android that is physical pixels, and
//! [`prototype_input::MousePosition`] is documented as window client
//! pixels, so the two spaces agree on a NativeActivity (no logical
//! scaling layer in between).
//!
//! Block 38 adds the LOOK finger (the touch look control,
//! `prototype_engine::look::LookControl`): the SECOND concurrent finger
//! (which the single-pointer scheme used to ignore) becomes the camera
//! look — its Move samples produce pixel deltas ([`TouchMapped::Look`])
//! the app feeds to the look orbit.  Roles are bound per finger id and
//! held until each finger lifts (the look finger is never promoted to
//! the mouse mid-gesture and vice versa).  With a single finger the
//! behaviour is exactly the pre-38 scheme; [`TouchMapper::map`] keeps
//! returning only the mouse events (the look deltas are visible only
//! through [`TouchMapper::map_all`]).
//!
//! Everything here is device-free and unit-tested on every host; the
//! Android glue only converts `winit`'s touch phases into
//! [`TouchPhase`] and calls [`TouchMapper::map_all`].

use prototype_input::{InputEvent, MouseButton, MousePosition};

/// The phase of one touch point (mirrors winit's `TouchPhase` 1:1 —
/// the Android glue converts mechanically).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    /// Finger touched the screen.
    Down,
    /// Finger moved.
    Move,
    /// Finger lifted.
    Up,
    /// System cancelled the touch (e.g. notification shade dragged in).
    Cancel,
}

/// One touch point sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    /// The OS pointer id (stable for the lifetime of one finger's touch).
    pub id: u32,
    /// Window x, in pixels.
    pub x: f32,
    /// Window y, in pixels.
    pub y: f32,
}

impl TouchPoint {
    /// Build one sample from its OS pointer id and window-pixel position.
    pub const fn new(id: u32, x: f32, y: f32) -> Self {
        Self { id, x, y }
    }

    fn position(self) -> MousePosition {
        MousePosition::new(self.x, self.y)
    }
}

/// What one touch sample produced (the full two-finger policy).
#[derive(Debug, Clone, PartialEq)]
pub enum TouchMapped {
    /// Primary-finger mouse emulation: the [`InputEvent`]s to feed
    /// [`prototype_input::InputState::apply`] (possibly none).
    Mouse(Vec<InputEvent>),
    /// Look-finger drag: pixel deltas since the previous look sample.
    /// Feed to `prototype_engine::LookControl::drag`.
    Look {
        /// Pixels right since the previous look sample (right-positive).
        dx: f32,
        /// Pixels down since the previous look sample (down-positive —
        /// winit touch locations).
        dy: f32,
    },
    /// Ignored sample (an untracked finger, or a role boundary).
    Ignored,
}

/// Stateful two-finger mapper: finger 1 = mouse, concurrent finger 2 =
/// camera look (block 38).
///
/// `map_all` returns the full [`TouchMapped`] policy for one sample;
/// [`TouchMapper::map`] is the single-pointer view (mouse events only,
/// exactly the pre-38 contract).
#[derive(Debug, Default, Clone)]
pub struct TouchMapper {
    /// The pointer id currently owning the mouse, if any.
    active: Option<u32>,
    /// The pointer id currently owning the camera look, if any
    /// (block 38).
    look: Option<u32>,
    /// The look finger's last sample position (pixel delta source).
    look_last: Option<(f32, f32)>,
}

impl TouchMapper {
    /// Start idle: no finger owns the mouse (or the look) yet.
    pub const fn new() -> Self {
        Self {
            active: None,
            look: None,
            look_last: None,
        }
    }

    /// Whether a finger currently owns the mouse.
    pub const fn is_tracking(&self) -> bool {
        self.active.is_some()
    }

    /// Whether a finger currently owns the camera look (block 38).
    pub const fn is_looking(&self) -> bool {
        self.look.is_some()
    }

    /// Map one touch sample to the full two-finger policy.
    pub fn map_all(&mut self, phase: TouchPhase, point: TouchPoint) -> TouchMapped {
        match phase {
            TouchPhase::Down => {
                if self.active.is_none() {
                    // First finger: press AND position the mouse in one
                    // sample — the game sees a click at the touched
                    // point, never a press at a stale position.
                    self.active = Some(point.id);
                    TouchMapped::Mouse(vec![
                        InputEvent::MouseMove(point.position()),
                        InputEvent::MouseDown(MouseButton::Left),
                    ])
                } else if self.look.is_none() {
                    // Second concurrent finger: it owns the camera look
                    // from now until it lifts (no events on Down — the
                    // first delta comes with the first Move).
                    self.look = Some(point.id);
                    self.look_last = Some((point.x, point.y));
                    TouchMapped::Ignored
                } else {
                    TouchMapped::Ignored
                }
            }
            TouchPhase::Move => {
                if self.active == Some(point.id) {
                    TouchMapped::Mouse(vec![InputEvent::MouseMove(point.position())])
                } else if self.look == Some(point.id) {
                    let (lx, ly) = self.look_last.replace((point.x, point.y)).unwrap_or((point.x, point.y));
                    TouchMapped::Look {
                        dx: point.x - lx,
                        dy: point.y - ly,
                    }
                } else {
                    TouchMapped::Ignored
                }
            }
            TouchPhase::Up | TouchPhase::Cancel => {
                // Cancel releases the button too: a mouse button cannot
                // stay pressed for a finger that no longer exists.  The
                // look finger just releases silently.
                if self.active == Some(point.id) {
                    self.active = None;
                    TouchMapped::Mouse(vec![InputEvent::MouseUp(MouseButton::Left)])
                } else if self.look == Some(point.id) {
                    self.look = None;
                    self.look_last = None;
                    TouchMapped::Ignored
                } else {
                    TouchMapped::Ignored
                }
            }
        }
    }

    /// Map one touch sample to the mouse-shaped events only (the
    /// pre-38 single-pointer contract; look deltas are swallowed).
    /// Delegates to [`TouchMapper::map_all`] — state advances once.
    pub fn map(&mut self, phase: TouchPhase, point: TouchPoint) -> Vec<InputEvent> {
        match self.map_all(phase, point) {
            TouchMapped::Mouse(evs) => evs,
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prototype_input::InputState;

    fn pt(id: u32, x: f32, y: f32) -> TouchPoint {
        TouchPoint::new(id, x, y)
    }

    #[test]
    fn new_mapper_starts_idle() {
        let m = TouchMapper::new();
        assert!(!m.is_tracking());
    }

    #[test]
    fn down_presses_and_positions_mouse() {
        let mut m = TouchMapper::new();
        let evs = m.map(TouchPhase::Down, pt(1, 40.0, 60.0));
        assert_eq!(
            evs,
            vec![
                InputEvent::MouseMove(MousePosition::new(40.0, 60.0)),
                InputEvent::MouseDown(MouseButton::Left),
            ]
        );
        assert!(m.is_tracking());
    }

    #[test]
    fn move_while_tracking_only_moves() {
        let mut m = TouchMapper::new();
        let _ = m.map(TouchPhase::Down, pt(1, 0.0, 0.0));
        let evs = m.map(TouchPhase::Move, pt(1, 12.0, 34.0));
        assert_eq!(evs, vec![InputEvent::MouseMove(MousePosition::new(12.0, 34.0))]);
        assert!(m.is_tracking(), "a move never releases the button");
    }

    #[test]
    fn up_releases_and_idles() {
        let mut m = TouchMapper::new();
        let _ = m.map(TouchPhase::Down, pt(7, 0.0, 0.0));
        let evs = m.map(TouchPhase::Up, pt(7, 5.0, 5.0));
        assert_eq!(evs, vec![InputEvent::MouseUp(MouseButton::Left)]);
        assert!(!m.is_tracking());
    }

    #[test]
    fn cancel_releases_too() {
        let mut m = TouchMapper::new();
        let _ = m.map(TouchPhase::Down, pt(7, 0.0, 0.0));
        let evs = m.map(TouchPhase::Cancel, pt(7, 5.0, 5.0));
        assert_eq!(evs, vec![InputEvent::MouseUp(MouseButton::Left)]);
        assert!(!m.is_tracking());
    }

    #[test]
    fn second_finger_ignored_until_first_lifts() {
        let mut m = TouchMapper::new();
        let _ = m.map(TouchPhase::Down, pt(1, 0.0, 0.0));
        assert!(m.map(TouchPhase::Down, pt(2, 9.0, 9.0)).is_empty());
        assert!(m.map(TouchPhase::Move, pt(2, 8.0, 8.0)).is_empty());
        assert!(m.map(TouchPhase::Up, pt(2, 7.0, 7.0)).is_empty());
        // The tracked finger still works throughout.
        assert!(!m.map(TouchPhase::Move, pt(1, 1.0, 1.0)).is_empty());
        // Lift it; NOW the second finger can take over.
        let _ = m.map(TouchPhase::Up, pt(1, 0.0, 0.0));
        let evs = m.map(TouchPhase::Down, pt(2, 9.0, 9.0));
        assert_eq!(evs.len(), 2, "second finger becomes the mouse after lift");
        assert!(m.is_tracking());
    }

    #[test]
    fn stray_move_and_up_without_down_are_ignored() {
        let mut m = TouchMapper::new();
        assert!(m.map(TouchPhase::Move, pt(3, 1.0, 1.0)).is_empty());
        assert!(m.map(TouchPhase::Up, pt(3, 1.0, 1.0)).is_empty());
        assert!(m.map(TouchPhase::Cancel, pt(3, 1.0, 1.0)).is_empty());
        assert!(!m.is_tracking());
    }

    #[test]
    fn full_cycle_then_repress() {
        let mut m = TouchMapper::new();
        let _ = m.map(TouchPhase::Down, pt(1, 0.0, 0.0));
        let _ = m.map(TouchPhase::Up, pt(1, 3.0, 4.0));
        let evs = m.map(TouchPhase::Down, pt(1, 10.0, 10.0));
        assert_eq!(
            evs,
            vec![
                InputEvent::MouseMove(MousePosition::new(10.0, 10.0)),
                InputEvent::MouseDown(MouseButton::Left),
            ]
        );
    }

    #[test]
    fn mapped_events_drive_input_state() {
        // Integration: the mapped stream must leave InputState exactly as
        // a real mouse would leave it.
        let mut m = TouchMapper::new();
        let mut input = InputState::new();
        for ev in m.map(TouchPhase::Down, pt(1, 40.0, 60.0)) {
            input.apply(ev);
        }
        assert!(input.is_button_down(MouseButton::Left));
        assert_eq!(input.mouse_position(), MousePosition::new(40.0, 60.0));
        for ev in m.map(TouchPhase::Move, pt(1, 50.0, 70.0)) {
            input.apply(ev);
        }
        assert_eq!(input.mouse_position(), MousePosition::new(50.0, 70.0));
        assert!(input.is_button_down(MouseButton::Left));
        for ev in m.map(TouchPhase::Up, pt(1, 50.0, 70.0)) {
            input.apply(ev);
        }
        assert!(!input.is_button_down(MouseButton::Left));
        assert_eq!(input.mouse_position(), MousePosition::new(50.0, 70.0));
    }

    // ----- Block 38: the second concurrent finger is the camera LOOK ----

    #[test]
    fn second_finger_becomes_the_look() {
        let mut m = TouchMapper::new();
        let _ = m.map_all(TouchPhase::Down, pt(1, 0.0, 0.0));
        // The second finger's Down binds the look silently...
        assert!(matches!(
            m.map_all(TouchPhase::Down, pt(2, 100.0, 100.0)),
            TouchMapped::Ignored
        ));
        assert!(m.is_looking());
        assert!(m.is_tracking(), "the mouse finger is unaffected");
        // ...and its Moves produce pixel deltas relative to the Down point.
        assert_eq!(
            m.map_all(TouchPhase::Move, pt(2, 115.0, 88.0)),
            TouchMapped::Look { dx: 15.0, dy: -12.0 }
        );
        assert_eq!(
            m.map_all(TouchPhase::Move, pt(2, 120.0, 88.0)),
            TouchMapped::Look { dx: 5.0, dy: 0.0 },
            "deltas chain sample to sample"
        );
        // Lift releases the look silently; the mouse finger still works.
        assert!(matches!(
            m.map_all(TouchPhase::Up, pt(2, 120.0, 88.0)),
            TouchMapped::Ignored
        ));
        assert!(!m.is_looking());
        assert!(matches!(
            m.map_all(TouchPhase::Move, pt(1, 3.0, 4.0)),
            TouchMapped::Mouse(_)
        ));
    }

    #[test]
    fn look_deltas_drive_the_look_control() {
        // The exact integration the App performs: TouchMapped::Look →
        // LookControl::drag; drag right/up-pins signs flow through.
        use prototype_engine::LookControl;
        let mut m = TouchMapper::new();
        let mut look = LookControl::new();
        let _ = m.map_all(TouchPhase::Down, pt(1, 0.0, 0.0));
        let _ = m.map_all(TouchPhase::Down, pt(2, 500.0, 400.0));
        for phase in [TouchPhase::Move, TouchPhase::Move] {
            if let TouchMapped::Look { dx, dy } = m.map_all(phase, pt(2, 560.0, 310.0)) {
                look.drag(dx, dy);
            }
        }
        assert!(look.yaw < 0.0, "drag right turns the view right: {}", look.yaw);
        assert!(look.pitch > 0.0, "drag up looks up: {}", look.pitch);
        // And the orbit reaches the camera follow.
        let mut follow = prototype_engine::player::CameraFollow::default();
        look.apply_to(&mut follow);
        assert_eq!(follow.yaw_offset, look.yaw);
        assert_eq!(follow.pitch_offset, look.pitch);
    }

    #[test]
    fn look_finger_never_becomes_the_mouse_and_versa() {
        // Roles bind per finger and hold until each lifts: the look
        // finger's samples never emit mouse events (map stays empty),
        // and the mouse finger never emits look deltas.
        let mut m = TouchMapper::new();
        let _ = m.map_all(TouchPhase::Down, pt(1, 0.0, 0.0));
        let _ = m.map_all(TouchPhase::Down, pt(2, 9.0, 9.0));
        assert!(m.map(TouchPhase::Move, pt(2, 50.0, 50.0)).is_empty(), "map hides look deltas");
        assert!(matches!(
            m.map_all(TouchPhase::Move, pt(1, 4.0, 4.0)),
            TouchMapped::Mouse(_)
        ), "the mouse finger never looks");
        // After the mouse finger lifts, the look finger keeps its role;
        // a NEW finger becomes the mouse.
        let _ = m.map_all(TouchPhase::Up, pt(1, 4.0, 4.0));
        assert!(m.is_looking(), "the look survives the mouse finger lifting");
        assert!(matches!(
            m.map_all(TouchPhase::Down, pt(3, 1.0, 1.0)),
            TouchMapped::Mouse(_)
        ));
        assert!(matches!(
            m.map_all(TouchPhase::Move, pt(2, 60.0, 40.0)),
            TouchMapped::Look { .. }
        ), "the look finger keeps looking");
    }

    #[test]
    fn cancel_releases_the_look_too() {
        let mut m = TouchMapper::new();
        let _ = m.map_all(TouchPhase::Down, pt(1, 0.0, 0.0));
        let _ = m.map_all(TouchPhase::Down, pt(2, 9.0, 9.0));
        assert!(m.is_looking());
        let _ = m.map_all(TouchPhase::Cancel, pt(2, 9.0, 9.0));
        assert!(!m.is_looking());
        assert!(m.is_tracking(), "cancel targets only the sampled finger");
        // The released finger id is reusable as a fresh look finger.
        let _ = m.map_all(TouchPhase::Down, pt(2, 20.0, 20.0));
        assert!(m.is_looking());
    }
}
