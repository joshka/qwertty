//! Terminal capabilities: the typed result of the DA1-fenced probe bundle (design 03/06).
//!
//! [`Capabilities`] is what [`probe_capabilities`](crate::TokioTerminalSession::probe_capabilities)
//! returns: a struct of typed, optional findings, one per queryable capability the probe bundle
//! asks about in a single write-and-fence round trip. Every field is `Option<T>`, and **`None`
//! means unknown, never unsupported** (FM-C4). A terminal that answers nothing — a silent terminal,
//! or a multiplexer that swallowed the queries — yields an all-`None` `Capabilities`; that is
//! different from a terminal that answered a DECRQM query with "mode reset" (`Some(false)`) or
//! "mode not recognized" (`None` for that one field). Consumers and qwertty's own emit-gating read
//! this distinction, so "we probed and it said no" and "nothing answered" degrade differently
//! (design 06).
//!
//! # Scope: this is the minimal M3-S1 result
//!
//! This slice produces the probe **mechanism** plus a minimal typed result. The full capability
//! model of design 06 — a `Finding<T>` with per-finding `Evidence` provenance (probed vs inferred
//! vs unknown), a `TerminalIdentity` cross-checked with env (`TERM_PROGRAM`, `TMUX`, `ZELLIJ`, …),
//! env-heuristic inference for capabilities with no query (OSC 8, `COLORTERM`), and
//! conformance-derived quirk findings — is **M3-S2**. M3-S2 enriches this struct with evidence
//! provenance, identity, and env inference; the flat `Option<T>` fields here are deliberately the
//! smallest thing that lets the probe round-trip be built and tested.

use crate::correlate::DeviceAttributes as CorrelateDeviceAttributes;

/// A 24-bit RGB colour, 8 bits per channel.
///
/// This is the normalized form of an OSC colour report (design 03): terminals report colours in the
/// X11 `rgb:R/G/B` form with 1–4 hex digits per channel, and
/// [`OscColorReport`](crate::report::OscColorReport) scales every width down to this 8-bit-per-
/// channel value so a consumer sees one shape regardless of the terminal's reporting width (FM-P9).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Rgb {
    red: u8,
    green: u8,
    blue: u8,
}

impl Rgb {
    /// Creates an RGB colour from its three 8-bit channels.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// Returns the red channel.
    #[must_use]
    pub const fn red(self) -> u8 {
        self.red
    }

    /// Returns the green channel.
    #[must_use]
    pub const fn green(self) -> u8 {
        self.green
    }

    /// Returns the blue channel.
    #[must_use]
    pub const fn blue(self) -> u8 {
        self.blue
    }
}

/// The Primary Device Attributes (DA1) a terminal reported as the probe fence.
///
/// DA1 (`CSI ? … c`) is the probe's fence, not a feature oracle (design 03, FM-C4): its arrival
/// means "every reply that was coming has arrived," and its *presence* alone proves nothing about
/// features (a real VT100 answers). This value preserves the raw attribute parameter bytes
/// (everything between `CSI ?` and the final `c`) so a later identity layer (M3-S2) can inspect
/// them — different terminals report different, sometimes widening, attribute lists.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DeviceAttributes {
    params: Vec<u8>,
}

impl DeviceAttributes {
    /// Creates device attributes from the raw DA1 parameter bytes (excluding `?` and the final
    /// `c`).
    #[must_use]
    pub fn new(params: impl Into<Vec<u8>>) -> Self {
        Self {
            params: params.into(),
        }
    }

    /// Returns the raw DA1 parameter bytes, excluding the `?` private marker and the final `c`.
    ///
    /// For `CSI ? 1 ; 2 c` this is `b"1;2"`. An empty slice is possible for a bare `CSI ? c`.
    #[must_use]
    pub fn params(&self) -> &[u8] {
        &self.params
    }
}

impl From<CorrelateDeviceAttributes> for DeviceAttributes {
    fn from(attrs: CorrelateDeviceAttributes) -> Self {
        Self::new(attrs.params().to_vec())
    }
}

/// The typed result of the capability probe bundle (design 03/06).
///
/// Every field is a finding the probe bundle asked about; every field is `Option<T>` where **`None`
/// means unknown, not unsupported** (FM-C4). Build one only through
/// [`probe_capabilities`](crate::TokioTerminalSession::probe_capabilities); this slice offers no
/// public constructor because a hand-built `Capabilities` would carry no evidence of how it was
/// obtained, which is the exact provenance M3-S2 adds.
///
/// # The four DECRQM booleans
///
/// [`synchronized_output`](Self::synchronized_output) (mode 2026),
/// [`grapheme_clustering`](Self::grapheme_clustering) (mode 2027),
/// [`in_band_resize`](Self::in_band_resize) (mode 2048), and
/// [`bracketed_paste`](Self::bracketed_paste) (mode 2004) each come from a DEC private-mode DECRQM
/// answer: `Some(true)` when the terminal reported the mode set or permanently set, `Some(false)`
/// when reset or permanently reset, and `None` when the terminal did not answer *or* answered "mode
/// not recognized" (value 0). The not-recognized-versus-silent difference is collapsed to `None`
/// here on purpose — both mean "do not assume this feature" — and M3-S2's evidence layer is where a
/// consumer that needs to tell them apart will read the provenance.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct Capabilities {
    /// Synchronized output (DEC private mode 2026): whether the terminal batches a frame so a
    /// redraw does not tear (FM-V4). `None` is unknown, not unsupported.
    pub synchronized_output: Option<bool>,
    /// Grapheme clustering / mode 2027: whether the terminal measures width by grapheme cluster
    /// (FM-P15). `None` is unknown.
    pub grapheme_clustering: Option<bool>,
    /// In-band resize (DEC private mode 2048): whether the terminal reports size changes in the
    /// input stream (design 01, R-IN-8). `None` is unknown.
    pub in_band_resize: Option<bool>,
    /// Bracketed paste (DEC private mode 2004): whether the terminal brackets pasted text
    /// (FM-P12). `None` is unknown.
    pub bracketed_paste: Option<bool>,
    /// The kitty keyboard progressive-enhancement flags the terminal reported active for the
    /// `CSI ? u` query (design 06). `None` is unknown (no `CSI ? u` answer).
    pub kitty_keyboard: Option<crate::KittyKeyboardFlags>,
    /// The Primary Device Attributes the terminal reported as the fence (design 03). `None` means
    /// no DA1 arrived — a fully silent terminal, in which case every other field is also `None`.
    pub primary_device_attributes: Option<DeviceAttributes>,
    /// The terminal's self-reported version string from XTVERSION (`CSI > q`). `None` is unknown.
    pub terminal_version: Option<String>,
    /// The terminal's default foreground colour from OSC 10. `None` is unknown.
    pub foreground_color: Option<Rgb>,
    /// The terminal's default background colour from OSC 11. `None` is unknown.
    pub background_color: Option<Rgb>,
}

impl Capabilities {
    /// Returns `true` when the terminal answered nothing at all — every finding is `None`.
    ///
    /// This is the fully-silent case (a terminal that ignored the probe, or a transport that
    /// swallowed it): unknown across the board, never a claim of unsupported (FM-C4).
    #[must_use]
    pub fn is_all_unknown(&self) -> bool {
        self.synchronized_output.is_none()
            && self.grapheme_clustering.is_none()
            && self.in_band_resize.is_none()
            && self.bracketed_paste.is_none()
            && self.kitty_keyboard.is_none()
            && self.primary_device_attributes.is_none()
            && self.terminal_version.is_none()
            && self.foreground_color.is_none()
            && self.background_color.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_are_all_unknown() {
        let caps = Capabilities::default();
        assert!(caps.is_all_unknown());
        assert!(caps.synchronized_output.is_none());
        assert!(caps.background_color.is_none());
    }

    #[test]
    fn one_answered_field_is_not_all_unknown() {
        let caps = Capabilities {
            synchronized_output: Some(true),
            ..Capabilities::default()
        };
        assert!(!caps.is_all_unknown());
    }

    #[test]
    fn rgb_channels_round_trip() {
        let rgb = Rgb::new(0x12, 0x34, 0x56);
        assert_eq!(rgb.red(), 0x12);
        assert_eq!(rgb.green(), 0x34);
        assert_eq!(rgb.blue(), 0x56);
    }

    #[test]
    fn device_attributes_preserve_params() {
        let attrs = DeviceAttributes::new(b"62;1;6".to_vec());
        assert_eq!(attrs.params(), b"62;1;6");
    }
}
