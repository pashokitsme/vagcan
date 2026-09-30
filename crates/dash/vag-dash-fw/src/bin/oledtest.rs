//! Does the glass light? A test picture on the SSD1322, and nothing else.
//!
//! Bring-up probe for the 3.12″ 256×64 OLED (`todo/dash/22-oled.md`). The link to the
//! controller is write-only, so the only judge is a person looking at it; this draws three
//! pictures a person can check, in turn, and says on the USB console which one is up:
//!
//! 1. a one-pixel border on all four edges, `vagcan` in the middle, `TL` in the top left
//!    corner and a diagonal from there — the border says the 256×64 window sits on the
//!    glass, `TL` says which way up it hangs, and the diagonal shows a swapped pixel pair
//!    as a staircase;
//! 2. the same picture at a quarter of the brightness — the grey levels work;
//! 3. a checkerboard of 8-pixel squares — half the glass lit, for dead rows and columns.
//!
//! It starts no CAN controller and no radio: it may be left on a board, and it transmits
//! nothing. Pins as `dash` will use them: SCLK `GPIO10`, SDIN `GPIO8`, D/C `GPIO0`,
//! RES `GPIO21`, CS `GPIO20`.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_10X20};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Text};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::systimer::SystemTimer;
use log::{info, warn};
use vag_dash_fw::panel::{Framebuffer, HEIGHT, WIDTH};
use vag_dash_fw::ssd1322::{CLOCK_HZ, FULL, Ssd1322};

// The library's panic handler prints through the allocator-backed logger, so the heap has
// to exist even though this probe allocates nothing itself.
extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// How long each picture stays up.
const HOLD: Duration = Duration::from_secs(5);

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
	esp_println::logger::init_logger(log::LevelFilter::Info);

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	let spi = Spi::new(peripherals.SPI2, Config::default().with_frequency(Rate::from_hz(CLOCK_HZ)))
		.expect("the glass's clock is one the chip can make")
		.with_sck(peripherals.GPIO10)
		.with_mosi(peripherals.GPIO8)
		.into_async();
	let pin = OutputConfig::default();
	let dc = Output::new(peripherals.GPIO0, Level::Low, pin);
	let cs = Output::new(peripherals.GPIO20, Level::High, pin);
	let res = Output::new(peripherals.GPIO21, Level::High, pin);
	let mut glass = Ssd1322::new(spi, dc, cs, res);

	info!("oledtest: SCLK GPIO10, SDIN GPIO8, D/C GPIO0, RES GPIO21, CS GPIO20, {CLOCK_HZ} Hz");
	match glass.start(&mut Delay, false).await {
		Ok(()) => info!("oledtest: the controller is set up and the display is on"),
		Err(e) => warn!("oledtest: setting the controller up failed: {e:?}"),
	}

	let mut frame = Framebuffer::new();
	let mut round = 0u32;
	loop {
		round += 1;
		border(&mut frame);
		show(
			&mut glass,
			&frame,
			FULL,
			round,
			"border, `vagcan`, `TL` top left, a diagonal — full brightness",
		)
		.await;
		show(&mut glass, &frame, FULL / 4, round, "the same picture, a quarter as bright").await;
		checkerboard(&mut frame);
		show(&mut glass, &frame, FULL, round, "checkerboard of 8-pixel squares").await;
	}
}

/// Sends one picture, says so, and leaves it up for [`HOLD`].
async fn show<SPI, PIN>(glass: &mut Ssd1322<SPI, PIN>, frame: &Framebuffer, ink: u8, round: u32, what: &str)
where
	SPI: embedded_hal_async::spi::SpiBus,
	PIN: embedded_hal::digital::OutputPin,
{
	match glass.rows(0, frame.rows(), ink).await {
		Ok(()) => info!("[{round}] on the glass: {what}"),
		Err(_) => warn!("[{round}] the frame did not go out: {what}"),
	}
	Timer::after(HOLD).await;
}

fn border(frame: &mut Framebuffer) {
	let on = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
	let (width, height) = (WIDTH as i32, HEIGHT as i32);
	frame.clear_all();
	// Drawing onto the framebuffer cannot fail: its error type is `Infallible`.
	let _ = Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32))
		.into_styled(on)
		.draw(frame);
	let _ = Line::new(Point::new(2, 14), Point::new(2 + 40, 14 + 40)).into_styled(on).draw(frame);
	let _ = Text::new("TL", Point::new(4, 11), MonoTextStyle::new(&FONT_6X10, BinaryColor::On)).draw(frame);
	let _ = Text::with_alignment(
		"vagcan",
		Point::new(width / 2, height / 2 + 6),
		MonoTextStyle::new(&FONT_10X20, BinaryColor::On),
		Alignment::Center,
	)
	.draw(frame);
	let _ = Text::with_alignment(
		"256x64",
		Point::new(width - 4, height - 5),
		MonoTextStyle::new(&FONT_6X10, BinaryColor::On),
		Alignment::Right,
	)
	.draw(frame);
}

fn checkerboard(frame: &mut Framebuffer) {
	frame.clear_all();
	let squares = (0..HEIGHT as i32)
		.flat_map(|y| (0..WIDTH as i32).map(move |x| (x, y)))
		.filter(|(x, y)| (x / 8 + y / 8) % 2 == 0)
		.map(|(x, y)| Pixel(Point::new(x, y), BinaryColor::On));
	let _ = frame.draw_iter(squares);
}
