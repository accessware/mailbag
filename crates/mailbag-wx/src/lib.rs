//! A Report a Problem dialog for wxDragon apps.
//!
//! [`Reporter::show`] asks the user what happened and sends it, with the app's logs, to a `mailbag-server`, which emails it to the developer. [`Session`] tells whether the last run crashed, so [`Reporter::offer_after_crash`] can offer the dialog at the next start. If sending fails, the user can save the report to a file instead.
//!
//! ```no_run
//! use mailbag_wx::{Reporter, Session};
//! use wxdragon::prelude::*;
//!
//! fn main() {
//!     let session = Session::begin("/path/to/config", "myapp");
//!     let _ = wxdragon::main(move |_| {
//!         let frame = Frame::builder().with_title("My App").build();
//!         let reporter = Reporter::new("My App", "1.2.0", "https://reports.example.com/v1/report")
//!             .with_log("/path/to/config/myapp.log")
//!             .with_log("/path/to/config/myapp.log.1");
//!         frame.show(true);
//!         reporter.offer_after_crash(&frame, &session);
//!     });
//! }
//! ```
//!
//! The dialog's strings are translated with patois. The app must use the same version of patois, 0.3, to translate them.

mod dialog;
mod session;

pub use dialog::Reporter;
pub use session::Session;
