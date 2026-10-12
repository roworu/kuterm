//! kitty graphics protocol: images programs send in `APC G ... ST` sequences. they are kept in
//! a store, and each placement marks the cell it starts at, so it scrolls with the text

use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use base64::Engine;
use gpui::RenderImage;

// files and direct transmissions bigger than this are refused, a 4k rgba image fits
const MAX_IMAGE_BYTES: usize = 64 << 20;

/// hyperlink ids of cells an image placement starts at
pub(crate) const IMAGE_MARK: &str = "kuterm-image-";

/// size of one cell in pixels, kept up to date by the terminal for the pty thread
#[derive(Default)]
pub struct CellSize {
    width: AtomicU32,
    height: AtomicU32,
}

impl CellSize {
    /// remember the cell size the grid is laid out with
    pub fn set(&self, width: f32, height: f32) {
        self.width.store(width.to_bits(), Ordering::Relaxed);
        self.height.store(height.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> (f32, f32) {
        (
            f32::from_bits(self.width.load(Ordering::Relaxed)),
            f32::from_bits(self.height.load(Ordering::Relaxed)),
        )
    }
}

struct StoredImage {
    render: Arc<RenderImage>,
    width: u32,
    height: u32,
}

/// where an image is shown, in cells from the marked one
struct Placement {
    image: u32,
    cols: usize,
    rows: usize,
    /// the program picked the cells, so the image is stretched over them
    fit: bool,
}

/// an image to paint over the grid
#[derive(Clone)]
pub struct VisibleImage {
    pub image: Arc<RenderImage>,
    pub cols: usize,
    pub rows: usize,
    /// pixels it is drawn with, none stretches it over its cells
    pub pixels: Option<(u32, u32)>,
}

/// images and placements of one terminal, shared by the pty thread and the renderer
pub struct Graphics {
    pub(super) enabled: bool,
    max_memory: usize,
    images: HashMap<u32, StoredImage>,
    /// image ids, oldest first, the oldest go when memory runs out
    order: VecDeque<u32>,
    memory: usize,
    placements: HashMap<u64, Placement>,
    next_mark: u64,
    /// ids given to images sent without one, counted down from the top so they never meet
    /// the ones programs pick
    next_auto_id: u32,
    /// images no longer used, their textures are freed by the next frame
    dropped: Vec<Arc<RenderImage>>,
}

impl Graphics {
    /// empty store, `max_memory_mb` limits the decoded images it keeps
    pub fn new(enabled: bool, max_memory_mb: usize) -> Self {
        Self {
            enabled,
            max_memory: max_memory_mb << 20,
            images: HashMap::new(),
            order: VecDeque::new(),
            memory: 0,
            placements: HashMap::new(),
            next_mark: 0,
            next_auto_id: u32::MAX,
            dropped: Vec::new(),
        }
    }

    /// change the settings, dropping images that no longer fit
    pub fn configure(&mut self, enabled: bool, max_memory_mb: usize) {
        self.enabled = enabled;
        self.max_memory = max_memory_mb << 20;
        if !enabled {
            self.clear(true);
        }
        self.evict(0);
    }

    /// true when there is nothing to paint
    pub fn is_empty(&self) -> bool {
        self.placements.is_empty()
    }

    /// image of the placement marked `mark`
    pub fn placement(&self, mark: u64) -> Option<VisibleImage> {
        let placement = self.placements.get(&mark)?;
        let image = self.images.get(&placement.image)?;
        Some(VisibleImage {
            image: image.render.clone(),
            cols: placement.cols,
            rows: placement.rows,
            pixels: (!placement.fit).then_some((image.width, image.height)),
        })
    }

    /// rows of the tallest placement, how far above the view a visible image can start
    pub fn tallest(&self) -> usize {
        self.placements
            .values()
            .map(|placement| placement.rows)
            .max()
            .unwrap_or(0)
    }

    /// images dropped since the last call, their textures should be freed
    pub fn take_dropped(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.dropped)
    }

    fn store(&mut self, id: u32, image: StoredImage) {
        let bytes = image.width as usize * image.height as usize * 4;
        self.remove_image(id);
        self.evict(bytes);
        self.memory += bytes;
        self.order.push_back(id);
        self.images.insert(id, image);
    }

    // drop the oldest images until `room` more bytes fit
    fn evict(&mut self, room: usize) {
        while self.memory + room > self.max_memory
            && let Some(oldest) = self.order.front().copied()
        {
            self.remove_image(oldest);
        }
    }

    fn remove_image(&mut self, id: u32) {
        let Some(image) = self.images.remove(&id) else {
            return;
        };
        self.memory -= image.width as usize * image.height as usize * 4;
        self.order.retain(|other| *other != id);
        self.placements.retain(|_, placement| placement.image != id);
        self.dropped.push(image.render);
    }

    fn clear(&mut self, images: bool) {
        self.placements.clear();
        if images {
            for id in self.order.clone() {
                self.remove_image(id);
            }
        }
    }
}

/// shared store of one terminal
pub type SharedGraphics = Arc<Mutex<Graphics>>;

/// keys of a graphics command, like `a=T,f=100,i=1`
#[derive(Clone, Debug, Default)]
struct Control {
    action: u8,
    format: u32,
    medium: u8,
    width: u32,
    height: u32,
    id: Option<u32>,
    number: Option<u32>,
    more: bool,
    compressed: bool,
    cols: usize,
    rows: usize,
    keep_cursor: bool,
    quiet: u8,
    delete: u8,
}

fn parse_control(text: &str) -> Control {
    let mut control = Control {
        action: b't',
        format: 32,
        medium: b'd',
        delete: b'a',
        ..Control::default()
    };
    for pair in text.split(',') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let number = || value.parse::<u32>().ok();
        let letter = value.bytes().next().unwrap_or(0);
        match key {
            "a" => control.action = letter,
            "f" => control.format = number().unwrap_or(32),
            "t" => control.medium = letter,
            "s" => control.width = number().unwrap_or(0),
            "v" => control.height = number().unwrap_or(0),
            "i" => control.id = number(),
            "I" => control.number = number(),
            "m" => control.more = value == "1",
            "o" => control.compressed = !value.is_empty(),
            "c" => control.cols = number().unwrap_or(0) as usize,
            "r" => control.rows = number().unwrap_or(0) as usize,
            "C" => control.keep_cursor = value == "1",
            "q" => control.quiet = number().unwrap_or(0).min(2) as u8,
            "d" => control.delete = letter,
            _ => {}
        }
    }
    control
}

/// what a command left to do in the output stream
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// bytes for alacritty in place of the command, like the mark of a placement
    pub output: Vec<u8>,
    /// answer for the program
    pub reply: Option<Vec<u8>>,
}

/// collects chunked transmissions of one terminal, running in the pty thread
pub struct GraphicsParser {
    shared: SharedGraphics,
    cell: Arc<CellSize>,
    /// first chunk's keys and the payload so far of a transmission sent in chunks
    pending: Option<(Control, Vec<u8>)>,
}

impl GraphicsParser {
    /// parser storing into `shared`, sizing placements with `cell`
    pub fn new(shared: SharedGraphics, cell: Arc<CellSize>) -> Self {
        Self {
            shared,
            cell,
            pending: None,
        }
    }

    /// true when graphics are turned on in settings
    pub fn enabled(&self) -> bool {
        self.shared.lock().unwrap().enabled
    }

    /// run one `G` command, `body` is what follows the `G`
    pub fn command(&mut self, body: &[u8]) -> Outcome {
        let (control_text, payload) = match body.iter().position(|&b| b == b';') {
            Some(at) => (&body[..at], &body[at + 1..]),
            None => (body, &[][..]),
        };
        let control = parse_control(&String::from_utf8_lossy(control_text));
        // later chunks only carry m and q, the first chunk tells the rest
        let (control, data) = match self.pending.take() {
            Some((first, mut data)) => {
                data.extend_from_slice(payload);
                if data.len() > MAX_IMAGE_BYTES * 2 {
                    return reply(&first, Err("EFBIG:image is too big"));
                }
                if control.more {
                    self.pending = Some((first, data));
                    return Outcome::default();
                }
                (first, data)
            }
            None if control.more => {
                self.pending = Some((control, payload.to_vec()));
                return Outcome::default();
            }
            None => (control, payload.to_vec()),
        };
        match control.action {
            b't' | b'T' | b'q' => {
                let image = match load(&control, &data) {
                    Ok(image) => image,
                    Err(error) => return reply(&control, Err(error)),
                };
                // a query only checks the image could be loaded
                if control.action == b'q' {
                    return reply(&control, Ok(()));
                }
                let mut graphics = self.shared.lock().unwrap();
                let id = control.id.unwrap_or_else(|| {
                    graphics.next_auto_id -= 1;
                    graphics.next_auto_id
                });
                let size = (image.width, image.height);
                graphics.store(id, image);
                drop(graphics);
                let mut outcome = reply(&control, Ok(()));
                if control.action == b'T' {
                    outcome.output = self.place(&control, id, size);
                }
                outcome
            }
            b'p' => {
                let graphics = self.shared.lock().unwrap();
                let Some(id) = control.id else {
                    return reply(&control, Err("EINVAL:no image id"));
                };
                let Some(image) = graphics.images.get(&id) else {
                    return reply(&control, Err("ENOENT:no such image"));
                };
                let size = (image.width, image.height);
                drop(graphics);
                let mut outcome = reply(&control, Ok(()));
                outcome.output = self.place(&control, id, size);
                outcome
            }
            b'd' => {
                let mut graphics = self.shared.lock().unwrap();
                match control.delete {
                    b'a' => graphics.clear(false),
                    b'A' => graphics.clear(true),
                    b'i' | b'I' => {
                        if let Some(id) = control.id {
                            if control.delete == b'I' {
                                graphics.remove_image(id);
                            } else {
                                graphics.placements.retain(|_, p| p.image != id);
                            }
                        }
                    }
                    // other ways to pick what to delete are not supported, keeping images
                    _ => {}
                }
                Outcome::default()
            }
            _ => reply(&control, Err("EINVAL:unknown action")),
        }
    }

    // mark the cell under the cursor as where the image starts, then move the cursor past it
    // like kitty does: right by its columns, down by its rows less one
    fn place(&self, control: &Control, id: u32, (width, height): (u32, u32)) -> Vec<u8> {
        let (cell_width, cell_height) = self.cell.get();
        let cells = |pixels: u32, cell: f32| {
            if cell > 0. {
                (pixels as f32 / cell).ceil().max(1.) as usize
            } else {
                1
            }
        };
        let cols = if control.cols > 0 {
            control.cols
        } else {
            cells(width, cell_width)
        };
        let rows = if control.rows > 0 {
            control.rows
        } else {
            cells(height, cell_height)
        };
        let mut graphics = self.shared.lock().unwrap();
        let mark = graphics.next_mark;
        graphics.next_mark += 1;
        let fit = control.cols > 0 || control.rows > 0;
        graphics.placements.insert(
            mark,
            Placement {
                image: id,
                cols,
                rows,
                fit,
            },
        );
        drop(graphics);
        let mut out =
            format!("\x1b]8;id={IMAGE_MARK}{mark};kuterm:\x1b\\ \x1b]8;;\x1b\\\x08").into_bytes();
        if !control.keep_cursor {
            out.extend(std::iter::repeat_n(b'\n', rows.saturating_sub(1)));
            out.extend_from_slice(format!("\x1b[{cols}C").as_bytes());
        }
        out
    }
}

/// `OK` or an error for the program, when it gave an id and did not ask for quiet
fn reply(control: &Control, result: Result<(), &str>) -> Outcome {
    let quiet = match result {
        Ok(()) => control.quiet >= 1,
        Err(_) => control.quiet >= 2,
    };
    let keys = match (control.id, control.number) {
        (Some(id), Some(number)) => format!("i={id},I={number}"),
        (Some(id), None) => format!("i={id}"),
        (None, Some(number)) => format!("I={number}"),
        // kitty only answers commands that name an image
        (None, None) => return Outcome::default(),
    };
    if quiet {
        return Outcome::default();
    }
    let message = match result {
        Ok(()) => "OK",
        Err(error) => error,
    };
    Outcome {
        output: Vec::new(),
        reply: Some(format!("\x1b_G{keys};{message}\x1b\\").into_bytes()),
    }
}

// the bytes a transmission points at: sent inline, or in a file
fn image_bytes(control: &Control, data: &[u8]) -> Result<Vec<u8>, &'static str> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(
            data.iter()
                .filter(|b| !b.is_ascii_whitespace())
                .copied()
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "EINVAL:bad base64")?;
    match control.medium {
        b'd' => Ok(decoded),
        b'f' | b't' => {
            let path = std::path::PathBuf::from(
                String::from_utf8(decoded).map_err(|_| "EINVAL:bad file name")?,
            );
            let mut bytes = Vec::new();
            std::fs::File::open(&path)
                .map_err(|_| "EBADF:can't open the file")?
                .take(MAX_IMAGE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "EBADF:can't read the file")?;
            if bytes.len() > MAX_IMAGE_BYTES {
                return Err("EFBIG:image is too big");
            }
            // a temp file is ours to remove, but only one made for this protocol
            if control.medium == b't' && is_protocol_temp_file(&path) {
                std::fs::remove_file(&path).ok();
            }
            Ok(bytes)
        }
        _ => Err("EINVAL:only direct and file transmission are supported"),
    }
}

fn is_protocol_temp_file(path: &Path) -> bool {
    path.to_string_lossy().contains("tty-graphics-protocol")
        && (path.starts_with(std::env::temp_dir()) || path.starts_with("/tmp"))
}

// decoded pixels as gpui wants them, blue green red alpha
fn load(control: &Control, data: &[u8]) -> Result<StoredImage, &'static str> {
    if control.compressed {
        return Err("EINVAL:compressed data is not supported");
    }
    let bytes = image_bytes(control, data)?;
    let rgba = match control.format {
        100 => image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .map_err(|_| "EBADPNG:not a png image")?
            .into_rgba8(),
        24 | 32 => {
            let (width, height) = (control.width, control.height);
            let channels = if control.format == 24 { 3 } else { 4 };
            let expected = width as usize * height as usize * channels;
            if width == 0 || height == 0 || bytes.len() < expected {
                return Err("ENODATA:not enough pixels for the size");
            }
            let pixels = bytes[..expected]
                .chunks_exact(channels)
                .flat_map(|px| [px[0], px[1], px[2], if channels == 4 { px[3] } else { 255 }])
                .collect();
            image::RgbaImage::from_raw(width, height, pixels).ok_or("EINVAL:bad size")?
        }
        _ => return Err("EINVAL:unknown format"),
    };
    let (width, height) = rgba.dimensions();
    if width as usize * height as usize * 4 > MAX_IMAGE_BYTES {
        return Err("EFBIG:image is too big");
    }
    let mut bgra = rgba;
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(StoredImage {
        render: Arc::new(RenderImage::new(vec![image::Frame::new(bgra)])),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use base64::Engine;

    use super::{CellSize, Graphics, GraphicsParser};

    fn parser(max_mb: usize) -> GraphicsParser {
        let cell = Arc::new(CellSize::default());
        cell.set(10., 20.);
        GraphicsParser::new(Arc::new(Mutex::new(Graphics::new(true, max_mb))), cell)
    }

    fn rgba(width: u32, height: u32) -> String {
        base64::engine::general_purpose::STANDARD.encode(vec![200u8; (width * height * 4) as usize])
    }

    #[test]
    fn transmit_and_display_marks_a_cell_and_moves_the_cursor() {
        let mut parser = parser(10);
        let command = format!("a=T,f=32,s=25,v=50,i=7;{}", rgba(25, 50));
        let outcome = parser.command(command.as_bytes());
        assert_eq!(outcome.reply.as_deref(), Some(&b"\x1b_Gi=7;OK\x1b\\"[..]));
        let output = String::from_utf8(outcome.output).unwrap();
        // 25 by 50 pixels on 10 by 20 cells is 3 columns and 3 rows
        assert_eq!(
            output,
            "\x1b]8;id=kuterm-image-0;kuterm:\x1b\\ \x1b]8;;\x1b\\\x08\n\n\x1b[3C"
        );
        let shown = parser.shared.lock().unwrap().placement(0).unwrap();
        assert_eq!((shown.cols, shown.rows), (3, 3));
    }

    #[test]
    fn chunks_queries_puts_and_errors() {
        let mut parser = parser(10);
        let data = rgba(2, 2);
        let (head, tail) = data.split_at(8);
        assert_eq!(
            parser.command(format!("a=t,s=2,v=2,i=1,m=1;{head}").as_bytes()),
            Default::default()
        );
        let outcome = parser.command(format!("m=0;{tail}").as_bytes());
        assert!(outcome.reply.is_some() && outcome.output.is_empty());
        // put it twice with its own size in cells, keeping the cursor
        let outcome = parser.command(b"a=p,i=1,c=4,r=2,C=1");
        assert!(String::from_utf8(outcome.output).unwrap().ends_with("\x08"));
        assert_eq!(parser.shared.lock().unwrap().tallest(), 2);
        // queries answer without storing, missing images and bad data are errors
        let outcome = parser.command(format!("a=q,i=31,s=1,v=1,f=24;{}", "AAAA").as_bytes());
        assert_eq!(outcome.reply.as_deref(), Some(&b"\x1b_Gi=31;OK\x1b\\"[..]));
        let outcome = parser.command(b"a=p,i=99");
        assert!(
            String::from_utf8(outcome.reply.unwrap())
                .unwrap()
                .contains("ENOENT")
        );
        let outcome = parser.command(b"a=t,f=100,i=5;AAAA");
        assert!(
            String::from_utf8(outcome.reply.unwrap())
                .unwrap()
                .contains("EBADPNG")
        );
        // quiet 2 hides errors, no id means no answer
        assert_eq!(parser.command(b"a=p,i=99,q=2").reply, None);
        assert_eq!(parser.command(b"a=t,f=100;AAAA").reply, None);
        // deleting placements keeps the image
        parser.command(b"a=d,d=a");
        assert!(parser.shared.lock().unwrap().is_empty());
        assert!(
            parser
                .command(b"a=p,i=1")
                .reply
                .unwrap()
                .ends_with(b"OK\x1b\\")
        );
    }

    #[test]
    fn png_files_load_and_old_images_make_room() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 2, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let path = std::env::temp_dir().join(format!("kuterm-test-{}.png", std::process::id()));
        std::fs::write(&path, &png).unwrap();
        let name =
            base64::engine::general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes());
        let mut parser = parser(1);
        let outcome = parser.command(format!("a=T,f=100,t=f,i=3;{name}").as_bytes());
        assert!(outcome.reply.unwrap().ends_with(b"OK\x1b\\"));
        // a file transmission leaves the file alone
        assert!(path.exists());
        std::fs::remove_file(&path).ok();

        // 1 MB holds one 400 by 400 image, the older one goes for the newer
        let big = rgba(400, 400);
        parser.command(format!("a=t,s=400,v=400,i=10;{big}").as_bytes());
        parser.command(format!("a=t,s=400,v=400,i=11;{big}").as_bytes());
        let graphics = parser.shared.lock().unwrap();
        assert!(!graphics.images.contains_key(&10));
        assert!(graphics.images.contains_key(&11));
        assert!(!graphics.dropped.is_empty());
    }

    #[test]
    fn shown_image_follows_its_cell_and_answers_the_program() {
        use crate::{
            settings::{Shell, TerminalSettings},
            terminal::tests::{profile, spawn_with, wait_for_text},
        };

        // a 20 by 40 pixel image is 2 by 2 cells of the 10 by 20 test grid
        let data = rgba(20, 40);
        let script = format!(
            "stty raw -echo; printf 'before\\r\\n\\033_Ga=T,f=32,s=20,v=40,i=9;{data}\\033\\\\'; \\
             printf 'after\\r\\n'; dd bs=1 count=11 2>/dev/null | od -An -c | tr -d ' \\n'; echo =got; sleep 5"
        );
        let profile = profile(Shell::WithArguments {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script],
        });
        let mut builder = spawn_with(&TerminalSettings::default(), &profile);
        let terminal = &mut builder.terminal;
        wait_for_text(terminal, "after");
        let events = terminal.handle_scan_events();
        assert!(events.is_empty(), "{events:?}");
        // the answer reached the program
        // od shows escape as 033
        wait_for_text(terminal, "033_Gi=9;OK033\\=got");
        let images = &terminal.last_content.images;
        assert_eq!(images.len(), 1);
        let (start, image) = &images[0];
        assert_eq!(
            (start.line.0, start.column.0, image.cols, image.rows),
            (1, 0, 2, 2)
        );
        // the cursor moved past it, "after" starts right of its last row
        let text = crate::terminal::tests::screen_text(terminal);
        assert!(
            text.lines().nth(2).unwrap().starts_with("  after"),
            "{text}"
        );

        // deleting placements hides it
        let parser_shared = terminal.graphics.clone();
        parser_shared.lock().unwrap().clear(false);
        terminal.scroll(0);
        terminal.sync();
        assert!(terminal.last_content.images.is_empty());
    }
}
