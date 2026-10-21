//! Ctrl-C ends `vagcan` within a second, whatever the command is doing.
//!
//! Reported 2026-10-21: Ctrl-C ended no command. The handler was a `select!` branch on the
//! main task, and a command that blocks its thread — a question waiting for a line, a file
//! that does not open, label work — never let the branch run, while polling the branch had
//! already taken SIGINT's default away; and a menu, in raw mode, got Ctrl-C as a key.
//!
//! Each test runs the built binary in a pseudo-terminal, as a person runs it, presses
//! Ctrl-C the way a terminal does — the byte `0x03` on the terminal's side, which is a
//! SIGINT to the process group in line mode and a key in raw mode — and wants exit code 130
//! (128 + SIGINT, the handler's own exit) within a second. A process killed by the default
//! disposition has no code and fails here too: it closes no link and leaves the terminal as
//! the command had it.
//!
//! Hermetic: `HOME` and `VAGCAN_HOME` are a temporary directory with nothing in it, and no
//! command here opens an adapter — each stops before the port.

#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long Ctrl-C may take to end the process: the handler's grace for links to close,
/// and room for a loaded machine.
const ENDS_WITHIN: Duration = Duration::from_secs(1);

/// How long a command may take to reach the point the test presses Ctrl-C at.
const REACHES_WITHIN: Duration = Duration::from_secs(30);

/// Held from opening a terminal to starting the program on it. `openpty` opens without
/// close-on-exec, and a program another test started in between would inherit this
/// test's terminal and keep it open past its own program's end.
static STARTING: Mutex<()> = Mutex::new(());

/// `vagcan` on a pseudo-terminal, and everything it wrote to it.
struct Run {
	child: Child,
	/// The terminal's side, written to as a keyboard would be.
	master: File,
	/// The program's side, kept open here to read its mode.
	slave: OwnedFd,
	screen: Arc<Mutex<Vec<u8>>>,
}

impl Run {
	fn start(home: &Path, args: &[&str]) -> Run {
		let starting = STARTING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
		let (mut master, mut slave) = (0, 0);
		// SAFETY: two out-pointers to live integers; no name, no settings, no size.
		let opened = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
		assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
		// SAFETY: both descriptors were just opened and are owned by nothing else.
		let (master, slave) = unsafe { (File::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
		for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
			// SAFETY: a live descriptor.
			unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
		}
		let size = libc::winsize {
			ws_row: 40,
			ws_col: 120,
			ws_xpixel: 0,
			ws_ypixel: 0,
		};
		// SAFETY: a live descriptor and a pointer to a live winsize.
		unsafe { libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
		let stdio = || Stdio::from(slave.try_clone().unwrap());
		let mut command = Command::new(env!("CARGO_BIN_EXE_vagcan"));
		command
			.args(args)
			.env("HOME", home)
			.env("VAGCAN_HOME", home.join(".vagcan"))
			.env("TERM", "xterm-256color")
			.env_remove("VAGCAN_PROJECT")
			.stdin(stdio())
			.stdout(stdio())
			.stderr(stdio());
		// SAFETY: async-signal-safe calls only, between fork and exec. A session of its own
		// with the pseudo-terminal as its controlling terminal, so 0x03 is its SIGINT.
		unsafe {
			command.pre_exec(|| {
				if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
					return Err(std::io::Error::last_os_error());
				}
				Ok(())
			});
		}
		let child = command.spawn().unwrap();
		drop(starting);
		let screen = Arc::new(Mutex::new(Vec::new()));
		// Read all along, or a program writing more than the terminal buffers would block.
		let (mut reader, seen) = (master.try_clone().unwrap(), Arc::clone(&screen));
		std::thread::spawn(move || {
			let mut buf = [0u8; 4096];
			while let Ok(n @ 1..) = reader.read(&mut buf) {
				seen.lock().unwrap().extend_from_slice(&buf[..n]);
			}
		});
		Run {
			child,
			master,
			slave,
			screen,
		}
	}

	fn screen(&self) -> String {
		String::from_utf8_lossy(&self.screen.lock().unwrap()).into_owned()
	}

	/// Whether the terminal is in raw mode — no lines, no echo, no Ctrl-C of its own — or
	/// `None` once it cannot be read: macOS revokes a terminal when the session that owns
	/// it ends, so the mode is read while the program still runs.
	fn raw(&self) -> Option<bool> {
		// SAFETY: zeroed is a valid termios to be overwritten; the descriptor is live.
		let mut mode: libc::termios = unsafe { std::mem::zeroed() };
		match unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut mode) } {
			0 => Some(mode.c_lflag & (libc::ICANON | libc::ISIG) == 0),
			_ => None,
		}
	}

	/// Wait until `ready` holds while the program runs, failing with what is on screen if
	/// it ends first or never gets there.
	fn reach(&mut self, what: &str, ready: impl Fn(&Run) -> bool) {
		let started = Instant::now();
		while !ready(self) {
			if let Some(status) = self.child.try_wait().unwrap() {
				panic!("ended ({status}) before {what}; on screen:\n{}", self.screen());
			}
			assert!(started.elapsed() < REACHES_WITHIN, "never reached {what}; on screen:\n{}", self.screen());
			std::thread::sleep(Duration::from_millis(5));
		}
	}

	/// The terminal back in line mode before the program has ended: the handler hands it
	/// back at once and ends the process a moment later, which is the time to look.
	///
	/// A 130 exit before the mode was read counts too: the handler gives the terminal back
	/// before it exits, and on macOS the terminal cannot be read once the program has ended,
	/// so a loaded machine can let the exit land between two looks.
	fn assert_gives_the_terminal_back(&mut self) {
		let started = Instant::now();
		while self.raw() != Some(false) {
			if let Some(status) = self.child.try_wait().unwrap() {
				assert_eq!(
					status.code(),
					Some(130),
					"ended ({status}) with the terminal still raw; on screen:\n{}",
					self.screen()
				);
				return;
			}
			assert!(started.elapsed() < ENDS_WITHIN, "the terminal stayed raw; on screen:\n{}", self.screen());
			std::thread::sleep(Duration::from_millis(5));
		}
	}

	/// Ctrl-C as a terminal sends it.
	fn press_ctrl_c(&mut self) -> Instant {
		self.master.write_all(&[0x03]).unwrap();
		Instant::now()
	}

	/// Exit code 130 within [`ENDS_WITHIN`] of `since`.
	fn assert_interrupted(&mut self, since: Instant) {
		while since.elapsed() < ENDS_WITHIN {
			match self.child.try_wait().unwrap() {
				Some(status) if status.code() == Some(130) => return,
				Some(status) => panic!(
					"ended {:?} after Ctrl-C with {status}, not 130; on screen:\n{}",
					since.elapsed(),
					self.screen()
				),
				None => std::thread::sleep(Duration::from_millis(5)),
			}
		}
		panic!("still running {ENDS_WITHIN:?} after Ctrl-C; on screen:\n{}", self.screen());
	}
}

impl Drop for Run {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _: std::io::Result<ExitStatus> = self.child.wait();
	}
}

/// A home with nothing in it: no project, no car.
fn empty_home() -> tempfile::TempDir {
	tempfile::tempdir().unwrap()
}

#[test]
fn ctrl_c_at_a_question_ends_the_run() {
	// `faults` with no label data offers to fetch some and waits for a line: the main
	// thread is blocked in a read, which is the case a `select!` on the main task missed.
	let home = empty_home();
	let mut run = Run::start(home.path(), &["faults"]);
	run.reach("the question", |run| run.screen().contains("[y/N]"));
	// The question is written just before the read starts.
	std::thread::sleep(Duration::from_millis(100));
	let pressed = run.press_ctrl_c();
	run.assert_interrupted(pressed);
}

#[test]
fn ctrl_c_during_blocking_work_ends_the_run() {
	// A `.rod` that is a pipe nobody writes to: the read blocks the main thread the way a
	// parse or a key search holds it — but for good, so the test cannot pass by the work
	// finishing first.
	let home = empty_home();
	let fifo = home.path().join("blocked.rod");
	assert!(Command::new("mkfifo").arg(&fifo).status().unwrap().success());
	let mut run = Run::start(home.path(), &["dev", "vcds", "rod", fifo.to_str().unwrap()]);
	// A writer that does not wait opens only once a reader has: then the program is in its
	// open, and with this end held open and silent it goes on to a read that never returns.
	let started = Instant::now();
	let _writer = loop {
		match std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo) {
			Ok(writer) => break writer,
			Err(_) if started.elapsed() < REACHES_WITHIN && run.child.try_wait().unwrap().is_none() => {
				std::thread::sleep(Duration::from_millis(5));
			}
			Err(why) => panic!("the program never opened the pipe ({why}); on screen:\n{}", run.screen()),
		}
	};
	std::thread::sleep(Duration::from_millis(100));
	let pressed = run.press_ctrl_c();
	run.assert_interrupted(pressed);
}

#[test]
fn ctrl_c_in_a_menu_ends_the_run_and_gives_the_terminal_back() {
	// `setup` with no path asks with a menu, in raw mode: Ctrl-C arrives as a key there.
	// Waited for by the terminal's mode, not by the menu's words.
	let home = empty_home();
	let mut run = Run::start(home.path(), &["setup"]);
	run.reach("raw mode", |run| run.raw() == Some(true));
	let pressed = run.press_ctrl_c();
	run.assert_gives_the_terminal_back();
	run.assert_interrupted(pressed);
}

#[test]
fn a_sigint_while_a_menu_is_up_gives_the_terminal_back() {
	// A SIGINT that does not come from the keyboard (`kill -INT`) finds the menu in raw
	// mode; the handler ends the process from its own thread and hands the terminal back
	// first.
	let home = empty_home();
	let mut run = Run::start(home.path(), &["setup"]);
	run.reach("raw mode", |run| run.raw() == Some(true));
	let pid = libc::pid_t::try_from(run.child.id()).unwrap();
	// SAFETY: a signal to the child this test started and has not reaped.
	assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
	let sent = Instant::now();
	run.assert_gives_the_terminal_back();
	run.assert_interrupted(sent);
}
