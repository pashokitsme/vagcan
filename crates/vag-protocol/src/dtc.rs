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
/// |  |   |     |       |     free-running seconds counter
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
/// * **The counter runs at 1 Hz.** During a survey the units were read
///   seconds apart and their counters differed by exactly those seconds; a
///   read 9.4 hours later differed by 33 756 counts.
///
/// What is **not** established is the counter's epoch. Read as a Unix
/// timestamp it lands about 92 days before the reading — either the car's
/// clock is wrong or the epoch is not 1970, and nothing here distinguishes
/// those. So this type keeps the raw counter, and callers should express age
/// *relative* to the same counter read from the same car, which needs no
/// epoch at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FaultContext {
    pub priority: u8,
    /// Occurrences, saturating: `0xFF` means "at least 255".
    pub occurrences: u8,
    pub cycle_counter: u16,
    pub mileage_km: u32,
    /// The car's own clock: a day counter in the high 16 bits and seconds
    /// since midnight in the low 16. See [`FaultContext::seconds_between`].
    pub clock: u32,
}

/// Split a car-clock value into its day counter and its second of the day.
///
/// **Evidence for the split.** The reference car's brake unit stores fault 297
/// with clock `0x69F60003`, and the car's own VCDS scan dates that fault
/// `2026.07.27 00:00:03` — the low half is exactly 3, the second of the day.
/// A 32-bit seconds counter would put an arbitrary value there; landing on the
/// scan's own seconds by chance is a 1-in-65 536 coincidence.
///
/// **What is not established** is the day counter's epoch: `0x69F6` is some
/// day in late July 2026 and nothing here says which numbering that is. So
/// this is used for *differences*, never to print a date.
pub fn split_clock(clock: u32) -> (u16, u16) {
    ((clock >> 16) as u16, (clock & 0xFFFF) as u16)
}

/// How many of the clock's days lie between two readings.
///
/// **This is not a count of calendar days.** On the reference car the brake
/// unit's fault is dated 2026-07-27 by VCDS and the clock has advanced four
/// days since, while six calendar days have passed — the two the car sat
/// unused did not count. So a date computed by subtracting these from today is
/// an **upper bound**: the event happened on or before it, possibly earlier.
pub fn days_between(earlier: u32, later: u32) -> Option<u16> {
    let (day_a, _) = split_clock(earlier);
    let (day_b, _) = split_clock(later);
    day_b.checked_sub(day_a)
}

/// Seconds between two car-clock readings.
///
/// A day is 86 400 seconds but only advances the counter's high half by one,
/// so subtracting the raw values loses 20 864 seconds per day crossed — which
/// made a fault stored yesterday look 18 hours old instead of 24.
pub fn seconds_between(earlier: u32, later: u32) -> Option<i64> {
    let (day_a, sec_a) = split_clock(earlier);
    let (day_b, sec_b) = split_clock(later);
    let days = i64::from(day_b) - i64::from(day_a);
    Some(days * 86_400 + i64::from(sec_b) - i64::from(sec_a)).filter(|s| *s >= 0)
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
/// Every unit that answers it returns `91 <mileage:3> <2 bytes> <clock:4>` —
/// the tail of a fault record without the fault. Reading it gives the *now*
/// against which a stored fault's counter becomes an age, with no epoch
/// involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitStamp {
    pub mileage_km: u32,
    pub clock: u32,
}

impl UnitStamp {
    /// Identifier holding it.
    pub const DID: u16 = 0x02BD;

    pub fn parse(data: &[u8]) -> Option<UnitStamp> {
        if data.len() < 10 {
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
        0x06, 0x09, 0x02, 0xB8, 0x03, 0x3F, 0x1B, 0x00, 0x00, 0x69, 0xF9, 0x04, 0x4B, 0x70,
        0x04, 0x04, 0x0A, 0x7B, 0x91,
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
    fn the_clock_is_a_day_counter_and_a_second_of_the_day() {
        // The anchor: this car's VCDS scan dates fault 297 at 00:00:03 and the
        // unit stores 0x69F60003 for it.
        assert_eq!(split_clock(0x69F6_0003), (0x69F6, 3));

        // Two records on the same day are simply their difference apart …
        assert_eq!(seconds_between(0x69F6_0003, 0x69F6_F7B1), Some(63_406));
        // … and one day later is a full day, not the 44 672 that subtracting
        // the raw values would give.
        assert_eq!(seconds_between(0x69F6_0003, 0x69F7_0003), Some(86_400));
        // A reading earlier than the fault means the two are not comparable.
        assert_eq!(seconds_between(0x69F7_0003, 0x69F6_0003), None);
    }

    #[test]
    fn the_live_stamp_carries_the_same_odometer_the_cluster_reports() {
        // 02BD read from the body control module, verbatim.
        let stamp = UnitStamp::parse(&[0x91, 0x03, 0x3F, 0x45, 0x00, 0x00, 0x69, 0xFA, 0x00, 0x5C])
            .unwrap();
        assert_eq!(stamp.mileage_km, 212_805);
        assert_eq!(stamp.clock, 0x69FA_005C);

        // And it is what turns a stored counter into an age without needing to
        // know the epoch: one day and just over 23 hours of it.
        let ctx = FaultContext::parse(&BCM_000107).unwrap();
        assert_eq!(seconds_between(ctx.clock, stamp.clock), Some(85_393));
        assert_eq!(stamp.mileage_km - ctx.mileage_km, 42);
    }
}
