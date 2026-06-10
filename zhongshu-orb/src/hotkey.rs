use crate::config::HotkeyConfig;
use anyhow::Context;
use global_hotkey::{
    GlobalHotKeyManager, GlobalHotKeyEvent,
    hotkey::{HotKey, Modifiers, Code},
};

// ── HotkeyManager ───────────────────────────────────────────────────

pub struct HotkeyManager {
    #[allow(dead_code)]
    active: HotKey,
    events: crossbeam_channel::Receiver<GlobalHotKeyEvent>,
    #[allow(dead_code)]
    manager: Option<GlobalHotKeyManager>,
}

impl HotkeyManager {
    pub fn new(config: &HotkeyConfig) -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()
            .context("GlobalHotKeyManager::new failed")?;

        let active = build_hotkey(config);
        if let Err(e) = manager.register(active) {
            tracing::warn!("Failed to register global hotkey: {e}");
        }

        let events = GlobalHotKeyEvent::receiver().clone();

        Ok(HotkeyManager { active, events, manager: Some(manager) })
    }

    /// Create a no-op manager when hotkey registration is unavailable.
    pub fn passive() -> Self {
        let (_, rx) = crossbeam_channel::unbounded();
        HotkeyManager {
            active: HotKey::new(None, Code::Semicolon),
            events: rx,
            manager: None,
        }
    }

    /// Consume the next pending hotkey event.  Call from a single owner.
    pub fn try_recv(&mut self) -> Option<GlobalHotKeyEvent> {
        self.events.try_recv().ok()
    }
}

// ── Hotkey construction ─────────────────────────────────────────────

fn build_hotkey(cfg: &HotkeyConfig) -> HotKey {
    build_hotkey_safe(cfg).unwrap_or_else(|| {
        tracing::warn!("Hotkey config unparseable ({:?}), using default Meta+Semicolon", cfg);
        HotKey::new(Some(Modifiers::META), Code::Semicolon)
    })
}

fn build_hotkey_safe(cfg: &HotkeyConfig) -> Option<HotKey> {
    let modifiers = parse_modifiers(&cfg.modifiers);
    let code = parse_code(&cfg.key)?;
    Some(HotKey::new(modifiers, code))
}

fn parse_modifiers(names: &[String]) -> Option<Modifiers> {
    let mut result = Modifiers::empty();
    for name in names {
        match name.as_str() {
            "Meta" | "Super" | "Win" => result |= Modifiers::META,
            "Ctrl" | "Control" => result |= Modifiers::CONTROL,
            "Alt" => result |= Modifiers::ALT,
            "Shift" => result |= Modifiers::SHIFT,
            unknown => tracing::warn!("Unknown modifier '{}', ignoring", unknown),
        }
    }
    if result.is_empty() { None } else { Some(result) }
}

fn parse_code(name: &str) -> Option<Code> {
    use Code::*;
    Some(match name {
        "A" => KeyA, "B" => KeyB, "C" => KeyC, "D" => KeyD, "E" => KeyE,
        "F" => KeyF, "G" => KeyG, "H" => KeyH, "I" => KeyI, "J" => KeyJ,
        "K" => KeyK, "L" => KeyL, "M" => KeyM, "N" => KeyN, "O" => KeyO,
        "P" => KeyP, "Q" => KeyQ, "R" => KeyR, "S" => KeyS, "T" => KeyT,
        "U" => KeyU, "V" => KeyV, "W" => KeyW, "X" => KeyX, "Y" => KeyY,
        "Z" => KeyZ,
        "0" => Digit0, "1" => Digit1, "2" => Digit2, "3" => Digit3, "4" => Digit4,
        "5" => Digit5, "6" => Digit6, "7" => Digit7, "8" => Digit8, "9" => Digit9,
        "F1" => F1, "F2" => F2, "F3" => F3, "F4" => F4, "F5" => F5,
        "F6" => F6, "F7" => F7, "F8" => F8, "F9" => F9, "F10" => F10,
        "F11" => F11, "F12" => F12,
        "Space" => Space, "Enter" => Enter, "Tab" => Tab,
        "Escape" => Escape, "Backspace" => Backspace, "Delete" => Delete,
        "Semicolon" => Semicolon, "Comma" => Comma, "Period" => Period,
        "Slash" => Slash, "Backslash" => Backslash,
        "Quote" => Quote, "Backquote" => Backquote,
        "Minus" => Minus, "Equal" => Equal,
        _ => return None,
    })
}
