//! Turning a report into an email, and sending it.

use std::future::Future;

use lettre::{
	AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
	message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
};
use mailbag::Report;

/// Where emails go. The real one is an SMTP server; tests keep them in memory.
pub trait Outbox: Send + Sync + 'static {
	fn send(&self, message: Message) -> impl Future<Output = Result<(), String>> + Send;
}

impl Outbox for AsyncSmtpTransport<Tokio1Executor> {
	async fn send(&self, message: Message) -> Result<(), String> {
		AsyncTransport::send(self, message).await.map(|_| ()).map_err(|e| e.to_string())
	}
}

/// The email for `report`, with its logs attached as text files.
///
/// # Errors
///
/// Returns why the reply address or the message couldn't be built.
pub fn compose(report: &Report, from: &Mailbox, to: &Mailbox) -> Result<Message, String> {
	let mut builder = Message::builder().from(from.clone()).to(to.clone()).subject(report.subject());
	if let Some(reply_to) = &report.reply_to {
		let reply_to: Mailbox = reply_to.parse().map_err(|_| "reply_to isn't an email address".to_owned())?;
		builder = builder.reply_to(reply_to);
	}
	let mut parts = MultiPart::mixed().singlepart(SinglePart::plain(report.body()));
	for attachment in &report.attachments {
		parts = parts.singlepart(
			Attachment::new(attachment.name.clone()).body(attachment.text.clone(), ContentType::TEXT_PLAIN),
		);
	}
	builder.multipart(parts).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn mailbox(address: &str) -> Mailbox {
		address.parse().unwrap()
	}

	#[test]
	fn the_email_carries_the_report() {
		let report = Report::new("Paperback", "1.2.0", "It crashed opening a PDF.")
			.crashed(true)
			.with_reply_to("reader@example.com")
			.with_log("paperback.log", "the last line");
		let message = compose(&report, &mailbox("sender@example.com"), &mailbox("developer@example.com")).unwrap();
		let text = String::from_utf8(message.formatted()).unwrap();
		assert!(text.contains("Subject: [Paperback 1.2.0] crash: It crashed opening a PDF."), "{text}");
		assert!(text.contains("Reply-To: reader@example.com"), "{text}");
		assert!(text.contains("To: developer@example.com"), "{text}");
		assert!(text.contains("filename=\"paperback.log\""), "{text}");
		assert!(text.contains("the last line"), "{text}");
	}
}
