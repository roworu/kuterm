//! `file:line` in output: the path and line it names, resolved against the folder of the
//! command that printed it, and the lines around it for a peek

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point as AlacPoint},
    term::search::Match,
};
use gpui::{Pixels, Point};

use super::{Terminal, decorations::PopoverLine};

// files bigger than this are never read for a peek
const PEEK_MAX_BYTES: u64 = 4 << 20;

/// a file and the line, maybe column, some text names in it
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileLine {
    pub path: String,
    pub line: u32,
    pub column: Option<u32>,
}

/// path and line of text like "src/main.rs:42" or "src/main.rs:42:7:", none when it names
/// no line of a file
pub fn parse_file_line(text: &str) -> Option<FileLine> {
    if text.contains("://") {
        return None;
    }
    let text = text.trim().trim_end_matches(':');
    let mut parts = text.rsplitn(3, ':');
    let last = parts.next()?;
    let middle = parts.next()?;
    let number = |part: &str| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
    };
    let (path, line, column) = match (number(middle), parts.next()) {
        (Some(line), Some(path)) => (path, line, Some(number(last)?)),
        _ => (middle_and_rest(text)?, number(last)?, None),
    };
    (!path.is_empty() && line > 0).then(|| FileLine {
        path: path.to_string(),
        line,
        column,
    })
}

// the path before the last ':'
fn middle_and_rest(text: &str) -> Option<&str> {
    Some(text.rsplit_once(':')?.0)
}

/// chars a path in output is made of
fn path_char(c: char) -> bool {
    c.is_alphanumeric() || "/._-~+@".contains(c)
}

/// the `file:line` around column `at` of `row`, with the columns it spans
pub fn file_line_in_row(row: &str, at: usize) -> Option<(FileLine, usize, usize)> {
    let chars: Vec<char> = row.chars().collect();
    if at >= chars.len() {
        return None;
    }
    let token = |c: char| path_char(c) || c == ':';
    if !token(chars[at]) {
        return None;
    }
    let mut start = at;
    while start > 0 && token(chars[start - 1]) {
        start -= 1;
    }
    let mut end = at;
    while end + 1 < chars.len() && token(chars[end + 1]) {
        end += 1;
    }
    // "error:" in front or ":" after belongs to the message, not the path
    let text: String = chars[start..=end].iter().collect();
    let lead = text.find(|c: char| c != ':').unwrap_or(0);
    let found = parse_file_line(&text[lead..])?;
    Some((found, start + lead, end))
}

/// `path` as the command that printed it saw it, from its folder
pub fn resolve(path: &str, cwd: Option<&Path>, home: Option<&Path>) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = home
    {
        return home.join(rest);
    }
    let path = Path::new(path);
    match cwd {
        Some(cwd) if path.is_relative() => cwd.join(path),
        _ => path.to_path_buf(),
    }
}

/// lines `line - around` to `line + around` of `path` with numbers, the named one
/// highlighted. none when it is no readable text file
pub fn snippet(path: &Path, line: u32, around: u32) -> Option<Vec<PopoverLine>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > PEEK_MAX_BYTES {
        return None;
    }
    let first = line.saturating_sub(around).max(1);
    let last = line + around;
    let width = last.to_string().len();
    let reader = BufReader::new(std::fs::File::open(path).ok()?);
    let mut lines = Vec::new();
    for (ix, text) in reader.lines().enumerate() {
        let number = ix as u32 + 1;
        if number > last {
            break;
        }
        // binary files are no peek
        let text = text.ok()?;
        if number >= first {
            lines.push(PopoverLine {
                text: format!("{number:>width$}  {}", text.replace('\t', "    ")),
                highlight: number == line,
            });
        }
    }
    (!lines.is_empty()).then_some(lines)
}

impl Terminal {
    /// the `file:line` under a window position, with its cells and the folder of the block
    /// that printed it
    pub fn file_line_at(
        &self,
        position: Point<Pixels>,
    ) -> Option<(FileLine, Match, Option<PathBuf>)> {
        let (point, _) = self.mouse_point(position);
        let term = self.term.lock_unfair();
        let row = &term.grid()[point.line];
        let text: String = (0..term.columns()).map(|c| row[Column(c)].c).collect();
        drop(term);
        let (found, start, end) = file_line_in_row(&text, point.column.0)?;
        let range =
            AlacPoint::new(point.line, Column(start))..=AlacPoint::new(point.line, Column(end));
        Some((found, range, self.folder_at(point.line)))
    }

    /// folder of the block whose output covers `line`, else the folder the shell reported
    pub fn folder_at(&self, line: Line) -> Option<PathBuf> {
        self.block_at(line)
            .and_then(|id| self.blocks.list.iter().find(|block| block.id == id))
            .and_then(|block| block.cwd.clone())
            .or_else(|| self.reported_cwd.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{FileLine, file_line_in_row, parse_file_line, resolve, snippet};

    fn at(path: &str, line: u32, column: Option<u32>) -> Option<FileLine> {
        Some(FileLine {
            path: path.into(),
            line,
            column,
        })
    }

    #[test]
    fn paths_with_lines_are_parsed() {
        assert_eq!(
            parse_file_line("src/main.rs:42"),
            at("src/main.rs", 42, None)
        );
        assert_eq!(
            parse_file_line("src/main.rs:42:7:"),
            at("src/main.rs", 42, Some(7))
        );
        assert_eq!(parse_file_line("C:x:3"), at("C:x", 3, None));
        assert_eq!(parse_file_line("src/main.rs"), None);
        assert_eq!(parse_file_line(":12"), None);
        assert_eq!(parse_file_line("f.rs:0"), None);
        assert_eq!(parse_file_line("http://host:8080"), None);
    }

    #[test]
    fn the_token_under_the_mouse_is_found() {
        let row = "error: src/lib.rs:10:5: mismatched types";
        let (found, start, end) = file_line_in_row(row, 9).unwrap();
        assert_eq!(found, at("src/lib.rs", 10, Some(5)).unwrap());
        assert_eq!((start, end), (7, 22));
        assert_eq!(file_line_in_row(row, 2), None);
        assert_eq!(file_line_in_row(row, 30), None);
        assert_eq!(file_line_in_row("short", 99), None);
    }

    #[test]
    fn paths_resolve_against_the_folder_of_the_output() {
        let cwd = Path::new("/work");
        let home = Path::new("/home/me");
        assert_eq!(
            resolve("a.rs", Some(cwd), Some(home)),
            Path::new("/work/a.rs")
        );
        assert_eq!(
            resolve("/etc/x", Some(cwd), Some(home)),
            Path::new("/etc/x")
        );
        assert_eq!(
            resolve("~/b", Some(cwd), Some(home)),
            Path::new("/home/me/b")
        );
        assert_eq!(resolve("a.rs", None, None), Path::new("a.rs"));
    }

    #[test]
    fn snippet_shows_lines_around_with_the_named_one_bright() {
        let dir = std::env::temp_dir().join(format!("kuterm_peek_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&file, text).unwrap();
        let lines = snippet(&file, 3, 5).unwrap();
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0].text, "1  line 1");
        assert!(lines[2].highlight && !lines[1].highlight);
        let lines = snippet(&file, 15, 2).unwrap();
        assert_eq!(lines[0].text, "13  line 13");
        assert_eq!(lines.len(), 5);
        assert_eq!(snippet(&dir, 1, 5), None);
        assert_eq!(snippet(&dir.join("missing"), 1, 5), None);
        std::fs::write(&file, [0xff, 0xfe, b'\n']).unwrap();
        assert_eq!(snippet(&file, 1, 5), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
