//! What to poll, and in what order — the part that can be tested without a car.
//!
//! One serial port means one conversation at a time, so reading measurements
//! that live on different control units is a sequence of re-addressed groups,
//! not a broadcast. This module decides the grouping; the live loop in the
//! parent module just walks it.
//!
//! A channel is keyed by the unit's **request id**, not by a unit number: the
//! two id blocks on this car have different response rules, and a number is
//! only a display convenience over the id (see `vag_protocol::address`).

use std::collections::BTreeMap;

use vag_data::catalog::{MeasurementDef, ReadId};
use vag_protocol::address::UnitAddress;

/// Identifiers per request. Measured on the reference car: eight are answered,
/// twelve are refused outright, and asking for more than a unit accepts makes
/// every batch look empty rather than erroring.
pub const BATCH: usize = 8;

/// One value the user can put on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    /// Diagnostic request id of the unit that owns it — `0x7E0` engine,
    /// `0x7E1` gearbox, `0x714` instrument cluster.
    pub request: u16,
    pub did: u16,
    /// How to read it, when this project has proven or standardised it.
    /// `None` means the bytes are shown raw.
    pub def: Option<MeasurementDef>,
    pub selected: bool,
}

impl Channel {
    /// How the unit is written on screen: its short number when this project
    /// has established one, otherwise its request id.
    pub fn unit(&self) -> String {
        UnitAddress::from_request(self.request)
            .map(|a| a.label())
            .unwrap_or_else(|| format!("{:03X}", self.request))
    }

    /// Column heading. A known channel uses its name; an unknown one is shown
    /// by address, since there is nothing honest to call it.
    pub fn label(&self) -> String {
        match &self.def {
            Some(d) => d.name.to_string(),
            None => format!("{}/{:04X}", self.unit(), self.did),
        }
    }

    pub fn unit_of_measure(&self) -> &str {
        self.def.as_ref().map(|d| d.unit.as_ref()).unwrap_or("")
    }

    /// What to display for a response body.
    ///
    /// A discrete state shows the state's name; a measured quantity shows its
    /// value; anything else shows its bytes tagged `(raw)`. Never a bare
    /// number for something unproven — a reader cannot tell those apart, and
    /// this project has twice caught itself believing an invented one.
    pub fn render(&self, data: &[u8]) -> String {
        let hex = || data.iter().map(|b| format!("{b:02X}")).collect::<String>();
        let Some(def) = &self.def else {
            return format!("{} (raw)", hex());
        };
        match def.describe(data) {
            Some(text) => text,
            None => format!("{} (raw)", hex()),
        }
    }
}

/// Which half of an actual/specified pair a measurement is.
///
/// A control unit publishes what it *asked for* and what it *got* as two
/// separate identifiers — boost pressure is `0x2029` specified and `0x202A`
/// actual. Read on two screen rows they say much less than side by side: the
/// gap between them is the whole diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Actual,
    Specified,
}

/// Suffixes that mark a measurement as one half of a pair.
///
/// Boost pressure is only the pair this project proved first — a gearbox
/// publishes specified and actual clutch pressure, an engine specified and
/// actual throttle angle, and so on. The corpus writes the distinction several
/// ways, so more than one spelling is recognised; the first two are what this
/// project's own catalogs use.
const ROLE_SUFFIXES: &[(&str, Role)] = &[
    (", actual", Role::Actual),
    (", specified", Role::Specified),
    (", current", Role::Actual),
    (", target", Role::Specified),
    (", requested", Role::Specified),
];

/// Split `"Boost pressure, actual"` into its base name and its role.
///
/// Matching is on the suffix only. A name that merely contains "actual"
/// somewhere is left alone: pairing two unrelated measurements onto one line
/// would present them as a comparison that nobody established.
pub fn split_role(name: &str) -> Option<(&str, Role)> {
    ROLE_SUFFIXES
        .iter()
        .find_map(|(suffix, role)| name.strip_suffix(suffix).map(|base| (base, *role)))
}

/// Request ids of the units this project has proven measurements for.
const ENGINE: u16 = 0x7E0;
const GEARBOX: u16 = 0x7E1;
const CLUSTER: u16 = 0x714;

/// Everything on offer from the catalogs, in a stable order: the standard
/// OBD-II parameters on the engine, then whatever each unit has proven.
///
/// This is what is *known*. Everything else the car answers comes from a
/// survey file — see [`with_survey`] — because a measurement nobody has
/// proven still has bytes worth watching.
pub fn available() -> Vec<Channel> {
    let mut out = Vec::new();
    for p in vag_data::obd::PIDS {
        out.push(Channel {
            request: ENGINE,
            did: vag_data::obd::did_for_pid(p.pid),
            def: Some(p.to_def()),
            selected: false,
        });
    }
    for (request, defs) in [
        (ENGINE, vag_data::catalog::proven_engine()),
        (GEARBOX, vag_data::catalog::proven_gearbox()),
        (CLUSTER, vag_data::catalog::proven_cluster()),
    ] {
        for def in defs {
            let ReadId::Uds(did) = def.address;
            // A control unit's own proven row wins over the standard one at
            // the same address: they can mean different things. F40D is one
            // byte of km/h on the engine and two little-endian bytes on the
            // gearbox.
            if let Some(existing) =
                out.iter_mut().find(|c| c.request == request && c.did == did)
            {
                existing.def = Some(def);
            } else {
                out.push(Channel { request, did, def: Some(def), selected: false });
            }
        }
    }
    out
}

/// Add every identifier a `vagcan survey` run found, on every unit it found
/// them on.
///
/// The survey is the only source that covers the whole car: the catalogs know
/// three units, the gateway lists fifteen more, and none of those fifteen has a
/// proven measurement yet. Their channels come through with no definition, so
/// they display as raw bytes — which is the honest rendering and is also
/// exactly what `vagcan calibrate` needs as input.
///
/// Identifiers already in `channels` keep their definition; a survey never
/// overrides a proven scaling with nothing.
pub fn with_survey(mut channels: Vec<Channel>, survey: &str) -> Vec<Channel> {
    for line in survey.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let Some(request) = value["request"]
            .as_str()
            .and_then(|s| u16::from_str_radix(s, 16).ok())
        else {
            continue;
        };
        let Some(dids) = value["dids"].as_array() else { continue };
        for entry in dids {
            let Some(did) = entry["did"].as_str().and_then(|s| u16::from_str_radix(s, 16).ok())
            else {
                continue;
            };
            if channels.iter().any(|c| c.request == request && c.did == did) {
                continue;
            }
            channels.push(Channel { request, did, def: None, selected: false });
        }
    }
    channels.sort_by_key(|c| (c.request, c.did));
    channels
}

/// One request: a control unit and the identifiers to ask it for at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub request: u16,
    pub dids: Vec<u16>,
}

/// Group the selected channels into requests.
///
/// Grouped by control unit because addressing changes between them, then split
/// into [`BATCH`]-sized requests. Units come out in ascending order so the
/// polling sequence is stable — a screen whose rows reshuffle between cycles
/// is unreadable.
pub fn plan(channels: &[Channel]) -> Vec<Batch> {
    let mut by_unit: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
    for c in channels.iter().filter(|c| c.selected) {
        let dids = by_unit.entry(c.request).or_default();
        // The same identifier twice in one request wastes a slot and makes the
        // response ambiguous to split.
        if !dids.contains(&c.did) {
            dids.push(c.did);
        }
    }
    by_unit
        .into_iter()
        .flat_map(|(request, dids)| {
            dids.chunks(BATCH)
                .map(|chunk| Batch { request, dids: chunk.to_vec() })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Parse `01:2029,202A 714:2203` or a bare `2029,202A` (engine assumed).
///
/// The unit before the colon is whatever `vag_protocol::address` accepts: a
/// short number for the units this project has established, or a request id
/// for the rest.
pub fn parse_spec(spec: &str) -> Result<Vec<(u16, u16)>, String> {
    let mut out = Vec::new();
    for group in spec.split_whitespace() {
        let (request, list) = match group.split_once(':') {
            Some((unit, rest)) => (vag_protocol::address::parse(unit)?.request, rest),
            None => (ENGINE, group),
        };
        for did in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let did = u16::from_str_radix(did, 16)
                .map_err(|_| format!("{did:?} is not a hex data identifier"))?;
            out.push((request, did));
        }
    }
    if out.is_empty() {
        return Err("no identifiers given".to_string());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use vag_data::catalog::{ReadId, Scaling};
    use vag_data::measure::{LinearScale, RawForm};

    fn known(request: u16, did: u16, name: &'static str) -> Channel {
        Channel {
            request,
            did,
            def: Some(MeasurementDef {
                name: Cow::Borrowed(name),
                unit: Cow::Borrowed("bar"),
                address: ReadId::Uds(did),
                raw_form: RawForm::U16Be,
                scaling: Scaling::Linear(LinearScale { factor: 0.001, offset: 0.0 }),
            }),
            selected: true,
        }
    }

    #[test]
    fn one_request_per_control_unit_per_eight_identifiers() {
        // The addressing changes between units, so a batch can never span two.
        let mut chans: Vec<Channel> =
            (0..10).map(|i| known(ENGINE, 0x2000 + i, "engine")).collect();
        chans.extend((0..3).map(|i| known(GEARBOX, 0x3800 + i, "gearbox")));

        let batches = plan(&chans);
        assert_eq!(batches.len(), 3, "8+2 on the engine, 3 on the gearbox: {batches:?}");
        assert_eq!(batches[0].request, ENGINE);
        assert_eq!(batches[0].dids.len(), 8);
        assert_eq!(batches[1].request, ENGINE);
        assert_eq!(batches[1].dids.len(), 2);
        assert_eq!(batches[2].request, GEARBOX);
        assert_eq!(batches[2].dids.len(), 3);
    }

    #[test]
    fn unselected_channels_are_not_polled_and_duplicates_collapse() {
        let mut chans =
            vec![known(ENGINE, 0x2029, "boost"), known(ENGINE, 0x2029, "boost again")];
        chans.push(Channel { selected: false, ..known(ENGINE, 0x206E, "rpm") });

        let batches = plan(&chans);
        assert_eq!(batches.len(), 1);
        // Asking twice in one request wastes a slot and makes the answer
        // ambiguous to split.
        assert_eq!(batches[0].dids, vec![0x2029]);
    }

    #[test]
    fn nothing_selected_plans_nothing() {
        let chans = vec![Channel { selected: false, ..known(ENGINE, 0x2029, "boost") }];
        assert!(plan(&chans).is_empty());
    }

    #[test]
    fn the_polling_order_is_stable_across_cycles() {
        // Rows that reshuffle between cycles are unreadable, so the plan must
        // not depend on hash iteration order. The cluster sorts *before* the
        // powertrain because its id is lower — the order is by id, not by the
        // number people call the unit.
        let chans = vec![
            known(CLUSTER, 0x2203, "odo"),
            known(ENGINE, 0x206E, "rpm"),
            known(GEARBOX, 0x380A, "in"),
        ];
        let a = plan(&chans);
        let b = plan(&chans);
        assert_eq!(a, b);
        assert_eq!(
            a.iter().map(|x| x.request).collect::<Vec<_>>(),
            vec![CLUSTER, ENGINE, GEARBOX]
        );
    }

    #[test]
    fn a_units_own_row_overrides_the_standard_one_at_the_same_address() {
        // F40D is one byte of km/h on the engine (the OBD mirror) and two
        // little-endian bytes on the gearbox. Listing both under one entry
        // would make one of them wrong.
        let all = available();
        let engine = all.iter().find(|c| c.request == ENGINE && c.did == 0xF40D).unwrap();
        let gearbox = all.iter().find(|c| c.request == GEARBOX && c.did == 0xF40D).unwrap();
        assert_eq!(engine.def.as_ref().unwrap().raw_form, RawForm::U8First);
        assert_eq!(gearbox.def.as_ref().unwrap().raw_form, RawForm::U16Le);
    }

    #[test]
    fn an_unknown_channel_is_labelled_by_address_and_renders_raw() {
        let c = Channel { request: GEARBOX, did: 0x38F0, def: None, selected: true };
        assert_eq!(c.label(), "02/38F0");
        assert_eq!(c.render(&[0x0B, 0x34]), "0B34 (raw)");
        // A unit with no established short number is named by its id, not by a
        // guessed number.
        let brakes = Channel { request: 0x713, did: 0x1234, def: None, selected: true };
        assert_eq!(brakes.label(), "713/1234");
    }

    #[test]
    fn a_discrete_state_shows_its_name_and_an_unlisted_code_shows_raw() {
        let gear = vag_data::catalog::proven_gearbox()
            .into_iter()
            .find(|d| matches!(d.address, ReadId::Uds(0x3816)))
            .unwrap();
        let c = Channel { request: GEARBOX, did: 0x3816, def: Some(gear), selected: true };
        assert_eq!(c.render(&[0x05]), "4");
        assert_eq!(c.render(&[0x0C]), "R");
        assert_eq!(c.render(&[0x09]), "09 (raw)");
    }

    #[test]
    fn a_spec_names_control_units_by_number_or_by_request_id() {
        assert_eq!(parse_spec("2029,202A").unwrap(), vec![(ENGINE, 0x2029), (ENGINE, 0x202A)]);
        assert_eq!(
            parse_spec("01:2029 02:380A,3816").unwrap(),
            vec![(ENGINE, 0x2029), (GEARBOX, 0x380A), (GEARBOX, 0x3816)]
        );
        // The cluster by number and by id are the same unit.
        assert_eq!(parse_spec("17:2203").unwrap(), vec![(CLUSTER, 0x2203)]);
        assert_eq!(parse_spec("714:2203").unwrap(), parse_spec("17:2203").unwrap());
        // A unit with no established number is still reachable by its id.
        assert_eq!(parse_spec("713:1001").unwrap(), vec![(0x713, 0x1001)]);
        assert!(parse_spec("zz").is_err());
        assert!(parse_spec("").is_err());
    }

    #[test]
    fn a_pair_is_recognised_by_its_suffix_in_any_of_the_spellings_used() {
        assert_eq!(split_role("Boost pressure, actual"), Some(("Boost pressure", Role::Actual)));
        assert_eq!(
            split_role("Clutch 1 pressure, specified"),
            Some(("Clutch 1 pressure", Role::Specified))
        );
        assert_eq!(split_role("Engine torque, requested"), Some(("Engine torque", Role::Specified)));
        // Not a pair: a name that merely mentions the word. Joining two
        // unrelated rows would show a comparison nobody established.
        assert_eq!(split_role("Actual gear"), None);
        assert_eq!(split_role("Engine speed"), None);
    }

    #[test]
    fn a_survey_adds_every_unit_it_found_without_overriding_a_proven_scaling() {
        // The whole point of surveying: units the catalogs know nothing about
        // become watchable, as raw bytes, on the strength of having answered.
        let survey = "\
{\"request\":\"70E\",\"unit\":\"09\",\"dids\":[{\"did\":\"190B\",\"data\":\"02240010\"},\
{\"did\":\"192F\",\"data\":\"0305AA11\"}]}
{\"request\":\"7E0\",\"unit\":\"01\",\"dids\":[{\"did\":\"2029\",\"data\":\"0B34\"}]}
";
        let channels = with_survey(available(), survey);
        let bcm: Vec<&Channel> = channels.iter().filter(|c| c.request == 0x70E).collect();
        assert_eq!(bcm.len(), 2, "both BCM identifiers are on offer");
        assert!(bcm.iter().all(|c| c.def.is_none()), "nothing proven, so nothing claimed");
        assert_eq!(bcm[0].label(), "09/190B");

        // 2029 is a proven engine measurement; the survey must not blank it.
        let boost = channels
            .iter()
            .find(|c| c.request == 0x7E0 && c.did == 0x2029)
            .expect("the engine row survives");
        assert!(boost.def.is_some());
    }

    #[test]
    fn a_malformed_survey_line_is_skipped_rather_than_fatal() {
        let before = available().len();
        let channels = with_survey(available(), "not json\n{\"request\":\"zz\"}\n\n");
        assert_eq!(channels.len(), before);
    }
}
