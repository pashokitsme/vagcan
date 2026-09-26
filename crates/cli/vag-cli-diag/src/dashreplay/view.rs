//! The terminal: the panel at the recording's pace, and the log under it.
//!
//! The only place in this command that reads a wall clock, and only to pace the frames: what
//! happens in them is decided in recording time by [`Replay`]. The terminal is taken through
//! [`term`], so `q`, `Esc`, Ctrl-C, an error and a panic all hand it back as it was.

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};

use super::engine::{HEIGHT, Replay, WIDTH};
use super::glass::{Canvas, braille, half_blocks};
use crate::ui::term;

/// Play `replay` to the end or until the person leaves, and return every line of the log —
/// `notes` first — for the caller to print once the screen is gone.
pub fn show(mut replay: Replay, notes: &[String], title: &str, speed: f64) -> Result<Vec<String>> {
	let screen = term::full_screen()
		.hiding_cursor()
		.enter()
		.map_err(|e| anyhow!("the panel needs an interactive terminal ({e}); piped, only the log is printed"))?;
	let mut out = std::io::stdout();
	let mut log = notes.to_vec();
	let mut canvas = Canvas::new(WIDTH, HEIGHT);
	let (first, end) = (replay.start_ms(), replay.end_ms());
	let started = Instant::now();
	let mut left = false;
	while let Some(tick) = replay.step() {
		let due = started + Duration::from_secs_f64((tick.t_ms - first) as f64 / 1000.0 / speed);
		if wait_until(due)? {
			left = true;
			break;
		}
		log.extend(tick.events.iter().map(|event| replay.line(tick.t_ms, event)));
		canvas.clear();
		replay.draw(&mut canvas);
		let status = format!(
			" {title} · {:.1} / {:.1} s · ×{speed} · [q] quit",
			tick.t_ms as f64 / 1000.0,
			end as f64 / 1000.0
		);
		paint(&mut out, &canvas, &log, &status)?;
	}
	if !left {
		paint(&mut out, &canvas, &log, &format!(" {title} · end of the recording — any key to leave"))?;
		loop {
			if let Event::Key(key) = event::read()?
				&& key.kind == KeyEventKind::Press
			{
				break;
			}
		}
	}
	drop(screen);
	Ok(log)
}

/// Wait for the frame's moment, listening for a way out. `true` when the person left.
fn wait_until(due: Instant) -> Result<bool> {
	loop {
		let now = Instant::now();
		if now >= due {
			return Ok(false);
		}
		if event::poll((due - now).min(Duration::from_millis(50)))?
			&& let Event::Key(key) = event::read()?
			&& key.kind == KeyEventKind::Press
			&& leaves(key)
		{
			return Ok(true);
		}
	}
}

/// `q`, `Esc`, and Ctrl-C — which in raw mode is a key like any other, not a signal.
fn leaves(key: KeyEvent) -> bool {
	matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// How the panel is drawn as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Glyphs {
	/// One character per pixel column, two pixel rows per line.
	HalfBlocks,
	/// Two pixel columns and four pixel rows per character.
	Braille,
}

/// Half blocks where the whole panel fits them — its width plus the frame, its height in
/// half-block lines plus the status line and the frame — braille where it does not.
fn glyphs(columns: usize, rows: usize, width: usize, height: usize) -> Glyphs {
	match columns >= width + 2 && rows >= height.div_ceil(2) + 3 {
		true => Glyphs::HalfBlocks,
		false => Glyphs::Braille,
	}
}

/// One screen: the status line, the panel in a frame, and as much of the log's end as fits.
fn paint(out: &mut impl Write, canvas: &Canvas, log: &[String], status: &str) -> Result<()> {
	// A terminal that does not say its size — or says it is 0 × 0, as a bare pty does — is
	// drawn as the smallest a person works in.
	let (columns, rows) = terminal::size().ok().filter(|&(c, r)| c > 0 && r > 0).unwrap_or((80, 24));
	let (columns, rows) = (usize::from(columns), usize::from(rows));
	let panel = match glyphs(columns, rows, canvas.width(), canvas.height()) {
		Glyphs::HalfBlocks => half_blocks(canvas),
		Glyphs::Braille => braille(canvas),
	};
	let inner = panel.first().map_or(0, |line| line.chars().count());
	let status = match inner + 2 > columns {
		true => format!("{status} · panel cut, needs {} columns", inner + 2),
		false => status.to_string(),
	};
	let mut lines = vec![status, format!("┌{}┐", "─".repeat(inner))];
	lines.extend(panel.iter().map(|line| format!("│{line}│")));
	lines.push(format!("└{}┘", "─".repeat(inner)));
	let room = rows.saturating_sub(lines.len());
	lines.extend(log[log.len().saturating_sub(room)..].iter().cloned());
	for (row, line) in lines.iter().take(rows).enumerate() {
		let fitted: String = line.chars().take(columns).collect();
		queue!(out, MoveTo(0, row as u16), Print(fitted), Clear(ClearType::UntilNewLine))?;
	}
	queue!(out, Clear(ClearType::FromCursorDown))?;
	out.flush()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn half_blocks_only_where_the_whole_panel_fits_in_both_directions() {
		// 256 × 64 pixels: half blocks are 258 columns by 32 + 3 rows (status, frame).
		assert_eq!(glyphs(258, 35, WIDTH, HEIGHT), Glyphs::HalfBlocks);
		assert_eq!(glyphs(257, 35, WIDTH, HEIGHT), Glyphs::Braille, "too narrow");
		assert_eq!(glyphs(300, 34, WIDTH, HEIGHT), Glyphs::Braille, "too short: the panel's bottom was cut");
	}
}
