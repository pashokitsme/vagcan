//! The serial port the sim is: a pseudo-terminal pair it creates itself.
//!
//! The tester opens the terminal side (`/dev/ttys…`, `/dev/pts/…`) as it opens a USB-CAN
//! adapter's port, through `--device`; the sim reads and writes the other side. Nothing else
//! is ever opened: there is no path to give the sim, so it cannot be pointed at a real
//! adapter or a bus.
//!
//! The sim keeps the terminal side open itself, so that one tester closing it does not end
//! the port for the next. It clears the exclusive-open flag the tester's serial library sets
//! (`TIOCEXCL`), because a tester killed before it could clear it would otherwise lock every
//! later one out, and it flushes what nobody read when a channel closes or opens.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::adapter::Adapter;

/// A pseudo-terminal pair.
pub struct Pty {
	master: OwnedFd,
	slave: OwnedFd,
	path: PathBuf,
}

fn check(result: libc::c_int) -> io::Result<libc::c_int> {
	if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

impl Pty {
	/// A new pair, its terminal side in raw mode: no echo, no line editing, no CR
	/// translation — bytes through as they are, which is what an slcan port carries.
	pub fn open() -> io::Result<Pty> {
		let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
		// SAFETY: openpty writes two descriptors; the name, termios and size are left out.
		check(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) })?;
		// SAFETY: both descriptors were just opened, and are owned here alone.
		let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
		let mut name = [0 as libc::c_char; 256];
		// SAFETY: the buffer is as long as said.
		let named = unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) };
		if named != 0 {
			return Err(io::Error::from_raw_os_error(named));
		}
		// SAFETY: ttyname_r wrote a NUL-terminated string.
		let path = PathBuf::from(unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned());
		raw(slave.as_raw_fd())?;
		for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
			// SAFETY: fcntl on an open descriptor.
			unsafe {
				let flags = check(libc::fcntl(fd, libc::F_GETFD))?;
				check(libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC))?;
			}
		}
		// SAFETY: as above.
		unsafe {
			let flags = check(libc::fcntl(master.as_raw_fd(), libc::F_GETFL))?;
			check(libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK))?;
		}
		Ok(Pty { master, slave, path })
	}

	/// The terminal side: what the tester opens with `--device`.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Let the next tester open the port, whatever the last one left set.
	fn allow_next_open(&self) {
		// SAFETY: an ioctl with no argument on an open terminal descriptor.
		unsafe {
			libc::ioctl(self.slave.as_raw_fd(), libc::TIOCNXCL as _);
		}
	}

	/// Drop what the sim wrote that nobody read: a closed channel's answers are not the
	/// next session's.
	fn flush_unread(&self) {
		// SAFETY: tcflush on an open terminal descriptor.
		unsafe {
			libc::tcflush(self.slave.as_raw_fd(), libc::TCIFLUSH);
		}
	}

	/// Write all of `bytes` to the tester, waiting at most `wait` for room. What does not
	/// fit by then is dropped and counted: a tester that stopped reading must not stop the car.
	fn write(&self, bytes: &[u8], wait: Duration) -> io::Result<usize> {
		let deadline = Instant::now() + wait;
		let mut written = 0;
		while written < bytes.len() {
			// SAFETY: writing from a live slice to an open descriptor.
			let n = unsafe { libc::write(self.master.as_raw_fd(), bytes[written..].as_ptr().cast(), bytes.len() - written) };
			if n >= 0 {
				written += n as usize;
				continue;
			}
			let err = io::Error::last_os_error();
			match err.kind() {
				io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted => {
					let left = deadline.saturating_duration_since(Instant::now());
					if left.is_zero() {
						return Ok(bytes.len() - written);
					}
					poll(self.master.as_raw_fd(), libc::POLLOUT, left)?;
				}
				_ => return Err(err),
			}
		}
		Ok(0)
	}
}

fn raw(fd: RawFd) -> io::Result<()> {
	let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
	// SAFETY: tcgetattr fills the struct on success, checked before it is read.
	unsafe {
		check(libc::tcgetattr(fd, termios.as_mut_ptr()))?;
		let mut termios = termios.assume_init();
		libc::cfmakeraw(&mut termios);
		check(libc::tcsetattr(fd, libc::TCSANOW, &termios))?;
	}
	Ok(())
}

/// Wait up to `wait` for `events` on `fd`; whether they came.
fn poll(fd: RawFd, events: libc::c_short, wait: Duration) -> io::Result<bool> {
	let mut pfd = libc::pollfd { fd, events, revents: 0 };
	let ms = wait.as_millis().min(i32::MAX as u128) as libc::c_int;
	// SAFETY: one pollfd, as said.
	let n = unsafe { libc::poll(&mut pfd, 1, ms) };
	match n {
		n if n > 0 => Ok(true),
		0 => Ok(false),
		_ => {
			let err = io::Error::last_os_error();
			if err.kind() == io::ErrorKind::Interrupted { Ok(false) } else { Err(err) }
		}
	}
}

/// How long one turn of [`serve`] waits for the tester when the car has nothing due.
const IDLE: Duration = Duration::from_millis(50);
/// How long the sim waits for the tester to make room before it drops what it says.
const WRITE_WAIT: Duration = Duration::from_millis(1000);

/// Serve `adapter` on `pty` until `stop` is set or the port fails. `notes` hears what the
/// adapter logs and the bus's events, as they happen.
pub fn serve(pty: &Pty, adapter: &mut Adapter, stop: &AtomicBool, mut notes: impl FnMut(Notice)) -> io::Result<()> {
	let started = Instant::now();
	let mut buf = [0u8; 1024];
	let mut out: Vec<u8> = Vec::new();
	let mut was_open = false;
	while !stop.load(Ordering::Relaxed) {
		let now = started.elapsed();
		let wait = adapter.next_due().map(|due| due.saturating_sub(now).min(IDLE)).unwrap_or(IDLE);
		if poll(pty.master.as_raw_fd(), libc::POLLIN, wait)? {
			// SAFETY: reading into a live buffer from an open descriptor.
			let n = unsafe { libc::read(pty.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
			if n > 0 {
				adapter.feed(started.elapsed(), &buf[..n as usize], &mut out);
			} else if n < 0 {
				let err = io::Error::last_os_error();
				// Linux says EIO on the master while no tester has the terminal side open
				// — it never does here, the sim holds it — and EAGAIN after a spurious wake.
				if !matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) && err.raw_os_error() != Some(libc::EIO) {
					return Err(err);
				}
				std::thread::sleep(Duration::from_millis(5));
			}
		}
		adapter.poll(started.elapsed(), &mut out);
		if adapter.is_open() != was_open {
			was_open = adapter.is_open();
			pty.flush_unread();
		}
		for note in adapter.log.drain(..) {
			notes(Notice::Adapter(note));
		}
		for event in adapter.bus.events.drain(..) {
			notes(Notice::Bus(event));
		}
		if !out.is_empty() {
			let dropped = pty.write(&out, WRITE_WAIT)?;
			if dropped > 0 {
				notes(Notice::Dropped(dropped));
			}
			out.clear();
		}
		pty.allow_next_open();
	}
	Ok(())
}

/// What [`serve`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
	Adapter(crate::adapter::Note),
	Bus(crate::bus::Event),
	/// Bytes the tester did not read in time, dropped.
	Dropped(usize),
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::bus::Bus;
	use crate::profile::Profile;
	use std::io::{Read as _, Write as _};
	use std::sync::Arc;

	/// The whole sim on a real pseudo-terminal, read and written as a tester would.
	#[test]
	fn a_tester_on_the_terminal_side_gets_its_answer() {
		let pty = Pty::open().unwrap();
		assert!(pty.path().is_absolute(), "{:?}", pty.path());
		let car = Profile::parse("[[unit]]\nrequest = \"7E0\"\n[unit.did]\nF40D = \"2A\"\n")
			.unwrap()
			.resolve()
			.unwrap();
		let mut port = std::fs::OpenOptions::new().read(true).write(true).open(pty.path()).unwrap();
		let stop = Arc::new(AtomicBool::new(false));
		let serving = {
			let stop = stop.clone();
			std::thread::spawn(move || {
				let mut adapter = Adapter::new(Bus::new(car));
				serve(&pty, &mut adapter, &stop, |_| {}).unwrap();
			})
		};
		port.write_all(b"C\rS6\rM0\rO\rt7E080322F40D00000000\r").unwrap();
		let mut got = Vec::new();
		let mut chunk = [0u8; 256];
		while !String::from_utf8_lossy(&got).ends_with("2A000000\r") {
			let n = port.read(&mut chunk).unwrap();
			got.extend_from_slice(&chunk[..n]);
		}
		stop.store(true, Ordering::Relaxed);
		serving.join().unwrap();
		assert_eq!(String::from_utf8_lossy(&got), "\r\r\r\rz\rt7E880462F40D2A000000\r");
	}
}
