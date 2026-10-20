//! The tables a command prints as its data: a header row, aligned columns, two spaces
//! between them, the wide text column last.
//!
//! Fixed column order, decided by the caller; a column is as wide as its widest cell. Only
//! the last cell is ever shortened, to the terminal's width with `…` — and only when that
//! width is known, which is to say only when stdout is a terminal: a pipe or a file gets
//! every character. With too little room left for it, the last cell goes whole on a line
//! of its own under its row ([`Table::render`]).

/// One table, built row by row.
#[derive(Debug, Clone, Default)]
pub struct Table {
	headers: Vec<String>,
	/// Which columns are right-aligned: the numbers.
	right: Vec<bool>,
	rows: Vec<Vec<String>>,
	/// See [`Table::no_cut`].
	no_cut: bool,
}

impl Table {
	pub fn new(headers: &[&str]) -> Table {
		Table {
			headers: headers.iter().map(|h| h.to_string()).collect(),
			right: vec![false; headers.len()],
			rows: Vec::new(),
			no_cut: false,
		}
	}

	/// Right-align column `at`.
	pub fn right(mut self, at: usize) -> Table {
		if let Some(r) = self.right.get_mut(at) {
			*r = true;
		}
		self
	}

	/// Add a row; missing cells are blank, extra ones are dropped.
	pub fn row(&mut self, mut cells: Vec<String>) {
		cells.resize(self.headers.len(), String::new());
		self.rows.push(cells);
	}

	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}

	pub fn len(&self) -> usize {
		self.rows.len()
	}

	/// Never cut the last cell, only move it under its row when it does not fit: for a
	/// column that holds paths, where a cut path is no path.
	pub fn no_cut(mut self) -> Table {
		self.no_cut = true;
		self
	}

	/// The table as lines, each ending in a newline, no trailing spaces. `width`: the
	/// columns the screen has, if known ([`terminal_width`]).
	///
	/// Only the last cell is ever shortened, and only to a known width: the columns before
	/// it are whole. When the last column would get fewer than [`MIN_LAST`] columns, it
	/// leaves the row and goes on an indented line under it, whole, and its header goes
	/// too; a [`Table::no_cut`] cell that does not fit does the same rather than being cut.
	pub fn render(&self, width: Option<usize>) -> String {
		let columns = self.headers.len();
		let mut widths: Vec<usize> = self.headers.iter().map(|h| chars(h)).collect();
		for row in &self.rows {
			for (at, cell) in row.iter().enumerate() {
				widths[at] = widths[at].max(chars(cell));
			}
		}
		// What the columns before the last take, separators included.
		let before: usize = widths[..columns.saturating_sub(1)].iter().map(|w| w + 2).sum();
		let room = width.map(|w| w.saturating_sub(before));
		let under = columns > 1 && room.is_some_and(|room| room < MIN_LAST);
		let indent = " ".repeat(widths.first().map_or(0, |w| w + 2));
		let mut out = String::new();
		for (index, row) in std::iter::once(&self.headers).chain(&self.rows).enumerate() {
			let header = index == 0;
			let mut line = String::new();
			for (at, cell) in row.iter().enumerate().take(columns.saturating_sub(1)) {
				if at > 0 {
					line.push_str("  ");
				}
				let pad = widths[at].saturating_sub(chars(cell));
				if self.right[at] {
					line.push_str(&" ".repeat(pad));
					line.push_str(cell);
				} else {
					line.push_str(cell);
					line.push_str(&" ".repeat(pad));
				}
			}
			let last = row.last().map(String::as_str).unwrap_or_default();
			let fits = room.is_none_or(|room| chars(last) <= room);
			let below = columns > 1 && (under || (!fits && self.no_cut));
			if columns == 1 || !below {
				if columns > 1 {
					line.push_str("  ");
				}
				match (fits, room) {
					(false, Some(room)) => line.push_str(&cut(last, room)),
					// A column of numbers last is still right-aligned.
					_ if self.right[columns - 1] => {
						line.push_str(&" ".repeat(widths[columns - 1].saturating_sub(chars(last))));
						line.push_str(last);
					}
					_ => line.push_str(last),
				}
			}
			out.push_str(line.trim_end());
			out.push('\n');
			if below && !header && !last.is_empty() {
				out.push_str(&indent);
				out.push_str(last);
				out.push('\n');
			}
		}
		out
	}
}

/// The fewest columns the last cell of a row is cut to; with less room it goes on a line
/// of its own under the row.
pub const MIN_LAST: usize = 30;

/// `text` cut to `width` columns with `…` as its last, when it is longer.
fn cut(text: &str, width: usize) -> String {
	if chars(text) <= width || width == 0 {
		return text.to_string();
	}
	let mut cut: String = text.chars().take(width - 1).collect();
	cut.push('…');
	cut
}

fn chars(text: &str) -> usize {
	text.chars().count()
}

/// The noun for a count: `1 file`, `2 files` — never `file(s)`. Regular plurals only,
/// because every count in this tool is of something with one.
pub fn plural(n: usize, singular: &str) -> String {
	if n == 1 { singular.to_string() } else { format!("{singular}s") }
}

/// The terminal's width in columns, when stdout is one; `None` for a pipe or a file.
pub fn terminal_width() -> Option<usize> {
	if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
		return None;
	}
	crossterm::terminal::size().ok().map(|(columns, _)| usize::from(columns))
}

/// A `key  value` block: the keys padded to one width, in the order given. A value of
/// several lines continues under the first, aligned.
pub fn pairs(rows: &[(&str, String)]) -> String {
	let width = rows.iter().map(|(k, _)| chars(k)).max().unwrap_or(0);
	let mut out = String::new();
	for (key, value) in rows {
		let mut lines = value.lines();
		let first = lines.next().unwrap_or("");
		out.push_str(format!("{key:<width$}  {first}").trim_end());
		out.push('\n');
		for more in lines {
			out.push_str(format!("{:width$}  {more}", "").trim_end());
			out.push('\n');
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn columns_align_and_the_last_runs_free() {
		let mut table = Table::new(&["unit", "id", "count", "text"]).right(2);
		table.row(vec!["16".into(), "70C".into(), "254".into(), "Unterbrechung".into()]);
		table.row(vec!["17".into(), "714".into(), "13".into(), "BAP keine Kommunikation".into()]);
		assert_eq!(
			table.render(None),
			"unit  id   count  text\n\
			 16    70C    254  Unterbrechung\n\
			 17    714     13  BAP keine Kommunikation\n"
		);
	}

	#[test]
	fn the_last_cell_alone_is_cut_and_only_to_a_known_width() {
		let text = "a long text that will not fit in the forty columns given here";
		let mut table = Table::new(&["id", "when", "text"]);
		table.row(vec!["70C".into(), "2026-09-28 14:20:59".into(), text.into()]);
		let cut = table.render(Some(60));
		assert_eq!(cut.lines().nth(1), Some("70C  2026-09-28 14:20:59  a long text that will not fit in …"));
		assert!(cut.lines().all(|l| l.chars().count() <= 60), "{cut}");
		assert!(table.render(None).contains(text), "a pipe gets every character");
	}

	#[test]
	fn with_too_little_room_the_last_cell_goes_whole_under_its_row() {
		// An 80-column terminal and the faults table: the text would get a handful of
		// columns, so it gets a line of its own instead (UX review of PR #38).
		let mut table = Table::new(&["unit", "when", "text"]);
		table.row(vec![
			"16".into(),
			"2026-09-28 14:20:59".into(),
			"Unterbrechung des Signalpfades \"Init\"".into(),
		]);
		table.row(vec!["17".into(), "2026-09-28 22:14:20".into(), String::new()]);
		assert_eq!(
			table.render(Some(40)),
			"unit  when\n\
			 16    2026-09-28 14:20:59\n\
			 \x20     Unterbrechung des Signalpfades \"Init\"\n\
			 17    2026-09-28 22:14:20\n"
		);
	}

	#[test]
	fn a_no_cut_cell_moves_under_its_row_rather_than_lose_a_character() {
		let path = "/Users/someone/.vagcan/data/SK37X/cache.sqlite";
		let mut table = Table::new(&["step", "detail"]).no_cut();
		table.row(vec!["labels".into(), path.into()]);
		table.row(vec!["names".into(), "21576 names".into()]);
		let out = table.render(Some(40));
		assert_eq!(out, format!("step    detail\nlabels\n        {path}\nnames   21576 names\n"));
	}

	#[test]
	fn a_blank_last_cell_leaves_no_trailing_space() {
		let mut table = Table::new(&["id", "name"]);
		table.row(vec!["70C".into()]);
		assert_eq!(table.render(None), "id   name\n70C\n");
	}

	#[test]
	fn pairs_pad_the_keys() {
		assert_eq!(
			pairs(&[("vin", "XW8".into()), ("engine", "8V0906264H".into()), ("x", String::new())]),
			"vin     XW8\nengine  8V0906264H\nx\n"
		);
	}
}
