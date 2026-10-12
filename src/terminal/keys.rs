use std::borrow::Cow;

use gpui::Keystroke;

use alacritty_terminal::term::TermMode;

#[derive(Debug, PartialEq, Eq)]
enum TerminalModifiers {
    None,
    Alt,
    Ctrl,
    Shift,
    CtrlShift,
    Other,
}

impl TerminalModifiers {
    fn new(ks: &Keystroke) -> Self {
        match (
            ks.modifiers.alt,
            ks.modifiers.control,
            ks.modifiers.shift,
            ks.modifiers.platform,
        ) {
            (false, false, false, false) => TerminalModifiers::None,
            (true, false, false, false) => TerminalModifiers::Alt,
            (false, true, false, false) => TerminalModifiers::Ctrl,
            (false, false, true, false) => TerminalModifiers::Shift,
            (false, true, true, false) => TerminalModifiers::CtrlShift,
            _ => TerminalModifiers::Other,
        }
    }
}

pub(crate) fn to_esc_str(
    keystroke: &Keystroke,
    mode: TermMode,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let modifiers = TerminalModifiers::new(keystroke);
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // manual bindings including modifiers
    let manual_esc_str: Option<&'static str> = match (keystroke.key.as_ref(), &modifiers) {
        // basic special keys
        ("tab", TerminalModifiers::None) => Some("\x09"),
        ("escape", TerminalModifiers::None) => Some("\x1b"),
        ("enter", TerminalModifiers::None) => Some("\x0d"),
        ("enter", TerminalModifiers::Shift) => Some("\x0a"),
        ("enter", TerminalModifiers::Alt) => Some("\x1b\x0d"),
        ("backspace", TerminalModifiers::None) => Some("\x7f"),
        // interesting escape codes
        ("tab", TerminalModifiers::Shift) => Some("\x1b[Z"),
        ("backspace", TerminalModifiers::Ctrl) => Some("\x08"),
        ("backspace", TerminalModifiers::Alt) => Some("\x1b\x7f"),
        ("backspace", TerminalModifiers::Shift) => Some("\x7f"),
        ("space", TerminalModifiers::Ctrl) => Some("\x00"),
        ("home", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOH"),
        ("home", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => Some("\x1b[H"),
        ("end", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOF"),
        ("end", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => Some("\x1b[F"),
        ("up", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOA"),
        ("up", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => Some("\x1b[A"),
        ("down", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOB"),
        ("down", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => Some("\x1b[B"),
        ("right", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOC"),
        ("right", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => {
            Some("\x1b[C")
        }
        ("left", TerminalModifiers::None) if mode.contains(TermMode::APP_CURSOR) => Some("\x1bOD"),
        ("left", TerminalModifiers::None) if !mode.contains(TermMode::APP_CURSOR) => Some("\x1b[D"),
        ("back", TerminalModifiers::None) => Some("\x7f"),
        ("insert", TerminalModifiers::None) => Some("\x1b[2~"),
        ("delete", TerminalModifiers::None) => Some("\x1b[3~"),
        ("pageup", TerminalModifiers::None) => Some("\x1b[5~"),
        ("pagedown", TerminalModifiers::None) => Some("\x1b[6~"),
        ("home", TerminalModifiers::None) => Some(if app_cursor { "\x1bOH" } else { "\x1b[H" }),
        ("end", TerminalModifiers::None) => Some(if app_cursor { "\x1bOF" } else { "\x1b[F" }),
        ("up", TerminalModifiers::None) => Some(if app_cursor { "\x1bOA" } else { "\x1b[A" }),
        ("down", TerminalModifiers::None) => Some(if app_cursor { "\x1bOB" } else { "\x1b[B" }),
        ("right", TerminalModifiers::None) => Some(if app_cursor { "\x1bOC" } else { "\x1b[C" }),
        ("left", TerminalModifiers::None) => Some(if app_cursor { "\x1bOD" } else { "\x1b[D" }),
        ("f1", TerminalModifiers::None) => Some("\x1bOP"),
        ("f2", TerminalModifiers::None) => Some("\x1bOQ"),
        ("f3", TerminalModifiers::None) => Some("\x1bOR"),
        ("f4", TerminalModifiers::None) => Some("\x1bOS"),
        ("f5", TerminalModifiers::None) => Some("\x1b[15~"),
        ("f6", TerminalModifiers::None) => Some("\x1b[17~"),
        ("f7", TerminalModifiers::None) => Some("\x1b[18~"),
        ("f8", TerminalModifiers::None) => Some("\x1b[19~"),
        ("f9", TerminalModifiers::None) => Some("\x1b[20~"),
        ("f10", TerminalModifiers::None) => Some("\x1b[21~"),
        ("f11", TerminalModifiers::None) => Some("\x1b[23~"),
        ("f12", TerminalModifiers::None) => Some("\x1b[24~"),
        ("f13", TerminalModifiers::None) => Some("\x1b[25~"),
        ("f14", TerminalModifiers::None) => Some("\x1b[26~"),
        ("f15", TerminalModifiers::None) => Some("\x1b[28~"),
        ("f16", TerminalModifiers::None) => Some("\x1b[29~"),
        ("f17", TerminalModifiers::None) => Some("\x1b[31~"),
        ("f18", TerminalModifiers::None) => Some("\x1b[32~"),
        ("f19", TerminalModifiers::None) => Some("\x1b[33~"),
        ("f20", TerminalModifiers::None) => Some("\x1b[34~"),
        ("@", TerminalModifiers::Ctrl) => Some("\x00"), //0
        ("[", TerminalModifiers::Ctrl) => Some("\x1b"), //27
        ("\\", TerminalModifiers::Ctrl) => Some("\x1c"), //28
        ("]", TerminalModifiers::Ctrl) => Some("\x1d"), //29
        ("^", TerminalModifiers::Ctrl) => Some("\x1e"), //30
        ("_", TerminalModifiers::Ctrl) => Some("\x1f"), //31
        ("?", TerminalModifiers::Ctrl) => Some("\x7f"), //127
        _ => None,
    };
    if let Some(esc_str) = manual_esc_str {
        return Some(Cow::Borrowed(esc_str));
    }

    // caret notation: ctrl-a..z and ctrl-shift-A..Z send 0x01..0x1a
    if let [key] = keystroke.key.as_bytes()
        && ((modifiers == TerminalModifiers::Ctrl && key.is_ascii_lowercase())
            || (modifiers == TerminalModifiers::CtrlShift && key.is_ascii_uppercase()))
    {
        return Some(Cow::Owned(char::from(key & 0x1f).to_string()));
    }

    // automated bindings applying modifiers
    if modifiers != TerminalModifiers::None {
        let modifier_code = modifier_code(keystroke);
        let modified_esc_str = match keystroke.key.as_ref() {
            "up" => Some(format!("\x1b[1;{}A", modifier_code)),
            "down" => Some(format!("\x1b[1;{}B", modifier_code)),
            "right" => Some(format!("\x1b[1;{}C", modifier_code)),
            "left" => Some(format!("\x1b[1;{}D", modifier_code)),
            "f1" => Some(format!("\x1b[1;{}P", modifier_code)),
            "f2" => Some(format!("\x1b[1;{}Q", modifier_code)),
            "f3" => Some(format!("\x1b[1;{}R", modifier_code)),
            "f4" => Some(format!("\x1b[1;{}S", modifier_code)),
            "f5" => Some(format!("\x1b[15;{}~", modifier_code)),
            "f6" => Some(format!("\x1b[17;{}~", modifier_code)),
            "f7" => Some(format!("\x1b[18;{}~", modifier_code)),
            "f8" => Some(format!("\x1b[19;{}~", modifier_code)),
            "f9" => Some(format!("\x1b[20;{}~", modifier_code)),
            "f10" => Some(format!("\x1b[21;{}~", modifier_code)),
            "f11" => Some(format!("\x1b[23;{}~", modifier_code)),
            "f12" => Some(format!("\x1b[24;{}~", modifier_code)),
            "f13" => Some(format!("\x1b[25;{}~", modifier_code)),
            "f14" => Some(format!("\x1b[26;{}~", modifier_code)),
            "f15" => Some(format!("\x1b[28;{}~", modifier_code)),
            "f16" => Some(format!("\x1b[29;{}~", modifier_code)),
            "f17" => Some(format!("\x1b[31;{}~", modifier_code)),
            "f18" => Some(format!("\x1b[32;{}~", modifier_code)),
            "f19" => Some(format!("\x1b[33;{}~", modifier_code)),
            "f20" => Some(format!("\x1b[34;{}~", modifier_code)),
            "insert" => Some(format!("\x1b[2;{}~", modifier_code)),
            "pageup" => Some(format!("\x1b[5;{}~", modifier_code)),
            "pagedown" => Some(format!("\x1b[6;{}~", modifier_code)),
            "end" => Some(format!("\x1b[1;{}F", modifier_code)),
            "home" => Some(format!("\x1b[1;{}H", modifier_code)),
            _ => None,
        };
        if let Some(esc_str) = modified_esc_str {
            return Some(Cow::Owned(esc_str));
        }
    }

    // handling alt keys, these work for single ASCII characters.
    if (!cfg!(target_os = "macos") || option_as_meta)
        && keystroke.modifiers.alt
        && keystroke.key.is_ascii()
        && keystroke.key.len() == 1
    {
        let key = keystroke.key.chars().next().unwrap();

        if modifiers == TerminalModifiers::Alt {
            return Some(Cow::Owned(format!("\x1b{key}")));
        } else if keystroke.modifiers.shift {
            return Some(Cow::Owned(format!("\x1b{}", key.to_ascii_uppercase())));
        } else if keystroke.modifiers.control && key.is_ascii_lowercase() {
            return Some(Cow::Owned(format!(
                "\x1b{}",
                (key as u8 - b'a' + 1) as char
            )));
        }
    }

    None
}

///   Code     Modifiers
/// ---------+---------------------------
///    2     | Shift
///    3     | Alt
///    4     | Shift + Alt
///    5     | Control
///    6     | Shift + Control
///    7     | Alt + Control
///    8     | Shift + Alt + Control
/// ---------+---------------------------
/// from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
fn modifier_code(keystroke: &Keystroke) -> u32 {
    let m = &keystroke.modifiers;
    1 + m.shift as u32 + ((m.alt as u32) << 1) + ((m.control as u32) << 2)
}

/// press, repeat or release of a key, kitty's keyboard protocol tells them apart
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

/// how a key is written in kitty's keyboard protocol: `CSI code u`, `CSI 1 X` for keys
/// ending in a letter like arrows, or `CSI number ~`
enum KittyKey {
    Code(u32),
    Letter(char),
    Tilde(u32),
}

fn kitty_key(key: &str) -> Option<KittyKey> {
    use KittyKey::*;
    Some(match key {
        "escape" => Code(27),
        "enter" => Code(13),
        "tab" => Code(9),
        "backspace" => Code(127),
        "space" => Code(32),
        "insert" => Tilde(2),
        "delete" => Tilde(3),
        "pageup" => Tilde(5),
        "pagedown" => Tilde(6),
        "up" => Letter('A'),
        "down" => Letter('B'),
        "right" => Letter('C'),
        "left" => Letter('D'),
        "home" => Letter('H'),
        "end" => Letter('F'),
        "f1" => Letter('P'),
        "f2" => Letter('Q'),
        // CSI R is also a cursor position report
        "f3" => Tilde(13),
        "f4" => Letter('S'),
        "f5" => Tilde(15),
        "f6" => Tilde(17),
        "f7" => Tilde(18),
        "f8" => Tilde(19),
        "f9" => Tilde(20),
        "f10" => Tilde(21),
        "f11" => Tilde(23),
        "f12" => Tilde(24),
        _ => {
            if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<u32>().ok())
                && (13..=35).contains(&n)
            {
                return Some(Code(57376 + n - 13));
            }
            let mut chars = key.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            Code(c.to_lowercase().next().unwrap_or(c) as u32)
        }
    })
}

/// a key in kitty's keyboard protocol, for programs that turned it on with `CSI > flags u`.
/// none leaves the key to the legacy encoding or to text input
pub(crate) fn to_kitty_str(
    keystroke: &Keystroke,
    mode: TermMode,
    kind: KeyEventKind,
) -> Option<String> {
    let all_keys = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    if !all_keys && !mode.contains(TermMode::DISAMBIGUATE_ESC_CODES) {
        return None;
    }
    let key = kitty_key(&keystroke.key)?;
    let m = &keystroke.modifiers;
    let mods = 1
        + m.shift as u32
        + ((m.alt as u32) << 1)
        + ((m.control as u32) << 2)
        + ((m.platform as u32) << 3);
    let text_key =
        matches!(key, KittyKey::Code(code) if !matches!(code, 27 | 13 | 9 | 127) && code < 57344);
    if !all_keys {
        match key {
            // enter, tab and backspace stay plain, so a shell left in this mode still runs
            // commands like `reset`
            KittyKey::Code(13 | 9 | 127) if mods == 1 => return None,
            // text keys keep typing text, shift only picks the character
            KittyKey::Code(_) if text_key && !(m.alt || m.control || m.platform) => {
                return None;
            }
            // other keys keep their legacy form unless the program wants the event type
            KittyKey::Letter(_) | KittyKey::Tilde(_)
                if !mode.contains(TermMode::REPORT_EVENT_TYPES) =>
            {
                return None;
            }
            _ => {}
        }
    }
    let event_types = mode.contains(TermMode::REPORT_EVENT_TYPES);
    let event = match kind {
        KeyEventKind::Press => None,
        // without event types a repeat is one more press
        KeyEventKind::Repeat if !event_types => None,
        KeyEventKind::Release if !event_types => return None,
        KeyEventKind::Repeat => Some(2),
        KeyEventKind::Release => Some(3),
    };
    let mut params = String::new();
    if mods != 1 || event.is_some() {
        params = mods.to_string();
        if let Some(event) = event {
            params.push_str(&format!(":{event}"));
        }
    }
    Some(match key {
        KittyKey::Code(code) => {
            let mut code = code.to_string();
            // the shifted key goes after the base key, like 97:65 for shift-a
            if mode.contains(TermMode::REPORT_ALTERNATE_KEYS)
                && m.shift
                && text_key
                && let Some(shifted) = keystroke.key_char.as_deref().and_then(single_char)
                && shifted.to_string() != keystroke.key
            {
                code.push_str(&format!(":{}", shifted as u32));
            }
            let text = (all_keys
                && mode.contains(TermMode::REPORT_ASSOCIATED_TEXT)
                && kind != KeyEventKind::Release)
                .then_some(keystroke.key_char.as_deref())
                .flatten()
                .filter(|text| text.chars().all(|c| !c.is_control()))
                .map(|text| {
                    text.chars()
                        .map(|c| (c as u32).to_string())
                        .collect::<Vec<_>>()
                        .join(":")
                });
            match text {
                Some(text) => {
                    let params = if params.is_empty() {
                        "1".into()
                    } else {
                        params
                    };
                    format!("\x1b[{code};{params};{text}u")
                }
                None if params.is_empty() => format!("\x1b[{code}u"),
                None => format!("\x1b[{code};{params}u"),
            }
        }
        KittyKey::Letter(letter) if params.is_empty() => format!("\x1b[{letter}"),
        KittyKey::Letter(letter) => format!("\x1b[1;{params}{letter}"),
        KittyKey::Tilde(number) if params.is_empty() => format!("\x1b[{number}~"),
        KittyKey::Tilde(number) => format!("\x1b[{number};{params}~"),
    })
}

fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    chars.next().is_none().then_some(c)
}

#[cfg(test)]
mod test {
    use gpui::Modifiers;

    use super::*;

    #[test]
    fn test_plain_inputs() {
        let ks = Keystroke {
            modifiers: Modifiers {
                control: false,
                alt: false,
                shift: false,
                platform: false,
                function: false,
            },
            key: "🖖🏻".to_string(), //2 char string
            key_char: None,
        };
        assert_eq!(to_esc_str(&ks, TermMode::empty(), false), None);
    }

    #[test]
    fn test_application_mode() {
        let app_cursor = TermMode::APP_CURSOR;
        let none = TermMode::empty();

        let up = Keystroke::parse("up").unwrap();
        let down = Keystroke::parse("down").unwrap();
        let left = Keystroke::parse("left").unwrap();
        let right = Keystroke::parse("right").unwrap();

        assert_eq!(to_esc_str(&up, none, false), Some("\x1b[A".into()));
        assert_eq!(to_esc_str(&down, none, false), Some("\x1b[B".into()));
        assert_eq!(to_esc_str(&right, none, false), Some("\x1b[C".into()));
        assert_eq!(to_esc_str(&left, none, false), Some("\x1b[D".into()));

        assert_eq!(to_esc_str(&up, app_cursor, false), Some("\x1bOA".into()));
        assert_eq!(to_esc_str(&down, app_cursor, false), Some("\x1bOB".into()));
        assert_eq!(to_esc_str(&right, app_cursor, false), Some("\x1bOC".into()));
        assert_eq!(to_esc_str(&left, app_cursor, false), Some("\x1bOD".into()));

        let home = Keystroke::parse("home").unwrap();
        let end = Keystroke::parse("end").unwrap();
        assert_eq!(to_esc_str(&home, none, false), Some("\x1b[H".into()));
        assert_eq!(to_esc_str(&end, none, false), Some("\x1b[F".into()));
        assert_eq!(to_esc_str(&home, app_cursor, false), Some("\x1bOH".into()));
        assert_eq!(to_esc_str(&end, app_cursor, false), Some("\x1bOF".into()));

        let shift_up = Keystroke::parse("shift-up").unwrap();
        let shift_down = Keystroke::parse("shift-down").unwrap();
        let shift_home = Keystroke::parse("shift-home").unwrap();
        let shift_end = Keystroke::parse("shift-end").unwrap();
        assert_eq!(to_esc_str(&shift_up, none, false), Some("\x1b[1;2A".into()));
        assert_eq!(
            to_esc_str(&shift_down, none, false),
            Some("\x1b[1;2B".into())
        );
        assert_eq!(
            to_esc_str(&shift_home, none, false),
            Some("\x1b[1;2H".into())
        );
        assert_eq!(
            to_esc_str(&shift_end, none, false),
            Some("\x1b[1;2F".into())
        );
    }

    #[test]
    fn test_ctrl_codes() {
        let letters_lower = 'a'..='z';
        let letters_upper = 'A'..='Z';
        let mode = TermMode::APP_CURSOR;

        for (lower, upper) in letters_lower.zip(letters_upper) {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-shift-{}", lower)).unwrap(),
                    mode,
                    false
                ),
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-{}", upper)).unwrap(),
                    mode,
                    false
                ),
                "On letter: {}/{}",
                lower,
                upper
            )
        }
    }

    #[test]
    fn alt_is_meta() {
        let ascii_printable = ' '..='~';
        for character in ascii_printable {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", character)).unwrap(),
                    TermMode::empty(),
                    true
                )
                .unwrap(),
                format!("\x1b{}", character)
            );
        }

        let gpui_keys = [
            "up", "down", "right", "left", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9",
            "f10", "f11", "f12", "f13", "f14", "f15", "f16", "f17", "f18", "f19", "f20", "insert",
            "pageup", "pagedown", "end", "home",
        ];

        for key in gpui_keys {
            assert_ne!(
                to_esc_str(
                    &Keystroke::parse(&format!("alt-{}", key)).unwrap(),
                    TermMode::empty(),
                    true
                )
                .unwrap(),
                format!("\x1b{}", key)
            );
        }
    }

    #[test]
    fn test_ctrl_alt_codes() {
        let letters_lower = 'a'..='z';
        let esc_codes = '\x01'..='\x1a';
        for (character, esc_code) in letters_lower.zip(esc_codes) {
            assert_eq!(
                to_esc_str(
                    &Keystroke::parse(&format!("ctrl-alt-{}", character)).unwrap(),
                    TermMode::empty(),
                    true
                )
                .unwrap(),
                format!("\x1b{}", esc_code)
            );
        }
    }

    #[test]
    fn test_shift_enter_newline() {
        let shift_enter = Keystroke::parse("shift-enter").unwrap();
        let regular_enter = Keystroke::parse("enter").unwrap();
        let mode = TermMode::empty();

        // Shift-enter should send line feed (newline)
        assert_eq!(to_esc_str(&shift_enter, mode, false), Some("\x0a".into()));

        // Regular enter should still send carriage return
        assert_eq!(to_esc_str(&regular_enter, mode, false), Some("\x0d".into()));
    }

    #[test]
    fn test_modifier_code_calc() {
        //   Code     Modifiers
        // ---------+---------------------------
        //    2     | Shift
        //    3     | Alt
        //    4     | Shift + Alt
        //    5     | Control
        //    6     | Shift + Control
        //    7     | Alt + Control
        //    8     | Shift + Alt + Control
        // ---------+---------------------------
        // from: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h2-PC-Style-Function-Keys
        assert_eq!(2, modifier_code(&Keystroke::parse("shift-a").unwrap()));
        assert_eq!(3, modifier_code(&Keystroke::parse("alt-a").unwrap()));
        assert_eq!(4, modifier_code(&Keystroke::parse("shift-alt-a").unwrap()));
        assert_eq!(5, modifier_code(&Keystroke::parse("ctrl-a").unwrap()));
        assert_eq!(6, modifier_code(&Keystroke::parse("shift-ctrl-a").unwrap()));
        assert_eq!(7, modifier_code(&Keystroke::parse("alt-ctrl-a").unwrap()));
        assert_eq!(
            8,
            modifier_code(&Keystroke::parse("shift-ctrl-alt-a").unwrap())
        );
    }

    #[test]
    fn test_extra_special_keys() {
        let none = TermMode::empty();
        assert_eq!(
            to_esc_str(&Keystroke::parse("alt-enter").unwrap(), none, false),
            Some("\x1b\x0d".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-tab").unwrap(), none, false),
            Some("\x1b[Z".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("ctrl-backspace").unwrap(), none, false),
            Some("\x08".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("alt-backspace").unwrap(), none, false),
            Some("\x1b\x7f".into())
        );
        assert_eq!(
            to_esc_str(&Keystroke::parse("shift-backspace").unwrap(), none, false),
            Some("\x7f".into())
        );
    }

    fn kitty(keys: &str, mode: TermMode, kind: KeyEventKind) -> Option<String> {
        to_kitty_str(&Keystroke::parse(keys).unwrap(), mode, kind)
    }

    #[test]
    fn kitty_disambiguate_encodes_ambiguous_keys_only() {
        let mode = TermMode::DISAMBIGUATE_ESC_CODES;
        let press = KeyEventKind::Press;
        assert_eq!(kitty("escape", mode, press), Some("\x1b[27u".into()));
        // ctrl-i and tab are told apart
        assert_eq!(kitty("ctrl-i", mode, press), Some("\x1b[105;5u".into()));
        assert_eq!(kitty("tab", mode, press), None);
        assert_eq!(
            kitty("ctrl-shift-a", mode, press),
            Some("\x1b[97;6u".into())
        );
        assert_eq!(kitty("alt-x", mode, press), Some("\x1b[120;3u".into()));
        assert_eq!(kitty("ctrl-enter", mode, press), Some("\x1b[13;5u".into()));
        assert_eq!(kitty("shift-tab", mode, press), Some("\x1b[9;2u".into()));
        // text keeps going through text input, shift only picks the character
        assert_eq!(kitty("a", mode, press), None);
        assert_eq!(kitty("shift-a", mode, press), None);
        assert_eq!(kitty("enter", mode, press), None);
        // other keys keep their legacy form
        assert_eq!(kitty("up", mode, press), None);
        assert_eq!(kitty("ctrl-up", mode, press), None);
        // repeats are presses, releases are not sent
        assert_eq!(
            kitty("ctrl-a", mode, KeyEventKind::Repeat),
            Some("\x1b[97;5u".into())
        );
        assert_eq!(kitty("ctrl-a", mode, KeyEventKind::Release), None);
        assert_eq!(kitty("ctrl-a", TermMode::empty(), press), None);
    }

    #[test]
    fn kitty_event_types_and_all_keys() {
        let events = TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_EVENT_TYPES;
        assert_eq!(
            kitty("ctrl-a", events, KeyEventKind::Release),
            Some("\x1b[97;5:3u".into())
        );
        assert_eq!(
            kitty("ctrl-a", events, KeyEventKind::Repeat),
            Some("\x1b[97;5:2u".into())
        );
        assert_eq!(
            kitty("up", events, KeyEventKind::Press),
            Some("\x1b[A".into())
        );
        assert_eq!(
            kitty("up", events, KeyEventKind::Release),
            Some("\x1b[1;1:3A".into())
        );
        assert_eq!(
            kitty("ctrl-f5", events, KeyEventKind::Press),
            Some("\x1b[15;5~".into())
        );
        assert_eq!(
            kitty("f3", events, KeyEventKind::Press),
            Some("\x1b[13~".into())
        );
        // a text key never sent as an escape code has no release either
        assert_eq!(kitty("a", events, KeyEventKind::Release), None);

        let all = TermMode::REPORT_ALL_KEYS_AS_ESC;
        assert_eq!(
            kitty("a", all, KeyEventKind::Press),
            Some("\x1b[97u".into())
        );
        assert_eq!(
            kitty("enter", all, KeyEventKind::Press),
            Some("\x1b[13u".into())
        );
        assert_eq!(
            kitty("f13", all, KeyEventKind::Press),
            Some("\x1b[57376u".into())
        );
        let mut shift_a = Keystroke::parse("shift-a").unwrap();
        shift_a.key_char = Some("A".into());
        assert_eq!(
            to_kitty_str(
                &shift_a,
                all | TermMode::REPORT_ALTERNATE_KEYS,
                KeyEventKind::Press
            ),
            Some("\x1b[97:65;2u".into())
        );
        assert_eq!(
            to_kitty_str(
                &shift_a,
                all | TermMode::REPORT_ASSOCIATED_TEXT,
                KeyEventKind::Press
            ),
            Some("\x1b[97;2;65u".into())
        );
    }
}
