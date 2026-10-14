//! [`Filter`]: the filter tree a query evaluates, as the caller writes it.

use std::ops::Bound;

use golemdb_cells::{CellName, CellValue};

use crate::{QueryError, Result};

/// A query's filter: plain data the caller builds, validated at admission.
///
/// Leaves are typed by their [`CellValue`], the same values records are
/// written with: a cell of another type under the same name is treated as
/// absent. The value's kind (field or attribute) plays no part in a lookup.
/// Variable-length `bytes` has no index term and is rejected at admission.
///
/// An [`In`](Filter::In)'s values and a [`Range`](Filter::Range)'s bounds
/// are single-typed by construction: build them through [`Filter::in_of`]
/// and [`Filter::range_of`], whose `T` fixes the type at compile time, or,
/// when the values only exist as `CellValue`s at runtime, through the
/// fallible [`Filter::try_in`] and [`Filter::try_range`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    /// The cell holds exactly `value`, with its type.
    Eq { field: CellName, value: CellValue },
    /// The cell holds one of `values`. The values are of one type, so a
    /// value given twice is read once, and no value matches nothing.
    In { field: CellName, values: InValues },
    /// The cell's value lies between the range's bounds, in its type's
    /// order: numeric for numbers and dates, false before true, byte order
    /// for strings (UTF-8) and fixed bytes. At least one side is bounded.
    Range {
        field: CellName,
        bounds: RangeBounds,
    },
    // Prefix { field: CellName, prefix: CellValue },           // str, bytesN
    // Exists { field: CellName, ty: CellType },
    // And(Vec<Filter>),
    // Or(Vec<Filter>),
    // Not(Box<Filter>),
}

impl Filter {
    /// The cell holds one of `values`, all of one type: `T` fixes the type
    /// at compile time, so a mixed list does not build.
    pub fn in_of<T: IntoCellValue>(field: CellName, values: impl IntoIterator<Item = T>) -> Self {
        Self::In {
            field,
            values: InValues {
                values: values
                    .into_iter()
                    .map(IntoCellValue::into_cell_value)
                    .collect(),
            },
        }
    }

    /// The cell's value lies between `lower` and `upper`, both of one type:
    /// `T` fixes the type at compile time, so bounds of two types do not
    /// build.
    pub fn range_of<T: IntoCellValue>(field: CellName, lower: Bound<T>, upper: Bound<T>) -> Self {
        Self::Range {
            field,
            bounds: RangeBounds {
                lower: lower.map(IntoCellValue::into_cell_value),
                upper: upper.map(IntoCellValue::into_cell_value),
            },
        }
    }

    /// The runtime-checked [`In`](Filter::In): for callers whose values
    /// only exist as `CellValue`s at runtime, such as deserialized queries.
    /// Fails with [`QueryError::InvalidQuery`] when the values differ in
    /// type.
    pub fn try_in(field: CellName, values: Vec<CellValue>) -> Result<Self> {
        Ok(Self::In {
            field,
            values: InValues::try_from(values)?,
        })
    }

    /// The runtime-checked [`Range`](Filter::Range): for callers whose
    /// bounds only exist as `CellValue`s at runtime. Fails with
    /// [`QueryError::InvalidQuery`] when the bounds differ in type.
    pub fn try_range(
        field: CellName,
        lower: Bound<CellValue>,
        upper: Bound<CellValue>,
    ) -> Result<Self> {
        Ok(Self::Range {
            field,
            bounds: RangeBounds::try_from_bounds(lower, upper)?,
        })
    }
}

/// The values of an [`In`](Filter::In): one type's, in the order given.
///
/// Built single-typed through [`Filter::in_of`], or checked at runtime
/// through [`Filter::try_in`] / [`TryFrom`]. The field is private, so a
/// mixed list cannot be written in place of a valid one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InValues {
    values: Vec<CellValue>,
}

impl InValues {
    /// The values, in the order given.
    pub fn iter(&self) -> std::slice::Iter<'_, CellValue> {
        self.values.iter()
    }

    /// The values, in the order given.
    pub fn as_slice(&self) -> &[CellValue] {
        &self.values
    }
}

impl TryFrom<Vec<CellValue>> for InValues {
    type Error = QueryError;

    /// The values, in the order given, checked to be of one type. This is
    /// the runtime half of the single-type rule, for values that arrive as
    /// `CellValue`s; `Filter::in_of` needs no check.
    fn try_from(values: Vec<CellValue>) -> Result<Self> {
        if let Some(first) = values.first()
            && values
                .iter()
                .any(|value| value.cell_type() != first.cell_type())
        {
            return Err(QueryError::InvalidQuery {
                reason: "`In` values must all have one type",
            });
        }
        Ok(Self { values })
    }
}

/// The bounds of a [`Range`](Filter::Range): one type's, in the order
/// given.
///
/// Built single-typed through [`Filter::range_of`], or checked at runtime
/// through [`Filter::try_range`]. The field is private, so bounds of two
/// types cannot be written in place of valid ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeBounds {
    lower: Bound<CellValue>,
    upper: Bound<CellValue>,
}

impl RangeBounds {
    /// The lower bound, of the range's one type.
    pub fn lower(&self) -> &Bound<CellValue> {
        &self.lower
    }

    /// The upper bound, of the range's one type.
    pub fn upper(&self) -> &Bound<CellValue> {
        &self.upper
    }

    /// The bounds, in the order given, checked to be of one type. This is
    /// the runtime half of the single-type rule, for bounds that arrive as
    /// `CellValue`s; `Filter::range_of` needs no check.
    fn try_from_bounds(lower: Bound<CellValue>, upper: Bound<CellValue>) -> Result<Self> {
        let type_of = |bound: &Bound<CellValue>| match bound {
            Bound::Included(value) | Bound::Excluded(value) => Some(value.cell_type()),
            Bound::Unbounded => None,
        };
        if let (Some(lower), Some(upper)) = (type_of(&lower), type_of(&upper))
            && lower != upper
        {
            return Err(QueryError::InvalidQuery {
                reason: "`Range` bounds must have one type",
            });
        }
        Ok(Self { lower, upper })
    }
}

/// A value an `In` or `Range` holds, built into a [`CellValue`] without a
/// check. Only types that map to one cell type and one value are `Sealed`
/// into the trait; the rest — the `i32`/`dec32`/`date32` family, the
/// `bytes32`/`u256_be`/`i256_be` family, the floats, which reject NaN —
/// build their `CellValue`s with the named `from_*` constructors and go
/// through the runtime-checked [`Filter::try_in`] /
/// [`Filter::try_range`].
pub trait IntoCellValue: private::Sealed {
    /// The value as a [`CellValue`], of its one cell type.
    fn into_cell_value(self) -> CellValue;
}

impl IntoCellValue for bool {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_bool(self)
    }
}

impl IntoCellValue for u32 {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_u32(self)
    }
}

impl IntoCellValue for u64 {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_u64(self)
    }
}

impl IntoCellValue for u128 {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_u128(self)
    }
}

impl IntoCellValue for &str {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_str(self)
    }
}

impl IntoCellValue for String {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_str(&self)
    }
}

impl IntoCellValue for [u8; 4] {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_bytes4(self)
    }
}

impl IntoCellValue for [u8; 8] {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_bytes8(self)
    }
}

impl IntoCellValue for [u8; 16] {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_bytes16(self)
    }
}

impl IntoCellValue for [u8; 20] {
    fn into_cell_value(self) -> CellValue {
        CellValue::from_bytes20(self)
    }
}

mod private {
    /// The types [`IntoCellValue`](super::IntoCellValue) is implemented for,
    /// and no others: an implementation free to pick a cell type per value
    /// would break the single-type rule the trait stands for.
    pub trait Sealed {}
    impl Sealed for bool {}
    impl Sealed for u32 {}
    impl Sealed for u64 {}
    impl Sealed for u128 {}
    impl Sealed for &str {}
    impl Sealed for String {}
    impl Sealed for [u8; 4] {}
    impl Sealed for [u8; 8] {}
    impl Sealed for [u8; 16] {}
    impl Sealed for [u8; 20] {}
}

#[cfg(test)]
mod tests {
    use golemdb_cells::{CellNameRef, CellValue};

    use super::*;

    fn name(field: &str) -> CellName {
        CellNameRef::raw(field.as_bytes()).into()
    }

    #[test]
    fn typed_in_values_share_their_t() {
        let filter = Filter::in_of(name("color"), ["red", "pink", "red"]);
        let Filter::In { values, .. } = &filter else {
            panic!("an In");
        };
        // One type, and the order given is kept: deduplication is the
        // plan's, not the constructor's.
        assert_eq!(
            values
                .iter()
                .map(|value| value.as_str().expect("one type: str"))
                .collect::<Vec<_>>(),
            ["red", "pink", "red"]
        );
    }

    #[test]
    fn typed_range_bounds_share_their_t() {
        let filter = Filter::range_of(name("n"), Bound::Included(0u32), Bound::Excluded(10u32));
        let Filter::Range { bounds, .. } = &filter else {
            panic!("a Range");
        };
        assert_eq!(bounds.lower(), &Bound::Included(CellValue::from_u32(0)));
        assert_eq!(bounds.upper(), &Bound::Excluded(CellValue::from_u32(10)));

        let unbounded = Filter::range_of::<u32>(name("n"), Bound::Unbounded, Bound::Unbounded);
        let Filter::Range { bounds, .. } = &unbounded else {
            panic!("a Range");
        };
        assert_eq!(bounds.lower(), &Bound::Unbounded);
        assert_eq!(bounds.upper(), &Bound::Unbounded);
    }

    #[test]
    fn a_mixed_in_is_an_invalid_query_at_construction() {
        // The runtime half of the rule, for values that arrive as
        // `CellValue`s: a mixed list does not become a filter.
        for values in [
            vec![CellValue::from_i32(1), CellValue::from_i64(1)],
            vec![
                CellValue::from_u32(1),
                CellValue::from_u32(2),
                CellValue::from_dec32_unscaled(3),
            ],
            vec![CellValue::from_str("x"), CellValue::from_bytes(b"x")],
        ] {
            assert!(
                matches!(
                    Filter::try_in(name("f"), values.clone()),
                    Err(QueryError::InvalidQuery { .. })
                ),
                "{values:?}"
            );
        }
    }

    #[test]
    fn mixed_range_bounds_are_an_invalid_query_at_construction() {
        let (i32_bound, i64_bound) = (
            Bound::Included(CellValue::from_i32(1)),
            Bound::Included(CellValue::from_i64(2)),
        );
        assert!(matches!(
            Filter::try_range(name("f"), i32_bound.clone(), i64_bound),
            Err(QueryError::InvalidQuery { .. })
        ));
        // A bound on one side only, and so of one type, builds.
        assert!(Filter::try_range(name("f"), i32_bound, Bound::Unbounded).is_ok());
    }

    #[test]
    fn an_empty_in_builds() {
        // No value matches nothing, so an empty list is valid; the plan
        // still checks the field's name.
        assert!(Filter::try_in(name("f"), vec![]).is_ok());
    }
}
