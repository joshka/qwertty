# Terminal specification index

A curated map of the primary sources for the terminal-control space: the standards, the DEC
manuals, the xterm reference, the vendor protocols, and the per-emulator documentation — grouped by
area so you can go from "what is `CSI 8 ; … t`?" to the document that defines it.

This page is hand-maintained and deliberately broader than the machine-validated citation registry
in [`db/sources.toml`][sources-toml]. That file only lists sources the database *already cites*, and
`qdb validate` enforces that every entry's `refs` key resolves there. This index is the superset: it
also points at the specs for areas qwertty has **not** cataloged yet, so the reference knows where to
reach when those families are filled in.

**Legend.** Each entry is tagged:

- **(in db)** — already cited in [`db/sources.toml`][sources-toml]; backs one or more cataloged
  sequences.
- **(gap)** — the spec is canonical but the area is **not yet in the database**. Listed here so the
  catalog has a known home to grow into.

Some canonical specs live on servers behind an anti-bot wall (the freedesktop GitLab instance
returns an "Access Denied / Anubis" page to automated fetchers); those URLs are correct and load
fine in a normal browser.

## Start here — comprehensive references

The four resources that cover the most ground in one place:

- [vtdn.dev — Terminal control-sequence reference][vtdn] **(gap as a cited source)** — a modern,
  well-organized, MDN-style reference covering C0 controls, ESC/CSI/OSC/DCS sequences, SGR, private
  modes, keyboard/input, graphics, and window manipulation. The closest existing thing to the site
  qwertty is building; excellent for orientation. Josh's find; not yet a `sources.toml` key.
- [XTerm Control Sequences (`ctlseqs`)][xterm-ctlseqs] **(in db)** — Thomas Dickey's exhaustive,
  authoritative catalog of what xterm implements. The de-facto reference every other emulator is
  measured against.
- [ECMA-48: Control Functions for Coded Character Sets][ecma48] **(in db)** — the base standard
  (identical in substance to ANSI X3.64 / ISO 6429). Defines the C0/C1 sets, the CSI/OSC/DCS
  framing, and the core control functions.
- [vt100.net (Paul Williams' terminal archive)][vt100net] — the home of the scanned DEC manuals
  below and of the canonical [VT500-series parser state machine][dec-parser], the reference diagram
  most terminal decoders (qwertty's syntax layer included) are validated against.
- [All Known Control Sequences (Kermit 95 / CKW)][ckwin] **(gap as a cited source)** — David Goodwin's
  exhaustive cross-referenced catalog of control sequences, each annotated with which hardware
  terminals and emulators implement it. Strong for "who supports this and where did it come from".

## Core standards and the parser

- [ECMA-48][ecma48] **(in db)** — control functions, C0/C1, CSI/OSC/DCS/APC/PM/SOS framing.
- [VT500-series parser state machine][dec-parser] — Williams' state/transition diagram for the
  ESC/CSI/OSC/DCS byte grammar; the canonical model for a conformant decoder.
- [C0 control codes (vtdn.dev)][vtdn-c0] **(gap)** — a readable per-character reference for the C0
  set (BEL, BS, HT, LF, VT, FF, CR, SO/SI, …). qwertty documents the *grammar* in `ecma48-syntax`
  but has no per-control MDN-style page yet.
- [XFree86 Control Sequences][xfree86] **(in db)** — the XFree86-era sequence list; still cited for
  a handful of DEC/cursor entries.

## DEC VT-series manuals

The primary sources for DEC private modes, charsets, rectangular editing, and the two DEC graphics
languages. Most are on [vt100.net][vt100net]; DEC STD 070 is DEC's own internal standard.

- [DEC STD 070: Video Systems Reference Manual][dec-std-070] **(gap)** — the internal DEC standard
  every DEC video terminal was built to comply with: what each escape sequence does and *why*, with
  pseudocode in many places. Far more detailed than any of DEC's public product documentation.
  A more workable HTMLised edition by James Holderness is at [j4james.github.io/vtdocs][vtdocs].
  This edition is ~VT420-era, so it predates the newer vendor extensions. (Thanks to David Goodwin
  for the pointer.)
- [VT320 Programmer Reference, Appendix E: Control Functions][vt320-e] **(in db)** — the compact
  control-function table backing much of the `dec` family.
- [VT510 Programmer Information — contents][vt510] **(gap)** — the fullest online DEC reference.
  Chapter 5 defines the **rectangular area operations** (`DECCRA` copy, `DECFRA` fill, `DECERA`
  erase, `DECSERA` selective erase) and **`DECRQSS`** (Request Selection or Setting) — all
  uncataloged today. Also the canonical source for `DECSCA`/selective erase and conformance levels.
- [VT330/VT340 Programmer Reference, Ch. 14: Sixel Graphics][vt3xx-sixel] **(gap)** — the Sixel
  bitmap protocol. qwertty currently keeps Sixel only as an opaque DCS-preservation entry in
  `vendor-dcs`; this is the spec for a real family.
- [VT330/VT340 Programmer Reference, Ch. 1: ReGIS][vt3xx-regis] **(gap)** — the ReGIS vector
  graphics instruction set. Not cataloged at all.

## xterm

- [XTerm Control Sequences (`ctlseqs`)][xterm-ctlseqs] **(in db)** — the master catalog, including
  the DEC private modes, `modifyOtherKeys`, mouse encodings, and the XTWINOPS `CSI … t` window ops.
- [XTerm FAQ][xterm-faq] **(gap)** — Dickey's FAQ; the best prose companion to `ctlseqs` for
  *why* a sequence behaves as it does and how xterm's resources gate features. Josh's find.

## OSC — Operating System Command

- [Hyperlinks in terminal emulators (OSC 8)][osc8] **(gap as a cited source)** — Egmont Koblinger's
  spec for `OSC 8`. `OSC 8` is cataloged; this canonical write-up is not yet a `sources.toml` key.
- [Semantic prompts / shell integration (OSC 133)][osc133] **(gap)** — Per Bothner's proposal, the
  reference for the `OSC 133 ; A/B/C/D` prompt-marking sequence adopted by iTerm2, kitty, WezTerm,
  Ghostty, and VS Code. A browser-friendly rendering is [Contour's OSC 133 page][contour-osc133].
- [VS Code terminal shell integration][vscode-shell] **(in db)** — Microsoft's `OSC 633` superset
  of the `OSC 133` prompt sequences.
- [ConEmu ANSI / OSC escape codes][conemu] **(gap)** — the reference for the ConEmu-specific
  `OSC 9 ; …` sequences, including `OSC 9 ; 4` progress-bar reporting later adopted by Windows
  Terminal. qwertty catalogs `OSC 9 ; 4` under the iTerm2 vendor name; this is the ConEmu origin.

## Graphics protocols

- [kitty Graphics Protocol][kitty-graphics] **(in db)** — the modern in-band raster protocol
  (`APC G …`); fully cataloged as `kitty-graphics`.
- [iTerm2 Inline Images Protocol][iterm2-images] **(in db)** — `OSC 1337 ; File=…`; cataloged in the
  `iterm2` family.
- [Sixel graphics (VT330/340 Ch. 14)][vt3xx-sixel] **(gap)** — see DEC manuals above.
- [ReGIS graphics (VT330/340 Ch. 1)][vt3xx-regis] **(gap)** — see DEC manuals above.

## Keyboard and input

- [kitty Keyboard Protocol][kitty-keyboard] **(in db)** — progressive-enhancement `CSI … u`
  encoding; cataloged as `kitty-keyboard`, and the model for qwertty's `xterm-input` CSI-u decode.
- [XTerm Control Sequences — `modifyOtherKeys`][xterm-ctlseqs] **(in db)** — xterm's legacy modified
  key reporting; documented within `ctlseqs`.
- [Windows Terminal spec #4999 — win32-input-mode][conpty-4999] **(in db)** — the ConPTY keyboard
  encoding; backs the `conpty` family.
- [Windows Terminal `doc/specs` index][wt-specs] **(gap)** — the full set of Windows Terminal /
  ConPTY design specs beyond #4999.

## kitty protocol suite

kitty documents each extension separately under [sw.kovidgoyal.net/kitty][kitty-home]. All are
**(in db)** and cataloged as the `kitty-*` families:

- [Keyboard][kitty-keyboard] · [Graphics][kitty-graphics] · [Color stack][kitty-color] ·
  [Text sizing][kitty-textsize] · [Pointer shapes][kitty-pointer] ·
  [Multiple cursors][kitty-multicursor] · [Desktop notifications][kitty-notify] ·
  [Miscellaneous protocol extensions][kitty-ext]

## iTerm2 proprietary

All **(in db)**, backing the `iterm2` family:

- [Proprietary escape codes][iterm2-codes] · [Inline images][iterm2-images] · [Badges][iterm2-badges]

## Terminal emulator references

Per-emulator canonical documentation — useful both as spec sources and as conformance-target
context for the support matrix:

- [WezTerm escape sequences][wezterm] **(in db)** — Wez Furlong's per-sequence support notes.
- [Contour VT extensions][contour] **(gap)** — index of Contour's non-standard extensions
  (synchronized output, buffer capture, semantic block query, unicode core, and more).
- [Ghostty Terminal API (VT)][ghostty] **(gap)** — Ghostty's control-sequence support overview.
- [mintty control sequences][mintty] **(gap)** — mintty's xterm-compatible sequence list.
- [foot][foot] **(gap)** — the foot repository; its `foot-ctlseqs(7)` man page
  (`doc/foot-ctlseqs.7.scd`) is the canonical per-sequence reference.
- [ConEmu ANSI / OSC codes][conemu] **(gap)** — see OSC above.

## Working groups and cross-vendor proposals

- [terminal-wg specifications][terminal-wg] **(in db)** — the freedesktop terminal working group's
  specification tracker (issues #14 and #20 are cited today). Anti-bot walled to fetchers; loads in
  a browser.
- [Per Bothner specifications (semantic prompts, etc.)][perbothner-specs] **(gap)** — the proposal
  repo that hosts the `OSC 133` semantic-prompts document above.
- [Contour VT extensions][contour] **(gap)** — see emulator references.

## Unicode and character width

Relevant because qwertty owns terminal-aware width measurement (`db/width/`), and cell width is
where terminals disagree most:

- [UAX #11: East Asian Width][uax11] **(gap)** — the width property that drives 1-vs-2 cell layout.
- [UAX #29: Text Segmentation][uax29] **(gap)** — grapheme cluster boundaries (what counts as one
  "character" on screen).
- [UTS #51: Unicode Emoji][uts51] **(gap)** — emoji presentation and ZWJ sequences, the hardest
  width cases.
- [kitty Text Sizing Protocol][kitty-textsize] **(in db)** — the in-band protocol for apps to state
  intended cell width explicitly rather than infer it.

<!-- Reference definitions. Long URLs live here so body lines stay within the 100-column limit. -->

[sources-toml]: https://github.com/joshka/qwertty/blob/main/db/sources.toml
[vtdn]: https://vtdn.dev/
[vtdn-c0]: https://vtdn.dev/docs/category/c0-control-codes
[xterm-ctlseqs]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
[xterm-faq]: https://invisible-island.net/xterm/xterm.faq.html
[xfree86]: https://www.xfree86.org/current/ctlseqs.html
[ecma48]: https://ecma-international.org/publications-and-standards/standards/ecma-48/
[vt100net]: https://vt100.net/
[dec-parser]: https://vt100.net/emu/dec_ansi_parser
[ckwin]: https://davidrg.github.io/ckwin/dev/all-ctlseqs.html
[dec-std-070]: https://bitsavers.org/pdf/dec/standards/EL-SM070-00_DEC_STD_070_Video_Systems_Reference_Manual_Dec91.pdf
[vtdocs]: https://j4james.github.io/vtdocs/
[vt320-e]: https://vt100.net/docs/vt320-uu/appendixe.html
[vt510]: https://vt100.net/docs/vt510-rm/contents.html
[vt3xx-sixel]: https://vt100.net/docs/vt3xx-gp/chapter14.html
[vt3xx-regis]: https://vt100.net/docs/vt3xx-gp/chapter1.html
[osc8]: https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda
[osc133]: https://gitlab.freedesktop.org/Per_Bothner/specifications/-/blob/master/proposals/semantic-prompts.md
[contour-osc133]: https://contour-terminal.org/vt-extensions/osc-133-shell-integration/
[vscode-shell]: https://code.visualstudio.com/docs/terminal/shell-integration
[conemu]: https://conemu.github.io/en/AnsiEscapeCodes.html
[kitty-home]: https://sw.kovidgoyal.net/kitty/
[kitty-keyboard]: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
[kitty-graphics]: https://sw.kovidgoyal.net/kitty/graphics-protocol/
[kitty-color]: https://sw.kovidgoyal.net/kitty/color-stack/
[kitty-textsize]: https://sw.kovidgoyal.net/kitty/text-sizing-protocol/
[kitty-pointer]: https://sw.kovidgoyal.net/kitty/pointer-shapes/
[kitty-multicursor]: https://sw.kovidgoyal.net/kitty/multiple-cursors-protocol/
[kitty-notify]: https://sw.kovidgoyal.net/kitty/desktop-notifications/
[kitty-ext]: https://sw.kovidgoyal.net/kitty/protocol-extensions/
[iterm2-codes]: https://iterm2.com/documentation-escape-codes.html
[iterm2-images]: https://iterm2.com/documentation-images.html
[iterm2-badges]: https://iterm2.com/documentation-badges.html
[wezterm]: https://wezterm.org/escape-sequences.html
[contour]: https://contour-terminal.org/vt-extensions/
[ghostty]: https://ghostty.org/docs/vt
[mintty]: https://github.com/mintty/mintty/wiki/CtrlSeqs
[foot]: https://codeberg.org/dnkl/foot
[conpty-4999]: https://github.com/microsoft/terminal/blob/main/doc/specs/%234999%20-%20Improved%20keyboard%20handling%20in%20Conpty.md
[wt-specs]: https://github.com/microsoft/terminal/tree/main/doc/specs
[terminal-wg]: https://gitlab.freedesktop.org/terminal-wg/specifications
[perbothner-specs]: https://gitlab.freedesktop.org/Per_Bothner/specifications
[uax11]: https://www.unicode.org/reports/tr11/
[uax29]: https://www.unicode.org/reports/tr29/
[uts51]: https://www.unicode.org/reports/tr51/
