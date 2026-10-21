//! Drives real pointer input through egui, without a window.
//!
//! Unit tests that call a view's helpers directly cannot see which widget a
//! click actually lands on, and that is exactly where the worktree card's
//! buttons once failed: the card claimed every click on top of them. These
//! helpers find a widget by its accessible name and press it the way a mouse
//! would, so egui's own hit testing decides who gets the click.

use eframe::egui::{self, accesskit, Pos2};

/// Draws `frame` until the widget named `name` has been clicked, and returns
/// the last value the frame produced along the way.
///
/// Panics if nothing on screen carries that name, so a renamed button fails
/// loudly instead of passing a test by clicking nothing.
pub fn click<R>(name: &str, mut frame: impl FnMut(&egui::Context) -> Option<R>) -> Option<R> {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();

    let settled = settle(&ctx, &mut frame);
    let target =
        centre_of(&settled, name).unwrap_or_else(|| panic!("nothing on screen is named {name:?}"));

    let mut result = None;
    for event in [
        egui::Event::PointerMoved(target),
        press(target, true),
        press(target, false),
    ] {
        let _ = ctx.run(input(vec![event]), |ctx| {
            if let Some(value) = frame(ctx) {
                result = Some(value);
            }
        });
    }
    result
}

/// Draws `frame` until its layout settles and reports, for each name, whether
/// anything on screen carries it.
pub fn shows<R, const N: usize>(
    names: &[&str; N],
    mut frame: impl FnMut(&egui::Context) -> Option<R>,
) -> [bool; N] {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();

    let output = settle(&ctx, &mut frame);
    names.map(|name| centre_of(&output, name).is_some())
}

fn settle<R>(
    ctx: &egui::Context,
    frame: &mut impl FnMut(&egui::Context) -> Option<R>,
) -> egui::FullOutput {
    // A window spends its first frame measuring itself, invisible, and only
    // takes its anchored place on a later one: positions are read once the
    // layout has settled, or the click lands where the button used to be.
    let mut output = None;
    for _ in 0..3 {
        output = Some(ctx.run(input(vec![]), |ctx| {
            frame(ctx);
        }));
    }
    output.expect("at least one frame ran")
}

fn input(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            Pos2::ZERO,
            egui::vec2(1280.0, 820.0),
        )),
        events,
        ..Default::default()
    }
}

fn press(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn centre_of(output: &egui::FullOutput, name: &str) -> Option<Pos2> {
    let update = output.platform_output.accesskit_update.as_ref()?;
    update
        .nodes
        .iter()
        .map(|(_, node)| node)
        .filter(|node| node.name() == Some(name))
        .find_map(accesskit::Node::bounds)
        // A pixel ratio of one, so accesskit's pixels are egui's points.
        .map(|bounds| {
            Pos2::new(
                ((bounds.x0 + bounds.x1) / 2.0) as f32,
                ((bounds.y0 + bounds.y1) / 2.0) as f32,
            )
        })
}
