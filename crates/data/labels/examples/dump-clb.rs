//! Print a `.clb` label file as plain text.
//!
//! The label files are the tool's data source, so being able to look at one file the
//! way the parser sees it is worth a permanent example rather than a throwaway
//! script.
fn main() {
	let Some(path) = std::env::args().nth(1) else {
		eprintln!("usage: dump-clb <file.clb>");
		std::process::exit(2);
	};
	let bytes = std::fs::read(&path).expect("reading the label file");
	let text = vag_data_labels::clb::decrypt_clb(&bytes);
	print!("{}", String::from_utf8_lossy(&text));
}
