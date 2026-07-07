# Fixture import manifest

Mechanical import of the audited host-to-terminal fixture corpus from prototype branch
`joshka/qwertty-reference-prototype` (`registry/fixtures/**/*.bytes`) into `fixtures/**/*.seq`.
Origin stamped on every imported file: `origin=prototype:audited-2026-07-06`.

The import is a rename plus header/trailing-LF normalization; the escaped-text payload is kept
verbatim (see `FORMAT.md`). The single exception is `ecma48/c0_lf`, whose `\n` escape was
normalized to `\x0a` so it conforms to the `FORMAT.md` encoding (`\e`/`\xNN`/`\\`) without
changing its decoded byte (`0x0a`).

## Selection rules

A prototype fixture is imported only if all hold:

- Its registry `direction` is `host_to_terminal`, or `bidirectional` in an unambiguous command
  form (BEL, LF, SS3, APC/PM/SOS payloads, sixel/ReGIS/DECUDK framing, vendor user-vars, etc.).
  Every `terminal_to_host`-only fixture is excluded (reports re-enter only via `origin=capture:`).
- It is not on the salvage discard list (`work/phase3/salvage.md` Part 2, applied verbatim).
- It is not flagged WRONG, and not flagged QUESTIONABLE for invalid/misdirected *bytes*, in the
  audit family files (`work/audit/{ecma48-dec,xterm,kitty-rio,osc-vendor}.md`). QUESTIONABLE
  fixtures whose bytes are a valid host command and whose only concern is a naming/semantic
  label are kept (the decoder tokenizes bytes, not labels).
- Its decoded bytes tokenize cleanly through `qwertty::SyntaxParser` with no `Malformed` token
  (see the parser-abort note below).

## Counts per family

| Family | Imported | Excluded | Total |
|--------|----------|----------|-------|
| dec    | 19       | 0        | 19    |
| ecma48 | 77       | 14       | 91    |
| xterm  | 48       | 42       | 90    |
| kitty  | 23       | 27       | 50    |
| iterm2 | 46       | 7        | 53    |
| osc    | 59       | 14       | 73    |
| vendor | 13       | 6        | 19    |
| rio    | 0        | 5        | 5     |
| Total  | 285      | 115      | 400   |

## Exclusions by reason

| Reason                     | Count | Meaning                                         |
|----------------------------|-------|-------------------------------------------------|
| report-direction           | 62    | registry direction is terminal_to_host only     |
| discard-list               | 47    | named on the salvage Part 2 discard list        |
| audit-flagged-wrong        | 3     | audit marks the fixture bytes WRONG             |
| audit-flagged-questionable | 1     | audit QUESTIONABLE: bytes misdirected/invalid   |
| parser-abort               | 2     | embedded raw ESC aborts the DCS at syntax layer |
| Total                      | 115   |                                                 |

Some excluded fixtures satisfy more than one rule (for example a `compat/` report is both a
report-direction and a discard-list item); each is counted once under its primary reason above,
with all matched reasons shown in the per-fixture lists below.

## Audit-flagged exclusions (host-direction, would otherwise import)

These are the only exclusions driven by the audit rather than by direction or the discard list.

| Fixture                       | Verdict      | Reason                                      |
|-------------------------------|--------------|---------------------------------------------|
| `xterm/xtsettcap_set`         | WRONG        | payload is `TN=xterm`, not a terminfo name  |
| `xterm/sgr_mouse_pixel_press` | WRONG        | invented 5-param SGR-pixel form (report)    |
| `xterm/xtgetxres_report`      | WRONG        | hex-encoded `=` in payload (report)         |
| `ecma48/dcs_decrqss`          | QUESTIONABLE | named a request; content is a DECRPSS reply |

## Parser-abort exclusions

`vendor/tmux_passthrough` and `vendor/negative/policy_denied/tmux_passthrough` decode to a DCS
(`ESC P tmux; ...`) carrying doubled raw `ESC` bytes as tmux-passthrough payload. At the pure
ECMA-48 syntax layer an `ESC` inside a control string that is not `ESC \` (ST) aborts the string,
so `qwertty::SyntaxParser` emits a `Malformed` token — correctly, per its design-02 abort
invariant. This is inherent to the sequence (the doubled ESCs are only payload once an outer
tmux layer unwraps them), not a fixable parser gap, so both fixtures are excluded rather than
weakening the invariant. Reconstruction and split-equivalence still hold for them; only the
no-Malformed property fails.

## Every excluded fixture

Grouped by family and primary reason. Paths are relative to the prototype's `registry/`.

### ecma48 (14 excluded)

- **report-direction** (6):
  - `csi_ansi_mode_report.bytes` — report-direction
  - `csi_cpr.bytes` — report-direction
  - `csi_da_primary_report.bytes` — report-direction
  - `csi_da_secondary_report.bytes` — report-direction
  - `csi_dsr_status_ok.bytes` — report-direction
  - `negative/csi_cpr_truncated.bytes` — report-direction
- **discard-list** (7):
  - `compat/c1/csi_da_primary_report.bytes` — report-direction;discard-list:compat attribution
  - `compat/c1/csi_da_secondary_report.bytes` — report-direction;discard-list:compat attribution
  - `compat/windows_terminal/csi_da_primary_report.bytes` — discard-list:compat attribution
  - `compat/xterm/csi_cpr_split.bytes` — report-direction;discard-list:compat attribution
  - `csi_da_tertiary_report.bytes` — report-direction;discard-list:DA-tertiary report
  - `csi_xtwinops_text_area_cells_report.bytes` — report-direction;discard-list:XTWINOPS report
  - `csi_xtwinops_text_area_pixels_report.bytes` — report-direction;discard-list:XTWINOPS report
- **audit-flagged-questionable** (1):
  - `dcs_decrqss.bytes` — audit-flagged-questionable

### xterm (42 excluded)

- **report-direction** (26):
  - `bracketed_paste_end.bytes` — report-direction
  - `bracketed_paste_start.bytes` — report-direction
  - `dec_mode_1_report.bytes` — report-direction
  - `dec_mode_25_report.bytes` — report-direction
  - `dec_mode_6_report.bytes` — report-direction
  - `dec_mode_7_report.bytes` — report-direction
  - `focus_gained.bytes` — report-direction
  - `focus_lost.bytes` — report-direction
  - `key_alt_delete.bytes` — report-direction
  - `key_alt_f1.bytes` — report-direction
  - `key_delete.bytes` — report-direction
  - `legacy_mouse_press_origin.bytes` — report-direction
  - `negative/bracketed_paste_end_truncated.bytes` — report-direction
  - `normal_mouse_release_origin.bytes` — report-direction
  - `sgr_mouse_drag.bytes` — report-direction
  - `sgr_mouse_move.bytes` — report-direction
  - `sgr_mouse_press.bytes` — report-direction
  - `sgr_mouse_release.bytes` — report-direction
  - `sgr_mouse_shift_middle.bytes` — report-direction
  - `sgr_mouse_wheel.bytes` — report-direction
  - `sgr_mouse_wheel_mod_edge.bytes` — report-direction
  - `urxvt_mouse_press.bytes` — report-direction
  - `urxvt_mouse_press_origin.bytes` — report-direction
  - `utf8_mouse_press_wide_column.bytes` — report-direction
  - `x10_mouse_press.bytes` — report-direction
  - `xtgettcap_report.bytes` — report-direction
- **discard-list** (13):
  - `compat/c1/focus_gained.bytes` — report-direction;discard-list:compat attribution
  - `compat/c1/sgr_mouse_press.bytes` — report-direction;discard-list:compat attribution
  - `compat/c1/xtsmgraphics_report.bytes` — report-direction;discard-list:XTSMGRAPHICS group
  - `compat/wezterm/sgr_mouse_press.bytes` — report-direction;discard-list:compat attribution
  - `in_band_resize_disable.bytes` — discard-list:in_band_resize group
  - `in_band_resize_enable.bytes` — discard-list:in_band_resize group
  - `in_band_resize_mode_query.bytes` — discard-list:in_band_resize group
  - `in_band_resize_mode_report.bytes` — report-direction;discard-list:in_band_resize group
  - `in_band_resize_query.bytes` — discard-list:in_band_resize group
  - `in_band_resize_report.bytes` — report-direction;discard-list:in_band_resize group
  - `xtsmgraphics_query.bytes` — discard-list:XTSMGRAPHICS group
  - `xtsmgraphics_report.bytes` — report-direction;discard-list:XTSMGRAPHICS group
  - `xtsmgraphics_report_zero.bytes` — report-direction;discard-list:XTSMGRAPHICS group
- **audit-flagged-wrong** (3):
  - `sgr_mouse_pixel_press.bytes` — report-direction;audit-flagged-wrong
  - `xtgetxres_report.bytes` — report-direction;audit-flagged-wrong
  - `xtsettcap_set.bytes` — audit-flagged-wrong

### kitty (27 excluded)

- **report-direction** (12):
  - `color_report.bytes` — report-direction
  - `graphics_response.bytes` — report-direction
  - `graphics_response_error.bytes` — report-direction
  - `keyboard_event.bytes` — report-direction
  - `keyboard_event_full.bytes` — report-direction
  - `keyboard_event_release.bytes` — report-direction
  - `keyboard_flags_report.bytes` — report-direction
  - `keyboard_keypad_add.bytes` — report-direction
  - `keyboard_right_alt_release.bytes` — report-direction
  - `multicursor_support_report.bytes` — report-direction
  - `negative/graphics_response_unterminated.bytes` — report-direction
  - `pointer_report.bytes` — report-direction
- **discard-list** (15):
  - `compat/c1/graphics_response.bytes` — report-direction;discard-list:compat attribution
  - `compat/c1/keyboard_flags_report.bytes` — report-direction;discard-list:compat attribution
  - `compat/wezterm/graphics_response.bytes` — report-direction;discard-list:compat attribution
  - `compat/wezterm/keyboard_event.bytes` — report-direction;discard-list:compat attribution
  - `file_transfer_cancel.bytes` — discard-list:kitty file_transfer
  - `file_transfer_send.bytes` — discard-list:kitty file_transfer
  - `file_transfer_status.bytes` — report-direction;discard-list:kitty file_transfer
  - `graphics_delete.bytes` — discard-list:graphics_delete
  - `graphics_query.bytes` — discard-list:graphics_query
  - `keyboard_named_key_f5.bytes` — report-direction;discard-list:keyboard_named_key_f5
  - `keyboard_push.bytes` — discard-list:keyboard_push
  - `negative/policy_denied/file_transfer_send.bytes` — discard-list:kitty file_transfer
  - `restore_modes.bytes` — discard-list:restore_modes
  - `save_modes.bytes` — discard-list:save_modes
  - `unscroll.bytes` — discard-list:unscroll

### iterm2 (7 excluded)

- **report-direction** (7):
  - `button_custom_event.bytes` — report-direction
  - `default_background_report.bytes` — report-direction
  - `default_foreground_report.bytes` — report-direction
  - `extended_device_attributes_response.bytes` — report-direction
  - `report_cell_size_report.bytes` — report-direction
  - `report_variable_report.bytes` — report-direction
  - `request_upload_response.bytes` — report-direction

### osc (14 excluded)

- **report-direction** (11):
  - `background_report.bytes` — report-direction
  - `clipboard_report.bytes` — report-direction
  - `cursor_report.bytes` — report-direction
  - `foreground_report.bytes` — report-direction
  - `highlight_background_report.bytes` — report-direction
  - `highlight_foreground_report.bytes` — report-direction
  - `negative/foreground_report_unterminated.bytes` — report-direction
  - `palette_report.bytes` — report-direction
  - `pointer_background_report.bytes` — report-direction
  - `pointer_foreground_report.bytes` — report-direction
  - `special_report.bytes` — report-direction
- **discard-list** (3):
  - `compat/bel/background_report.bytes` — report-direction;discard-list:compat attribution
  - `compat/bel/foreground_report.bytes` — report-direction;discard-list:compat attribution
  - `notification_kitty_created.bytes` — report-direction;discard-list:notification_kitty_created

### vendor (6 excluded)

- **discard-list** (4):
  - `advanced/decaupss.bytes` — discard-list:decaupss
  - `advanced/negative/recovery/decaupss_unterminated.bytes` — discard-list:decaupss
  - `ghostty_extension.bytes` — discard-list:ghostty_extension
  - `screen_passthrough.bytes` — discard-list:screen_passthrough
- **parser-abort** (2):
  - `negative/policy_denied/tmux_passthrough.bytes` — parser-abort:embedded-ESC-in-DCS
  - `tmux_passthrough.bytes` — parser-abort:embedded-ESC-in-DCS

### rio (5 excluded)

- **discard-list** (5):
  - `glyph_query.bytes` — discard-list:rio family
  - `glyph_query_response.bytes` — report-direction;discard-list:rio family
  - `glyph_register_colrv0.bytes` — discard-list:rio family
  - `glyph_register_colrv1.bytes` — discard-list:rio family
  - `glyph_register_glyf.bytes` — discard-list:rio family
