//! mouse reports for programs that ask for the mouse, encoded as in xterm's ctlseqs "mouse tracking"

use alacritty_terminal::term::TermMode;
use gpui::Modifiers;

use super::Terminal;

// x10 puts each coordinate in one byte offset by 32, so 223 is the last one it can send
const X10_MAX_COORD: usize = 255 - 32;
// utf-8 mode puts the offset coordinate in one character of at most two bytes
const UTF8_MAX_COORD: usize = 0x7ff - 32;

/// button of a report, `None` is motion with no button held
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
    None,
}

impl MouseButton {
    /// report button for a gpui button, none for buttons programs can't be told about
    pub fn from_gpui(button: gpui::MouseButton) -> Option<Self> {
        match button {
            gpui::MouseButton::Left => Some(Self::Left),
            gpui::MouseButton::Middle => Some(Self::Middle),
            gpui::MouseButton::Right => Some(Self::Right),
            gpui::MouseButton::Navigate(_) => None,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
            Self::None => 3,
            Self::WheelUp => 64,
            Self::WheelDown => 65,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// bytes telling the program about a mouse event at a 0 based visible `(column, line)`,
/// none when the modes in effect don't report it or the cell is past what the encoding can hold
pub fn mouse_report(
    mode: TermMode,
    (column, line): (usize, usize),
    button: MouseButton,
    action: MouseAction,
    modifiers: &Modifiers,
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let reported = match action {
        // 1002 adds motion while a button is held, 1003 all motion
        MouseAction::Motion => {
            mode.contains(TermMode::MOUSE_MOTION)
                || (button != MouseButton::None && mode.contains(TermMode::MOUSE_DRAG))
        }
        // the wheel has no release, and a press needs a button
        MouseAction::Release => !matches!(
            button,
            MouseButton::WheelUp | MouseButton::WheelDown | MouseButton::None
        ),
        MouseAction::Press => button != MouseButton::None,
    };
    if !reported {
        return None;
    }
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let mut code = button.code();
    // only sgr tells which button went up, the older encodings send 3 for any release
    if action == MouseAction::Release && !sgr {
        code = 3;
    }
    if action == MouseAction::Motion {
        code += 32;
    }
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt || modifiers.platform {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    let (x, y) = (column + 1, line + 1);
    if sgr {
        let end = if action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes());
    }
    let mut bytes = b"\x1b[M".to_vec();
    bytes.push(32 + code);
    if mode.contains(TermMode::UTF8_MOUSE) {
        if x > UTF8_MAX_COORD || y > UTF8_MAX_COORD {
            return None;
        }
        for value in [x, y] {
            let ch = char::from_u32(32 + value as u32)?;
            bytes.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
        }
    } else {
        if x > X10_MAX_COORD || y > X10_MAX_COORD {
            return None;
        }
        bytes.extend([32 + x as u8, 32 + y as u8]);
    }
    Some(bytes)
}

/// arrow keys for wheel `lines` on the alternate screen, positive is up, so programs like
/// less scroll with the wheel. none when the program did not keep alternate scroll on
pub fn alternate_scroll(mode: TermMode, lines: i32) -> Option<Vec<u8>> {
    if !mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) || lines == 0 {
        return None;
    }
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let key: &[u8] = match (lines > 0, app_cursor) {
        (true, false) => b"\x1b[A",
        (true, true) => b"\x1bOA",
        (false, false) => b"\x1b[B",
        (false, true) => b"\x1bOB",
    };
    Some(key.repeat(lines.unsigned_abs() as usize))
}

impl Terminal {
    /// true when the program asked for the mouse, shift keeps it for local selection
    pub fn owns_mouse(&self, modifiers: &Modifiers) -> bool {
        !modifiers.shift && self.last_content.mode.intersects(TermMode::MOUSE_MODE)
    }

    /// tell the program about a mouse event at a visible cell, if its modes report it
    pub fn report_mouse(
        &mut self,
        cell: (usize, usize),
        button: MouseButton,
        action: MouseAction,
        modifiers: &Modifiers,
    ) {
        if let Some(bytes) = mouse_report(self.last_content.mode, cell, button, action, modifiers) {
            self.write_to_pty(bytes);
        }
    }

    /// send the wheel to a program on the alternate screen as arrow keys, false when not wanted
    pub fn alternate_scroll(&mut self, lines: i32) -> bool {
        match alternate_scroll(self.last_content.mode, lines) {
            Some(keys) => {
                self.write_to_pty(keys);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {

    use alacritty_terminal::term::TermMode;
    use gpui::Modifiers;

    use super::{MouseAction, MouseButton, alternate_scroll, mouse_report};

    const M1000: TermMode = TermMode::MOUSE_REPORT_CLICK;
    const M1002: TermMode = TermMode::MOUSE_DRAG;
    const M1003: TermMode = TermMode::MOUSE_MOTION;
    const SGR: TermMode = TermMode::SGR_MOUSE;
    const UTF8: TermMode = TermMode::UTF8_MOUSE;

    fn plain() -> Modifiers {
        Modifiers::default()
    }

    fn rep(
        mode: TermMode,
        cell: (usize, usize),
        button: MouseButton,
        action: MouseAction,
    ) -> Option<Vec<u8>> {
        mouse_report(mode, cell, button, action, &plain())
    }

    fn sgr_str(
        mode: TermMode,
        cell: (usize, usize),
        button: MouseButton,
        action: MouseAction,
    ) -> Option<String> {
        rep(mode | SGR, cell, button, action).map(|b| String::from_utf8(b).unwrap())
    }

    /// x10 style report: ESC [ M, then 32 + code, 32 + column 1 based, 32 + line 1 based
    fn x10(code: u8, column: usize, line: usize) -> Vec<u8> {
        vec![
            0x1b,
            b'[',
            b'M',
            32 + code,
            33 + column as u8,
            33 + line as u8,
        ]
    }

    #[test]
    fn no_tracking_mode_reports_nothing() {
        for mode in [
            TermMode::empty(),
            SGR,
            UTF8,
            SGR | UTF8,
            TermMode::ALT_SCREEN,
        ] {
            for button in [MouseButton::Left, MouseButton::Right, MouseButton::WheelUp] {
                for action in [
                    MouseAction::Press,
                    MouseAction::Release,
                    MouseAction::Motion,
                ] {
                    assert_eq!(
                        rep(mode, (1, 1), button, action),
                        None,
                        "{mode:?} {button:?} {action:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn x10_is_default_encoding_for_1000() {
        assert_eq!(
            rep(M1000, (0, 0), MouseButton::Left, MouseAction::Press).unwrap(),
            x10(0, 0, 0)
        );
        assert_eq!(
            rep(M1000, (9, 4), MouseButton::Middle, MouseAction::Press).unwrap(),
            x10(1, 9, 4)
        );
        assert_eq!(
            rep(M1000, (9, 4), MouseButton::Right, MouseAction::Press).unwrap(),
            x10(2, 9, 4)
        );
    }

    #[test]
    fn x10_release_is_button_3() {
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            assert_eq!(
                rep(M1000, (2, 3), button, MouseAction::Release).unwrap(),
                x10(3, 2, 3)
            );
        }
    }

    #[test]
    fn mode_1000_reports_no_motion() {
        for button in [MouseButton::Left, MouseButton::None] {
            assert_eq!(rep(M1000, (2, 3), button, MouseAction::Motion), None);
            assert_eq!(rep(M1000 | SGR, (2, 3), button, MouseAction::Motion), None);
        }
    }

    #[test]
    fn mode_1002_reports_drag_only() {
        assert_eq!(
            sgr_str(M1002, (7, 1), MouseButton::Left, MouseAction::Motion).as_deref(),
            Some("\x1b[<32;8;2M")
        );
        assert_eq!(
            sgr_str(M1002, (7, 1), MouseButton::Middle, MouseAction::Motion).as_deref(),
            Some("\x1b[<33;8;2M")
        );
        assert_eq!(
            sgr_str(M1002, (7, 1), MouseButton::None, MouseAction::Motion),
            None
        );
        assert_eq!(
            rep(M1002, (7, 1), MouseButton::Left, MouseAction::Motion).unwrap(),
            x10(32, 7, 1)
        );
    }

    #[test]
    fn mode_1002_still_reports_clicks() {
        assert_eq!(
            sgr_str(M1002, (0, 0), MouseButton::Left, MouseAction::Press).as_deref(),
            Some("\x1b[<0;1;1M")
        );
        assert_eq!(
            sgr_str(M1002, (0, 0), MouseButton::Left, MouseAction::Release).as_deref(),
            Some("\x1b[<0;1;1m")
        );
    }

    #[test]
    fn mode_1003_reports_all_motion() {
        assert_eq!(
            sgr_str(M1003, (3, 5), MouseButton::None, MouseAction::Motion).as_deref(),
            Some("\x1b[<35;4;6M")
        );
        assert_eq!(
            sgr_str(M1003, (3, 5), MouseButton::Right, MouseAction::Motion).as_deref(),
            Some("\x1b[<34;4;6M")
        );
        assert_eq!(
            rep(M1003, (3, 5), MouseButton::None, MouseAction::Motion).unwrap(),
            x10(35, 3, 5)
        );
        assert!(rep(M1003, (3, 5), MouseButton::Left, MouseAction::Press).is_some());
    }

    #[test]
    fn press_or_release_without_button_reports_nothing() {
        for mode in [M1000, M1002, M1003] {
            assert_eq!(
                rep(mode | SGR, (0, 0), MouseButton::None, MouseAction::Press),
                None
            );
            assert_eq!(
                rep(mode | SGR, (0, 0), MouseButton::None, MouseAction::Release),
                None
            );
        }
    }

    #[test]
    fn sgr_press_uppercase_release_lowercase_keeps_button() {
        for (button, code) in [
            (MouseButton::Left, 0),
            (MouseButton::Middle, 1),
            (MouseButton::Right, 2),
        ] {
            assert_eq!(
                sgr_str(M1000, (10, 20), button, MouseAction::Press),
                Some(format!("\x1b[<{code};11;21M"))
            );
            assert_eq!(
                sgr_str(M1000, (10, 20), button, MouseAction::Release),
                Some(format!("\x1b[<{code};11;21m"))
            );
        }
    }

    #[test]
    fn sgr_takes_precedence_over_utf8() {
        assert_eq!(
            rep(
                M1000 | SGR | UTF8,
                (300, 2),
                MouseButton::Left,
                MouseAction::Press
            )
            .unwrap(),
            b"\x1b[<0;301;3M"
        );
    }

    #[test]
    fn sgr_coordinates_are_unbounded() {
        assert_eq!(
            sgr_str(M1000, (9999, 4999), MouseButton::Left, MouseAction::Press).as_deref(),
            Some("\x1b[<0;10000;5000M")
        );
    }

    #[test]
    fn x10_largest_coordinate_is_223() {
        assert_eq!(
            rep(M1000, (222, 0), MouseButton::Left, MouseAction::Press).unwrap(),
            vec![0x1b, b'[', b'M', 32, 255, 33]
        );
        assert_eq!(
            rep(M1000, (223, 0), MouseButton::Left, MouseAction::Press),
            None
        );
        assert_eq!(
            rep(M1000, (0, 223), MouseButton::Left, MouseAction::Press),
            None
        );
        assert_eq!(
            rep(M1000, (1000, 1000), MouseButton::Left, MouseAction::Press),
            None
        );
    }

    #[test]
    fn utf8_small_coordinates_match_x10() {
        assert_eq!(
            rep(M1000 | UTF8, (5, 6), MouseButton::Left, MouseAction::Press).unwrap(),
            x10(0, 5, 6)
        );
    }

    #[test]
    fn utf8_encodes_large_coordinates_as_characters() {
        let got = rep(
            M1000 | UTF8,
            (499, 249),
            MouseButton::Right,
            MouseAction::Press,
        )
        .unwrap();
        let mut want = b"\x1b[M\x22".to_vec();
        want.extend(char::from_u32(32 + 500).unwrap().to_string().as_bytes());
        want.extend(char::from_u32(32 + 250).unwrap().to_string().as_bytes());
        assert_eq!(got, want);
        // x10 could not send this cell at all
        assert_eq!(
            rep(M1000, (499, 249), MouseButton::Right, MouseAction::Press),
            None
        );
    }

    #[test]
    fn utf8_has_a_two_byte_limit() {
        // 0x7ff is the last two byte character, the coordinate adds 32
        assert!(
            rep(
                M1000 | UTF8,
                (0x7ff - 33, 0),
                MouseButton::Left,
                MouseAction::Press
            )
            .is_some()
        );
        assert_eq!(
            rep(
                M1000 | UTF8,
                (0x7ff - 32, 0),
                MouseButton::Left,
                MouseAction::Press
            ),
            None
        );
    }

    #[test]
    fn wheel_is_64_and_65() {
        assert_eq!(
            sgr_str(M1000, (1, 2), MouseButton::WheelUp, MouseAction::Press).as_deref(),
            Some("\x1b[<64;2;3M")
        );
        assert_eq!(
            sgr_str(M1000, (1, 2), MouseButton::WheelDown, MouseAction::Press).as_deref(),
            Some("\x1b[<65;2;3M")
        );
        assert_eq!(
            rep(M1000, (1, 2), MouseButton::WheelUp, MouseAction::Press).unwrap(),
            x10(64, 1, 2)
        );
        assert_eq!(
            rep(M1000, (1, 2), MouseButton::WheelDown, MouseAction::Press).unwrap(),
            x10(65, 1, 2)
        );
    }

    #[test]
    fn wheel_has_no_release() {
        for mode in [M1000, M1000 | SGR, M1000 | UTF8, M1003 | SGR] {
            for button in [MouseButton::WheelUp, MouseButton::WheelDown] {
                assert_eq!(rep(mode, (0, 0), button, MouseAction::Release), None);
            }
        }
    }

    #[test]
    fn modifiers_add_shift_4_alt_8_ctrl_16() {
        let m = |shift, alt, control| Modifiers {
            shift,
            alt,
            control,
            ..Default::default()
        };
        let cases = [
            (m(true, false, false), 4),
            (m(false, true, false), 8),
            (m(false, false, true), 16),
            (m(true, true, false), 12),
            (m(false, true, true), 24),
            (m(true, true, true), 28),
        ];
        for (mods, add) in cases {
            let got = mouse_report(
                M1000 | SGR,
                (0, 0),
                MouseButton::Right,
                MouseAction::Press,
                &mods,
            );
            assert_eq!(
                got,
                Some(format!("\x1b[<{};1;1M", 2 + add).into_bytes()),
                "{mods:?}"
            );
            let wheel = mouse_report(
                M1000 | SGR,
                (0, 0),
                MouseButton::WheelUp,
                MouseAction::Press,
                &mods,
            );
            assert_eq!(
                wheel,
                Some(format!("\x1b[<{};1;1M", 64 + add).into_bytes()),
                "{mods:?}"
            );
            let x = mouse_report(M1000, (0, 0), MouseButton::Left, MouseAction::Press, &mods);
            assert_eq!(x, Some(x10(add, 0, 0)), "{mods:?}");
        }
    }

    #[test]
    fn modifiers_combine_with_motion() {
        let ctrl = Modifiers::control();
        let got = mouse_report(
            M1003 | SGR,
            (0, 0),
            MouseButton::None,
            MouseAction::Motion,
            &ctrl,
        );
        assert_eq!(got, Some(b"\x1b[<51;1;1M".to_vec()));
    }

    #[test]
    fn modifiers_on_release_keep_sgr_button() {
        let shift = Modifiers::shift();
        let got = mouse_report(
            M1000 | SGR,
            (0, 0),
            MouseButton::Middle,
            MouseAction::Release,
            &shift,
        );
        assert_eq!(got, Some(b"\x1b[<5;1;1m".to_vec()));
    }

    const ALT: TermMode = TermMode::ALT_SCREEN.union(TermMode::ALTERNATE_SCROLL);

    #[test]
    fn alternate_scroll_up_sends_up_arrows() {
        assert_eq!(alternate_scroll(ALT, 1).unwrap(), b"\x1b[A");
        assert_eq!(alternate_scroll(ALT, 4).unwrap(), b"\x1b[A".repeat(4));
    }

    #[test]
    fn alternate_scroll_down_sends_down_arrows() {
        assert_eq!(alternate_scroll(ALT, -1).unwrap(), b"\x1b[B");
        assert_eq!(alternate_scroll(ALT, -3).unwrap(), b"\x1b[B".repeat(3));
    }

    #[test]
    fn alternate_scroll_uses_app_cursor_arrows() {
        let app = ALT | TermMode::APP_CURSOR;
        assert_eq!(alternate_scroll(app, 2).unwrap(), b"\x1bOA\x1bOA");
        assert_eq!(alternate_scroll(app, -2).unwrap(), b"\x1bOB\x1bOB");
    }

    #[test]
    fn alternate_scroll_off_on_primary_screen() {
        assert_eq!(alternate_scroll(TermMode::ALTERNATE_SCROLL, 3), None);
        assert_eq!(alternate_scroll(TermMode::empty(), 3), None);
    }

    #[test]
    fn alternate_scroll_off_when_program_disabled_it() {
        assert_eq!(alternate_scroll(TermMode::ALT_SCREEN, 3), None);
        assert_eq!(
            alternate_scroll(TermMode::ALT_SCREEN | TermMode::APP_CURSOR, -3),
            None
        );
    }

    #[test]
    fn alternate_scroll_zero_lines_sends_nothing() {
        assert_eq!(alternate_scroll(ALT, 0), None);
    }

    #[test]
    fn back_and_forward_buttons_are_not_reported() {
        // the terminal mouse protocol has no code for them
        for nav in [
            gpui::NavigationDirection::Back,
            gpui::NavigationDirection::Forward,
        ] {
            assert_eq!(
                MouseButton::from_gpui(gpui::MouseButton::Navigate(nav)),
                None
            );
        }
        assert_eq!(
            MouseButton::from_gpui(gpui::MouseButton::Middle),
            Some(MouseButton::Middle)
        );
    }
}
