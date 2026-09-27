//! Settings that survive the ignition being switched off.
//!
//! # Where it goes
//!
//! In its own flash partition, labelled `config` — found by reading the
//! partition table at run time, never by an offset in the source. The table is
//! the one authority on where anything lives; a constant here would be correct
//! until the first time `partitions.csv` changed, and then wrong in a way that
//! corrupts rather than fails.
//!
//! # How it survives a power cut
//!
//! Two slots, one flash sector each, and a generation counter. A save always
//! writes the slot that is *not* current, then it becomes current by having the
//! higher generation. Lose power mid-write and the damaged slot is the old one;
//! the previous configuration is still whole in the other. There is no moment
//! at which both are invalid.
//!
//! A CRC over the payload is what makes "damaged" detectable at all. Erased
//! flash reads as `0xff`, which is a perfectly plausible-looking blob if
//! nothing checks it.

use crate::config::{Config, SCHEMA_VERSION};
use crate::schema::{self, Held, Slot};
use embedded_storage::{ReadStorage, Storage};
use esp_bootloader_esp_idf::partitions;
use esp_storage::FlashStorage;

/// The label in `partitions.csv`. Matching by label rather than by subtype
/// because subtypes are a small set and several partitions can share
/// `undefined`; a label is what a person wrote down on purpose.
const PARTITION_LABEL: &str = "config";

/// One flash sector. The erase granularity is the slot granularity: a slot
/// smaller than a sector could not be rewritten without disturbing its
/// neighbour, which is precisely what the two-slot scheme exists to avoid.
const SLOT_SIZE: u32 = FlashStorage::SECTOR_SIZE;
const SLOTS: u32 = 2;

/// `VDSH`, little-endian. Cheap first rejection of an erased or foreign sector
/// before the CRC is computed.
const MAGIC: u32 = 0x4853_4456;
const HEADER_LEN: usize = 16;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
	/// The partition table has no `config` partition — the firmware was
	/// flashed against the default table.
	NoPartition,
	/// The partition exists but is too small for two slots.
	PartitionTooSmall,
	Flash,
	/// Nothing valid is stored yet. Not a failure: it is what a new board says.
	Empty,
	/// The configuration does not fit in a slot.
	TooBig,
}

pub struct Store {
	flash: FlashStorage,
	/// Absolute flash address of the partition, from the table.
	offset: u32,
	/// Its full length, also from the table. Larger than what the two slots
	/// use — room to grow without moving anything.
	len: u32,
	/// The generation of what is currently stored, 0 if nothing is — the newest record
	/// whatever its version, so a save never goes out under a lower one.
	generation: u32,
	/// Which slot holds it: a save goes to the other.
	current: u32,
	/// The newest record's version, where this image cannot read it ([`Held::unreadable`]).
	unreadable: Option<u16>,
}

impl Store {
	/// Reads the partition table and locates the settings partition.
	///
	/// Allocates the table buffer (3 KB) and drops it before returning: this
	/// runs once, at start-up, which is the only time this firmware allocates
	/// anything at all.
	pub fn open() -> Result<Self, Error> {
		let mut flash = FlashStorage::new();
		let mut buffer = alloc::vec![0u8; partitions::PARTITION_TABLE_MAX_LEN];
		let table = partitions::read_partition_table(&mut flash, &mut buffer).map_err(|_| Error::Flash)?;

		let mut found = None;
		for index in 0..table.len() {
			let entry = table.get_partition(index).map_err(|_| Error::Flash)?;
			if entry.label_as_str() == PARTITION_LABEL {
				found = Some((entry.offset(), entry.len()));
				break;
			}
		}
		let (offset, len) = found.ok_or(Error::NoPartition)?;
		if len < SLOT_SIZE * SLOTS {
			return Err(Error::PartitionTooSmall);
		}

		let mut store = Self {
			flash,
			offset,
			len,
			generation: 0,
			current: 0,
			unreadable: None,
		};
		// Learn which slot is live now, so the first save goes to the other one
		// even if nothing has been read yet.
		store.scan()?;
		Ok(store)
	}

	/// Where it is, how big it is, and how much of that the slots occupy.
	pub fn partition(&self) -> (u32, u32, u32) {
		(self.offset, self.len, SLOT_SIZE * SLOTS)
	}

	pub fn generation(&self) -> u32 {
		self.generation
	}

	/// The version of the newest record in flash where this image cannot read it: a newer
	/// image wrote it. [`Store::load`] then gives the record before it, if there is one; a
	/// write nobody asked for must not go over it — only an explicit `save` does.
	pub fn unreadable(&self) -> Option<u16> {
		self.unreadable
	}

	/// The stored configuration, or `Error::Empty` on a board that has never
	/// been configured. The caller decides what to do about that; this does
	/// not quietly substitute defaults, because "never saved" and "saved these
	/// defaults" are different facts and only one of them is a bug.
	///
	/// The newest record this image can read: behind one it cannot
	/// ([`Store::unreadable`]), the one before it — or `Error::Empty` with none.
	pub fn load(&mut self) -> Result<Config, Error> {
		self.scan()?.config.ok_or(Error::Empty)
	}

	/// Writes to the slot that is not current, then that slot is current.
	/// Returns the new generation.
	pub fn save(&mut self, config: &Config) -> Result<u32, Error> {
		let mut payload = [0u8; SLOT_SIZE as usize - HEADER_LEN];
		let payload = postcard::to_slice(config, &mut payload).map_err(|_| Error::TooBig)?;

		let generation = self.generation.wrapping_add(1);
		let target = (self.current + 1) % SLOTS;

		let mut sector = alloc::vec![0u8; HEADER_LEN + payload.len()];
		sector[0..4].copy_from_slice(&MAGIC.to_le_bytes());
		sector[4..6].copy_from_slice(&SCHEMA_VERSION.to_le_bytes());
		sector[6..8].copy_from_slice(&(payload.len() as u16).to_le_bytes());
		sector[8..12].copy_from_slice(&generation.to_le_bytes());
		sector[12..16].copy_from_slice(&CRC.checksum(payload).to_le_bytes());
		sector[HEADER_LEN..].copy_from_slice(payload);

		self.flash.write(self.offset + target * SLOT_SIZE, &sector).map_err(|_| Error::Flash)?;

		self.current = target;
		self.generation = generation;
		self.unreadable = None;
		Ok(generation)
	}

	/// Erases both slots. A board with nothing stored boots on defaults, so
	/// this is "forget my settings", not "break the device".
	pub fn erase(&mut self) -> Result<(), Error> {
		let blank = [0u8; HEADER_LEN];
		for slot in 0..SLOTS {
			self.flash.write(self.offset + slot * SLOT_SIZE, &blank).map_err(|_| Error::Flash)?;
		}
		self.current = 0;
		self.generation = 0;
		self.unreadable = None;
		Ok(())
	}

	/// Reads both slots, learns where the newest record is — whatever its version — and says
	/// what of flash this image can use ([`schema::held`]).
	fn scan(&mut self) -> Result<Held, Error> {
		let mut slots: [Option<Slot>; SLOTS as usize] = [const { None }; SLOTS as usize];
		for (slot, read) in (0..SLOTS).zip(slots.iter_mut()) {
			// A half-written slot is expected, not exceptional: it is what a power cut during
			// a save leaves behind, and it is no record.
			*read = self.read_slot(slot)?;
		}
		let held = schema::held(slots);
		if let Some((slot, generation)) = held.newest {
			self.current = slot;
			self.generation = generation;
		}
		self.unreadable = held.unreadable;
		Ok(held)
	}

	/// One slot's record, if its magic, its length and its CRC check out: `None` for an erased
	/// slot, a foreign one, or one a power cut left half written.
	fn read_slot(&mut self, slot: u32) -> Result<Option<Slot>, Error> {
		let base = self.offset + slot * SLOT_SIZE;
		let mut header = [0u8; HEADER_LEN];
		self.flash.read(base, &mut header).map_err(|_| Error::Flash)?;

		if u32::from_le_bytes(header[0..4].try_into().unwrap()) != MAGIC {
			return Ok(None);
		}
		// Any version `schema::decode` knows is read and carried forward; the next save writes
		// the current one. One it does not know was written by a newer image: its generation
		// still counts ([`schema::held`]), its record is not guessed at.
		let version = u16::from_le_bytes(header[4..6].try_into().unwrap());
		let len = usize::from(u16::from_le_bytes(header[6..8].try_into().unwrap()));
		if len == 0 || len > SLOT_SIZE as usize - HEADER_LEN {
			return Ok(None);
		}
		let generation = u32::from_le_bytes(header[8..12].try_into().unwrap());
		let expected = u32::from_le_bytes(header[12..16].try_into().unwrap());

		let mut payload = alloc::vec![0u8; len];
		self.flash.read(base + HEADER_LEN as u32, &mut payload).map_err(|_| Error::Flash)?;
		if CRC.checksum(&payload) != expected {
			return Ok(None);
		}
		Ok(Some(Slot {
			generation,
			version,
			config: schema::decode(version, &payload).ok().flatten(),
		}))
	}
}
