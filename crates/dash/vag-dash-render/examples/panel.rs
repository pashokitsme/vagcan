//! Render the panel to PNG, at the glass's 256×64, with no display attached.
//!
//! This is how the layout stops being a claim. `embedded-graphics-simulator`
//! with `default-features = false` needs no SDL and opens no window, so the same
//! drawing code that will run on the OLED writes files anybody can look at —
//! and, later, files CI can diff.
//!
//! Usage: `cargo run -p vag-dash-render --example panel -- <output directory>`, and `--short`
//! for a 256×32 panel, where the renderer puts the unit beside the number (the status strip's
//! pictures, 10 to 22, are laid out for the glass).
//!
//! The values are stand-ins with the right shape, not readings from the car.
//! Every channel named here is one the catalog declares for the reference car
//! (`todo/dash/README.md`), so the digit counts and unit strings are the ones a
//! real plan will have to fit.

use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics_simulator::{BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay};
use vag_dash_render::stopwatch::{self, Labels, Stopwatch, Words};
use vag_dash_render::{Board, Cell, Deviation, Faults, Frame, Links, PANEL, Strip, Theme, Volts, draw, draw_page};

fn main() {
	let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
	std::fs::create_dir_all(&dir).unwrap();

	// A blue-on-black OLED, scaled up so the panel is legible on a laptop.
	// The scale is a viewing convenience only — every pixel below is a pixel on
	// the panel.
	let settings = OutputSettingsBuilder::new().scale(3).theme(BinaryColorTheme::OledBlue).build();

	// `--short`: a pass at 256×32, the first target and not a real part. Nothing in the
	// renderer is told about it — the layout reads its height from the target — so this shows
	// what 32 rows lose against the glass.
	let short = std::env::args().any(|a| a == "--short");
	let size = if short { Size::new(PANEL.width, 32) } else { PANEL };

	let shot = |name: &str, frame: &Frame<'_>, theme: &Theme| {
		let mut display = SimulatorDisplay::<BinaryColor>::new(size);
		let report = draw(frame, theme, &mut display);
		display.to_rgb_output_image(&settings).save_png(format!("{dir}/{name}.png")).unwrap();
		println!("{name:<28} {report:?}");
	};

	// 1. Temperatures — the reference photograph's page.
	let temps = [
		Cell::new("МАСЛО", Some(93.0), "°C", 0),
		Cell::new("КОРОБКА", Some(72.0), "°C", 0),
		Cell::new("ОЖ", Some(93.0), "°C", 0),
		Cell::new("ВПУСК", Some(46.0), "°C", 0),
	];
	let temps = Frame::Values { cells: &temps };
	shot("1-temps-mono", &temps, &Theme::bold_mono());
	shot("2-temps-heavy", &temps, &Theme::heavy());
	shot("3-temps-segments", &temps, &Theme::segments());

	// 2. Motion — four digits and a decimal point, the widest case.
	let motion = [
		Cell::new("МОМЕНТ", Some(342.0), "Nm", 0),
		Cell::new("ОБОРОТЫ", Some(4820.0), "", 0),
		Cell::new("СКОРОСТЬ", Some(128.0), "", 0),
		Cell::new("НАДДУВ", Some(1.82), "", 2),
	];
	shot("4-motion", &Frame::Values { cells: &motion }, &Theme::bold_mono());

	// 3. Three cells — what a page looks like when the labels are allowed to be
	// words rather than abbreviations.
	let three = [
		Cell::new("МОМЕНТ", Some(342.0), "Nm", 0),
		Cell::new("НАДДУВ", Some(1.82), "bar", 2),
		Cell::new("ПРОСКАЛЬЗ", Some(112.0), "", 0),
	];
	shot("5-three-cells", &Frame::Values { cells: &three }, &Theme::bold_mono());

	// 4. Ignition retard per cylinder, with cylinder 2 past the threshold. The
	// alarm inverts one cell, so which cylinder survives the highlight.
	let retard = [
		Cell::new("ЦИЛ 1", Some(-0.8), "°", 1),
		Cell::new("ЦИЛ 2", Some(-2.6), "°", 1).alarmed(),
		Cell::new("ЦИЛ 3", Some(-0.4), "°", 1),
		Cell::new("ЦИЛ 4", Some(0.0), "°", 1),
	];
	shot("6-retard-alarm", &Frame::Values { cells: &retard }, &Theme::bold_mono());

	// 5. A channel that has not answered. A dash, never a zero.
	let missing = [
		Cell::new("МАСЛО", Some(93.0), "°C", 0),
		Cell::new("КОРОБКА", None, "°C", 0),
		Cell::new("ОЖ", Some(93.0), "°C", 0),
		Cell::new("ВПУСК", Some(46.0), "°C", 0),
	];
	shot("7-no-answer", &Frame::Values { cells: &missing }, &Theme::bold_mono());

	// 6. A boost trace through a pull: spool, plateau, the dip at the shift,
	// and back on it.
	let samples: Vec<f32> = (0..190)
		.map(|i| {
			let t = i as f32 / 190.0;
			let spool = (1.0 - (-t * 9.0).exp()) * 1.9;
			let shift = if (0.55..0.62).contains(&t) { -1.4 } else { 0.0 };
			(spool + shift + (i as f32 * 0.7).sin() * 0.04).max(0.0)
		})
		.collect();
	let chart = Frame::Chart {
		cell: Cell::new("НАДДУВ", Some(1.82), "bar", 2),
		min: 0.0,
		max: 2.5,
		samples: &samples,
		seconds_per_sample: 0.2,
	};
	shot("8-chart-boost", &chart, &Theme::bold_mono());

	// 7. The same chart eight samples in — a run that has just started draws a
	// short trace, not a stretched one.
	let chart_short = Frame::Chart {
		cell: Cell::new("НАДДУВ", Some(0.42), "bar", 2),
		min: 0.0,
		max: 2.5,
		samples: &samples[..8],
		seconds_per_sample: 0.2,
	};
	shot("9-chart-cold", &chart_short, &Theme::bold_mono());

	// 8. The status strip over a page: the title in the middle, the voltage at the left, the
	// page in the rows under it — with a BLE host and a fault count at the strip's right end.
	let board = Board {
		links: Links { usb: false, ble: true },
		rates: None,
		faults: Some(Faults::Counted {
			stored: 2,
			failing_now: false,
		}),
	};
	let shot_with = |name: &str, frame: &Frame<'_>, strip: &Strip<'_>, board: &Board| {
		let mut display = SimulatorDisplay::<BinaryColor>::new(size);
		let report = draw_page(frame, strip, board, &Theme::bold_mono(), &mut display);
		display.to_rgb_output_image(&settings).save_png(format!("{dir}/{name}.png")).unwrap();
		println!("{name:<28} {report:?}");
	};
	let strip_shot = |name: &str, frame: &Frame<'_>, strip: &Strip<'_>| {
		let mut display = SimulatorDisplay::<BinaryColor>::new(size);
		let report = draw_page(frame, strip, &board, &Theme::bold_mono(), &mut display);
		display.to_rgb_output_image(&settings).save_png(format!("{dir}/{name}.png")).unwrap();
		println!("{name:<28} {report:?}");
	};
	let temps_strip = [
		Cell::new("МАСЛО", Some(93.0), "°C", 0),
		Cell::new("КОРОБКА", Some(72.0), "°C", 0),
		Cell::new("ОЖ", Some(91.0), "°C", 0),
		Cell::new("ВПУСК", Some(34.0), "°C", 0),
	];
	let titled = |title| Strip {
		title,
		volts: Volts::Shown(Some(14.2)),
	};
	strip_shot("10-strip-values", &Frame::Values { cells: &temps_strip }, &titled("ТЕМПЕРАТУРЫ"));
	// The voltage is a cell of this page, so the plan shows none in its strip.
	let battery = [
		Cell::new("АКБ", Some(14.2), "V", 1),
		Cell::new("ОЖ", Some(91.0), "°C", 0),
		Cell::new("МАСЛО", Some(93.0), "°C", 0),
	];
	let no_volts = Strip {
		title: "ПИТАНИЕ",
		volts: Volts::Hidden,
	};
	strip_shot("11-strip-volts-is-a-cell", &Frame::Values { cells: &battery }, &no_volts);
	// Under a title the chart's header drops its label and keeps its range and seconds.
	strip_shot("12-strip-chart", &chart, &titled("НАДДУВ"));
	// No title and no voltage: the strip all the same, with the board's rune and badge in it.
	strip_shot("13-strip-none", &Frame::Values { cells: &temps_strip }, &Strip::NONE);
	// The voltage has not answered: a dash, never a zero.
	let unanswered = Strip {
		title: "ТЕМПЕРАТУРЫ",
		volts: Volts::Shown(None),
	};
	strip_shot("14-strip-no-answer", &Frame::Values { cells: &temps_strip }, &unanswered);

	// 9. The strip's right end for each thing the board says, over a drive page whose last cell
	// is a four-character number: the page under the strip is the same picture in every one.
	let drive = [
		Cell::new("СКОРОСТЬ", Some(87.0), "km/h", 0),
		Cell::new("ОБОРОТЫ", Some(2840.0), "", 0),
		Cell::new("НАДДУВ", Some(1.42), "bar", 2).with_deviation(Deviation::Value(-0.06)),
		Cell::new("ОБОРОТЫ", Some(6500.0), "", 0).with_deviation(Deviation::Value(-120.0)),
	];
	let ride = Strip {
		title: "ЕДЕМ",
		volts: Volts::Shown(Some(13.9)),
	};
	let both = Links { usb: true, ble: true };
	let said = |links, faults| Board { links, rates: None, faults };
	let two = |failing_now| Some(Faults::Counted { stored: 2, failing_now });
	let drive = Frame::Values { cells: &drive };
	shot_with("15-strip-both-faults-2", &drive, &ride, &said(both, two(false)));
	shot_with("16-strip-both-faults-2-failing", &drive, &ride, &said(both, two(true)));
	shot_with("17-strip-both-faults-failed", &drive, &ride, &said(both, Some(Faults::Failed)));
	shot_with("18-strip-nothing-connected", &drive, &ride, &said(Links::NONE, None));
	shot_with(
		"19-strip-usb-faults-12",
		&drive,
		&ride,
		&said(
			Links { usb: true, ble: false },
			Some(Faults::Counted {
				stored: 12,
				failing_now: false,
			}),
		),
	);
	// The chart, and a page with no title and no voltage, with both hosts and the count.
	shot_with("20-strip-chart-both-faults-2", &chart, &titled("НАДДУВ"), &said(both, two(false)));
	shot_with("21-strip-untitled-both-faults-2", &drive, &Strip::NONE, &said(both, two(false)));
	// The stopwatch page, as the board draws it: a strip with no title or voltage over it.
	let marks = [60u16, 100, 150];
	let labels = Labels::new(&marks);
	let words = Words::of("ru");
	let mut armed = Stopwatch::new(&marks, 0.01);
	armed.sample(Some(0.0), 0);
	armed.sample(Some(0.0), stopwatch::ARMING_HOLD_MS);
	let saved = [(60u16, 5.43f32), (100, 9.87), (150, 18.4)];
	let (row, count) = stopwatch::cells(&armed, Some(0.0), &saved, &words, &labels);
	shot_with(
		"22-stopwatch-both-faults-2-failing",
		&Frame::Values { cells: &row[..count] },
		&Strip::NONE,
		&said(both, two(true)),
	);
}
