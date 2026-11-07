//! Sends a test report, for checking a server end to end: `cargo run --example send -- https://reports.example.com/v1/report`.

fn main() {
	let endpoint =
		std::env::args().nth(1).expect("pass the report endpoint, such as https://reports.example.com/v1/report");
	let report =
		mailbag::Report::new("Paperback", env!("CARGO_PKG_VERSION"), "A test report from the mailbag send example.")
			.with_log("example.log", "If this arrived, the server works.\n");
	match report.send(&endpoint) {
		Ok(()) => println!("sent"),
		Err(e) => {
			eprintln!("{e}");
			std::process::exit(1);
		}
	}
}
