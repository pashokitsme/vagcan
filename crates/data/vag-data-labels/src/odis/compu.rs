//! Computation methods — how a coded value out of a response becomes an
//! engineering one — and their translation into this crate's [`Scaling`].
//!
//! This is the module the whole ODIS effort is for. `research/labels/rod-labels.md`
//! §4.0c established that Ross-Tech's label files provably cannot supply a
//! measurement scaling, so every proven row in `vag_data_labels::catalog` had to be
//! measured by driving the car. A `COMPU-METHOD` is that scaling, written down.
//! The design's §1 records the cross-check that made it worth trusting: DID
//! `0x380A` on the reference engine comes back `IDENTICAL` — raw `u16` — which
//! is what driving proved, from the other direction and years earlier.
//!
//! ## What is translated, and what is refused
//! ODX defines eight categories. Three map onto a [`Scaling`] this crate can
//! honestly represent:
//!
//! | category | becomes |
//! |---|---|
//! | `IDENTICAL` | `Linear { factor: 1, offset: 0 }` |
//! | `LINEAR` | `Linear` from the single scale's rational coefficients |
//! | `TEXTTAB` | `Enum`, one level per scale |
//!
//! The other five (`SCALE-LINEAR`, `COMPUCODE`, `TAB-INTP`, `RAT-FUNC`,
//! `SCALE-RAT-FUNC`) are piecewise, interpolated, polynomial or externally
//! coded, and [`Scaling`] has no shape for any of them. They are an error that
//! **names the category**, never a silent factor of 1 — a channel reported with
//! the wrong slope is worse than a channel not reported at all, and
//! `research/labels/scaling-audit.md` §4 is the record of what happens when a
//! plausible-looking scaling is trusted without proof.

use crate::catalog::Scaling;
use crate::measure::LinearScale;

use super::Error;
use super::loaders::code;
use super::object::{Stream, Value};

/// ODX's `COMPU-CATEGORY`, as the kernel numbers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
	/// The coded value *is* the physical value.
	Identical,
	/// One rational scale over the whole domain.
	Linear,
	/// A rational scale per interval.
	ScaleLinear,
	/// A lookup from coded value to text.
	TextTable,
	/// An externally supplied code module.
	CompuCode,
	/// A table of points with linear interpolation between them.
	TabIntp,
	/// A rational function.
	RatFunc,
	/// A rational function per interval.
	ScaleRatFunc,
}

impl Category {
	/// Read the one-byte category enum.
	fn read(stream: &mut Stream<'_>) -> Result<Category, Error> {
		Ok(match stream.u8()? {
			0 => Category::Identical,
			1 => Category::Linear,
			2 => Category::ScaleLinear,
			3 => Category::TextTable,
			4 => Category::CompuCode,
			5 => Category::TabIntp,
			6 => Category::RatFunc,
			7 => Category::ScaleRatFunc,
			other => return Err(Error::Format(format!("compu category {other} is not one of the eight ODX defines"))),
		})
	}

	/// The ODX spelling, for messages. A refusal has to be able to say which
	/// category it refused, or it tells nobody what to do next.
	pub fn name(self) -> &'static str {
		match self {
			Category::Identical => "IDENTICAL",
			Category::Linear => "LINEAR",
			Category::ScaleLinear => "SCALE-LINEAR",
			Category::TextTable => "TEXTTABLE",
			Category::CompuCode => "COMPUCODE",
			Category::TabIntp => "TAB-INTP",
			Category::RatFunc => "RAT-FUNC",
			Category::ScaleRatFunc => "SCALE-RAT-FUNC",
		}
	}
}

/// Which end of a range a limit is, and whether the end itself is included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
	/// The bound is excluded.
	Open,
	/// The bound is included.
	Closed,
	/// There is no bound in this direction.
	Infinite,
}

/// One end of a value range.
#[derive(Debug, Clone, PartialEq)]
pub struct Limit {
	/// The bound itself; absent for [`LimitKind::Infinite`].
	pub value: Option<Value>,
	/// How the bound behaves.
	pub kind: LimitKind,
}

/// `COMPU-RATIONAL-COEFFS`: the two polynomials of a rational scale.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Coeffs {
	/// `COMPU-NUMERATOR`, ascending powers. For a `LINEAR` method this is
	/// `[offset, factor]`.
	pub numerator: Vec<f64>,
	/// `COMPU-DENOMINATOR`. For a `LINEAR` method it is empty or one divisor.
	pub denominator: Vec<f64>,
}

/// One `COMPU-SCALE`: a rule over one interval of the coded domain.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scale {
	/// The text id of this scale's label, if it has one. For a `TEXTTAB` this
	/// is the id of the *text*, which is what makes an ODIS project readable
	/// against `TTTEXT` (`research/labels/odis-crib.md` §3).
	pub label_id: Option<String>,
	/// The rational coefficients, on the coded → physical direction.
	pub coeffs: Option<Coeffs>,
	/// The physical lower bound.
	pub lower: Option<Limit>,
	/// The physical upper bound.
	pub upper: Option<Limit>,
	/// `COMPU-CONST` — the value the whole interval maps to. A `TEXTTAB`'s text.
	pub constant: Option<Value>,
	/// The coded lower bound. For a `TEXTTAB` this is the raw value a level
	/// answers to; the physical bounds carry the text, not a number.
	pub lower_coded: Option<Limit>,
	/// The coded upper bound.
	pub upper_coded: Option<Limit>,
}

/// `COMPU-INTERNAL-TO-PHYS` or `COMPU-PHYS-TO-INTERNAL`: one direction.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Base {
	/// The intervals, in file order.
	pub scales: Vec<Scale>,
	/// What a coded value outside every interval maps to.
	pub default: Option<Value>,
}

/// A whole `COMPU-METHOD`.
#[derive(Debug, Clone, PartialEq)]
pub struct Method {
	/// Which of the eight rules applies.
	pub category: Category,
	/// The coded → physical direction. Absent for `IDENTICAL`, which needs no
	/// rule; present for everything else.
	pub internal_to_phys: Option<Base>,
}

/// Read a `DB_LIMIT`.
pub fn limit(stream: &mut Stream<'_>) -> Result<Limit, Error> {
	let value = stream.value()?;
	// The kind is stored as the low byte of MCDLimitType (0x6D01..0x6D03).
	let kind = match stream.u8()? {
		1 => LimitKind::Open,
		2 => LimitKind::Closed,
		3 => LimitKind::Infinite,
		other => return Err(Error::Format(format!("limit type {other} is not one of OPEN, CLOSED or INFINITE"))),
	};
	Ok(Limit { value, kind })
}

/// Read a `DB_COMPU_RATIONAL_COEFFS`.
pub fn coeffs(stream: &mut Stream<'_>) -> Result<Coeffs, Error> {
	let read = |stream: &mut Stream<'_>| -> Result<Vec<f64>, Error> {
		let count = usize::from(stream.u8()?);
		(0..count).map(|_| stream.f64()).collect()
	};
	let numerator = read(stream)?;
	let denominator = read(stream)?;
	Ok(Coeffs { numerator, denominator })
}

/// Read a `DB_COMPU_SCALE`.
pub fn scale(stream: &mut Stream<'_>) -> Result<Scale, Error> {
	let label_id = stream.ascii()?.map(str::to_owned);
	// The inverse coefficients come first and are the physical → coded
	// direction, which nothing here needs; they are read to keep the cursor
	// aligned, not to be used.
	let _inverse = super::loaders::nested(stream, code::DB_COMPU_RATIONAL_COEFFS, coeffs)?;
	let coefficients = super::loaders::nested(stream, code::DB_COMPU_RATIONAL_COEFFS, coeffs)?;
	let lower = super::loaders::nested(stream, code::DB_LIMIT, limit)?;
	let upper = super::loaders::nested(stream, code::DB_LIMIT, limit)?;
	let constant = stream.value()?;
	let _inverse_value = stream.value()?;
	let _constant_as_coded = stream.value()?;
	let lower_coded = super::loaders::nested(stream, code::DB_LIMIT, limit)?;
	let upper_coded = super::loaders::nested(stream, code::DB_LIMIT, limit)?;
	Ok(Scale {
		label_id,
		coeffs: coefficients,
		lower,
		upper,
		constant,
		lower_coded,
		upper_coded,
	})
}

/// Read a `DB_COMPU_SCALES` collection.
pub fn scales(stream: &mut Stream<'_>) -> Result<Vec<Scale>, Error> {
	let count = stream.count32()?;
	let mut out = Vec::with_capacity(count.min(1024));
	for _ in 0..count {
		let Some(one) = super::loaders::nested(stream, code::DB_COMPU_SCALE, scale)? else {
			return Err(Error::Format("a compu scales collection holds an absent scale".into()));
		};
		out.push(one);
	}
	Ok(out)
}

/// Read a `DB_COMPU_BASE`.
pub fn base(stream: &mut Stream<'_>) -> Result<Base, Error> {
	let scales = super::loaders::nested(stream, code::DB_COMPU_SCALES, scales)?.unwrap_or_default();
	let default = stream.value()?;
	let _code_byte_stream = stream.value()?;
	// `code_information` names an external code module — `MCD_DB_CODE_INFORMATION`
	// is on the permanent refusal list, so its presence stops the parse here
	// rather than being read past.
	if stream.flag()? {
		return Err(Error::Refused("MCD_DB_CODE_INFORMATION"));
	}
	let _inverse_value = stream.value()?;
	Ok(Base { scales, default })
}

/// Read a `DB_COMPU_METHOD`.
pub fn method(stream: &mut Stream<'_>) -> Result<Method, Error> {
	let category = Category::read(stream)?;
	// Physical → coded first, then coded → physical. Only the second is used.
	let _phys_to_internal = super::loaders::nested(stream, code::DB_COMPU_BASE, base)?;
	let internal_to_phys = super::loaders::nested(stream, code::DB_COMPU_BASE, base)?;
	// A TEXTTABLE with an inverse or a default value carries the text id of
	// each; they are ids, not values, and nothing here needs them.
	if category == Category::TextTable {
		if _phys_to_internal.is_some() {
			let _inverse_id = stream.ascii()?;
		}
		if internal_to_phys.as_ref().is_some_and(|b| b.default.is_some()) {
			let _default_id = stream.ascii()?;
		}
	}
	Ok(Method { category, internal_to_phys })
}

impl Method {
	/// Translate into a [`Scaling`], or say why not.
	///
	/// An unsupported category is an error naming it. That is the whole
	/// contract: no category ever silently becomes a factor of 1.
	pub fn scaling(&self) -> Result<Scaling, Error> {
		match self.category {
			Category::Identical => Ok(Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 })),
			Category::Linear => self.linear(),
			Category::TextTable => self.text_table(),
			other => Err(Error::Format(format!(
				"compu category {} has no scaling this crate can represent honestly",
				other.name()
			))),
		}
	}

	/// `LINEAR`: exactly one scale, `physical = (VN0 + VN1 * coded) / VD0`.
	///
	/// The numerator's terms are in ascending powers, so `[offset, factor]`.
	/// A shorter numerator is legal and means the missing terms default —
	/// no numerator at all is the identity, one term is a bare offset.
	fn linear(&self) -> Result<Scaling, Error> {
		let base = self
			.internal_to_phys
			.as_ref()
			.ok_or_else(|| Error::Format("a LINEAR compu method has no coded-to-physical direction".into()))?;
		let [scale] = base.scales.as_slice() else {
			return Err(Error::Format(format!(
				"a LINEAR compu method has {} scales; ODX allows exactly one",
				base.scales.len()
			)));
		};
		let coeffs = scale.coeffs.clone().unwrap_or_default();
		let (offset, factor) = match coeffs.numerator.as_slice() {
			[] => (0.0, 1.0),
			[offset] => (*offset, 1.0),
			[offset, factor] => (*offset, *factor),
			more => {
				return Err(Error::Format(format!(
					"a LINEAR compu method's numerator has {} terms; ODX allows at most two",
					more.len()
				)));
			}
		};
		let divisor = match coeffs.denominator.as_slice() {
			[] => 1.0,
			[divisor] => *divisor,
			more => {
				return Err(Error::Format(format!(
					"a LINEAR compu method's denominator has {} terms; ODX allows at most one",
					more.len()
				)));
			}
		};
		if divisor == 0.0 {
			return Err(Error::Format("a LINEAR compu method divides by zero".into()));
		}
		Ok(Scaling::Linear(LinearScale {
			factor: factor / divisor,
			offset: offset / divisor,
		}))
	}

	/// `TEXTTAB`: one level per scale, keyed by the scale's **coded** lower
	/// bound and named by its `COMPU-CONST`.
	///
	/// The physical bounds of a text table hold the same text as the constant,
	/// not a number — reading the level's key off them instead of off the coded
	/// bound is the mistake this comment exists to stop.
	fn text_table(&self) -> Result<Scaling, Error> {
		let base = self
			.internal_to_phys
			.as_ref()
			.ok_or_else(|| Error::Format("a TEXTTABLE compu method has no coded-to-physical direction".into()))?;
		let mut levels = Vec::with_capacity(base.scales.len());
		for scale in &base.scales {
			let Some(raw) = scale.lower_coded.as_ref().and_then(|l| l.value.as_ref()).and_then(as_i32) else {
				// A level whose key is not an integer is not a state this
				// crate can match a raw reading against, so it is dropped
				// rather than guessed at.
				continue;
			};
			let Some(name) = scale.constant.as_ref().and_then(as_text) else {
				continue;
			};
			levels.push((raw, name));
		}
		if levels.is_empty() {
			return Err(Error::Format(
				"a TEXTTABLE compu method has no level with both an integer key and a text".into(),
			));
		}
		Ok(Scaling::Enum { levels })
	}
}

/// A value as an `i32`, when it is one.
fn as_i32(value: &Value) -> Option<i32> {
	match value {
		Value::I32(v) => Some(*v),
		Value::U32(v) => i32::try_from(*v).ok(),
		_ => None,
	}
}

/// A value as text, when it is a string.
fn as_text(value: &Value) -> Option<String> {
	match value {
		Value::Unicode(Some(s)) | Value::Ascii(Some(s)) => Some(s.clone()),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn linear_method(numerator: &[f64], denominator: &[f64]) -> Method {
		Method {
			category: Category::Linear,
			internal_to_phys: Some(Base {
				scales: vec![Scale {
					coeffs: Some(Coeffs {
						numerator: numerator.to_vec(),
						denominator: denominator.to_vec(),
					}),
					..Scale::default()
				}],
				default: None,
			}),
		}
	}

	#[test]
	fn identical_is_a_raw_reading() {
		// The design's §1 cross-check: DID 0x380A on the reference engine is
		// IDENTICAL, and driving proved the same channel is raw u16.
		let method = Method {
			category: Category::Identical,
			internal_to_phys: None,
		};
		assert_eq!(
			method.scaling().expect("IDENTICAL always translates"),
			Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 })
		);
	}

	#[test]
	fn linear_reads_its_rational_coefficients() {
		// (0 + 1 * x) / 20 — one of the slopes this project proved by driving.
		assert_eq!(
			linear_method(&[0.0, 1.0], &[20.0]).scaling().expect("LINEAR translates"),
			Scaling::Linear(LinearScale { factor: 0.05, offset: 0.0 })
		);
		// (-40 + 1 * x) / 1 — the shape of a temperature channel.
		assert_eq!(
			linear_method(&[-40.0, 1.0], &[]).scaling().expect("LINEAR translates"),
			Scaling::Linear(LinearScale { factor: 1.0, offset: -40.0 })
		);
		// An offset alone: the factor defaults to 1.
		assert_eq!(
			linear_method(&[7.0], &[]).scaling().expect("LINEAR translates"),
			Scaling::Linear(LinearScale { factor: 1.0, offset: 7.0 })
		);
		// No coefficients at all: the identity.
		assert_eq!(
			linear_method(&[], &[]).scaling().expect("LINEAR translates"),
			Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 })
		);
	}

	#[test]
	fn a_linear_method_that_divides_by_zero_is_refused() {
		let err = linear_method(&[0.0, 1.0], &[0.0]).scaling().expect_err("a zero divisor must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_linear_method_with_more_than_one_scale_is_refused() {
		let mut method = linear_method(&[0.0, 1.0], &[]);
		let base = method.internal_to_phys.as_mut().expect("the fixture has a direction");
		base.scales.push(Scale::default());
		let err = method.scaling().expect_err("LINEAR allows exactly one scale");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_text_table_becomes_an_enum_keyed_on_the_coded_bound() {
		let level = |raw: i32, text: &str| Scale {
			lower_coded: Some(Limit {
				value: Some(Value::U32(raw as u32)),
				kind: LimitKind::Closed,
			}),
			upper_coded: Some(Limit {
				value: Some(Value::U32(raw as u32)),
				kind: LimitKind::Closed,
			}),
			// The physical bounds carry the text, not a number — a level read
			// off these instead of off the coded bound has no key at all.
			lower: Some(Limit {
				value: Some(Value::Unicode(Some(text.into()))),
				kind: LimitKind::Closed,
			}),
			constant: Some(Value::Unicode(Some(text.into()))),
			..Scale::default()
		};
		let method = Method {
			category: Category::TextTable,
			internal_to_phys: Some(Base {
				scales: vec![level(0, "nicht aktiv"), level(1, "aktiv")],
				default: None,
			}),
		};
		assert_eq!(
			method.scaling().expect("TEXTTABLE translates"),
			Scaling::Enum {
				levels: vec![(0, "nicht aktiv".into()), (1, "aktiv".into())]
			}
		);
	}

	#[test]
	fn an_unsupported_category_names_itself_and_never_becomes_a_factor_of_one() {
		for category in [
			Category::ScaleLinear,
			Category::CompuCode,
			Category::TabIntp,
			Category::RatFunc,
			Category::ScaleRatFunc,
		] {
			let method = Method {
				category,
				internal_to_phys: Some(Base::default()),
			};
			let err = method.scaling().expect_err("an unsupported category must be refused");
			let Error::Format(message) = &err else { panic!("got {err:?}") };
			assert!(message.contains(category.name()), "the refusal must name the category; got {message:?}");
		}
	}
}
