//! The HTTP side: taking a report, checking it, and handing its email to the outbox.

use std::{
	net::{IpAddr, SocketAddr},
	sync::{Arc, Mutex, PoisonError},
	time::Instant,
};

use axum::{
	Router,
	body::Bytes,
	extract::{ConnectInfo, DefaultBodyLimit, State},
	http::{HeaderMap, StatusCode},
	routing::{get, post},
};
use ipnet::IpNet;
use lettre::message::Mailbox;
use mailbag::Report;
use tower_http::{decompression::RequestDecompressionLayer, limit::RequestBodyLimitLayer};

use crate::{
	dns::Mailhosts,
	limit::Limiter,
	mail::{Outbox, compose},
};

/// The most a request can send, compressed. Logs compress well, so this leaves room for several full ones.
const MAX_REQUEST_BYTES: usize = 2 << 20;
/// The most a request can hold once decompressed: four full logs and a long description.
const MAX_REPORT_BYTES: usize = 6 << 20;

/// What every request shares.
pub struct Mailroom<O, H> {
	pub outbox: O,
	pub mailhosts: H,
	pub from: Mailbox,
	pub to: Mailbox,
	/// The apps whose reports are taken, in lowercase.
	pub apps: Vec<String>,
	/// Client addresses whose reports are refused before they're counted, so a blocked sender can't use up the day's reports.
	pub blocked: Vec<IpNet>,
	pub limiter: Mutex<Limiter>,
}

pub fn router<O: Outbox, H: Mailhosts>(mailroom: Arc<Mailroom<O, H>>) -> Router {
	Router::new()
		.route("/v1/report", post(receive::<O, H>))
		.route("/health", get(|| async { "ok" }))
		.layer(DefaultBodyLimit::max(MAX_REPORT_BYTES))
		.layer(RequestDecompressionLayer::new())
		.layer(RequestBodyLimitLayer::new(MAX_REQUEST_BYTES))
		.with_state(mailroom)
}

async fn receive<O: Outbox, H: Mailhosts>(
	State(mailroom): State<Arc<Mailroom<O, H>>>,
	ConnectInfo(peer): ConnectInfo<SocketAddr>,
	headers: HeaderMap,
	body: Bytes,
) -> (StatusCode, String) {
	let address = client_address(peer, &headers);
	if mailroom.blocked.iter().any(|range| range.contains(&address)) {
		tracing::debug!(%address, "blocked");
		return (StatusCode::FORBIDDEN, "reports from this address aren't taken".into());
	}
	// Counted before anything else, so junk costs its sender a report too.
	if !mailroom.limiter.lock().unwrap_or_else(PoisonError::into_inner).allow(address, Instant::now()) {
		tracing::warn!(%address, "rate limited");
		return (StatusCode::TOO_MANY_REQUESTS, "too many reports; try again tomorrow".into());
	}
	let report: Report = match serde_json::from_slice(&body) {
		Ok(report) => report,
		Err(e) => return (StatusCode::BAD_REQUEST, format!("not a report: {e}")),
	};
	if let Err(invalid) = report.validate() {
		return (StatusCode::BAD_REQUEST, invalid.to_string());
	}
	if !mailroom.apps.contains(&report.app.to_lowercase()) {
		return (StatusCode::FORBIDDEN, format!("reports for {} aren't taken here", report.app));
	}
	if let Some(domain) =
		report.reply_to.as_deref().and_then(|reply_to| reply_to.rsplit_once('@')).map(|(_, domain)| domain)
		&& mailroom.mailhosts.takes_mail(domain).await == Some(false)
	{
		return (StatusCode::UNPROCESSABLE_ENTITY, format!("{domain} doesn't take email"));
	}
	let message = match compose(&report, &mailroom.from, &mailroom.to) {
		Ok(message) => message,
		Err(e) => return (StatusCode::BAD_REQUEST, e),
	};
	match mailroom.outbox.send(message).await {
		Ok(()) => {
			tracing::info!(%address, app = %report.app, version = %report.version, crashed = report.crashed, "report sent");
			(StatusCode::ACCEPTED, "sent".into())
		}
		Err(e) => {
			tracing::error!(%address, app = %report.app, error = %e, "sending the email failed");
			(StatusCode::BAD_GATEWAY, "the email couldn't be sent".into())
		}
	}
}

/// The address a report came from. Behind Caddy on the same machine, that's the last address Caddy put in `X-Forwarded-For`, since Caddy replaces whatever the client sent there.
fn client_address(peer: SocketAddr, headers: &HeaderMap) -> IpAddr {
	if peer.ip().is_loopback() {
		let forwarded = headers
			.get("x-forwarded-for")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| value.rsplit(',').next())
			.and_then(|value| value.trim().parse().ok());
		if let Some(address) = forwarded {
			return address;
		}
	}
	peer.ip()
}

#[cfg(test)]
mod tests {
	use std::{future::Future, io::Write as _};

	use axum::{
		body::Body,
		extract::connect_info::MockConnectInfo,
		http::{Request, header},
	};
	use flate2::{Compression, write::GzEncoder};
	use lettre::Message;
	use tower::ServiceExt as _;

	use super::*;

	#[derive(Default)]
	struct Inbox {
		messages: Mutex<Vec<Message>>,
		fail: bool,
	}

	impl Outbox for Arc<Inbox> {
		fn send(&self, message: Message) -> impl Future<Output = Result<(), String>> + Send {
			let result = if self.fail {
				Err("the SMTP server is down".into())
			} else {
				self.messages.lock().unwrap().push(message);
				Ok(())
			};
			std::future::ready(result)
		}
	}

	/// Every domain takes mail but `gmial.com`, and looking up `flaky.example` fails.
	struct Dns;

	impl Mailhosts for Dns {
		fn takes_mail(&self, domain: &str) -> impl Future<Output = Option<bool>> + Send {
			std::future::ready(match domain {
				"gmial.com" => Some(false),
				"flaky.example" => None,
				_ => Some(true),
			})
		}
	}

	type TestMailroom = Arc<Mailroom<Arc<Inbox>, Dns>>;

	fn mailroom(inbox: &Arc<Inbox>, per_address: usize) -> TestMailroom {
		mailroom_limited(inbox, Limiter::new(per_address, 100))
	}

	fn mailroom_limited(inbox: &Arc<Inbox>, limiter: Limiter) -> TestMailroom {
		Arc::new(Mailroom {
			outbox: Arc::clone(inbox),
			mailhosts: Dns,
			from: "sender@example.com".parse().unwrap(),
			to: "developer@example.com".parse().unwrap(),
			apps: vec!["paperback".into(), "fedra".into()],
			blocked: vec!["192.0.2.0/24".parse().unwrap(), "2001:db8::1/128".parse().unwrap()],
			limiter: Mutex::new(limiter),
		})
	}

	fn report_json(app: &str) -> Vec<u8> {
		serde_json::to_vec(&Report::new(app, "1.2.0", "Something broke.").with_log("paperback.log", "a line")).unwrap()
	}

	async fn post_report(mailroom: TestMailroom, body: Vec<u8>, gzip: bool, forwarded_for: Option<&str>) -> StatusCode {
		let mut request = Request::post("/v1/report")
			.header(header::CONTENT_TYPE, "application/json")
			.header(header::CONTENT_LENGTH, body.len());
		if gzip {
			request = request.header(header::CONTENT_ENCODING, "gzip");
		}
		if let Some(forwarded_for) = forwarded_for {
			request = request.header("x-forwarded-for", forwarded_for);
		}
		let app = router(mailroom).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40000))));
		app.oneshot(request.body(Body::from(body)).unwrap()).await.unwrap().status()
	}

	fn gzip(bytes: &[u8]) -> Vec<u8> {
		let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
		encoder.write_all(bytes).unwrap();
		encoder.finish().unwrap()
	}

	#[tokio::test]
	async fn a_report_becomes_one_email() {
		let inbox = Arc::new(Inbox::default());
		assert_eq!(
			post_report(mailroom(&inbox, 10), report_json("Paperback"), false, None).await,
			StatusCode::ACCEPTED
		);
		let formatted: Vec<Vec<u8>> = inbox.messages.lock().unwrap().iter().map(Message::formatted).collect();
		assert_eq!(formatted.len(), 1);
		let text = String::from_utf8(formatted[0].clone()).unwrap();
		assert!(text.contains("Subject: [Paperback 1.2.0] Something broke."), "{text}");
	}

	#[tokio::test]
	async fn a_compressed_report_is_taken() {
		let inbox = Arc::new(Inbox::default());
		assert_eq!(
			post_report(mailroom(&inbox, 10), gzip(&report_json("Fedra")), true, None).await,
			StatusCode::ACCEPTED
		);
		assert_eq!(inbox.messages.lock().unwrap().len(), 1);
	}

	#[tokio::test]
	async fn other_apps_are_refused() {
		let inbox = Arc::new(Inbox::default());
		assert_eq!(
			post_report(mailroom(&inbox, 10), report_json("Spamware"), false, None).await,
			StatusCode::FORBIDDEN
		);
		assert!(inbox.messages.lock().unwrap().is_empty());
	}

	#[tokio::test]
	async fn an_invalid_report_is_refused() {
		let inbox = Arc::new(Inbox::default());
		let mut report = Report::new("Paperback", "1.2.0", "x");
		report.version = "1.2.0\r\nBcc: victim@example.com".into();
		assert_eq!(
			post_report(mailroom(&inbox, 10), serde_json::to_vec(&report).unwrap(), false, None).await,
			StatusCode::BAD_REQUEST
		);
		assert_eq!(post_report(mailroom(&inbox, 10), b"not json".to_vec(), false, None).await, StatusCode::BAD_REQUEST);
	}

	#[tokio::test]
	async fn each_client_address_is_limited_on_its_own() {
		let inbox = Arc::new(Inbox::default());
		let mailroom = mailroom(&inbox, 1);
		assert_eq!(
			post_report(Arc::clone(&mailroom), report_json("Paperback"), false, Some("198.51.100.1")).await,
			StatusCode::ACCEPTED
		);
		assert_eq!(
			post_report(Arc::clone(&mailroom), report_json("Paperback"), false, Some("198.51.100.1")).await,
			StatusCode::TOO_MANY_REQUESTS
		);
		assert_eq!(
			post_report(mailroom, report_json("Paperback"), false, Some("198.51.100.2")).await,
			StatusCode::ACCEPTED
		);
	}

	#[tokio::test]
	async fn blocked_addresses_are_refused_without_using_up_the_limit() {
		let inbox = Arc::new(Inbox::default());
		let mailroom = mailroom_limited(&inbox, Limiter::new(1, 1));
		for blocked in ["192.0.2.77", "2001:db8::1"] {
			assert_eq!(
				post_report(Arc::clone(&mailroom), report_json("Paperback"), false, Some(blocked)).await,
				StatusCode::FORBIDDEN
			);
		}
		assert_eq!(
			post_report(mailroom, report_json("Paperback"), false, Some("2001:db8::2")).await,
			StatusCode::ACCEPTED
		);
	}

	#[tokio::test]
	async fn a_reply_address_whose_domain_takes_no_mail_is_refused() {
		let inbox = Arc::new(Inbox::default());
		let with_reply_to =
			|address| serde_json::to_vec(&Report::new("Paperback", "1.2.0", "x").with_reply_to(address)).unwrap();
		assert_eq!(
			post_report(mailroom(&inbox, 10), with_reply_to("reader@gmial.com"), false, None).await,
			StatusCode::UNPROCESSABLE_ENTITY
		);
		assert_eq!(
			post_report(mailroom(&inbox, 10), with_reply_to("reader@flaky.example"), false, None).await,
			StatusCode::ACCEPTED
		);
		assert_eq!(inbox.messages.lock().unwrap().len(), 1);
	}

	#[tokio::test]
	async fn a_huge_request_is_refused() {
		let inbox = Arc::new(Inbox::default());
		assert_eq!(
			post_report(mailroom(&inbox, 10), vec![b' '; MAX_REQUEST_BYTES + 1], false, None).await,
			StatusCode::PAYLOAD_TOO_LARGE
		);
	}

	#[tokio::test]
	async fn a_report_that_grows_too_big_once_decompressed_is_refused() {
		let inbox = Arc::new(Inbox::default());
		let bomb = gzip(&vec![b' '; MAX_REPORT_BYTES + 1]);
		assert_eq!(post_report(mailroom(&inbox, 10), bomb, true, None).await, StatusCode::PAYLOAD_TOO_LARGE);
	}

	#[tokio::test]
	async fn a_failed_send_is_reported() {
		let inbox = Arc::new(Inbox { fail: true, ..Inbox::default() });
		assert_eq!(
			post_report(mailroom(&inbox, 10), report_json("Paperback"), false, None).await,
			StatusCode::BAD_GATEWAY
		);
	}

	#[test]
	fn only_a_local_proxy_is_believed_about_the_client() {
		let mut headers = HeaderMap::new();
		headers.insert("x-forwarded-for", "203.0.113.9, 192.0.2.1".parse().unwrap());
		assert_eq!(client_address(SocketAddr::from(([127, 0, 0, 1], 1)), &headers), IpAddr::from([192, 0, 2, 1]));
		assert_eq!(client_address(SocketAddr::from(([198, 51, 100, 7], 1)), &headers), IpAddr::from([198, 51, 100, 7]));
	}
}
