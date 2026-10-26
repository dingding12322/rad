# Zed Design System — Token Reference

Pulled directly from source at [zed-industries/zed](https://github.com/zed-industries/zed), `main` branch, snapshot 2026‑09‑25 (tree `7fdb97cad5…`). Zed's UI is built on **GPUI** (their own Rust UI framework) with a Tailwind‑CSS‑style utility API, so most tokens below are GPUI/`ui` crate constants, not CSS.

> **Everything scales.** Almost every value here is stored in `rem`s, not raw pixels. At runtime it's multiplied by (a) the user's `ui_font_size` setting (default **16px = 1rem**) and (b) a density multiplier from `DynamicSpacing` (§2). All px figures below are the values **at default UI font size + Default density** — change either setting and they shift proportionally.

---

## 1. Base scale (`gpui_macros/src/styles.rs`)

The primitive spacing/sizing scale behind every `p_`, `m_`, `gap_`, `w_`, `h_`, `size_`, `inset_*` utility (`rem = 16px`; `p5` suffix = a half‑step):

| Suffix | px | rem | | Suffix | px | rem |
|---|---|---|---|---|---|---|
| `0` | 0 | 0 | | `8` | 32 | 2 |
| `0p5` | 2 | 0.125 | | `9` | 36 | 2.25 |
| `1` | 4 | 0.25 | | `10` | 40 | 2.5 |
| `1p5` | 6 | 0.375 | | `11` | 44 | 2.75 |
| `2` | 8 | 0.5 | | `12` | 48 | 3 |
| `2p5` | 10 | 0.625 | | `16` | 64 | 4 |
| `3` | 12 | 0.75 | | `20` | 80 | 5 |
| `3p5` | 14 | 0.875 | | `24` | 96 | 6 |
| `4` | 16 | 1 | | `32` | 128 | 8 |
| `5` | 20 | 1.25 | | `40` | 160 | 10 |
| `6` | 24 | 1.5 | | `48` | 192 | 12 |
| `7` | 28 | 1.75 | | `64` | 256 | 16 |
| | | | | `72` | 288 | 18 |
| | | | | `96` | 384 | 24 |

`px` suffix = literal 1px (unscaled). `full` = 100%.

**Corner radius scale** (`rounded_*`): `none` 0 · `xs` 2px · `sm` 4px · `md` 6px · `lg` 8px · `xl` 12px · `2xl` 16px · `3xl` 24px · `full` 9999px

**Border width scale** (`border_*`): direct px, 0–12 in steps of 1, then 16 / 20 / 24 / 32.

---

## 2. Dynamic spacing — `DynamicSpacing` (`ui/src/styles/spacing.rs`)

The **real** spacing system components use (not the raw scale above). Three density settings — Compact / **Default** / Comfortable — set in Settings → UI Density (still marked experimental in‑repo).

| Token | Compact | Default | Comfortable |
|---|---|---|---|
| `Base00` | 0 | 0 | 0 |
| `Base01` | 1px | 1px | 2px |
| `Base02` | 1px | 2px | 4px |
| `Base03` | 2px | 3px | 4px |
| `Base04` | 2px | **4px** | 6px |
| `Base06` | 3px | **6px** | 8px |
| `Base08` | 4px | **8px** | 10px |
| `Base12` | 10px | **12px** | 14px |
| `Base16` | 14px | **16px** | 18px |
| `Base20` | 18px | **20px** | 22px |
| `Base24` | 20px | **24px** | 28px |
| `Base32` | 28px | **32px** | 36px |
| `Base40` | 36px | **40px** | 44px |
| `Base48` | 44px | **48px** | 52px |

Naming = the pixel value **at Default density**. All of these also scale with `ui_font_size` on top of density.

---

## 3. Typography (`ui/src/styles/typography.rs`, `theme/src/buffer_line_height.rs`)

**`TextSize`** (drives `.text_ui()` family of helpers) / **`LabelSize`** (same scale, used by the `Label` component):

| Variant | px | rem |
|---|---|---|
| `Large` | 16px | 1rem |
| `Default` | 14px | 0.875rem *(Zed's own doc‑comment says 0.825rem — that's a typo in‑repo; the code computes 0.875)* |
| `Small` | 12px | 0.75rem |
| `XSmall` | 10px | 0.625rem |
| `Ui` / `Editor` | = user's `ui_font_size` / `buffer_font_size` setting | — |

**`HeadlineSize`** (all use `line-height: 1.6`):

| Variant | rem | ≈px |
|---|---|---|
| `XSmall` | 0.88 | 14 |
| `Small` | 1.0 | 16 |
| `Medium` (default) | 1.125 | 18 |
| `Large` | 1.27 | 20 |
| `XLarge` | 1.43 | 23 |

**Buffer / menu line-height** (`BufferLineHeight`): `Comfortable` (default) = **1.618×** · `Standard` = **1.3×** · `Custom(f32)`. Context menus always force `Comfortable` regardless of the user's buffer setting.

---

## 4. Icon sizes (`ui/src/components/icon.rs`)

| `IconSize` | Icon px | Padding (→ square hit‑target) | Total square |
|---|---|---|---|
| `Indicator` | 10px | `Base00` = 0 | 10px |
| `XSmall` | 12px | `Base02` = 2px | 16px |
| `Small` | 14px | `Base02` = 2px | 18px |
| `Medium` (default) | 16px | `Base02` = 2px | 20px |
| `XLarge` | 48px | `Base02` = 2px | 52px |
| `Custom(Rems)` | — | fixed, no padding | — |

*(No `Large` variant currently exists — jump is Medium 16px → XLarge 48px.)*

---

## 5. Buttons (`ui/src/components/button/*.rs`)

**`ButtonSize`** (height):

| Variant | Height | Horizontal padding |
|---|---|---|
| `Large` | 32px | `Base08` = 8px |
| `Medium` | 28px | `Base08` = 8px |
| `Default` | **22px** | `Base04` = 4px |
| `Compact` | 18px | `Base04` = 4px |
| `None` | 16px | `px_px()` = **1px** (fixed, unscaled) |

| Other | Value |
|---|---|
| Icon ↔ label gap | `Base04` = 4px |
| Label ↔ keybinding gap | `Base06` = 6px |
| Corner radius | `rounded_sm` = 4px (per‑corner; grouped buttons round only outer corners) |
| Border (Outlined styles) | 1px |
| `IconButton` square‑shape size | = `IconSize` square total (table above), overrides normal button width/height |

---

## 6. List items (`ui/src/components/list/list_item.rs`)

No single fixed row‑height constant — height = text line‑height + spacing variant. Dense rows land close to **22px**, which is why `ButtonSize::Default` is *also* 22px (its doc comment says it's sized to align non‑button rows with buttons).

| Property | Value |
|---|---|
| Horizontal padding | `Base06` = 6px |
| Outer padding (`.inset(true)`) | `Base04` = 4px |
| Indent per nesting level | 12px (`indent_step_size`, fixed, overridable) |
| Icon/start‑slot ↔ content gap | `gap_1` = 4px |
| Content‑slot internal gap | `Base06` = 6px |
| **Spacing** — `Dense` (default) | +0px |
| **Spacing** — `ExtraDense` | `py_neg_px` = **−1px** each side (−2px total) |
| **Spacing** — `Sparse` | `py_1` = +4px each side (+8px total) |
| Rounding (`.rounded()` / `.outlined()`) | `rounded_sm` = 4px |

---

## 7. Popovers, dropdowns & context menus

Three layered pieces: **`Popover`** (bare elevated box) → **`ContextMenu`** (the list‑based menu almost everything uses) → **`PopoverMenu`** (positions either against a trigger).

### `Popover` (`ui/src/components/popover.rs`)
| Property | Value |
|---|---|
| Vertical padding | `POPOVER_Y_PADDING` = **8px total** (4px top + 4px bottom) |
| Horizontal padding | **none** — supplied by children (e.g. a `ListItem`'s 6px) |
| Elevation | `elevation_2` (`ElevatedSurface`) |
| Aside (side panel) padding | `px_1` = 4px horizontal only |
| Main ↔ aside gap | `gap_1` = 4px |

### `ContextMenu` (`ui/src/components/context_menu.rs`)
| Property | Value |
|---|---|
| Min width (no `.fixed_width()`) | **200px** |
| Max height | **75%** of window viewport height |
| Default entry icon size | `IconSize::Small` (14px) |
| Link‑entry icon size | `IconSize::XSmall` (12px) |
| Documentation aside width | **384px** (window ≳800px wide) / **192px** (narrower) |
| Line height | forced `1.618×` (ignores buffer setting) |

### Positioning (`PopoverMenu`, `popover_menu.rs`)
| Property | Value |
|---|---|
| Default offset from trigger | **5px** (documented as 4px padding + 1px border compensation) |
| Snap‑to‑window‑edge margin | **8px** |

---

## 8. Modals (`ui/src/components/modal.rs`)

| Property | Value |
|---|---|
| Header padding | `px` 12px (`Base12`) · `pt` 8px (`Base08`) · `pb` 4px (`Base04`) |
| Header gap | `Base08` = 8px |
| Body/section gap | `Base08` = 8px |
| Section padding (`new_contained`) | `Base12` = 12px |
| Section padding (default, padded) | 12px (`Base06` + `Base06`) |
| Section row gap (default) | `Base04` = 4px |
| Section header row height | `h_7` = **28px** |
| Footer padding | `Base08` = 8px, all sides |
| Footer gap | `gap_1` = 4px |
| Footer border | 1px top |
| Elevation | `elevation_3` (`ModalSurface`) |

---

## 9. Tooltips (`ui/src/components/tooltip.rs`)

| Property | Value |
|---|---|
| Outer offset (keeps tooltip off the cursor) | `pl_2` 8px left · `pt_2p5` 10px top |
| Inner padding | `py_1` 4px vertical · `px_2` 8px horizontal |
| Title max‑width | `max_w_72` = **288px** |
| Title ↔ keybinding gap | `gap_4` = 16px |
| Elevation | `elevation_2` (`ElevatedSurface`) |

---

## 10. Text input — `InputField` (`ui_input/src/input_field.rs`)

| Property | Value |
|---|---|
| Min width | **192px** |
| Min height | `min_h_8` = **32px** |
| Padding | `px_2` 8px horizontal · `py_1p5` 6px vertical |
| Label/icon ↔ field gap | `gap_1` = 4px |
| Border | 1px |
| Corner radius | `rounded_md` = 6px |
| Default label size | `LabelSize::Small` (12px) |

---

## 11. Elevation & shadow — `ElevationIndex` (`ui/src/styles/elevation.rs`, `traits/styled_ext.rs`)

| Layer | Used for | Shadow |
|---|---|---|
| `Background` | App background, behind panels | none |
| `Surface` | Title bar, panel, tab bar, editor chrome | none |
| `EditorSurface` | Buffer/editor content area | none |
| `ElevatedSurface` | Popovers, palettes, notifications, floating windows/panels | 2 layers ↓ |
| `ModalSurface` | Modals, dialogs, settings, wizards | 4 layers ↓ |

All elevated surfaces get `rounded_lg` (8px) + 1px border (`_borderless` variants drop the border).

**`ElevatedSurface` shadow:**

| # | Y‑offset | Blur | Color |
|---|---|---|---|
| 1 | 2px | 3px | `rgba(0,0,0,.12)` |
| 2 | 1px | 0 | `rgba(0,0,0,.03)` light / `.06` dark |

**`ModalSurface` shadow:**

| # | Y‑offset | Blur | Color |
|---|---|---|---|
| 1 | 2px | 3px | `rgba(0,0,0,.06)` light / `.12` dark |
| 2 | 3px | 6px | `rgba(0,0,0,.06)` light / `.08` dark |
| 3 | 6px | 12px | `rgba(0,0,0,.04)` |
| 4 | 1px | 0 | `rgba(0,0,0,.04)` light / `.12` dark |

---

## 12. Motion (`ui/src/styles/animation.rs`)

| `AnimationDuration` | ms |
|---|---|
| `Instant` | 50 |
| `Fast` (default for `animate_in_*`) | 150 |
| `Slow` | 300 |

Easing: `ease_out_quint`. Slide‑in distance: 40px (`animate_in_from_*` helpers).

---

## 13. Scrollbar (`ui/src/components/scrollbar.rs`)

| Property | Value |
|---|---|
| `ScrollbarStyle::Regular` width | **6px** |
| `ScrollbarStyle::Editor` width | **15px** |
| Padding around thumb | 4px |
| Track border | 1px |
| Minimum thumb length | 25px |
| Autohide delay / hide / show duration | 1000ms / 400ms / 50ms |

---

## Sources

- [`ui/src/styles/spacing.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/styles/spacing.rs) · [`ui_macros/src/dynamic_spacing.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui_macros/src/dynamic_spacing.rs) · [`theme/src/ui_density.rs`](https://github.com/zed-industries/zed/blob/main/crates/theme/src/ui_density.rs)
- [`gpui_macros/src/styles.rs`](https://github.com/zed-industries/zed/blob/main/crates/gpui_macros/src/styles.rs) · [`ui/src/styles/units.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/styles/units.rs)
- [`ui/src/styles/typography.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/styles/typography.rs) · [`ui/src/components/label/label_like.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/label/label_like.rs) · [`theme/src/buffer_line_height.rs`](https://github.com/zed-industries/zed/blob/main/crates/theme/src/buffer_line_height.rs)
- [`ui/src/components/icon.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/icon.rs)
- [`ui/src/components/button/button_like.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/button/button_like.rs) · [`.../icon_button.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/button/icon_button.rs)
- [`ui/src/components/list/list_item.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/list/list_item.rs)
- [`ui/src/components/popover.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/popover.rs) · [`popover_menu.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/popover_menu.rs) · [`context_menu.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/context_menu.rs)
- [`ui/src/components/modal.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/modal.rs) · [`tooltip.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/tooltip.rs)
- [`ui_input/src/input_field.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui_input/src/input_field.rs)
- [`ui/src/styles/elevation.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/styles/elevation.rs) · [`ui/src/traits/styled_ext.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/traits/styled_ext.rs)
- [`ui/src/styles/animation.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/styles/animation.rs) · [`ui/src/components/scrollbar.rs`](https://github.com/zed-industries/zed/blob/main/crates/ui/src/components/scrollbar.rs)
