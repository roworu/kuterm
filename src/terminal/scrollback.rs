//! the whole history as text with its colors kept, for a pager or an editor

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line},
    term::cell::{Cell, Flags},
    vte::ansi::{Color, NamedColor},
};

use super::Terminal;

// sgr parameters of a color, `base` is 30 for text and 40 for backgrounds
fn color_sgr(color: Color, base: u8) -> String {
    match color {
        Color::Named(NamedColor::Foreground | NamedColor::Background) => (base + 9).to_string(),
        Color::Named(named) => match named as usize {
            index @ 0..=7 => (base as usize + index).to_string(),
            index @ 8..=15 => (base as usize + 60 + index - 8).to_string(),
            // dim colors have no sgr of their own, the plain ones stand in
            index @ 259..=266 => (base as usize + index - 259).to_string(),
            _ => (base + 9).to_string(),
        },
        Color::Indexed(index) => format!("{};5;{index}", base + 8),
        Color::Spec(rgb) => format!("{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b),
    }
}

const STYLES: [(Flags, &str); 6] = [
    (Flags::BOLD, "1"),
    (Flags::DIM, "2"),
    (Flags::ITALIC, "3"),
    (Flags::UNDERLINE, "4"),
    (Flags::INVERSE, "7"),
    (Flags::STRIKEOUT, "9"),
];

// what `cell_sgr` reads from a cell, so equal cells need no new sgr
fn style_key(cell: &Cell) -> (Flags, Color, Color) {
    let flags = STYLES
        .iter()
        .fold(Flags::empty(), |all, (flag, _)| all | *flag);
    (cell.flags & flags, cell.fg, cell.bg)
}

// sgr that turns a reset style into the style of `cell`
fn cell_sgr(cell: &Cell) -> String {
    let mut params = vec!["0".to_string()];
    for (flag, code) in STYLES {
        if cell.flags.contains(flag) {
            params.push(code.into());
        }
    }
    params.push(color_sgr(cell.fg, 30));
    params.push(color_sgr(cell.bg, 40));
    format!("\x1b[{}m", params.join(";"))
}

impl Terminal {
    /// history and screen as text with sgr escapes for colors and styles
    pub fn scrollback_ansi(&self) -> String {
        let term = self.term.lock();
        let mut out = String::new();
        for line in term.topmost_line().0..=term.bottommost_line().0 {
            let row = &term.grid()[Line(line)];
            // blank cells at the end of a line are left out
            let end = (0..term.columns())
                .rev()
                .find(|&column| {
                    let cell = &row[Column(column)];
                    cell.c != ' ' || !matches!(cell.bg, Color::Named(NamedColor::Background))
                })
                .map_or(0, |column| column + 1);
            let mut style = String::new();
            let mut last_key = None;
            for column in 0..end {
                let cell = &row[Column(column)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                // the whole history goes through here, so sgr is only built on style changes
                let key = style_key(cell);
                if last_key != Some(key) {
                    last_key = Some(key);
                    let sgr = cell_sgr(cell);
                    if sgr != style {
                        out.push_str(&sgr);
                        style = sgr;
                    }
                }
                out.push(cell.c);
                for &c in cell.zerowidth().unwrap_or_default() {
                    out.push(c);
                }
            }
            if !style.is_empty() {
                out.push_str("\x1b[0m");
            }
            out.push('\n');
        }
        // the empty lines under the prompt are no part of it
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::tests::{profile, spawn_with, wait_for_text},
    };

    #[test]
    fn scrollback_keeps_colors_and_history() {
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "i=0; while [ $i -lt 50 ]; do echo old_$i; i=$((i+1)); done; printf '\\033[1;31mred\\033[0m \\033[38;2;1;2;3mrgb\\033[0m\\n'; echo dump_ready; sleep 5".into(),
            ],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "dump_ready");
        let text = terminal.scrollback_ansi();
        // lines scrolled out of view are there too
        assert!(text.starts_with("\x1b[0;39;49mold_0\x1b[0m\n"), "{text:?}");
        assert!(text.contains("\x1b[0;1;31;49mred\x1b[0;39;49m \x1b[0;38;2;1;2;3;49mrgb\x1b[0m\n"));
        assert!(text.ends_with("dump_ready\x1b[0m\n"));
    }
}
