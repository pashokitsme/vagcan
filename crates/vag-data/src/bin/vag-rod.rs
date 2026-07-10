//! `vag-rod` — dev tool: decrypt + decompress every section of a `.rod` file,
//! recovering the `product != 0` first-block IV offline where needed
//! (multithreaded brute force, ~1 min per blocked section), and print each
//! section's decoded size + a short preview.
//!
//! Usage:
//!   vag-rod <file.rod> [--no-crack] [--cache <path>]
//!
//! Recovered IVs are cached (default: `<file>.ivcache.json`) so repeat runs are
//! instant. `--no-crack` uses only already-cached values.

use std::path::PathBuf;
use std::process::ExitCode;

use vag_data::rod::{IvCache, RodStatus, decode_rod_recover};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut path: Option<String> = None;
    let mut cache_path: Option<PathBuf> = None;
    let mut dump_dir: Option<PathBuf> = None;
    let mut run_crack = true;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--no-crack" => run_crack = false,
            "--cache" => cache_path = args.next().map(PathBuf::from),
            "--dump" => dump_dir = args.next().map(PathBuf::from),
            other => {
                if path.is_none() {
                    path = Some(other.to_string());
                } else {
                    eprintln!("unexpected argument: {other}");
                    return ExitCode::from(2);
                }
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: vag-rod <file.rod> [--no-crack] [--cache <path>] [--dump <dir>]");
        return ExitCode::from(2);
    };

    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let cache_path = cache_path.unwrap_or_else(|| PathBuf::from(format!("{path}.ivcache.json")));

    let mut cache = IvCache::load(&cache_path);
    let sections = decode_rod_recover(&data, &file_name, &mut cache, run_crack);
    if let Err(e) = cache.save(&cache_path) {
        eprintln!("warning: could not write cache {}: {e}", cache_path.display());
    }

    if let Some(dir) = &dump_dir {
        let _ = std::fs::create_dir_all(dir);
        for s in &sections {
            if let Some(t) = &s.text {
                let bytes: Vec<u8> = t.chars().map(|c| c as u8).collect();
                let out = dir.join(format!("{}.bin", s.tag));
                if let Err(e) = std::fs::write(&out, &bytes) {
                    eprintln!("warning: could not write {}: {e}", out.display());
                }
            }
        }
    }

    println!("{path}: {} section(s)", sections.len());
    let mut ok = 0usize;
    for s in &sections {
        let (status, size, preview) = match (&s.status, &s.text) {
            (RodStatus::Undecodable, _) | (_, None) => {
                ("UNDECODABLE".to_string(), 0usize, String::new())
            }
            (st, Some(t)) => {
                ok += 1;
                let label = match st {
                    RodStatus::Tea => "tea",
                    RodStatus::Zlib => "zlib",
                    RodStatus::Undecodable => unreachable!(),
                };
                let preview: String = t
                    .chars()
                    .take(60)
                    .map(|c| if c.is_control() { '.' } else { c })
                    .collect();
                (label.to_string(), t.len(), preview)
            }
        };
        println!("  [{:<8}] {:>4}  {:>9} bytes  {}", s.tag, status, size, preview);
    }
    println!("decoded {ok}/{} section(s)", sections.len());
    ExitCode::SUCCESS
}
