//! A read-only reader for a VW ODIS-Service runtime project.
//!
//! An extracted ODIS project is a directory of `<PoolID>.db` / `<PoolID>.key`
//! pairs plus two plaintext string pools. Nothing in it is encrypted
//! (`research/labels/odis-crib.md` §2); the three layers are a B+Tree index
//! ([`keyfile`], Peter Graf's PBL), zlib members ([`pool`]), and a positional
//! object stream ([`object`]) whose field order per type was reverse-engineered
//! by `ODIS-project-explorer` against a decompiled MCD kernel.
//!
//! ## Read-only, in two senses
//! Nothing here writes to a project — there is no insert, delete or split path
//! in the B+Tree at all. And nothing here parses an object type whose only
//! purpose is a write service: flashing, access keys, adaptation and coding
//! cases are refused by name in [`loaders::refused_type_name`], permanently.
//! See `SAFETY.md` and the design's §2.

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
/// Opening one reads the pools eagerly and nothing else: they are 88 MB
/// inflated and every name in the project resolves through them, so they are
/// paid for once. Pools are opened per call, because a project is 472 files
/// and no single question needs more than a handful of them.
#[derive(Debug)]
pub struct Project {
	dir: std::path::PathBuf,
	id: String,
	version: Option<String>,
	strings: strings::Strings,
	/// Every PoolID that has both a `.db` and a `.key`, sorted.
	pools: Vec<String>,
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
	/// (`research/labels/rod-labels.md:433`, established byte by byte against a
	/// log), and the ODIS file says the same — `is_high_low_byte_order` is
	/// false for it. A decoder that assumed big-endian would read 690 /min as
	/// 45570.
	pub big_endian: bool,
	/// How the raw value becomes an engineering one.
	pub scaling: crate::catalog::Scaling,
	/// The text id of [`Reading::name`] — the join to `TTTEXT`
	/// (`research/labels/odis-crib.md` §3).
	pub text_id: Option<String>,
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
	/// assumption from the name (`research/labels/odis-format.md`).
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
		let strings = strings::Strings::open(dir)?;
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
		Ok(Project {
			id: project_id(dir),
			version: project_version(dir),
			dir: dir.to_owned(),
			strings,
			pools,
		})
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
			let Some(files) = self.open_pool(pool_id)? else { continue };
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
		let mut store = Store::new(self);
		let Some(own) = store.layer_data(variant)? else {
			return Ok(Vec::new());
		};
		// The service may belong to the variant's *base* variant rather than to
		// the variant — see [`Store::measurement_layer`]. Everything below then
		// runs against that layer and its pool, because that is where the
		// service's own objects are indexed.
		let Some((layer, home)) = store.measurement_layer(variant, own)? else {
			return Ok(Vec::new());
		};
		let Some(service_ref) = layer
			.services
			.iter()
			.find(|(name, _)| name.as_deref() == Some(loaders::identity::RDBI_MEASUREMENT))
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
		let mut out = std::collections::BTreeMap::new();
		for pool_id in &self.pools {
			let Some(files) = self.open_pool(pool_id)? else { continue };
			for record in files.key.records()? {
				let Ok(locator) = pool::Locator::parse(&record.data) else { continue };
				let Ok(bytes) = files.db.member(&locator) else { continue };
				let Ok((type_code, mut stream)) = object::Stream::open(&bytes, &self.strings) else {
					continue;
				};
				if let Ok(loaders::Outcome::Object(object)) = loaders::load(type_code, &mut stream) {
					harvest(&object, &mut out);
				}
			}
		}
		Ok(out)
	}

	/// Open a pool's `.db`/`.key` pair, or `None` if either is missing.
	fn open_pool(&self, pool_id: &str) -> Result<Option<PoolFiles>, Error> {
		let key_path = self.dir.join(format!("{pool_id}.key"));
		let db_path = self.dir.join(format!("{pool_id}.db"));
		if !key_path.is_file() || !db_path.is_file() {
			return Ok(None);
		}
		Ok(Some(PoolFiles {
			key: keyfile::KeyFile::open(&key_path)?,
			db: pool::Pool::open(&db_path)?,
		}))
	}
}

/// A pool's two files, opened together.
#[derive(Debug)]
struct PoolFiles {
	key: keyfile::KeyFile,
	db: pool::Pool,
}

impl PoolFiles {
	/// The inflated bytes of one named object, or `None` if this pool has no
	/// such name.
	fn object(&self, strings: &strings::Strings, object_id: &str) -> Result<Option<Vec<u8>>, Error> {
		let Some(hash) = strings.ascii.hash_of(object_id) else { return Ok(None) };
		// A `.key` key is the hash's four bytes, little-endian.
		let Some(data) = self.key.find(&hash.to_le_bytes())? else {
			return Ok(None);
		};
		Ok(Some(self.db.member(&pool::Locator::parse(&data)?)?))
	}
}

/// A pool cache for one question, plus the reference resolution it needs.
///
/// Held for the length of a single [`Project::readings`] call rather than on
/// the project: the chain touches a handful of pools but touches them many
/// times, and reopening a megabyte per data object property would dominate.
struct Store<'a> {
	project: &'a Project,
	open: std::collections::HashMap<String, Option<PoolFiles>>,
	/// Layer data of the parent pools, resolved on first need. A reference
	/// that omits its pool is looked up here.
	inherited: Vec<loaders::identity::LayerData>,
}

impl<'a> Store<'a> {
	fn new(project: &'a Project) -> Store<'a> {
		Store {
			project,
			open: std::collections::HashMap::new(),
			inherited: Vec::new(),
		}
	}

	/// A pool, opened at most once per store.
	fn pool(&mut self, pool_id: &str) -> Result<Option<&PoolFiles>, Error> {
		if !self.open.contains_key(pool_id) {
			let opened = self.project.open_pool(pool_id)?;
			self.open.insert(pool_id.to_owned(), opened);
		}
		Ok(self.open.get(pool_id).and_then(Option::as_ref))
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
		let inherited: Vec<(Option<String>, loaders::Ref)> = self
			.inherited
			.iter()
			.flat_map(|l| l.properties.iter().chain(&l.tables).cloned())
			.collect();
		for (name, indexed) in inherited {
			if name.as_deref() == Some(object_id.as_str())
				&& let Some(pool_id) = indexed.pool
			{
				return self.named(&pool_id, &object_id);
			}
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
	fn measurement_layer(
		&mut self,
		variant: &Variant,
		own: loaders::identity::LayerData,
	) -> Result<Option<(loaders::identity::LayerData, String)>, Error> {
		let declares = |layer: &loaders::identity::LayerData| {
			layer
				.services
				.iter()
				.any(|(name, _)| name.as_deref() == Some(loaders::identity::RDBI_MEASUREMENT))
		};
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
/// one `research/labels/odis-crib.md` §3 turns into `TTTEXT` plaintext.
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
		loaders::Object::Service(_)
		| loaders::Object::Table(_)
		| loaders::Object::ProjectData(_)
		| loaders::Object::LayerData(_)
		| loaders::Object::EcuVariant(_) => {}
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
			let mut db = Vec::new();
			let mut records: Vec<(u32, Vec<u8>)> = Vec::new();
			for (object_id, body) in &self.objects {
				let member = miniz_oxide::deflate::compress_to_vec_zlib(body, 6);
				let mut locator = (db.len() as u32).to_le_bytes().to_vec();
				locator.extend_from_slice(&(member.len() as u32).to_le_bytes());
				locator.extend_from_slice(&(body.len() as u32).to_le_bytes());
				db.extend_from_slice(&member);
				records.push((hash::of_bytes(object_id.as_bytes()), locator));
			}
			std::fs::write(dir.join(format!("{pool_id}.db")), &db).expect("the fixture writes");
			std::fs::write(dir.join(format!("{pool_id}.key")), leaf_block(&records)).expect("the fixture writes");

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

	/// Build the whole fixture: one pool, one variant, two readings.
	///
	/// `inherit` selects which of the two shapes the real files use. `false`
	/// puts the measurement service on the ECU variant's own layer. `true` gives
	/// that layer **no** services and one parent, and puts the service on the
	/// pool's base-variant layer instead — which is what
	/// `EV_DCUDriveSideEWMAXCONT_006` looks like on disk. Both must yield the
	/// same channels; that is the whole assertion.
	fn miniature_project(dir: &std::path::Path, inherit: bool) -> (Project, String) {
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
		let rdbi = b.a(loaders::identity::RDBI_MEASUREMENT);
		let svc = b.a("SVC_Measu");
		// One layer, built twice with different contents when the fixture is
		// exercising inheritance: the service sits on whichever layer is
		// supposed to own it, and the other names a parent instead.
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
			o = o
				.u16(0) // no DTC properties
				.u16(0)
				.u16(0) // no property or table index
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

		b.write(dir, pool_id);
		std::fs::write(dir.join("index.xml"), "<CATALOG><SHORT-NAME>TEST7X</SHORT-NAME></CATALOG>").expect("the fixture writes");
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
