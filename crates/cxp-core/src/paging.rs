//! Bounded list paging. Every list query takes a [`Page`], so nothing a
//! caller sends can ask the shared database for an unbounded result.

/// A `LIMIT`/`OFFSET` pair, already clamped: `limit` is in
/// `1..=`[`Page::MAX_LIMIT`] and `offset` in `0..=`[`Page::MAX_OFFSET`].
///
/// The fields are private so an unclamped page cannot be built; read them
/// with [`Page::limit`] and [`Page::offset`] and bind them as `i64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    limit: i64,
    offset: i64,
}

impl Page {
    /// The limit when the caller gives none.
    pub const DEFAULT_LIMIT: i64 = 50;
    /// The largest limit any list honours; larger requests are cut to it.
    pub const MAX_LIMIT: i64 = 200;
    /// The deepest offset any list honours; deeper requests are cut to it.
    /// `OFFSET n` reads and discards n rows, so an unbounded offset is an
    /// unbounded scan; past this, narrow the query with a filter instead.
    pub const MAX_OFFSET: i64 = 10_000;

    /// Clamp a caller's optional `limit` and `offset`: a missing limit is
    /// [`Page::DEFAULT_LIMIT`], one below 1 is 1, one above
    /// [`Page::MAX_LIMIT`] is the maximum; a missing or negative offset is 0,
    /// and one above [`Page::MAX_OFFSET`] is the maximum.
    pub fn new(limit: Option<i64>, offset: Option<i64>) -> Self {
        Self {
            limit: limit
                .unwrap_or(Self::DEFAULT_LIMIT)
                .clamp(1, Self::MAX_LIMIT),
            offset: offset.unwrap_or(0).clamp(0, Self::MAX_OFFSET),
        }
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }
}

impl Default for Page {
    fn default() -> Self {
        Self::new(None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::Page;

    #[test]
    fn defaults_to_fifty_from_the_start() {
        let p = Page::default();
        assert_eq!((p.limit(), p.offset()), (50, 0));
        assert_eq!(Page::new(None, None), p);
    }

    #[test]
    fn limit_is_clamped_to_one_through_two_hundred() {
        assert_eq!(Page::new(Some(0), None).limit(), 1);
        assert_eq!(Page::new(Some(-5), None).limit(), 1);
        assert_eq!(Page::new(Some(1), None).limit(), 1);
        assert_eq!(Page::new(Some(200), None).limit(), 200);
        assert_eq!(Page::new(Some(201), None).limit(), 200);
        assert_eq!(Page::new(Some(i64::MAX), None).limit(), 200);
        assert_eq!(Page::new(Some(75), None).limit(), 75);
    }

    #[test]
    fn offset_is_clamped_to_zero_through_ten_thousand() {
        assert_eq!(Page::new(None, Some(-1)).offset(), 0);
        assert_eq!(Page::new(None, Some(i64::MIN)).offset(), 0);
        assert_eq!(Page::new(None, Some(120)).offset(), 120);
        assert_eq!(Page::new(None, Some(10_000)).offset(), 10_000);
        assert_eq!(Page::new(None, Some(10_001)).offset(), 10_000);
        assert_eq!(Page::new(None, Some(i64::MAX)).offset(), Page::MAX_OFFSET);
    }
}
