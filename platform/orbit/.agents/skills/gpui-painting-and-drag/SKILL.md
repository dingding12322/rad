---
name: gpui-painting-and-drag
description: Custom painting and drag-and-drop in GPUI 0.2.2. Use when drawing with canvas(), building paths and quads, implementing the Element trait, applying gradients, or handling internal and OS-file drags with on_drag/on_drop/on_drag_move.
---

# GPUI Custom Painting & Drag-and-Drop

Verified against **gpui 0.2.2**. This covers drawing and interacting outside the
`div()` box: raw painting, custom elements, gradients, and drag-and-drop.

## Choosing a level of control

| Need | Use |
|---|---|
| Layout, borders, flat/gradient fills | styled `div()` — see `gpui-styling` |
| Draw into a known box (charts, sparklines, backdrops) | `canvas(prepaint, paint)` |
| Control layout *and* painting of one unit | `impl Element` |
| Custom hit-testing or gesture | `impl Element` + `window.insert_hitbox` |

Prefer `canvas` — implementing `Element` by hand is rarely necessary.

---

## canvas()

```rust
use gpui::{Bounds, Pixels, Window, canvas};

canvas(
    |bounds, window, cx| { /* prepaint: measure, return any T */ () },
    move |bounds, state, window, cx| { /* paint */ },
)
.size_full()
```

- **prepaint** returns a `T` that **paint** receives. Use it to do expensive work
  (text shaping, layout of sub-items) once and reuse it while painting.
- Both closures get the element's `bounds` — already absolute, window-space.
- `canvas` does not size itself. Give it a box: `.size_full()`, `.w(...)`, or an
  absolute/inset parent.
- Paint is clipped to the surrounding `content_mask` automatically; you do not mask
  yourself, but anything painted outside bounds is discarded.

### Primitive drawing calls

```rust
use gpui::{BorderStyle, px, quad, transparent_black, white};

// filled rect, optional rounded corners + border
window.paint_quad(quad(
    bounds,              // Bounds<Pixels>
    px(4.),              // corner radii: Pixels or Corners<Pixels>
    white(),             // background: impl Into<Background>  (solid or gradient)
    px(1.),              // border widths: Pixels or Edges<Pixels>
    transparent_black(), // border color: impl Into<Hsla>
    BorderStyle::Solid,  // BorderStyle::Solid | Dashed
));

// arbitrary path
window.paint_path(path, color);
```

`quad(...)` argument order is easy to get wrong — it is `(bounds, corner_radii,
background, border_widths, border_color, border_style)`. All six are required;
pass `px(0.)` and `transparent_black()` to omit border work.

Drawing order = call order. There is no z-index inside one canvas; paint back to front.

### PathBuilder

```rust
use gpui::{PathBuilder, px, point};

let mut builder = PathBuilder::stroke(px(2.));      // or PathBuilder::fill()
builder.move_to(point(px(0.), px(0.)));
builder.line_to(point(px(40.), px(0.)));
builder.cubic_bezier_to(point(px(80.), px(40.)), point(px(60.), px(0.)));
builder.arc_to(
    point(px(10.), px(10.)),   // radii
    px(0.),                    // x_rotation
    false,                     // large_arc
    true,                      // sweep
    point(px(20.), px(20.)),   // to
);
builder.add_polygon(&points, true);                 // closed polygon
builder.close();

let path = builder.build()?;                        // -> Result<Path<Pixels>, Error>
window.paint_path(path, color);
```

- Also available: `relative_arc_to`, `curve_to`, `dash_array(&[px(4.), px(4.)])`,
  `translate`, `scale`, `rotate`, `transform`.
- `build()` is fallible — a path that is not a valid polygon fails. Handle it or
  skip drawing; do not `unwrap()`.
- `PathBuilder::stroke(width)` + `.dash_array(...)` is how you draw dashed strokes.

---

## Custom Element

The trait contract, as used in this codebase (`crates/orbit-pi/src/shimmer.rs`):

```rust
use gpui::{
    App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style, Window, relative,
};

impl IntoElement for MyThing {
    type Element = Self;
    fn into_element(self) -> Self::Element { self }
}

impl Element for MyThing {
    type RequestLayoutState = ();          // rarely useful; usually ()
    type PrepaintState = MyMeasurement;    // expensive work, done once

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        // measure: shape text, compute per-run geometry, etc.
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // draw using `prepaint`; must not allocate layout or measure.
    }
}
```

Rules that matter:

- **`request_layout` must not paint, `paint` must not measure.** Everything you
  measured belongs in `PrepaintState`.
- Pass children through `window.request_layout(style, children, cx)` if the element
  has any — an empty iterator means you are a leaf.
- `id()` returning `None` means the element is unkeyed; return a real id if the
  element owns focus or cached state.
- The element participates in the inspector only if `inspector_id` is honoured; the
  boilerplate above is the minimum accepted shape.

---

## Gradients

```rust
use gpui::{linear_color_stop, linear_gradient};

div().bg(linear_gradient(
    180.,                                    // degrees, 0 = top, clockwise
    linear_color_stop(top, 0.),              // color, stop in 0.0..=1.0
    linear_color_stop(bottom, 1.),
))
```

- **Exactly two stops** in 0.2.2. No three-stop constructor.
- `angle`: `0.` points up, increasing values rotate clockwise — CSS semantics, so
  `180.` runs top→bottom (the vertical scrim used throughout this app) and `90.`
  runs left→right.
- `linear_gradient` returns a `Background`, so it works anywhere `.bg(...)` does —
  `div().bg(...)`, `StyleRefinement::default().bg(...)`, and `quad(..., background, ...)`.
- **There is no `radial_gradient` in 0.2.2.** For radial or multi-stop fills, paint
  with `canvas` + `paint_path` / `paint_quad`.

---

## Drag and drop

### Internal drags (drag a value between elements)

```rust
use gpui::{Point, Pixels};

// drag source — value is cloned into the drag session
div()
    .id(("item", ix))
    .cursor_move()
    .on_drag(DragInfo { ix }, |info: &DragInfo, position, window, cx| {
        cx.new(|_| DragGhost { info: *info, position })
        // returns the Entity<W> rendered under the cursor
    })

// drop target — typed by the dragged value
div()
    .id("drop-target")
    .on_drop(cx.listener(|this, info: &DragInfo, _window, cx| {
        this.accept(*info);
        cx.notify();
    }))
```

- The dragged type is the contract. `on_drop::<T>` only fires for a matching `T`.
- The constructor returns an `Entity<W>`; that entity renders the floating ghost, so
  it must implement `Render` and position itself from the `position` argument.
- `.cursor_move()` on the source is what signals draggability to the user. It maps to
  `CursorStyle::ClosedHand`; for an idle "grab me" affordance use
  `.cursor(CursorStyle::OpenHand)` and switch to `ClosedHand` while dragging.

### Drag-move / hover detection

```rust
use gpui::DragMoveEvent;

div()
    .id("composer")
    .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _window, cx| {
        let hovered = event.bounds.contains(&event.event.position);
        if hovered != this.hovered {
            this.hovered = hovered;
            cx.notify();
        }
    }))
```

`DragMoveEvent<T>` exposes `event` (`MouseMoveEvent`, with `.position`), `bounds`
(the element's own bounds), and `drag(cx) -> &T` for the dragged value.

**0.2.2 has no `on_drag_enter` / `on_drag_leave`.** `on_drag_move` fires for *every*
move during a drag, so enter and leave must be derived yourself by testing
`event.bounds.contains(&event.event.position)` and storing the boolean. This is
exactly how `crates/orbit-pi/src/app/composer_ops.rs` does it.

### OS file drags

An external file drag arrives as a `DragMoveEvent<ExternalPaths>`; `ExternalPaths`
carries the paths via `.paths() -> &[PathBuf]`. Handle them with the same
`on_drag_move` + `on_drop::<ExternalPaths>` pair — there is no separate OS-drop API.

---

## 0.2.2 API checklist

Present and verified:

`canvas`, `quad`, `PaintQuad`, `window.paint_quad`, `window.paint_path`,
`PathBuilder` (`stroke`/`fill`/`move_to`/`line_to`/`curve_to`/`cubic_bezier_to`/
`arc_to`/`relative_arc_to`/`add_polygon`/`close`/`dash_array`/`transform`/`build`),
`Path`, `PathStyle`, `BorderStyle`, `window.request_layout`, `linear_gradient`,
`linear_color_stop`, `on_drag`, `on_drop`, `on_drag_move`, `DragMoveEvent`,
`ExternalPaths`, `CursorStyle`/`.cursor_move()`.

Not in 0.2.2 — do not use:

`radial_gradient`, `on_drag_enter`, `on_drag_leave`, `DragDropEvent`,
`background-clip: text` equivalents, `gestures`, `spring`, `popover`, `anchor`.
