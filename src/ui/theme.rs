//! The neon-on-black palette and the egui style built from it.
//!
//! Colour lives here and nowhere else. The domain decides *which* lane index a
//! line gets; this module decides what that index looks like, so the whole
//! product can be restyled without touching the layout algorithm.

use eframe::egui::{self, Color32, Rounding, Stroke};

use crate::domain::LANE_COLOR_COUNT;

/// Base surfaces and text. Deliberately very dark: the neon accents only read
/// as neon against near-black.
pub struct Palette;

impl Palette {
    /// Behind everything.
    pub const BACKDROP: Color32 = Color32::from_rgb(0x07, 0x09, 0x0D);
    /// Side panels and headers.
    pub const PANEL: Color32 = Color32::from_rgb(0x0C, 0x10, 0x17);
    /// Raised surfaces: chips, cards.
    pub const SURFACE: Color32 = Color32::from_rgb(0x13, 0x18, 0x23);
    /// Row under the cursor.
    pub const HOVER: Color32 = Color32::from_rgb(0x16, 0x1D, 0x2B);
    /// Selected row.
    pub const SELECTED: Color32 = Color32::from_rgb(0x1B, 0x26, 0x3A);
    pub const BORDER: Color32 = Color32::from_rgb(0x1E, 0x26, 0x35);

    pub const TEXT: Color32 = Color32::from_rgb(0xCB, 0xD6, 0xE4);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x74, 0x84, 0x9B);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x4A, 0x57, 0x69);

    /// The product accent.
    pub const CYAN: Color32 = Color32::from_rgb(0x00, 0xE5, 0xFF);
    /// Uncommitted work.
    pub const DIRTY: Color32 = Color32::from_rgb(0xFF, 0xB3, 0x00);
    /// Conflicts and stale worktrees.
    pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x3B, 0x5C);
    /// Clean and in sync.
    pub const OK: Color32 = Color32::from_rgb(0x39, 0xFF, 0x8A);
}

/// One colour per lane index the layout can hand out.
///
/// Chosen so neighbours stay distinguishable: hue jumps by roughly a third of
/// the wheel between consecutive entries rather than walking around it, because
/// adjacent lanes are what the eye compares.
pub const LANE_COLORS: [Color32; LANE_COLOR_COUNT] = [
    Color32::from_rgb(0x00, 0xE5, 0xFF), // cyan
    Color32::from_rgb(0xFF, 0x2E, 0x97), // magenta
    Color32::from_rgb(0x39, 0xFF, 0x14), // acid green
    Color32::from_rgb(0xFF, 0xB3, 0x00), // amber
    Color32::from_rgb(0xB1, 0x4B, 0xFF), // violet
    Color32::from_rgb(0x2E, 0x9B, 0xFF), // electric blue
    Color32::from_rgb(0xFF, 0x6B, 0x2C), // orange
    Color32::from_rgb(0x00, 0xFF, 0xC8), // turquoise
];

/// Colour for a lane, wrapping if the graph is wider than the palette.
pub fn lane_color(index: usize) -> Color32 {
    LANE_COLORS[index % LANE_COLOR_COUNT]
}

/// A dim version of a lane colour, for the glow behind a node.
pub fn lane_glow(index: usize) -> Color32 {
    let base = lane_color(index);
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 38)
}

/// How much of a colour survives when a row falls outside the focused history.
const FADED: f32 = 0.22;

/// Fades a colour towards the backdrop.
///
/// Blending towards the background rather than lowering the alpha keeps the
/// result opaque, so a faded line never shows whatever is drawn behind it.
pub fn faded(color: Color32) -> Color32 {
    let blend = |channel: u8, backdrop: u8| {
        (channel as f32 * FADED + backdrop as f32 * (1.0 - FADED)).round() as u8
    };

    Color32::from_rgb(
        blend(color.r(), Palette::BACKDROP.r()),
        blend(color.g(), Palette::BACKDROP.g()),
        blend(color.b(), Palette::BACKDROP.b()),
    )
}

/// A colour, faded when `focused` is false.
pub fn focus_aware(color: Color32, focused: bool) -> Color32 {
    if focused {
        color
    } else {
        faded(color)
    }
}

/// Installs the theme. Call once, at startup.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    visuals.override_text_color = Some(Palette::TEXT);
    visuals.panel_fill = Palette::PANEL;
    visuals.window_fill = Palette::PANEL;
    visuals.extreme_bg_color = Palette::BACKDROP;
    visuals.faint_bg_color = Palette::SURFACE;
    visuals.window_stroke = Stroke::new(1.0_f32, Palette::BORDER);

    visuals.selection.bg_fill = Palette::SELECTED;
    visuals.selection.stroke = Stroke::new(1.0_f32, Palette::CYAN);

    visuals.widgets.noninteractive.bg_fill = Palette::PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Palette::BORDER);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Palette::TEXT_DIM);

    visuals.widgets.inactive.bg_fill = Palette::SURFACE;
    visuals.widgets.inactive.weak_bg_fill = Palette::SURFACE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Palette::TEXT);

    visuals.widgets.hovered.bg_fill = Palette::HOVER;
    visuals.widgets.hovered.weak_bg_fill = Palette::HOVER;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Palette::CYAN);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Palette::TEXT);

    visuals.widgets.active.bg_fill = Palette::SELECTED;
    visuals.widgets.active.weak_bg_fill = Palette::SELECTED;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, Palette::CYAN);

    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.rounding = Rounding::same(4.0_f32);
    }

    ctx.set_visuals(visuals);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(10.0);
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_one_colour_for_every_lane_index_the_domain_can_produce() {
        assert_eq!(LANE_COLORS.len(), LANE_COLOR_COUNT);
    }

    #[test]
    fn lane_colours_wrap_instead_of_panicking_on_a_wide_graph() {
        assert_eq!(lane_color(0), lane_color(LANE_COLOR_COUNT));
        assert_eq!(lane_color(3), lane_color(LANE_COLOR_COUNT * 7 + 3));
    }

    #[test]
    fn every_lane_colour_is_distinct() {
        let mut seen = std::collections::HashSet::new();
        for color in LANE_COLORS {
            assert!(seen.insert(color.to_array()), "duplicate lane colour");
        }
    }

    #[test]
    fn the_glow_is_a_translucent_version_of_its_lane_colour() {
        let base = lane_color(1);
        let glow = lane_glow(1);

        assert!(glow.a() < base.a(), "the glow must be translucent");

        // `Color32` stores premultiplied alpha, so the channels come back
        // scaled down rather than unchanged. What must survive is the hue: the
        // relative order of the channels.
        assert!(base.r() > base.b() && base.b() > base.g());
        assert!(
            glow.r() >= glow.b() && glow.b() >= glow.g(),
            "premultiplication must not reorder the channels: {glow:?}"
        );
    }

    #[test]
    fn fading_moves_a_colour_towards_the_backdrop_without_passing_it() {
        let faded_cyan = faded(Palette::CYAN);
        assert!(faded_cyan.g() < Palette::CYAN.g());
        assert!(faded_cyan.g() > Palette::BACKDROP.g());
    }

    #[test]
    fn a_faded_colour_stays_opaque() {
        // Blending, not transparency: a faded line must not reveal what is
        // drawn behind it.
        assert_eq!(faded(Palette::CYAN).a(), 255);
    }

    #[test]
    fn fading_the_backdrop_itself_changes_nothing() {
        assert_eq!(faded(Palette::BACKDROP), Palette::BACKDROP);
    }

    #[test]
    fn focus_aware_only_fades_what_is_out_of_focus() {
        assert_eq!(focus_aware(Palette::CYAN, true), Palette::CYAN);
        assert_eq!(focus_aware(Palette::CYAN, false), faded(Palette::CYAN));
    }

    #[test]
    fn each_lane_gets_its_own_glow() {
        let glows: std::collections::HashSet<_> =
            (0..LANE_COLOR_COUNT).map(|i| lane_glow(i).to_array()).collect();
        assert_eq!(glows.len(), LANE_COLOR_COUNT);
    }
}
