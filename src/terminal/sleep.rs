//! tab sleeping: the history of a tab hidden for long is packed into a compressed buffer and
//! taken out of the grid, and put back above whatever came since when the tab is shown. the
//! shell keeps running, only history moves

use std::{
    io::{Read, Write},
    sync::atomic::Ordering,
};

use alacritty_terminal::{
    grid::{Dimensions, Row},
    index::{Column, Line},
    term::cell::{Cell, Flags, Hyperlink},
    vte::ansi::{Color, NamedColor, Rgb},
};

use super::Terminal;

// named colors in a fixed order, a color is stored as its place here
const NAMED: [NamedColor; 29] = [
    NamedColor::Black,
    NamedColor::Red,
    NamedColor::Green,
    NamedColor::Yellow,
    NamedColor::Blue,
    NamedColor::Magenta,
    NamedColor::Cyan,
    NamedColor::White,
    NamedColor::BrightBlack,
    NamedColor::BrightRed,
    NamedColor::BrightGreen,
    NamedColor::BrightYellow,
    NamedColor::BrightBlue,
    NamedColor::BrightMagenta,
    NamedColor::BrightCyan,
    NamedColor::BrightWhite,
    NamedColor::Foreground,
    NamedColor::Background,
    NamedColor::Cursor,
    NamedColor::DimBlack,
    NamedColor::DimRed,
    NamedColor::DimGreen,
    NamedColor::DimYellow,
    NamedColor::DimBlue,
    NamedColor::DimMagenta,
    NamedColor::DimCyan,
    NamedColor::DimWhite,
    NamedColor::BrightForeground,
    NamedColor::DimForeground,
];

// which parts of a cell differ from an empty one
const HAS_FG: u8 = 1;
const HAS_BG: u8 = 2;
const HAS_FLAGS: u8 = 4;
const HAS_ZEROWIDTH: u8 = 8;
const HAS_UNDERLINE_COLOR: u8 = 16;
const HAS_HYPERLINK: u8 = 32;

/// history taken out of a sleeping tab
pub(super) struct Slept {
    /// deflated rows, oldest first
    data: Vec<u8>,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_str(out: &mut Vec<u8>, text: &str) {
    put_varint(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn put_color(out: &mut Vec<u8>, color: Color) {
    match color {
        Color::Named(named) => {
            out.push(0);
            out.push(NAMED.iter().position(|n| *n == named).unwrap_or(16) as u8);
        }
        Color::Spec(rgb) => out.extend_from_slice(&[1, rgb.r, rgb.g, rgb.b]),
        Color::Indexed(index) => out.extend_from_slice(&[2, index]),
    }
}

/// reads what the `put_` functions wrote
struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (first, rest) = self.bytes.split_first()?;
        self.bytes = rest;
        Some(*first)
    }

    fn varint(&mut self) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    fn str(&mut self) -> Option<String> {
        let len = self.varint()? as usize;
        let (text, rest) = (self.bytes.get(..len)?, self.bytes.get(len..)?);
        self.bytes = rest;
        String::from_utf8(text.to_vec()).ok()
    }

    fn color(&mut self) -> Option<Color> {
        Some(match self.byte()? {
            0 => Color::Named(*NAMED.get(self.byte()? as usize)?),
            1 => Color::Spec(Rgb {
                r: self.byte()?,
                g: self.byte()?,
                b: self.byte()?,
            }),
            2 => Color::Indexed(self.byte()?),
            _ => return None,
        })
    }
}

fn put_cell(out: &mut Vec<u8>, cell: &Cell) {
    let empty = Cell::default();
    let zerowidth = cell.zerowidth().filter(|chars| !chars.is_empty());
    let underline = cell.underline_color();
    let link = cell.hyperlink();
    let mut has = 0;
    has |= if cell.fg != empty.fg { HAS_FG } else { 0 };
    has |= if cell.bg != empty.bg { HAS_BG } else { 0 };
    has |= if !cell.flags.is_empty() { HAS_FLAGS } else { 0 };
    has |= if zerowidth.is_some() {
        HAS_ZEROWIDTH
    } else {
        0
    };
    has |= if underline.is_some() {
        HAS_UNDERLINE_COLOR
    } else {
        0
    };
    has |= if link.is_some() { HAS_HYPERLINK } else { 0 };
    put_varint(out, u64::from(cell.c));
    out.push(has);
    if has & HAS_FG != 0 {
        put_color(out, cell.fg);
    }
    if has & HAS_BG != 0 {
        put_color(out, cell.bg);
    }
    if has & HAS_FLAGS != 0 {
        out.extend_from_slice(&cell.flags.bits().to_le_bytes());
    }
    if let Some(chars) = zerowidth {
        put_str(out, &chars.iter().collect::<String>());
    }
    if let Some(color) = underline {
        put_color(out, color);
    }
    if let Some(link) = link {
        put_str(out, link.id());
        put_str(out, link.uri());
    }
}

fn read_cell(reader: &mut Reader) -> Option<Cell> {
    let mut cell = Cell {
        c: char::from_u32(reader.varint()? as u32)?,
        ..Cell::default()
    };
    let has = reader.byte()?;
    if has & HAS_FG != 0 {
        cell.fg = reader.color()?;
    }
    if has & HAS_BG != 0 {
        cell.bg = reader.color()?;
    }
    if has & HAS_FLAGS != 0 {
        let bits = u16::from_le_bytes([reader.byte()?, reader.byte()?]);
        cell.flags = Flags::from_bits_retain(bits);
    }
    if has & HAS_ZEROWIDTH != 0 {
        for c in reader.str()?.chars() {
            cell.push_zerowidth(c);
        }
    }
    if has & HAS_UNDERLINE_COLOR != 0 {
        cell.set_underline_color(Some(reader.color()?));
    }
    if has & HAS_HYPERLINK != 0 {
        let id = reader.str()?;
        let uri = reader.str()?;
        cell.set_hyperlink(Some(Hyperlink::new(Some(id), uri)));
    }
    Some(cell)
}

/// rows packed small: each row its width, the count of cells up to the last non empty one,
/// then those cells
pub(super) fn encode_rows(rows: &[Row<Cell>]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varint(&mut out, rows.len() as u64);
    let empty = Cell::default();
    for row in rows {
        let cells: Vec<&Cell> = row.into_iter().collect();
        let used = cells
            .iter()
            .rposition(|cell| **cell != empty)
            .map_or(0, |last| last + 1);
        put_varint(&mut out, cells.len() as u64);
        put_varint(&mut out, used as u64);
        for cell in &cells[..used] {
            put_cell(&mut out, cell);
        }
    }
    out
}

/// rows `encode_rows` packed, each made `columns` wide, none when the bytes are broken
pub(super) fn decode_rows(bytes: &[u8], columns: usize) -> Option<Vec<Row<Cell>>> {
    let mut reader = Reader { bytes };
    let count = reader.varint()? as usize;
    let mut rows = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let _width = reader.varint()?;
        let used = reader.varint()? as usize;
        let mut row = Row::new(columns.max(1));
        for column in 0..used {
            let cell = read_cell(&mut reader)?;
            // a narrower grid now cuts what no longer fits
            if column < columns {
                row[Column(column)] = cell;
            }
        }
        rows.push(row);
    }
    Some(rows)
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(bytes).ok();
    encoder.finish().unwrap_or_default()
}

fn inflate(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::DeflateDecoder::new(bytes)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

impl Terminal {
    /// true while the history of this tab is packed away
    pub fn asleep(&self) -> bool {
        self.slept.is_some()
    }

    /// pack the history away, keeping the screen. false when there is none or it sleeps
    pub fn sleep(&mut self) -> bool {
        if self.slept.is_some() || self.frozen.is_some() {
            return false;
        }
        let mut term = self.term.lock();
        let history = term.grid().history_size();
        if history == 0 {
            return false;
        }
        let rows: Vec<Row<Cell>> = (1..=history)
            .rev()
            .map(|back| term.grid()[Line(-(back as i32))].clone())
            .collect();
        term.grid_mut().clear_history();
        drop(term);
        self.slept = Some(Slept {
            data: deflate(&encode_rows(&rows)),
        });
        // a search would point at lines that are gone
        self.search = None;
        true
    }

    /// put the packed history back above what came since, a no-op while awake
    pub fn wake(&mut self) {
        let Some(slept) = self.slept.take() else {
            return;
        };
        let mut term = self.term.lock();
        let columns = term.columns();
        let screen = term.screen_lines();
        let Some(mut stored) = inflate(&slept.data).and_then(|bytes| decode_rows(&bytes, columns))
        else {
            eprintln!("the history of a sleeping tab could not be read back");
            return;
        };
        let grid = term.grid_mut();
        let since = grid.history_size();
        stored.extend(
            (1..=since)
                .rev()
                .map(|back| grid[Line(-(back as i32))].clone()),
        );
        let shown: Vec<Row<Cell>> = (0..screen)
            .map(|line| grid[Line(line as i32)].clone())
            .collect();
        grid.clear_history();
        // each row goes to the top line and is scrolled up into history, the oldest first.
        // the history limit drops the oldest ones on its own
        let region = Line(0)..Line(screen as i32);
        for row in stored {
            grid[Line(0)] = row;
            grid.scroll_up(&region, 1);
        }
        for (line, row) in shown.into_iter().enumerate() {
            grid[Line(line as i32)] = row;
        }
        drop(term);
        self.dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::{
        grid::Row,
        index::Column,
        term::cell::{Cell, Flags, Hyperlink},
        vte::ansi::{Color, NamedColor, Rgb},
    };

    use super::{decode_rows, deflate, encode_rows, inflate};
    use crate::{
        settings::{Shell, TerminalSettings},
        terminal::{
            InputSource,
            tests::{profile, screen_text, spawn_with, wait_for_text},
        },
    };

    #[test]
    fn cells_round_trip_with_colors_marks_and_hyperlinks() {
        let mut row: Row<Cell> = Row::new(10);
        row[Column(0)].c = 'a';
        row[Column(1)] = Cell {
            c: 'ü',
            fg: Color::Spec(Rgb { r: 1, g: 2, b: 3 }),
            bg: Color::Indexed(200),
            flags: Flags::BOLD | Flags::WRAPLINE,
            extra: None,
        };
        row[Column(2)].c = 'e';
        row[Column(2)].push_zerowidth('\u{301}');
        row[Column(2)].set_underline_color(Some(Color::Named(NamedColor::DimRed)));
        row[Column(3)].c = '$';
        row[Column(3)].set_hyperlink(Some(Hyperlink::new(
            Some("kuterm-prompt-4"),
            "kuterm:".into(),
        )));
        let empty: Row<Cell> = Row::new(10);
        let rows = vec![row.clone(), empty.clone()];
        let packed = deflate(&encode_rows(&rows));
        let back = decode_rows(&inflate(&packed).unwrap(), 10).unwrap();
        assert_eq!(back, rows);
        assert_eq!(
            back[0][Column(3)].hyperlink().unwrap().id(),
            "kuterm-prompt-4"
        );
        // a narrower grid cuts, a wider one pads
        let narrow = decode_rows(&encode_rows(&rows), 2).unwrap();
        assert_eq!(narrow[0][Column(1)].c, 'ü');
        let wide = decode_rows(&encode_rows(&rows), 20).unwrap();
        assert_eq!(wide[0][Column(19)], Cell::default());
        assert_eq!(decode_rows(&[5, 1], 10), None);
    }

    #[test]
    fn sleeping_moves_history_out_and_back_above_new_output() {
        let script = "read _; seq 1 300; echo first_done; read _; echo after_sleep; sleep 5";
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        terminal.input(InputSource::Keyboard, b"\r".to_vec());
        wait_for_text(terminal, "first_done");
        let history = terminal.history_size();
        assert!(history > 200);
        assert!(terminal.sleep());
        assert!(terminal.asleep());
        assert_eq!(terminal.history_size(), 0);
        assert!(!terminal.sleep());
        // output while asleep scrolls into the empty history
        terminal.input(InputSource::Keyboard, b"\r".to_vec());
        // sync would wake it, so the grid is read directly
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !terminal
            .visible_screen_text(false)
            .0
            .contains("after_sleep")
        {
            assert!(
                std::time::Instant::now() < deadline,
                "no output while asleep"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(terminal.asleep());
        terminal.wake();
        assert!(!terminal.asleep());
        let (text, _) = terminal.visible_screen_text(true);
        let lines: Vec<&str> = text.lines().collect();
        // the echo of the first enter comes before the numbers
        assert_eq!(lines.iter().find(|line| !line.is_empty()), Some(&"1"));
        let at = |needle: &str| lines.iter().position(|line| *line == needle).unwrap();
        assert!(at("300") < at("first_done") && at("first_done") < at("after_sleep"));
        assert!(terminal.history_size() >= history);
        terminal.sync();
        assert!(screen_text(terminal).contains("after_sleep"));
    }
}
