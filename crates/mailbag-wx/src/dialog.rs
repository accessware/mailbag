use std::{
	fs,
	path::{Path, PathBuf},
};

use mailbag::{Report, SendError};
use patois::t;
use wx_utils::{
	add_single_button_footer, add_yes_no_footer, build_yes_no_buttons, confirm, dialog_padding,
	progress::{Ended, run_with_progress},
	show_error, show_info,
};
use wxdragon::prelude::*;

use crate::Session;

/// What a report says about the app, and where it goes.
#[derive(Debug, Clone)]
pub struct Reporter {
	app: String,
	version: String,
	commit: Option<String>,
	os: Option<String>,
	endpoint: String,
	logs: Vec<PathBuf>,
}

impl Reporter {
	/// Reports for `app` at `version`, sent to the `mailbag-server` at `endpoint`, such as `https://reports.example.com/v1/report`.
	///
	/// `app` is also shown to the user, so pass the app's display name.
	#[must_use]
	pub fn new(app: impl Into<String>, version: impl Into<String>, endpoint: impl Into<String>) -> Self {
		Self {
			app: app.into(),
			version: version.into(),
			commit: None,
			os: None,
			endpoint: endpoint.into(),
			logs: Vec::new(),
		}
	}

	/// Sets the commit the app was built from.
	#[must_use]
	pub fn with_commit(mut self, commit: impl Into<String>) -> Self {
		self.commit = Some(commit.into());
		self
	}

	/// Replaces the operating system description, which is otherwise only its name and processor, such as `windows x86_64`.
	#[must_use]
	pub fn with_os(mut self, os: impl Into<String>) -> Self {
		self.os = Some(os.into());
		self
	}

	/// Attaches the log at `path`, under its file name, if it exists when the report is made. Its last megabyte is kept.
	///
	/// Pass the previous run's log too: after a crash, that's the one that has it.
	#[must_use]
	pub fn with_log(mut self, path: impl Into<PathBuf>) -> Self {
		self.logs.push(path.into());
		self
	}

	/// Asks whether to report the problem if `session` found that the last run ended unexpectedly, and shows the Report a Problem dialog if the user agrees.
	///
	/// The question has a Don't ask again check box, which [`Session`] remembers. Call it once the main window is showing.
	pub fn offer_after_crash(&self, parent: &dyn WxWidget, session: &Session) {
		if !session.ended_unexpectedly() || !session.asks_after_crash() {
			return;
		}
		// TRANSLATORS: Title of the question asked after the app closed unexpectedly
		let dialog = Dialog::builder(parent, &t("Report a Problem")).build();
		let padding = dialog_padding(&dialog);
		// TRANSLATORS: Asked at startup after the app crashed or was closed unexpectedly. %s is the app's name.
		let question = t("%s closed unexpectedly the last time it ran. Would you like to report the problem?")
			.replacen("%s", &self.app, 1);
		let label = StaticText::builder(&dialog).with_label(&question).build();
		// TRANSLATORS: Check box in the question asked after a crash, which stops the question being asked again
		let dont_ask = CheckBox::builder(&dialog).with_label(&t("&Don't ask again")).build();
		let (yes_button, no_button) = build_yes_no_buttons(&dialog);
		let content = BoxSizer::builder(Orientation::Vertical).build();
		content.add(&label, 0, SizerFlag::All, padding);
		content.add(&dont_ask, 0, SizerFlag::Left | SizerFlag::Right, padding);
		add_yes_no_footer(content, yes_button, no_button);
		dialog.set_sizer_and_fit(content, true);
		dialog.centre();
		let report = dialog.show_modal() == ID_YES;
		if dont_ask.is_checked() {
			session.set_asks_after_crash(false);
		}
		dialog.destroy();
		if report {
			self.show(parent, true);
		}
	}

	/// Shows the Report a Problem dialog. `crashed` marks the report as coming after a crash, as [`Reporter::offer_after_crash`] does.
	pub fn show(&self, parent: &dyn WxWidget, crashed: bool) {
		// TRANSLATORS: Title of the dialog for sending a problem report to the developer
		let title = t("Report a Problem");
		let dialog = Dialog::builder(parent, &title).build();
		let padding = dialog_padding(&dialog);
		let intro_text = if crashed {
			// TRANSLATORS: Shown at the top of the Report a Problem dialog after a crash. %s is the app's name.
			t("%s closed unexpectedly the last time it ran. Describe what you were doing when it happened.")
				.replacen("%s", &self.app, 1)
		} else {
			// TRANSLATORS: Shown at the top of the Report a Problem dialog
			t("Describe the problem, and what you were doing when it happened.")
		};
		let intro = StaticText::builder(&dialog).with_label(&intro_text).build();
		// TRANSLATORS: Label of the field for describing the problem in the Report a Problem dialog
		let description_label = StaticText::builder(&dialog).with_label(&t("&What happened:")).build();
		let description = TextCtrl::builder(&dialog)
			.with_style(TextCtrlStyle::MultiLine)
			.with_size(dialog.from_dip(Size::new(420, 160)))
			.build();
		// TRANSLATORS: Label of the optional email field in the Report a Problem dialog
		let email_label =
			StaticText::builder(&dialog).with_label(&t("&Your email address, if you'd like a reply:")).build();
		let email = TextCtrl::builder(&dialog).build();
		// TRANSLATORS: Check box in the Report a Problem dialog that attaches the app's log to the report
		let include_logs = CheckBox::builder(&dialog).with_label(&t("&Include the log")).build();
		include_logs.set_value(true);
		let has_logs = self.logs.iter().any(|path| path.is_file());
		include_logs.enable(has_logs);
		// TRANSLATORS: Explains what a problem report contains, in the Report a Problem dialog. %s is the app's name.
		let note_text = t("The report also includes %s's version and your operating system. Select Preview to see everything that will be sent.").replacen("%s", &self.app, 1);
		let note = StaticText::builder(&dialog).with_label(&note_text).build();
		// TRANSLATORS: Button in the Report a Problem dialog that shows the report before it is sent
		let preview_button = Button::builder(&dialog).with_label(&t("&Preview")).build();
		// A stock OK ID would end the dialog on click, before the report is checked and sent.
		// TRANSLATORS: Button in the Report a Problem dialog that sends the report
		let send_button = Button::builder(&dialog).with_label(&t("&Send")).build();
		// TRANSLATORS: Label for the cancellation button
		let cancel_button = Button::builder(&dialog).with_id(ID_CANCEL).with_label(&t("Cancel")).build();
		dialog.set_escape_id(ID_CANCEL);
		send_button.set_default();
		let content = BoxSizer::builder(Orientation::Vertical).build();
		content.add(&intro, 0, SizerFlag::All, padding);
		content.add(&description_label, 0, SizerFlag::Left | SizerFlag::Right, padding);
		content.add(&description, 1, SizerFlag::Expand | SizerFlag::All, padding);
		content.add(&email_label, 0, SizerFlag::Left | SizerFlag::Right, padding);
		content.add(&email, 0, SizerFlag::Expand | SizerFlag::All, padding);
		content.add(&include_logs, 0, SizerFlag::Left | SizerFlag::Right, padding);
		content.add(&note, 0, SizerFlag::All, padding);
		// The platform's own sizer, so Send and Cancel come in the order the platform uses.
		let standard_buttons = StdDialogButtonSizerBuilder::new().build();
		standard_buttons.set_affirmative_button(&send_button);
		standard_buttons.set_cancel_button(&cancel_button);
		standard_buttons.realize();
		let buttons = BoxSizer::builder(Orientation::Horizontal).build();
		buttons.add(&preview_button, 0, SizerFlag::AlignCenterVertical, 0);
		buttons.add_stretch_spacer(1);
		buttons.add_sizer(&standard_buttons, 0, SizerFlag::AlignCenterVertical, 0);
		content.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, padding);
		dialog.set_sizer_and_fit(content, true);
		let form = Form { reporter: self.clone(), crashed, description, email, include_logs };
		let preview_form = form.clone();
		preview_button.on_click(move |_| show_preview(dialog, &preview_form.report()));
		send_button.on_click(move |_| send(dialog, &form));
		dialog.centre();
		description.set_focus();
		dialog.show_modal();
		dialog.destroy();
	}
}

/// The dialog's fields, read when the user previews or sends.
#[derive(Clone)]
struct Form {
	reporter: Reporter,
	crashed: bool,
	description: TextCtrl,
	email: TextCtrl,
	include_logs: CheckBox,
}

impl Form {
	fn report(&self) -> Report {
		let reporter = &self.reporter;
		let mut report = Report::new(&reporter.app, &reporter.version, self.description.get_value())
			.with_reply_to(self.email.get_value())
			.crashed(self.crashed);
		if let Some(commit) = &reporter.commit {
			report = report.with_commit(commit);
		}
		if let Some(os) = &reporter.os {
			report = report.with_os(os);
		}
		if self.include_logs.is_checked() {
			for path in &reporter.logs {
				if let (Some(name), Ok(bytes)) = (path.file_name(), fs::read(path)) {
					report = report.with_log(name.to_string_lossy(), &String::from_utf8_lossy(&bytes));
				}
			}
		}
		report
	}
}

fn send(dialog: Dialog, form: &Form) {
	// TRANSLATORS: Title of an error message
	let error = t("Error");
	if form.description.get_value().trim().is_empty() {
		// TRANSLATORS: Shown when Send is pressed in the Report a Problem dialog with the description empty
		show_error(&dialog, t("Describe the problem before sending the report."), &error);
		form.description.set_focus();
		return;
	}
	let report = form.report();
	if let Err(invalid) = report.validate() {
		// TRANSLATORS: Shown when a problem report breaks a limit, such as being too long. %s is the reason, in English.
		show_error(&dialog, t("The report can't be sent: %s").replacen("%s", &invalid.to_string(), 1), &error);
		form.description.set_focus();
		return;
	}
	let endpoint = form.reporter.endpoint.clone();
	let to_send = report.clone();
	let email = form.email;
	run_with_progress(
		&dialog,
		// TRANSLATORS: Title of the progress window shown while a problem report is sent
		&t("Sending Report"),
		// TRANSLATORS: Message in the progress window shown while a problem report is sent
		&t("Sending the report..."),
		move |_| to_send.send(&endpoint),
		move |result, ended| {
			if ended == Ended::Cancelled && result.is_err() {
				return;
			}
			match result {
				Ok(()) => {
					// TRANSLATORS: Shown after a problem report was sent
					// TRANSLATORS: Title of the message shown after a problem report was sent
					show_info(&dialog, t("Thank you. Your report was sent."), &t("Report Sent"));
					dialog.end_modal(ID_OK);
				}
				Err(SendError::ReplyAddressTakesNoMail) => {
					// TRANSLATORS: Shown when the email address given in a problem report can't receive email
					show_error(
						&dialog,
						t("Email can't be delivered to that address. Check it for typing mistakes, or leave it blank."),
						&error,
					);
					email.set_focus();
				}
				Err(SendError::RateLimited) => {
					// TRANSLATORS: Shown when the report server has taken as many reports as it will for the day
					offer_to_save(dialog, &report, &t("Too many reports have been sent today."));
				}
				Err(SendError::Invalid(invalid)) => {
					show_error(
						&dialog,
						t("The report can't be sent: %s").replacen("%s", &invalid.to_string(), 1),
						&error,
					);
				}
				Err(SendError::Rejected(reason) | SendError::Failed(reason)) => {
					// TRANSLATORS: Shown when sending a problem report failed. %s is the reason, in English.
					offer_to_save(dialog, &report, &t("The report couldn't be sent: %s").replacen("%s", &reason, 1));
				}
			}
		},
	);
}

/// Sending failed, so the user can still pass the report on another way.
fn offer_to_save(dialog: Dialog, report: &Report, problem: &str) {
	// TRANSLATORS: Asked after a problem report couldn't be sent. %s explains what went wrong.
	let question = t("%s\n\nWould you like to save the report to a file, so you can send it another way?")
		.replacen("%s", problem, 1);
	// TRANSLATORS: Title of the question asked after a problem report couldn't be sent
	if !confirm(&dialog, question, &t("Report Not Sent")) {
		return;
	}
	let file_dialog = FileDialog::builder(&dialog)
		// TRANSLATORS: Title of the dialog for saving a problem report to a file
		.with_message(&t("Save Report"))
		// TRANSLATORS: Default file name for a saved problem report
		.with_default_file(&format!("{}.txt", t("Problem report")))
		// TRANSLATORS: File type filter in the dialog for saving a problem report
		.with_wildcard(&t("Text files (*.txt)|*.txt"))
		.with_style(FileDialogStyle::Save | FileDialogStyle::OverwritePrompt)
		.build();
	let path = if file_dialog.show_modal() == ID_OK { file_dialog.get_path() } else { None };
	file_dialog.destroy();
	let Some(path) = path else { return };
	match fs::write(Path::new(&path), report.to_text()) {
		Ok(()) => {
			// TRANSLATORS: Shown after a problem report was saved to a file
			// TRANSLATORS: Title of the message shown after a problem report was saved to a file
			show_info(&dialog, t("The report was saved."), &t("Report Saved"));
			dialog.end_modal(ID_OK);
		}
		// TRANSLATORS: Shown when saving a problem report to a file failed. %s is the reason.
		Err(e) => {
			show_error(&dialog, t("The report couldn't be saved: %s").replacen("%s", &e.to_string(), 1), &t("Error"));
		}
	}
}

fn show_preview(parent: Dialog, report: &Report) {
	// TRANSLATORS: Title of the window that shows a problem report before it is sent
	let dialog = Dialog::builder(&parent, &t("Report Preview")).build();
	let padding = dialog_padding(&dialog);
	let text = TextCtrl::builder(&dialog)
		.with_value(&report.to_text())
		.with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly | TextCtrlStyle::Rich2)
		.with_size(dialog.from_dip(Size::new(520, 360)))
		.build();
	// TRANSLATORS: Button that closes the report preview
	let close_button = Button::builder(&dialog).with_id(ID_CANCEL).with_label(&t("Close")).build();
	dialog.set_escape_id(ID_CANCEL);
	let content = BoxSizer::builder(Orientation::Vertical).build();
	content.add(&text, 1, SizerFlag::Expand | SizerFlag::All, padding);
	add_single_button_footer(content, close_button);
	dialog.set_sizer_and_fit(content, true);
	dialog.centre();
	text.set_focus();
	dialog.show_modal();
	dialog.destroy();
}
