use super::*;
use crate::theme::tokens::{Radius, TextSize};
use crate::theme::ThemeId;
use gpui::size;

fn solid_image(width: u32, height: u32) -> std::sync::Arc<gpui::RenderImage> {
    let buffer = image::RgbaImage::from_pixel(width, height, image::Rgba([120, 120, 120, 255]));
    std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(buffer)]))
}

/// gpui's `ObjectFit::Cover` scales the image up and centers it, so the
/// painted bounds overflow the element whenever the ratios differ (a 16:9
/// image in a narrower main area). Without a clip on the wrapper, that
/// overflow painted over the sessions sidebar. This documents the gpui
/// behavior the wrapper's `overflow_hidden` guards against.
#[test]
fn cover_bounds_overflow_the_element_on_a_ratio_mismatch() {
    let element = gpui::bounds(point(px(240.), px(0.)), size(px(1000.), px(700.)));
    let painted = ObjectFit::Cover.get_bounds(
        element,
        size(gpui::DevicePixels(1600), gpui::DevicePixels(900)),
    );
    assert!(
        painted.origin.x < element.origin.x,
        "cover must overflow left (painted {:?} vs element {:?})",
        painted.origin.x,
        element.origin.x
    );
    assert!(painted.size.width > element.size.width);
}

/// The backdrop wrapper fills the main area exactly — it must never claim
/// the sidebar's column, however the image inside it is fitted.
#[gpui::test]
fn backdrop_wrapper_stays_inside_the_main_area(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1240.), px(700.)), |_, _| {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("backdrop-test-sidebar")
                    .debug_selector(|| "backdrop-test-sidebar".to_string())
                    .flex_none()
                    .w(px(240.))
                    .h_full(),
            )
            .child(
                div()
                    .id("backdrop-test-main")
                    .debug_selector(|| "backdrop-test-main".to_string())
                    .flex_1()
                    .h_full()
                    .relative()
                    .children(OrbitApp::backdrop_image(
                        Some(solid_image(1600, 900)),
                        crate::dither::Tuning::default(),
                        Theme::for_id(ThemeId::Orbit),
                    )),
            )
    });
    let sidebar = cx.debug_bounds("backdrop-test-sidebar").expect("sidebar");
    let main = cx.debug_bounds("backdrop-test-main").expect("main");
    assert_eq!(sidebar.origin.x, px(0.));
    assert_eq!(sidebar.size.width, px(240.));
    assert_eq!(main.origin.x, px(240.), "main starts after the sidebar");
    assert_eq!(main.size.width, px(1000.));
}

/// Renders just the blur slider so its geometry can be read off the painted
/// tree. The real `OrbitApp` is the value holder, exactly as in Settings.
struct BlurSliderProbe {
    app: Entity<OrbitApp>,
    sigma: f32,
}

impl Render for BlurSliderProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *theme::get(cx);
        let this = self.app.clone();
        let sigma = self.sigma;
        self.app
            .update(cx, |app, _| app.blur_slider(theme, this, sigma))
    }
}

/// The `Img` element sets its own `aspect_ratio` from the source. Inside a
/// `size_full()` backdrop that must not win over the definite size the
/// wrapper gives it — if it does, `ObjectFit::Cover` scales relative to the
/// wrong box and the picture is placed/cropped wrong.
#[gpui::test]
fn backdrop_img_box_matches_its_wrapper(cx: &mut gpui::TestAppContext) {
    use gpui::{point, size};

    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1075.), px(740.)), |_, _| {
        div()
            .id("backdrop-img-wrap")
            .debug_selector(|| "backdrop-img-wrap".to_string())
            .absolute()
            .inset_0()
            .overflow_hidden()
            .child(
                img(ImageSource::Render(solid_image(1600, 900)))
                    .id("backdrop-img")
                    .debug_selector(|| "backdrop-img".to_string())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
    });
    let wrap = cx.debug_bounds("backdrop-img-wrap").expect("wrapper");
    let image = cx.debug_bounds("backdrop-img").expect("image");
    assert_eq!(
        image.size, wrap.size,
        "the image box must be the wrapper's box, not the source aspect ratio"
    );
}

/// The blur slider's handle and fill are driven by the tuning's sigma: the
/// handle's left edge is the normalized position across the usable track
/// (track minus the knob), so 0 % is flush left and 100 % is flush right, and
/// the accent fill always ends at the handle's centre. Pin the geometry so a
/// future restyle cannot silently desync the control from the value it scrubs.
#[gpui::test]
fn blur_slider_handle_tracks_sigma(cx: &mut gpui::TestAppContext) {
    use crate::dither::BLUR_MAX;
    use crate::theme::{Theme, ThemeId};

    cx.update(|cx| cx.set_global(Theme::for_id(ThemeId::Orbit)));
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| cx.new(OrbitApp::new));

    for (sigma, expected) in [(0., 0.), (BLUR_MAX / 2., 0.5), (BLUR_MAX, 1.)] {
        let _ = cx.draw(point(px(0.), px(0.)), size(px(320.), px(60.)), |_, cx| {
            cx.new(|_| BlurSliderProbe {
                app: app.clone(),
                sigma,
            })
        });
        let track = cx.debug_bounds("backdrop-blur-slider").expect("track");
        let knob = cx.debug_bounds("backdrop-blur-knob").expect("knob");
        let fill = cx.debug_bounds("backdrop-blur-fill").expect("fill");
        let usable = f32::from(track.size.width) - f32::from(knob.size.width);
        let offset = f32::from(knob.origin.x) - f32::from(track.origin.x);
        assert!(
            (offset - usable * expected).abs() < 1.,
            "sigma {sigma} put the knob at {offset}px, expected {}px",
            usable * expected
        );
        let centre = offset + f32::from(knob.size.width) / 2.;
        assert!(
            (f32::from(fill.size.width) - centre).abs() < 1.,
            "the accent fill must end at the knob centre: {} vs {centre}",
            f32::from(fill.size.width)
        );
    }
}

/// The bottom fade is how the picture drops into the page, and its height is
/// exactly what the Settings → Appearance control writes. Pin the wiring:
/// 0.22 of a 700 px column is 154 px, anchored to the bottom edge.
#[gpui::test]
fn backdrop_fade_tracks_the_tuning(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1000.), px(700.)), |_, _| {
        div()
            .size_full()
            .relative()
            .children(OrbitApp::backdrop_image(
                Some(solid_image(1600, 900)),
                crate::dither::Tuning {
                    blur: 0.,
                    cell: 4,
                    fade: crate::dither::FADE_HEIGHTS[3],
                },
                Theme::for_id(ThemeId::Orbit),
            ))
    });
    let fade = cx.debug_bounds("backdrop-fade").expect("fade painted");
    assert_eq!(fade.origin.y, px(546.), "fade must sit on the bottom edge");
    assert_eq!(fade.size.height, px(154.), "fade height follows the tuning");
}

/// "None" is the real off switch for the gradient, not a zero-height ghost
/// layer — DESIGN.md allows exactly one backdrop gradient, and a user who
/// turns it off should not pay for it.
#[gpui::test]
fn backdrop_fade_none_paints_no_gradient(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1000.), px(700.)), |_, _| {
        div()
            .size_full()
            .relative()
            .children(OrbitApp::backdrop_image(
                Some(solid_image(1600, 900)),
                crate::dither::Tuning {
                    blur: 0.,
                    cell: 4,
                    fade: 0.,
                },
                Theme::for_id(ThemeId::Orbit),
            ))
    });
    assert!(
        cx.debug_bounds("backdrop-fade").is_none(),
        "fade None must not paint a gradient layer"
    );
}

/// The new-task backdrop must span the whole chat column below the top bar.
/// It used to live inside the empty-state slot — the `flex_1` region *above*
/// the composer column — so the image stopped at the composer's top edge and
/// left the bottom of the window on a flat slab at every fade setting,
/// including "None". This pins the layer to the column instead, so the
/// floating composer and the status bar below it paint over the picture.
#[gpui::test]
fn new_task_backdrop_spans_the_composer_region(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    let app = cx.update(|_, cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1000.), px(700.)), |_, cx| {
        let theme = Theme::for_id(ThemeId::Orbit);
        let backdrop = app.update(cx, |app, _| app.new_task_backdrop(theme, 0., true));
        div()
            .size_full()
            .flex()
            .flex_col()
            .relative()
            .child(
                div()
                    .id("test-top-bar")
                    .debug_selector(|| "test-top-bar".to_string())
                    .flex_none()
                    .h(px(super::view::TOP_BAR_H))
                    .w_full(),
            )
            .children(backdrop)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("test-composer")
                            .debug_selector(|| "test-composer".to_string())
                            .flex_none()
                            .h(px(125.))
                            .w_full(),
                    ),
            )
    });

    let backdrop = cx
        .debug_bounds("new-task-backdrop")
        .expect("backdrop layer laid out");
    let composer = cx.debug_bounds("test-composer").expect("composer");
    assert_eq!(
        backdrop.origin.y,
        px(super::view::TOP_BAR_H),
        "backdrop must start below the top bar"
    );
    assert_eq!(
        backdrop.origin.y + backdrop.size.height,
        px(700.),
        "backdrop must run to the window's bottom edge"
    );
    assert!(
        backdrop.origin.y + backdrop.size.height > composer.origin.y,
        "backdrop must extend below the composer's top edge: {backdrop:?} vs {composer:?}"
    );
}

/// The field's clock runs only while the new-task page is on screen, and never
/// under Reduce Motion. Those are the two ways an animated backdrop goes wrong:
/// repainting a page nobody is looking at, and ignoring the platform's motion
/// preference.
#[gpui::test]
fn the_backdrop_clock_runs_only_on_the_new_task_page(cx: &mut gpui::TestAppContext) {
    let app = cx.update(|cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        cx.new(OrbitApp::new)
    });
    // Not on screen: the heartbeat must not even start the clock.
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.backdrop_live = false;
            app.tick(cx);
            assert!(
                app.backdrop_clock.is_none(),
                "an off-screen backdrop keeps no clock"
            );
            assert_eq!(app.backdrop_phase, 0., "and no phase");
        })
    });

    // On screen: the phase follows wall-clock time.
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.backdrop_live = true;
            app.tick(cx);
            assert!(app.backdrop_clock.is_some(), "the clock starts");
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(25));
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.tick(cx);
            assert!(
                app.backdrop_phase > 0.,
                "the phase advanced with time: {}",
                app.backdrop_phase
            );
        })
    });

    // Reduce Motion freezes it on the frame it was showing.
    let frozen = cx.update(|cx| app.read(cx).backdrop_phase);
    cx.update(|cx| {
        let mut theme = Theme::for_id(ThemeId::Orbit);
        theme.ui.reduce_motion = true;
        cx.set_global(theme);
    });
    std::thread::sleep(std::time::Duration::from_millis(25));
    cx.update(|cx| {
        app.update(cx, |app, cx| {
            app.tick(cx);
            assert_eq!(
                app.backdrop_phase, frozen,
                "Reduce Motion holds the field still"
            );
            assert!(app.backdrop_clock.is_none(), "and drops the clock");
        })
    });
}

/// The chat page deliberately has no backdrop.
#[gpui::test]
fn new_task_backdrop_is_absent_on_a_session(cx: &mut gpui::TestAppContext) {
    let cx = cx.add_empty_window();
    cx.update(|_, cx| {
        cx.set_global(Theme::for_id(ThemeId::Orbit));
        let app = cx.new(OrbitApp::new);
        assert!(
            app.update(cx, |app, _| app.new_task_backdrop(
                Theme::for_id(ThemeId::Orbit),
                0.,
                false
            ))
            .is_none(),
            "a session page has no backdrop"
        );
    });
}

/// Regression: a provider card's name/id column must get a real width.
/// `truncate()` collapses to a bare ellipsis when the text element's width
/// resolves to 0 — which happens when `w_full()`/`min_w_0()` are set on the
/// text inside a `flex_1` column. Relying on the parent's stretch is what
/// keeps the name visible.
/// Regression: a provider card's name/id column must get a real width.
/// `truncate()` renders a bare ellipsis when its element resolves to zero
/// width, which happens when `w_full()` is used inside a flex-grow column.
/// The card relies on the parent's stretch and gives the name the full
/// width (the status pill lives on the badges row, not beside the name).
#[gpui::test]
fn provider_name_column_has_a_real_width(cx: &mut gpui::TestAppContext) {
    use gpui::{point, size};

    struct CardHeaderTestView;
    impl Render for CardHeaderTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_id(ThemeId::Orbit);
            // One 3-column grid cell's worth of width.
            div()
                .w(px(320.))
                .flex()
                .flex_col()
                .p(px(14.))
                .gap_2p5()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_3()
                        .child(div().size(px(52.)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .id("provider-name")
                                        .debug_selector(|| "provider-name".to_string())
                                        .text_size(TextSize::Default.px(&theme))
                                        .truncate()
                                        .child("Amazon Bedrock"),
                                )
                                .child(
                                    div()
                                        .id("provider-id")
                                        .debug_selector(|| "provider-id".to_string())
                                        .text_size(theme.code_px(10.5))
                                        .truncate()
                                        .child("amazon-bedrock"),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("provider-endpoint")
                        .debug_selector(|| "provider-endpoint".to_string())
                        .text_size(theme.code_px(10.5))
                        .truncate()
                        .child("https://api.example.com/v1 · openai-completions"),
                )
        }
    }

    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1000.), px(300.)), |_, cx| {
        cx.new(|_| CardHeaderTestView)
    });
    let name = cx.debug_bounds("provider-name").expect("name laid out");
    let id = cx.debug_bounds("provider-id").expect("id laid out");
    let endpoint = cx
        .debug_bounds("provider-endpoint")
        .expect("endpoint laid out");
    assert!(
        name.size.width > px(150.),
        "provider name collapsed to an ellipsis: {:?}",
        name.size
    );
    assert!(
        id.size.width > px(150.),
        "provider id collapsed: {:?}",
        id.size
    );
    assert!(
        endpoint.size.width > px(250.),
        "provider endpoint collapsed: {:?}",
        endpoint.size
    );
}

/// Regression: the Agent tab's long card descriptions blew the settings
/// content column past the window. A text element's min-content width is
/// its full unwrapped line in gpui, so a `flex_1` column with default
/// `min-width: auto` refuses to shrink and the card (and its toggle)
/// overflow off-screen. `min_w_0` on the settings root and content column
/// lets the column take the window width; the description then wraps.
#[gpui::test]
fn settings_cards_stay_inside_the_content_column(cx: &mut gpui::TestAppContext) {
    use gpui::{point, size};

    struct SettingsCardTestView;
    impl Render for SettingsCardTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_id(ThemeId::Orbit);
            let card = div()
                    .id("settings-card")
                    .debug_selector(|| "settings-card".to_string())
                    .bg(theme.surface_secondary)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Radius::XLarge.px(&theme))
                    .px(px(14.))
                    .py(px(12.))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(TextSize::Default.px(&theme))
                                    .child("Follow-up messages"),
                            )
                            .child(
                                div()
                                    .id("settings-card-desc")
                                    .debug_selector(|| "settings-card-desc".to_string())
                                    .text_size(TextSize::Small.px(&theme))
                                    .child("Messages sent while the agent is running wait in the queue above the composer and are delivered once the current task finishes. All delivers the whole queue at once; One at a time delivers one per run."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().h(px(28.)).px(px(10.)).child("One at a time"))
                            .child(div().h(px(28.)).px(px(10.)).child("All")),
                    );

            div()
                .size_full()
                .flex()
                .child(div().flex_none().w(px(240.)).h_full())
                .child(
                    div()
                        .id("settings-content-column")
                        .debug_selector(|| "settings-content-column".to_string())
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("settings-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .child(
                                    div()
                                        .w_full()
                                        .px(px(24.))
                                        .flex()
                                        .flex_col()
                                        .gap_3()
                                        .child(card),
                                ),
                        ),
                )
        }
    }

    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1240.), px(700.)), |_, cx| {
        cx.new(|_| SettingsCardTestView)
    });
    let card = cx.debug_bounds("settings-card").expect("card");
    let desc = cx.debug_bounds("settings-card-desc").expect("description");
    let content = cx
        .debug_bounds("settings-content-column")
        .expect("content column");
    assert_eq!(
        content.size.width,
        px(1000.),
        "content column must take the space beside the 240 px nav"
    );
    assert!(
        card.origin.x + card.size.width <= px(1240.),
        "card overflows the window: {:?}",
        card
    );
    assert!(
        desc.size.height > px(30.),
        "long description must wrap to multiple lines, got {:?}",
        desc.size
    );
    assert!(
        desc.size.width < card.size.width,
        "description must be narrower than the card so the control fits: {:?}",
        desc.size
    );
}

/// Regression: the Providers page's 3-column grid blew past the content
/// column and clipped its third column off the window. `grid_cols(3)` is
/// `repeat(3, minmax(0, 1fr))`, but `1fr` resolves against max-content when
/// the grid's parent is given only `w_full().max_w()` *inside* a vertical
/// scroll container — the tracks size to the widest card instead of the
/// column. The max-width belongs on the scroll container itself, so the
/// grid is laid out against a definite width and the columns stay equal.
#[gpui::test]
fn provider_grid_columns_fit_inside_the_content_column(cx: &mut gpui::TestAppContext) {
    use gpui::{point, size};

    const NAMES: [&str; 3] = ["grid-card-0", "grid-card-1", "grid-card-2"];

    struct ProviderGridTestView;
    impl Render for ProviderGridTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_id(ThemeId::Orbit);
            let card = |i: usize| {
                let name = NAMES[i];
                div()
                    .id(ElementId::Name(name.to_string().into()))
                    .debug_selector(move || name.to_string())
                    .w_full()
                    .min_w_0()
                    .bg(theme.surface_secondary)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(Radius::XLarge.px(&theme))
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(TextSize::Default.px(&theme))
                            .child(format!("Provider {i}")),
                    )
                    .child(
                        div()
                            .font_family(theme::code_font_family())
                            .text_size(theme.code_px(10.5))
                            .truncate()
                            .child("https://api.example.com/v1 · openai-completions plus a long trailing note"),
                    )
            };

            div()
                .size_full()
                .flex()
                .child(div().flex_none().w(px(240.)).h_full())
                .child(
                    div().flex_1().min_w_0().min_h_0().flex().flex_col().child(
                        div()
                            .id("grid-scroll")
                            .debug_selector(|| "grid-scroll".to_string())
                            .flex_1()
                            .min_h_0()
                            .w_full()
                            .max_w(px(CONTENT_MAX_W))
                            .mx_auto()
                            .overflow_y_scroll()
                            .child(
                                div().w_full().px(px(24.)).flex().flex_col().gap_3().child(
                                    div()
                                        .w_full()
                                        .grid()
                                        .grid_cols(3)
                                        .gap_3()
                                        .children((0..3).map(card)),
                                ),
                            ),
                    ),
                )
        }
    }

    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(1240.), px(700.)), |_, cx| {
        cx.new(|_| ProviderGridTestView)
    });
    let scroll = cx.debug_bounds("grid-scroll").expect("scroll container");
    assert_eq!(scroll.size.width, px(CONTENT_MAX_W));
    for (i, name) in NAMES.iter().enumerate() {
        let card = cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("card {i} laid out"));
        assert!(
            card.size.width > px(250.),
            "grid column {i} collapsed: {:?}",
            card.size
        );
        assert!(
            card.size.width < px(400.),
            "grid column {i} blew past its share — `fr` resolved to max-content: {:?}",
            card.size
        );
        assert!(
            card.origin.x + card.size.width <= scroll.origin.x + scroll.size.width,
            "card {i} overflows the content column: {:?}",
            card
        );
    }
}

/// Regression: an SVG defaults to `flex-shrink: 1`, so beside wide text
/// (e.g. the transcript's activity label + copy button) it collapses to
/// zero width — the "icons render tiny" bug. `icon`/`icon_dyn`/`glyph`
/// set `flex_none`; this pins both halves of that behavior.
#[gpui::test]
fn flex_none_keeps_icons_from_collapsing(cx: &mut gpui::TestAppContext) {
    use gpui::{point, size};

    struct Probe;
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(120.))
                .h(px(40.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .whitespace_nowrap()
                        .child("a very long unwrapping label that will not shrink at all"),
                )
                .child(
                    gpui::svg()
                        .path("icons/copy.svg")
                        .id("probe-icon-fixed")
                        .debug_selector(|| "probe-icon-fixed".to_string())
                        .flex_none()
                        .size(px(16.)),
                )
                .child(
                    gpui::svg()
                        .path("icons/copy.svg")
                        .id("probe-icon-shrunk")
                        .debug_selector(|| "probe-icon-shrunk".to_string())
                        .size(px(16.)),
                )
        }
    }

    let cx = cx.add_empty_window();
    let _ = cx.draw(point(px(0.), px(0.)), size(px(400.), px(200.)), |_, cx| {
        cx.new(|_| Probe)
    });
    let fixed = cx.debug_bounds("probe-icon-fixed").expect("fixed icon");
    let shrunk = cx.debug_bounds("probe-icon-shrunk").expect("shrunk icon");
    assert_eq!(
        fixed.size.width,
        px(16.),
        "flex_none icon must keep its size: {:?}",
        fixed
    );
    assert!(
        shrunk.size.width < px(16.),
        "without flex_none the icon collapses: {:?}",
        shrunk
    );
}
