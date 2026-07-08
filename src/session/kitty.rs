//! Kitty keyboard progressive-enhancement flags: the caller-chosen request set and the granted
//! result of verify-after-push.
//!
//! The kitty keyboard protocol is a *progressive enhancement*: an application pushes a set of flags
//! (`CSI > flags u`), each bit turning on one reporting behaviour, and the terminal enables the
//! subset it supports. There is no single "enable kitty" switch — the caller chooses which
//! behaviours it wants (design 06, rabbitui P0-4), and must then verify which were actually granted
//! because a terminal may grant a subset, or (over a mux, or an old terminal) none at all.

/// A set of kitty keyboard progressive-enhancement flags.
///
/// Each flag is one reporting behaviour the application requests with `CSI > flags u`. Combine them
/// with [`KittyKeyboardFlags::union`] or the individual constructors; read them back with
/// [`KittyKeyboardFlags::contains`]. The bit values are the protocol's own (design 06).
///
/// # Example
///
/// ```
/// use qwertty::KittyKeyboardFlags;
///
/// let requested =
///     KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES.union(KittyKeyboardFlags::REPORT_EVENT_TYPES);
///
/// assert!(requested.contains(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES));
/// assert_eq!(requested.bits(), 0b0000_0011);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct KittyKeyboardFlags(u8);

impl KittyKeyboardFlags {
    /// Disambiguate escape codes (bit `0b0000_0001`).
    ///
    /// Makes keys like Escape, and modified keys that collide with legacy sequences, report an
    /// unambiguous `CSI u` form. This is the flag most applications want first; it removes the bare
    /// Escape timing ambiguity (design 02).
    pub const DISAMBIGUATE_ESCAPE_CODES: Self = Self(0b0000_0001);
    /// Report event types (bit `0b0000_0010`): press, repeat, and release.
    pub const REPORT_EVENT_TYPES: Self = Self(0b0000_0010);
    /// Report alternate keys (bit `0b0000_0100`): the shifted-key and base-layout-key subfields.
    pub const REPORT_ALTERNATE_KEYS: Self = Self(0b0000_0100);
    /// Report all keys as escape codes (bit `0b0000_1000`), including plain text keys.
    pub const REPORT_ALL_KEYS_AS_ESCAPE_CODES: Self = Self(0b0000_1000);
    /// Report associated text (bit `0b0001_0000`): the text-as-code-points field.
    pub const REPORT_ASSOCIATED_TEXT: Self = Self(0b0001_0000);

    /// Returns the empty flag set.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Builds a flag set from its raw protocol bits.
    ///
    /// Bits outside the five defined flags are preserved so a terminal reporting a future flag this
    /// version does not name is recorded faithfully rather than silently masked.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the raw protocol bits, as written after `CSI >` and read from `CSI ? … u`.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns `true` when no flag is set.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns the union of two flag sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns the intersection of two flag sets.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Returns `true` when every flag in `other` is set in `self`.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The outcome of a kitty keyboard verify-after-push request (design 06).
///
/// A request pushes the caller's [`requested`](Self::requested) flags, queries the terminal for the
/// flags it actually turned on, and records the granted reality. The three shapes matter because
/// **unknown is not unsupported** (FM-C4): a terminal that never answers the query leaves the
/// grant [`unknown`](Self::is_unknown), which is different from a terminal that answered with a
/// smaller set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KittyKeyboardGrant {
    requested: KittyKeyboardFlags,
    granted: Option<KittyKeyboardFlags>,
}

impl KittyKeyboardGrant {
    /// Builds a grant from the requested flags and the terminal's answer (`None` when it did not
    /// answer within the caller's budget).
    #[must_use]
    #[cfg_attr(
        not(all(feature = "tokio", unix)),
        expect(dead_code, reason = "constructed only by the tokio+unix async session")
    )]
    pub(crate) const fn new(
        requested: KittyKeyboardFlags,
        granted: Option<KittyKeyboardFlags>,
    ) -> Self {
        Self { requested, granted }
    }

    /// Returns the flags the caller requested.
    #[must_use]
    pub const fn requested(self) -> KittyKeyboardFlags {
        self.requested
    }

    /// Returns the flags the terminal reported as active, or `None` when it never answered.
    ///
    /// `None` means the query timed out or hit EOF: the terminal's support is *unknown*, and no
    /// enhancement should be assumed (FM-C4). A terminal that granted nothing answers with
    /// `Some(`[`KittyKeyboardFlags::empty`]`)`, which is distinct.
    #[must_use]
    pub const fn granted(self) -> Option<KittyKeyboardFlags> {
        self.granted
    }

    /// Returns `true` when the terminal never answered the flags query.
    ///
    /// The request degraded gracefully: nothing was recorded as granted and no enhancement is
    /// assumed. The push bytes were still written, but the session ledger records the pop only for
    /// flags that were actually granted, so an unknown grant leaves no keyboard entry to undo.
    #[must_use]
    pub const fn is_unknown(self) -> bool {
        self.granted.is_none()
    }

    /// Returns `true` when the terminal granted every flag the caller requested.
    ///
    /// This is `false` when the grant is unknown, or when the terminal granted only a subset — the
    /// verify-after-push mismatch case the caller must handle (helix handshake, design 06).
    #[must_use]
    pub fn granted_all_requested(self) -> bool {
        self.granted
            .is_some_and(|granted| granted.contains(self.requested))
    }
}
