//! The Report a Problem dialog, for trying it by hand.
//!
//! Run it with a server address to send real reports, such as `cargo run -p mailbag-wx --example report -- https://reports.example.com/v1/report`. Choose Simulate a Crash, then run it again, to see the question asked after a crash.
use std::{cell::RefCell, env, fs, process};

use mailbag_wx::{Reporter, Session};
use wxdragon::prelude::*;

fn main() {
	let endpoint = env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8025/v1/report".into());
	let dir = env::temp_dir().join("mailbag-wx-example");
	let session = Session::begin(&dir, "example");
	let log = dir.join("example.log");
	let _ =
		fs::write(&log, "12:00:01 INFO starting\n12:00:02 INFO opened a book\n12:00:03 ERROR something went wrong\n");
	let _ = wxdragon::main(move |_| {
		let reporter = Reporter::new("Paperback", "mailbag-wx example", &endpoint).with_log(&log);
		let frame = Frame::builder().with_title("Report a Problem example").build();
		let panel = Panel::builder(&frame).build();
		let report = Button::builder(&panel).with_label("&Report a Problem...").build();
		let crash = Button::builder(&panel).with_label("Simulate a &Crash").build();
		let offer = reporter.clone();
		report.on_click(move |_| reporter.show(&frame, false));
		// Exits without ending the session, as a crash would.
		crash.on_click(|_| process::exit(1));
		let sizer = BoxSizer::builder(Orientation::Vertical).build();
		sizer.add(&report, 0, SizerFlag::All, 10);
		sizer.add(&crash, 0, SizerFlag::All, 10);
		panel.set_sizer(sizer, true);
		frame.set_size(Size::new(400, 200));
		frame.show(true);
		report.set_focus();
		offer.offer_after_crash(&frame, &session);
		let session = RefCell::new(Some(session));
		frame.on_close(move |event| {
			if let Some(session) = session.borrow_mut().take() {
				session.end();
			}
			event.skip(true);
		});
	});
}
