//! Platform-independent input snapshots for deterministic simulation.

use std::collections::HashSet;

/// Keyboard keys currently relevant to the engine's action layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputKey {
    W,
    A,
    S,
    D,
    Space,
    Escape,
    Other(u32),
}

/// Mouse buttons exposed by the engine input layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u16),
}

/// A stable input state consumed by simulation and scripts.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSnapshot {
    held_keys: HashSet<InputKey>,
    pressed_keys: HashSet<InputKey>,
    released_keys: HashSet<InputKey>,
    held_mouse: HashSet<MouseButton>,
    pressed_mouse: HashSet<MouseButton>,
    released_mouse: HashSet<MouseButton>,
    cursor_position: (f64, f64),
    cursor_delta: (f64, f64),
    focused: bool,
}

impl Default for InputSnapshot {
    fn default() -> Self {
        Self {
            held_keys: HashSet::new(),
            pressed_keys: HashSet::new(),
            released_keys: HashSet::new(),
            held_mouse: HashSet::new(),
            pressed_mouse: HashSet::new(),
            released_mouse: HashSet::new(),
            cursor_position: (0.0, 0.0),
            cursor_delta: (0.0, 0.0),
            focused: true,
        }
    }
}

impl InputSnapshot {
    /// Records a keyboard state transition without losing a quick press/release.
    pub fn key_event(&mut self, key: InputKey, is_down: bool) {
        if is_down {
            if self.held_keys.insert(key) {
                self.pressed_keys.insert(key);
            }
        } else if self.held_keys.remove(&key) {
            self.released_keys.insert(key);
        }
    }

    /// Records a mouse-button state transition without losing a quick click.
    pub fn mouse_event(&mut self, button: MouseButton, is_down: bool) {
        if is_down {
            if self.held_mouse.insert(button) {
                self.pressed_mouse.insert(button);
            }
        } else if self.held_mouse.remove(&button) {
            self.released_mouse.insert(button);
        }
    }

    /// Updates the cursor position and accumulates movement until the next frame boundary.
    pub fn cursor_moved(&mut self, x: f64, y: f64) {
        self.cursor_delta.0 += x - self.cursor_position.0;
        self.cursor_delta.1 += y - self.cursor_position.1;
        self.cursor_position = (x, y);
    }

    /// Updates focus. Losing focus clears held buttons to prevent stuck input.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.held_keys.clear();
            self.held_mouse.clear();
        }
    }

    /// Clears edge transitions and cursor delta after the simulation/render frame.
    pub fn finish_frame(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.cursor_delta = (0.0, 0.0);
    }

    /// Clears held and transient state, used when pausing or losing focus.
    pub fn clear_all(&mut self) {
        self.held_keys.clear();
        self.held_mouse.clear();
        self.clear_transitions();
    }

    /// Clears transient transitions, used when entering a paused state.
    pub fn clear_transitions(&mut self) {
        self.pressed_keys.clear();
        self.released_keys.clear();
        self.pressed_mouse.clear();
        self.released_mouse.clear();
        self.cursor_delta = (0.0, 0.0);
    }

    #[must_use]
    pub fn is_down(&self, key: InputKey) -> bool {
        self.held_keys.contains(&key)
    }

    #[must_use]
    pub fn just_pressed(&self, key: InputKey) -> bool {
        self.pressed_keys.contains(&key)
    }

    #[must_use]
    pub fn just_released(&self, key: InputKey) -> bool {
        self.released_keys.contains(&key)
    }

    #[must_use]
    pub fn mouse_is_down(&self, button: MouseButton) -> bool {
        self.held_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.pressed_mouse.contains(&button)
    }

    #[must_use]
    pub fn mouse_just_released(&self, button: MouseButton) -> bool {
        self.released_mouse.contains(&button)
    }

    #[must_use]
    pub fn axis(&self, axis: &str) -> f32 {
        match axis {
            "horizontal" => {
                self.is_down(InputKey::D) as i32 as f32 - self.is_down(InputKey::A) as i32 as f32
            }
            "vertical" => {
                self.is_down(InputKey::W) as i32 as f32 - self.is_down(InputKey::S) as i32 as f32
            }
            _ => 0.0,
        }
    }

    #[must_use]
    pub const fn cursor_position(&self) -> (f64, f64) {
        self.cursor_position
    }

    #[must_use]
    pub const fn cursor_delta(&self) -> (f64, f64) {
        self.cursor_delta
    }

    #[must_use]
    pub const fn is_focused(&self) -> bool {
        self.focused
    }
}

#[cfg(test)]
mod tests {
    use super::{InputKey, InputSnapshot, MouseButton};

    #[test]
    fn preserves_press_and_release_until_frame_boundary() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::Space, true);
        input.key_event(InputKey::Space, false);
        assert!(input.just_pressed(InputKey::Space));
        assert!(input.just_released(InputKey::Space));
        assert!(!input.is_down(InputKey::Space));
        input.finish_frame();
        assert!(!input.just_pressed(InputKey::Space));
        assert!(!input.just_released(InputKey::Space));
    }

    #[test]
    fn computes_wasd_axes_from_held_state() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::D, true);
        input.key_event(InputKey::S, true);
        assert_eq!(input.axis("horizontal"), 1.0);
        assert_eq!(input.axis("vertical"), -1.0);
        input.key_event(InputKey::A, true);
        assert_eq!(input.axis("horizontal"), 0.0);
    }

    #[test]
    fn focus_loss_clears_held_state_and_pause_clears_edges() {
        let mut input = InputSnapshot::default();
        input.key_event(InputKey::W, true);
        input.mouse_event(MouseButton::Left, true);
        input.set_focused(false);
        assert!(!input.is_down(InputKey::W));
        assert!(!input.mouse_is_down(MouseButton::Left));
        input.key_event(InputKey::Space, true);
        input.clear_transitions();
        assert!(!input.just_pressed(InputKey::Space));
    }

    #[test]
    fn accumulates_cursor_delta() {
        let mut input = InputSnapshot::default();
        input.cursor_moved(10.0, 20.0);
        input.cursor_moved(13.0, 18.0);
        assert_eq!(input.cursor_position(), (13.0, 18.0));
        assert_eq!(input.cursor_delta(), (13.0, 18.0));
        input.finish_frame();
        assert_eq!(input.cursor_delta(), (0.0, 0.0));
    }
}
