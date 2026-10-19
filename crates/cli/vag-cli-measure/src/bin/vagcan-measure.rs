//! `vagcan-measure` — the stopwatch as its own program.
//!
//! `vagcan measure` is the same thing reached through the diagnostics binary,
//! and both go through [`vag_cli_measure::dispatch`]: the flags are declared
//! once and the behaviour is one function, so the two spellings cannot drift.

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	let cli = vag_cli_measure::args::Cli::parse();
	if let Some(id) = &cli.project {
		vag_cli_core::project::select(id);
	}
	// Resolved only by what reads the rows: `view` opens a file, and needs no project.
	let catalogs = |given: Option<&str>| -> anyhow::Result<String> {
		let dir = vag_cli_core::datadir::or_default(given, || Ok(vag_cli_core::project::current()?.measurements_dir()))?;
		Ok(dir.to_string_lossy().into_owned())
	};
	let slcan = cli.slcan;
	vag_cli_measure::dispatch(cli.args, catalogs, async |device: Option<String>| {
		vag_cli_core::device::connect(device.as_deref(), slcan).await
	})
	.await
}
