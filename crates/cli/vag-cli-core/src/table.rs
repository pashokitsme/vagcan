//! The tables a command prints as its data: a header row, aligned columns, two spaces
//! between them, the wide text column last.
//!
//! Fixed column order, decided by the caller; a column is as wide as its widest cell. The
//! last column is cut to the terminal's width with `…` — only when that width is known,
//! which is to say only when stdout is a terminal: a pipe or a file gets every character.

/// One table, built row by row.
#[derive(Debug, Clone, Default)]
pub struct Table {
	headers: Vec<String>,
	/// Which columns are right-aligned: the numbers.
	right: Vec<bool>,
	rows: Vec<Vec<String>>,
}

impl Table {
	pub fn new(headers: &[&str]) -> Table {
		Table {
			headers: headers.iter().map(|h| h.to_string()).collect(),
			right: vec![false; headers.len()],
			rows: Vec::new(),
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

	/// The table as lines, each ending in a newline, no trailing spaces. `width`: the
	/// columns the screen has, if known ([`terminal_width`]); the last column is cut to fit.
	pub fn render(&self, width: Option<usize>) -> String {
		let columns = self.headers.len();
		let mut widths: Vec<usize> = self.headers.iter().map(|h| chars(h)).collect();
		for row in &self.rows {
			for (at, cell) in row.iter().enumerate() {
				widths[at] = widths[at].max(chars(cell));
			}
		}
		let mut out = String::new();
		for row in std::iter::once(&self.headers).chain(&self.rows) {
			let mut line = String::new();
			for (at, cell) in row.iter().enumerate() {
				if at > 0 {
					line.push_str("  ");
				}
				let last = at + 1 == columns;
				let pad = widths[at].saturating_sub(chars(cell));
				match (last, self.right[at]) {
					(true, false) => line.push_str(cell),
					(_, true) => {
						line.push_str(&" ".repeat(pad));
						line.push_str(cell);
					}
					(false, false) => {
						line.push_str(cell);
						line.push_str(&" ".repeat(pad));
					}
				}
			}
			let line = line.trim_end();
			out.push_str(&fit(line, width));
			out.push('\n');
		}
		out
	}
}

fn chars(text: &str) -> usize {
	text.chars().count()
}

/// `line` cut to `width` columns with `…` as its last, when it is longer.
pub fn fit(line: &str, width: Option<usize>) -> String {
	match width {
		Some(width) if width > 1 && chars(line) > width => {
			let mut cut: String = line.chars().take(width - 1).collect();
			cut.push('…');
			cut
		}
		_ => line.to_string(),
	}
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
	fn the_last_column_is_cut_only_to_a_known_width() {
		let mut table = Table::new(&["id", "text"]);
		table.row(vec!["70C".into(), "a long text that will not fit".into()]);
		let cut = table.render(Some(16));
		assert_eq!(cut.lines().nth(1), Some("70C  a long tex…"));
		assert!(cut.lines().all(|l| l.chars().count() <= 16), "{cut}");
		assert!(table.render(None).contains("will not fit"));
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
