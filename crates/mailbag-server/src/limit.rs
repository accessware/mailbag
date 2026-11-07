//! How many reports the server takes in a day.

use std::{
	collections::{HashMap, VecDeque},
	net::IpAddr,
	time::{Duration, Instant},
};

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Counts reports over the last day, for each address and in all.
pub struct Limiter {
	per_address: usize,
	total: usize,
	by_address: HashMap<IpAddr, VecDeque<Instant>>,
	all: VecDeque<Instant>,
}

impl Limiter {
	pub fn new(per_address: usize, total: usize) -> Self {
		Self { per_address, total, by_address: HashMap::new(), all: VecDeque::new() }
	}

	/// Counts a report from `address` at `now`, unless that would go over a limit.
	pub fn allow(&mut self, address: IpAddr, now: Instant) -> bool {
		let expired = |at: &Instant| now.duration_since(*at) >= DAY;
		while self.all.front().is_some_and(expired) {
			self.all.pop_front();
		}
		self.by_address.retain(|_, times| {
			while times.front().is_some_and(expired) {
				times.pop_front();
			}
			!times.is_empty()
		});
		let from_address = self.by_address.get(&address).map_or(0, VecDeque::len);
		if self.all.len() >= self.total || from_address >= self.per_address {
			return false;
		}
		self.all.push_back(now);
		self.by_address.entry(address).or_default().push_back(now);
		true
	}
}

#[cfg(test)]
mod tests {
	use std::net::Ipv4Addr;

	use super::*;

	const ONE: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
	const TWO: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 2));

	#[test]
	fn each_address_has_its_own_limit() {
		let mut limiter = Limiter::new(2, 10);
		let now = Instant::now();
		assert!(limiter.allow(ONE, now));
		assert!(limiter.allow(ONE, now));
		assert!(!limiter.allow(ONE, now));
		assert!(limiter.allow(TWO, now));
	}

	#[test]
	fn the_total_covers_every_address() {
		let mut limiter = Limiter::new(5, 2);
		let now = Instant::now();
		assert!(limiter.allow(ONE, now));
		assert!(limiter.allow(TWO, now));
		assert!(!limiter.allow(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 3)), now));
	}

	#[test]
	fn a_day_later_the_count_starts_again() {
		let mut limiter = Limiter::new(1, 10);
		let start = Instant::now();
		assert!(limiter.allow(ONE, start));
		assert!(!limiter.allow(ONE, start + Duration::from_secs(60)));
		assert!(limiter.allow(ONE, start + DAY));
	}
}
