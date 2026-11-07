use std::{
	fs::{self, File, OpenOptions},
	io::{Read, Seek, SeekFrom, Write},
	path::{Path, PathBuf},
};

const RUNNING: &str = "running";

/// Tells whether the app's last run ended without exiting normally, such as by crashing.
///
/// [`Session::begin`] marks a file as running, and [`Session::end`] clears the mark. A mark that is still there at the next start means the last run never got to `end`: it crashed, was killed, or the computer lost power. The file stays locked while the app runs, so a second copy of the app, which finds it locked, doesn't mistake the first copy for a crash.
#[derive(Debug)]
pub struct Session {
	file: Option<File>,
	ended_unexpectedly: bool,
	/// Exists once the user has asked not to be offered a report after a crash.
	no_crash_question: PathBuf,
}

impl Session {
	/// Marks this run as running, in a file named after `name` in `dir`, and finds out how the last run ended.
	///
	/// Call it early in `main`, so a crash during startup counts. If the file can't be written, the session never reports a crash.
	#[must_use]
	pub fn begin(dir: impl AsRef<Path>, name: &str) -> Self {
		let dir = dir.as_ref();
		let _ = fs::create_dir_all(dir);
		Self::begin_at(&dir.join(format!("{name}.running")))
	}

	fn begin_at(path: &Path) -> Self {
		let no_crash_question = path.with_extension("no-crash-question");
		let unknown = Self { file: None, ended_unexpectedly: false, no_crash_question: no_crash_question.clone() };
		let Ok(mut file) = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path) else {
			return unknown;
		};
		// Another copy of the app holds the lock, so the mark is its own and not a crash.
		if file.try_lock().is_err() {
			return unknown;
		}
		let mut contents = String::new();
		let ended_unexpectedly = file.read_to_string(&mut contents).is_ok() && contents.trim() == RUNNING;
		let marked = file.set_len(0).is_ok()
			&& file.seek(SeekFrom::Start(0)).is_ok()
			&& file.write_all(RUNNING.as_bytes()).is_ok()
			&& file.sync_all().is_ok();
		if !marked {
			return Self { file: None, ended_unexpectedly, no_crash_question };
		}
		Self { file: Some(file), ended_unexpectedly, no_crash_question }
	}

	/// Whether the last run ended without calling [`Session::end`].
	#[must_use]
	pub const fn ended_unexpectedly(&self) -> bool {
		self.ended_unexpectedly
	}

	/// Whether [`Reporter::offer_after_crash`](crate::Reporter::offer_after_crash) asks to report a crash. It stops asking once the user checks Don't ask again.
	#[must_use]
	pub fn asks_after_crash(&self) -> bool {
		!self.no_crash_question.exists()
	}

	/// Turns the question asked after a crash on or off, such as from a setting in the app.
	pub fn set_asks_after_crash(&self, ask: bool) {
		if ask {
			let _ = fs::remove_file(&self.no_crash_question);
		} else {
			let _ = fs::write(&self.no_crash_question, "");
		}
	}

	/// Records that this run exited normally. Call it as the app exits; a run that never calls it counts as having ended unexpectedly.
	pub fn end(self) {
		// Emptied rather than deleted, since Windows can't delete a file that is open.
		if let Some(file) = self.file {
			let _ = file.set_len(0);
			let _ = file.sync_all();
		}
	}
}

#[cfg(test)]
mod tests {
	use std::path::PathBuf;

	use super::*;

	fn marker(test: &str) -> PathBuf {
		let dir = std::env::temp_dir().join(format!("mailbag-wx-session-{test}-{}", std::process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir.join("app.running")
	}

	#[test]
	fn a_first_run_and_a_run_after_a_normal_exit_are_not_crashes() {
		let path = marker("normal");
		let first = Session::begin_at(&path);
		assert!(!first.ended_unexpectedly());
		first.end();
		assert!(!Session::begin_at(&path).ended_unexpectedly());
	}

	#[test]
	fn a_run_that_never_ended_is_reported() {
		let path = marker("crash");
		drop(Session::begin_at(&path));
		assert!(Session::begin_at(&path).ended_unexpectedly());
	}

	#[test]
	fn a_second_copy_of_the_app_is_not_mistaken_for_a_crash() {
		let path = marker("second");
		let first = Session::begin_at(&path);
		let second = Session::begin_at(&path);
		assert!(!second.ended_unexpectedly());
		second.end();
		first.end();
		assert!(!Session::begin_at(&path).ended_unexpectedly());
	}

	#[test]
	fn the_crash_question_can_be_turned_off_and_back_on() {
		let path = marker("question");
		let session = Session::begin_at(&path);
		assert!(session.asks_after_crash());
		session.set_asks_after_crash(false);
		assert!(!Session::begin_at(&path).asks_after_crash());
		session.set_asks_after_crash(true);
		assert!(session.asks_after_crash());
	}
}
