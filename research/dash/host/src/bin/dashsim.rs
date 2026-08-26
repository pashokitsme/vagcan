//! `dashsim` — be the panel and the buttons, so the board can be the device.
//!
//! The board renders a real 256×64 frame with the real `vag-dash` code and
//! sends it over the USB serial it is already flashed and logged through. This
//! draws it in the terminal and sends button presses back. Nothing here decides
//! anything: no layout, no page order, no formatting. If this program were
//! clever, the thing being tested would be this program.
//!
//! Two ways to draw pixels in a terminal, both of which keep the aspect ratio
//! square because a terminal cell is about twice as tall as it is wide:
//!
//! * **half blocks** (`▀ ▄ █`) — one pixel per column, two per row, so 256×64
//!   wants 256 columns. Crisp, and the closest thing to seeing the panel.
//! * **braille** (`⠀`–`⣿`) — two pixels per column, four per row, so the same
//!   panel fits in **128 columns**. Denser and less crisp, and the only option
//!   in a window that is not 258 columns wide.
//!
//! The mode is chosen from the terminal width each time it draws, so widening
//! the window switches back on its own; `b` forces braille either way.

use anyhow::{Context, Result};
use vag_dash_host::frame::{self, Bitmap};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::{execute, terminal};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::Duration;

const BAUD: u32 = 115_200;
/// How many of the board's log lines to keep under the panel. Enough to see
/// what just happened, few enough that the panel stays on screen.
const LOG_LINES: usize = 8;

/// How pixels become characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    HalfBlocks,
    Braille,
}

impl Mode {
    /// Terminal columns one panel row needs, borders included.
    fn columns_for(self, width: u32) -> u32 {
        match self {
            Mode::HalfBlocks => width + 2,
            Mode::Braille => width.div_ceil(2) + 2,
        }
    }
}

/// Renders the bitmap as terminal rows, without borders.
///
/// Braille packs a 2×4 block of pixels into one code point. The dot numbering
/// is the historical one and is *not* row-major — dots 1,2,3 run down the left
/// column, 4,5,6 down the right, and 7,8 are the fourth row added later for
/// computing. Hence the table rather than a shift.
fn rows(bitmap: &Bitmap, mode: Mode) -> Vec<String> {
    match mode {
        Mode::HalfBlocks => (0..bitmap.height)
            .step_by(2)
            .map(|y| {
                (0..bitmap.width)
                    .map(|x| match (bitmap.get(x, y), bitmap.get(x, y + 1)) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    })
                    .collect()
            })
            .collect(),
        Mode::Braille => {
            const DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
            (0..bitmap.height)
                .step_by(4)
                .map(|y| {
                    (0..bitmap.width)
                        .step_by(2)
                        .map(|x| {
                            let mut bits = 0u8;
                            for (dy, row) in DOTS.iter().enumerate() {
                                for (dx, dot) in row.iter().enumerate() {
                                    if bitmap.get(x + dx as u32, y + dy as u32) {
                                        bits |= dot;
                                    }
                                }
                            }
                            char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
                        })
                        .collect()
                })
                .collect()
        }
    }
}

enum FromBoard {
    Frame(Box<Bitmap>),
    Log(String),
    Gone(String),
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    match first.as_deref() {
        Some("--demo") => return demo(),
        Some("--list") => return list_ports(),
        _ => {}
    }
    let port_name = match first {
        Some(name) => name,
        None => guess_port()?,
    };
    run(&port_name)
}

fn list_ports() -> Result<()> {
    for port in serialport::available_ports()? {
        println!("{}", port.port_name);
    }
    Ok(())
}

/// The ESP32-C3's native USB shows up as a `usbmodem`; picking it beats making
/// every run start with a path nobody remembers.
fn guess_port() -> Result<String> {
    let ports = serialport::available_ports()?;
    ports
        .iter()
        .map(|p| p.port_name.clone())
        .find(|n| n.contains("usbmodem"))
        .context("no usbmodem port found — pass one explicitly, or --list to see them")
}

fn run(port_name: &str) -> Result<()> {
    let port = serialport::new(port_name, BAUD)
        .timeout(Duration::from_millis(200))
        .open()
        .with_context(|| format!("opening {port_name}"))?;
    let mut writer = port.try_clone().context("cloning the port for writing")?;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(port);
        let mut line = String::new();
        loop {
            line.clear();
            // A read timeout is not an error here: the board is simply quiet.
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(FromBoard::Gone("port closed".into()));
                    return;
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    let message = match frame::decode(trimmed) {
                        Ok(bitmap) => FromBoard::Frame(Box::new(bitmap)),
                        Err(frame::DecodeError::NotAFrame) => FromBoard::Log(strip_ansi(trimmed)),
                        // A malformed frame is worth seeing, not hiding: it
                        // means the two encoders have drifted apart.
                        Err(e) => FromBoard::Log(format!("[bad frame] {e}")),
                    };
                    if tx.send(message).is_err() {
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => {
                    let _ = tx.send(FromBoard::Gone(e.to_string()));
                    return;
                }
            }
        }
    });

    terminal::enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, terminal::EnterAlternateScreen, crossterm::cursor::Hide)?;

    let result = event_loop(&mut out, &mut writer, &rx);

    execute!(out, crossterm::cursor::Show, terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;
    result
}

fn event_loop(
    out: &mut std::io::Stdout,
    writer: &mut Box<dyn serialport::SerialPort>,
    rx: &mpsc::Receiver<FromBoard>,
) -> Result<()> {
    let mut logs: VecDeque<String> = VecDeque::new();
    let mut latest: Option<Bitmap> = None;
    let mut frames = 0u64;
    let mut status = String::from("waiting for the board");
    // `None` means "pick whatever fits"; `b` pins it to braille.
    let mut forced: Option<Mode> = None;
    let mut redraw = true;

    loop {
        // Drain everything the board has said, then draw once. Drawing per
        // message would make the terminal the bottleneck.
        let mut dirty = false;
        while let Ok(message) = rx.try_recv() {
            dirty = true;
            match message {
                FromBoard::Frame(bitmap) => {
                    frames += 1;
                    status = format!("{}×{}, {frames} frames", bitmap.width, bitmap.height);
                    latest = Some(*bitmap);
                }
                FromBoard::Log(line) => {
                    if !line.trim().is_empty() {
                        logs.push_back(line);
                        while logs.len() > LOG_LINES {
                            logs.pop_front();
                        }
                    }
                }
                FromBoard::Gone(why) => {
                    status = format!("board gone: {why}");
                }
            }
        }
        if dirty || redraw {
            redraw = false;
            draw(out, latest.as_ref(), &logs, &status, forced)?;
        }

        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    // The two gestures the device has. They are written as
                    // events rather than as a held level because the board's
                    // debounce belongs to the board — this is a keyboard, and
                    // a keyboard cannot honestly imitate a contact bouncing.
                    KeyCode::Char(' ') => {
                        writeln!(writer, "BTN S")?;
                        writer.flush()?;
                    }
                    KeyCode::Char('l') | KeyCode::Char('L') => {
                        writeln!(writer, "BTN L")?;
                        writer.flush()?;
                    }
                    KeyCode::Char('b') | KeyCode::Char('B') => {
                        forced = match forced {
                            Some(Mode::Braille) => None,
                            _ => Some(Mode::Braille),
                        };
                        redraw = true;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw(
    out: &mut std::io::Stdout,
    bitmap: Option<&Bitmap>,
    logs: &VecDeque<String>,
    status: &str,
    forced: Option<Mode>,
) -> Result<()> {
    execute!(out, terminal::Clear(terminal::ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
    let (columns, _) = terminal::size().unwrap_or((80, 24));
    let mut mode_note = "";

    match bitmap {
        Some(bitmap) => {
            // Half blocks if they fit, braille if they do not. Chosen per draw
            // rather than at start-up so resizing the window just works.
            let mode = forced.unwrap_or({
                if u32::from(columns) >= Mode::HalfBlocks.columns_for(bitmap.width) {
                    Mode::HalfBlocks
                } else {
                    Mode::Braille
                }
            });
            let lines = rows(bitmap, mode);
            let inner = lines.first().map(|l| l.chars().count()).unwrap_or(0);
            if u32::from(columns) < mode.columns_for(bitmap.width) {
                writeln!(
                    out,
                    "terminal is {columns} columns; even braille needs {}. Make the window wider.\r",
                    mode.columns_for(bitmap.width)
                )?;
            } else {
                writeln!(out, "┌{}┐\r", "─".repeat(inner))?;
                for line in &lines {
                    writeln!(out, "│{line}│\r")?;
                }
                writeln!(out, "└{}┘\r", "─".repeat(inner))?;
            }
            mode_note = match mode {
                Mode::HalfBlocks => "half blocks",
                Mode::Braille => "braille",
            };
        }
        None => writeln!(out, "(no frame yet)\r")?,
    }

    writeln!(
        out,
        "\r\n  {status}   ·   {mode_note}   ·   space = short, L = long, b = braille, q = quit\r\n\r"
    )?;
    for line in logs {
        writeln!(out, "  {line}\r")?;
    }
    out.flush()?;
    Ok(())
}

/// The board's logger colours its output. In the alternate screen those codes
/// would smear, so they come off.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Draws one synthetic frame and exits. Proves the renderer without a board —
/// and, when the board is connected, tells you whether a blank panel is the
/// board's fault or this program's.
fn demo() -> Result<()> {
    let (width, height) = (256u32, 64u32);
    let mut pixels = vec![false; (width * height) as usize];
    let mut set = |x: u32, y: u32| {
        if x < width && y < height {
            pixels[(y * width + x) as usize] = true;
        }
    };
    for x in 0..width {
        set(x, 0);
        set(x, height - 1);
    }
    for y in 0..height {
        set(0, y);
        set(width - 1, y);
    }
    // A ramp, so the half-block packing is visibly right rather than plausible.
    for x in 0..width {
        let h = (x * (height - 4) / width) + 2;
        set(x, h);
    }
    for y in 20..40 {
        for x in 100..160 {
            set(x, y);
        }
    }
    let bitmap = Bitmap { width, height, pixels };
    let line = frame::encode(&bitmap);
    println!("encoded frame is {} characters", line.len());
    let decoded = frame::decode(&line)?;
    let (columns, _) = terminal::size().unwrap_or((80, 24));
    let mode = if u32::from(columns) >= Mode::HalfBlocks.columns_for(width) {
        Mode::HalfBlocks
    } else {
        Mode::Braille
    };
    let lines = rows(&decoded, mode);
    let inner = lines.first().map(|l| l.chars().count()).unwrap_or(0);
    println!("┌{}┐", "─".repeat(inner));
    for line in &lines {
        println!("│{line}│");
    }
    println!("└{}┘", "─".repeat(inner));
    Ok(())
}
