// `to_rust` on the test fixture of `src/dash.rs` (a lever and a stopwatch), not on any car's
// data. Compiled by `tests/generated_plan.rs`; rewritten by `BLESS=1 cargo test -p vag-cli-core generated_source`.
use vag_dash_render::plan::{Band, Channel, Page, Plan, StalkPlan, StopwatchPlan, Unit};
use vag_dash_render::stalk::{StateIndex, States};
use vag_dash_render::alarm::Alarm;

pub static PLAN: Plan = Plan { vin: "TESTVIN0000000001", language: "en", units: &UNITS, channels: &CHANNELS, pages: &PAGES, alarms: &ALARMS, stalk: Some(StalkPlan { rocker: 3, switch: 4, cruise: 5, rocker_states: &STALK_ROCKER, switch_states: &STALK_SWITCH, cruise_states: &STALK_CRUISE, states: States { next: StateIndex(1), previous: StateIndex(2), measure: StateIndex(3), switch_off: StateIndex(3), cruise_off: StateIndex(0) } }), stopwatch: Some(StopwatchPlan { speed: 1, km_h_per_unit: 0.0271, marks: &MARKS }) };

static UNITS: [Unit; 2] = [
	Unit { request: 0x7E0, response: 0x7E8, part_number: "PART1" },
	Unit { request: 0x70C, response: 0x776, part_number: "PART2" },
];

static CHANNELS: [Channel; 6] = [
	Channel { unit: 0x7E0, did: 0x1001, bit_offset: 0, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: 0.0, decimals: 0, unit_text: "°C", label: "One", proven: false, hz: 2.0, setpoint: None },
	Channel { unit: 0x7E0, did: 0x3001, bit_offset: 0, bit_length: 16, signed: false, big_endian: true, factor: 1.0, offset: 0.0, decimals: 0, unit_text: "°C", label: "Speed", proven: false, hz: 50.0, setpoint: None },
	Channel { unit: 0x7E0, did: 0x3002, bit_offset: 0, bit_length: 16, signed: false, big_endian: true, factor: 1.0, offset: -40.0, decimals: 0, unit_text: "°C", label: "Offset speed", proven: false, hz: 2.0, setpoint: None },
	Channel { unit: 0x70C, did: 0x1105, bit_offset: 64, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: 0.0, decimals: 0, unit_text: "", label: "Rocker", proven: false, hz: 2.0, setpoint: None },
	Channel { unit: 0x70C, did: 0x1105, bit_offset: 72, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: 0.0, decimals: 0, unit_text: "", label: "Switch", proven: false, hz: 2.0, setpoint: None },
	Channel { unit: 0x7E0, did: 0x2001, bit_offset: 0, bit_length: 16, signed: false, big_endian: true, factor: 1.0, offset: 0.0, decimals: 0, unit_text: "", label: "Cruise status", proven: false, hz: 2.0, setpoint: None },
];

static CELLS_0: [u16; 1] = [0];
static PAGES: [Page; 1] = [
	Page::Values { title: "T", cells: &CELLS_0 },
];

static ALARMS: [Alarm<'static>; 0] = [
];

static STALK_ROCKER: [Band; 6] = [
	Band { lower: 0, upper: 74 }, // "shorted"
	Band { lower: 75, upper: 110 }, // "plus"
	Band { lower: 111, upper: 145 }, // "minus"
	Band { lower: 146, upper: 181 }, // "limit"
	Band { lower: 182, upper: 221 }, // "rest"
	Band { lower: 222, upper: 255 }, // "open"
];

static STALK_SWITCH: [Band; 6] = [
	Band { lower: 0, upper: 74 }, // "shorted"
	Band { lower: 75, upper: 110 }, // "on"
	Band { lower: 111, upper: 145 }, // "cancel"
	Band { lower: 146, upper: 181 }, // "off"
	Band { lower: 182, upper: 221 }, // "lifted"
	Band { lower: 222, upper: 255 }, // "open"
];

static STALK_CRUISE: [Band; 3] = [
	Band { lower: 0, upper: 0 }, // "off"
	Band { lower: 1, upper: 1 }, // "standby"
	Band { lower: 2, upper: 2 }, // "passive"
];

static MARKS: [u16; 2] = [60, 100];
