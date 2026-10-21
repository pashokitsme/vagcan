//! Everything whose input is a VCDS file.
//!
//! Not one of these needs a car, an adapter, or a key in the ignition: they read
//! a VCDS installation, the artefacts recovered from one, and the logs VCDS
//! itself writes. That is why they are here rather than at the top level, which
//! is reserved for the commands worth having in front of you at an open driver's
//! door.
//!
//! Three of them — [`Tool::Rod`], [`Tool::Dump`], [`Tool::Tttext`] — run rarely:
//! once per VCDS installation, or once per question about an artefact already
//! committed. They used to be separate binaries under `vag-data-labels`, which meant
//! that by the time anyone needed one again nobody remembered it existed, what
//! to feed it, or what it left behind. So each subcommand's help says three
//! things, deliberately: **what it is for**, **what it expects on input** by
//! name, and **what it writes**.
//!
//! They also chain, which is the other thing that was lost — the text table
//! `tttext` reads is a section `rod --dump` extracts:
//!
//! ```text
//! vagcan dev vcds rod TTTEXT.ROD --dump out/     →  out/TXT.bin
//! vagcan dev vcds tttext out/TXT.bin --out names.tsv
//! ```

mod dump;
// `vagcan setup` chains these two the way this module's own help documents —
// `rod --dump` writes the text section, `tttext` reads it — so it needs them by
// name rather than through the subcommand enum.
pub mod rod;
pub mod tttext;

use std::path::PathBuf;
use vag_cli_core::report::Advice as _;

use anyhow::Result;
use clap::Subcommand;

use crate::device::DeviceArg;
use crate::{analyse, datadir, labels, names};
use vag_cli_core::table::{self, Table};

/// `labels`' `--device`: only `--from-car` opens one.
const LABELS_DEVICE_HELP: &str = "Adapter to use with --from-car: a serial path (a USB-CAN adapter, or the dash board on its USB cable), \
                                  `ble` for the board over Bluetooth, or `ble:<name>`. Omit it to use the one adapter or board on USB; \
                                  Bluetooth is looked for only when asked";

// Clone so `vagcan`'s dispatcher can keep a copy of the command it is running:
// one that stops for want of label data is run again once the data exists.
#[derive(Clone, Subcommand)]
pub enum Tool {
	/// Look measurements up in a VCDS label directory.
	///
	/// FOR: resolving what a control unit calls its measuring blocks, from the
	/// label files VCDS ships. Answers from a SQLite cache, so it is
	/// milliseconds after the first run.
	///
	/// IN: a VCDS install root or any directory below it, plus one of
	/// `--part`, `--block`, `--odx`, or `--from-car` to say what to look up.
	///
	/// OUT, on stdout: with no lookup, the files by kind — `.lbl`/`.clb` in the
	/// label directory, the ones the label cache is built from, and `.rod` in the
	/// whole tree; `--part`, the file it resolves to and its measurements;
	/// `--block`, every file defining the block; `--odx`, the file's sections,
	/// each `open` or `sealed`. `-v` gives the cipher of each: `tea`, `zlib`,
	/// `no key` (`vagcan dev vcds rod` recovers it), or `no crib` (no key can be
	/// searched for; the file is not damaged). `--part` and `--block` read this
	/// project's label cache, built on first use.
	#[command(mut_arg("device", |a| a.help(LABELS_DEVICE_HELP)))]
	Labels {
		/// VCDS install root, or any directory below it.
		#[arg(value_name = "DIR")]
		dir: String,
		/// Resolve a part number to its label file and measurements.
		#[arg(long, value_name = "PART")]
		part: Option<String>,
		/// List every file defining this measuring block.
		#[arg(long, value_name = "N")]
		block: Option<u16>,
		/// Narrow --block to one field.
		#[arg(long, requires = "block", value_name = "N")]
		field: Option<u8>,
		/// Resolve the ODX file a control unit names for itself, e.g.
		/// `EV_ECM18TFS0208V0906264H` — the value of identifier F19E, which
		/// `vagcan units --identify` reads off the car.
		#[arg(long, value_name = "NAME")]
		odx: Option<String>,
		/// Read F19E from the car and resolve that, instead of passing --odx.
		/// The one thing here that touches a vehicle.
		#[arg(long, conflicts_with = "odx")]
		from_car: bool,
		/// Control unit to ask when using --from-car: a short number (01 engine)
		/// or a request id (713).
		//
		// `--ecu` was its name until 2026-10-19 and still parses, unlisted.
		#[arg(long = "unit", alias = "ecu", default_value = "01", value_name = "UNIT")]
		ecu: String,
		/// Rebuild the label cache even if it looks current.
		#[arg(long)]
		refresh: bool,
		/// Where the keys for reading encrypted .rod label files are cached.
		///
		/// VW ships .rod files with their contents encrypted, and the key for a
		/// section has to be recovered by a separate, slow tool before that
		/// section can be read. A section with no key here is listed as `sealed`
		/// rather than guessed, and the command that recovers its key is printed
		/// on stderr.
		/// Default: this project's `rod-keys.json`, written by `vagcan setup`.
		#[arg(long, value_name = "FILE")]
		iv_cache: Option<String>,
		#[command(flatten)]
		device: DeviceArg,
	},

	/// Search the measurement names `vagcan setup` recovered.
	///
	/// FOR: finding what VW calls something, when all you have is a word.
	///
	/// IN: a substring, and the names (`--catalog`, by default what `vagcan setup`
	/// wrote into this project from a VCDS installation, an ODIS project, or both).
	///
	/// OUT: a table of matches (id, name; the source when both files are
	/// searched) and a count, on stdout. A name is keyed by its text id, not by
	/// data identifier: a match is a guess to test on the car.
	Names {
		/// Substring to look for, case-insensitive.
		#[arg(value_name = "TEXT")]
		text: String,
		/// Stop after this many matches.
		#[arg(long, default_value_t = 40, value_name = "N")]
		limit: usize,
		/// Names file to search.
		/// Default: this project's `names.json` (from a VCDS installation) and
		/// `names-odis.json` (from an ODIS project), whichever `vagcan setup` wrote.
		#[arg(long, value_name = "FILE")]
		catalog: Option<String>,
	},

	/// Cross a bus capture with a VCDS log to prove measurement scalings.
	///
	/// FOR: learning what raw bytes mean, by watching VCDS read the same car at
	/// the same moment and print the engineering value.
	///
	/// IN: a capture from `vagcan dev sniff --out`, and the VCDS measuring-blocks
	/// CSV export recorded alongside it. The two are aligned by their wall-clock
	/// stamps — a subtraction, never a search.
	///
	/// OUT: one row per logged measurement on stdout — its scaling and `proven`,
	/// or why there is none; a fit that cleared the bar and lost to a better one
	/// is `outranked` (proportional quantities fit each other). `-v` adds the
	/// clock offset and the near misses. With `--out`, the proven rows as a
	/// measurement catalog that `vagcan watch` reads directly.
	Analyse {
		/// Capture written by `vagcan dev sniff`.
		#[arg(long, value_name = "FILE")]
		capture: String,
		/// VCDS measuring-blocks CSV export recorded at the same time.
		#[arg(long, value_name = "FILE")]
		log: String,
		/// Write the proven scalings as a measurement catalog.
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Minimum R² for a fit to count (the whole bar: R² ≥ 0.995, ≥ 20
		/// points over ≥ 4 distinct raw values).
		#[arg(long, default_value_t = 0.995, value_name = "R2")]
		min_r2: f64,
		/// Minimum matched samples for a fit to count (the whole bar: R² ≥
		/// 0.995, ≥ 20 points over ≥ 4 distinct raw values).
		#[arg(long, default_value_t = 20, value_name = "N")]
		min_points: usize,
	},

	/// Open a `.rod` container: decrypt and inflate every section.
	///
	/// FOR: reading a VW ODX label file, and recovering the per-section key
	/// that VW's encryption blocks. The recovered keys are the whole point —
	/// they are cached, committed, and every later run of every other command
	/// reads the cache instead of searching again.
	///
	/// IN: one `.rod` file from a VCDS installation, e.g. `TTTEXT.ROD` or
	/// `EV_ECM18TFS0208V0906264H.rod`.
	///
	/// OUT: the file and its key cache, then one row per section on stdout:
	/// section, state (`open`, `sealed`), bytes, key (`new` when recovered this
	/// run), preview. `-v` gives the cipher of each: `tea`, `zlib`, `no key`, or
	/// `no crib` (the file masks its IVs, so the key cannot be searched for; the
	/// file is not damaged). Recovered keys are added to the cache; with
	/// `--dump DIR`, each opened section is written to `DIR/<section>.bin`
	/// (`DIR/TXT.bin`).
	Rod {
		/// The `.rod` file to open.
		#[arg(value_name = "FILE")]
		file: String,
		/// Use only keys already in the cache; do not search for missing ones.
		/// Searching costs minutes per file; a file under its own name needs none.
		#[arg(long)]
		no_crack: bool,
		/// Where recovered keys are read and written. Default: next to the
		/// input, as `<FILE>.ivcache.json`. The cache `vagcan setup` fills is
		/// this project's `rod-keys.json`.
		#[arg(long, value_name = "PATH")]
		cache: Option<String>,
		/// Also write each decoded section to `DIR/<section>.bin`. This is how
		/// the input to `vagcan dev vcds tttext` is produced.
		#[arg(long, value_name = "DIR")]
		dump: Option<String>,
	},

	/// Parse a whole VCDS `Labels/` directory into one JSON file.
	///
	/// FOR: getting the entire set of label files into a single structured file that
	/// can be searched, diffed between VCDS versions, or handed to something
	/// that is not this program. Encrypted `.clb` files are decrypted on the
	/// way through, so the output holds plaintext and needs no key.
	///
	/// IN: a VCDS `Labels/` directory, or any directory below it.
	///
	/// OUT: the files and the records by kind, on stdout; with `--out`, a JSON
	/// array of label files sorted by source name.
	///
	/// To look ONE part number up, use `vagcan dev vcds labels --part` instead — it
	/// answers from a cache instead of reparsing the label files.
	Dump {
		/// VCDS `Labels/` directory, or any directory below it.
		#[arg(value_name = "DIR")]
		dir: String,
		/// Write the parsed label files here as JSON. Without it, only the summary
		/// is printed.
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
	},

	/// Read the label files' global text table: every name, exactly.
	///
	/// FOR: seeing what VCDS calls a text id, and which ODX id (`IDE`/`MAS`)
	/// that text names. Each record of `TTTEXT.ROD` is enciphered under its
	/// own id, so every one reads, digits included — seconds, nothing guessed.
	/// `vagcan setup` does the same read into a project's `names.json`.
	///
	/// IN: the decrypted, inflated `[TXT]` section of `TTTEXT.ROD` — produced
	/// by `vagcan dev vcds rod TTTEXT.ROD --dump DIR`, which writes it as
	/// `DIR/TXT.bin`. Read as Windows-1252, the English build's code page.
	///
	/// OUT: one line per record, `<id> TAB <name> TAB <kind> TAB <value>`, to
	/// stdout or `--out`. Kind and value are the record's tail, empty when it
	/// has none: kind 2 is an IDE and kind 7 a MAS, so `2 TAB 00594` is
	/// IDE00594. A count on stderr; `-v` lists the records that read as
	/// neither shape.
	Tttext {
		/// The `[TXT]` section, decrypted and inflated.
		#[arg(value_name = "FILE")]
		file: String,
		/// Write the lines here instead of stdout.
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Also write every name as `{"<text id>": "<name>"}` JSON — the
		/// `names.json` that `vagcan setup` writes and `vagcan dev vcds names`
		/// searches.
		#[arg(long, value_name = "FILE")]
		catalog: Option<String>,
	},

	/// Count what the label cache holds, table by table, and the rows of VCDS's
	/// other registries by section (ADP, FFMUX, GES, MUX, SOT, XPL).
	///
	/// FOR: telling "setup wrote nothing" apart from "setup wrote something I
	/// cannot find", which are the same screenful of missing names otherwise.
	/// EXPECTS: nothing — it reads this project's own cache. WRITES: nothing.
	///
	/// OUT: rows per table on stdout; `-v` names the cache and the sources it
	/// was built from.
	Stats {
		/// Read this cache instead of the current project's.
		#[arg(long, value_name = "FILE")]
		catalog: Option<String>,
	},
}

/// Everything except `labels --from-car` is a pure file operation; that one
/// case needs the async runtime, so the caller hands it back rather than
/// nesting a second runtime inside this one.
pub enum Outcome {
	Done,
	/// `labels --from-car`: read F19E off the unit, then resolve it. No
	/// `refresh`: resolving an ODX name goes at the `.rod` file directly, not
	/// through the part-number cache that flag rebuilds.
	FromCar {
		dir: String,
		ecu: String,
		iv_cache: PathBuf,
		device: Option<String>,
	},
}

pub fn run(tool: Tool) -> Result<Outcome> {
	match tool {
		Tool::Labels {
			dir,
			from_car: true,
			ecu,
			iv_cache,
			device,
			..
		} => {
			// The label files are checked before the adapter is opened: reading F19E
			// off the car and only then discovering that the label directory
			// does not exist costs the port for nothing.
			if !std::path::Path::new(&dir).is_dir() {
				return Err(anyhow::anyhow!("{dir:?} is not a directory").help("point it at the VCDS install root"));
			}
			let iv_cache = datadir::or_default(iv_cache.as_deref(), || Ok(crate::project::vcds()?.rod_keys()))?;
			let device = device.requested().map(str::to_owned);
			Ok(Outcome::FromCar { dir, ecu, iv_cache, device })
		}
		Tool::Labels {
			dir,
			odx: Some(name),
			iv_cache,
			..
		} => {
			let keys = datadir::or_default(iv_cache.as_deref(), || Ok(crate::project::vcds()?.rod_keys()))?;
			labels::resolve_odx(&dir, &name, &keys)?;
			Ok(Outcome::Done)
		}
		Tool::Labels {
			dir,
			part,
			block,
			field,
			refresh,
			..
		} => {
			labels::labels_cmd(&dir, part.as_deref(), block, field, refresh)?;
			Ok(Outcome::Done)
		}
		Tool::Names { text, limit, catalog } => {
			let files = match catalog.as_deref() {
				Some(given) => vec![names::NamesFile {
					source: "",
					path: std::path::PathBuf::from(given),
				}],
				None => names::project_files(&crate::project::stores()?),
			};
			names::run(&text, limit, &files)?;
			Ok(Outcome::Done)
		}
		Tool::Analyse {
			capture,
			log,
			out,
			min_r2,
			min_points,
		} => {
			analyse::run(
				&capture,
				&log,
				out.as_deref(),
				analyse::Thresholds {
					min_r2,
					min_points,
					..Default::default()
				},
			)?;
			Ok(Outcome::Done)
		}
		Tool::Rod { file, no_crack, cache, dump } => {
			rod::run(&file, !no_crack, cache.as_deref(), dump.as_deref())?;
			Ok(Outcome::Done)
		}
		Tool::Dump { dir, out } => {
			dump::run(&dir, out.as_deref())?;
			Ok(Outcome::Done)
		}
		Tool::Stats { catalog } => {
			// Whether the reader named the file matters to what an absence means,
			// so it is carried rather than re-derived: `--catalog` is somebody
			// pointing at a path, and no amount of `vagcan setup` will create a
			// path they invented.
			let named = catalog.is_some();
			let path = datadir::or_default(catalog.as_deref(), || Ok(crate::project::vcds()?.cache()))?;
			stats(&path, named)?;
			Ok(Outcome::Done)
		}
		Tool::Tttext { file, out, catalog } => {
			tttext::run(tttext::Options {
				file: &file,
				out: out.as_deref(),
				catalog: catalog.as_deref(),
			})?;
			Ok(Outcome::Done)
		}
	}
}

/// Row counts of a label cache, table by table.
///
/// The counts come from `vag_data_db::row_counts` rather than from a query
/// written here: the table names are that crate's schema, and a caller that
/// spelled them out itself would keep compiling after a rename and fail only
/// when run.
///
/// A missing file is answered as a missing file. `setup` not having been run
/// and `setup` having been run into a different project produce the same empty
/// screen everywhere else, and this is the command somebody reaches for to tell
/// them apart — so it must not report the second as a database error.
fn stats(db_path: &std::path::Path, named: bool) -> Result<()> {
	if !db_path.exists() {
		// Two callers, two different facts, and one message cannot be true of
		// both. Without `--catalog` this is a project that was never built, and
		// `missing` says how to build it. With `--catalog` the reader named this
		// path themselves — `vagcan setup` writes into `~/.vagcan/` and will
		// never create the file they typed, so offering it would be advice that
		// cannot work.
		if named {
			anyhow::bail!("no label cache at {}", db_path.display());
		}
		return Err(
			crate::missing::NoLabelData::new("there is no label cache to count")
				.looked_for(db_path)
				.into_error(),
		);
	}
	let counts = vag_data_db::row_counts(db_path)?;
	// The other registries' rows, by the section a unit selects them with.
	let sections: Vec<(String, i64)> = vag_data_db::registry_row_counts(db_path, vag_data_db::VCDS)?
		.into_iter()
		.map(|(section, n)| (format!("registry_row {section}"), i64::try_from(n).unwrap_or(i64::MAX)))
		.collect();
	let mut counts: Vec<(&str, i64)> = counts;
	counts.extend(sections.iter().map(|(name, n)| (name.as_str(), *n)));
	vag_cli_core::out!("{}", render_stats(&counts));
	vag_cli_core::note!("label cache {}", db_path.display());
	for (kind, dir) in vag_data_db::sources_of(db_path).unwrap_or_default() {
		vag_cli_core::note!("source {kind} {dir}");
	}
	Ok(())
}

/// `stats`' table: rows per table, in the schema's order, under `what  count` like every
/// count table in `dev vcds`.
fn render_stats(counts: &[(&str, i64)]) -> String {
	let mut table = Table::new(&["what", "count"]).right(1);
	for (name, n) in counts {
		table.row(vec![name.to_string(), n.to_string()]);
	}
	table.render(table::terminal_width())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn stats_are_one_row_per_table() {
		assert_eq!(
			render_stats(&[("label_file", 1), ("measurement", 101889)]),
			"what          count
label_file        1
measurement  101889
"
		);
	}

	#[test]
	fn a_named_cache_that_is_not_there_is_an_error_naming_it() {
		let err = stats(std::path::Path::new("/nope/cache.sqlite"), true).unwrap_err().to_string();
		assert_eq!(err, "no label cache at /nope/cache.sqlite");
	}
}
