//! Cutting a multi-identifier `ReadDataByIdentifier` answer into its records.
//!
//! Two ways, and the planner picks by what it knows: [`split_by_lengths`] when
//! the plan knows every requested identifier's record length, [`split_records`]
//! (a bounded search for the one unique parse) when it does not. Neither
//! guesses: an answer that does not read exactly one way is `None`.
//!
//! Moved here from `vag-cli-core`'s `analyse.rs` so the board and the laptop
//! split with the same code; `analyse` and `plan::read_batch` call this one.

use alloc::vec::Vec;

/// One response, cut into the identifiers it answered and their bytes.
pub type Records = Vec<(u16, Vec<u8>)>;

/// Split an answer whose record lengths are all known, walking it in request
/// order.
///
/// `dids` is the request, in the order it was sent, each with its record's
/// length in bytes (not counting the two-byte identifier echo). An identifier
/// the unit left out is skipped, as [`split_records`] allows; an echo that is
/// not a later requested identifier, a record running past the end, or bytes
/// left over make the answer `None`: the lengths the plan claims do not
/// describe it.
pub fn split_by_lengths(payload: &[u8], dids: &[(u16, u16)]) -> Option<Records> {
	let mut records = Vec::new();
	let mut at = 0;
	let mut next = 0;
	while at < payload.len() {
		let head = u16::from_be_bytes([*payload.get(at)?, *payload.get(at + 1)?]);
		let offset = dids[next..].iter().position(|(did, _)| *did == head)?;
		let len = usize::from(dids[next + offset].1);
		let body = payload.get(at + 2..at + 2 + len)?;
		records.push((head, body.to_vec()));
		at += 2 + len;
		next += offset + 1;
	}
	Some(records)
}
/// Split a positive `ReadDataByIdentifier` response into per-identifier records.
///
/// The server does not state record lengths, so the only thing marking a
/// boundary is the next identifier's own bytes appearing. Everything here
/// follows from that.
///
/// **An identifier that was asked for need not be in the answer.** A control
/// unit answers a multi-identifier request with only the identifiers it
/// supports — measured on the reference car and recorded in `todo/README.md`,
/// and the reason a sweep can group-test at all. This function used to require
/// all of them, so one unsupported identifier in a batch discarded the whole
/// response. That is not a hypothetical: `measure` asks the engine unit for the
/// mass air flow (`F410`, standard PID 10), which the reference car does not
/// implement, and so lost engine speed, both boost channels and the air mass
/// together, on every cycle of every run. Eleven saved sessions carry
/// `engine_speed: 0 points` for that reason and nothing else.
///
/// **What is still refused is a guess.** Among the ways of reading a response,
/// the one accounting for the most identifiers is the better explanation and
/// wins; if two readings account for equally many, the response genuinely reads
/// two ways and `None` is returned. A mis-split attributes one measurement's
/// bytes to another identifier, which is worse than having no sample.
pub fn split_records(payload: &[u8], dids: &[u16]) -> Option<Records> {
	split_within(payload, dids, SPLIT_BUDGET).0
}

/// The split, and how much of the budget was left when it finished.
///
/// The remainder is what the guard is tested through. Timing the call instead
/// would assert on how loaded the machine is: the same search that takes a
/// millisecond alone takes far longer when the rest of the suite is running
/// beside it, and a test that fails for that reason teaches nothing.
fn split_within(payload: &[u8], dids: &[u16], budget: u32) -> (Option<Records>, u32) {
	let mut best = Best { budget, ..Best::default() };
	parse_from(payload, dids, 0, &mut Vec::new(), &mut best);
	let records = match (best.budget, best.ties) {
		// Out of budget is not "no reading" — it is a response this function
		// did not finish examining, and calling that a unique parse would ship
		// whichever reading it happened to find first.
		(0, _) => None,
		(_, 1) => best.records(payload),
		_ => None,
	};
	(records, best.budget)
}

/// How many placements the split may try before giving up on a response.
///
/// The search is small on anything a control unit actually sends — eight
/// identifiers, a few hundred bytes, and a header occurring in a handful of
/// places. It is not small in the worst case: a payload whose bytes look like
/// identifier headers throughout branches at every position of every level. The
/// caller is a poll loop timing an acceleration run, so an unbounded search is
/// a stall on a moving car; this bounds it at a cost far above any real
/// response and well below anything a driver would notice.
///
/// The number comes from both ends. A real response branches two or three ways
/// per identifier — a header rarely occurs inside another record's data — so
/// eight of them cost a few thousand placements at most. A placement itself is
/// a push of two integers, because the search carries byte *ranges* and only
/// the winning parse is ever copied out; that keeps the ceiling in the
/// low milliseconds, well inside one cycle of the poll loop.
const SPLIT_BUDGET: u32 = 20_000;

/// The best reading of a response so far, and whether anything ties with it.
///
/// Kept as a running maximum rather than a list of every parse: the search is
/// bounded but not tiny, and only the top rank and the size of its tie can
/// change the answer.
#[derive(Default)]
struct Best {
	/// Where the held parse cut the payload, as `(identifier, start, end)`.
	/// Ranges rather than bytes: the search offers a candidate at every leaf it
	/// reaches, and copying each one out made the cost of a placement scale
	/// with the size of the response — which is exactly backwards for a
	/// pathological payload, the case the budget exists to survive.
	parse: Option<Vec<(u16, usize, usize)>>,
	/// How many identifiers the held parse places. A parse placing more
	/// replaces it outright.
	placed: usize,
	/// How many parses place exactly `placed` identifiers. Anything but one
	/// means the response does not read a single way.
	ties: usize,
	/// Placements left before the search gives up. See [`SPLIT_BUDGET`].
	budget: u32,
}

impl Best {
	fn offer(&mut self, parse: &[(u16, usize, usize)]) {
		match parse.len().cmp(&self.placed) {
			core::cmp::Ordering::Greater => {
				self.placed = parse.len();
				self.parse = Some(parse.to_vec());
				self.ties = 1;
			}
			core::cmp::Ordering::Equal => self.ties += 1,
			core::cmp::Ordering::Less => {}
		}
	}

	/// The held parse with its bytes, once the search has finished and there is
	/// exactly one of it.
	fn records(&self, payload: &[u8]) -> Option<Records> {
		Some(
			self
				.parse
				.as_ref()?
				.iter()
				.map(|(did, from, to)| (*did, payload[*from..*to].to_vec()))
				.collect(),
		)
	}
}

/// Enumerate every self-consistent way to cut `payload` into records.
///
/// Two choices are open at each identifier: the unit answered it, and its
/// record starts here; or the unit did not implement it and there is nothing to
/// place. The second is what makes a partial answer readable.
///
/// A record's end is only ever a position where **some still-unplaced
/// identifier's** header sits, or the end of the payload — which is what keeps
/// the search cheap. Trying every byte position instead would be exponential in
/// the payload length rather than in the handful of places a header occurs, and
/// those bytes do occur inside a record's data by coincidence, which
/// little-endian gearbox values do readily.
///
/// At most eight identifiers per request and a few hundred bytes.
fn parse_from(payload: &[u8], dids: &[u16], at: usize, prefix: &mut Vec<(u16, usize, usize)>, best: &mut Best) {
	match best.budget.checked_sub(1) {
		Some(left) => best.budget = left,
		None => return,
	}
	let Some((&did, rest)) = dids.split_first() else {
		// Every identifier accounted for — placed or absent — and the payload
		// fully consumed. A parse that leaves bytes over has mis-read one of
		// the records it did place.
		if at == payload.len() {
			best.offer(prefix);
		}
		return;
	};

	// The unit did not answer this one. Nothing is consumed, and the next
	// identifier is tried at the same position.
	parse_from(payload, rest, at, prefix, best);

	let head = [(did >> 8) as u8, (did & 0xFF) as u8];
	if payload.get(at..at + 2) != Some(&head[..]) {
		return;
	}
	let body = at + 2;

	let heads: Vec<[u8; 2]> = rest.iter().map(|d| [(*d >> 8) as u8, (*d & 0xFF) as u8]).collect();
	let boundaries = (body..payload.len().saturating_sub(1))
		.filter(|i| heads.iter().any(|h| payload[*i..*i + 2] == *h))
		// The record can also be the last one in the response, whether or not
		// it is the last one requested.
		.chain(core::iter::once(payload.len()));
	for end in boundaries {
		prefix.push((did, body, end));
		parse_from(payload, rest, end, prefix, best);
		prefix.pop();
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec;

	#[test]
	fn known_lengths_split_an_answer_the_search_cannot() {
		// `F187`'s header sits inside `F186`'s record, so without lengths the
		// answer reads two ways and the search refuses it. The lengths say which.
		let payload = [0xF1, 0x86, 0xF1, 0x87, 0x01, 0xF1, 0x87, 0x02];
		assert_eq!(split_records(&payload, &[0xF186, 0xF187]), None);
		assert_eq!(
			split_by_lengths(&payload, &[(0xF186, 3), (0xF187, 1)]),
			Some(vec![(0xF186, vec![0xF1, 0x87, 0x01]), (0xF187, vec![0x02])])
		);
	}

	#[test]
	fn known_lengths_skip_an_identifier_the_unit_left_out() {
		let payload = [0xF1, 0x86, 0x01, 0xF1, 0x89, b'0', b'5'];
		assert_eq!(
			split_by_lengths(&payload, &[(0xF186, 1), (0xF187, 2), (0xF189, 2)]),
			Some(vec![(0xF186, vec![0x01]), (0xF189, b"05".to_vec())])
		);
	}

	#[test]
	fn lengths_that_do_not_describe_the_answer_split_nothing() {
		let payload = [0xF1, 0x86, 0x01, 0x02, 0xF1, 0x87, 0x03];
		// Too short for `F186`: the next echo is not an identifier.
		assert_eq!(split_by_lengths(&payload, &[(0xF186, 1), (0xF187, 1)]), None);
		// Too long: the last record runs past the end.
		assert_eq!(split_by_lengths(&payload, &[(0xF186, 2), (0xF187, 2)]), None);
		// Out of request order.
		assert_eq!(split_by_lengths(&payload, &[(0xF187, 1), (0xF186, 2)]), None);
	}

	#[test]
	fn a_single_identifier_response_splits_trivially() {
		assert_eq!(split_records(&[0xF1, 0x90, b'X', b'W'], &[0xF190]), Some(vec![(0xF190, b"XW".to_vec())]));
		// A response echoing a different identifier is not ours.
		assert_eq!(split_records(&[0xF1, 0x91, b'X'], &[0xF190]), None);
	}

	#[test]
	fn a_multi_identifier_response_splits_on_the_requested_order() {
		// Exactly the shape the car returns for a batched read.
		let payload = [0xF1, 0x86, 0x01, 0xF1, 0x87, b'8', b'V', 0xF1, 0x89, b'0', b'5'];
		let split = split_records(&payload, &[0xF186, 0xF187, 0xF189]).unwrap();
		assert_eq!(split, vec![(0xF186, vec![0x01]), (0xF187, b"8V".to_vec()), (0xF189, b"05".to_vec()),]);
	}

	#[test]
	fn an_identifier_the_unit_does_not_support_costs_only_itself() {
		// A control unit answers a multi-identifier request with **only the
		// identifiers it supports** — established on this car and recorded in
		// `todo/README.md`: asking for `F190` together with something the unit
		// does not implement returns just `F190`.
		//
		// This used to return `None`, which threw the whole response away. It
		// cost every telemetry channel on the engine unit for eleven recorded
		// sessions: `measure` asks it for the mass air flow (`F410`, standard
		// PID 10) which this car does not implement, so all five identifiers in
		// that batch — engine speed included — were discarded every cycle, and
		// every saved run carries `engine_speed: 0 points`.
		let payload = [0xF1, 0x90, b'X', b'W'];
		assert_eq!(
			split_records(&payload, &[0xF190, 0xF187]),
			Some(vec![(0xF190, b"XW".to_vec())]),
			"the identifier that was answered is still readable"
		);

		// The omission can fall anywhere in the request, including first.
		let payload = [0xF1, 0x87, b'8', b'V'];
		assert_eq!(split_records(&payload, &[0xF190, 0xF187]), Some(vec![(0xF187, b"8V".to_vec())]));

		// And in the middle of three, which is the shape that loses a whole
		// batch: the two that answered still split on the requested order.
		let payload = [0xF1, 0x86, 0x01, 0xF1, 0x89, b'0', b'5'];
		assert_eq!(
			split_records(&payload, &[0xF186, 0xF187, 0xF189]),
			Some(vec![(0xF186, vec![0x01]), (0xF189, b"05".to_vec())])
		);
	}

	#[test]
	fn the_engine_batch_that_measure_actually_sends_splits() {
		// The exact request `measure` makes of the engine unit on the reference
		// car: boost specified, boost actual, engine speed, road speed, and the
		// mass air flow — standard PID 10, which this car does not implement
		// and never answers. Everything but the last is in the response.
		//
		// Values from a saved session: 1.15 bar specified as 0x047E, 1.13 bar
		// actual as 0x0468, 4284 /min as 0x10BC, 87 km/h as 0x57.
		let dids = [0x2029, 0x202A, 0x206E, 0xF40D, 0xF410];
		let payload = [
			0x20, 0x29, 0x04, 0x7E, //
			0x20, 0x2A, 0x04, 0x68, //
			0x20, 0x6E, 0x10, 0xBC, //
			0xF4, 0x0D, 0x57,
		];
		assert_eq!(
			split_records(&payload, &dids),
			Some(vec![
				(0x2029, vec![0x04, 0x7E]),
				(0x202A, vec![0x04, 0x68]),
				(0x206E, vec![0x10, 0xBC]),
				(0xF40D, vec![0x57]),
			]),
			"this is the read that produced `engine_speed: 0 points` in eleven sessions"
		);
	}

	#[test]
	fn a_response_that_genuinely_reads_two_ways_is_still_refused() {
		// Tolerating an unanswered identifier must not become tolerating a
		// guess. Here `F187`'s header occurs twice, so there are two ways to
		// place it and both place the same number of identifiers — nothing
		// distinguishes them, and a mis-split attributes one measurement's
		// bytes to another.
		let payload = [0xF1, 0x86, 0xF1, 0x87, 0x01, 0xF1, 0x87, 0x02];
		assert_eq!(split_records(&payload, &[0xF186, 0xF187]), None);
	}

	#[test]
	fn a_payload_built_to_explode_the_search_returns_rather_than_hangs() {
		// Every byte pair in this payload is one of the requested headers, so
		// every position of every level branches. The caller is the poll loop
		// of a stopwatch running on a moving car: the guarantee that matters is
		// that this returns, and that it does not call whatever it found first
		// a unique reading.
		let dids: Vec<u16> = (0..8).map(|i| 0xF100 + i).collect();
		let payload: Vec<u8> = (0..300).flat_map(|i| [0xF1u8, (i % 8) as u8]).collect();
		let (records, left) = split_within(&payload, &dids, SPLIT_BUDGET);
		assert_eq!(left, 0, "the search finished, so the guard was never the reason it stopped");
		assert_eq!(records, None, "and giving up reports no reading, not the first one found");

		// The same payload with a budget it can afford returns the same answer
		// by exhausting the search rather than by running out — the two paths
		// to `None` are different and only one of them is the guard.
		let short: Vec<u16> = dids[..2].to_vec();
		let (_, left) = split_within(&payload[..8], &short, SPLIT_BUDGET);
		assert!(left > 0, "a real response must never come near the ceiling");
	}

	#[test]
	fn the_parse_that_places_the_most_identifiers_wins() {
		// `F187`'s header also appears inside `F186`'s data, so the response
		// can be read as one record or as two. Reading it as two accounts for
		// an identifier that was asked for and is therefore the better
		// explanation — and it is the reading this code has always taken. The
		// ranking exists to keep it that way now that a short parse is legal
		// at all.
		let payload = [0xF1, 0x86, 0xF1, 0x87, 0x01];
		assert_eq!(
			split_records(&payload, &[0xF186, 0xF187]),
			Some(vec![(0xF186, vec![]), (0xF187, vec![0x01])])
		);
	}
}
