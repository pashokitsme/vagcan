//! Render the panel to PNG, at 256×32, with no display attached.
//!
//! This is how the layout stops being a claim. `embedded-graphics-simulator`
//! with `default-features = false` needs no SDL and opens no window, so the same
//! drawing code that will run on the OLED writes files anybody can look at —
//! and, later, files CI can diff.
//!
//! Usage: `cargo run -p vag-dash --example panel -- <output directory>`
//!
//! The values are stand-ins with the right shape, not readings from the car.
//! Every channel named here is one the catalog declares for the reference car
//! (`todo/dash/README.md`), so the digit counts and unit strings are the ones a
//! real plan will have to fit.

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics_simulator::{BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay};
use vag_dash::{Cell, Frame, PANEL, Theme, draw};

fn main() {
	let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
	std::fs::create_dir_all(&dir).unwrap();

	// A blue-on-black OLED, scaled up so a 256×32 strip is legible on a laptop.
	// The scale is a viewing convenience only — every pixel below is a pixel on
	// the panel.
	let settings = OutputSettingsBuilder::new().scale(3).theme(BinaryColorTheme::OledBlue).build();

	let shot = |name: &str, frame: &Frame<'_>, theme: &Theme| {
		let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);
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
		window_seconds: 19.0,
	};
	shot("8-chart-boost", &chart, &Theme::bold_mono());

	// 7. The same chart eight samples in — a run that has just started draws a
	// short trace, not a stretched one.
	let chart_short = Frame::Chart {
		cell: Cell::new("НАДДУВ", Some(0.42), "bar", 2),
		min: 0.0,
		max: 2.5,
		samples: &samples[..8],
		window_seconds: 19.0,
	};
	shot("9-chart-cold", &chart_short, &Theme::bold_mono());
}
