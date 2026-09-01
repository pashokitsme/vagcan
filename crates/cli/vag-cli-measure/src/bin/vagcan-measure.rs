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
	let catalogs = vag_cli_core::datadir::or_default(None, || Ok(vag_cli_core::project::current()?.measurements_dir()))?;
	vag_cli_measure::dispatch(cli.args, &catalogs.to_string_lossy()).await
}
