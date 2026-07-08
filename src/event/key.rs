//! Key event vocabulary: [`KeyEvent`] and its parts.
//!
//! These types describe a single key press in the kitty-shaped model design 02 settled on: a
//! keycode ([`Key`]), a set of active [`Modifiers`], an event [`kind`](KeyEventKind), and optional
//! associated [`text`](TextPayload). The shape mirrors the kitty keyboard wire protocol, and the
//! full `CSI u` decode maps onto it without changing the vocabulary.
//!
//! This vocabulary covers single-character text keys, the C0 controls the old decoder named, the
//! four arrow keys, a standalone Escape, and the kitty functional keys (navigation keys and
//! function keys, decoded from both the legacy CSI forms and the kitty `CSI u` Unicode-key-code
//! range). The enums are `#[non_exhaustive]` so release and repeat events and richer modifiers add
//! cleanly later.

/// A keycode in the semantic input vocabulary.
///
/// Text input arrives as [`Key::Char`] (the trivial keycode carried alongside the decoded
/// [`TextPayload`]); the C0 controls the legacy path names map to [`Key::Enter`], [`Key::Tab`], and
/// [`Key::Backspace`]; the four arrow-key CSI sequences map to [`Key::Up`], [`Key::Down`],
/// [`Key::Left`], and [`Key::Right`]; a standalone Escape maps to [`Key::Escape`]. Every other C0
/// control is preserved losslessly as [`Key::Control`] so no byte is lost or turned into a fake
/// named key.
///
/// The kitty keyboard protocol's `CSI u` decode adds the functional keys the protocol names by
/// Unicode code point or legacy CSI final: navigation ([`Key::Home`], [`Key::End`],
/// [`Key::PageUp`], [`Key::PageDown`], [`Key::Insert`], [`Key::Delete`]) and the function keys
/// [`Key::Function`]. The enum is `#[non_exhaustive]` so the remaining kitty functional keys
/// (keypad, media, and modifier keys) add without churning the vocabulary; only the variants the
/// decode and fixtures exercise are present.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Key {
    /// A character key. For legacy UTF-8 input this is the decoded character, and the same
    /// character is also carried in the event's [`TextPayload`].
    ///
    /// From a kitty `CSI u` sequence this is the `char` the unicode-key-code names — the code point
    /// the physical key would produce with no modifiers (design 02). A functional key the protocol
    /// gives a named code point instead becomes the matching named variant, not `Char`.
    Char(char),
    /// Up arrow, from `ESC [ A`, `ESC [ 1 A`, or the kitty `CSI 1 ; mods A` / `CSI 57352 u` form.
    Up,
    /// Down arrow, from `ESC [ B`, `ESC [ 1 B`, or the kitty `CSI 1 ; mods B` / `CSI 57353 u` form.
    Down,
    /// Right arrow, from `ESC [ C`, `ESC [ 1 C`, or the kitty `CSI 1 ; mods C` / `CSI 57351 u`
    /// form.
    Right,
    /// Left arrow, from `ESC [ D`, `ESC [ 1 D`, or the kitty `CSI 1 ; mods D` / `CSI 57350 u` form.
    Left,
    /// Enter, from the carriage return control byte (`CR`, `0x0d`) or kitty `CSI 13 u`.
    Enter,
    /// Horizontal tab, from the tab control byte (`HT`, `0x09`) or kitty `CSI 9 u`.
    Tab,
    /// Backspace, from the delete control byte (`DEL`, `0x7f`), the backspace control byte (`BS`,
    /// `0x08`), or kitty `CSI 127 u`.
    ///
    /// Terminals disagree on which byte the Backspace key sends: most modern terminals send `DEL`
    /// and reserve `BS` for `Ctrl+H`, while some send `BS`. This layer folds both `0x08` and `0x7f`
    /// into the Backspace *key* to match the kitty model and consumer expectations. The distinct
    /// Delete key (the `CSI 3 ~` functional key) is [`Key::Delete`].
    Backspace,
    /// Escape, from a standalone Escape control byte (`ESC`, `0x1b`) flushed by the layer above, or
    /// kitty `CSI 27 u`.
    Escape,
    /// Home, from the kitty `CSI 1 ; mods H` / `CSI 7 ~` / `CSI 57356 u` forms.
    Home,
    /// End, from the kitty `CSI 1 ; mods F` / `CSI 8 ~` / `CSI 57357 u` forms.
    End,
    /// Page Up, from the kitty `CSI 5 ~` / `CSI 57354 u` forms.
    PageUp,
    /// Page Down, from the kitty `CSI 6 ~` / `CSI 57355 u` forms.
    PageDown,
    /// Insert, from the kitty `CSI 2 ~` / `CSI 57348 u` forms.
    Insert,
    /// Delete (the dedicated editing key), from the kitty `CSI 3 ~` / `CSI 57349 u` forms.
    ///
    /// This is distinct from [`Key::Backspace`]: `DEL` (`0x7f`) as a control byte is the Backspace
    /// key, while this named code is the forward-delete editing key.
    Delete,
    /// A function key, numbered from 1. `F1` is `Key::Function(1)`.
    ///
    /// The kitty protocol names F1-F4 through legacy `CSI P/Q/S` and `SS3` forms and F1-F35 through
    /// `CSI number ~` and the `CSI 57376+ u` code-point range. Only the number is carried here; the
    /// wire form it came from is not.
    Function(u8),
    /// Any other C0 control byte (`0x00..=0x1f`), preserved by its raw value so no input is lost.
    ///
    /// `ESC` (`0x1b`) never appears here: a standalone Escape is [`Key::Escape`], and an Escape
    /// that begins a sequence is decoded (or preserved as [`crate::event::Event::Syntax`]) by
    /// the layers around this one.
    Control(u8),
}

/// A bitset of active keyboard modifiers.
///
/// The bit order matches the kitty keyboard protocol's modifier encoding so the `CSI u` decode maps
/// its modifier field (`1 + bitset`, "value-1 encoding") onto these flags directly. This is a
/// hand-rolled bitset rather than a dependency: qwertty keeps its dependency surface minimal, and
/// only the small slice of bitset behavior the decoder needs is implemented.
///
/// Legacy text, C0 controls, and the bare arrow keys carry no modifier information, so those events
/// report [`Modifiers::empty`]. A kitty `CSI u` sequence (and the legacy `CSI 1 ; mods A` modified
/// forms) populate these flags, including the [`Modifiers::CAPS_LOCK`] and [`Modifiers::NUM_LOCK`]
/// lock states the protocol reports in the same field.
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

    /// Builds a modifier set from a raw kitty modifier bitset (the field value minus one).
    ///
    /// The kitty `CSI u` modifier field is `1 + bitset`; this takes the already-decremented bitset.
    /// The bit order matches the constants above, so this is a direct newtype wrap.
    #[must_use]
    pub(crate) const fn from_kitty_bits(bits: u8) -> Self {
        Self(bits)
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

    /// Returns the union of two modifier sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Whether a key event is a press, an auto-repeat, or a release.
///
/// Legacy terminal input carries no press-versus-release distinction, so every legacy event is a
/// [`KeyEventKind::Press`]. The kitty `CSI u` decode reports all three kinds from the event-type
/// subfield (`1` press, `2` repeat, `3` release) when the terminal grants
/// [`KittyKeyboardFlags::REPORT_EVENT_TYPES`](crate::KittyKeyboardFlags::REPORT_EVENT_TYPES).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum KeyEventKind {
    /// The key was pressed. The default when the event-type subfield is absent.
    Press,
    /// The key auto-repeated while held (event type `2`).
    Repeat,
    /// The key was released (event type `3`).
    Release,
}

/// Text associated with a key event.
///
/// A `TextPayload` is a small, multi-codepoint-capable string. Legacy UTF-8 input decodes one
/// character per key event, so a payload from that path always holds exactly one character; the
/// multi-codepoint capacity is for the kitty `CSI u` associated-text field (design 02), which can
/// carry decomposed accents, jamo runs, and ZWJ clusters as one event's text.
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

/// A decoded key event: a keycode, its modifiers, the event kind, associated text, and the kitty
/// alternate keys.
///
/// This is the kitty-shaped key event design 02 settled on. Legacy plain UTF-8 input becomes a
/// `KeyEvent` whose [`Key`] is the trivial [`Key::Char`] keycode and whose [`text`](KeyEvent::text)
/// carries the same character, so text and key association is never lost. C0 controls and arrow
/// keys become `KeyEvent` values with the mapped [`Key`] and no text.
///
/// # Alternate keys
///
/// The kitty keyboard protocol reports up to two *alternate* code points alongside the main key
/// (`CSI unicode-key-code:shifted-key:base-layout-key ; …`): the [`shifted_key`](Self::shifted_key)
/// is the code point the key produces under Shift (e.g. `A` for the `a` key), and the
/// [`base_layout_key`](Self::base_layout_key) is the code point the physical key would produce on
/// the standard (PC-101 US) layout regardless of the active one, which lets an application key
/// shortcuts to physical positions. Both are part of the vocabulary (design 02) and are `None` for
/// legacy input and for kitty events that omit the subfields.
///
/// The struct is `#[non_exhaustive]`: construct it with [`KeyEvent::new`] and refine it with the
/// `with_*` builder methods.
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
/// assert_eq!(event.shifted_key(), None);
/// assert_eq!(event.base_layout_key(), None);
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct KeyEvent {
    key: Key,
    modifiers: Modifiers,
    kind: KeyEventKind,
    text: Option<TextPayload>,
    shifted_key: Option<char>,
    base_layout_key: Option<char>,
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
            shifted_key: None,
            base_layout_key: None,
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

    /// Sets the kitty shifted-key alternate (the code point produced under Shift) and returns the
    /// event.
    #[must_use]
    pub fn with_shifted_key(mut self, shifted_key: char) -> Self {
        self.shifted_key = Some(shifted_key);
        self
    }

    /// Sets the kitty base-layout-key alternate (the code point on the standard layout) and returns
    /// the event.
    #[must_use]
    pub fn with_base_layout_key(mut self, base_layout_key: char) -> Self {
        self.base_layout_key = Some(base_layout_key);
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

    /// Returns the kitty shifted-key alternate, the code point this key produces under Shift.
    ///
    /// This is `None` for legacy input and for kitty events whose shifted-key subfield was empty or
    /// absent. For `Shift+a` (`CSI 97:65 ; 2 u`) it is `Some('A')`.
    #[must_use]
    pub fn shifted_key(&self) -> Option<char> {
        self.shifted_key
    }

    /// Returns the kitty base-layout-key alternate, the code point the physical key produces on the
    /// standard (PC-101 US) layout.
    ///
    /// This is `None` for legacy input and for kitty events that omit the base-layout subfield. It
    /// lets an application bind shortcuts to physical key positions independent of the active
    /// layout.
    #[must_use]
    pub fn base_layout_key(&self) -> Option<char> {
        self.base_layout_key
    }
}
