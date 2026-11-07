//! Whether a reply address's domain takes mail, so a typo is caught while the user can still fix it.

use std::{future::Future, time::Duration};

use hickory_resolver::{TokioResolver, proto::rr::RData};

/// Answers whether a domain takes mail. The real one asks DNS; tests answer from a list.
pub trait Mailhosts: Send + Sync + 'static {
	/// `Some(false)` only when DNS says the domain takes no mail. `None` when the lookup itself failed, which shouldn't cost a real report.
	fn takes_mail(&self, domain: &str) -> impl Future<Output = Option<bool>> + Send;
}

impl Mailhosts for TokioResolver {
	async fn takes_mail(&self, domain: &str) -> Option<bool> {
		// The client waits up to a minute for the whole report, and sending the email needs most of that.
		tokio::time::timeout(Duration::from_secs(10), lookup(self, domain)).await.ok().flatten()
	}
}

async fn lookup(resolver: &TokioResolver, domain: &str) -> Option<bool> {
	// Fully qualified, so the server's own search domains aren't tried.
	let name = format!("{}.", domain.trim_end_matches('.'));
	let exchanges: Vec<bool> = match resolver.mx_lookup(name.as_str()).await {
		// An exchange of "." is a null MX (RFC 7505): the domain says it takes no mail.
		Ok(lookup) => lookup
			.answers()
			.iter()
			.filter_map(|record| match &record.data {
				RData::MX(mx) => Some(!mx.exchange.is_root()),
				_ => None,
			})
			.collect(),
		Err(e) if e.is_nx_domain() => return Some(false),
		Err(e) if e.is_no_records_found() => Vec::new(),
		Err(e) => {
			tracing::warn!(domain, error = %e, "looking up the reply address's mail servers failed");
			return None;
		}
	};
	if !exchanges.is_empty() {
		return Some(exchanges.contains(&true));
	}
	// With no MX record, mail goes to the domain's own address (RFC 5321).
	match resolver.lookup_ip(name.as_str()).await {
		Ok(lookup) => Some(lookup.iter().next().is_some()),
		Err(e) if e.is_no_records_found() => Some(false),
		Err(e) => {
			tracing::warn!(domain, error = %e, "looking up the reply address's domain failed");
			None
		}
	}
}
