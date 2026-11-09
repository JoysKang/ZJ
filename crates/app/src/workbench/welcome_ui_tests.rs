//! A double click on the welcome page opens a new untitled file (as in VS Code); a single
//! click does nothing.

use super::test_support::{install_globals, new_window, open_window};
use super::*;
use crate::settings::Settings;
#[allow(unused_imports)]
use core::prelude::v1::test;
use gpui_kit::{
    MouseDownEvent, MouseUpEvent, TestAppContext, VisualTestContext, test::TestWindowExt,
};

fn click(cx: &mut VisualTestContext, position: Point<Pixels>, click_count: usize) {
    cx.simulate_event(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::default(),
        click_count,
    });
}

#[gpui_kit::test]
async fn a_double_click_on_the_welcome_page_opens_an_untitled_file(cx: &mut TestAppContext) {
    let (window, this) = open_window(cx, None, Settings::default());
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    // Below the welcome page's list, in the editor area.
    let blank = point(px(1000.), px(820.));
    click(cx, blank, 1);
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.documents.is_empty()));
    click(cx, blank, 2);
    cx.run_until_parked();
    this.read_with(cx, |p, _| {
        assert_eq!(p.documents.len(), 1);
        assert!(p.documents[0].untitled);
        assert_eq!(p.active, Pane::Document(p.documents[0].id));
    });
}

#[gpui_kit::test]
async fn restored_tabs_stay_in_sight_on_the_welcome_page(cx: &mut TestAppContext) {
    let (window, this) = open_window(cx, None, Settings::default());
    // Never read: the paths only have to name files.
    let tabs = vec![
        PathBuf::from("/zj-test/a.txt"),
        PathBuf::from("/zj-test/b.txt"),
    ];
    cx.update_window(window.into(), |_, window, cx| {
        this.update(cx, |p, cx| p.restore_tabs(tabs, None, window, cx));
        window.render_frame(cx);
    })
    .unwrap();
    let state = |cx: &mut VisualTestContext| {
        this.read_with(cx, |p, _| {
            (
                p.active == Pane::Welcome,
                p.pending_tabs.len(),
                p.shows_tab_bar(),
            )
        })
    };
    let cx = &mut VisualTestContext::from_window(window.into(), cx);
    assert_eq!(state(cx), (true, 2, true));
    // A new file joins them; closing it leaves them where they were.
    click(cx, point(px(1000.), px(820.)), 2);
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.documents.iter().all(|d| d.untitled)));
    assert_eq!(state(cx), (false, 2, true));
    cx.update(|window, cx| this.update(cx, |p, cx| p.close_editor(window, cx)));
    cx.run_until_parked();
    this.read_with(cx, |p, _| assert!(p.documents.is_empty()));
    assert_eq!(state(cx), (true, 2, true));
}

#[gpui_kit::test]
async fn editor_backgrounds_do_not_fill_the_workbench_corners(cx: &mut TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    let (handle, this) = open_window(cx, None, Settings::default());
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        for tabs in 0..3 {
            cx.update_window(handle.into(), |_, window, cx| {
                Theme::change(mode, Some(window), cx);
                this.update(cx, |p, cx| {
                    p.pending_tabs.clear();
                    p.active = Pane::Welcome;
                    match tabs {
                        1 => p.restore_tabs(
                            vec![PathBuf::from(format!("/zj-test/{}.txt", "long".repeat(200)))],
                            None,
                            window,
                            cx,
                        ),
                        2 => { p.new_untitled(window, cx); }
                        _ => {},
                    }
                    cx.notify();
                });
                window.simulate_mouse_move(point(px(0.), px(0.)), cx);
                window.render_frame(cx);
                window.render_frame(cx);
                let editor = window.find("workbench-editor").bounds();
                let scale = window.scale_factor();
                let shell = editor.scale(scale);
                let colors = theme::colors(cx);
                // Probe outside the current shell curve; keep this valid when its radius changes.
                // Rectangular overflow masks cannot hide child backgrounds there.
                let inset = theme::WORKBENCH_RADIUS / 4.;
                for (left, top) in [(true, true), (false, true), (true, false), (false, false)] {
                    if !top && tabs != 0 { continue; }
                    let probe = point(
                        if left { editor.left() + inset } else { editor.right() - inset },
                        if top { editor.top() + inset } else { editor.bottom() - inset },
                    ).scale(scale);
                    let offending = window.painted_quads().into_iter().find(|quad| {
                            // Kit's whole-window background is painted below the workbench.
                            // Only the shell and its child surfaces can protrude here.
                            if quad.bounds.left() < shell.left()
                                || quad.bounds.right() > shell.right()
                                || quad.bounds.top() < shell.top()
                                || quad.bounds.bottom() > shell.bottom()
                                || ![colors.editor, colors.tabs, colors.input_bg].iter().any(|color| quad.background == (*color).into())
                                || !quad.bounds.contains(&probe)
                                || !quad.content_mask.bounds.contains(&probe)
                            { return false; }
                            let dx = if left { probe.x - quad.bounds.left() } else { quad.bounds.right() - probe.x };
                            let dy = if top { probe.y - quad.bounds.top() } else { quad.bounds.bottom() - probe.y };
                            let radius = match (left, top) {
                                (true, true) => quad.corner_radii.top_left,
                                (false, true) => quad.corner_radii.top_right,
                                (true, false) => quad.corner_radii.bottom_left,
                                (false, false) => quad.corner_radii.bottom_right,
                            };
                            dx >= radius || dy >= radius
                                || (radius - dx).0.powi(2) + (radius - dy).0.powi(2) <= radius.0.powi(2)
                        });
                    assert!(
                        offending.is_none(),
                        "editor background protrudes from shell in {mode:?}, tabs={tabs}, left={left}, top={top}: {offending:#?}"
                    );
                }
            }).unwrap();
        }
    }
}

#[gpui_kit::test]
async fn workbench_seams_stay_quiet_and_dividers_still_resize(cx: &mut TestAppContext) {
    use gpui_kit::component::{Theme, ThemeMode};
    let (handle, this) = open_window(cx, None, Settings::default());
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        cx.update_window(handle.into(), |_, window, cx| {
            Theme::change(mode, Some(window), cx);
            this.update(cx, |p, cx| {
                p.sidebar_visible = true;
                cx.notify();
            });
            window.simulate_mouse_move(point(px(0.), px(0.)), cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let sidebar = window.find("workbench-sidebar").bounds();
            let editor = window.find("workbench-editor").bounds();
            let scale = window.scale_factor();
            let colors = theme::colors(cx);
            let divider_color = Theme::global(cx).border;
            let seams = [(sidebar.right() + editor.left()) / 2.];
            let quads = window.painted_quads();
            for x in seams {
                let probe = point(x + px(0.5), editor.top() + px(80.)).scale(scale);
                assert!(
                    !quads.iter().any(|quad| {
                        quad.background == divider_color.into()
                            && quad.bounds.contains(&probe)
                            && quad.content_mask.bounds.contains(&probe)
                    }),
                    "idle divider paints a line in {mode:?} at {x:?}"
                );
            }
            let title_edge = point(editor.center().x, theme::TITLE_HEIGHT - px(0.5)).scale(scale);
            assert!(
                !quads.iter().any(|quad| {
                    quad.border_color == colors.border
                        && quad.border_widths.bottom > Default::default()
                        && quad.bounds.size.width >= window.viewport_size().width.scale(scale)
                        && quad.bounds.contains(&title_edge)
                        && quad.content_mask.bounds.contains(&title_edge)
                }),
                "title bar paints a full-width bottom border in {mode:?}"
            );
            assert_eq!(sidebar.left(), px(2.));
            assert_eq!(window.viewport_size().width - editor.right(), px(2.));
            assert_eq!(editor.left() - sidebar.right(), px(4.));
            let command_center = window.find("command-center").bounds();
            assert_eq!(command_center.top(), px(2.5));
            assert_eq!(editor.top() - command_center.bottom(), px(2.5));
            assert_eq!(
                window.viewport_size().height - theme::STATUS_HEIGHT - editor.bottom(),
                px(4.)
            );
            assert_eq!(sidebar.top(), editor.top());
            assert_eq!(sidebar.bottom(), editor.bottom());

            let divider = point(seams[0], editor.center().y);
            window.simulate_mouse_move(divider, cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let indicator_size = size(theme::INDICATOR, theme::TAB_HEIGHT).scale(scale);
            assert!(
                window.painted_quads().iter().any(|quad| {
                    quad.background == colors.muted.into()
                        && quad.bounds.size == indicator_size
                        && quad.bounds.center().x >= (divider.x - px(2.)).scale(scale)
                        && quad.bounds.center().x <= (divider.x + px(2.)).scale(scale)
                        && quad.content_mask.bounds.contains(&quad.bounds.center())
                }),
                "hovered divider has no short indicator in {mode:?}"
            );
            window.drag(divider, divider - point(px(40.), px(0.)), cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let resized = window.find("workbench-sidebar").bounds();
            assert!(resized.size.width < sidebar.size.width - px(20.));
        })
        .unwrap();
    }
}

#[gpui_kit::test]
async fn workbench_panels_fit_small_windows_with_every_visibility_combination(
    cx: &mut TestAppContext,
) {
    let documents = cx.update(|cx| install_globals(cx, Settings::default()));
    for width in [760., 1024., 1400., 1920.] {
        for sidebar in [true, false] {
            let (handle, _this) = cx.update(|cx| {
                let (handle, this) = new_window(
                    cx,
                    None,
                    documents.clone(),
                    Bounds::new(point(px(0.), px(0.)), size(px(width), px(640.))),
                );
                this.update(cx, |p, cx| {
                    p.sidebar_visible = sidebar;
                    cx.notify();
                });
                (handle, this)
            });
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| {
                window.render_frame(cx);
                window.render_frame(cx);
                let viewport = window.viewport_size();
                let editor = window.find("workbench-editor").bounds();
                assert!(editor.size.width > px(0.) && editor.size.height > px(0.));
                let mut panels = vec![editor];
                if sidebar {
                    let bounds = window.find("workbench-sidebar").bounds();
                    eprintln!(
                        "event=sidebar_layout_test viewport={width} sidebar={} editor={}",
                        bounds.size.width.as_f32(),
                        editor.size.width.as_f32()
                    );
                    assert_eq!(viewport.width, px(width));
                    assert!(
                        (bounds.size.width - (theme::SIDEBAR_WIDTH - theme::WORKBENCH_INSET * 2.))
                            .abs()
                            <= px(1.),
                        "default sidebar width changed during layout: {bounds:?}"
                    );
                    assert!(bounds.right() < editor.left(), "sidebar overlaps editor");
                    panels.push(bounds);
                }
                for bounds in panels {
                    assert!(
                        bounds.left() >= px(0.) && bounds.right() <= viewport.width,
                        "panel overflows {width}px window: {bounds:?}"
                    );
                    assert!(
                        bounds.top() >= px(0.) && bounds.bottom() <= viewport.height,
                        "panel overflows window height: {bounds:?}"
                    );
                }
            })
            .unwrap();
        }
    }
}

#[gpui_kit::test]
async fn a_window_resize_keeps_the_sidebar_width_before_squeezing_it(cx: &mut TestAppContext) {
    let documents = cx.update(|cx| install_globals(cx, Settings::default()));
    let (handle, _this) = cx.update(|cx| {
        new_window(
            cx,
            None,
            documents,
            Bounds::new(point(px(0.), px(0.)), size(px(1400.), px(900.))),
        )
    });
    let sidebar = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find("workbench-sidebar").bounds().size.width
        })
        .unwrap()
    };
    let editor = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.find("workbench-editor").bounds().size.width
        })
        .unwrap()
    };
    assert_eq!(sidebar(cx), px(396.));
    // 900 px still has room: the sidebar keeps its width and the editor takes the rest.
    cx.simulate_window_resize(handle, size(px(900.), px(900.)));
    cx.run_until_parked();
    assert_eq!(sidebar(cx), px(396.));
    assert_eq!(editor(cx), px(496.));
    // 760 px is the window minimum: the editor keeps its minimum, the sidebar its width.
    cx.simulate_window_resize(handle, size(px(760.), px(900.)));
    cx.run_until_parked();
    assert_eq!(sidebar(cx), px(396.));
    assert_eq!(editor(cx), px(356.));
}

#[gpui_kit::test]
async fn the_sidebar_stops_before_the_editor_gets_too_narrow(cx: &mut TestAppContext) {
    let documents = cx.update(|cx| install_globals(cx, Settings::default()));
    for width in [760., 1400.] {
        let (handle, _this) = cx.update(|cx| {
            new_window(
                cx,
                None,
                documents.clone(),
                Bounds::new(point(px(0.), px(0.)), size(px(width), px(640.))),
            )
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let sidebar = window.find("workbench-sidebar").bounds();
            let editor = window.find("workbench-editor").bounds();
            let divider = point(sidebar.right() + px(2.), editor.center().y);
            window.simulate_mouse_move(divider, cx);
            // Ask for more than the window can give; the contract clamps it.
            window.drag(divider, divider + point(px(400.), px(0.)), cx);
            window.render_frame(cx);
            window.render_frame(cx);
            let sidebar = window.find("workbench-sidebar").bounds();
            let editor = window.find("workbench-editor").bounds();
            assert!(
                editor.size.width >= theme::EDITOR_MIN,
                "{width}px window left the editor at {:?}",
                editor.size.width
            );
            assert!(
                sidebar.size.width <= theme::SIDEBAR_MAX - theme::WORKBENCH_INSET * 2.,
                "{width}px window overshot the sidebar maximum: {:?}",
                sidebar.size.width
            );
        })
        .unwrap();
    }
}
