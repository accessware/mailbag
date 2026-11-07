//! Receives mailbag problem reports and emails each one to the developer.
//!
//! Configured by environment variables, listed in the README. It listens on the loopback address by default, for a reverse proxy such as Caddy to put HTTPS in front of it.

mod dns;
mod limit;
mod mail;
mod server;

use std::{
	env, fs,
	net::SocketAddr,
	process::ExitCode,
	sync::{Arc, Mutex},
};

use hickory_resolver::{ResolverBuilder, TokioResolver};
use ipnet::IpNet;
use lettre::{AsyncSmtpTransport, Tokio1Executor, message::Mailbox, transport::smtp::authentication::Credentials};

use crate::{limit::Limiter, server::Mailroom};

struct Config {
	listen: SocketAddr,
	smtp_host: String,
	smtp_port: u16,
	smtp_user: String,
	smtp_password: String,
	from: Mailbox,
	to: Mailbox,
	apps: Vec<String>,
	blocked: Vec<IpNet>,
	per_address: usize,
	total: usize,
}

impl Config {
	fn from_env() -> Result<Self, String> {
		let smtp_user = required("MAILBAG_SMTP_USER")?;
		// A file, so a container secret can hold the password instead of the environment.
		let smtp_password = match env::var("MAILBAG_SMTP_PASSWORD_FILE") {
			Ok(path) => fs::read_to_string(&path)
				.map_err(|e| format!("MAILBAG_SMTP_PASSWORD_FILE {path}: {e}"))?
				.trim()
				.to_owned(),
			Err(_) => required("MAILBAG_SMTP_PASSWORD")?,
		};
		let from = optional("MAILBAG_FROM").unwrap_or_else(|| format!("mailbag <{smtp_user}>"));
		let apps: Vec<String> = required("MAILBAG_APPS")?
			.split(',')
			.map(|app| app.trim().to_lowercase())
			.filter(|app| !app.is_empty())
			.collect();
		if apps.is_empty() {
			return Err("MAILBAG_APPS names no apps".into());
		}
		let blocked = optional("MAILBAG_BLOCK")
			.unwrap_or_default()
			.split(|c: char| c == ',' || c.is_whitespace())
			.filter(|entry| !entry.trim().is_empty())
			.map(|entry| {
				let entry = entry.trim();
				entry
					.parse::<IpNet>()
					.or_else(|_| entry.parse::<std::net::IpAddr>().map(IpNet::from))
					.map_err(|_| format!("MAILBAG_BLOCK {entry:?} isn't an address or range"))
			})
			.collect::<Result<_, _>>()?;
		Ok(Self {
			listen: parse("MAILBAG_LISTEN", optional("MAILBAG_LISTEN").as_deref().unwrap_or("127.0.0.1:8025"))?,
			smtp_host: optional("MAILBAG_SMTP_HOST").unwrap_or_else(|| "smtp.gmail.com".into()),
			smtp_port: parse("MAILBAG_SMTP_PORT", optional("MAILBAG_SMTP_PORT").as_deref().unwrap_or("587"))?,
			smtp_user,
			smtp_password,
			from: parse("MAILBAG_FROM", &from)?,
			to: parse("MAILBAG_TO", &required("MAILBAG_TO")?)?,
			apps,
			blocked,
			per_address: parse(
				"MAILBAG_PER_ADDRESS_PER_DAY",
				optional("MAILBAG_PER_ADDRESS_PER_DAY").as_deref().unwrap_or("5"),
			)?,
			// Well under the roughly 500 messages a day after which Gmail stops an account sending.
			total: parse("MAILBAG_PER_DAY", optional("MAILBAG_PER_DAY").as_deref().unwrap_or("100"))?,
		})
	}

	fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, String> {
		// Port 465 starts with TLS; any other port, such as 587, upgrades with STARTTLS.
		let builder = if self.smtp_port == 465 {
			AsyncSmtpTransport::<Tokio1Executor>::relay(&self.smtp_host)
		} else {
			AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.smtp_host)
		};
		let builder = builder.map_err(|e| format!("SMTP host {}: {e}", self.smtp_host))?;
		Ok(builder
			.port(self.smtp_port)
			.credentials(Credentials::new(self.smtp_user.clone(), self.smtp_password.clone()))
			.build())
	}
}

fn optional(name: &str) -> Option<String> {
	env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn required(name: &str) -> Result<String, String> {
	optional(name).ok_or_else(|| format!("{name} isn't set"))
}

fn parse<T: std::str::FromStr>(name: &str, value: &str) -> Result<T, String>
where
	T::Err: std::fmt::Display,
{
	value.trim().parse().map_err(|e| format!("{name} {value:?}: {e}"))
}

#[tokio::main]
async fn main() -> ExitCode {
	// Colour only for a person watching; journald would store the escape codes.
	tracing_subscriber::fmt().with_target(false).with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout())).init();
	let config = match Config::from_env() {
		Ok(config) => config,
		Err(e) => {
			tracing::error!("{e}");
			return ExitCode::FAILURE;
		}
	};
	let outbox = match config.transport() {
		Ok(outbox) => outbox,
		Err(e) => {
			tracing::error!("{e}");
			return ExitCode::FAILURE;
		}
	};
	let mailhosts = match TokioResolver::builder_tokio().and_then(ResolverBuilder::build) {
		Ok(resolver) => resolver,
		Err(e) => {
			tracing::error!("reading the DNS settings: {e}");
			return ExitCode::FAILURE;
		}
	};
	let mailroom = Arc::new(Mailroom {
		outbox,
		mailhosts,
		from: config.from,
		to: config.to,
		apps: config.apps,
		blocked: config.blocked,
		limiter: Mutex::new(Limiter::new(config.per_address, config.total)),
	});
	let listener = match tokio::net::TcpListener::bind(config.listen).await {
		Ok(listener) => listener,
		Err(e) => {
			tracing::error!("listening on {}: {e}", config.listen);
			return ExitCode::FAILURE;
		}
	};
	tracing::info!(listen = %config.listen, "taking reports");
	let app = server::router(mailroom).into_make_service_with_connect_info::<SocketAddr>();
	if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown()).await {
		tracing::error!("{e}");
		return ExitCode::FAILURE;
	}
	ExitCode::SUCCESS
}

/// Finishes when the container is stopped.
async fn shutdown() {
	#[cfg(unix)]
	{
		let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) else {
			return;
		};
		tokio::select! {
			_ = tokio::signal::ctrl_c() => {}
			_ = terminate.recv() => {}
		}
	}
	#[cfg(not(unix))]
	let _ = tokio::signal::ctrl_c().await;
}
