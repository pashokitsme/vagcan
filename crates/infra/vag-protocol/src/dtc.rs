use alloc::vec::Vec;

/// One DTC entry as returned by ReadDTCInformation subfunction 0x02:
/// 3 raw code bytes + 1 status byte. Semantic decoding happens in vag-data (P2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDtc {
	pub code: [u8; 3],
	pub status: u8,
}

/// A snapshot ("freeze frame") stored with a fault: the record number the unit
/// filed it under, and the bytes it captured at that moment.
///
/// The layout of `data` is defined per control unit by its own label file, not
/// by the standard, so this type deliberately keeps the bytes raw. What is
/// standard is the framing — record number, then a count of
/// identifier/value pairs — and that much is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtcSnapshot {
	pub record: u8,
	/// The `(identifier, bytes)` pairs the unit captured, in the order given.
	pub values: Vec<(u16, Vec<u8>)>,
}

/// Extended data stored with a fault — on VW units this is where "how many
/// times" and "how long ago" live.
///
/// As with snapshots, the meaning of each record number is per-unit; the
/// framing is standard and that is what is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DtcExtendedData {
	pub record: u8,
	pub data: Vec<u8>,
}

/// When a fault happened, as the control unit recorded it.
///
/// This is extended-data record `0x01`, which every unit on the reference car
/// answers with the same layout:
///
/// ```text
/// 06 09  02B8  033F1B  0000  69F9044B
/// ^  ^   ^     ^       ^     ^
/// |  |   |     |       |     the car's own date and time, packed — see CarTime
/// |  |   |     |       two bytes, zero in every sample seen
/// |  |   |     odometer, km, u24 big-endian
/// |  |   a counter that rises with time, shared across units
/// |  how many times it happened (saturates at 0xFF)
/// priority
/// ```
///
/// Evidence for the two fields that matter, from 17 stored faults across six
/// control units:
///
/// * **Mileage.** No fault's value exceeds the instrument cluster's odometer
///   (`0x033F45` = 212 805 km when this was written), the newest faults equal
///   it exactly, and older faults order the same way by mileage and by
///   counter. A three-byte field landing on the odometer in every one of 17
///   records is not available to another reading.
/// * **The clock is a date, not a tally.** See [`CarTime`]; the same field is
///   answered live at [`UnitStamp::DID`], where it decodes to the instrument
///   cluster's own displayed date and time.
///
/// **This field looked like a free-running 1 Hz counter, and it is not.** The
/// appearance comes from subtracting two raw stamps: the seconds field is six
/// bits but wraps at 60, so a raw difference overshoots the elapsed time by 4
/// per minute boundary crossed, 256 per hour and 32 768 per day. Across seven
/// units between the two driving sweeps the raw differences were 528–533 where
/// the elapsed time was 496–497 s — a 6–7 % overshoot that a counter cannot
/// explain and that this layout predicts exactly, unit by unit (pinned in
/// [`tests`]). Take differences with [`seconds_between`], never on the raw
/// values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultContext {
	pub priority: u8,
	/// Occurrences, saturating: `0xFF` means "at least 255".
	pub occurrences: u8,
	pub cycle_counter: u16,
	pub mileage_km: u32,
	/// The car's own date and time, packed — see [`CarTime`].
	pub clock: u32,
}

/// The car's own date and time, as a control unit stamps a fault with it.
///
/// The 32 bits are packed, most significant first: **6 bits year (from 2000),
/// 4 month, 5 day, 5 hour, 6 minute, 6 second**.
///
/// Established against two independent VCDS printouts of this car, each
/// reproduced to the second: `0x69F60003` → `2026.07.27 00:00:03` on the brake
/// unit, and `0x69F97C82` → `2026.07.28 23:50:02` on the steering assist. Two
/// exact hits over six fields is not something a wrong layout produces.
///
/// This replaces an earlier reading of the same field as a day counter plus a
/// second of the day. That fitted the first anchor — whose time is three
/// seconds past midnight, where the two layouts agree — and nothing else: it
/// reads the second anchor as `08:51:14` where VCDS printed `23:50:02`.
///
/// **The same layout holds at [`UnitStamp::DID`] (`0x02BD`), read live.** The
/// two places were suspected of meaning different things because raw `02BD`
/// differences behave like a counter; they do not (see [`FaultContext`]). Four
/// independent checks, each able to have failed:
///
/// * **The instrument cluster's own clock.** Its `2238`/`2239`/`223A`/`223B`/
///   `223C` are a separate unit's real-time clock, read part-way through each
///   sweep. In all three whole-car sweeps it lands inside the bracket the
///   neighbouring units' `02BD` stamps set — `23:51` between `23:50:17` and
///   `23:52:41`, `03:18` between `03:17:19` and `03:19:44`, `03:26` between
///   `03:25:36` and `03:28:01` — and its year, month and day match the unpacked
///   ones exactly (2026-07-28, then 2026-07-29 twice).
/// * **Sweep order.** Within a sweep the units are read one after another, and
///   the unpacked times rise monotonically in file order, every time.
/// * **Raw differences.** Seven units, two sweeps: predicted exactly, 528 /
///   529 / 533, including which units get which.
/// * **Elapsed wall time.** Two single-unit reads 94.5 s apart by the host
///   clock differ by 94 s unpacked and by 98 raw.
///
/// The clock the units carry is the **car's**, and on this car it runs four
/// days behind real time while keeping the correct time of day: three sweep
/// files were written 4 d + 3.0 s, 4 d + 3.6 s and 4 d + 4.3 s after the last
/// stamp they recorded, the residual being the time to finish and close the
/// file. So a stamp is a real moment, but it is the car's idea of the date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarTime {
	pub year: u16,
	pub month: u8,
	pub day: u8,
	pub hour: u8,
	pub minute: u8,
	pub second: u8,
}

impl CarTime {
	/// Unpack a stamp, or `None` if the fields are not a real date — an
	/// unset or corrupt stamp must not print as a plausible moment.
	pub fn parse(clock: u32) -> Option<CarTime> {
		let time = CarTime {
			year: 2000 + (clock >> 26) as u16,
			month: ((clock >> 22) & 0xF) as u8,
			day: ((clock >> 17) & 0x1F) as u8,
			hour: ((clock >> 12) & 0x1F) as u8,
			minute: ((clock >> 6) & 0x3F) as u8,
			second: (clock & 0x3F) as u8,
		};
		let sane = (1..=12).contains(&time.month) && (1..=31).contains(&time.day) && time.hour < 24 && time.minute < 60 && time.second < 60;
		sane.then_some(time)
	}

	/// Seconds since 2000-01-01, for taking differences without a calendar
	/// library. Days-from-civil, the standard algorithm.
	pub fn epoch_seconds(&self) -> i64 {
		let (y, m) = if self.month <= 2 {
			(self.year as i64 - 1, self.month as i64 + 12)
		} else {
			(self.year as i64, self.month as i64)
		};
		let era = if y >= 0 { y } else { y - 399 } / 400;
		let yoe = y - era * 400;
		let doy = (153 * (m - 3) + 2) / 5 + self.day as i64 - 1;
		let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
		let days = era * 146_097 + doe - 719_468;
		days * 86_400 + self.hour as i64 * 3_600 + self.minute as i64 * 60 + self.second as i64
	}
}

/// Seconds between two car-clock stamps, or `None` if either is not a date or
/// the later one is earlier.
pub fn seconds_between(earlier: u32, later: u32) -> Option<i64> {
	let a = CarTime::parse(earlier)?;
	let b = CarTime::parse(later)?;
	let seconds = b.epoch_seconds() - a.epoch_seconds();
	(seconds >= 0).then_some(seconds)
}

impl FaultContext {
	/// Parse extended-data record `0x01`.
	///
	/// Records shorter than the stamp are rejected rather than padded: a
	/// half-read record would produce a mileage that is simply wrong.
	pub fn parse(data: &[u8]) -> Option<FaultContext> {
		if data.len() < 13 {
			return None;
		}
		Some(FaultContext {
			priority: data[0],
			occurrences: data[1],
			cycle_counter: u16::from_be_bytes([data[2], data[3]]),
			mileage_km: u32::from_be_bytes([0, data[4], data[5], data[6]]),
			clock: u32::from_be_bytes([data[9], data[10], data[11], data[12]]),
		})
	}
}

/// The same "when" stamp, read live from identifier `0x02BD`.
///
/// The units that answer it in this layout return exactly ten bytes,
/// `9x <mileage:3> <2 bytes> <clock:4>` — the tail of a fault record without
/// the fault. Reading it gives the *now* against which a stored fault becomes
/// an age. The `clock` is the same packed [`CarTime`] the fault records carry;
/// the evidence that it is the same in both places is on [`CarTime`].
///
/// **Anything that is not ten bytes is refused rather than read from the
/// front.** The reference car's two door units (`0x74A`, `0x74B`) answer with
/// eleven, and the packed clock inside is offset by seven bits, so a
/// byte-aligned read of their record yields 9 516 020 km and a date in 2013 —
/// both of which the plain sanity checks accept. Refusing an unfamiliar length
/// costs a reading; accepting one costs a wrong date, and this project has
/// retracted three decodings already.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitStamp {
	pub mileage_km: u32,
	pub clock: u32,
}

impl UnitStamp {
	/// Identifier holding it.
	pub const DID: u16 = 0x02BD;

	/// The established record length. Not a magic number: it is the width of
	/// the layout above, and a response of any other width is a layout this
	/// project has not established.
	pub const LEN: usize = 10;

	pub fn parse(data: &[u8]) -> Option<UnitStamp> {
		if data.len() != Self::LEN {
			return None;
		}
		Some(UnitStamp {
			mileage_km: u32::from_be_bytes([0, data[1], data[2], data[3]]),
			clock: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Extended record 0x01 read off the body control module, verbatim.
	const BCM_000107: [u8; 19] = [
		0x06, 0x09, 0x02, 0xB8, 0x03, 0x3F, 0x1B, 0x00, 0x00, 0x69, 0xF9, 0x04, 0x4B, 0x70, 0x04, 0x04, 0x0A, 0x7B, 0x91,
	];

	#[test]
	fn a_fault_records_its_mileage_and_how_often_it_happened() {
		let ctx = FaultContext::parse(&BCM_000107).unwrap();
		assert_eq!(ctx.occurrences, 9);
		assert_eq!(ctx.mileage_km, 212_763);
		assert_eq!(ctx.clock, 0x69F9_044B);
		// The odometer read from the cluster at the same time was 212 805 km,
		// so the fault is 42 km old — and no fault may be newer than the car.
		assert!(ctx.mileage_km <= 212_805);
	}

	#[test]
	fn an_occurrence_count_saturates_rather_than_wrapping() {
		// Read off the steering assist: 0xFF, not a count of 255 exactly.
		let mut data = BCM_000107;
		data[1] = 0xFF;
		assert_eq!(FaultContext::parse(&data).unwrap().occurrences, 0xFF);
	}

	#[test]
	fn a_truncated_record_is_refused_rather_than_padded() {
		// Padding would invent a mileage, which is the one field a reader
		// would act on.
		assert!(FaultContext::parse(&BCM_000107[..12]).is_none());
		assert!(FaultContext::parse(&[]).is_none());
	}

	#[test]
	fn the_clock_is_a_packed_date_and_time() {
		// Both anchors are VCDS printouts of this car, and both are matched to
		// the second across all six fields.
		let brake = CarTime::parse(0x69F6_0003).unwrap();
		assert_eq!(
			(brake.year, brake.month, brake.day, brake.hour, brake.minute, brake.second),
			(2026, 7, 27, 0, 0, 3)
		);
		let steering = CarTime::parse(0x69F9_7C82).unwrap();
		assert_eq!(
			(
				steering.year,
				steering.month,
				steering.day,
				steering.hour,
				steering.minute,
				steering.second
			),
			(2026, 7, 28, 23, 50, 2)
		);

		// A day apart is a day, and the two anchors are 47 hours and change.
		assert_eq!(seconds_between(0x69F6_0003, 0x69F9_7C82), Some(172_199));
		assert_eq!(seconds_between(0x69F9_7C82, 0x69F6_0003), None);
	}

	/// Live `02BD` clocks, verbatim from `research/dumps/survey-parked.jsonl`
	/// and the two `survey-driving-20260802-*.jsonl`, in the order the units
	/// were read. The cluster (`0x714`) does not answer `02BD`; its own
	/// real-time clock identifiers are quoted alongside.
	/// One whole-car sweep: its name, the seven units that answered `02BD` in
	/// the order they were read, and the instrument cluster's own displayed
	/// hour and minute from part-way through it.
	type Sweep = (&'static str, [(&'static str, u32); 7], (u8, u8));

	const SWEEPS: [Sweep; 3] = [
		(
			"parked",
			[
				("710", 0x69F9_7C2F),
				("70A", 0x69F9_7C51),
				("70E", 0x69F9_7C80),
				("712", 0x69F9_7C91),
				("746", 0x69F9_7D29),
				("767", 0x69F9_7D88),
				("773", 0x69F9_7D9B),
			],
			(23, 51), // cluster 2238=0x17, 2239=0x33, read between 712 and 746
		),
		(
			"driving 03:14",
			[
				("710", 0x69FA_33F2),
				("70A", 0x69FA_3413),
				("70E", 0x69FA_3443),
				("712", 0x69FA_3453),
				("746", 0x69FA_34EC),
				("767", 0x69FA_354A),
				("773", 0x69FA_355D),
			],
			(3, 18), // cluster 2238=0x03, 2239=0x12
		),
		(
			"driving 03:22",
			[
				("710", 0x69FA_3607),
				("70A", 0x69FA_3624),
				("70E", 0x69FA_3654),
				("712", 0x69FA_3664),
				("746", 0x69FA_3701),
				("767", 0x69FA_375B),
				("773", 0x69FA_376D),
			],
			(3, 26), // cluster 2238=0x03, 2239=0x1A
		),
	];

	#[test]
	fn the_live_stamp_at_02bd_is_the_same_packed_date_the_cluster_shows() {
		// The instrument cluster keeps its own real-time clock and was read
		// part-way through each sweep, between units 712 and 746. If 02BD were
		// anything but this date, its unpacked minute would not bracket the
		// cluster's — that is the check that could have failed.
		for (name, units, (hour, minute)) in SWEEPS {
			let before = CarTime::parse(units[3].1).unwrap(); // 712
			let after = CarTime::parse(units[4].1).unwrap(); // 746
			let cluster = (hour as i64) * 60 + minute as i64;
			let at = |t: &CarTime| (t.hour as i64) * 60 + t.minute as i64;
			assert!(
				at(&before) <= cluster && cluster <= at(&after),
				"{name}: cluster {hour:02}:{minute:02} outside {:02}:{:02}..{:02}:{:02}",
				before.hour,
				before.minute,
				after.hour,
				after.minute
			);
			// And the units are read in order, so their stamps must rise.
			for pair in units.windows(2) {
				let (a, b) = (pair[0], pair[1]);
				assert!(
					seconds_between(a.1, b.1).is_some_and(|s| s > 0),
					"{name}: {} then {} did not advance",
					a.0,
					b.0
				);
			}
		}
	}

	#[test]
	fn raw_differences_overshoot_exactly_as_a_packed_field_must() {
		// This is what made 02BD look like a counter. Between the two driving
		// sweeps the elapsed time was 496–497 s and the raw differences were
		// 528–533. A 1 Hz counter predicts 496–497 for all seven units; this
		// layout predicts each unit's own value, because the overshoot is 4 per
		// minute boundary the interval crosses and the units cross different
		// numbers of them.
		let (_, first, _) = SWEEPS[1];
		let (_, second, _) = SWEEPS[2];
		for (i, (unit, a)) in first.iter().enumerate() {
			let b = second[i].1;
			let elapsed = seconds_between(*a, b).unwrap();
			let raw = (b - a) as i64;
			let (a_t, b_t) = (CarTime::parse(*a).unwrap(), CarTime::parse(b).unwrap());
			let boundaries = (b_t.hour as i64 * 60 + b_t.minute as i64) - (a_t.hour as i64 * 60 + a_t.minute as i64);
			assert_eq!(raw, elapsed + 4 * boundaries, "unit {unit}");
			assert!((496..=497).contains(&elapsed), "unit {unit}: elapsed {elapsed}");
			assert!((528..=533).contains(&raw), "unit {unit}: raw {raw}");
			assert_ne!(raw, elapsed, "unit {unit}: the two readings must disagree");
		}
	}

	#[test]
	fn a_record_of_an_unfamiliar_length_is_refused_rather_than_read_from_the_front() {
		// The reference car's door units answer 02BD with eleven bytes, and the
		// packed clock inside sits seven bits off a byte boundary. Read as the
		// ten-byte layout, 0x74A gives a date in 2013 and an odometer of
		// 9 516 020 km on a car that has done 212 805 — and CarTime::parse
		// accepts that date, so only the length check stops it.
		let door = [0x00, 0x91, 0x33, 0xF4, 0x50, 0x00, 0x34, 0xFC, 0xBE, 0xA7, 0x00];
		assert_eq!(UnitStamp::parse(&door), None);
		// What it would have produced, spelt out so the cost of relaxing the
		// check is on the record.
		let wrong = u32::from_be_bytes([0x34, 0xFC, 0xBE, 0xA7]);
		assert_eq!(CarTime::parse(wrong).unwrap().year, 2013);
		assert_eq!(u32::from_be_bytes([0, 0x91, 0x33, 0xF4]), 9_516_020);

		// Nine bytes is short of the layout, and eleven is not it either.
		assert_eq!(UnitStamp::parse(&door[..9]), None);
		assert!(UnitStamp::parse(&[0x91, 0x03, 0x3F, 0x45, 0, 0, 0x69, 0xF9, 0x7C, 0x2F]).is_some());
	}

	#[test]
	fn a_stamp_that_is_not_a_date_is_refused() {
		// Month 0 and hour 31 are what an unset or corrupt stamp looks like;
		// printing them as a moment would invent one.
		assert!(CarTime::parse(0x0000_0000).is_none());
		assert!(CarTime::parse(0xFFFF_FFFF).is_none());
	}

	#[test]
	fn the_live_stamp_carries_the_same_odometer_the_cluster_reports() {
		// 02BD read from the body control module, verbatim.
		let stamp = UnitStamp::parse(&[0x91, 0x03, 0x3F, 0x45, 0x00, 0x00, 0x69, 0xFA, 0x00, 0x5C]).unwrap();
		assert_eq!(stamp.mileage_km, 212_805);
		assert_eq!(stamp.clock, 0x69FA_005C);

		// And it is what turns a stamp into an age: 2026-07-28 16:17:11 to
		// 2026-07-29 00:01:28 by the car's clock, seven and three quarter
		// hours.
		let ctx = FaultContext::parse(&BCM_000107).unwrap();
		assert_eq!(seconds_between(ctx.clock, stamp.clock), Some(27_857));
		assert_eq!(stamp.mileage_km - ctx.mileage_km, 42);
	}
}
