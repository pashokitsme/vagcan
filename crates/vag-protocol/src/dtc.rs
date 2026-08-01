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
