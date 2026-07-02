//! Parser for Ross-Tech VCDS plaintext `.lbl` label files.
//!
//! `.lbl` files are ISO-8859-1, CRLF-terminated, `;`-commented. Each content line
//! is a comma-separated record whose first field selects the record kind:
//! a numeric block id (a measuring-value label), `REDIRECT`, `A###` (adaptation
//! channel), `LC` (long coding), or others we keep generically.
//!
//! The compiled/encrypted `.clb` sibling format is NOT handled here (see the crate
//! README / follow-up notes): it is a fixed-keystream-XOR container that needs its
//! own reverse-engineering pass. Most MQB-era labels ship only as `.clb`.

use serde::Serialize;

/// One parsed label file.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabelFile {
    /// Source file name (basename), e.g. `06F-906-056-AXW.lbl`.
    pub source: String,
    pub records: Vec<Record>,
}

/// A single label record.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum Record {
    /// A measuring-value label: `block,field,name[,location[,description]]`.
    Measurement(Measurement),
    /// `REDIRECT,target_file,selector` — points to the label file that applies
    /// to a given ECU part number / coding.
    Redirect {
        target: String,
        selector: Option<String>,
        comment: Option<String>,
    },
    /// `A###,index,name[,location[,description]]` — an adaptation channel label.
    Adaptation {
        channel: String,
        index: String,
        name: String,
        location: String,
        description: String,
    },
    /// `LC,byte,bits,value,meaning` — a long-coding bit-field label.
    LongCoding {
        byte: String,
        bits: String,
        value: String,
        meaning: String,
    },
    /// Any other record kind, kept verbatim so nothing is silently dropped.
    Other {
        tag: String,
        fields: Vec<String>,
        comment: Option<String>,
    },
}

/// A measuring value: an ECU measurement identified by (block, field).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    pub block: u16,
    pub field: u8,
    pub name: String,
    pub location: String,
    pub description: String,
    /// Unit parsed from the description's `Range:` clause, if any (e.g. `RPM`, `°C`, `%`).
    pub unit: Option<String>,
    /// Numeric `[min, max]` parsed from the description's `Range:` clause, if any.
    pub range: Option<[f64; 2]>,
}

/// Decode ISO-8859-1 (Latin-1) bytes: each byte maps directly to its code point.
fn decode_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Split a line into content and an optional inline comment. A `;` starts a
/// comment when it is at the line start or preceded by whitespace (Ross-Tech
/// inline comments are written as `... ; text`); this leaves semicolons that sit
/// inside a value untouched.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let bytes = line.as_bytes();
    let mut prev_ws = true; // start-of-line counts as "preceded by whitespace"
    for (i, &b) in bytes.iter().enumerate() {
        if b == b';' && prev_ws {
            let content = line[..i].trim_end();
            let comment = line[i + 1..].trim();
            return (content, if comment.is_empty() { None } else { Some(comment) });
        }
        prev_ws = (b as char).is_whitespace();
    }
    (line.trim_end(), None)
}

/// Read a leading signed decimal number, returning the value and the remaining text.
fn parse_leading_number(s: &str) -> Option<(f64, &str)> {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let start_digits = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == start_digits {
        return None; // no digits consumed
    }
    let value: f64 = s[..i].parse().ok()?;
    Some((value, &s[i..]))
}

/// Extract `(unit, [min, max])` from a measurement description's `Range:` clause.
/// Returns `(None, None)` for non-numeric ranges (enumerations like `1/2/3H/...`).
fn extract_range_unit(description: &str) -> (Option<String>, Option<[f64; 2]>) {
    let Some(idx) = description.find("Range:") else {
        return (None, None);
    };
    let after = &description[idx + "Range:".len()..];
    // Descriptions use a literal backslash-n as a line break; a Range clause ends there.
    let segment = after.split("\\n").next().unwrap_or(after);
    let Some((lhs, rhs)) = segment.split_once("...") else {
        return (None, None);
    };
    let Some((min, _)) = parse_leading_number(lhs) else {
        return (None, None);
    };
    let Some((max, rest)) = parse_leading_number(rhs) else {
        return (None, None);
    };
    let unit = rest.trim();
    let unit = if unit.is_empty() {
        None
    } else {
        Some(unit.to_string())
    };
    (unit, Some([min, max]))
}

/// The verbatim substring of `content` following the `n`-th comma (0-indexed:
/// `n == 1` is everything after the first comma). Preserves free-text exactly,
/// including embedded commas and internal spacing. Returns `""` if there are
/// fewer than `n` commas.
fn tail_after_comma(content: &str, n: usize) -> String {
    let mut seen = 0;
    for (i, b) in content.bytes().enumerate() {
        if b == b',' {
            seen += 1;
            if seen == n {
                return content[i + 1..].to_string();
            }
        }
    }
    String::new()
}

/// Parse a single content line (comment already stripped) into a record.
fn parse_record(content: &str, comment: Option<&str>) -> Record {
    let fields: Vec<String> = content.split(',').map(|f| f.trim().to_string()).collect();
    let tag = fields[0].clone();
    let get = |i: usize| fields.get(i).cloned().unwrap_or_default();
    // Free-text tails (description, coding meaning) are taken verbatim from the
    // original line so embedded commas AND internal spacing survive exactly;
    // the short structured fields before them are still trimmed.
    let tail = |n: usize| tail_after_comma(content, n);

    if tag.chars().all(|c| c.is_ascii_digit()) && !tag.is_empty() {
        let block = tag.parse().unwrap_or(0);
        let field = get(1).parse().unwrap_or(0);
        let name = get(2);
        let location = get(3);
        let description = tail(4);
        let (unit, range) = extract_range_unit(&description);
        return Record::Measurement(Measurement {
            block,
            field,
            name,
            location,
            description,
            unit,
            range,
        });
    }

    match tag.as_str() {
        "REDIRECT" => Record::Redirect {
            target: get(1),
            selector: fields.get(2).filter(|s| !s.is_empty()).cloned(),
            comment: comment.map(|c| c.to_string()),
        },
        "LC" => Record::LongCoding {
            byte: get(1),
            bits: get(2),
            value: get(3),
            meaning: tail(4),
        },
        // Adaptation channels: A followed by three digits.
        t if t.len() == 4
            && t.starts_with('A')
            && t[1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            Record::Adaptation {
                channel: tag.clone(),
                index: get(1),
                name: get(2),
                location: get(3),
                description: tail(4),
            }
        }
        _ => Record::Other {
            tag: tag.clone(),
            fields: fields[1..].to_vec(),
            comment: comment.map(|c| c.to_string()),
        },
    }
}

/// Parse the raw bytes of a `.lbl` file into a [`LabelFile`].
pub fn parse_label(source: impl Into<String>, bytes: &[u8]) -> LabelFile {
    let text = decode_latin1(bytes);
    let mut records = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.trim().is_empty() {
            continue;
        }
        let (content, comment) = split_comment(line);
        if content.is_empty() {
            continue; // full-line comment
        }
        records.push(parse_record(content, comment));
    }
    LabelFile {
        source: source.into(),
        records,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurements(lf: &LabelFile) -> Vec<&Measurement> {
        lf.records
            .iter()
            .filter_map(|r| match r {
                Record::Measurement(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_measurement_with_range_and_unit() {
        let lf = parse_label("t.lbl", b"001,1,Engine Speed,(G28),Range: 0...6500 RPM\\nSpec: 640...800 RPM");
        let m = measurements(&lf)[0];
        assert_eq!(m.block, 1);
        assert_eq!(m.field, 1);
        assert_eq!(m.name, "Engine Speed");
        assert_eq!(m.location, "(G28)");
        assert_eq!(m.unit.as_deref(), Some("RPM"));
        assert_eq!(m.range, Some([0.0, 6500.0]));
    }

    #[test]
    fn parses_signed_temperature_range() {
        let lf = parse_label("t.lbl", b"001,2,Coolant,Temperature (G62),Range: -48.0...+143.0 \xb0C");
        let m = measurements(&lf)[0];
        assert_eq!(m.range, Some([-48.0, 143.0]));
        assert_eq!(m.unit.as_deref(), Some("°C")); // 0xB0 in Latin-1 decodes to °
    }

    #[test]
    fn enumeration_range_has_no_numeric_range_or_unit() {
        let lf = parse_label("t.lbl", b"001,4,Engaged Gear,,Range: 1/2/3H/3M/4H/4M");
        let m = measurements(&lf)[0];
        assert_eq!(m.range, None);
        assert_eq!(m.unit, None);
    }

    #[test]
    fn description_with_comma_is_preserved() {
        let lf = parse_label("t.lbl", b"005,0,Lever position,P/N,Back-up,T15");
        let m = measurements(&lf)[0];
        assert_eq!(m.name, "Lever position");
        assert_eq!(m.location, "P/N");
        assert_eq!(m.description, "Back-up,T15");
    }

    #[test]
    fn description_preserves_comma_space_and_internal_spacing() {
        // Free-text tail is taken verbatim: an embedded ", " keeps its space,
        // and leading space after the location comma is not trimmed away.
        let lf = parse_label("t.lbl", b"010,1,Mode,, 0 = Off, 1 = On");
        let m = measurements(&lf)[0];
        assert_eq!(m.name, "Mode");
        assert_eq!(m.location, "");
        assert_eq!(m.description, " 0 = Off, 1 = On");
    }

    #[test]
    fn parses_redirect_with_inline_comment() {
        let lf = parse_label(
            "t.lbl",
            b"REDIRECT,077-910-560-BFMS.CLB,006-410-010-A0  ; BFM (Spyker C8 MY 2005)",
        );
        match &lf.records[0] {
            Record::Redirect { target, selector, comment } => {
                assert_eq!(target, "077-910-560-BFMS.CLB");
                assert_eq!(selector.as_deref(), Some("006-410-010-A0"));
                assert_eq!(comment.as_deref(), Some("BFM (Spyker C8 MY 2005)"));
            }
            other => panic!("expected Redirect, got {other:?}"),
        }
    }

    #[test]
    fn parses_adaptation_and_long_coding() {
        let lf = parse_label(
            "t.lbl",
            b"A003,1,Engine Speed,, 860...945 RPM\nLC,00,0~7,01,Manufacturer: Audi",
        );
        assert!(matches!(&lf.records[0], Record::Adaptation { channel, .. } if channel == "A003"));
        match &lf.records[1] {
            Record::LongCoding { byte, bits, value, meaning } => {
                assert_eq!(byte, "00");
                assert_eq!(bits, "0~7");
                assert_eq!(value, "01");
                assert_eq!(meaning, "Manufacturer: Audi");
            }
            other => panic!("expected LongCoding, got {other:?}"),
        }
    }

    #[test]
    fn skips_comment_and_blank_lines() {
        let lf = parse_label("t.lbl", b"; a header comment\r\n\r\n001,1,Speed\r\n");
        assert_eq!(lf.records.len(), 1);
    }
}
