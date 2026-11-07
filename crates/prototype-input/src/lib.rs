//! prototype-input
//! =================
//! Clean-room reconstruction of the Prototype (2009) **input layer**:
//! typed keyboard, mouse, and gamepad state.
//!
//! ## What this crate provides
//!
//! * [`KeyCode`] — a typed keyboard key (33 physical keys + 4
//!   modifier bits, mirroring the subset the engine's
//!   `EngineKeyboardService` documents — see module docs).
//! * [`MouseButton`] — left, right, middle.
//! * [`InputEvent`] — discrete input events (key down/up, mouse
//!   move, button down/up, gamepad axis, gamepad button).
//! * [`InputState`] — the live input snapshot: pressed keys,
//!   mouse position, mouse button states, gamepad axes/buttons.
//!   Updated by applying [`InputEvent`]s via
//!   [`InputState::apply`].
//! * [`InputServiceId`] — typed wrappers for the engine's PROVEN
//!   service dispatch IDs (`EngineKeyboardService`,
//!   `EngineMouseService`).
//!
//! ## What this crate does NOT provide
//!
//! * Actual platform event capture (Win32 / X11 / Wayland /
//!   Android).  This crate is a typed STATE MODEL — a future
//!   `prototype-bin` example or platform crate would feed real
//!   platform events into [`InputState::apply`].
//! * The original engine's exact key-code mapping (the engine
//!   used Win32 virtual-key codes; we use a typed enum to keep
//!   the surface portable).  Conversion from `Win32 VK_*` to
//!   [`KeyCode`] is M11 follow-up work.
//!
//! ## Confidence labels
//!
//! * PROVEN: `EngineKeyboardService` and `EngineMouseService` are
//!   observed as literal strings in `prototypef.exe`'s `.rdata`
//!   section (Phase 2D); see [`InputServiceId`] for the verbatim
//!   strings.
//! * LIKELY: the 33-key subset of [`KeyCode`] covers the gameplay-
//!   relevant keys (WASD, arrow keys, modifiers, action keys like
//!   E/Q/F/Space/Shift); the original engine almost certainly
//!   accepted more keys (full A-Z, 0-9, F1-F12) — this enum is a
//!   gameplay-relevant subset, not an exhaustive Win32 mirror.
//! * UNKNOWN: the exact Win32 VK_* → gameplay-action mapping
//!   (which key opens the pause menu, which key devours a target).
//!   Those bindings live in `.p3d` config files we haven't
//!   recovered for this purpose.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Service IDs (PROVEN from prototypef.exe .rdata strings)
// ---------------------------------------------------------------------------

/// Engine service dispatch IDs related to input (PROVEN — verbatim
/// from `prototypef.exe`'s `.rdata` section, recovered in Phase 2D;
/// mirrors `prototype_boot::EngineServiceId::KeyboardService` /
/// `MouseService` to keep this crate a leaf dependency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputServiceId {
    /// `"EngineKeyboardService"` — keyboard input service.
    Keyboard,
    /// `"EngineMouseService"` — mouse input service.
    Mouse,
}

impl InputServiceId {
    /// The literal string the launcher uses for this service ID.
    /// PROVEN — exact match to the strings in `prototypef.exe`.
    pub fn as_str(&self) -> &'static str {
        match self {
            InputServiceId::Keyboard => "EngineKeyboardService",
            InputServiceId::Mouse => "EngineMouseService",
        }
    }
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

/// A typed keyboard key.  Gameplay-relevant subset (not an
/// exhaustive Win32 mirror — see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyCode {
    // Movement
    W, A, S, D,
    Up, Down, Left, Right,
    Space,
    Shift, Control,

    // Action keys (Prototype's common binds)
    E, Q, F, R, Tab, Escape,

    // Number row
    One, Two, Three, Four, Five, Six, Seven, Eight, Nine, Zero,

    // Modifier / system
    Enter, Backspace,
}

impl KeyCode {
    /// The Win32 virtual-key code for this key (LIKELY — the
    /// original engine's `EngineKeyboardService` consumed Win32
    /// VK_* values; this mapping is the standard Win32 layout).
    pub fn win32_vk(&self) -> u32 {
        match self {
            KeyCode::W => 0x57,
            KeyCode::A => 0x41,
            KeyCode::S => 0x53,
            KeyCode::D => 0x44,
            KeyCode::Up => 0x26,    // VK_UP
            KeyCode::Down => 0x28,  // VK_DOWN
            KeyCode::Left => 0x25,  // VK_LEFT
            KeyCode::Right => 0x27, // VK_RIGHT
            KeyCode::Space => 0x20, // VK_SPACE
            KeyCode::Shift => 0x10, // VK_SHIFT
            KeyCode::Control => 0x11, // VK_CONTROL
            KeyCode::E => 0x45,
            KeyCode::Q => 0x51,
            KeyCode::F => 0x46,
            KeyCode::R => 0x52,
            KeyCode::Tab => 0x09,    // VK_TAB
            KeyCode::Escape => 0x1B, // VK_ESCAPE
            KeyCode::One => 0x31,
            KeyCode::Two => 0x32,
            KeyCode::Three => 0x33,
            KeyCode::Four => 0x34,
            KeyCode::Five => 0x35,
            KeyCode::Six => 0x36,
            KeyCode::Seven => 0x37,
            KeyCode::Eight => 0x38,
            KeyCode::Nine => 0x39,
            KeyCode::Zero => 0x30,
            KeyCode::Enter => 0x0D,    // VK_RETURN
            KeyCode::Backspace => 0x08, // VK_BACK
        }
    }
}

// ---------------------------------------------------------------------------
// Mouse
// ---------------------------------------------------------------------------

/// Mouse buttons (LIKELY — the original engine's mouse service
/// almost certainly distinguished these three; the original Win32
/// MK_* / WM_LBUTTON constants are standard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// A 2D mouse position, in window client pixels.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MousePosition {
    pub x: f32,
    pub y: f32,
}

impl MousePosition {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

// ---------------------------------------------------------------------------
// Gamepad
// ---------------------------------------------------------------------------

/// A gamepad axis value, normalized to `[-1.0, 1.0]` for sticks,
/// `[0.0, 1.0]` for triggers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AxisValue(pub f32);

impl AxisValue {
    pub const fn raw(self) -> f32 {
        self.0
    }
}

/// A typed gamepad button (Xbox-style layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GamepadButton {
    A, B, X, Y,
    LeftBumper, RightBumper,
    Back, Start,
    LeftStick, RightStick, // stick-press
    DpadUp, DpadDown, DpadLeft, DpadRight,
}

/// A typed gamepad axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GamepadAxis {
    LeftX, LeftY,
    RightX, RightY,
    LeftTrigger, RightTrigger,
}

// ---------------------------------------------------------------------------
// InputEvent — the discrete events that flow into InputState
// ---------------------------------------------------------------------------

/// One discrete input event.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    MouseMove(MousePosition),
    MouseDown(MouseButton),
    MouseUp(MouseButton),
    GamepadAxis(GamepadAxis, AxisValue),
    GamepadButtonDown(GamepadButton),
    GamepadButtonUp(GamepadButton),
}

// ---------------------------------------------------------------------------
// InputState — the live snapshot
// ---------------------------------------------------------------------------

/// Live input state: pressed keys, mouse position/buttons, gamepad
/// axes/buttons.  Updated by applying [`InputEvent`]s via
/// [`InputState::apply`].
///
/// Designed as the engine's per-frame input snapshot — the gameplay
/// code reads it through `is_key_down` / `is_button_down` /
/// `mouse_position` on each frame.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    pressed_keys: BTreeSet<KeyCode>,
    mouse_pos: MousePosition,
    pressed_mouse: BTreeSet<MouseButton>,
    gamepad_axes: std::collections::BTreeMap<GamepadAxis, AxisValue>,
    pressed_gamepad: BTreeSet<GamepadButton>,
    /// Cumulative count of events applied (for tests + frame
    /// audits).
    pub events_applied: usize,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one [`InputEvent`], mutating the state.  Returns
    /// `true` if the event changed the state (e.g. a KeyDown for
    /// a key that wasn't already pressed), `false` otherwise.
    pub fn apply(&mut self, ev: InputEvent) -> bool {
        self.events_applied += 1;
        match ev {
            InputEvent::KeyDown(k) => self.pressed_keys.insert(k),
            InputEvent::KeyUp(k) => self.pressed_keys.remove(&k),
            InputEvent::MouseMove(p) => {
                if p == self.mouse_pos {
                    false
                } else {
                    self.mouse_pos = p;
                    true
                }
            }
            InputEvent::MouseDown(b) => self.pressed_mouse.insert(b),
            InputEvent::MouseUp(b) => self.pressed_mouse.remove(&b),
            InputEvent::GamepadAxis(a, v) => {
                let prev = self.gamepad_axes.insert(a, v);
                prev.map_or(true, |old| old != v)
            }
            InputEvent::GamepadButtonDown(b) => self.pressed_gamepad.insert(b),
            InputEvent::GamepadButtonUp(b) => self.pressed_gamepad.remove(&b),
        }
    }

    /// Apply a batch of events in order.  Returns the number of
    /// events that changed state.
    pub fn apply_batch(&mut self, events: &[InputEvent]) -> usize {
        let mut changed = 0;
        for ev in events {
            if self.apply(ev.clone()) {
                changed += 1;
            }
        }
        changed
    }

    // ----- keyboard ------------------------------------------------

    pub fn is_key_down(&self, k: KeyCode) -> bool {
        self.pressed_keys.contains(&k)
    }
    pub fn is_key_up(&self, k: KeyCode) -> bool {
        !self.is_key_down(k)
    }
    pub fn pressed_keys(&self) -> &BTreeSet<KeyCode> {
        &self.pressed_keys
    }
    /// Number of keys currently down.
    pub fn pressed_key_count(&self) -> usize {
        self.pressed_keys.len()
    }

    // ----- mouse ---------------------------------------------------

    pub fn mouse_position(&self) -> MousePosition {
        self.mouse_pos
    }
    pub fn is_button_down(&self, b: MouseButton) -> bool {
        self.pressed_mouse.contains(&b)
    }
    pub fn pressed_mouse_buttons(&self) -> &BTreeSet<MouseButton> {
        &self.pressed_mouse
    }

    // ----- gamepad -------------------------------------------------

    pub fn axis(&self, a: GamepadAxis) -> AxisValue {
        self.gamepad_axes.get(&a).copied().unwrap_or_default()
    }
    pub fn is_gamepad_button_down(&self, b: GamepadButton) -> bool {
        self.pressed_gamepad.contains(&b)
    }
    pub fn pressed_gamepad_buttons(&self) -> &BTreeSet<GamepadButton> {
        &self.pressed_gamepad
    }

    /// Reset to the empty state (call between gameplay sessions).
    pub fn clear(&mut self) {
        self.pressed_keys.clear();
        self.mouse_pos = MousePosition::default();
        self.pressed_mouse.clear();
        self.gamepad_axes.clear();
        self.pressed_gamepad.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_service_id_strings_match_prototypef_exe() {
        // PROVEN — these are the literal strings in the binary's
        // .rdata section (Phase 2D recovery).
        assert_eq!(InputServiceId::Keyboard.as_str(), "EngineKeyboardService");
        assert_eq!(InputServiceId::Mouse.as_str(), "EngineMouseService");
    }

    #[test]
    fn key_code_win32_vk_returns_standard_codes() {
        // Sanity-check a few standard Win32 VK_* codes.
        assert_eq!(KeyCode::W.win32_vk(), 0x57);
        assert_eq!(KeyCode::Up.win32_vk(), 0x26);
        assert_eq!(KeyCode::Space.win32_vk(), 0x20);
        assert_eq!(KeyCode::Escape.win32_vk(), 0x1B);
        assert_eq!(KeyCode::Enter.win32_vk(), 0x0D);
    }

    #[test]
    fn key_down_up_toggles_state() {
        let mut s = InputState::new();
        assert!(!s.is_key_down(KeyCode::W));
        // Down → state changes to pressed.
        assert!(s.apply(InputEvent::KeyDown(KeyCode::W)));
        assert!(s.is_key_down(KeyCode::W));
        // Repeated Down → state doesn't change (already pressed).
        assert!(!s.apply(InputEvent::KeyDown(KeyCode::W)));
        assert!(s.is_key_down(KeyCode::W));
        // Up → state changes to released.
        assert!(s.apply(InputEvent::KeyUp(KeyCode::W)));
        assert!(!s.is_key_down(KeyCode::W));
        // Repeated Up → state doesn't change.
        assert!(!s.apply(InputEvent::KeyUp(KeyCode::W)));
        assert_eq!(s.events_applied, 4);
    }

    #[test]
    fn multiple_keys_can_be_held_simultaneously() {
        let mut s = InputState::new();
        s.apply(InputEvent::KeyDown(KeyCode::W));
        s.apply(InputEvent::KeyDown(KeyCode::A));
        s.apply(InputEvent::KeyDown(KeyCode::Shift));
        assert!(s.is_key_down(KeyCode::W));
        assert!(s.is_key_down(KeyCode::A));
        assert!(s.is_key_down(KeyCode::Shift));
        assert_eq!(s.pressed_key_count(), 3);
        // Releasing one key doesn't release the others.
        s.apply(InputEvent::KeyUp(KeyCode::A));
        assert!(!s.is_key_down(KeyCode::A));
        assert!(s.is_key_down(KeyCode::W));
        assert!(s.is_key_down(KeyCode::Shift));
        assert_eq!(s.pressed_key_count(), 2);
    }

    #[test]
    fn mouse_move_updates_position() {
        let mut s = InputState::new();
        assert_eq!(s.mouse_position(), MousePosition::new(0.0, 0.0));
        assert!(s.apply(InputEvent::MouseMove(MousePosition::new(100.0, 200.0))));
        assert_eq!(s.mouse_position(), MousePosition::new(100.0, 200.0));
        // Re-applying the same position is a no-op.
        assert!(!s.apply(InputEvent::MouseMove(MousePosition::new(100.0, 200.0))));
    }

    #[test]
    fn mouse_buttons_independent() {
        let mut s = InputState::new();
        s.apply(InputEvent::MouseDown(MouseButton::Left));
        assert!(s.is_button_down(MouseButton::Left));
        assert!(!s.is_button_down(MouseButton::Right));
        s.apply(InputEvent::MouseDown(MouseButton::Right));
        assert!(s.is_button_down(MouseButton::Left));
        assert!(s.is_button_down(MouseButton::Right));
        s.apply(InputEvent::MouseUp(MouseButton::Left));
        assert!(!s.is_button_down(MouseButton::Left));
        assert!(s.is_button_down(MouseButton::Right));
    }

    #[test]
    fn gamepad_axis_normalizes_to_default_when_unapplied() {
        let s = InputState::new();
        assert_eq!(s.axis(GamepadAxis::LeftX).raw(), 0.0);
        assert_eq!(s.axis(GamepadAxis::LeftTrigger).raw(), 0.0);
    }

    #[test]
    fn gamepad_axis_updates_change_state() {
        let mut s = InputState::new();
        // First set: state changes (from 0.0 to 0.5).
        assert!(s.apply(InputEvent::GamepadAxis(GamepadAxis::LeftX, AxisValue(0.5))));
        assert_eq!(s.axis(GamepadAxis::LeftX).raw(), 0.5);
        // Same value: state doesn't change.
        assert!(!s.apply(InputEvent::GamepadAxis(GamepadAxis::LeftX, AxisValue(0.5))));
        // Different value: state changes.
        assert!(s.apply(InputEvent::GamepadAxis(GamepadAxis::LeftX, AxisValue(-0.3))));
        assert_eq!(s.axis(GamepadAxis::LeftX).raw(), -0.3);
    }

    #[test]
    fn gamepad_buttons_independent() {
        let mut s = InputState::new();
        s.apply(InputEvent::GamepadButtonDown(GamepadButton::A));
        assert!(s.is_gamepad_button_down(GamepadButton::A));
        assert!(!s.is_gamepad_button_down(GamepadButton::B));
        s.apply(InputEvent::GamepadButtonDown(GamepadButton::B));
        s.apply(InputEvent::GamepadButtonUp(GamepadButton::A));
        assert!(!s.is_gamepad_button_down(GamepadButton::A));
        assert!(s.is_gamepad_button_down(GamepadButton::B));
    }

    #[test]
    fn apply_batch_returns_count_of_state_changes() {
        let mut s = InputState::new();
        let events = vec![
            InputEvent::KeyDown(KeyCode::W),
            InputEvent::KeyDown(KeyCode::W), // no-op (already pressed)
            InputEvent::KeyUp(KeyCode::W),
            InputEvent::MouseMove(MousePosition::new(1.0, 2.0)),
            InputEvent::MouseDown(MouseButton::Left),
            InputEvent::MouseDown(MouseButton::Left), // no-op
        ];
        let changed = s.apply_batch(&events);
        assert_eq!(changed, 4); // 4 state changes out of 6 events
        assert_eq!(s.events_applied, 6);
    }

    #[test]
    fn clear_resets_all_state() {
        let mut s = InputState::new();
        s.apply(InputEvent::KeyDown(KeyCode::W));
        s.apply(InputEvent::MouseDown(MouseButton::Left));
        s.apply(InputEvent::MouseMove(MousePosition::new(50.0, 50.0)));
        s.apply(InputEvent::GamepadAxis(GamepadAxis::LeftX, AxisValue(0.5)));
        s.apply(InputEvent::GamepadButtonDown(GamepadButton::A));
        assert!(!s.pressed_keys.is_empty());
        assert!(!s.pressed_mouse.is_empty());
        assert_eq!(s.mouse_position(), MousePosition::new(50.0, 50.0));
        s.clear();
        assert!(s.pressed_keys.is_empty());
        assert!(s.pressed_mouse.is_empty());
        assert_eq!(s.mouse_position(), MousePosition::new(0.0, 0.0));
        assert_eq!(s.axis(GamepadAxis::LeftX).raw(), 0.0);
        assert!(s.pressed_gamepad.is_empty());
        // events_applied is NOT reset by clear — it's a cumulative
        // audit counter, not a state field.
        assert!(s.events_applied > 0);
    }

    #[test]
    fn typical_gameplay_input_walk() {
        // Simulate a typical gameplay frame: W pressed + mouse
        // moved to look around + left mouse held (fire).
        let mut s = InputState::new();
        let frame_events = vec![
            InputEvent::KeyDown(KeyCode::W),
            InputEvent::MouseMove(MousePosition::new(320.0, 240.0)),
            InputEvent::MouseDown(MouseButton::Left),
        ];
        s.apply_batch(&frame_events);
        // The gameplay code reads:
        assert!(s.is_key_down(KeyCode::W));    // forward
        assert_eq!(s.mouse_position(), MousePosition::new(320.0, 240.0));
        assert!(s.is_button_down(MouseButton::Left)); // fire
        // Next frame: W still down, mouse moves slightly, fire
        // released.
        let next_frame = vec![
            InputEvent::MouseMove(MousePosition::new(325.0, 242.0)),
            InputEvent::MouseUp(MouseButton::Left),
        ];
        s.apply_batch(&next_frame);
        assert!(s.is_key_down(KeyCode::W)); // still forward
        assert_eq!(s.mouse_position(), MousePosition::new(325.0, 242.0));
        assert!(!s.is_button_down(MouseButton::Left)); // fire released
    }
}
