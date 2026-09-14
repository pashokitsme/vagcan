//! How much of the bus a frame takes, and the bit rate a stream of frames makes.
//!
//! The board in adapter mode shows the traffic in each direction in kb/s. The controller
//! hands up frames, not bits, so the bits are counted from each frame's fields: the
//! widths ISO 11898-1 gives a classic data or remote frame, plus the interframe space
//! every frame is followed by. **Stuff bits are not counted**: they depend on the bit
//! pattern, and the controller does not report them — so the figure is the nominal
//! length, a lower bound on what the wire carried (up to about a fifth under it).
//!
//! Pure data, no clock: the caller brings the time.

/// Start of frame, 11-bit id, RTR, IDE, r0, 4-bit DLC (19); CRC and its delimiter (16);
/// ACK slot and delimiter (2); end of frame (7). ISO 11898-1, base format.
const BASE_FIELDS: u32 = 44;
/// The same with SRR, IDE, the 18 more id bits, RTR, r1, r0 in the arbitration and
/// control fields (39 in place of 19). ISO 11898-1, extended format.
const EXTENDED_FIELDS: u32 = 64;
/// The intermission that follows every frame. ISO 11898-1.
const INTERMISSION: u32 = 3;

/// The nominal bits a classic CAN frame takes on the bus, interframe space included and
/// stuff bits not. `data_len` is the bytes the frame carries — 0 for a remote frame,
/// whatever its DLC says; more than 8 is taken as 8.
pub fn frame_bits(extended: bool, data_len: usize) -> u32 {
	let fields = if extended { EXTENDED_FIELDS } else { BASE_FIELDS };
	fields + 8 * data_len.min(8) as u32 + INTERMISSION
}

/// A bit rate from a running count of bits, measured over whole windows.
///
/// The count is the caller's and **wraps**: only the difference between two looks at it
/// means anything, so it never has to be reset, and the meter never has to know when it
/// was. The first look only starts a window; until a window has closed the rate is not
/// known, which is `None` — never a zero.
#[derive(Debug, Clone, Copy)]
pub struct BitRate {
	window_ms: u64,
	start: Option<(u64, u32)>,
	last: Option<u32>,
}

impl BitRate {
	/// A meter that says a new rate once `window_ms` have gone by since the last one.
	pub const fn new(window_ms: u64) -> Self {
		BitRate {
			window_ms,
			start: None,
			last: None,
		}
	}

	/// Look at the count `bits` at `now_ms`; the rate over the last closed window, in bits
	/// per second, or `None` while none has closed. A clock that goes backwards starts the
	/// window over.
	pub fn update(&mut self, now_ms: u64, bits: u32) -> Option<u32> {
		match self.start {
			Some((at, from)) if now_ms >= at => {
				let elapsed = now_ms - at;
				if elapsed >= self.window_ms.max(1) {
					let rate = u64::from(bits.wrapping_sub(from)) * 1000 / elapsed;
					self.last = Some(u32::try_from(rate).unwrap_or(u32::MAX));
					self.start = Some((now_ms, bits));
				}
			}
			_ => self.start = Some((now_ms, bits)),
		}
		self.last
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_frame_is_its_fields_its_data_and_the_intermission() {
		assert_eq!(frame_bits(false, 0), 47);
		assert_eq!(frame_bits(false, 8), 111);
		assert_eq!(frame_bits(true, 0), 67);
		assert_eq!(frame_bits(true, 8), 131);
	}

	#[test]
	fn a_length_past_eight_is_eight() {
		assert_eq!(frame_bits(false, 15), frame_bits(false, 8));
	}

	#[test]
	fn nothing_is_known_until_a_window_has_closed() {
		let mut meter = BitRate::new(1000);
		assert_eq!(meter.update(0, 0), None);
		assert_eq!(meter.update(200, 50_000), None);
		assert_eq!(meter.update(999, 90_000), None);
		assert_eq!(meter.update(1000, 100_000), Some(100_000));
	}

	#[test]
	fn a_rate_holds_until_the_next_window_closes() {
		let mut meter = BitRate::new(1000);
		meter.update(0, 0);
		meter.update(1000, 2_000);
		assert_eq!(meter.update(1500, 900_000), Some(2_000));
		assert_eq!(meter.update(2000, 3_000), Some(1_000));
	}

	#[test]
	fn a_window_that_ran_long_is_divided_by_its_real_length() {
		let mut meter = BitRate::new(1000);
		meter.update(0, 0);
		assert_eq!(meter.update(1600, 8_000), Some(5_000));
	}

	#[test]
	fn a_count_that_wrapped_is_still_a_difference() {
		let mut meter = BitRate::new(1000);
		meter.update(0, u32::MAX - 999);
		assert_eq!(meter.update(1000, 24_000), Some(25_000));
	}

	#[test]
	fn a_quiet_bus_is_a_measured_zero() {
		let mut meter = BitRate::new(1000);
		meter.update(0, 7);
		assert_eq!(meter.update(1000, 7), Some(0));
	}

	#[test]
	fn a_clock_that_went_back_starts_the_window_over() {
		let mut meter = BitRate::new(1000);
		meter.update(5000, 0);
		assert_eq!(meter.update(100, 10_000), None);
		assert_eq!(meter.update(1100, 11_000), Some(1_000));
	}
}
