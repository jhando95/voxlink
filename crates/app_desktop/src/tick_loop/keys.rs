use device_query::Keycode;

/// Parse a config key name string to a Keycode.
pub fn parse_key(name: &str) -> Option<Keycode> {
    Some(match name.to_lowercase().as_str() {
        // Letters
        "a" => Keycode::A,
        "b" => Keycode::B,
        "c" => Keycode::C,
        "d" => Keycode::D,
        "e" => Keycode::E,
        "f" => Keycode::F,
        "g" => Keycode::G,
        "h" => Keycode::H,
        "i" => Keycode::I,
        "j" => Keycode::J,
        "k" => Keycode::K,
        "l" => Keycode::L,
        "m" => Keycode::M,
        "n" => Keycode::N,
        "o" => Keycode::O,
        "p" => Keycode::P,
        "q" => Keycode::Q,
        "r" => Keycode::R,
        "s" => Keycode::S,
        "t" => Keycode::T,
        "u" => Keycode::U,
        "v" => Keycode::V,
        "w" => Keycode::W,
        "x" => Keycode::X,
        "y" => Keycode::Y,
        "z" => Keycode::Z,
        // Numbers
        "0" | "key0" => Keycode::Key0,
        "1" | "key1" => Keycode::Key1,
        "2" | "key2" => Keycode::Key2,
        "3" | "key3" => Keycode::Key3,
        "4" | "key4" => Keycode::Key4,
        "5" | "key5" => Keycode::Key5,
        "6" | "key6" => Keycode::Key6,
        "7" | "key7" => Keycode::Key7,
        "8" | "key8" => Keycode::Key8,
        "9" | "key9" => Keycode::Key9,
        // Function keys
        "f1" => Keycode::F1,
        "f2" => Keycode::F2,
        "f3" => Keycode::F3,
        "f4" => Keycode::F4,
        "f5" => Keycode::F5,
        "f6" => Keycode::F6,
        "f7" => Keycode::F7,
        "f8" => Keycode::F8,
        "f9" => Keycode::F9,
        "f10" => Keycode::F10,
        "f11" => Keycode::F11,
        "f12" => Keycode::F12,
        // Special keys
        "space" => Keycode::Space,
        "tab" => Keycode::Tab,
        "capslock" => Keycode::CapsLock,
        "escape" | "esc" => Keycode::Escape,
        "enter" | "return" => Keycode::Enter,
        "backspace" => Keycode::Backspace,
        "delete" => Keycode::Delete,
        "grave" | "`" => Keycode::Grave,
        // Modifiers
        "lshift" | "leftshift" | "left shift" => Keycode::LShift,
        "rshift" | "rightshift" | "right shift" => Keycode::RShift,
        "lcontrol" | "lctrl" | "leftcontrol" | "left ctrl" => Keycode::LControl,
        "rcontrol" | "rctrl" | "rightcontrol" | "right ctrl" => Keycode::RControl,
        "lalt" | "leftalt" | "left alt" => Keycode::LAlt,
        "ralt" | "rightalt" | "right alt" => Keycode::RAlt,
        "lmeta" | "lcommand" | "lcmd" | "lsuper" => Keycode::LMeta,
        "rmeta" | "rcommand" | "rcmd" | "rsuper" => Keycode::RMeta,
        _ => return None,
    })
}

/// Config-safe name for a keycode (lowercase, for storage).
pub fn keycode_to_config_name(key: Keycode) -> &'static str {
    match key {
        Keycode::A => "a",
        Keycode::B => "b",
        Keycode::C => "c",
        Keycode::D => "d",
        Keycode::E => "e",
        Keycode::F => "f",
        Keycode::G => "g",
        Keycode::H => "h",
        Keycode::I => "i",
        Keycode::J => "j",
        Keycode::K => "k",
        Keycode::L => "l",
        Keycode::M => "m",
        Keycode::N => "n",
        Keycode::O => "o",
        Keycode::P => "p",
        Keycode::Q => "q",
        Keycode::R => "r",
        Keycode::S => "s",
        Keycode::T => "t",
        Keycode::U => "u",
        Keycode::V => "v",
        Keycode::W => "w",
        Keycode::X => "x",
        Keycode::Y => "y",
        Keycode::Z => "z",
        Keycode::Key0 => "0",
        Keycode::Key1 => "1",
        Keycode::Key2 => "2",
        Keycode::Key3 => "3",
        Keycode::Key4 => "4",
        Keycode::Key5 => "5",
        Keycode::Key6 => "6",
        Keycode::Key7 => "7",
        Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::F1 => "f1",
        Keycode::F2 => "f2",
        Keycode::F3 => "f3",
        Keycode::F4 => "f4",
        Keycode::F5 => "f5",
        Keycode::F6 => "f6",
        Keycode::F7 => "f7",
        Keycode::F8 => "f8",
        Keycode::F9 => "f9",
        Keycode::F10 => "f10",
        Keycode::F11 => "f11",
        Keycode::F12 => "f12",
        Keycode::Space => "space",
        Keycode::Tab => "tab",
        Keycode::CapsLock => "capslock",
        Keycode::Escape => "escape",
        Keycode::Enter => "enter",
        Keycode::Backspace => "backspace",
        Keycode::Delete => "delete",
        Keycode::Grave => "grave",
        Keycode::LShift => "lshift",
        Keycode::RShift => "rshift",
        Keycode::LControl => "lcontrol",
        Keycode::RControl => "rcontrol",
        Keycode::LAlt => "lalt",
        Keycode::RAlt => "ralt",
        Keycode::LMeta => "lmeta",
        Keycode::RMeta => "rmeta",
        _ => "unknown",
    }
}

/// Human-readable display name for a keycode.
pub fn keycode_to_display_name(key: Keycode) -> &'static str {
    match key {
        Keycode::A => "A",
        Keycode::B => "B",
        Keycode::C => "C",
        Keycode::D => "D",
        Keycode::E => "E",
        Keycode::F => "F",
        Keycode::G => "G",
        Keycode::H => "H",
        Keycode::I => "I",
        Keycode::J => "J",
        Keycode::K => "K",
        Keycode::L => "L",
        Keycode::M => "M",
        Keycode::N => "N",
        Keycode::O => "O",
        Keycode::P => "P",
        Keycode::Q => "Q",
        Keycode::R => "R",
        Keycode::S => "S",
        Keycode::T => "T",
        Keycode::U => "U",
        Keycode::V => "V",
        Keycode::W => "W",
        Keycode::X => "X",
        Keycode::Y => "Y",
        Keycode::Z => "Z",
        Keycode::Key0 => "0",
        Keycode::Key1 => "1",
        Keycode::Key2 => "2",
        Keycode::Key3 => "3",
        Keycode::Key4 => "4",
        Keycode::Key5 => "5",
        Keycode::Key6 => "6",
        Keycode::Key7 => "7",
        Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::F1 => "F1",
        Keycode::F2 => "F2",
        Keycode::F3 => "F3",
        Keycode::F4 => "F4",
        Keycode::F5 => "F5",
        Keycode::F6 => "F6",
        Keycode::F7 => "F7",
        Keycode::F8 => "F8",
        Keycode::F9 => "F9",
        Keycode::F10 => "F10",
        Keycode::F11 => "F11",
        Keycode::F12 => "F12",
        Keycode::Space => "Space",
        Keycode::Tab => "Tab",
        Keycode::CapsLock => "Caps Lock",
        Keycode::Escape => "Escape",
        Keycode::Enter => "Enter",
        Keycode::Backspace => "Backspace",
        Keycode::Delete => "Delete",
        Keycode::Grave => "`",
        Keycode::LShift => "Shift",
        Keycode::RShift => "R.Shift",
        Keycode::LControl => "Ctrl",
        Keycode::RControl => "R.Ctrl",
        Keycode::LAlt => "Alt",
        Keycode::RAlt => "R.Alt",
        Keycode::LMeta => "Cmd",
        Keycode::RMeta => "R.Cmd",
        _ => "Unknown",
    }
}

/// Sort order: modifiers first (shift, ctrl, alt, meta), then regular keys.
pub fn keycode_sort_order(k: Keycode) -> u8 {
    match k {
        Keycode::LControl | Keycode::RControl => 0,
        Keycode::LAlt | Keycode::RAlt => 1,
        Keycode::LShift | Keycode::RShift => 2,
        Keycode::LMeta | Keycode::RMeta => 3,
        _ => 10,
    }
}

/// Parse a combo config string like "lshift+m" into a sorted Vec of Keycodes.
pub fn parse_combo(config: &str) -> Vec<Keycode> {
    if config.is_empty() {
        return Vec::new();
    }
    let mut keys: Vec<Keycode> = config
        .split('+')
        .filter_map(|part| parse_key(part.trim()))
        .collect();
    keys.sort_by_key(|k| keycode_sort_order(*k));
    keys.dedup();
    keys
}

/// Display name for a combo: "Shift + M".
pub fn combo_to_display(combo: &[Keycode]) -> String {
    if combo.is_empty() {
        return String::new();
    }
    combo
        .iter()
        .map(|k| keycode_to_display_name(*k))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Config storage name for a combo: "lshift+m".
pub fn combo_to_config(combo: &[Keycode]) -> String {
    if combo.is_empty() {
        return String::new();
    }
    combo
        .iter()
        .map(|k| keycode_to_config_name(*k))
        .collect::<Vec<_>>()
        .join("+")
}

/// Check if all keys in a combo are currently held.
pub fn combo_held(combo: &[Keycode], keys: &[Keycode]) -> bool {
    !combo.is_empty() && combo.iter().all(|k| keys.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_letters() {
        assert_eq!(parse_key("a"), Some(Keycode::A));
        assert_eq!(parse_key("M"), Some(Keycode::M));
        assert_eq!(parse_key("Z"), Some(Keycode::Z));
    }

    #[test]
    fn parse_key_space() {
        assert_eq!(parse_key("space"), Some(Keycode::Space));
        assert_eq!(parse_key("Space"), Some(Keycode::Space));
        assert_eq!(parse_key("SPACE"), Some(Keycode::Space));
    }

    #[test]
    fn parse_key_modifiers() {
        assert_eq!(parse_key("lcontrol"), Some(Keycode::LControl));
        assert_eq!(parse_key("lctrl"), Some(Keycode::LControl));
        assert_eq!(parse_key("rshift"), Some(Keycode::RShift));
        assert_eq!(parse_key("lalt"), Some(Keycode::LAlt));
    }

    #[test]
    fn parse_key_numbers() {
        assert_eq!(parse_key("0"), Some(Keycode::Key0));
        assert_eq!(parse_key("9"), Some(Keycode::Key9));
    }

    #[test]
    fn parse_key_function() {
        assert_eq!(parse_key("f1"), Some(Keycode::F1));
        assert_eq!(parse_key("F12"), Some(Keycode::F12));
    }

    #[test]
    fn parse_key_special() {
        assert_eq!(parse_key("tab"), Some(Keycode::Tab));
        assert_eq!(parse_key("capslock"), Some(Keycode::CapsLock));
        assert_eq!(parse_key("grave"), Some(Keycode::Grave));
        assert_eq!(parse_key("`"), Some(Keycode::Grave));
        assert_eq!(parse_key("enter"), Some(Keycode::Enter));
    }

    #[test]
    fn parse_key_unknown() {
        assert_eq!(parse_key("unknown"), None);
        assert_eq!(parse_key("xyz"), None);
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn config_name_round_trip() {
        let keys = [
            Keycode::Space,
            Keycode::M,
            Keycode::D,
            Keycode::LShift,
            Keycode::F5,
        ];
        for &key in &keys {
            let name = keycode_to_config_name(key);
            assert_eq!(parse_key(name), Some(key), "Round-trip failed for {name}");
        }
    }

    #[test]
    fn display_names() {
        assert_eq!(keycode_to_display_name(Keycode::Space), "Space");
        assert_eq!(keycode_to_display_name(Keycode::LShift), "Shift");
        assert_eq!(keycode_to_display_name(Keycode::M), "M");
        assert_eq!(keycode_to_display_name(Keycode::F1), "F1");
    }

    #[test]
    fn parse_combo_single() {
        assert_eq!(parse_combo("m"), vec![Keycode::M]);
        assert_eq!(parse_combo("space"), vec![Keycode::Space]);
    }

    #[test]
    fn parse_combo_multi() {
        let combo = parse_combo("lshift+m");
        assert_eq!(combo, vec![Keycode::LShift, Keycode::M]);
    }

    #[test]
    fn parse_combo_display() {
        let combo = parse_combo("lshift+m");
        assert_eq!(combo_to_display(&combo), "Shift + M");
    }

    #[test]
    fn parse_combo_config_round_trip() {
        let combo = parse_combo("lctrl+lalt+d");
        let config = combo_to_config(&combo);
        let parsed = parse_combo(&config);
        assert_eq!(combo, parsed);
    }

    #[test]
    fn combo_held_check() {
        let combo = parse_combo("lshift+m");
        assert!(combo_held(
            &combo,
            &[Keycode::LShift, Keycode::M, Keycode::A]
        ));
        assert!(!combo_held(&combo, &[Keycode::LShift]));
        assert!(!combo_held(&combo, &[Keycode::M]));
        assert!(!combo_held(&combo, &[]));
    }

    #[test]
    fn empty_combo() {
        assert!(parse_combo("").is_empty());
        assert_eq!(combo_to_display(&[]), "");
        assert_eq!(combo_to_config(&[]), "");
        assert!(!combo_held(&[], &[Keycode::A]));
    }
}
