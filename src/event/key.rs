//! Key event vocabulary: [`KeyEvent`] and its parts.
//!
//! These types describe a single key press in the kitty-shaped model design 02 settled on: a
//! keycode ([`Key`]), a set of active [`Modifiers`], an event [`kind`](KeyEventKind), and optional
//! associated [`text`](TextPayload). The shape mirrors the kitty keyboard wire protocol so the full
//! `CSI u` decode landing in milestone M4 maps onto it without changing the vocabulary.
//!
//! The parity scope of this slice produces only a small part of this vocabulary: single-character
//! text keys, the C0 controls the old decoder named, the four arrow keys, and a standalone Escape.
//! The enums are `#[non_exhaustive]` so kitty functional keys, release and repeat events, and
//! richer modifiers add cleanly later.

/// A keycode in the semantic input vocabulary.
///
/// This is intentionally the smallest set the current parity scope can produce. Text input arrives
/// as [`Key::Char`] (the trivial keycode carried alongside the decoded [`TextPayload`]); the C0
/// controls the old decoder named map to [`Key::Enter`], [`Key::Tab`], and [`Key::Backspace`]; the
/// four arrow-key CSI sequences map to [`Key::Up`], [`Key::Down`], [`Key::Left`], and
/// [`Key::Right`]; a standalone Escape maps to [`Key::Escape`]. Every other C0 control is preserved
/// losslessly as [`Key::Control`] so no byte is lost or turned into a fake named key.
///
/// The kitty functional keys (Home, End, F1, and the rest of the `CSI u` set) are not produced yet;
/// they arrive with the `CSI u` decode in milestone M4. The enum is `#[non_exhaustive]` so they add
/// without churning the vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Key {
    /// A character key. For legacy UTF-8 input this is the decoded character, and the same
    /// character is also carried in the event's [`TextPayload`].
    Char(char),
    /// Up arrow, from `ESC [ A` (or `ESC [ 1 A`).
    Up,
    /// Down arrow, from `ESC [ B` (or `ESC [ 1 B`).
    Down,
    /// Right arrow, from `ESC [ C` (or `ESC [ 1 C`).
    Right,
    /// Left arrow, from `ESC [ D` (or `ESC [ 1 D`).
    Left,
    /// Enter, from the carriage return control byte (`CR`, `0x0d`).
    Enter,
    /// Horizontal tab, from the tab control byte (`HT`, `0x09`).
    Tab,
    /// Backspace, from the delete control byte (`DEL`, `0x7f`) or the backspace control byte (`BS`,
    /// `0x08`).
    ///
    /// Terminals disagree on which byte the Backspace key sends: most modern terminals send `DEL`
    /// and reserve `BS` for `Ctrl+H`, while some send `BS`. The old decoder named `0x08` and `0x7f`
    /// as separate `ControlInput::Backspace` and `ControlInput::Delete` byte classifications; this
    /// layer folds both into the Backspace *key* to match the kitty model and consumer
    /// expectations. A dedicated Delete key (the `CSI 3 ~` functional key) is a distinct code
    /// that arrives with the milestone M4 `CSI u` decode.
    Backspace,
    /// Escape, from a standalone Escape control byte (`ESC`, `0x1b`) flushed by the layer above.
    Escape,
    /// Any other C0 control byte (`0x00..=0x1f`), preserved by its raw value so no input is lost.
    ///
    /// `ESC` (`0x1b`) never appears here: a standalone Escape is [`Key::Escape`], and an Escape
    /// that begins a sequence is decoded (or preserved as [`crate::event::Event::Syntax`]) by
    /// the layers around this one.
    Control(u8),
}

/// A bitset of active keyboard modifiers.
///
/// The bit order matches the kitty keyboard protocol's modifier encoding so the milestone M4
/// `CSI u` decode can map its modifier field onto these flags directly. This is a hand-rolled
/// bitset rather than a dependency: qwertty keeps its dependency surface minimal, and only the
/// small slice of bitset behavior the decoder needs is implemented.
///
/// The current parity scope never sets a modifier (legacy text, controls, and arrow keys carry no
/// modifier information), so every event this slice produces reports [`Modifiers::empty`]. The
/// flags exist so the vocabulary does not churn when `CSI u` decoding begins reporting them.
///
/// # Example
///
/// ```
/// use qwertty::Modifiers;
///
/// let mods = Modifiers::CTRL;
/// assert!(mods.contains(Modifiers::CTRL));
/// assert!(!mods.contains(Modifiers::SHIFT));
/// assert!(Modifiers::empty().is_empty());
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Modifiers(u8);

impl Modifiers {
    /// Shift, kitty modifier bit `0b0000_0001`.
    pub const SHIFT: Self = Self(0b0000_0001);
    /// Alt (Option), kitty modifier bit `0b0000_0010`.
    pub const ALT: Self = Self(0b0000_0010);
    /// Control, kitty modifier bit `0b0000_0100`.
    pub const CTRL: Self = Self(0b0000_0100);
    /// Super (Command / Windows), kitty modifier bit `0b0000_1000`.
    pub const SUPER: Self = Self(0b0000_1000);
    /// Hyper, kitty modifier bit `0b0001_0000`.
    pub const HYPER: Self = Self(0b0001_0000);
    /// Meta, kitty modifier bit `0b0010_0000`.
    pub const META: Self = Self(0b0010_0000);
    /// Caps Lock, kitty modifier bit `0b0100_0000`.
    pub const CAPS_LOCK: Self = Self(0b0100_0000);
    /// Num Lock, kitty modifier bit `0b1000_0000`.
    pub const NUM_LOCK: Self = Self(0b1000_0000);

    /// Returns the empty modifier set (no modifiers active).
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Returns `true` when no modifier is active.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when every flag in `other` is active in `self`.
    ///
    /// Returns `true` when `other` is empty, following the usual bitset containment convention.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Adds every flag in `other` to this set.
    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Whether a key event is a press, an auto-repeat, or a release.
///
/// The current parity scope produces only [`KeyEventKind::Press`]: legacy terminal input carries no
/// press-versus-release distinction, so every event this slice decodes is a press. The other kinds
/// exist because the kitty keyboard protocol reports them, and the milestone M4 `CSI u` decode will
/// produce them without changing this vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum KeyEventKind {
    /// The key was pressed. The only kind this slice produces.
    Press,
    /// The key auto-repeated while held. Produced by the milestone M4 `CSI u` decode.
    Repeat,
    /// The key was released. Produced by the milestone M4 `CSI u` decode.
    Release,
}

/// Text associated with a key event.
///
/// A `TextPayload` is a small, multi-codepoint-capable string. Legacy UTF-8 input decodes one
/// character per key event, so a payload from this slice always holds exactly one character (design
/// 02: the multi-codepoint capacity exists for the kitty `CSI u` associated-text field that arrives
/// in milestone M4, not for legacy bytes). Keeping text as an owned string rather than a bare
/// `char` lets that later path represent decomposed accents, jamo runs, and ZWJ clusters as one
/// event.
///
/// The representation is a `std` [`String`] today. An inline small-string optimization is an
/// implementation detail deferred to a later slice; the newtype exists so that change carries no
/// API break.
///
/// # Example
///
/// ```
/// use qwertty::TextPayload;
///
/// let payload = TextPayload::from_char('é');
/// assert_eq!(payload.as_str(), "é");
/// ```
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TextPayload(String);

impl TextPayload {
    /// Creates a payload holding a single character.
    #[must_use]
    pub fn from_char(character: char) -> Self {
        Self(character.to_string())
    }

    /// Creates a payload from a string slice.
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        Self(text.to_owned())
    }

    /// Returns the payload text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the payload and returns its owned string.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::ops::Deref for TextPayload {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// A decoded key event: a keycode, its modifiers, the event kind, and any associated text.
///
/// This is the kitty-shaped key event design 02 settled on. Legacy plain UTF-8 input becomes a
/// `KeyEvent` whose [`Key`] is the trivial [`Key::Char`] keycode and whose [`text`](KeyEvent::text)
/// carries the same character, so text and key association is never lost. C0 controls and arrow
/// keys become `KeyEvent` values with the mapped [`Key`] and no text.
///
/// The struct is `#[non_exhaustive]`: construct it with [`KeyEvent::new`] and refine it with the
/// `with_*` builder methods. Fields are added over later slices (kitty associated text, alternate
/// keycodes) without breaking construction.
///
/// # Example
///
/// ```
/// use qwertty::{Key, KeyEvent, KeyEventKind, Modifiers};
///
/// let event = KeyEvent::new(Key::Char('a')).with_text('a');
///
/// assert_eq!(event.key(), Key::Char('a'));
/// assert_eq!(event.modifiers(), Modifiers::empty());
/// assert_eq!(event.kind(), KeyEventKind::Press);
/// assert_eq!(event.text().map(|t| t.as_str()), Some("a"));
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct KeyEvent {
    key: Key,
    modifiers: Modifiers,
    kind: KeyEventKind,
    text: Option<TextPayload>,
}

impl KeyEvent {
    /// Creates a key press with no modifiers and no associated text.
    ///
    /// The event's [`kind`](KeyEvent::kind) is [`KeyEventKind::Press`] and its
    /// [`modifiers`](KeyEvent::modifiers) are [`Modifiers::empty`]. Use the `with_*` methods to set
    /// text, modifiers, or a different kind.
    #[must_use]
    pub fn new(key: Key) -> Self {
        Self {
            key,
            modifiers: Modifiers::empty(),
            kind: KeyEventKind::Press,
            text: None,
        }
    }

    /// Sets the associated text to a single character and returns the event.
    #[must_use]
    pub fn with_text(mut self, character: char) -> Self {
        self.text = Some(TextPayload::from_char(character));
        self
    }

    /// Sets the associated text payload and returns the event.
    #[must_use]
    pub fn with_text_payload(mut self, text: TextPayload) -> Self {
        self.text = Some(text);
        self
    }

    /// Sets the active modifiers and returns the event.
    #[must_use]
    pub fn with_modifiers(mut self, modifiers: Modifiers) -> Self {
        self.modifiers = modifiers;
        self
    }

    /// Sets the event kind and returns the event.
    #[must_use]
    pub fn with_kind(mut self, kind: KeyEventKind) -> Self {
        self.kind = kind;
        self
    }

    /// Returns the keycode.
    #[must_use]
    pub fn key(&self) -> Key {
        self.key
    }

    /// Returns the active modifiers.
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Returns whether this event is a press, repeat, or release.
    #[must_use]
    pub fn kind(&self) -> KeyEventKind {
        self.kind
    }

    /// Returns the associated text, if any.
    ///
    /// Character keys from legacy UTF-8 input carry the decoded character here; control and arrow
    /// keys carry no text.
    #[must_use]
    pub fn text(&self) -> Option<&TextPayload> {
        self.text.as_ref()
    }
}
