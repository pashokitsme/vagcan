//! One picture's worth of already-decided facts.
//!
//! Everything here is past tense: the value has been scaled, the label has been
//! translated, the alarm has already fired. Nothing in this module can fail,
//! because every way of failing happened earlier — in the plan, on the bus, in
//! the decoder — and arrived here as an [`Option`].

/// One cell of a values page: a label, a number, a unit.
///
/// `value` is an [`Option`] and that is the most important type in this crate.
/// A cell whose channel has not answered draws a dash, never a zero. A number
/// the car never gave is the failure this whole project is built against, and on
/// a panel with no room for a footnote it matters more, not less.
pub struct Cell<'a> {
	/// Already in the reader's language. Ten characters at most in a four-column
	/// layout — see [`crate::render`], which reports an overrun rather than
	/// quietly clipping it.
	pub label: &'a str,
	/// Scaled, in the unit named by `unit`. `None` means the channel has not
	/// answered.
	pub value: Option<f32>,
	/// `"°C"`, `"bar"`, `"Nm"` — or empty, for a count.
	pub unit: &'a str,
	/// Places after the point. Boost wants two, a temperature wants none, and
	/// deciding per cell is cheaper than deciding per panel.
	pub decimals: u8,
	/// Draw this cell inverted — black on white.
	///
	/// This is how an alarm shows *which* cylinder. Filling the whole panel
	/// would lose exactly the thing the alarm view exists to say; inverting one
	/// cell keeps the label and the number, they simply swap with the ground.
	pub alarm: bool,
}

impl<'a> Cell<'a> {
	pub const fn new(label: &'a str, value: Option<f32>, unit: &'a str, decimals: u8) -> Self {
		Cell {
			label,
			value,
			unit,
			decimals,
			alarm: false,
		}
	}

	pub const fn alarmed(mut self) -> Self {
		self.alarm = true;
		self
	}
}

/// What to draw.
pub enum Frame<'a> {
	/// Up to four cells across. The photograph's layout, and the one the
	/// per-cylinder screens use unchanged — one column per cylinder.
	Values { cells: &'a [Cell<'a>] },
	/// One channel: the value large on the left, its recent history on the right.
	Chart {
		cell: Cell<'a>,
		/// The vertical scale, **fixed**, from the plan.
		///
		/// Not autoscaled, and this is a decision rather than an omission.
		/// Autoscale lies twice: it turns a flat trace into drama, and the first
		/// outlier widens the range until a real collapse reads as flat. A boost
		/// trace that always fills the box says nothing at all.
		min: f32,
		max: f32,
		/// Oldest first, one per pixel column. Fewer samples than columns draws
		/// a shorter trace — never an invented point.
		samples: &'a [f32],
		/// How long one sample is — the poll period, in seconds.
		///
		/// The *window* the header prints is derived from it here rather than
		/// passed in, because only the renderer knows how many columns the plot
		/// got: one pixel is one poll, so a history deeper than the plot is wide
		/// has its oldest samples dropped, and a caller multiplying its whole
		/// history by the period prints a window twice what the screen shows.
		/// A window is printed at all because one nobody can see is a chart
		/// nobody can read — the same argument `watch/history.rs` makes for
		/// printing its own.
		seconds_per_sample: f32,
	},
	/// The board as a plain CAN adapter — the `dash` image's mode 2, `vagcan --slcan`:
	/// the host drives the pair and the panel says so instead of showing cells
	/// (`todo/dash/14` §3).
	Adapter(Adapter),
}

/// What the adapter screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Adapter {
	/// The open channel's bit rate in kbit/s; `None` while the channel is closed.
	pub kbit: Option<u32>,
	/// The channel is open listen-only: nothing is acknowledged and nothing sent.
	pub listen_only: bool,
	/// Frames taken off the bus.
	pub rx: u32,
	/// Frames the host put on the bus that completed.
	pub tx: u32,
	/// What went wrong: frames the ring had no room for, transmits refused,
	/// controller faults.
	pub errors: u32,
}
