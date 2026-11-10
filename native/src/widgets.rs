//! Theme and reusable widgets for the native client.

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Label, Layout, Rect, Response, RichText, Sense, Ui,
    UiBuilder, Vec2,
};
use egui_material_icons::icons::{ICON_GRAPHIC_EQ, ICON_MUSIC_NOTE, ICON_PLAY_ARROW};

use crate::api::{thumb_sized, Card, Track};

pub const BG: Color32 = Color32::from_rgb(14, 14, 17);
pub const SIDE: Color32 = Color32::from_rgb(9, 9, 11);
pub const PANEL: Color32 = Color32::from_rgb(18, 18, 22);
pub const BAR: Color32 = Color32::from_rgb(19, 19, 23);
pub const FIELD: Color32 = Color32::from_rgb(23, 23, 28);
pub const HOVER: Color32 = Color32::from_rgb(29, 29, 35);
pub const TEXT: Color32 = Color32::from_rgb(242, 242, 244);
pub const SUB: Color32 = Color32::from_rgb(142, 142, 153);
pub const MUTED: Color32 = Color32::from_rgb(111, 111, 122);
pub const ICON: Color32 = Color32::from_rgb(201, 201, 209);
pub const ACCENT: Color32 = Color32::from_rgb(255, 77, 109);
const HERO: Color32 = Color32::from_rgb(43, 26, 61);
const HERO_SUB: Color32 = Color32::from_rgb(190, 180, 236);

pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = FIELD;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke.color = Color32::WHITE;
    visuals.slider_trailing_fill = true;
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(58, 58, 66);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(90, 90, 100);
    visuals.widgets.active.bg_fill = Color32::WHITE;
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
    });
}

/// Lays widgets out inside `rect` without moving the parent's layout cursor.
/// (The row/tile that owns `rect` has already been allocated by the caller.)
pub fn sub_ui(ui: &mut Ui, rect: Rect, layout: Layout, add_contents: impl FnOnce(&mut Ui)) {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    add_contents(&mut child);
}

/// Paints a rounded cover image (or a placeholder while it loads / if missing).
pub fn cover(ui: &Ui, rect: Rect, url: &str, px: u32, radius: u8) {
    ui.painter().rect_filled(rect, radius, HOVER);
    if url.is_empty() {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            ICON_MUSIC_NOTE.codepoint,
            FontId::proportional(rect.height() * 0.45),
            MUTED,
        );
    } else {
        egui::Image::new(thumb_sized(url, px)).corner_radius(radius).paint_at(ui, rect);
    }
}

/// Sidebar entry: icon + label, highlighted when selected. Returns true on click.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(rect, 8, if selected { HOVER } else { FIELD });
    }
    let color = if selected { TEXT } else { Color32::from_rgb(170, 170, 180) };
    ui.painter().text(
        pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        icon,
        FontId::proportional(20.0),
        if selected { ACCENT } else { color },
    );
    let text = Rect::from_min_max(pos2(rect.left() + 42.0, rect.top()), pos2(rect.right() - 6.0, rect.bottom()));
    sub_ui(ui, text, Layout::left_to_right(Align::Center), |ui| {
        ui.add(Label::new(RichText::new(label).size(14.0).color(color)).truncate());
    });
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// Round, borderless icon button.
pub fn icon_button(ui: &mut Ui, icon: &str, size: f32, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size + 14.0), Sense::click());
    if response.hovered() {
        ui.painter().circle_filled(rect.center(), rect.width() / 2.0, HOVER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(size), color);
    response
}

/// The white circular play / pause button.
pub fn play_button(ui: &mut Ui, icon: &str, diameter: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::click());
    let radius = diameter / 2.0 * if response.hovered() { 1.05 } else { 1.0 };
    ui.painter().circle_filled(rect.center(), radius, Color32::WHITE);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(diameter * 0.62), BG);
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

#[derive(Default, Clone, Copy)]
pub struct RowOpts {
    /// This is the song that is playing.
    pub active: bool,
    pub album: bool,
    pub duration: bool,
}

/// A song row: cover, title, artist, optional album and duration. Returns true on click.
pub fn song_row(ui: &mut Ui, size: Vec2, track: &Track, opts: RowOpts) -> bool {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    if opts.active || response.hovered() {
        ui.painter().rect_filled(rect, 8, HOVER);
    }
    let inner = rect.shrink2(vec2(8.0, 6.0));
    let art = Rect::from_min_size(inner.min, Vec2::splat(inner.height()));
    cover(ui, art, &track.thumb, 120, 6);
    if opts.active {
        ui.painter().rect_filled(art, 6, Color32::from_black_alpha(150));
        ui.painter().text(art.center(), Align2::CENTER_CENTER, ICON_GRAPHIC_EQ.codepoint, FontId::proportional(20.0), ACCENT);
    } else if response.hovered() {
        ui.painter().rect_filled(art, 6, Color32::from_black_alpha(150));
        ui.painter().text(art.center(), Align2::CENTER_CENTER, ICON_PLAY_ARROW.codepoint, FontId::proportional(24.0), TEXT);
    }

    let show_duration = opts.duration && track.duration > 0.0;
    let text_left = art.right() + 12.0;
    let text_right = inner.right() - if show_duration { 48.0 } else { 0.0 };
    let show_album = opts.album && inner.width() > 520.0;
    let album_left = if show_album { text_left + (text_right - text_left) * 0.58 } else { text_right };

    let main = Rect::from_min_max(pos2(text_left, inner.center().y - 18.0), pos2(album_left - 12.0, inner.bottom()));
    sub_ui(ui, main, Layout::top_down(Align::LEFT), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let title = RichText::new(&track.title).size(14.0).color(if opts.active { ACCENT } else { TEXT });
        ui.add(Label::new(title).truncate());
        ui.add(Label::new(RichText::new(&track.artist).size(12.5).color(SUB)).truncate());
    });
    if show_album {
        let album = Rect::from_min_max(pos2(album_left, inner.top()), pos2(text_right - 8.0, inner.bottom()));
        sub_ui(ui, album, Layout::left_to_right(Align::Center), |ui| {
            ui.add(Label::new(RichText::new(&track.album).size(13.0).color(SUB)).truncate());
        });
    }
    if show_duration {
        let secs = track.duration as u64;
        ui.painter().text(
            pos2(inner.right(), inner.center().y),
            Align2::RIGHT_CENTER,
            format!("{}:{:02}", secs / 60, secs % 60),
            FontId::proportional(12.5),
            SUB,
        );
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.clicked()
}

/// A playlist / album / mix tile for the horizontal shelves. Returns true on click.
pub fn card_tile(ui: &mut Ui, card: &Card) -> bool {
    const SIZE: f32 = 156.0;
    let (rect, response) = ui.allocate_exact_size(vec2(SIZE, SIZE + 50.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    let art = Rect::from_min_size(rect.min, Vec2::splat(SIZE));
    cover(ui, art, &card.thumb, 320, 12);
    if response.hovered() {
        ui.painter().rect_filled(art, 12, Color32::from_black_alpha(110));
        let centre = pos2(art.right() - 30.0, art.bottom() - 30.0);
        ui.painter().circle_filled(centre, 20.0, Color32::WHITE);
        ui.painter().text(centre, Align2::CENTER_CENTER, ICON_PLAY_ARROW.codepoint, FontId::proportional(26.0), BG);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let text = Rect::from_min_max(pos2(rect.left() + 2.0, art.bottom() + 8.0), rect.max);
    sub_ui(ui, text, Layout::top_down(Align::LEFT), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add(Label::new(RichText::new(&card.title).size(14.0).color(TEXT)).truncate());
        ui.add(Label::new(RichText::new(&card.subtitle).size(12.5).color(SUB)).truncate());
    });
    response.clicked()
}

/// The featured banner on Home. Returns true when Play is clicked.
pub fn hero(ui: &mut Ui, title: &str, subtitle: &str, thumb: &str) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 164.0), Sense::hover());
    ui.painter().rect_filled(rect, 16, HERO);
    let art = Rect::from_min_size(rect.min + vec2(20.0, 20.0), Vec2::splat(124.0));
    cover(ui, art, thumb, 320, 12);

    let text = Rect::from_min_max(pos2(art.right() + 20.0, rect.top() + 22.0), pos2(rect.right() - 20.0, rect.bottom()));
    let mut clicked = false;
    sub_ui(ui, text, Layout::top_down(Align::LEFT), |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(RichText::new("Picked for you").size(12.5).color(HERO_SUB));
        ui.add(Label::new(RichText::new(title).size(26.0).strong().color(Color32::WHITE)).truncate());
        ui.add(Label::new(RichText::new(subtitle).size(14.0).color(HERO_SUB)).truncate());
        ui.add_space(10.0);
        let label = RichText::new(format!("{}  Play", ICON_PLAY_ARROW.codepoint)).size(14.0).strong().color(BG);
        let button = egui::Button::new(label).fill(Color32::WHITE).corner_radius(18).min_size(vec2(96.0, 34.0));
        let response = ui.add(button);
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        clicked = response.clicked();
    });
    clicked
}
