---
name: gpui-lists-and-media
description: Lists, scrolling, deferred overlays, and images in GPUI 0.2.2. Use when rendering scrollable or virtualized lists, chat/transcript scrollers with tail following, nested scroll containers, popups that must paint above siblings, or embedded SVG/PNG assets.
---

# GPUI Lists, Scrolling & Media

Everything here is verified against **gpui 0.2.2**, the version this workspace pins.
These are the areas with the highest bug density and the least obvious API surface.

## Choosing a list primitive

| Primitive | Row heights | Use when |
|---|---|---|
| `uniform_list(id, count, f)` | All equal | Long, simple rows — settings lists, command results, pickers |
| `list(state, render_item)` | Variable | Chat/transcript, diffs, anything that measures real content |
| plain `div()` + `overflow_y_scroll()` | n/a | Short, bounded content (< ~50 rows) |

Both `uniform_list` and `list` virtualize: only the visible range is rendered, so
`.id()` on each row is required for stable state.

---

## uniform_list — fixed-height rows

```rust
use gpui::{div, prelude::*, px, uniform_list};

uniform_list("settings-select-list", rows.len(), move |range, _window, _cx| {
    let mut children = Vec::new();
    for ix in range {
        children.push(
            div()
                .id(ix)
                .h(px(30.))
                .w_full()
                .child(format!("row {ix}")),
        );
    }
    children
})
.h_full()
```

Rules:
- The callback receives a `Range<usize>` and returns `Vec<impl IntoElement>`, **not** an element.
- `uniform_list` sets `overflow.y = Scroll` on itself. Give it a bounded height
  (`.h_full()`, `.h(px(…))`, or `.max_h(…)`) or it grows to fit every row.
- Height is measured from the **first** item only. Mixed-height rows will be clipped.

### Programmatic scrolling

```rust
use gpui::{ScrollStrategy, UniformListScrollHandle};

uniform_list("list", count, f)
    .track_scroll(scroll_handle.clone())   // takes UniformListScrollHandle by value
// later
scroll_handle.scroll_to_item(ix, ScrollStrategy::Top);   // Top | Center | Bottom
```

`UniformListScrollHandle::scroll_to_item(ix, strategy)` only. There is no `scroll_to_end` —
pass `count - 1`.

---

## list + ListState — variable-height rows

```rust
use gpui::{div, list, ListAlignment, ListOffset, ListScrollEvent, ListState, px};

// once, in the entity constructor
let list = ListState::new(item_count, ListAlignment::Top, px(400.));

// in render
list(self.list.clone(), move |ix, _window, _cx| {
    div().id(ix).child(self.row(ix)).into_any_element()
})
```

`ListState::new(item_count, alignment, overdraw)`
- `alignment`: `ListAlignment::Top` (default document flow) or `ListAlignment::Bottom`
  (chat/transcript — pins the last row to the viewport bottom).
- `overdraw`: extra pixels rendered beyond the viewport, to cover fast scrolling.
  `px(400.)` is the value used throughout this repo.

### ListState surface (complete)

```rust
// content
list.reset(element_count);                      // reset to N rows (no scroll change)
list.splice(old_range, count);                  // replace rows; clears offsets in range
list.item_count();                              // usize
let list = list.measure_all();                 // builder: consumes self, returns Self

// scrolling
list.scroll_to(ListOffset { item_ix, offset_in_item });
list.scroll_by(distance);                       // relative
list.logical_scroll_top() -> ListOffset;
list.scroll_to_reveal_item(ix);
list.bounds_for_item(ix) -> Option<Bounds<Pixels>>;
list.viewport_bounds() -> Bounds<Pixels>;
list.max_offset_for_scrollbar() -> Size<Pixels>;

// observers
list.set_scroll_handler(|event: &ListScrollEvent, window, cx| { … });
```

`ListOffset { item_ix: usize, offset_in_item: Pixels }` — the scroll anchor.
`ListScrollEvent { visible_range: Range<usize>, count: usize, is_scrolled: bool }`.

### The scroll handler does NOT fire on programmatic scrolls

`scroll_to`, `scroll_by`, `scroll_to_reveal_item`, and `reset` bypass
`set_scroll_handler`. If you mirror scroll state into your own fields, update them
by hand after any programmatic call or they desync.

### Tail following (chat / transcript)

**`FollowMode::Tail` does not exist in 0.2.2.** The equivalent is
`ListAlignment::Bottom` plus a scroll to a *past-the-end* offset:

```rust
list.scroll_to(ListOffset {
    item_ix: item_count,
    offset_in_item: px(0.),
});
```

Detect "reader left the live edge" from the scroll handler — under
`ListAlignment::Bottom`, `event.is_scrolled` means the reader has scrolled away
from the tail:

```rust
list.set_scroll_handler(move |event: &ListScrollEvent, _, cx| {
    following_tail.set(!event.is_scrolled);
    visible_start.set(event.visible_range.start);
    cx.refresh_windows();
});
```

While tail-following, `logical_scroll_top()` reads **past the end**, so it is not a
usable anchor. To anchor the visible top row, move back one viewport first:

```rust
let viewport = list.viewport_bounds().size.height;
list.scroll_by(-viewport);
let anchor = list.logical_scroll_top();   // now a real item
```

### splice() zeroes the anchor — restore it

`splice(range, count)` clears `offset_in_item` **when the anchor lies inside the
invalidated range**, snapping that row's top to the viewport top. This is the classic
"it jumps when I open a card in the message I'm reading" bug.

Capture and re-apply the anchor around every splice that could contain it:

```rust
let anchor = list.logical_scroll_top();
list.splice(ix..ix + 1, 1);
list.scroll_to(anchor);
```

For **remeasuring the same rows** (streaming growth, expand/collapse), pass an
identity splice — `splice(range, range.len())` — then restore the anchor. Use the
same pattern for `reset`.

### Appending without moving the reader

```rust
// only chase the bottom if the reader is already there
list.splice(old_range, count);
if following_tail.get() {
    list.scroll_to(ListOffset { item_ix: list.item_count(), offset_in_item: px(0.) });
}
```

See `crates/orbit-pi/src/message_scroller.rs` for the full production version of
tail following, prepend-without-jump, and streaming remeasure.

---

## Scroll containers

```rust
div().id("scroll-area").overflow_scroll()      // both axes
div().id("scroll-area").overflow_y_scroll()    // vertical only
```

- **`.id()` is required** on any scrollable container, or scroll state is lost on re-render.
- Nesting two scroll containers (vertical outer, horizontal inner) works — see
  `gpui-0.2.2/examples/scrollable.rs`.
- Attach a `ScrollHandle` when you need to read or drive position:

```rust
let handle = ScrollHandle::new();
div().id("x").overflow_y_scroll().track_scroll(&handle)
// handle.offset(), handle.max_offset(), handle.bounds_for_item(ix), handle.top_item()
```

---

## deferred() — paint above siblings

GPUI paints in tree order, so a child cannot paint over a later sibling. `deferred`
keeps the child's **layout** in place but delays its **paint** until after its
ancestors, drawing it on top.

```rust
use gpui::{deferred, px};

deferred(popup).with_priority(8)
```

- `with_priority(usize)` / `.priority(usize)` — higher draws closer to the viewer.
  Use ascending priorities for stacked layers (menu = 8, tooltip = 9, modal = 10).
- `deferred` must still be given real bounds by its parent — it does not position itself.
  Pair it with an absolute/inset container.
- Use for: popups, dropdowns, command palettes, tooltips, drag ghosts — anything that
  visually escapes its parent's box.

---

## Images and SVG

### The embedded-asset trap

`img("name.png")` is a **trap** for compile-time embedded assets. gpui's
`From<&str>` runs the string through a URI heuristic, and a slashless filename parses
as a valid URI *authority*, so it is fetched over the network instead of being loaded
from the app's `AssetSource`.

```rust
use gpui::{ImageSource, Resource, img};

// correct — stays on the asset-source path
img(ImageSource::Resource(Resource::Embedded(path.into())))

// wrong — silently goes to the network
img("logo.png")
```

`ImageSource` variants: `Resource(Resource)`, `Render(Arc<RenderImage>)`,
`Image(Arc<Image>)`, `Custom(Arc<dyn Fn(&mut Window, &mut App) -> …>)`.

Use `ImageSource::Image(...)` for already-decoded/in-memory images, and
`Resource::Embedded(path)` for anything shipped in the binary.

### Sizing

An `Img` has no intrinsic size — it fills its box, so size it explicitly:

```rust
img(ImageSource::Resource(Resource::Embedded("logo.svg".into())))
    .h(px(24.))
    .flex_none()                    // stop flex from stretching it
    .object_fit(ObjectFit::Contain) // Fill | Contain | Cover | ScaleDown | None
```

`ObjectFit::Cover` for thumbnails/avatars, `Contain` for logos and icons that must
not crop, `ScaleDown` for previews that should never upscale.

For SVG icons use `svg().path(...)` — it is recolourable via `.text_color(...)`, unlike a
raster `img`.

---

## 0.2.2 API checklist

Present and verified in this version:

`uniform_list`, `UniformListScrollHandle`, `ScrollStrategy`, `list`, `ListState`,
`ListAlignment`, `ListOffset`, `ListScrollEvent`, `ScrollHandle`, `overflow_scroll`,
`overflow_y_scroll`, `track_scroll`, `deferred`, `Deferred::with_priority`, `img`,
`ImageSource`, `Resource::Embedded`, `ObjectFit`, `svg`.

Not in 0.2.2 — do not use:
`FollowMode` / `FollowMode::Tail`, `ListState::scroll_to_end`,
`Deferred::priority` as a constructor arg, `popover`, `anchor`, `spring`,
`gestures`, `hot_reload`, `profiler`, `debug_overlay`, `_accessibility`.
These live in Zed's `main` branch and will not compile here.
