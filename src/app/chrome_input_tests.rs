//! Exercises production title-bar widgets with egui events only. Viewport
//! commands are inspected, never executed against the working desktop.
use super::*;

const SIZE: egui::Vec2 = egui::vec2(1400.0, 820.0);

fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
            events,
            ..Default::default()
        },
        |ctx| app.draw(ctx, None),
    )
}

fn setup() -> (App, egui::Context, egui::FullOutput) {
    let ctx = egui::Context::default();
    let mut app = App::preview(&ctx, &[]);
    for _ in 0..3 {
        let _ = frame(&mut app, &ctx, vec![]);
    }
    let output = frame(&mut app, &ctx, vec![]);
    (app, ctx, output)
}

fn label(output: &egui::FullOutput, name: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == name => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing production widget {name}"))
}

fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

fn commands(output: &egui::FullOutput) -> &[egui::ViewportCommand] {
    &output.viewport_output[&egui::ViewportId::ROOT].commands
}

#[test]
fn clicks_still_activate_the_production_navigation_and_caption_controls() {
    for name in [
        "Settings", "About", "Capture", "minimize", "maximize", "close",
    ] {
        let (mut app, ctx, output) = setup();
        let point = match name {
            "minimize" => egui::pos2(1296.0, 20.0),
            "maximize" => egui::pos2(1336.0, 20.0),
            "close" => egui::pos2(1376.0, 20.0),
            _ => label(&output, name),
        };
        let pressed = frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        assert!(
            !commands(&pressed).contains(&egui::ViewportCommand::StartDrag),
            "{name} press swallowed"
        );
        let released = frame(&mut app, &ctx, vec![button(point, false)]);
        assert!(!commands(&released).contains(&egui::ViewportCommand::StartDrag));
        match name {
            "Settings" => assert!(app.tab == Tab::Settings),
            "About" => assert!(app.tab == Tab::About),
            "Capture" => {
                let menu = frame(&mut app, &ctx, vec![]);
                assert!(menu.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text.contains("Fullscreen"))));
            }
            "minimize" => {
                assert!(commands(&released).contains(&egui::ViewportCommand::Minimized(true)))
            }
            "maximize" => {
                assert!(commands(&released).contains(&egui::ViewportCommand::Maximized(true)))
            }
            "close" => {
                assert!(commands(&released).contains(&egui::ViewportCommand::Visible(false)))
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn dragging_buttons_search_swatch_and_gaps_moves_without_activating_controls() {
    for name in [
        "TrontSnap",
        "Capture",
        "Gallery",
        "Settings",
        "About",
        "All",
        "ShareX",
        "Search",
        "swatch",
        "gap",
        "minimize",
        "maximize",
        "close",
    ] {
        let (mut app, ctx, output) = setup();
        let point = match name {
            "swatch" => egui::pos2(1254.0, 20.0),
            "gap" => egui::pos2(1150.0, 20.0),
            "minimize" => egui::pos2(1296.0, 20.0),
            "maximize" => egui::pos2(1336.0, 20.0),
            "close" => egui::pos2(1376.0, 20.0),
            _ => label(&output, name),
        };
        let pressed = frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        assert!(!commands(&pressed).contains(&egui::ViewportCommand::StartDrag));
        let destination = point + egui::vec2(25.0, 35.0);
        let moving = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(destination)]);
        assert!(
            commands(&moving).ends_with(&[
                egui::ViewportCommand::Focus,
                egui::ViewportCommand::StartDrag
            ]),
            "{name}: {:?}",
            commands(&moving)
        );
        let released = frame(&mut app, &ctx, vec![button(destination, false)]);
        assert!(app.tab == Tab::Gallery, "drag activated {name}");
        assert!(!app.show_theme_window, "drag opened theme");
        assert!(!commands(&released).iter().any(|c| matches!(
            c,
            egui::ViewportCommand::Minimized(_)
                | egui::ViewportCommand::Maximized(_)
                | egui::ViewportCommand::Visible(_)
        )));
    }
}

#[test]
fn small_pointer_jitter_keeps_a_click_and_focus_retries_keep_a_drag() {
    let (mut app, ctx, output) = setup();
    let point = label(&output, "Settings");
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    let jitter = point + egui::vec2(2.0, 1.0);
    let output = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(jitter)]);
    assert!(!commands(&output).contains(&egui::ViewportCommand::StartDrag));
    let _ = frame(&mut app, &ctx, vec![button(jitter, false)]);
    assert!(app.tab == Tab::Settings);
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    let moved = point + egui::vec2(30.0, 35.0);
    let _ = frame(&mut app, &ctx, vec![egui::Event::PointerMoved(moved)]);
    let retry = frame(&mut app, &ctx, vec![]);
    assert!(commands(&retry).ends_with(&[
        egui::ViewportCommand::Focus,
        egui::ViewportCommand::StartDrag
    ]));
}

#[test]
fn gallery_body_and_resize_edges_do_not_start_a_window_move() {
    for point in [
        egui::pos2(400.0, 80.0),
        egui::pos2(600.0, 3.0),
        egui::pos2(8.0, 8.0),
    ] {
        let (mut app, ctx, _) = setup();
        let _ = frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        let moved = frame(
            &mut app,
            &ctx,
            vec![egui::Event::PointerMoved(point + egui::vec2(25.0, 35.0))],
        );
        assert!(!commands(&moved).contains(&egui::ViewportCommand::StartDrag));
    }
}

#[test]
fn double_clicking_the_wordmark_still_maximizes() {
    let (mut app, ctx, output) = setup();
    let point = label(&output, "TrontSnap");
    let _ = frame(
        &mut app,
        &ctx,
        vec![egui::Event::PointerMoved(point), button(point, true)],
    );
    let _ = frame(&mut app, &ctx, vec![button(point, false)]);
    let _ = frame(&mut app, &ctx, vec![button(point, true)]);
    let double_click = frame(&mut app, &ctx, vec![button(point, false)]);
    assert!(commands(&double_click).contains(&egui::ViewportCommand::Maximized(true)));
    assert!(!commands(&double_click).contains(&egui::ViewportCommand::StartDrag));
}

#[test]
fn a_floating_panel_over_the_title_strip_keeps_its_own_input() {
    let (mut app, ctx, _) = setup();
    let mut render = |events| {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE)),
                events,
                ..Default::default()
            },
            |ctx| {
                app.draw(ctx, None);
                egui::Area::new(egui::Id::new("floating-panel-fixture"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(190.0, 8.0))
                    .show(ctx, |ui| {
                        ui.add_sized([120.0, 40.0], egui::Button::new("Popup action"));
                    });
            },
        )
    };
    for _ in 0..3 {
        let _ = render(vec![]);
    }
    let point = egui::pos2(240.0, 20.0);
    let _ = render(vec![egui::Event::PointerMoved(point), button(point, true)]);
    let moved = render(vec![egui::Event::PointerMoved(
        point + egui::vec2(25.0, 35.0),
    )]);
    assert!(!commands(&moved).contains(&egui::ViewportCommand::StartDrag));
}
