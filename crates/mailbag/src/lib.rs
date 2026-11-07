//! Problem reports from desktop apps.
//!
//! A [`Report`] holds what the user says happened, the app and system it happened on, and the app's logs. [`Report::send`] posts it to a `mailbag-server`, which emails it to the developer, so the app never holds a mail password.
//!
//! ```no_run
//! let report = mailbag::Report::new("Paperback", "1.2.0", "The table of contents is empty for this book.").crashed(true).with_log("paperback.log", "...");
//! println!("{}", report.to_text());
//! report.send("https://reports.example.com/v1/report")?;
//! # Ok::<(), mailbag::SendError>(())
//! ```

use std::{
	env,
	fmt::{self, Display, Formatter, Write as _},
};

use serde::{Deserialize, Serialize};

/// The most of a log a report carries. A longer log keeps its end, where the lines leading up to a problem are.
pub const MAX_LOG_BYTES: usize = 1 << 20;
/// The most characters a description can have.
pub const MAX_DESCRIPTION_CHARS: usize = 20_000;
/// The most attachments a report can have.
pub const MAX_ATTACHMENTS: usize = 4;

/// A problem report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
	/// The app's name, such as `Paperback`.
	pub app: String,
	/// The app's version.
	pub version: String,
	/// The commit the app was built from.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub commit: Option<String>,
	/// The operating system and processor.
	pub os: String,
	/// What the user says happened.
	pub description: String,
	/// Where the developer can reply, if the user gave an address.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reply_to: Option<String>,
	/// Whether the app crashed on its last run.
	#[serde(default)]
	pub crashed: bool,
	/// Logs and other text files.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub attachments: Vec<Attachment>,
}

/// A text file sent with a report, such as a log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
	/// The file name, such as `paperback.log`.
	pub name: String,
	/// The file's text.
	pub text: String,
}

impl Report {
	/// A report from `app` at `version`, on the operating system and processor this was built for.
	#[must_use]
	pub fn new(app: impl Into<String>, version: impl Into<String>, description: impl Into<String>) -> Self {
		Self {
			app: app.into(),
			version: version.into(),
			commit: None,
			os: format!("{} {}", env::consts::OS, env::consts::ARCH),
			description: description.into(),
			reply_to: None,
			crashed: false,
			attachments: Vec::new(),
		}
	}

	/// Sets the commit the app was built from.
	#[must_use]
	pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
		self.commit = Some(commit.into());
		self
	}

	/// Replaces the operating system description, such as with one that includes its version.
	#[must_use]
	pub fn with_os(mut self, os: impl Into<String>) -> Self {
		self.os = os.into();
		self
	}

	/// Sets where the developer can reply. A blank address leaves it unset.
	#[must_use]
	pub fn with_reply_to(mut self, address: impl Into<String>) -> Self {
		let address = address.into();
		self.reply_to = if address.trim().is_empty() { None } else { Some(address.trim().to_owned()) };
		self
	}

	/// Sets whether the app crashed on its last run.
	#[must_use]
	pub const fn crashed(mut self, crashed: bool) -> Self {
		self.crashed = crashed;
		self
	}

	/// Attaches a log, keeping its last [`MAX_LOG_BYTES`] bytes. An empty log isn't attached.
	#[must_use]
	pub fn with_log(mut self, name: impl Into<String>, text: &str) -> Self {
		if !text.is_empty() {
			self.attachments.push(Attachment { name: name.into(), text: tail(text, MAX_LOG_BYTES).to_owned() });
		}
		self
	}

	/// The subject line of the report's email, such as `[Paperback 1.2.0] crash: The window closed`.
	#[must_use]
	pub fn subject(&self) -> String {
		let first_line = self.description.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default();
		let summary: String = first_line.chars().take(80).collect();
		let kind = if self.crashed { "crash: " } else { "" };
		format!("[{} {}] {kind}{summary}", self.app, self.version)
	}

	/// The description followed by the app and system details, as the email body shows them.
	#[must_use]
	pub fn body(&self) -> String {
		let mut body = format!("{}\n\nApp: {} {}", self.description.trim(), self.app, self.version);
		if let Some(commit) = &self.commit {
			let _ = write!(body, " (commit {commit})");
		}
		let _ =
			write!(body, "\nSystem: {}\nCrashed on its last run: {}", self.os, if self.crashed { "yes" } else { "no" });
		if let Some(reply_to) = &self.reply_to {
			let _ = write!(body, "\nReply to: {reply_to}");
		}
		body.push('\n');
		body
	}

	/// The whole report as text, attachments included, for showing the user before sending or saving to a file when sending fails.
	#[must_use]
	pub fn to_text(&self) -> String {
		let mut text = format!("{}\n\n{}", self.subject(), self.body());
		for attachment in &self.attachments {
			let _ = write!(text, "\n--- {} ---\n{}\n", attachment.name, attachment.text.trim_end());
		}
		text
	}

	/// Checks the report against the limits a server enforces.
	///
	/// # Errors
	///
	/// Returns the first problem found.
	pub fn validate(&self) -> Result<(), Invalid> {
		check_line("app", &self.app, 64)?;
		check_line("version", &self.version, 64)?;
		if let Some(commit) = &self.commit {
			check_line("commit", commit, 64)?;
		}
		check_line("os", &self.os, 200)?;
		if self.description.trim().is_empty() {
			return Err(Invalid::Empty("description"));
		}
		if self.description.chars().count() > MAX_DESCRIPTION_CHARS {
			return Err(Invalid::TooLong("description"));
		}
		if let Some(reply_to) = &self.reply_to {
			check_line("reply_to", reply_to, 254)?;
			let looks_like_address = reply_to.split_once('@').is_some_and(|(user, domain)| {
				!user.is_empty() && domain.contains('.') && !reply_to.contains(char::is_whitespace)
			});
			if !looks_like_address {
				return Err(Invalid::NotAnAddress);
			}
		}
		if self.attachments.len() > MAX_ATTACHMENTS {
			return Err(Invalid::TooMany("attachments"));
		}
		for attachment in &self.attachments {
			check_line("attachment name", &attachment.name, 64)?;
			// Kept to plain file names, since the name ends up in a mail header.
			if !attachment.name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) {
				return Err(Invalid::BadName);
			}
			if attachment.text.len() > MAX_LOG_BYTES {
				return Err(Invalid::TooLong("attachment"));
			}
		}
		Ok(())
	}

	/// Sends the report to the `mailbag-server` at `endpoint`, such as `https://reports.example.com/v1/report`.
	///
	/// # Errors
	///
	/// Returns why the server didn't take the report, or why it couldn't be reached. Offer to save [`Report::to_text`] instead.
	#[cfg(feature = "send")]
	pub fn send(&self, endpoint: &str) -> Result<(), SendError> {
		use std::{io::Write as _, time::Duration};

		use flate2::{Compression, write::GzEncoder};
		self.validate().map_err(SendError::Invalid)?;
		let json = serde_json::to_vec(self).map_err(|e| SendError::Failed(e.to_string()))?;
		let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
		encoder.write_all(&json).map_err(|e| SendError::Failed(e.to_string()))?;
		let body = encoder.finish().map_err(|e| SendError::Failed(e.to_string()))?;
		let agent: ureq::Agent = ureq::Agent::config_builder()
			.timeout_global(Some(Duration::from_secs(60)))
			.http_status_as_error(false)
			.build()
			.into();
		let mut response = agent
			.post(endpoint)
			.header("Content-Type", "application/json")
			.header("Content-Encoding", "gzip")
			.send(&body[..])
			.map_err(|e| SendError::Failed(e.to_string()))?;
		let status = response.status().as_u16();
		match status {
			200..=299 => Ok(()),
			422 => Err(SendError::ReplyAddressTakesNoMail),
			429 => Err(SendError::RateLimited),
			400..=499 => Err(SendError::Rejected(response.body_mut().read_to_string().unwrap_or_default())),
			_ => Err(SendError::Failed(format!("the server answered {status}"))),
		}
	}
}

/// Why a report isn't valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invalid {
	/// A required field is blank.
	Empty(&'static str),
	/// A field is longer than allowed.
	TooLong(&'static str),
	/// There are more of something than allowed.
	TooMany(&'static str),
	/// A single-line field contains a line break or other control character.
	ControlCharacter(&'static str),
	/// The reply address isn't an email address.
	NotAnAddress,
	/// An attachment name has characters other than letters, digits, `.`, `-` and `_`.
	BadName,
}

impl Display for Invalid {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		match self {
			Self::Empty(field) => write!(f, "{field} is empty"),
			Self::TooLong(field) => write!(f, "{field} is too long"),
			Self::TooMany(field) => write!(f, "too many {field}"),
			Self::ControlCharacter(field) => write!(f, "{field} has a control character"),
			Self::NotAnAddress => f.write_str("reply_to isn't an email address"),
			Self::BadName => {
				f.write_str("an attachment name has characters other than letters, digits, '.', '-' and '_'")
			}
		}
	}
}

impl std::error::Error for Invalid {}

/// Why sending a report failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
	/// The report breaks a limit, so it wasn't sent.
	Invalid(Invalid),
	/// The reply address's domain doesn't exist or takes no mail, which is usually a typo, so the user can correct it and send again.
	ReplyAddressTakesNoMail,
	/// The server has taken as many reports as it will for now.
	RateLimited,
	/// The server refused the report, for the reason given.
	Rejected(String),
	/// The server couldn't be reached, or failed to send the email.
	Failed(String),
}

impl Display for SendError {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		match self {
			Self::Invalid(invalid) => write!(f, "the report isn't valid: {invalid}"),
			Self::ReplyAddressTakesNoMail => f.write_str("the reply address's domain doesn't take email"),
			Self::RateLimited => f.write_str("too many reports have been sent; try again tomorrow"),
			Self::Rejected(reason) => write!(f, "the server refused the report: {reason}"),
			Self::Failed(reason) => write!(f, "the report couldn't be sent: {reason}"),
		}
	}
}

impl std::error::Error for SendError {}

fn check_line(field: &'static str, value: &str, max_chars: usize) -> Result<(), Invalid> {
	if value.trim().is_empty() {
		return Err(Invalid::Empty(field));
	}
	if value.chars().count() > max_chars {
		return Err(Invalid::TooLong(field));
	}
	if value.chars().any(char::is_control) {
		return Err(Invalid::ControlCharacter(field));
	}
	Ok(())
}

/// The last `max` bytes of `text`, moved forward to a character boundary.
fn tail(text: &str, max: usize) -> &str {
	if text.len() <= max {
		return text;
	}
	let mut start = text.len() - max;
	while !text.is_char_boundary(start) {
		start += 1;
	}
	&text[start..]
}

#[cfg(test)]
mod tests {
	use super::*;

	fn report() -> Report {
		Report::new("Paperback", "1.2.0", "The table of contents is empty.\nIt happens with every EPUB.")
			.with_os("windows x86_64")
	}

	#[test]
	fn the_subject_is_the_first_line_and_says_when_it_crashed() {
		assert_eq!(report().subject(), "[Paperback 1.2.0] The table of contents is empty.");
		assert_eq!(report().crashed(true).subject(), "[Paperback 1.2.0] crash: The table of contents is empty.");
	}

	#[test]
	fn the_body_lists_the_details() {
		let body = report().with_commit("abc123").with_reply_to("reader@example.com").body();
		assert_eq!(
			body,
			"The table of contents is empty.\nIt happens with every EPUB.\n\nApp: Paperback 1.2.0 (commit abc123)\nSystem: windows x86_64\nCrashed on its last run: no\nReply to: reader@example.com\n"
		);
	}

	#[test]
	fn a_long_log_keeps_its_end() {
		let log = format!("é{}", "x".repeat(MAX_LOG_BYTES));
		let report = report().with_log("paperback.log", &log);
		assert_eq!(report.attachments[0].text.len(), MAX_LOG_BYTES);
		assert!(report.attachments[0].text.ends_with('x'));
		assert!(report.validate().is_ok());
	}

	#[test]
	fn blank_logs_and_reply_addresses_are_left_out() {
		let report = report().with_log("paperback.log", "").with_reply_to("  ");
		assert_eq!(report.attachments, []);
		assert_eq!(report.reply_to, None);
	}

	#[test]
	fn a_line_break_in_a_header_field_is_refused() {
		let mut report = report();
		report.version = "1.2.0\nBcc: someone@example.com".into();
		assert_eq!(report.validate(), Err(Invalid::ControlCharacter("version")));
	}

	#[test]
	fn the_reply_address_must_look_like_one() {
		assert_eq!(report().with_reply_to("not an address").validate(), Err(Invalid::NotAnAddress));
		assert_eq!(report().with_reply_to("reader@localhost").validate(), Err(Invalid::NotAnAddress));
		assert!(report().with_reply_to("reader@example.com").validate().is_ok());
	}

	#[test]
	fn attachment_names_stay_plain() {
		assert_eq!(report().with_log("../secrets", "x").validate(), Err(Invalid::BadName));
	}

	#[test]
	fn an_empty_description_is_refused() {
		assert_eq!(Report::new("Paperback", "1.2.0", " \n ").validate(), Err(Invalid::Empty("description")));
	}

	#[test]
	fn the_text_ends_with_the_attachments() {
		let text = report().with_log("paperback.log", "line one\n").to_text();
		assert!(text.starts_with("[Paperback 1.2.0] The table of contents is empty.\n\nThe table of contents"));
		assert!(text.ends_with("\n--- paperback.log ---\nline one\n"));
	}

	#[test]
	fn a_report_survives_json() {
		let report = report().with_commit("abc").crashed(true).with_log("paperback.log", "x");
		let json = serde_json::to_vec(&report).unwrap();
		assert_eq!(serde_json::from_slice::<Report>(&json).unwrap(), report);
	}
}
