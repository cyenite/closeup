use std::collections::HashSet;
use std::time::{Duration, Instant};

use evdev::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Level(usize),
    ToggleLock,
    PickSource,
    TogglePause,
    Paused(bool),
    Access(KeyAccess),
}

/// How far global keys reach, from least to most capable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyAccess {
    #[default]
    StageOnly,
    ChordsOnly,
    Full,
}

impl KeyAccess {
    pub fn label(self) -> &'static str {
        match self {
            Self::StageOnly => "stage",
            Self::ChordsOnly => "chords",
            Self::Full => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Forward,
    Consume,
}

/// Typing a letter keeps bare digits typing for this long, which covers
/// apps that never tell the desktop a text field has focus.
const TYPING_WINDOW: Duration = Duration::from_millis(1200);

const MODIFIERS: [KeyCode; 8] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
];

#[derive(Clone, Copy, Debug)]
pub struct Context {
    pub now: Instant,
    pub is_grabbed: bool,
    pub is_text_focused: bool,
    pub has_shell_shortcuts: bool,
}

/// Decides, per key event on one keyboard, whether it triggers an action
/// and whether it may reach the focused app. Bare digits zoom unless the
/// presenter is typing; only a grabbed keyboard can swallow them.
#[derive(Default)]
pub struct KeyRouter {
    held: HashSet<KeyCode>,
    consumed: HashSet<KeyCode>,
    last_text_key: Option<Instant>,
}

impl KeyRouter {
    pub fn is_idle(&self) -> bool {
        self.held.is_empty()
    }

    /// True for a bare digit press that may zoom, the only case where the
    /// caller needs to ask the desktop whether a text field has focus.
    pub fn needs_focus_check(&self, key: KeyCode, value: i32, is_grabbed: bool) -> bool {
        value == 1 && is_grabbed && self.is_bare() && digit_action(key).is_some()
    }

    pub fn handle(&mut self, key: KeyCode, value: i32, context: Context) -> (Verdict, Option<Action>) {
        match value {
            0 => {
                self.held.remove(&key);
                let verdict = if self.consumed.remove(&key) { Verdict::Consume } else { Verdict::Forward };
                (verdict, None)
            }
            1 => {
                let (verdict, action) = self.press(key, context);
                self.held.insert(key);
                if verdict == Verdict::Consume {
                    self.consumed.insert(key);
                }
                (verdict, action)
            }
            _ if self.consumed.contains(&key) => (Verdict::Consume, None),
            _ => (Verdict::Forward, None),
        }
    }

    fn press(&mut self, key: KeyCode, context: Context) -> (Verdict, Option<Action>) {
        let is_ctrl = self.is_held(KeyCode::KEY_LEFTCTRL) || self.is_held(KeyCode::KEY_RIGHTCTRL);
        let is_alt = self.is_held(KeyCode::KEY_LEFTALT) || self.is_held(KeyCode::KEY_RIGHTALT);
        let is_shift = self.is_held(KeyCode::KEY_LEFTSHIFT) || self.is_held(KeyCode::KEY_RIGHTSHIFT);
        let is_meta = self.is_held(KeyCode::KEY_LEFTMETA) || self.is_held(KeyCode::KEY_RIGHTMETA);
        let swallow = if context.is_grabbed { Verdict::Consume } else { Verdict::Forward };
        if is_ctrl && is_alt && !is_shift && !is_meta && !context.has_shell_shortcuts {
            let action = match key {
                KeyCode::KEY_P => Some(Action::TogglePause),
                KeyCode::KEY_S => Some(Action::PickSource),
                _ => digit_action(key),
            };
            return (if action.is_some() { swallow } else { Verdict::Forward }, action);
        }
        if is_ctrl || is_alt || is_meta {
            return (Verdict::Forward, None);
        }
        let is_typing = context.is_text_focused || self.is_recently_typing(context.now);
        if let Some(action) = digit_action(key).filter(|_| context.is_grabbed && !is_shift && !is_typing) {
            return (Verdict::Consume, Some(action));
        }
        if is_text_key(key) {
            self.last_text_key = Some(context.now);
        }
        (Verdict::Forward, None)
    }

    fn is_recently_typing(&self, now: Instant) -> bool {
        self.last_text_key.is_some_and(|last| now.saturating_duration_since(last) < TYPING_WINDOW)
    }

    fn is_bare(&self) -> bool {
        !MODIFIERS.iter().any(|modifier| self.is_held(*modifier))
    }

    fn is_held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }
}

fn is_text_key(key: KeyCode) -> bool {
    matches!(key.code(), 2..=14 | 16..=27 | 30..=41 | 43..=53 | 57) || digit_action(key).is_some()
}

fn digit_action(key: KeyCode) -> Option<Action> {
    let level = match key {
        KeyCode::KEY_0 | KeyCode::KEY_KP0 => return Some(Action::ToggleLock),
        KeyCode::KEY_1 | KeyCode::KEY_KP1 => 1,
        KeyCode::KEY_2 | KeyCode::KEY_KP2 => 2,
        KeyCode::KEY_3 | KeyCode::KEY_KP3 => 3,
        KeyCode::KEY_4 | KeyCode::KEY_KP4 => 4,
        KeyCode::KEY_5 | KeyCode::KEY_KP5 => 5,
        KeyCode::KEY_6 | KeyCode::KEY_KP6 => 6,
        KeyCode::KEY_7 | KeyCode::KEY_KP7 => 7,
        KeyCode::KEY_8 | KeyCode::KEY_KP8 => 8,
        KeyCode::KEY_9 | KeyCode::KEY_KP9 => 9,
        _ => return None,
    };
    Some(Action::Level(level))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Harness {
        router: KeyRouter,
        start: Instant,
        is_grabbed: bool,
        is_text_focused: bool,
        has_shell_shortcuts: bool,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                router: KeyRouter::default(),
                start: Instant::now(),
                is_grabbed: true,
                is_text_focused: false,
                has_shell_shortcuts: false,
            }
        }

        fn at(&self, millis: u64) -> Context {
            Context {
                now: self.start + Duration::from_millis(millis),
                is_grabbed: self.is_grabbed,
                is_text_focused: self.is_text_focused,
                has_shell_shortcuts: self.has_shell_shortcuts,
            }
        }

        fn press(&mut self, key: KeyCode, millis: u64) -> (Verdict, Option<Action>) {
            let context = self.at(millis);
            self.router.handle(key, 1, context)
        }

        fn tap(&mut self, key: KeyCode, millis: u64) -> (Verdict, Option<Action>, Verdict) {
            let (press, action) = self.press(key, millis);
            let context = self.at(millis + 50);
            let (release, _) = self.router.handle(key, 0, context);
            (press, action, release)
        }
    }

    #[test]
    fn bare_digits_zoom_and_are_swallowed() {
        let mut keys = Harness::new();
        assert_eq!(keys.tap(KeyCode::KEY_3, 0), (Verdict::Consume, Some(Action::Level(3)), Verdict::Consume));
        assert_eq!(keys.tap(KeyCode::KEY_KP0, 100), (Verdict::Consume, Some(Action::ToggleLock), Verdict::Consume));
    }

    #[test]
    fn digits_type_into_a_focused_text_field() {
        let mut keys = Harness::new();
        keys.is_text_focused = true;
        assert_eq!(keys.tap(KeyCode::KEY_3, 0), (Verdict::Forward, None, Verdict::Forward));
    }

    #[test]
    fn digits_type_right_after_letters_then_zoom_again() {
        let mut keys = Harness::new();
        keys.tap(KeyCode::KEY_A, 0);
        assert_eq!(keys.tap(KeyCode::KEY_6, 400).1, None);
        assert_eq!(keys.tap(KeyCode::KEY_6, 900).1, None);
        assert_eq!(keys.tap(KeyCode::KEY_2, 3000).1, Some(Action::Level(2)));
    }

    #[test]
    fn typed_digits_keep_the_typing_burst_alive() {
        let mut keys = Harness::new();
        keys.tap(KeyCode::KEY_SPACE, 0);
        for (index, key) in [KeyCode::KEY_2, KeyCode::KEY_0, KeyCode::KEY_2, KeyCode::KEY_6].into_iter().enumerate() {
            assert_eq!(keys.tap(key, 1000 * (index as u64 + 1)).1, None);
        }
    }

    #[test]
    fn shortcuts_do_not_count_as_typing() {
        let mut keys = Harness::new();
        keys.press(KeyCode::KEY_LEFTCTRL, 0);
        keys.tap(KeyCode::KEY_C, 10);
        keys.router.handle(KeyCode::KEY_LEFTCTRL, 0, keys.at(60));
        assert_eq!(keys.tap(KeyCode::KEY_2, 100).1, Some(Action::Level(2)));
    }

    #[test]
    fn digits_type_normally_when_not_grabbed() {
        let mut keys = Harness::new();
        keys.is_grabbed = false;
        assert_eq!(keys.tap(KeyCode::KEY_3, 0), (Verdict::Forward, None, Verdict::Forward));
    }

    #[test]
    fn shifted_digits_still_type_symbols() {
        let mut keys = Harness::new();
        keys.press(KeyCode::KEY_LEFTSHIFT, 0);
        assert!(!keys.router.needs_focus_check(KeyCode::KEY_1, 1, true));
        assert_eq!(keys.tap(KeyCode::KEY_1, 10), (Verdict::Forward, None, Verdict::Forward));
    }

    #[test]
    fn chords_zoom_even_while_typing() {
        let mut keys = Harness::new();
        keys.is_text_focused = true;
        keys.press(KeyCode::KEY_LEFTCTRL, 0);
        keys.press(KeyCode::KEY_LEFTALT, 0);
        assert_eq!(keys.tap(KeyCode::KEY_4, 10), (Verdict::Consume, Some(Action::Level(4)), Verdict::Consume));
        assert_eq!(keys.tap(KeyCode::KEY_P, 20).1, Some(Action::TogglePause));
    }

    #[test]
    fn chords_pass_through_when_the_shell_owns_them() {
        let mut keys = Harness::new();
        keys.has_shell_shortcuts = true;
        keys.press(KeyCode::KEY_LEFTCTRL, 0);
        keys.press(KeyCode::KEY_LEFTALT, 0);
        assert_eq!(keys.tap(KeyCode::KEY_4, 10), (Verdict::Forward, None, Verdict::Forward));
        assert_eq!(keys.tap(KeyCode::KEY_P, 20), (Verdict::Forward, None, Verdict::Forward));
    }

    #[test]
    fn focus_check_only_for_bare_digit_presses() {
        let keys = Harness::new();
        assert!(keys.router.needs_focus_check(KeyCode::KEY_5, 1, true));
        assert!(!keys.router.needs_focus_check(KeyCode::KEY_5, 0, true));
        assert!(!keys.router.needs_focus_check(KeyCode::KEY_5, 1, false));
        assert!(!keys.router.needs_focus_check(KeyCode::KEY_A, 1, true));
    }

    #[test]
    fn repeats_of_swallowed_keys_are_swallowed() {
        let mut keys = Harness::new();
        keys.press(KeyCode::KEY_2, 0);
        let context = keys.at(500);
        assert_eq!(keys.router.handle(KeyCode::KEY_2, 2, context), (Verdict::Consume, None));
        assert_eq!(keys.router.handle(KeyCode::KEY_2, 0, context), (Verdict::Consume, None));
        assert!(keys.router.is_idle());
    }
}
