//! Benchmark for the hot `LabelDb` lookups `vagcan info` performs:
//! part-number -> label file (`resolve`, incl. REDIRECT chains) and
//! (part_no, block, field) -> measurement name (`measurement`).
//!
//! Uses a synthetic label files sized like the reference VCDS install
//! (~2,900 label files, ~3,700 REDIRECT rows, ~43k measurements) so the
//! numbers are representative without shipping proprietary data.
//!
//! Run: `cargo bench -p vag-data --bench lookup`

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use vag_data::{LabelDb, LabelFile, parse_label};

/// Number of target label files (reference install: ~2,884).
const FILES: usize = 2_900;
/// Number of wildcard REDIRECT rows on top of the per-file exact ones
/// (reference install: ~3,739 redirects total).
const WILDCARD_REDIRECTS: usize = 800;
/// Measurements per file (~43k total, like the reference `.lbl` label files).
const MEASUREMENTS_PER_FILE: usize = 15;

/// Part number for target file `i`, shaped like a real VAG part number.
fn part_no(i: usize) -> String {
	format!("{:03}-906-{:03}-AB", i / 500, i % 500)
}

fn synth_label_files() -> Vec<LabelFile> {
	let mut files = Vec::with_capacity(FILES + 3);
	let mut index_src = String::new();

	for i in 0..FILES {
		index_src.push_str(&format!("REDIRECT,T{i:04}.LBL,{}\n", part_no(i)));
		let mut body = String::new();
		for m in 0..MEASUREMENTS_PER_FILE {
			body.push_str(&format!("{:03},{},Measurement {i}-{m},,Range: 0...6500 RPM\n", m + 1, 1));
		}
		files.push(parse_label(format!("T{i:04}.LBL"), body.as_bytes()));
	}

	// Wildcard redirects: same length as the exact selectors so they are
	// real match candidates for every lookup, like index files in the wild.
	for i in 0..WILDCARD_REDIRECTS {
		index_src.push_str(&format!("REDIRECT,CHAIN.LBL,9{:02}-906-???-AB\n", i % 100));
	}
	files.push(parse_label("INDEX.LBL", index_src.as_bytes()));

	// Two-hop chain behind the wildcards.
	files.push(parse_label("CHAIN.LBL", b"REDIRECT,FINAL.LBL,9??-906-???-AB\n"));
	files.push(parse_label("FINAL.LBL", b"003,1,Chain Terminal,,Range: 0...100 %\n"));
	files
}

fn bench_lookup(c: &mut Criterion) {
	let db = LabelDb::new(synth_label_files());
	let exact_pn = part_no(1_234);
	let wildcard_pn = "912-906-555-AB"; // only the wildcard redirects match
	let miss_pn = "999-999-999-ZZ";

	let mut g = c.benchmark_group("label_lookup");

	g.bench_function("resolve_exact_redirect", |b| b.iter(|| black_box(db.resolve(black_box(&exact_pn)))));
	g.bench_function("resolve_wildcard_chain", |b| b.iter(|| black_box(db.resolve(black_box(wildcard_pn)))));
	g.bench_function("resolve_miss", |b| b.iter(|| black_box(db.resolve(black_box(miss_pn)))));
	g.bench_function("measurement_by_block_field", |b| {
		b.iter(|| black_box(db.measurement(black_box(&exact_pn), black_box(7), black_box(1))))
	});
	// A realistic `vagcan info` unit of work: resolve one ECU and read
	// several measuring-block names from it.
	g.bench_function("info_ecu_10_measurements", |b| {
		b.iter(|| {
			for block in 1..=10u16 {
				black_box(db.measurement(black_box(&exact_pn), block, 1));
			}
		})
	});
	g.finish();

	let files = synth_label_files();

	// Cold (un-memoized) path: a fresh LabelDb per iteration, resolving every
	// distinct part number exactly once. Per-lookup time = reported time / FILES
	// (also visible via the elements/sec throughput line).
	let all_pns: Vec<String> = (0..FILES).map(part_no).collect();
	let mut g = c.benchmark_group("label_lookup_cold");
	g.sample_size(10);
	g.throughput(Throughput::Elements(FILES as u64));
	g.bench_function("resolve_2900_distinct_uncached", |b| {
		b.iter_batched(
			|| LabelDb::new(files.clone()),
			|db| {
				for pn in &all_pns {
					black_box(db.resolve(black_box(pn)));
				}
			},
			BatchSize::PerIteration,
		)
	});
	g.finish();

	// Label files -> LabelDb build cost (startup, not per-lookup).
	let mut g = c.benchmark_group("label_db_build");
	g.sample_size(10);
	g.bench_function("labeldb_new_2900_files", |b| {
		b.iter_batched(|| files.clone(), |f| black_box(LabelDb::new(f)), criterion::BatchSize::LargeInput)
	});
	g.finish();
}

criterion_group!(benches, bench_lookup);
criterion_main!(benches);
