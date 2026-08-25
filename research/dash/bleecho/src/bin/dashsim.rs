//! `dashsim` — be the panel and the buttons, so the board can be the device.
//!
//! The board renders a real 256×64 frame with the real `vag-dash` code and
//! sends it over the USB serial it is already flashed and logged through. This
//! draws it in the terminal and sends button presses back. Nothing here decides
//! anything: no layout, no page order, no formatting. If this program were
//! clever, the thing being tested would be this program.
//!
//! Two vertical pixels share one terminal cell (`▀ ▄ █`), so 256×64 becomes
//! 256 columns by 32 rows and the aspect ratio is right. A terminal narrower
//! than the panel is told so rather than silently showing half a picture.

use anyhow::{Context, Result};
use bleecho::frame::{self, Bitmap};
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
        if dirty {
            draw(out, latest.as_ref(), &logs, &status)?;
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
                    _ => {}
                }
            }
        }
    }
}

fn draw(out: &mut std::io::Stdout, bitmap: Option<&Bitmap>, logs: &VecDeque<String>, status: &str) -> Result<()> {
    execute!(out, terminal::Clear(terminal::ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
    let (columns, _) = terminal::size().unwrap_or((80, 24));

    match bitmap {
        Some(bitmap) => {
            if u32::from(columns) < bitmap.width + 2 {
                writeln!(
                    out,
                    "terminal is {columns} columns; the panel needs {}. Make the window wider or the font smaller.\r",
                    bitmap.width + 2
                )?;
            } else {
                writeln!(out, "┌{}┐\r", "─".repeat(bitmap.width as usize))?;
                for pair in (0..bitmap.height).step_by(2) {
                    let mut row = String::with_capacity(bitmap.width as usize);
                    for x in 0..bitmap.width {
                        row.push(match (bitmap.get(x, pair), bitmap.get(x, pair + 1)) {
                            (true, true) => '█',
                            (true, false) => '▀',
                            (false, true) => '▄',
                            (false, false) => ' ',
                        });
                    }
                    writeln!(out, "│{row}│\r")?;
                }
                writeln!(out, "└{}┘\r", "─".repeat(bitmap.width as usize))?;
            }
        }
        None => writeln!(out, "(no frame yet)\r")?,
    }

    writeln!(out, "\r\n  {status}   ·   space = short press, L = long press, q = quit\r\n\r")?;
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
    println!("┌{}┐", "─".repeat(width as usize));
    for pair in (0..height).step_by(2) {
        let mut row = String::new();
        for x in 0..width {
            row.push(match (decoded.get(x, pair), decoded.get(x, pair + 1)) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
            });
        }
        println!("│{row}│");
    }
    println!("└{}┘", "─".repeat(width as usize));
    Ok(())
}
