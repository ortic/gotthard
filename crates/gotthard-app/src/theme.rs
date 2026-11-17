//! Visual design: palettes for dark and light mode, spacing and rounding.

use egui::style::{Selection, WidgetVisuals};
use egui::{
    Color32, CornerRadius, FontId, Frame, Margin, Shadow, Stroke, Style, TextStyle, Theme, Vec2,
};

/// Warnings, conflicts.
pub const WARN: Color32 = Color32::from_rgb(0xe0, 0x9b, 0x2d);
/// Deletions, damage.
pub const DANGER: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
pub const ACCENT: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
/// Something checked out fine.
pub const SUCCESS: Color32 = Color32::from_rgb(0x36, 0xa8, 0x5f);

struct Palette {
    /// The navigation at the left (darkest in dark mode).
    base: Color32,
    panel: Color32,
    /// Lists, tables, text fields.
    surface: Color32,
    /// Dialogs, menus, cards.
    raised: Color32,
    field: Color32,
    border: Color32,
    text: Color32,
    text_weak: Color32,
    hover: Color32,
    active: Color32,
    selection: Color32,
    stripe: Color32,
    error: Color32,
    warn: Color32,
    link: Color32,
}

const DARK: Palette = Palette {
    base: Color32::from_rgb(0x17, 0x18, 0x1b),
    panel: Color32::from_rgb(0x1e, 0x1f, 0x23),
    surface: Color32::from_rgb(0x1a, 0x1b, 0x1e),
    raised: Color32::from_rgb(0x27, 0x29, 0x2e),
    field: Color32::from_rgb(0x15, 0x16, 0x19),
    border: Color32::from_rgb(0x32, 0x35, 0x3b),
    text: Color32::from_rgb(0xdc, 0xde, 0xe3),
    text_weak: Color32::from_rgb(0x8b, 0x90, 0x9a),
    hover: Color32::from_rgb(0x2c, 0x2f, 0x35),
    active: Color32::from_rgb(0x35, 0x39, 0x41),
    selection: Color32::from_rgba_premultiplied(0x23, 0x4e, 0x94, 0xb0),
    stripe: Color32::from_rgb(0x22, 0x23, 0x28),
    error: Color32::from_rgb(0xf2, 0x6d, 0x6d),
    warn: Color32::from_rgb(0xe8, 0xb3, 0x4a),
    link: Color32::from_rgb(0x6c, 0xa6, 0xff),
};

const LIGHT: Palette = Palette {
    base: Color32::from_rgb(0xe9, 0xeb, 0xef),
    panel: Color32::from_rgb(0xf4, 0xf5, 0xf7),
    surface: Color32::from_rgb(0xff, 0xff, 0xff),
    raised: Color32::from_rgb(0xff, 0xff, 0xff),
    field: Color32::from_rgb(0xff, 0xff, 0xff),
    border: Color32::from_rgb(0xd0, 0xd4, 0xda),
    text: Color32::from_rgb(0x1f, 0x23, 0x28),
    text_weak: Color32::from_rgb(0x6a, 0x71, 0x7c),
    hover: Color32::from_rgb(0xe3, 0xe6, 0xea),
    active: Color32::from_rgb(0xd5, 0xd9, 0xe0),
    selection: Color32::from_rgba_unmultiplied_const(0xa6, 0xc8, 0xff, 0xc0),
    stripe: Color32::from_rgb(0xec, 0xee, 0xf1),
    error: Color32::from_rgb(0xc8, 0x2b, 0x2b),
    warn: Color32::from_rgb(0xa8, 0x62, 0x00),
    link: Color32::from_rgb(0x1d, 0x5f, 0xd6),
};

pub fn install(ctx: &egui::Context) {
    // The UI font lacks arrows and some symbols; the monospace font (Hack)
    // has them, before the emoji fonts.
    let mut fonts = egui::FontDefinitions::default();
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(1.min(family.len()), "Hack".to_owned());
    }
    ctx.set_fonts(fonts);
    ctx.style_mut_of(Theme::Dark, |s| style(s, &DARK));
    ctx.style_mut_of(Theme::Light, |s| style(s, &LIGHT));
}

fn style(style: &mut Style, p: &Palette) {
    for (text_style, font) in [
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Body, FontId::proportional(14.0)),
        (TextStyle::Button, FontId::proportional(14.0)),
        (TextStyle::Heading, FontId::proportional(20.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
    ] {
        style.text_styles.insert(text_style, font);
    }
    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(8.0, 6.0);
    spacing.button_padding = Vec2::new(10.0, 4.0);
    spacing.interact_size = Vec2::new(36.0, 24.0);
    spacing.window_margin = Margin::same(16);
    spacing.menu_margin = Margin::same(6);
    spacing.scroll = egui::style::ScrollStyle {
        dormant_handle_opacity: 0.6,
        active_handle_opacity: 0.8,
        bar_width: 10.0,
        ..egui::style::ScrollStyle::solid()
    };

    let v = &mut style.visuals;
    v.panel_fill = p.panel;
    v.window_fill = p.raised;
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.field);
    v.code_bg_color = p.surface;
    v.faint_bg_color = p.stripe;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow_alpha = if v.dark_mode { 110 } else { 40 };
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.selection = Selection {
        bg_fill: p.selection,
        stroke: Stroke::new(1.0, p.text),
    };
    v.hyperlink_color = p.link;
    v.error_fg_color = p.error;
    v.warn_fg_color = p.warn;
    v.striped = true;
    v.text_cursor.stroke = Stroke::new(2.0, ACCENT);

    let widget = |bg: Color32, stroke: Color32, fg: Color32| WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(5),
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    };
    let w = &mut v.widgets;
    w.noninteractive = widget(p.panel, p.border, p.text);
    w.inactive = widget(p.hover, p.border, p.text);
    w.hovered = widget(p.active, p.border, p.text);
    w.active = widget(p.active, ACCENT, p.text);
    w.open = widget(p.active, p.border, p.text);
    v.weak_text_color = Some(p.text_weak);
}

fn palette(visuals: &egui::Visuals) -> &'static Palette {
    if visuals.dark_mode { &DARK } else { &LIGHT }
}

/// The background of the navigation.
pub fn base(visuals: &egui::Visuals) -> Color32 {
    palette(visuals).base
}

pub fn border(visuals: &egui::Visuals) -> Color32 {
    palette(visuals).border
}

/// A box that groups related things on a view.
pub fn card(style: &Style) -> Frame {
    let p = palette(&style.visuals);
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
}

/// A banner across a view: a warning, an error, a result.
pub fn banner(style: &Style, color: Color32) -> Frame {
    let p = palette(&style.visuals);
    Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
}
