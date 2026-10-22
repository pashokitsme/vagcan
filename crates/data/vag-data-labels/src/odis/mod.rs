//! A read-only reader for a VW ODIS-Service runtime project.
//!
//! An extracted ODIS project is a directory of `<PoolID>.db` / `<PoolID>.key`
//! pairs plus two plaintext string pools. Nothing in it is encrypted
//! (`.archive/research/labels/odis-crib.md` §2); the three layers are a B+Tree index
//! ([`keyfile`], Peter Graf's PBL), zlib members ([`pool`]), and a positional
//! object stream ([`object`]) whose field order per type was reverse-engineered
//! by `ODIS-project-explorer` against a decompiled MCD kernel.
//!
//! ## Read-only, in two senses
//! Nothing here writes to a project — there is no insert, delete or split path
//! in the B+Tree at all. And nothing here parses an object type whose only
//! purpose is a write service: flashing, access keys, adaptation and coding
//! cases are refused by name in [`loaders::refused_type_name`], permanently.
//! See [`loaders`] for the list, and why it is spelled out in two places.

pub mod compu;
pub mod hash;
pub mod keyfile;
pub mod loaders;
pub mod object;
pub mod pool;
pub mod strings;

/// Everything that can go wrong reading a project.
///
/// Hand-rolled in the style of `vag_data_db::Error` — `vag-data-labels` has no `anyhow` and
/// gains none here. Every variant carries enough to name the file or the field
/// that failed, because "the project is broken" is not a usable message when a
/// project is 472 files.
#[derive(Debug)]
pub enum Error {
	/// A file could not be read.
	Io(std::io::Error),
	/// A file was read but does not hold what its format promises: a truncated
	/// buffer, a length that overruns, a missing terminator, an enum value that
	/// is not one of the defined ones.
	Format(String),
	/// Something the project should contain was not there: a pool, a named
	/// object, a reference's target.
	Missing(String),
	/// The object contains a type on [`loaders::REFUSED`], the permanent
	/// never-parsed list, so it was not parsed at all.
	///
	/// Kept apart from [`Error::Format`] because it says the opposite thing: a
	/// `Format` error means the file is wrong, this means the file is fine and
	/// the tool declines. A caller that wants the rest of a project should skip
	/// what raised this and carry on — see the note on the `CASE` family in
	/// [`loaders`].
	Refused(&'static str),
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Error::Io(e) => write!(f, "io error: {e}"),
			Error::Format(m) => write!(f, "malformed ODIS project: {m}"),
			Error::Missing(m) => write!(f, "not in this ODIS project: {m}"),
			Error::Refused(t) => write!(f, "contains {t}, which this tool never parses"),
		}
	}
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
	fn from(e: std::io::Error) -> Self {
		Error::Io(e)
	}
}

/// An extracted ODIS-Service runtime project.
///
/// A directory of `0.0.0@<name>.<kind>.db` / `.key` pairs — six kinds, of
/// which `.bv` (base variants) and `.sd` (shared service data) carry
/// everything this reader wants — plus the two string pools.
///
/// Opening one reads everything: the two string pools — 88 MB inflated, and
/// every name in the project resolves through them — and every object pool,
/// each with an index over its `.key` tree ([`PoolFiles`]). All of it is
/// **kept**: the questions asked of a project ask the same handful of pools
/// thousands of times over, and re-opening a pool per question was most of
/// what made `setup` slow. The reference project is 127 MB on disk and
/// opens in well under a second on ten cores, so there is nothing to save by
/// being lazy about it.
#[derive(Debug)]
pub struct Project {
	dir: std::path::PathBuf,
	id: String,
	version: Option<String>,
	language: Option<String>,
	strings: strings::Strings,
	/// Every PoolID that has both a `.db` and a `.key`, sorted.
	pools: Vec<String>,
	/// The opened pools, one slot per entry of `pools`, filled on first use.
	///
	/// `None` inside a filled slot means the pair went missing between
	/// [`Project::open`] and the first read. A `OnceLock` per pool rather than
	/// one lock over a map so that readers on different threads never wait on
	/// each other's pool, and a pool once opened is never opened again.
	opened: Vec<std::sync::OnceLock<Option<PoolFiles>>>,
}

/// One ECU variant an ODIS project describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
	/// The ObjectID, e.g. `EV_ECM18TFS0208V0906264H`.
	pub name: String,
	/// Which pool it came from.
	pub pool: String,
	/// The base variant it derives from; `None` when this *is* a base variant.
	pub base_variant: Option<String>,
}

/// What an ODIS project calls one control unit, and the number it gives it.
///
/// One row per variant and logical link: a variant is reached through its base variant, and a
/// logical link (`LL_DashBoardUDS`, in the `.vi` pool) names the base variant it reaches. The
/// link's long name is the unit's name, and VW's table of logical links by diagnostic address
/// ([`loaders::identity::LOGIC_LINK_NAMES`]) is its number. Nothing here is asked of a car:
/// which variant a unit is comes from its own `F19E`/`F1A2`, matched by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitName {
	/// An ECU variant (`EV_DashBoardVDDMQBAB_009`), or a base variant (`BV_DashBoardUDS`).
	pub variant: String,
	/// The base variant it derives from — itself, for a base variant.
	pub base_variant: String,
	/// The logical link that reaches the base variant.
	pub link: String,
	/// The link's long name, `Dash Board UDS`; `None` where the link has none.
	pub name: Option<String>,
	/// The diagnostic address the table of logical links gives the link (`0x17`); `None`
	/// where no table names the link, or the tables give it more than one.
	pub address: Option<u32>,
}

/// The pool kind that holds a project's logical links: `0.0.0@VI_SK37X.vi`.
const VEHICLE_INFO_POOL: &str = ".vi";

/// The word a link's long name carries for the protocol it speaks, by the protocol layer's
/// short name: VW's converter names a UDS-on-CAN link `Dash Board UDS`. What a tester calls
/// the unit is the name before it. A link on any other protocol keeps its name as written.
const PROTOCOL_IN_NAME: &[(&str, &str)] = &[("PR_UDSOnCAN", " UDS")];

/// A link's long name without the word its protocol adds ([`PROTOCOL_IN_NAME`]).
fn unit_name_of(long_name: &str, protocol: Option<&str>) -> String {
	let added = PROTOCOL_IN_NAME.iter().find(|(layer, _)| Some(*layer) == protocol).map(|(_, word)| *word);
	match added.and_then(|word| long_name.strip_suffix(word)).map(str::trim_end) {
		Some(name) if !name.is_empty() => name.to_string(),
		_ => long_name.to_string(),
	}
}

/// What [`Project::unit_names`] found, and how many logical links it could not read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NamedUnits {
	pub units: Vec<UnitName>,
	/// Links — or objects of a `.vi` pool — that did not inflate, open or parse: skipped,
	/// each costing its own unit's name and nothing else.
	pub skipped: usize,
}

/// One readable channel of one variant, in this project's terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
	/// The UDS identifier `0x22` would ask for.
	pub did: u16,
	/// What the channel is called, in the project's language (German, for the
	/// reference project — see `odis-crib.md` §6; nothing here translates).
	pub name: String,
	/// The engineering unit as a tester would print it, when the file gives one.
	pub unit: Option<String>,
	/// Bits into the positive response, counted after the three-byte
	/// `62 <DID hi> <DID lo>` header.
	pub bit_offset: u32,
	/// How many bits the value occupies.
	pub bit_length: u32,
	/// Whether those bits are a signed quantity.
	pub signed: bool,
	/// Whether the bytes run most-significant first.
	///
	/// Not decoration, and not safe to assume. UDS payloads are big-endian by
	/// convention, and the reference car's own proven row is not: DID `0x380A`
	/// on the gearbox is `u16` **little-endian**
	/// (`.archive/research/labels/rod-labels.md:433`, established byte by byte against a
	/// log), and the ODIS file says the same — `is_high_low_byte_order` is
	/// false for it. A decoder that assumed big-endian would read 690 /min as
	/// 45570.
	pub big_endian: bool,
	/// How the raw value becomes an engineering one.
	pub scaling: crate::catalog::Scaling,
	/// The text id of [`Reading::name`] — the join to `TTTEXT`
	/// (`.archive/research/labels/odis-crib.md` §3).
	pub text_id: Option<String>,
}

/// One fault code of one variant, as the project describes it.
///
/// The row `vagcan faults` names a code from. Everything here is what the
/// `MCD_DB_DIAG_TROUBLE_CODE` object carries, recorded rather than
/// interpreted — see [`loaders::dtc`] for the layout and
/// `research/odis-dtc/README.md` for how it was established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
	/// The short name of the fault-code table this row came from,
	/// `DTCDOP_VAGUDS` — a variant can carry more than one.
	pub dop: String,
	/// The 24-bit number the control unit sends in a `0x19` response, read
	/// big-endian as one integer. The join key from the car to this row.
	pub code: u32,
	/// The code a tester prints, `P150B00` — a string the file carries, not
	/// an encoding of [`Fault::code`].
	pub display_code: Option<String>,
	/// The text, in whichever language its supplier wrote it. One per code;
	/// the format has no per-language table.
	pub text: Option<String>,
	/// The text's own identifier — usually equal to the display code.
	pub text_id: Option<String>,
	/// `DTC_<number>`.
	pub short_name: Option<String>,
	/// The ODX `LEVEL`, 1 to 9 on the reference project. Not interpreted.
	pub level: u32,
	/// `IS-TEMPORARY`, false on every code of the reference project.
	pub temporary: bool,
}

/// One record a variant's fault records can hold — what its extended-data multiplexer says
/// of one case (`research/odis-dtc/freeze-frames.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentRecord {
	/// The record numbers this case covers, both ends included; `None` for the default
	/// case, which covers every number no other case does.
	pub range: Option<(i32, i32)>,
	/// The case's long name (`Standard Freeze Frame#1`).
	pub name: Option<String>,
	/// The record's length in bytes after its number, where the file proves one: the
	/// structure's declared size, else the end of its last field. `None` for a record whose
	/// length is not fixed (the dynamic and unknown records run to the end of the answer)
	/// or not established.
	pub length: Option<u32>,
	/// Its fields, with bit offsets counted from the first byte after the record number.
	pub fields: Vec<EnvironmentField>,
}

/// One field of an [`EnvironmentRecord`].
#[derive(Debug, Clone, PartialEq)]
pub struct EnvironmentField {
	pub name: Option<String>,
	/// The text id of [`EnvironmentField::name`].
	pub text_id: Option<String>,
	pub unit: Option<String>,
	/// Bits from the first byte after the record number. `None` where the field sits in
	/// the record number's own byte or before it, which no field of the reference project does.
	pub bit_offset: Option<u32>,
	/// `None` where the width is not a fixed number of bits: a field of a type this reader
	/// does not load (a multiplexed value), or one that runs to the end of the answer.
	pub bit_length: Option<u32>,
	pub signed: bool,
	pub big_endian: bool,
	/// `None` where the conversion is not one this crate can represent honestly: the raw
	/// bytes are then the value.
	pub scaling: Option<crate::catalog::Scaling>,
	/// A coded constant: a field with no data object property of its own, a placeholder
	/// the unit fills with a fixed value. Kept for its position, not shown.
	pub constant: bool,
}

/// One vehicle a project declares it covers.
///
/// Read from `PRNR-INFO.xml` — see [`Project::vehicles`] for why that file and
/// not the `.vi` pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vehicle {
	/// `PRODUCT-ID` — VW's three-character type code, `5E0`, `5EP`, `55A`.
	///
	/// The key, and never absent: an entry without one names a vehicle nothing
	/// can select on, so it is refused rather than carried as a `None` for a
	/// caller to puzzle over.
	pub type_code: String,
	/// `NAME` — what a person calls it: `A7 / Octavia III (Limo, Combi)`.
	pub name: String,
	/// `VEHICLE-PROJECT` — `SK37X/0EU_X`. The string VW's own `S42`
	/// (*Fahrzeugprojektzuordnung*) keys on, kept because it is what a reader
	/// of that document recognises.
	pub vehicle_project: String,
	/// Whether the project marks this as a default vehicle.
	///
	/// **Not a tie-breaker.** All six of the reference project's vehicles carry
	/// `IS-DEFAULT="true"`, so it selects nothing; it is carried because it is
	/// in the file, not because it is useful for choosing.
	pub is_default: bool,
}

impl Project {
	/// Every vehicle this project declares it covers.
	///
	/// ## Why `PRNR-INFO.xml` and not the `.vi` pool
	/// The `.vi` pool looks like the obvious source and is not. Its
	/// `MCD_DB_VEHICLE_INFORMATION` describes **how to talk to the car** — one
	/// object per project, named after the project (`VINFO_SK37XCAN`, long name
	/// "SK37X CAN"), holding the logical links, the physical CAN link and the
	/// diagnostic connector's pins. There is no vehicle list in it, and none in
	/// the pools at all: the reference project's 1,155,437 ASCII and 153,704
	/// Unicode strings contain no `Karoq`, no `Kodiaq`, no `Octavia`, no
	/// `Fahrzeugprojekt` and no `5EP`. That is a measured negative, not an
	/// assumption from the name (`.archive/research/labels/odis-format.md`).
	///
	/// `PRNR-INFO.xml` carries it instead, and carries it in exactly the shape
	/// `S42` is written in: `VEHICLE-PROJECT`, `NAME`, `PRODUCT-ID`. The file is
	/// one of the entries declared in `rt_index.xml`'s `RUNTIME` block, so it is
	/// part of every extracted project by construction rather than by luck of
	/// this copy — which is what makes reading it safe to depend on.
	///
	/// A project without the file is an [`Error::Missing`], not an empty list. A
	/// project that covers nothing and a project that cannot say must not read
	/// alike; that is the same rule [`Project::readings`] follows.
	pub fn vehicles(&self) -> Result<Vec<Vehicle>, Error> {
		let path = self.dir.join(COVERAGE_FILE);
		let text = std::fs::read_to_string(&path)
			.map_err(|_| Error::Missing(format!("{} — the file that says which vehicles this project covers", path.display())))?;
		parse_coverage(&text)
	}

	/// The vehicle this project covers for a type code, if any.
	///
	/// Case-insensitive: whether a caller has the code upper- or lower-cased is
	/// not a distinction this format makes.
	///
	/// **A type code is an argument, not something this crate derives.** The
	/// obvious-looking rule — the leading three characters of an `F187` part
	/// number — does not hold: on the reference car three units report `5E0`
	/// (this project's default vehicle) but the engine reports `8V0`, an Audi
	/// platform number sitting in a Škoda, and `5Q0`/`3Q0` are MQB-common across
	/// the whole group. A part number is evidence about a *component*. Choosing
	/// which of fifteen answers to believe is a policy over live data and lives
	/// with the car's other answers, not in a file-format reader.
	///
	/// One caution for whoever writes that policy: it is **not established that
	/// `PRODUCT-ID` sets are disjoint between projects.** Selection works on the
	/// reference car because `5E0` is present and specific, and that is a sample
	/// of one project. A second project settles it.
	pub fn covers(&self, type_code: &str) -> Result<Option<Vehicle>, Error> {
		Ok(self.vehicles()?.into_iter().find(|v| v.type_code.eq_ignore_ascii_case(type_code)))
	}

	/// Open a project directory.
	///
	/// Every `*.key` with a matching `*.db` is a pool, whatever its kind: the
	/// base variants alone are 54 files in the reference project, and a reader
	/// that looked only at `.sd` would find no variants at all.
	pub fn open(dir: &std::path::Path) -> Result<Project, Error> {
		// The string pools and the object pools are independent files, so
		// they are read side by side: the strings are two gzip members
		// inflated and hashed, the pools are 230 files read and indexed, and
		// neither needs the other to be ready.
		let (strings, pools) = rayon::join(
			|| strings::Strings::open(dir),
			|| {
				use rayon::prelude::*;
				let pools = Self::list_pools(dir)?;
				let opened = pools
					.par_iter()
					.map(|pool_id| open_pool(dir, pool_id).map(std::sync::OnceLock::from))
					.collect::<Result<Vec<_>, Error>>()?;
				Ok::<_, Error>((pools, opened))
			},
		);
		let (strings, (pools, opened)) = (strings?, pools?);
		Ok(Project {
			id: project_id(dir),
			version: project_version(dir),
			language: project_language(dir),
			dir: dir.to_owned(),
			strings,
			pools,
			opened,
		})
	}

	/// Every PoolID under `dir` that has both a `.db` and a `.key`, sorted.
	fn list_pools(dir: &std::path::Path) -> Result<Vec<String>, Error> {
		let mut pools = Vec::new();
		for entry in std::fs::read_dir(dir).map_err(Error::Io)? {
			let path = entry.map_err(Error::Io)?.path();
			let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
				continue;
			};
			let Some(id) = name.strip_suffix(".key") else { continue };
			if dir.join(format!("{id}.db")).is_file() {
				pools.push(id.to_owned());
			}
		}
		if pools.is_empty() {
			return Err(Error::Missing(format!("{} holds no .db/.key pool pair", dir.display())));
		}
		pools.sort_unstable();
		Ok(pools)
	}

	/// The project's own name — the identifier VW's tooling uses (design §4.1).
	///
	/// Taken from `index.xml`'s `<SHORT-NAME>` where there is one, and from the
	/// directory name otherwise. The file wins because a directory gets renamed
	/// by an unzip — `SK37X (1)` — and `<SHORT-NAME>` does not.
	pub fn id(&self) -> &str {
		&self.id
	}

	/// The converter's project version, from `DatabaseVersionInfo.txt`, for a
	/// provenance log (design §4.4). `None` when the file is absent.
	pub fn version(&self) -> Option<&str> {
		self.version.as_deref()
	}

	/// The language the project declares for itself — `<LANGUAGE>deu</LANGUAGE>`
	/// in `index.xml`'s `ADMIN-DATA`, an ISO 639-2 code. `None` when the file
	/// does not say.
	///
	/// **Declared once, for the whole project, and it is a declaration.** The
	/// object model has no language field anywhere: a fault code carries one
	/// text and a measurement one name, in whichever language the supplier
	/// wrote it. On the reference project, which declares `deu`, the engine's
	/// 202,863 fault texts are English and the body electronics' are German
	/// (`research/odis-dtc/README.md`). So this is the language of the
	/// *source*, recorded so that a second project in another language can be
	/// told apart from it — not a promise about every text inside.
	pub fn language(&self) -> Option<&str> {
		self.language.as_deref()
	}

	/// Every PoolID this project holds, sorted.
	pub fn pools(&self) -> &[String] {
		&self.pools
	}

	/// Every ECU variant, across every pool.
	///
	/// Read out of each base-variant pool's `DB_PROJECT_DATA`, which names the
	/// base variant and every variant derived from it — one object per pool, no
	/// scanning. A pool without one is skipped rather than fatal: five of the
	/// six pool kinds do not have variants, and a project is not broken for
	/// that.
	pub fn variants(&self) -> Result<Vec<Variant>, Error> {
		let mut out = Vec::new();
		for pool_id in &self.pools {
			let Some(files) = self.pool(pool_id)? else { continue };
			let Some(bytes) = files.object(&self.strings, loaders::identity::PROJECT_DATA_ID)? else {
				continue;
			};
			let (type_code, mut stream) = object::Stream::open(&bytes, &self.strings)?;
			if type_code != loaders::code::DB_PROJECT_DATA {
				continue;
			}
			// A pool whose variant list does not parse is named, not swallowed:
			// the list is the answer, so there is nothing to carry on with.
			let data = loaders::identity::project_data(&mut stream)
				.and_then(|data| stream.end().map(|()| data))
				.map_err(|e| match e {
					Error::Format(m) => Error::Format(format!("{pool_id}: {m}")),
					other => other,
				})?;
			let base = data.base_variant.object.clone();
			if let Some(name) = base.clone() {
				out.push(Variant {
					name,
					pool: pool_id.clone(),
					base_variant: None,
				});
			}
			for (name, target) in data.ecu_variants {
				let Some(name) = name.or_else(|| target.object.clone()) else { continue };
				out.push(Variant {
					name,
					pool: target.pool.unwrap_or_else(|| pool_id.clone()),
					base_variant: base.clone(),
				});
			}
		}
		Ok(out)
	}

	/// What this project calls each control unit, and the diagnostic address it gives it:
	/// one [`UnitName`] per variant and the logical link that reaches its base variant.
	///
	/// Three things the project holds, joined by name and nothing else:
	///
	/// 1. the variants and the base variant each derives from ([`Project::variants`]);
	/// 2. the logical links of the `.vi` pools, each naming the base variant it reaches and
	///    carrying the long name a tester shows for the unit;
	/// 3. VW's table of logical links by diagnostic address
	///    ([`loaders::identity::LOGIC_LINK_NAMES`]) — a `TEXTTABLE` from the address to the
	///    link's short name, in the standard UDS libraries.
	///
	/// A variant whose base variant no link reaches has no row: the project does not name
	/// it. A link no table lists, or one the tables list under two addresses, has a name and
	/// no address. A table or a link that does not read costs its own rows, not the others':
	/// it is skipped and counted ([`NamedUnits::skipped`]). Only the variant list failing
	/// is an error — without it nothing can be named.
	///
	/// The name is the link's long name less the word its protocol adds
	/// ([`PROTOCOL_IN_NAME`]): `Dash Board`, not `Dash Board UDS`.
	pub fn unit_names(&self) -> Result<NamedUnits, Error> {
		let (links, skipped) = self.logical_links();
		let addresses = self.link_addresses();
		let mut out = Vec::new();
		for variant in self.variants()? {
			let base = variant.base_variant.clone().unwrap_or_else(|| variant.name.clone());
			for link in links.iter().filter(|l| l.base_variant.as_deref() == Some(base.as_str())) {
				let Some(short) = link.short_name.clone() else { continue };
				let address = match addresses.get(&short) {
					Some(found) if found.len() == 1 => found.first().copied(),
					_ => None,
				};
				out.push(UnitName {
					variant: variant.name.clone(),
					base_variant: base.clone(),
					link: short,
					name: link.long_name.as_deref().map(|name| unit_name_of(name, link.protocol.as_deref())),
					address,
				});
			}
		}
		Ok(NamedUnits { units: out, skipped })
	}

	/// Every logical link of the project's `.vi` pools, its head read
	/// ([`loaders::identity::logical_link`]), and how many objects there did not read. A
	/// pool of vehicle information is a handful of objects, so each is opened and kept when
	/// it is a link; one that will not inflate, open or parse is counted and skipped.
	fn logical_links(&self) -> (Vec<loaders::identity::LogicalLink>, usize) {
		let mut out = Vec::new();
		let mut skipped = 0;
		for pool_id in self.pools.iter().filter(|id| id.ends_with(VEHICLE_INFO_POOL)) {
			let Ok(Some(files)) = self.pool(pool_id) else {
				skipped += 1;
				continue;
			};
			for locator in files.index.values() {
				let link = files.db.member(locator).and_then(|bytes| {
					let (type_code, mut stream) = object::Stream::open(&bytes, &self.strings)?;
					match type_code == loaders::code::MCD_DB_LOGICAL_LINK {
						true => loaders::identity::logical_link(&mut stream).map(Some),
						false => Ok(None),
					}
				});
				match link {
					Ok(Some(link)) => out.push(link),
					Ok(None) => {}
					Err(_) => skipped += 1,
				}
			}
		}
		// The index is a hash map: the order a project's links come in is made stable here.
		out.sort_by(|a, b| a.short_name.cmp(&b.short_name));
		(out, skipped)
	}

	/// Logical link → every diagnostic address the project's tables of logical links give
	/// it ([`loaders::identity::LOGIC_LINK_NAMES`]).
	///
	/// The tables are found by name: an object whose ObjectID holds the table's short name is
	/// opened, and kept when it is a data object property of exactly that short name — the
	/// ObjectID only narrows the search (`DOP_<library>_DOP_TEXTTABLELogicLinkNames`), the
	/// short name decides. Only one-value levels count: a level is one address.
	fn link_addresses(&self) -> std::collections::BTreeMap<String, std::collections::BTreeSet<u32>> {
		use loaders::identity::LOGIC_LINK_NAMES;
		let mut out: std::collections::BTreeMap<String, std::collections::BTreeSet<u32>> = Default::default();
		for pool_id in &self.pools {
			let Ok(Some(files)) = self.pool(pool_id) else { continue };
			for (hash, locator) in &files.index {
				if !self.strings.ascii.get(*hash).is_some_and(|name| name.contains(LOGIC_LINK_NAMES)) {
					continue;
				}
				let Ok(bytes) = files.db.member(locator) else { continue };
				let Ok((type_code, mut stream)) = object::Stream::open(&bytes, &self.strings) else {
					continue;
				};
				let Ok(loaders::Outcome::Object(loaders::Object::Dop(dop))) = loaders::load(type_code, &mut stream) else {
					continue;
				};
				if dop.short_name.as_deref() != Some(LOGIC_LINK_NAMES) {
					continue;
				}
				let Some(Ok(crate::catalog::Scaling::Enum { levels })) = dop.compu.as_ref().map(|c| c.scaling()) else {
					continue;
				};
				for level in levels.iter().filter(|l| l.lower() == l.upper()) {
					if let Ok(address) = u32::try_from(level.lower()) {
						out.entry(level.name().to_string()).or_default().insert(address);
					}
				}
			}
		}
		out
	}

	/// The readable channels of one variant.
	///
	/// Walks the chain in [`loaders::measurement`]'s module note: the variant's
	/// layer data names its `ReadDataByIdentifier` service, whose positive
	/// response carries a table key (the DIDs) and a table struct (the
	/// measurements).
	///
	/// **An empty list means one thing: the variant declares no measurement
	/// service.** That is a fact about the control unit — 44 of the reference
	/// project's 717 variants are like that — and not a fact about this reader.
	/// Every way of failing to *find* the chain is an error instead, because an
	/// empty success is the failure that hides: nothing in a run says anything
	/// is wrong, and a broken lookup and a unit with no measurements read
	/// identically to a caller.
	///
	/// A channel whose scaling this crate cannot represent honestly, or which
	/// reaches a refused type, is **skipped**. One unreadable channel must not
	/// cost a control unit all its others.
	pub fn readings(&self, variant: &Variant) -> Result<Vec<Reading>, Error> {
		self.read_through(variant, loaders::identity::RDBI_MEASUREMENT)
	}

	/// The adaptation channels of one variant: what its
	/// [`loaders::identity::RDBI_ADAPTATION`] service reads, by DID, with each
	/// field's layout and scaling.
	///
	/// The same chain as [`Project::readings`], started at the other service: its
	/// positive response is the measurement response's shape, `62 <DID> <data>`, a
	/// table key and a table struct. Only the read service is looked up; nothing
	/// here can describe a write. An empty list means the variant, and every
	/// parent it names, declares no adaptation read.
	pub fn adaptations(&self, variant: &Variant) -> Result<Vec<Reading>, Error> {
		self.read_through(variant, loaders::identity::RDBI_ADAPTATION)
	}

	/// The channels one `ReadDataByIdentifier` service of a variant reads.
	fn read_through(&self, variant: &Variant, service: &str) -> Result<Vec<Reading>, Error> {
		let mut store = Store::new(self);
		let Some(own) = store.layer_data(variant)? else {
			return Ok(Vec::new());
		};
		// The service may belong to the variant's *base* variant rather than to
		// the variant — see [`Store::measurement_layer`]. Everything below then
		// runs against that layer and its pool, because that is where the
		// service's own objects are indexed.
		let Some((layer, home)) = store.measurement_layer(variant, own, service)? else {
			return Ok(Vec::new());
		};
		let Some(service_ref) = layer
			.services
			.iter()
			.find(|(name, _)| name.as_deref() == Some(service))
			.map(|(_, target)| target.clone())
		else {
			return Ok(Vec::new());
		};

		let Some(loaders::Object::Service(service)) = store.object(&layer, &home, &service_ref)? else {
			return Err(Error::Format(format!("{}'s measurement service is not a service", variant.name)));
		};
		let Some(response_ref) = service.positive_responses.first().cloned() else {
			return Ok(Vec::new());
		};
		let Some(loaders::Object::Response(response)) = store.object(&layer, &home, &response_ref)? else {
			return Err(Error::Format(format!("{}'s measurement service has no positive response", variant.name)));
		};

		// A UDS 0x22 positive response is `62 <DID hi> <DID lo>` then data, so
		// the identifier sits at byte 1 and the payload at byte 3. That is the
		// protocol, not this car — and the file is checked against it rather
		// than assumed to agree.
		const DID_BYTE: u32 = 1;
		const PAYLOAD_BYTE: u32 = 3;
		let key = response
			.parameters
			.iter()
			.find(|p| p.type_code == loaders::code::MCD_DB_PARAMETER_TABLE_KEY);
		let payload = response
			.parameters
			.iter()
			.find(|p| p.type_code == loaders::code::MCD_DB_PARAMETER_TABLESTRUCT);
		let (Some(key), Some(payload)) = (key, payload) else {
			return Ok(Vec::new());
		};
		if key.byte_position != Some(DID_BYTE) || payload.byte_position != Some(PAYLOAD_BYTE) {
			return Err(Error::Format(format!(
				"{}'s measurement response puts its identifier at byte {:?} and its payload at byte {:?}, not {DID_BYTE} and {PAYLOAD_BYTE}",
				variant.name, key.byte_position, payload.byte_position
			)));
		}

		let dids = store.identifiers(&layer, &home, key)?;
		let rows = store.rows(&layer, &home, payload)?;

		let mut out = Vec::new();
		for (did, name, text_id) in dids {
			let Some(row_ref) = row_for(&rows, &name) else { continue };
			// Anything below here can legitimately fail for one channel — a
			// multiplexed shape, a compu category with no honest scaling — and
			// one channel must not cost the rest.
			let Ok(channel) = store.channel(&layer, &home, &row_ref, did, &name, text_id) else {
				continue;
			};
			out.extend(channel);
		}
		Ok(out)
	}

	/// The fault codes one variant can report, with their texts.
	///
	/// The chain is the measurement chain's shape with two hops fewer: the
	/// variant's layer data names its fault-code data object properties by
	/// short name (`DTCDOP_VAGUDS`), its property index turns each name into
	/// the `DB_DOP_DTC` object, and that object maps every number to its
	/// `MCD_DB_DIAG_TROUBLE_CODE`.
	///
	/// **The layer that declares the fault memory may be a parent.** Same rule
	/// as [`Store::measurement_layer`]: a variant that names no fault-code
	/// property of its own is read through the first parent layer that does,
	/// and a variant whose own layer names one is read from that layer alone.
	///
	/// An empty list means the variant, and every parent it names, declares no
	/// fault-code property — a fact about the unit. A code whose object cannot
	/// be read is skipped rather than fatal, as one channel is in
	/// [`Project::readings`]; a refused type stops the variant, as it does there.
	pub fn faults(&self, variant: &Variant) -> Result<Vec<Fault>, Error> {
		let mut store = Store::new(self);
		let Some(own) = store.layer_data(variant)? else {
			return Ok(Vec::new());
		};
		let Some((layer, home)) = store.fault_layer(variant, own)? else {
			return Ok(Vec::new());
		};

		let mut out = Vec::new();
		for name in &layer.dtc_properties {
			let Some((_, indexed)) = layer.properties.iter().find(|(key, _)| key.as_deref() == Some(name.as_str())) else {
				continue;
			};
			let Some(object_id) = indexed.object.clone() else { continue };
			let pool = indexed.pool.clone().unwrap_or_else(|| home.clone());
			// Absent and wrong are different failures and read differently: a
			// pool that does not carry the object is a project missing a
			// piece, an object of another type is a project this reader
			// misunderstood. Reporting both as "is not a DB_DOP_DTC" sent
			// anyone chasing the second when it was the first.
			let dop = match store.named(&pool, &object_id)? {
				Some(loaders::Object::DtcDop(dop)) => dop,
				Some(_) => {
					return Err(Error::Format(format!(
						"{}'s fault-code property {object_id} is in {pool} and is not a DB_DOP_DTC",
						variant.name
					)));
				}
				None => {
					return Err(Error::Missing(format!(
						"{}'s fault-code property {object_id} is not in {pool}",
						variant.name
					)));
				}
			};
			let dop_name = dop.short_name.clone().unwrap_or_else(|| name.clone());
			for (number, target) in &dop.codes {
				let code = match store.object(&layer, &pool, target) {
					Ok(Some(loaders::Object::TroubleCode(code))) => code,
					Err(e @ Error::Refused(_)) => return Err(e),
					_ => continue,
				};
				out.push(Fault {
					dop: dop_name.clone(),
					// The map's key is what the table is looked up by, so it is
					// the row's key too. The object carries the number as well,
					// and the two agree on every one of the reference project's
					// 291,346 codes — checked, not assumed, and not re-checked
					// here because a disagreement would have no right answer.
					code: *number,
					display_code: code.display_code,
					text: code.text,
					text_id: code.text_id,
					short_name: code.short_name,
					level: code.level,
					temporary: code.temporary,
				});
			}
		}
		Ok(out)
	}

	/// What a variant's fault records hold: every case of its extended-data multiplexer,
	/// the standard records `0x01`/`0x02` among them, with each field's layout and scaling.
	///
	/// The multiplexer is found by its short name
	/// ([`loaders::extended::EXTENDED_DATA_MULTIPLEXER`]) in the variant's own layer, then in
	/// each parent's in order — the base variant, the shared libraries, the protocol — the
	/// way ODX inheritance resolves a short name. An empty list means no layer names one.
	///
	/// A field this reader cannot lay out (a multiplexed measurement value, a type it has
	/// no loader for) is kept with no width rather than dropping the record: its bytes are
	/// still the record's, and the record's length may still be proven by what follows.
	pub fn extended_records(&self, variant: &Variant) -> Result<Vec<EnvironmentRecord>, Error> {
		Ok(self.extended_data(variant)?.map(|(_, records)| records).unwrap_or_default())
	}

	/// [`Self::extended_records`], with the ObjectID of the multiplexer they came from —
	/// `None` where no layer names one.
	pub fn extended_data(&self, variant: &Variant) -> Result<Option<(String, Vec<EnvironmentRecord>)>, Error> {
		let mut store = Store::new(self);
		let Some(own) = store.layer_data(variant)? else {
			return Ok(None);
		};
		let Some(found) = store.extended_multiplexer(&own)? else {
			return Ok(None);
		};
		let (mux_id, mux_pool) = found;
		let strings = &self.strings;
		let Some(bytes) = self.pool(&mux_pool)?.map(|files| files.object(strings, &mux_id)).transpose()?.flatten() else {
			return Err(Error::Missing(format!("{}'s {mux_id} is not in {mux_pool}", variant.name)));
		};
		let (type_code, mut stream) = object::Stream::open(&bytes, strings)?;
		if type_code != loaders::code::MCD_DB_PARAMETER_MULTIPLEXER {
			return Err(Error::Format(format!("{mux_id} is type {type_code:#06x}, not a multiplexer")));
		}
		let mux = loaders::extended::multiplexer(&mut stream)?;
		stream.end()?;

		// A case keyed by the key's text is turned into the number through the key's own
		// text table — never matched by position or guessed.
		// The key is the record number: the answer's one byte at the multiplexer's start. A
		// key anywhere else, or of another width, is a multiplexer this reading does not fit.
		let key = match &mux.key_dop {
			Some(target) => match store.object(&own, &variant.pool, target)? {
				Some(loaders::Object::Dop(dop)) => Some(dop),
				_ => None,
			},
			None => None,
		};
		let key_bits = key.as_ref().and_then(|dop| dop.coded.as_ref()).and_then(|c| c.bits);
		if key_bits != Some(8) {
			return Err(Error::Format(format!(
				"{mux_id}: its key is {key_bits:?} bits wide, not the record number's one byte"
			)));
		}
		let key_levels = match key.as_ref().and_then(|dop| dop.compu.as_ref()).map(|c| c.scaling()) {
			Some(Ok(crate::catalog::Scaling::Enum { levels })) => levels,
			_ => Vec::new(),
		};
		let range_of = |case: &loaders::extended::Case| match (&case.range, &case.text) {
			(Some(range), _) => Ok(Some(*range)),
			(None, Some(text)) => key_levels
				.iter()
				.find(|level| level.name() == text)
				.map(|level| Some((level.lower(), level.upper())))
				.ok_or_else(|| Error::Format(format!("{mux_id}: no record number in its key's table is named {text:?}"))),
			(None, None) => Ok(None),
		};

		let mut out = Vec::new();
		// Each record number has one case: two that start on the same number (or two default
		// cases) would be stored as one record, the fields of both under it.
		let ranges = mux.cases.iter().map(&range_of).collect::<Result<Vec<_>, _>>()?;
		if let Some(start) = repeated_start(&ranges) {
			return Err(Error::Format(format!("{mux_id}: two cases for record {start:?}")));
		}
		for (case, range) in mux.cases.iter().zip(ranges) {
			let mut fields = Vec::new();
			let mut declared = None;
			if let Some(target) = &case.structure {
				match store.object(&own, &variant.pool, target)? {
					Some(loaders::Object::Structure(structure)) => {
						declared = (structure.bytes > 0).then_some(u32::from(structure.bytes));
						store.environment_fields(&own, &variant.pool, &structure, 0, 0, &mut fields)?;
					}
					_ => {
						return Err(Error::Missing(format!(
							"{}'s case {:?} names no structure",
							variant.name, case.short_name
						)));
					}
				}
			}
			// Positions in the file count from the multiplexer's start, the record number's
			// byte, plus where the cases start; the decoder counts from the byte after the
			// number. On 169 of 171 multiplexers that is no shift at all.
			let shift = i64::from(mux.case_start) * 8 - 8;
			for field in &mut fields {
				field.bit_offset = field.bit_offset.and_then(|bits| u32::try_from(i64::from(bits) + shift).ok());
			}
			let length = match declared {
				Some(bytes) => (mux.case_start + bytes).checked_sub(1),
				None => known_extent(&fields),
			};
			out.push(EnvironmentRecord {
				range,
				name: case.long_name.clone().or_else(|| case.short_name.clone()),
				length,
				fields,
			});
		}
		Ok(Some((mux_id, out)))
	}

	/// Every human-readable name this project knows, keyed by its text id, for
	/// merging into `names.json`.
	///
	/// This is a **whole-project pass**: every member of every pool is inflated
	/// and parsed, and the types that carry a `(text id, text)` pair give it
	/// up. There is no cheaper way — the two string pools hold the ids and the
	/// texts but nothing that pairs them; only an object does. It is a
	/// setup-time cost, paid once per project, and on a project the size of the
	/// reference one it is minutes rather than seconds.
	///
	/// A member that does not parse is skipped, not fatal. The point of this
	/// pass is coverage, and one unreadable object should not cost the rest.
	pub fn names(&self) -> Result<std::collections::BTreeMap<String, String>, Error> {
		use rayon::prelude::*;
		// Every pool's records, then every pool's objects, each in parallel.
		// The harvest is split into chunks of records that each fill a map of
		// their own, and the maps are folded **in pool order, then record
		// order** with the same first-writer-wins rule — so the answer is the
		// one the sequential walk gave, only the work is spread out. This
		// pass was 8 s of one core; it is under 2 s of ten.
		const CHUNK: usize = 4096;
		let mut located: Vec<(&PoolFiles, Vec<pool::Locator>)> = Vec::new();
		for pool_id in &self.pools {
			let Some(files) = self.pool(pool_id)? else { continue };
			let mut locators = Vec::new();
			files.key.each_record(|_, data| {
				// A record that is not a locator is skipped, as the sequential
				// walk skipped it.
				if let Ok(locator) = pool::Locator::parse(data) {
					locators.push(locator);
				}
				Ok(())
			})?;
			located.push((files, locators));
		}
		let jobs: Vec<(&PoolFiles, &[pool::Locator])> = located
			.iter()
			.flat_map(|(files, locators)| locators.chunks(CHUNK).map(move |chunk| (*files, chunk)))
			.collect();
		let harvested: Vec<std::collections::BTreeMap<String, String>> = jobs
			.par_iter()
			.map(|(files, chunk)| {
				let mut found = std::collections::BTreeMap::new();
				for locator in *chunk {
					let Ok(bytes) = files.db.member(locator) else { continue };
					let Ok((type_code, mut stream)) = object::Stream::open(&bytes, &self.strings) else {
						continue;
					};
					if let Ok(loaders::Outcome::Object(object)) = loaders::load(type_code, &mut stream) {
						harvest(&object, &mut found);
					}
				}
				found
			})
			.collect();
		let mut out = std::collections::BTreeMap::new();
		for found in harvested {
			for (id, text) in found {
				out.entry(id).or_insert(text);
			}
		}
		Ok(out)
	}

	/// A pool, opened and indexed by [`Project::open`] and kept for the life
	/// of the project. `None` when the pair is not in this project, or went
	/// missing.
	fn pool(&self, pool_id: &str) -> Result<Option<&PoolFiles>, Error> {
		let Ok(at) = self.pools.binary_search_by(|p| p.as_str().cmp(pool_id)) else {
			return Ok(None);
		};
		let slot = &self.opened[at];
		if let Some(files) = slot.get() {
			return Ok(files.as_ref());
		}
		// `open` fills every slot, so this is the path for a slot it did not —
		// there is none today, and if one appears the pool is opened here, on
		// first use, rather than reported missing. Two threads arriving at
		// once both open it and the first to finish is kept.
		let opened = open_pool(&self.dir, pool_id)?;
		let _ = slot.set(opened);
		Ok(slot.get().and_then(Option::as_ref))
	}
}

/// Open a pool's `.db`/`.key` pair, or `None` if either is missing.
fn open_pool(dir: &std::path::Path, pool_id: &str) -> Result<Option<PoolFiles>, Error> {
	let key_path = dir.join(format!("{pool_id}.key"));
	let db_path = dir.join(format!("{pool_id}.db"));
	if !key_path.is_file() || !db_path.is_file() {
		return Ok(None);
	}
	Ok(Some(PoolFiles::open(&key_path, &db_path)?))
}

/// A pool's two files, opened together, plus an index over the key tree.
///
/// The index is what a lookup goes through. A `.key` tree answers a lookup
/// by descending and then scanning a leaf, expanding one prefix-compressed
/// item after another, and the measurement chain asks a few million times —
/// walking the leaf chain once and remembering where every object is turns
/// each of those into one hash probe.
#[derive(Debug)]
struct PoolFiles {
	key: keyfile::KeyFile,
	db: pool::Pool,
	/// Where each object's member is, keyed by the object's name hash — the
	/// four little-endian bytes every VW-written `.key` stores as its key.
	///
	/// **Only four-byte keys are indexed.** A lookup by object name is always
	/// by such a hash ([`PoolFiles::object`]), so a record under a key of any
	/// other width could never be found by name anyway; it is still reachable
	/// through [`keyfile::KeyFile::records`]. A record whose data is not a
	/// locator is left out for the same reason — [`PoolFiles::object`] would
	/// only refuse it later.
	///
	/// First record in key order wins on a duplicate key. VW's writer stores
	/// each object under its own name hash, probed on collision
	/// ([`strings::Pool::hash_of`]), so a duplicate is not a shape a project
	/// produces; the rule is written down so that the answer is at least
	/// deterministic if one ever does.
	index: std::collections::HashMap<u32, pool::Locator>,
}

impl PoolFiles {
	/// Read both files and build the index.
	fn open(key_path: &std::path::Path, db_path: &std::path::Path) -> Result<PoolFiles, Error> {
		let key = keyfile::KeyFile::open(key_path)?;
		let db = pool::Pool::open(db_path)?;
		let mut index = std::collections::HashMap::new();
		key.each_record(|key, data| {
			if let Ok(hash) = <[u8; 4]>::try_from(key)
				&& let Ok(locator) = pool::Locator::parse(data)
			{
				index.entry(u32::from_le_bytes(hash)).or_insert(locator);
			}
			Ok(())
		})?;
		Ok(PoolFiles { key, db, index })
	}

	/// The inflated bytes of one named object, or `None` if this pool has no
	/// such name.
	fn object(&self, strings: &strings::Strings, object_id: &str) -> Result<Option<Vec<u8>>, Error> {
		let Some(hash) = strings.ascii.hash_of(object_id) else { return Ok(None) };
		// A `.key` key is the hash's four bytes, little-endian — which is what
		// the index is keyed by.
		let Some(locator) = self.index.get(&hash) else {
			return Ok(None);
		};
		Ok(Some(self.db.member(locator)?))
	}
}

/// The reference resolution one question needs.
///
/// Held for the length of a single [`Project::readings`] call: the parent
/// layers it resolves through belong to that variant. The pools themselves
/// are the project's ([`Project::pool`]) — every variant of a base variant
/// walks the same few, and a store that opened its own copies paid for the
/// same megabytes 717 times over.
struct Store<'a> {
	project: &'a Project,
	/// Layer data of the parent pools, resolved on first need. A reference
	/// that omits its pool is looked up here.
	inherited: Vec<loaders::identity::LayerData>,
}

impl<'a> Store<'a> {
	fn new(project: &'a Project) -> Store<'a> {
		Store {
			project,
			inherited: Vec::new(),
		}
	}

	/// A pool, opened at most once per project.
	fn pool(&mut self, pool_id: &str) -> Result<Option<&'a PoolFiles>, Error> {
		self.project.pool(pool_id)
	}

	/// Load a named object from a named pool.
	fn named(&mut self, pool_id: &str, object_id: &str) -> Result<Option<loaders::Object>, Error> {
		let strings = &self.project.strings;
		let Some(files) = self.pool(pool_id)? else { return Ok(None) };
		let Some(bytes) = files.object(strings, object_id)? else {
			return Ok(None);
		};
		let (type_code, mut stream) = object::Stream::open(&bytes, strings)?;
		match loaders::load(type_code, &mut stream)? {
			loaders::Outcome::Object(object) => Ok(Some(object)),
			loaders::Outcome::Refused => Err(Error::Refused(loaders::refused_type_name(type_code).unwrap_or("a refused type"))),
			loaders::Outcome::Unsupported(code) => Err(Error::Format(format!(
				"{object_id} is type {code:#06x}, which this reader has no loader for"
			))),
		}
	}

	/// Load whatever a reference points at.
	///
	/// A reference that names no pool is resolved through the layer's own
	/// indexes, then through its parents' — that is what the indexes are for,
	/// and it is how inheritance works in this format.
	fn object(&mut self, layer: &loaders::identity::LayerData, home: &str, target: &loaders::Ref) -> Result<Option<loaders::Object>, Error> {
		let Some(object_id) = target.object.clone() else { return Ok(None) };
		if let Some(pool_id) = &target.pool {
			return self.named(pool_id, &object_id);
		}
		for (name, indexed) in layer.properties.iter().chain(&layer.tables) {
			if name.as_deref() == Some(object_id.as_str())
				&& let Some(pool_id) = indexed.pool.clone()
			{
				return self.named(&pool_id, &object_id);
			}
		}
		self.load_inherited(layer, home)?;
		// The pool is found first and the lookup made after, so the parents'
		// indexes are searched in place: copying them out per reference was a
		// clone of every property of every parent layer, per channel field.
		let inherited = self
			.inherited
			.iter()
			.flat_map(|l| l.properties.iter().chain(&l.tables))
			.find(|(name, indexed)| name.as_deref() == Some(object_id.as_str()) && indexed.pool.is_some())
			.and_then(|(_, indexed)| indexed.pool.clone());
		if let Some(pool_id) = inherited {
			return self.named(&pool_id, &object_id);
		}
		// Last resort: the same pool the referrer lives in.
		self.named(home, &object_id)
	}

	/// The layer that actually declares the measurement service, and the pool it
	/// lives in.
	///
	/// **An ECU variant need not declare any service of its own.** ODX layers
	/// inherit, and the converter uses that: `EV_DCUDriveSideEWMAXCONT_006` — the
	/// reference car's driver's-door unit — has a layer that parses completely,
	/// declares **zero** services, and names one parent,
	/// `0.0.0@BV_DoorElectDriveSideUDS.bv`, whose base-variant layer declares the
	/// measurement service and 118 channels. A reader that stops at the variant's
	/// own layer reports that control unit as having no measurements at all,
	/// which is what this project did until 2026-08-10 for both front doors.
	///
	/// The pool comes back with the layer because the service's own objects are
	/// indexed by *that* layer, in *that* pool; resolving them against the
	/// variant's pool finds nothing.
	///
	/// A parent that cannot be read is an error rather than a silent empty list.
	/// The variant has no service either way, so without the parent there is
	/// nothing to say, and saying nothing is how a broken parse became "this unit
	/// has no measurements".
	///
	/// `service` is the short name looked for: the measurement service, or the
	/// adaptation read, which inherit by the same rule.
	fn measurement_layer(
		&mut self,
		variant: &Variant,
		own: loaders::identity::LayerData,
		service: &str,
	) -> Result<Option<(loaders::identity::LayerData, String)>, Error> {
		let declares = |layer: &loaders::identity::LayerData| layer.services.iter().any(|(name, _)| name.as_deref() == Some(service));
		if declares(&own) {
			return Ok(Some((own, variant.pool.clone())));
		}
		for pool_id in own.parents.clone() {
			if let Some(loaders::Object::LayerData(parent)) = self.named(&pool_id, loaders::identity::LAYER_DATA_ID)?
				&& declares(&parent)
			{
				return Ok(Some((parent, pool_id)));
			}
		}
		Ok(None)
	}

	/// The extended-data multiplexer a variant resolves to: `(ObjectID, pool)` of the first
	/// layer — its own, then each parent nearest first — whose property index names
	/// [`loaders::extended::EXTENDED_DATA_MULTIPLEXER`].
	fn extended_multiplexer(&mut self, own: &loaders::identity::LayerData) -> Result<Option<(String, String)>, Error> {
		let wanted = loaders::extended::EXTENDED_DATA_MULTIPLEXER;
		let find = |layer: &loaders::identity::LayerData| {
			layer
				.properties
				.iter()
				.find(|(name, _)| name.as_deref() == Some(wanted))
				.and_then(|(_, target)| Some((target.object.clone()?, target.pool.clone()?)))
		};
		if let Some(found) = find(own) {
			return Ok(Some(found));
		}
		for pool_id in own.parents.clone() {
			if let Some(loaders::Object::LayerData(parent)) = self.named(&pool_id, loaders::identity::LAYER_DATA_ID)?
				&& let Some(found) = find(&parent)
			{
				return Ok(Some(found));
			}
		}
		Ok(None)
	}

	/// The fields of one case structure, flattened: a field whose data object property is a
	/// structure itself (the time stamp) is its fields, at its own byte position plus
	/// theirs. `base` is that position, in bytes from the multiplexer's start; `depth` how
	/// many structures deep this one is, refused past [`MAX_STRUCTURE_DEPTH`] — a structure
	/// that names itself would otherwise recurse until the stack ran out, inside `setup`'s walk.
	fn environment_fields(
		&mut self,
		layer: &loaders::identity::LayerData,
		home: &str,
		structure: &loaders::measurement::Structure,
		base: u32,
		depth: usize,
		out: &mut Vec<EnvironmentField>,
	) -> Result<(), Error> {
		if depth > MAX_STRUCTURE_DEPTH {
			return Err(Error::Format(format!(
				"structures nested more than {MAX_STRUCTURE_DEPTH} deep at {:?}: a structure that contains itself",
				structure.short_name
			)));
		}
		for field in &structure.fields {
			let byte = base.saturating_add(field.byte_position.unwrap_or(0));
			let bit_offset = Some(byte.saturating_mul(8).saturating_add(u32::from(field.bit_position)));
			let mut item = EnvironmentField {
				name: field.long_name.clone(),
				text_id: field.long_name_id.clone(),
				unit: None,
				bit_offset,
				bit_length: None,
				signed: false,
				big_endian: true,
				scaling: None,
				constant: field.dop.is_none(),
			};
			let Some(target) = field.dop.clone() else {
				out.push(item);
				continue;
			};
			// A refusal or a type with no loader costs this field its layout, not the
			// record: a multiplexed measurement value sits inside some records.
			match self.object(layer, home, &target) {
				Ok(Some(loaders::Object::Structure(inner))) => {
					self.environment_fields(layer, home, &inner, byte, depth + 1, out)?;
					continue;
				}
				Ok(Some(loaders::Object::Dop(dop))) => {
					if let Some(coded) = &dop.coded {
						item.bit_length = coded.bits.filter(|_| coded.length == loaders::measurement::CodedLength::Standard);
						item.signed = coded.base.is_signed();
						item.big_endian = coded.high_low_byte_order;
					}
					// A scaling only for an integer: a string, a float or a byte field is its
					// bytes, not a number its compu method was never meant for.
					let integer = dop.coded.as_ref().is_some_and(|c| c.base.is_integer());
					item.scaling = dop.compu.as_ref().filter(|_| integer).and_then(|c| c.scaling().ok());
					item.unit = match dop.unit.clone() {
						Some(target) => match self.object(layer, home, &target) {
							Ok(Some(loaders::Object::Unit(unit))) => unit.display_name.or(unit.long_name),
							_ => None,
						},
						None => None,
					};
				}
				Err(Error::Io(e)) => return Err(Error::Io(e)),
				_ => {}
			}
			out.push(item);
		}
		Ok(())
	}

	/// The layer that declares the fault-code properties, and its pool —
	/// [`Store::measurement_layer`] for the fault chain, and the same rule:
	/// the variant's own layer if it names any, else the first parent that does.
	fn fault_layer(&mut self, variant: &Variant, own: loaders::identity::LayerData) -> Result<Option<(loaders::identity::LayerData, String)>, Error> {
		if !own.dtc_properties.is_empty() {
			return Ok(Some((own, variant.pool.clone())));
		}
		for pool_id in own.parents.clone() {
			if let Some(loaders::Object::LayerData(parent)) = self.named(&pool_id, loaders::identity::LAYER_DATA_ID)?
				&& !parent.dtc_properties.is_empty()
			{
				return Ok(Some((parent, pool_id)));
			}
		}
		Ok(None)
	}

	/// Read the parent layers' data, once.
	fn load_inherited(&mut self, layer: &loaders::identity::LayerData, home: &str) -> Result<(), Error> {
		if !self.inherited.is_empty() {
			return Ok(());
		}
		let mut parents = vec![home.to_owned()];
		parents.extend(layer.parents.iter().cloned());
		for pool_id in parents {
			// Best-effort on purpose: a parent layer is consulted only to give a
			// reference its pool back, and a parent this reader cannot read
			// costs the lookups that needed it, not the whole variant.
			if let Ok(Some(loaders::Object::LayerData(data))) = self.named(&pool_id, loaders::identity::LAYER_DATA_ID) {
				self.inherited.push(data);
			}
		}
		Ok(())
	}

	/// A variant's own layer data.
	///
	/// Looked up by the generated name `LD_<variant>` first — VW's converter
	/// writes it for every ECU variant — and by the pool's own
	/// `#RtGen_DB_LAYER_DATA` when the variant *is* the pool's base variant.
	/// Neither is a car-specific constant: both are the converter's naming, and
	/// a miss falls through to scanning the pool for the layer data that names
	/// this variant.
	fn layer_data(&mut self, variant: &Variant) -> Result<Option<loaders::identity::LayerData>, Error> {
		let generated = format!("LD_{}", variant.name);
		for object_id in [generated.as_str(), loaders::identity::LAYER_DATA_ID] {
			// A name that is simply absent falls through to the next candidate;
			// a name that is present and does not parse is an error and is said
			// so. Swallowing it was how a broken layer became an empty channel
			// list, and an `Ok` with nothing in it says nothing is wrong.
			match self.named(&variant.pool, object_id)? {
				Some(loaders::Object::LayerData(data)) if data.variant.object.as_deref() == Some(variant.name.as_str()) => return Ok(Some(data)),
				_ => continue,
			}
		}
		self.scan_for_layer_data(variant)
	}

	/// Find a variant's layer data by walking its pool.
	///
	/// The slow path, and the reason it exists is the refusal list: the
	/// reference implementation reaches this object through an
	/// `MCD_ACCESS_KEY`, which this project never parses. Walking is the price
	/// of not parsing one, and it is only paid when the generated name misses.
	fn scan_for_layer_data(&mut self, variant: &Variant) -> Result<Option<loaders::identity::LayerData>, Error> {
		let strings = &self.project.strings;
		let Some(files) = self.pool(&variant.pool)? else { return Ok(None) };
		for record in files.key.records()? {
			let Ok(locator) = pool::Locator::parse(&record.data) else { continue };
			let Ok(bytes) = files.db.member(&locator) else { continue };
			if !matches!(object::type_code(&bytes), Ok(loaders::code::DB_LAYER_DATA)) {
				continue;
			}
			let Ok((_, mut stream)) = object::Stream::open(&bytes, strings) else {
				continue;
			};
			let Ok(data) = loaders::identity::layer_data(&mut stream) else {
				continue;
			};
			if data.variant.object.as_deref() == Some(variant.name.as_str()) {
				return Ok(Some(data));
			}
		}
		Ok(None)
	}

	/// The `(DID, name, text id)` list a table key's text table holds.
	fn identifiers(
		&mut self,
		layer: &loaders::identity::LayerData,
		home: &str,
		key: &loaders::measurement::Parameter,
	) -> Result<Vec<(u16, String, Option<String>)>, Error> {
		let table = match &key.inline_table {
			Some(table) => (**table).clone(),
			None => {
				let target = key.table.clone().ok_or_else(|| Error::Format("a table key names no table".into()))?;
				match self.object(layer, home, &target)? {
					Some(loaders::Object::Table(table)) => table,
					_ => return Err(Error::Missing(format!("the table a key names, {:?}", target.object))),
				}
			}
		};
		let target = table
			.key_dop
			.ok_or_else(|| Error::Format("a table has no key data object property".into()))?;
		let Some(loaders::Object::Dop(dop)) = self.object(layer, home, &target)? else {
			return Err(Error::Missing(format!("the data object property a table key names, {:?}", target.object)));
		};
		loaders::measurement::key_levels(&dop)
	}

	/// The `(row key, row reference)` list a table struct's table holds.
	fn rows(
		&mut self,
		layer: &loaders::identity::LayerData,
		home: &str,
		payload: &loaders::measurement::Parameter,
	) -> Result<Vec<(Option<String>, loaders::Ref)>, Error> {
		let target = payload
			.table
			.clone()
			.ok_or_else(|| Error::Format("a table struct names no table".into()))?;
		match self.object(layer, home, &target)? {
			Some(loaders::Object::Table(table)) => Ok(table.rows),
			_ => Err(Error::Missing(format!("the table a struct names, {:?}", target.object))),
		}
	}

	/// One DID's readings: its row, that row's structure, and each field of it.
	fn channel(
		&mut self,
		layer: &loaders::identity::LayerData,
		home: &str,
		row_ref: &loaders::Ref,
		did: u16,
		did_name: &str,
		text_id: Option<String>,
	) -> Result<Vec<Reading>, Error> {
		let Some(loaders::Object::TableRow(row)) = self.object(layer, home, row_ref)? else {
			return Err(Error::Missing(format!("the table row {:?}", row_ref.object)));
		};
		let row_byte = row.parameter.byte_position.unwrap_or(0);
		let structure_ref = row
			.parameter
			.dop
			.clone()
			.ok_or_else(|| Error::Format("a table row names no structure".into()))?;
		let Some(loaders::Object::Structure(structure)) = self.object(layer, home, &structure_ref)? else {
			return Err(Error::Missing(format!("the structure {:?}", structure_ref.object)));
		};

		let mut out = Vec::new();
		for field in &structure.fields {
			let Some(dop_ref) = field.dop.clone() else { continue };
			let Ok(Some(loaders::Object::Dop(dop))) = self.object(layer, home, &dop_ref) else {
				continue;
			};
			let (Some(coded), Some(compu)) = (dop.coded.as_ref(), dop.compu.as_ref()) else {
				continue;
			};
			let Some(bits) = coded.bits else { continue };
			// A scaling turns an integer into a number and nothing else: a string, a float or
			// a byte field read through it prints its bytes as an integer nobody wrote. Such a
			// channel has no honest reading here, so it is not one (`DataType::is_integer`).
			if !coded.base.is_integer() {
				continue;
			}
			let Ok(scaling) = compu.scaling() else { continue };
			let unit = match dop.unit.clone() {
				Some(target) => match self.object(layer, home, &target) {
					Ok(Some(loaders::Object::Unit(unit))) => unit.display_name.or(unit.long_name),
					_ => None,
				},
				None => None,
			};
			// A structure with one field is the DID itself; a structure with
			// several names each field after the channel it is. Neither name is
			// invented — both come out of the file.
			let name = if structure.fields.len() == 1 {
				did_name.to_owned()
			} else {
				field.long_name.clone().unwrap_or_else(|| did_name.to_owned())
			};
			let byte = row_byte.saturating_add(field.byte_position.unwrap_or(0));
			out.push(Reading {
				did,
				name,
				unit,
				bit_offset: byte.saturating_mul(8).saturating_add(u32::from(field.bit_position)),
				bit_length: bits,
				signed: coded.base.is_signed(),
				big_endian: coded.high_low_byte_order,
				scaling,
				text_id: if structure.fields.len() == 1 {
					text_id.clone()
				} else {
					field.long_name_id.clone()
				},
			});
		}
		Ok(out)
	}
}

/// The first record number two cases start on — `None` inside for two default cases — or
/// `None` when every case starts on its own.
fn repeated_start(ranges: &[Option<(i32, i32)>]) -> Option<Option<i32>> {
	let starts: Vec<Option<i32>> = ranges.iter().map(|r| r.map(|(lower, _)| lower)).collect();
	starts.iter().enumerate().find(|(at, s)| starts[..*at].contains(s)).map(|(_, s)| *s)
}

/// How many structures deep a case's fields may sit: the time stamp inside record `0x02` is
/// one level down. A deeper nesting than this is taken for a structure that names itself.
const MAX_STRUCTURE_DEPTH: usize = 8;

/// A record's length from its fields, in bytes after the record number: where its last
/// field ends, when that is known.
///
/// A field of unknown width (a multiplexed value, a coded constant) is allowed only where a
/// field of known width comes after it — it then ends before that one starts. A record whose
/// last field's width is unknown has no length here.
fn known_extent(fields: &[EnvironmentField]) -> Option<u32> {
	let end = fields
		.iter()
		.filter_map(|f| Some(f.bit_offset? + f.bit_length?))
		.max()
		.filter(|bits| *bits > 0)?;
	let unbounded_after = fields
		.iter()
		.any(|f| f.bit_length.is_none() && f.bit_offset.is_none_or(|start| start >= end));
	(!unbounded_after).then(|| end.div_ceil(8))
}

/// Find the row a DID's name keys.
///
/// Some pools spell a row key with spaces where the identifier list spells it
/// with underscores. That is a defect in VW's own files — their kernel
/// complains about it too — so the swap is tried rather than the channel
/// dropped.
fn row_for(rows: &[(Option<String>, loaders::Ref)], name: &str) -> Option<loaders::Ref> {
	let spaced = name.replace('_', " ");
	rows
		.iter()
		.find(|(key, _)| key.as_deref() == Some(name))
		.or_else(|| rows.iter().find(|(key, _)| key.as_deref() == Some(spaced.as_str())))
		.map(|(_, target)| target.clone())
}

/// Harvest every `(text id, text)` pair a parsed object carries, for
/// [`Project::names`].
///
/// Four shapes hold one. A parameter names itself; a structure holds fields
/// that do; a table row holds a parameter; and a data object property's text
/// table holds one pair per level — that last is the richest source, and the
/// one `.archive/research/labels/odis-crib.md` §3 turns into `TTTEXT` plaintext.
///
/// First writer wins. The pools are shared across a project, so a text id means
/// the same thing everywhere it appears; a later, differing text would be a
/// defect in the project, not new information.
fn harvest(object: &loaders::Object, into: &mut std::collections::BTreeMap<String, String>) {
	let mut pair = |id: &Option<String>, text: &Option<String>| {
		if let (Some(id), Some(text)) = (id, text)
			&& !id.is_empty()
			&& !text.is_empty()
		{
			into.entry(id.clone()).or_insert_with(|| text.clone());
		}
	};
	match object {
		loaders::Object::Parameter(p) => pair(&p.long_name_id, &p.long_name),
		loaders::Object::Structure(s) => {
			for field in &s.fields {
				pair(&field.long_name_id, &field.long_name);
			}
		}
		loaders::Object::TableRow(row) => pair(&row.parameter.long_name_id, &row.parameter.long_name),
		loaders::Object::Response(r) => {
			for p in &r.parameters {
				pair(&p.long_name_id, &p.long_name);
			}
		}
		loaders::Object::Unit(u) => pair(&u.long_name_id, &u.long_name),
		loaders::Object::Dop(dop) => {
			let Some(base) = dop.compu.as_ref().and_then(|m| m.internal_to_phys.as_ref()) else {
				return;
			};
			for scale in &base.scales {
				let text = match &scale.constant {
					Some(object::Value::Unicode(text) | object::Value::Ascii(text)) => text.clone(),
					_ => None,
				};
				pair(&scale.label_id, &text);
			}
		}
		// A fault text is keyed by its own text id and read through
		// [`Project::faults`], not through `names.json`: the id is the display
		// code (`P150B00`), which nothing that names a channel will ask for.
		loaders::Object::Service(_)
		| loaders::Object::Table(_)
		| loaders::Object::ProjectData(_)
		| loaders::Object::LayerData(_)
		| loaders::Object::EcuVariant(_)
		| loaders::Object::DtcDop(_)
		| loaders::Object::TroubleCode(_) => {}
	}
}

/// The project file that declares which vehicles it covers.
const COVERAGE_FILE: &str = "PRNR-INFO.xml";

/// Parse `PRNR-INFO.xml` into the vehicles it declares.
///
/// Scanned rather than parsed as XML, the way [`project_id`] scans `index.xml`:
/// the shape is flat and fixed, and `vag-data-labels` gains no dependency for it.
fn parse_coverage(text: &str) -> Result<Vec<Vehicle>, Error> {
	let mut out = Vec::new();
	for entry in entries(text, "VEHICLE") {
		let field = |tag: &str| element(entry, tag).map(decode_entities);
		let Some(type_code) = field("PRODUCT-ID") else {
			return Err(Error::Format(format!(
				"a vehicle in {COVERAGE_FILE} has no PRODUCT-ID, so nothing can select on it"
			)));
		};
		out.push(Vehicle {
			type_code,
			name: field("NAME").unwrap_or_default(),
			vehicle_project: field("VEHICLE-PROJECT").unwrap_or_default(),
			// The attribute is simply absent on an entry that is not the default.
			is_default: opening_tag(entry).contains("IS-DEFAULT=\"true\""),
		});
	}
	Ok(out)
}

/// The bodies of every `<tag …>…</tag>` element, opening tag included.
///
/// Matching on `<tag` alone would take `<VEHICLES>` for a `<VEHICLE>` and lose
/// every row to its container, so the character after the name has to be a
/// space or a `>`.
fn entries<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
	let open = format!("<{tag}");
	let close = format!("</{tag}>");
	let mut out = Vec::new();
	let mut at = 0usize;
	while let Some(found) = text[at..].find(&open) {
		let start = at + found;
		let after = start + open.len();
		let Some(next) = text[after..].chars().next() else { break };
		at = after;
		if next != ' ' && next != '>' && next != '\n' && next != '\t' && next != '\r' {
			continue; // `<VEHICLE-PROJECT>`, not `<VEHICLE>`.
		}
		let Some(end) = text[after..].find(&close) else { break };
		out.push(&text[start..after + end]);
		at = after + end + close.len();
	}
	out
}

/// The opening tag of an element body, for reading its attributes.
fn opening_tag(entry: &str) -> &str {
	entry.find('>').map_or(entry, |end| &entry[..end])
}

/// The text of the first `<tag>…</tag>` inside `text`.
fn element<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
	let open = format!("<{tag}>");
	let close = format!("</{tag}>");
	let start = text.find(&open)? + open.len();
	let end = text[start..].find(&close)?;
	Some(text[start..start + end].trim())
}

/// Decode XML's five predefined entities.
///
/// Nothing else is decoded, because nothing else occurs: these files carry
/// vehicle names and type codes, not markup.
fn decode_entities(text: &str) -> String {
	text
		.replace("&lt;", "<")
		.replace("&gt;", ">")
		.replace("&quot;", "\"")
		.replace("&apos;", "'")
		.replace("&amp;", "&")
}

/// The project's own short name, from `index.xml`, falling back to the
/// directory's.
///
/// Scanned for rather than parsed as XML: it is one element at a fixed depth,
/// and `vag-data-labels` gains no dependency for it.
fn project_id(dir: &std::path::Path) -> String {
	let fallback = || dir.file_name().and_then(|n| n.to_str()).unwrap_or("odis").to_owned();
	let Ok(text) = std::fs::read_to_string(dir.join("index.xml")) else {
		return fallback();
	};
	let Some(open) = text.find("<SHORT-NAME>") else { return fallback() };
	let rest = &text[open + "<SHORT-NAME>".len()..];
	let Some(close) = rest.find("</SHORT-NAME>") else { return fallback() };
	let name = rest[..close].trim();
	if name.is_empty() { fallback() } else { name.to_owned() }
}

/// The language `index.xml` declares in its `ADMIN-DATA`, if it declares one.
///
/// Scanned the way [`project_id`] scans for the short name. The first
/// `<LANGUAGE>` in the file is the catalog's own; the ones that may follow
/// belong to individual blocks and repeat it.
fn project_language(dir: &std::path::Path) -> Option<String> {
	let text = std::fs::read_to_string(dir.join("index.xml")).ok()?;
	let code = element(&text, "LANGUAGE")?;
	(!code.is_empty()).then(|| code.to_ascii_lowercase())
}

/// The converter's project version, from `DatabaseVersionInfo.txt`.
///
/// Plain `KEY="value"` lines. Only `VWMCD_ProjectVersionInfo` is taken; the
/// rest are there if a provenance log ever wants them.
fn project_version(dir: &std::path::Path) -> Option<String> {
	let text = std::fs::read_to_string(dir.join("DatabaseVersionInfo.txt")).ok()?;
	text.lines().find_map(|line| {
		let value = line.trim().strip_prefix("VWMCD_ProjectVersionInfo=")?;
		Some(value.trim().trim_matches('"').to_owned())
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::catalog::Scaling;
	use crate::measure::LinearScale;
	use loaders::code;

	/// Builds a whole miniature project — pools, objects and string pools — in
	/// a temporary directory.
	///
	/// **Nothing under `~/Downloads` or any real ODIS project is read by this
	/// test.** Every byte here is synthesised, which is also what makes the
	/// fixture readable: the shapes below are the format, spelled out.
	#[derive(Default)]
	struct Build {
		ascii: Vec<String>,
		unicode: Vec<String>,
		objects: Vec<(String, Vec<u8>)>,
	}

	/// One object under construction.
	#[derive(Default)]
	struct Obj {
		out: Vec<u8>,
	}

	impl Obj {
		fn u8(mut self, v: u8) -> Self {
			self.out.push(v);
			self
		}
		fn u16(mut self, v: u16) -> Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn u32(mut self, v: u32) -> Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn f64(mut self, v: f64) -> Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn hash(self, v: u32) -> Self {
			self.u32(v)
		}
		fn none(self) -> Self {
			self.u8(0)
		}
		fn some(self, type_code: u16) -> Self {
			self.u8(1).u16(type_code)
		}
		fn no_value(self) -> Self {
			self.u8(0xFF)
		}
		fn uint_value(self, v: u32) -> Self {
			self.u8(0x0B).u32(v)
		}
	}

	impl Build {
		/// Register an ASCII string and return the hash an object refers to it by.
		fn a(&mut self, s: &str) -> u32 {
			self.ascii.push(s.to_owned());
			hash::of_bytes(s.as_bytes())
		}
		/// Register a Unicode string and return its hash.
		fn u(&mut self, s: &str) -> u32 {
			self.unicode.push(s.to_owned());
			hash::of_utf16(&s.encode_utf16().collect::<Vec<_>>())
		}
		/// Store a finished object under an ObjectID.
		fn put(&mut self, object_id: &str, type_code: u16, obj: Obj) {
			self.a(object_id);
			let mut body = type_code.to_le_bytes().to_vec();
			body.extend_from_slice(&obj.out);
			body.extend_from_slice(&object::END);
			self.objects.push((object_id.to_owned(), body));
		}

		/// Write the project out: one pool plus the two string pools.
		fn write(&self, dir: &std::path::Path, pool_id: &str) {
			self.write_split(dir, |_| pool_id);
		}

		/// Write the project out with each object in the pool `pool_of` names for it, plus
		/// the two string pools.
		fn write_split<'p>(&self, dir: &std::path::Path, pool_of: impl Fn(&str) -> &'p str) {
			let mut pools: Vec<&str> = self.objects.iter().map(|(id, _)| pool_of(id)).collect();
			pools.sort_unstable();
			pools.dedup();
			for pool_id in pools {
				self.write_pool(dir, pool_id, |id| pool_of(id) == pool_id);
			}
			self.write_strings(dir);
		}

		/// One pool of the objects `mine` picks.
		fn write_pool(&self, dir: &std::path::Path, pool_id: &str, mine: impl Fn(&str) -> bool) {
			let mut db = Vec::new();
			let mut records: Vec<(u32, Vec<u8>)> = Vec::new();
			for (object_id, body) in self.objects.iter().filter(|(id, _)| mine(id)) {
				let member = miniz_oxide::deflate::compress_to_vec_zlib(body, 6);
				let mut locator = (db.len() as u32).to_le_bytes().to_vec();
				locator.extend_from_slice(&(member.len() as u32).to_le_bytes());
				locator.extend_from_slice(&(body.len() as u32).to_le_bytes());
				db.extend_from_slice(&member);
				records.push((hash::of_bytes(object_id.as_bytes()), locator));
			}
			std::fs::write(dir.join(format!("{pool_id}.db")), &db).expect("the fixture writes");
			std::fs::write(dir.join(format!("{pool_id}.key")), leaf_block(&records)).expect("the fixture writes");
		}

		/// The two string pools.
		fn write_strings(&self, dir: &std::path::Path) {
			let mut a = Vec::new();
			for s in &self.ascii {
				a.extend_from_slice(&(s.len() as u32).to_le_bytes());
				a.extend_from_slice(s.as_bytes());
			}
			let mut u = Vec::new();
			for s in &self.unicode {
				let units: Vec<u16> = s.encode_utf16().collect();
				u.extend_from_slice(&(units.len() as u32).to_le_bytes());
				for unit in units {
					u.extend_from_slice(&unit.to_le_bytes());
				}
			}
			std::fs::write(dir.join("AStringData.data"), a).expect("the fixture writes");
			std::fs::write(dir.join("UStringData.data"), u).expect("the fixture writes");
		}
	}

	/// One 4096-byte leaf block holding PBL's magic pseudo-item and the records.
	fn leaf_block(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
		let mut items: Vec<(Vec<u8>, Vec<u8>)> = vec![(Vec::new(), b"1.00 Peter's B Tree\0".to_vec())];
		items.extend(records.iter().map(|(hash, data)| (hash.to_le_bytes().to_vec(), data.clone())));

		let mut out = vec![0u8; keyfile::BLOCK];
		out[9..11].copy_from_slice(&(items.len() as u16).to_be_bytes());
		let mut at = 13usize;
		let mut prev: Vec<u8> = Vec::new();
		for (i, (key, data)) in items.iter().enumerate() {
			let common = key.iter().zip(prev.iter()).take_while(|(a, b)| a == b).count();
			let mut item = vec![key.len() as u8, common as u8, data.len() as u8];
			item.extend_from_slice(&key[common..]);
			item.extend_from_slice(data);
			out[at..at + item.len()].copy_from_slice(&item);
			let slot = keyfile::BLOCK - 2 * (i + 1);
			out[slot..slot + 2].copy_from_slice(&(at as u16).to_be_bytes());
			at += item.len();
			prev = key.clone();
		}
		out[11..13].copy_from_slice(&(at as u16).to_be_bytes());
		out
	}

	/// A `DB_DOP_SIMPLE_BASE`: `bits` unsigned bits, converted by `compu`.
	fn dop(b: &mut Build, name: &str, bits: u32, compu: Obj, unit: Option<&str>) -> Obj {
		let short = b.a(name);
		let mut o = Obj::default().hash(short);
		o.out.extend_from_slice(&compu.out);
		o = o
			.some(code::DB_DIAG_CODED_TYPE)
			.u8(2) // eSTANDARD_LENGTH_TYPE
			.u32(bits)
			.u8(0) // no bit mask
			.u8(1) // eDB_UINT32
			.u8(11) // eNONE encoding
			.u8(1) // high-low byte order
			.u8(0) // not condensed
			.some(code::DB_PHYSICAL_TYPE)
			.u8(3) // float64
			.u8(0) // no precision
			.u8(10) // decimal
			.u16(0)
			.u16(0); // the two index maps, both empty
		o = match unit {
			Some(unit) => {
				let object = b.a(unit);
				o.u8(1).hash(object).u32(0)
			}
			None => o.none(),
		};
		o.none().none() // no internal or physical constraint
	}

	/// A `DB_COMPU_METHOD` of category `IDENTICAL` — the coded value as it is.
	fn identical() -> Obj {
		Obj::default().some(code::DB_COMPU_METHOD).u8(0).none().none()
	}

	/// A `DB_COMPU_METHOD` of category `LINEAR`: `(offset + factor * x) / divisor`.
	fn linear(offset: f64, factor: f64, divisor: f64) -> Obj {
		Obj::default()
			.some(code::DB_COMPU_METHOD)
			.u8(1)
			.none()
			.some(code::DB_COMPU_BASE)
			.some(code::DB_COMPU_SCALES)
			.u32(1)
			.some(code::DB_COMPU_SCALE)
			.u32(0) // no label id
			.none() // no inverse coefficients
			.some(code::DB_COMPU_RATIONAL_COEFFS)
			.u8(2)
			.f64(offset)
			.f64(factor)
			.u8(1)
			.f64(divisor)
			.none()
			.none() // no physical limits
			.no_value()
			.no_value()
			.no_value()
			.none()
			.none() // no coded limits
			.no_value()
			.no_value() // the base's default and code byte stream
			.none() // no code information
			.no_value() // the base's inverse value
	}

	/// The common head of an `MCD_DB_PARAMETER`, up to and including its flags.
	fn parameter_head(b: &mut Build, long_name: Option<&str>, text_id: Option<&str>, byte: u32, dop_name: Option<&str>) -> Obj {
		let name = long_name.map_or(0, |s| b.u(s));
		let id = text_id.map_or(0, |s| b.a(s));
		let mut flags = 1 << 5; // the byte position is real
		if dop_name.is_some() {
			flags |= 1 << 3;
		}
		let o = Obj::default()
			.u32(0) // description
			.hash(name)
			.u32(0) // short name
			.u32(0) // some id
			.hash(id)
			.u32(0) // unique object id
			.u8(0) // bit position
			.u32(byte)
			.u8(flags)
			.u32(0) // display level
			.u32(0) // sys param
			.u8(1) // eVALUE
			.u8(0xFF); // no layer id
		match dop_name {
			Some(dop) => {
				let object = b.a(dop);
				o.hash(object).u32(0)
			}
			None => o,
		}
	}

	/// A `DB_COMPU_METHOD` of category `TEXTTABLE`: one closed one-value level per
	/// `(coded value, text)`.
	fn text_table(b: &mut Build, levels: &[(u32, &str)]) -> Obj {
		let mut o = Obj::default()
			.some(code::DB_COMPU_METHOD)
			.u8(3)
			.none() // no physical-to-coded direction
			.some(code::DB_COMPU_BASE)
			.some(code::DB_COMPU_SCALES)
			.u32(levels.len() as u32);
		for (value, text) in levels {
			let text = b.u(text);
			o = o
				.some(code::DB_COMPU_SCALE)
				.u32(0) // no label id
				.none()
				.none() // no coefficients either way
				.none()
				.none() // no physical limits
				.u8(0x0E)
				.hash(text) // the constant: the level's text
				.no_value()
				.no_value()
				.some(code::DB_LIMIT)
				.uint_value(*value)
				.u8(2) // the coded lower limit, closed
				.some(code::DB_LIMIT)
				.uint_value(*value)
				.u8(2); // the coded upper limit, closed
		}
		o.no_value() // the base's default
			.no_value() // its code byte stream
			.none() // no code information
			.no_value() // its inverse value
	}

	/// A project with units to name: one base variant `BV_SimDash` with one ECU variant, a
	/// `.vi` pool whose links reach it (`LL_SimDash`, on UDS) and every system (`LL_AllSim`),
	/// a link on another protocol (`LL_SimOld`, to `BV_SimOld`, which no variant derives
	/// from), one object there that does not parse, and a library whose table of logical links
	/// gives `LL_SimDash` the address `0x17` — beside a two-byte table of the same links,
	/// which is not the one-byte numbering and is not read. `linked` false leaves the base
	/// variant reached by no link. Made-up names.
	fn named_units_project(dir: &std::path::Path, linked: bool) -> Project {
		let bv_pool = "0.0.0@BV_SimDash.bv";
		let vi_pool = "0.0.0@VI_Sim.vi";
		let lib_pool = "0.0.0@ES_SimLib.sd";
		let mut b = Build::default();
		let pool_hash = b.a(bv_pool);
		b.a(vi_pool);
		b.a(lib_pool);
		let bv = b.a("BV_SimDash");
		let ev = b.a("EV_SimDash_001");
		let pd = Obj::default()
			.u16(0) // no location references
			.u32(0)
			.u32(0)
			.u32(0) // no functional group
			.hash(bv)
			.hash(pool_hash)
			.u32(0) // the base variant reference
			.u16(1)
			.hash(ev)
			.hash(ev)
			.hash(pool_hash) // one ECU variant: name, object, pool
			.u32(0)
			.u32(0)
			.u32(0) // the trailing ECU variant reference
			.u32(0)
			.u32(0)
			.u32(0) // three names
			.u16(0) // no functional groups
			.u16(0); // no nested project data
		b.put("#RtGen_DB_PROJECT_DATA", code::DB_PROJECT_DATA, pd);

		for (link, long, protocol, reaches, group) in [
			("LL_SimDash", "Sim Dash UDS", "PR_UDSOnCAN", linked.then_some("BV_SimDash"), None),
			("LL_AllSim", "All Sim Systems", "PR_UDSOnCAN", None, Some("FG_AllSim")),
			("LL_SimOld", "Sim Old UDS", "PR_KWP2000OnCAN", Some("BV_SimOld"), None),
		] {
			let protocol = b.a(protocol);
			let short = b.a(link);
			let long = b.u(long);
			let reaches = reaches.map_or(0, |r| b.a(r));
			let group = group.map_or(0, |g| b.a(g));
			let o = Obj::default()
				.hash(short)
				.hash(long)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0) // description, reserved, the two text ids
				.hash(protocol)
				.hash(reaches)
				.hash(group);
			b.put(&format!("{link}.object"), code::MCD_DB_LOGICAL_LINK, o);
		}
		// A link whose bytes stop before its first name: skipped, and counted.
		b.put("LL_SimBroken.object", code::MCD_DB_LOGICAL_LINK, Obj::default());

		for (object, short, levels) in [
			(
				"DOP_ES_SimLib_DOP_TEXTTABLELogicLinkNames",
				loaders::identity::LOGIC_LINK_NAMES,
				&[(0x17, "LL_SimDash"), (0x19, "LL_SimGateway")][..],
			),
			(
				"DOP_ES_SimLib_DOP_TEXTTABLELogicLinkNames2ByteECUID",
				"DOP_TEXTTABLELogicLinkNames2ByteECUID",
				&[(0x8117, "LL_SimDash")][..],
			),
		] {
			let compu = text_table(&mut b, levels);
			let o = dop(&mut b, short, 8, compu, None);
			b.put(object, code::DB_DOP_SIMPLE_BASE, o);
		}
		b.write_split(dir, |id| match id {
			"#RtGen_DB_PROJECT_DATA" => bv_pool,
			id if id.starts_with("LL_") => vi_pool,
			_ => lib_pool,
		});
		Project::open(dir).expect("the fixture opens")
	}

	#[test]
	fn a_unit_is_named_by_the_link_that_reaches_it_and_numbered_by_the_table_of_links() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let project = named_units_project(dir.path(), true);
		let named = project.unit_names().expect("unit names read");
		let row = |variant: &str| UnitName {
			variant: variant.into(),
			base_variant: "BV_SimDash".into(),
			link: "LL_SimDash".into(),
			name: Some("Sim Dash".into()),
			address: Some(0x17),
		};
		// The base variant and its ECU variant, both through the one link; the two-byte table
		// and the link to every system give nothing. The broken link costs itself alone.
		assert_eq!(named.units, [row("BV_SimDash"), row("EV_SimDash_001")]);
		assert_eq!(named.skipped, 1);
	}

	#[test]
	fn a_name_loses_the_word_its_links_protocol_adds_and_no_other() {
		assert_eq!(unit_name_of("Dash Board UDS", Some("PR_UDSOnCAN")), "Dash Board");
		// Another protocol's link keeps its name as written, and so does one with no protocol.
		assert_eq!(unit_name_of("Sim Old UDS", Some("PR_KWP2000OnCAN")), "Sim Old UDS");
		assert_eq!(unit_name_of("Dash Board UDS", None), "Dash Board UDS");
		// A name that is nothing but the word stays whole; one without it is untouched.
		assert_eq!(unit_name_of("UDS", Some("PR_UDSOnCAN")), "UDS");
		assert_eq!(unit_name_of("Gateway", Some("PR_UDSOnCAN")), "Gateway");
	}

	#[test]
	fn a_unit_no_link_reaches_has_no_name() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let project = named_units_project(dir.path(), false);
		assert_eq!(project.unit_names().expect("unit names read").units, []);
	}

	/// Build the whole fixture: one pool, one variant, two readings.
	///
	/// `inherit` selects which of the two shapes the real files use. `false`
	/// puts the measurement service on the ECU variant's own layer. `true` gives
	/// that layer **no** services and one parent, and puts the service on the
	/// pool's base-variant layer instead — which is what
	/// `EV_DCUDriveSideEWMAXCONT_006` looks like on disk. Both must yield the
	/// same channels; that is the whole assertion.
	fn miniature_project(dir: &std::path::Path, inherit: bool) -> (Project, String) {
		miniature_project_serving(dir, inherit, loaders::identity::RDBI_MEASUREMENT)
	}

	/// The same project, its one `ReadDataByIdentifier` service under the short name
	/// `service`.
	fn miniature_project_serving(dir: &std::path::Path, inherit: bool, service: &str) -> (Project, String) {
		let pool_id = "0.0.0@BV_Test.bv";
		let mut b = Build::default();
		let pool_hash = b.a(pool_id);

		// The project data: one base variant, one ECU variant derived from it.
		let bv = b.a("BV_Test");
		let ev = b.a("EV_Test");
		// One location reference carrying an access key, because every real
		// base-variant pool has them in front of the variant list, and the
		// point of the fixture is the shape the reader actually meets.
		let mut pd = Obj::default().u16(1);
		pd = pd.u32(0).hash(pool_hash).u8(1).u16(code::MCD_ACCESS_KEY);
		for _ in 0..7 {
			pd = pd.u32(0);
		}
		pd = pd.u16(0x0102).u32(0); // the key's location type and its last name
		pd = pd
			.u32(0)
			.u32(0)
			.u32(0) // no functional group
			.hash(bv)
			.hash(pool_hash)
			.u32(0) // the base variant reference
			.u16(1)
			.hash(ev)
			.hash(ev)
			.hash(pool_hash) // one ECU variant: name, object, pool
			.u32(0)
			.u32(0)
			.u32(0) // the trailing ECU variant reference
			.u32(0)
			.u32(0)
			.u32(0) // three names
			.u16(0) // no functional groups
			.u16(0); // no nested project data
		b.put("#RtGen_DB_PROJECT_DATA", code::DB_PROJECT_DATA, pd);

		// The variant's layer data: one service, no inherited indexes.
		let rdbi = b.a(service);
		let svc = b.a("SVC_Measu");
		// One layer, built twice with different contents when the fixture is
		// exercising inheritance: the service sits on whichever layer is
		// supposed to own it, and the other names a parent instead.
		// The fault-code property's short name and the object it indexes. The
		// name is what the layer's `dtc_properties` list holds and what its
		// property index is keyed by; the object is what the index points at.
		let dtcdop = b.a("DTCDOP_VAGUDS");
		let dtcdop_object = b.a("DOP_EV_Test_DTCDOP_VAGUDS");
		// The extended-data multiplexer, indexed beside it by its short name.
		let mux_key = b.a(loaders::extended::EXTENDED_DATA_MULTIPLEXER);
		let mux_object = b.a("DOP_EV_Test_MUX_DTCExtenDataRecor");
		let layer = |variant_hash: u32, services: bool, parents: bool| {
			let mut o = Obj::default()
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0) // five leading names
				.u16(0x0102) // eECU_VARIANT
				.hash(variant_hash)
				.hash(pool_hash);
			o = match services {
				// The five bytes after the pool belong to the *entry* — the
				// attributed reference's tail, the number, the object type and
				// the flag — so a map with no entries must not carry them.
				true => o.u16(1).hash(rdbi).hash(svc).hash(pool_hash).u8(0).u8(0).u16(0x0C83).u8(0),
				// A layer with no services of its own — legal, and common:
				// 36 of this project's variants are like it.
				false => o.u16(0),
			};
			// The fault memory sits on the same layer as the services, so the
			// inheritance case exercises both chains at once: one DTC property
			// by name, and the property index entry that resolves it.
			o = match services {
				true => o
					.u16(1)
					.hash(dtcdop)
					.u16(2)
					.hash(dtcdop)
					.hash(dtcdop_object)
					.u32(0)
					.hash(mux_key)
					.hash(mux_object)
					.hash(pool_hash),
				false => o.u16(0).u16(0),
			};
			o = o
				.u16(0) // no table index
				.u16(0)
				.u16(0)
				.u16(0) // requests, global negative responses, functional classes
				.u16(0) // no functional-class data primitives
				.u16(0)
				.u16(0)
				.u16(0) // the three always-empty maps
				.u16(0); // no environment-data descriptions
			o = match parents {
				true => o.u16(1).hash(pool_hash),
				false => o.u16(0),
			};
			o.u16(0) // no shared-data parents
				.u16(0)
				.u16(0)
				.u16(0)
				.u16(0) // four string-vector maps
				.u16(0)
				.u16(0) // unit groups, units
				.u16(0) // no protocol parameters
				.u8(0)
				.u8(0) // the trailing byte, and no special data groups
				.u16(0) // the final diag-com map
		};
		b.put("LD_EV_Test", code::DB_LAYER_DATA, layer(ev, !inherit, inherit));
		if inherit {
			// The base variant's layer, which is where the parent lookup goes.
			b.put(loaders::identity::LAYER_DATA_ID, code::DB_LAYER_DATA, layer(bv, true, false));
		}

		// The service and its positive response.
		let rsp = b.a("RSP_Measu");
		b.put(
			"SVC_Measu",
			code::MCD_DB_SERVICE,
			Obj::default()
				.u8(1) // repetition mode
				.u16(0) // no protocol parameter sets
				.u16(0x6901) // runtime mode
				.u8(0) // not multiple
				.none() // no access level
				.none() // no audience
				.u8(1) // repetition
				.u16(0) // no related primitives
				.u8(0) // status byte
				.u32(0)
				.u32(0)
				.u32(0) // id, long name id, unique object id
				.u32(0)
				.u32(0)
				.u32(0) // description, long name, short name
				.none() // no request
				.u16(1)
				.hash(rsp)
				.hash(rsp)
				.hash(pool_hash) // one positive response: name, object, pool
				.u16(0)
				.u16(0) // no negative responses, no functional classes
				.u32(0) // semantic
				.u16(0x6A02) // transmission mode
				.u8(1)
				.u8(0) // executable, not a no-op
				.u8(0) // diagnostic class
				.u16(0)
				.u16(0) // no state transitions, no states
				.u8(0), // no suppress-positive-response capability
		);

		// The response: a table key at byte 1 and a table struct at byte 3.
		let mut key = parameter_head(&mut b, None, None, 1, None);
		key = key.some(code::MCD_DB_TABLE); // the key's table, inline
		let keydop = b.a("DOP_Key");
		key = key
			.u32(0)
			.u32(0)
			.u32(0) // reserved, some id, object id
			.u32(0)
			.u32(0)
			.u32(0) // description, long name, short name
			.u32(0) // no rows
			.u32(0) // semantic
			.u16(0) // no diag com primitives
			.u8(1)
			.hash(keydop)
			.u32(0) // the key data object property
			.u8(0) // no special data groups
			.u8(0) // not a table row reference
			.u32(0); // trailing name

		let tab = b.a("TAB_Measu");
		let payload = parameter_head(&mut b, None, None, 3, None)
			.u32(0) // the key parameter's short name
			.hash(tab)
			.hash(pool_hash)
			.u8(0); // the table reference

		let mut response = Obj::default().u32(0).u32(0).u32(0).u32(0).u32(0).u32(0);
		response = response.some(code::MCD_DB_RESPONSE_PARAMETERS).u16(2);
		response = response.some(code::MCD_DB_PARAMETER_TABLE_KEY);
		response.out.extend_from_slice(&key.out);
		response = response.some(code::MCD_DB_PARAMETER_TABLESTRUCT);
		response.out.extend_from_slice(&payload.out);
		b.put("RSP_Measu", code::MCD_DB_RESPONSE, response.u16(0).u8(0));

		// The key's text table: two DIDs, each naming a row of the table.
		let speed = "Getriebe-Eingangsdrehzahl";
		let rpm = "Motordrehzahl";
		let mut levels = Obj::default()
			.some(code::DB_COMPU_METHOD)
			.u8(3)
			.none()
			.some(code::DB_COMPU_BASE)
			.some(code::DB_COMPU_SCALES)
			.u32(2);
		for (did, name, id) in [(0x380Au32, speed, "000116"), (0x2000, rpm, "000117")] {
			let label = b.a(id);
			let text = b.u(name);
			levels = levels
				.some(code::DB_COMPU_SCALE)
				.hash(label)
				.none()
				.none() // no coefficients either way
				.none()
				.none() // no physical limits
				.u8(0x0E)
				.hash(text) // the COMPU-CONST: the channel's name
				.no_value()
				.no_value()
				.some(code::DB_LIMIT)
				.uint_value(did)
				.u8(2)
				.some(code::DB_LIMIT)
				.uint_value(did)
				.u8(2);
		}
		levels = levels.no_value().no_value().none().no_value();
		let key_dop = dop(&mut b, "DOP_Key_inner", 16, levels, None);
		b.put("DOP_Key", code::DB_DOP_SIMPLE_BASE, key_dop);

		// The row table, one row per DID.
		let row_a = b.a("TBP_Speed");
		let row_b = b.a("TBP_Rpm");
		let speed_key = b.u(speed);
		let rpm_key = b.u(rpm);
		b.put(
			"TAB_Measu",
			code::MCD_DB_TABLE,
			Obj::default()
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(2)
				.hash(speed_key)
				.hash(row_a)
				.hash(pool_hash)
				.u32(0)
				.hash(rpm_key)
				.hash(row_b)
				.hash(pool_hash)
				.u32(0)
				.u32(0) // semantic
				.u16(0) // no diag com primitives
				.u8(0) // no key data object property
				.u8(0), // no special data groups
		);

		// Each row: a parameter pointing at a one-field structure.
		for (row, structure) in [("TBP_Speed", "STRUC_Speed"), ("TBP_Rpm", "STRUC_Rpm")] {
			let head = parameter_head(&mut b, None, None, 0, Some(structure));
			let mut o = Obj::default()
				.u32(0) // the row key
				.none() // no audience
				.u8(0)
				.u8(0); // no disabled or enabled audiences
			o.out.extend_from_slice(&head.out);
			b.put(row, code::MCD_DB_TABLE_PARAMETER, o);
		}

		// Each structure: one field, pointing at its data object property.
		for (structure, property, name, id) in [("STRUC_Speed", "DOP_Speed", speed, "000116"), ("STRUC_Rpm", "DOP_Rpm", rpm, "000117")] {
			let field = parameter_head(&mut b, Some(name), Some(id), 0, Some(property));
			let mut o = Obj::default()
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0) // the structure's six names
				.u16(2) // two bytes wide
				.some(code::MCD_DB_PARAMETERS)
				.u16(1)
				.some(code::MCD_DB_PARAMETER);
			o.out.extend_from_slice(&field.out);
			b.put(structure, code::MCD_DB_PARAMETER_STRUCTURE, o);
		}

		// The two scalings. `IDENTICAL` is the design's §1 cross-check: DID
		// 0x380A on the reference engine comes back raw, which is what driving
		// proved from the other direction.
		let identical_dop = dop(&mut b, "DOP_Speed_inner", 16, identical(), None);
		b.put("DOP_Speed", code::DB_DOP_SIMPLE_BASE, identical_dop);
		let linear_dop = dop(&mut b, "DOP_Rpm_inner", 16, linear(0.0, 1.0, 4.0), Some("UNIT_Rpm"));
		b.put("DOP_Rpm", code::DB_DOP_SIMPLE_BASE, linear_dop);

		let display = b.u("/min");
		b.put(
			"UNIT_Rpm",
			code::MCD_DB_UNIT,
			Obj::default()
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0) // the unit's six names
				.hash(display)
				.f64(1.0)
				.f64(0.0)
				.none() // no physical dimension
				.u8(0), // no unit group references
		);

		// The fault records: an extended-data multiplexer keyed by the record number.
		// Case 1, two one-byte fields, two bytes declared; case 112, an engine speed (the
		// 16-bit `/min` property above) and an 8-bit voltage, no size declared; and a
		// default case with no structure.
		let byte_dop = dop(&mut b, "DOP_Byte_inner", 8, identical(), None);
		b.put("DOP_Byte", code::DB_DOP_SIMPLE_BASE, byte_dop);
		let volt_dop = dop(&mut b, "DOP_Volt_inner", 8, linear(0.0, 1.0, 10.0), None);
		b.put("DOP_Volt", code::DB_DOP_SIMPLE_BASE, volt_dop);
		let key_dop = dop(&mut b, "DOP_UINTHEX1Byte_inner", 8, identical(), None);
		b.put("DOP_UINTHEX1Byte", code::DB_DOP_SIMPLE_BASE, key_dop);
		let structure = |b: &mut Build, bytes: u16, fields: &[(&str, u32, &str)]| {
			let mut o = Obj::default().u32(0).u32(0).u32(0).u32(0).u32(0).u32(0).u16(bytes);
			o = o.some(code::MCD_DB_PARAMETERS).u16(fields.len() as u16);
			for (name, byte, property) in fields {
				o = o.some(code::MCD_DB_PARAMETER);
				let field = parameter_head(b, Some(name), None, *byte, Some(property));
				o.out.extend_from_slice(&field.out);
			}
			o
		};
		let info = structure(&mut b, 2, &[("Priority", 0, "DOP_Byte"), ("Occurrences", 1, "DOP_Byte")]);
		b.put("STRUC_Info", code::MCD_DB_PARAMETER_STRUCTURE, info);
		let own = structure(&mut b, 0, &[("Engine speed", 0, "DOP_Rpm"), ("Voltage", 2, "DOP_Volt")]);
		b.put("STRUC_Own", code::MCD_DB_PARAMETER_STRUCTURE, own);
		// A structure whose one field is itself: no case names it; the depth test reads it.
		let looped = structure(&mut b, 0, &[("Again", 0, "STRUC_Loop")]);
		b.put("STRUC_Loop", code::MCD_DB_PARAMETER_STRUCTURE, looped);
		let key_dop = b.a("DOP_UINTHEX1Byte");
		let mut mux = Obj::default()
			.u32(1) // the cases start one byte in, after the record number
			.some(0x004C)
			.u8(0)
			.u32(0)
			.hash(key_dop)
			.u32(0)
			.some(code::DB_CASES)
			.u32(2);
		for (number, short, target) in [(1u32, "Case_Info", "STRUC_Info"), (112, "Case_Own", "STRUC_Own")] {
			mux = mux.some(code::DB_CASE);
			for _ in 0..2 {
				mux = mux.some(code::DB_LIMIT).uint_value(number).u8(2);
			}
			let (short, long, target) = (b.a(short), b.u("a case"), b.a(target));
			mux = mux.hash(short).hash(long).u32(0).u32(0).u32(0).u32(0).u8(1).hash(target).hash(pool_hash);
		}
		let (short, long) = (b.a("Case_Unknown"), b.u("unknown freeze frame"));
		mux = mux.some(code::DB_DEFAULT_CASE).hash(short).hash(long).u32(0).u32(0).u32(0).u32(0).u8(0);
		b.put("DOP_EV_Test_MUX_DTCExtenDataRecor", code::MCD_DB_PARAMETER_MULTIPLEXER, mux);

		// The fault memory: a table of two codes, each its own object. The
		// numbers are the reference car's brake unit's `00 01 29` and steering
		// column's `04 71 20`, read as one integer each, which is what the join
		// from a `0x19` response to a row rests on. The first reference names
		// its pool and the second omits it, as the real files do both.
		let dtc_a = b.a("DTC_EV_Test_DTCDOP_VAGUDS.DTC_297");
		let dtc_b = b.a("DTC_EV_Test_DTCDOP_VAGUDS.DTC_291104");
		let dop_long = b.u("VAG UDS");
		b.put(
			"DOP_EV_Test_DTCDOP_VAGUDS",
			code::DB_DOP_DTC,
			Obj::default()
				.u16(2)
				.u32(297)
				.hash(dtc_a)
				.hash(pool_hash)
				.u32(291_104)
				.hash(dtc_b)
				.u32(0)
				.u16(0) // the always-empty second collection
				.some(code::DB_COMPU_METHOD)
				.u8(0)
				.none()
				.none() // identical
				.some(code::DB_DIAG_CODED_TYPE)
				.u8(2)
				.u32(24)
				.u8(0)
				.u8(1)
				.u8(11)
				.u8(1)
				.u8(0) // 24 unsigned bits, high-low
				.some(code::DB_PHYSICAL_TYPE)
				.u8(1)
				.u8(0)
				.u8(16) // an unsigned integer in base 16
				.hash(dtcdop)
				.hash(dop_long)
				.u32(0)
				.u32(0)
				.u32(0)
				.u32(0), // description and three ids, absent
		);
		for (object, number, display, text, level) in [
			(
				"DTC_EV_Test_DTCDOP_VAGUDS.DTC_297",
				297u32,
				"B1168F2",
				"Lenkwinkelsensor\nkeine Grundeinstellung",
				2u32,
			),
			(
				"DTC_EV_Test_DTCDOP_VAGUDS.DTC_291104",
				291_104,
				"B145501",
				"Temperatursensor Lenkradheizung",
				3,
			),
		] {
			let short = b.a(&format!("DTC_{number}"));
			let display_hash = b.a(display);
			let text_hash = b.u(text);
			b.put(
				object,
				code::MCD_DB_DIAG_TROUBLE_CODE,
				Obj::default()
					.u32(0) // no ODX id, as the body electronics have none
					.hash(short)
					.hash(display_hash)
					.hash(text_hash)
					.u32(level)
					.u32(number)
					.u8(0) // not temporary
					.hash(display_hash), // the text id, equal to the display code
			);
		}

		b.write(dir, pool_id);
		std::fs::write(
			dir.join("index.xml"),
			"<CATALOG><SHORT-NAME>TEST7X</SHORT-NAME><ADMIN-DATA><LANGUAGE>deu</LANGUAGE></ADMIN-DATA></CATALOG>",
		)
		.expect("the fixture writes");
		std::fs::write(
			dir.join("DatabaseVersionInfo.txt"),
			"VWMCD_ConverterVersionInfo=\"26.1.0.0\"\nVWMCD_ProjectVersionInfo=\"2610.2.688\"\n",
		)
		.expect("the fixture writes");
		(Project::open(dir).expect("the fixture is a project"), pool_id.to_owned())
	}

	#[test]
	fn a_variant_that_declares_no_service_inherits_its_base_variant_s() {
		// Measured on the reference car, 2026-08-10: its two front door units
		// answer `EV_DCUDriveSideEWMAXCONT_006` and `EV_DCUPasseSideEWMAXCONT_006`,
		// both of which are in the project, both of whose layers parse
		// completely, and both of which declare **zero** services while naming
		// `0.0.0@BV_DoorElectDriveSideUDS.bv` as a parent. Reading only the
		// variant's own layer reported those units as having no measurements at
		// all — which is what `watch` showed for the doors, and what made them
		// look like a gap in VW's data rather than in this reader.
		//
		// Across the whole project it is 36 variants and 88,549 channels.
		let direct = tempfile::tempdir().expect("a temporary directory");
		let inherited = tempfile::tempdir().expect("a temporary directory");
		let (a, _) = miniature_project(direct.path(), false);
		let (b, pool_id) = miniature_project(inherited.path(), true);

		// The fixture is the shape this test is about, and not by accident: a
		// mis-encoded layer that still carried its own service would make
		// everything below pass while testing nothing.
		let mut store = Store::new(&b);
		let variants = b.variants().expect("the project data parses");
		let ev = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
		let own = store.layer_data(ev).expect("the layer reads").expect("the layer is there");
		assert!(own.services.is_empty(), "the variant declares nothing itself");
		assert_eq!(own.parents, vec![pool_id], "and names where to look");

		let readings = |p: &Project| {
			let variants = p.variants().expect("the project data parses");
			let variant = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
			let mut rows = p.readings(variant).expect("the measurement chain walks");
			rows.sort_by_key(|r| r.did);
			rows
		};

		let inherited_rows = readings(&b);
		assert_eq!(inherited_rows.len(), 2, "the channels come through the parent: {inherited_rows:#?}");
		// Identical, not merely non-empty. Resolving the chain against the
		// wrong pool is the way this half-works: the service is found and its
		// tables are not, and the result is a shorter list nobody notices.
		assert_eq!(inherited_rows, readings(&a));
	}

	#[test]
	fn the_adaptation_read_is_its_own_service_and_reads_through_the_same_chain() {
		// Each list comes from its own service and from nothing else: a unit's
		// adaptations are not its measurements, and a variant that serves one has
		// none of the other.
		let rows = |service: &str, inherit: bool| {
			let dir = tempfile::tempdir().expect("a temporary directory");
			let (project, _) = miniature_project_serving(dir.path(), inherit, service);
			let variants = project.variants().expect("the project data parses");
			let variant = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
			let mut adaptations = project.adaptations(variant).expect("the chain walks");
			let mut readings = project.readings(variant).expect("the chain walks");
			adaptations.sort_by_key(|r| r.did);
			readings.sort_by_key(|r| r.did);
			(adaptations, readings)
		};
		let (adaptations, readings) = rows(loaders::identity::RDBI_ADAPTATION, false);
		assert!(readings.is_empty(), "{readings:#?}");
		let (none, measured) = rows(loaders::identity::RDBI_MEASUREMENT, false);
		assert!(none.is_empty(), "{none:#?}");
		// The same rows, layout and scaling, as the measurement service gives.
		assert_eq!(adaptations, measured);
		assert_eq!(adaptations.len(), 2);
		// And inherited from a base variant by the same rule.
		assert_eq!(rows(loaders::identity::RDBI_ADAPTATION, true).0, adaptations);
	}

	#[test]
	fn a_variants_fault_records_come_from_its_extended_data_multiplexer() {
		for inherit in [false, true] {
			let dir = tempfile::tempdir().expect("a temporary directory");
			let (project, _) = miniature_project(dir.path(), inherit);
			let variants = project.variants().expect("the project data parses");
			let ev = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
			let records = project.extended_records(ev).expect("the multiplexer reads");
			let shape = records.iter().map(|r| (r.range, r.length, r.fields.len())).collect::<Vec<_>>();
			assert_eq!(
				shape,
				vec![(Some((1, 1)), Some(2), 2), (Some((112, 112)), Some(3), 2), (None, None, 0)],
				"inherit {inherit}"
			);
			let speed = &records[1].fields[0];
			assert_eq!(
				(speed.name.as_deref(), speed.bit_offset, speed.bit_length, speed.unit.as_deref()),
				(Some("Engine speed"), Some(0), Some(16), Some("/min"))
			);
			assert_eq!(
				speed.scaling,
				Some(crate::catalog::Scaling::Linear(crate::LinearScale { factor: 0.25, offset: 0.0 }))
			);
			let volts = &records[1].fields[1];
			assert_eq!((volts.bit_offset, volts.bit_length), (Some(16), Some(8)));
		}
	}

	#[test]
	fn two_cases_on_one_record_number_are_found() {
		assert_eq!(repeated_start(&[Some((1, 1)), Some((2, 2)), None]), None);
		assert_eq!(repeated_start(&[Some((112, 112)), Some((112, 113))]), Some(Some(112)));
		assert_eq!(repeated_start(&[None, Some((1, 1)), None]), Some(None));
	}

	#[test]
	fn a_structure_that_contains_itself_is_refused_not_followed_down() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, pool_id) = miniature_project(dir.path(), false);
		let variants = project.variants().expect("the project data parses");
		let ev = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
		let mut store = Store::new(&project);
		let own = store.layer_data(ev).expect("the layer reads").expect("the layer is there");
		let Some(loaders::Object::Structure(looped)) = store.named(&pool_id, "STRUC_Loop").expect("the fixture reads") else {
			panic!("the fixture has the structure");
		};
		let mut fields = Vec::new();
		let refused = store.environment_fields(&own, &pool_id, &looped, 0, 0, &mut fields);
		assert!(matches!(&refused, Err(Error::Format(m)) if m.contains("contains itself")), "{refused:?}");
	}

	#[test]
	fn a_project_reads_its_variants_and_their_readings() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, pool_id) = miniature_project(dir.path(), false);

		// The identity comes off index.xml, not off the directory name — a
		// folder gets renamed by an unzip and `<SHORT-NAME>` does not.
		assert_eq!(project.id(), "TEST7X");
		assert_eq!(project.version(), Some("2610.2.688"));
		assert_eq!(project.pools(), std::slice::from_ref(&pool_id));

		let variants = project.variants().expect("the project data parses");
		assert_eq!(
			variants,
			vec![
				Variant {
					name: "BV_Test".into(),
					pool: pool_id.clone(),
					base_variant: None
				},
				Variant {
					name: "EV_Test".into(),
					pool: pool_id.clone(),
					base_variant: Some("BV_Test".into())
				},
			]
		);

		let variant = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
		let mut readings = project.readings(variant).expect("the measurement chain walks");
		readings.sort_by_key(|r| r.did);
		assert_eq!(readings.len(), 2, "both channels must come back, got {readings:#?}");

		// 0x2000: (0 + 1 * x) / 4, in /min.
		assert_eq!(
			readings[0],
			Reading {
				did: 0x2000,
				name: "Motordrehzahl".into(),
				unit: Some("/min".into()),
				bit_offset: 0,
				bit_length: 16,
				signed: false,
				big_endian: true,
				scaling: Scaling::Linear(LinearScale { factor: 0.25, offset: 0.0 }),
				text_id: Some("000117".into()),
			}
		);
		// 0x380A: IDENTICAL, i.e. raw u16 — the design's §1 cross-check.
		assert_eq!(
			readings[1],
			Reading {
				did: 0x380A,
				name: "Getriebe-Eingangsdrehzahl".into(),
				unit: None,
				bit_offset: 0,
				bit_length: 16,
				signed: false,
				big_endian: true,
				scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
				text_id: Some("000116".into()),
			}
		);
	}

	#[test]
	fn a_project_reads_a_variants_fault_codes_by_their_number() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		// Declared once, in index.xml, for the whole project.
		assert_eq!(project.language(), Some("deu"));

		let variants = project.variants().expect("the project data parses");
		let variant = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
		let mut faults = project.faults(variant).expect("the fault chain walks");
		faults.sort_by_key(|f| f.code);
		assert_eq!(
			faults,
			vec![
				Fault {
					dop: "DTCDOP_VAGUDS".into(),
					code: 297,
					display_code: Some("B1168F2".into()),
					text: Some("Lenkwinkelsensor\nkeine Grundeinstellung".into()),
					text_id: Some("B1168F2".into()),
					short_name: Some("DTC_297".into()),
					level: 2,
					temporary: false,
				},
				Fault {
					dop: "DTCDOP_VAGUDS".into(),
					code: 291_104,
					display_code: Some("B145501".into()),
					text: Some("Temperatursensor Lenkradheizung".into()),
					text_id: Some("B145501".into()),
					short_name: Some("DTC_291104".into()),
					level: 3,
					temporary: false,
				},
			]
		);
		// The base variant's own layer is the pool's `#RtGen_DB_LAYER_DATA`,
		// which this fixture does not write when nothing inherits, so the base
		// variant declares nothing and says so with an empty list.
		let base = variants.iter().find(|v| v.name == "BV_Test").expect("the base variant is listed");
		assert!(project.faults(base).expect("an absent layer is not an error").is_empty());
	}

	#[test]
	fn a_variant_that_declares_no_fault_memory_inherits_its_base_variant_s() {
		// The same shape as the measurement chain's inheritance, and the same
		// assertion: the codes through the parent are the codes read directly.
		let direct = tempfile::tempdir().expect("a temporary directory");
		let inherited = tempfile::tempdir().expect("a temporary directory");
		let (a, _) = miniature_project(direct.path(), false);
		let (b, _) = miniature_project(inherited.path(), true);
		let faults = |p: &Project| {
			let variants = p.variants().expect("the project data parses");
			let variant = variants.iter().find(|v| v.name == "EV_Test").expect("the ECU variant is listed");
			let mut rows = p.faults(variant).expect("the fault chain walks");
			rows.sort_by_key(|f| f.code);
			rows
		};
		let through_parent = faults(&b);
		assert_eq!(through_parent.len(), 2, "{through_parent:#?}");
		assert_eq!(through_parent, faults(&a));
	}

	#[test]
	fn a_project_hands_over_its_names_keyed_by_text_id() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		let names = project.names().expect("the name pass runs");
		assert_eq!(names.get("000116").map(String::as_str), Some("Getriebe-Eingangsdrehzahl"));
		assert_eq!(names.get("000117").map(String::as_str), Some("Motordrehzahl"));
	}

	/// Write a `PRNR-INFO.xml` of the shape a real project ships.
	fn prnr_info(dir: &std::path::Path, vehicles: &[(&str, &str, &str, bool)]) {
		let mut out = String::from(
			"<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<PRNR-INFO VERSION=\"1.0.0\">\n  <REVISION-DATE>2026-06-04T14:38:10</REVISION-DATE>\n  <ODX-PLATFORM>TEST7X</ODX-PLATFORM>\n  <VEHICLES>\n",
		);
		for (project, name, code, default) in vehicles {
			out.push_str(&format!(
				"    <VEHICLE HAS-PRNR-DIFFERENTIATION=\"false\" IS-DEFAULT=\"{default}\">\n      <VEHICLE-PROJECT>{project}</VEHICLE-PROJECT>\n      <NAME>{name}</NAME>\n      <PRODUCT-ID>{code}</PRODUCT-ID>\n    </VEHICLE>\n"
			));
		}
		out.push_str("  </VEHICLES>\n</PRNR-INFO>\n");
		std::fs::write(dir.join("PRNR-INFO.xml"), out).expect("the fixture writes");
	}

	#[test]
	fn a_project_declares_the_vehicles_it_covers() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		prnr_info(
			dir.path(),
			&[
				("SK37X/0EU_X", "A7 / Octavia III (Limo, Combi)", "5E0", true),
				("SK326/0EU_K", "Karoq (EU) / A-SUV", "5EP", false),
			],
		);
		let vehicles = project.vehicles().expect("the coverage file parses");
		assert_eq!(
			vehicles,
			vec![
				Vehicle {
					type_code: "5E0".into(),
					name: "A7 / Octavia III (Limo, Combi)".into(),
					vehicle_project: "SK37X/0EU_X".into(),
					is_default: true
				},
				Vehicle {
					type_code: "5EP".into(),
					name: "Karoq (EU) / A-SUV".into(),
					vehicle_project: "SK326/0EU_K".into(),
					is_default: false
				},
			]
		);
	}

	#[test]
	fn covers_answers_for_one_type_code() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		prnr_info(dir.path(), &[("SK37X/0EU_X", "A7 / Octavia III (Limo, Combi)", "5E0", true)]);
		let hit = project.covers("5E0").expect("the coverage file parses").expect("the project covers 5E0");
		assert_eq!(hit.name, "A7 / Octavia III (Limo, Combi)");
		// Case is not a distinction a caller should have to know about.
		assert!(project.covers("5e0").expect("the coverage file parses").is_some());
		// A platform this project does not cover is None, not an error.
		assert_eq!(project.covers("8V0").expect("the coverage file parses"), None);
	}

	/// A project with no coverage file is a project that cannot say, and that is
	/// an error rather than an empty list — the same rule `readings` follows.
	/// An empty answer and an unanswerable question must not read alike.
	#[test]
	fn a_project_without_the_coverage_file_says_so() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		let err = project.vehicles().expect_err("a missing coverage file must be an error");
		assert!(matches!(err, Error::Missing(_)), "got {err:?}");
		assert!(matches!(project.covers("5E0"), Err(Error::Missing(_))));
	}

	/// The type code is the key, so an entry without one is not a vehicle this
	/// can select on, and dropping it silently would be worse than refusing it.
	#[test]
	fn a_vehicle_without_a_type_code_is_refused() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		std::fs::write(
			dir.path().join("PRNR-INFO.xml"),
			"<PRNR-INFO><VEHICLES>\n<VEHICLE IS-DEFAULT=\"true\">\n<VEHICLE-PROJECT>SK37X/0EU_X</VEHICLE-PROJECT>\n<NAME>A7</NAME>\n</VEHICLE>\n</VEHICLES></PRNR-INFO>",
		)
		.expect("the fixture writes");
		let err = project.vehicles().expect_err("a vehicle with no PRODUCT-ID must be refused");
		let Error::Format(message) = &err else { panic!("got {err:?}") };
		assert!(message.contains("PRODUCT-ID"), "the refusal must name the missing field; got {message:?}");
	}

	/// `<VEHICLES>` opens with the same five characters as `<VEHICLE`, and an
	/// entry may carry no attributes at all. Both are ways to lose every row.
	#[test]
	fn the_container_element_is_not_read_as_a_vehicle() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let (project, _) = miniature_project(dir.path(), false);
		std::fs::write(
			dir.path().join("PRNR-INFO.xml"),
			"<PRNR-INFO><VEHICLES><VEHICLE><VEHICLE-PROJECT>P</VEHICLE-PROJECT><NAME>Škoda &amp; co</NAME><PRODUCT-ID>5E0</PRODUCT-ID></VEHICLE></VEHICLES></PRNR-INFO>",
		)
		.expect("the fixture writes");
		let vehicles = project.vehicles().expect("the coverage file parses");
		assert_eq!(vehicles.len(), 1, "the container must not be counted as an entry");
		assert_eq!(vehicles[0].name, "Škoda & co", "the predefined entities are decoded");
		assert!(!vehicles[0].is_default, "an entry with no attribute is not the default");
	}

	#[test]
	fn a_directory_that_is_not_a_project_is_refused() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		let err = Project::open(dir.path()).expect_err("an empty directory is not a project");
		assert!(matches!(err, Error::Missing(_)), "got {err:?}");
	}

	#[test]
	fn the_directory_name_stands_in_for_a_missing_index() {
		let dir = tempfile::tempdir().expect("a temporary directory");
		assert_eq!(project_id(dir.path()), dir.path().file_name().and_then(|n| n.to_str()).expect("a name"));
		assert_eq!(project_version(dir.path()), None);
	}
}
