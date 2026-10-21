//! Look and small widgets. Style after the ArtCraft launcher (quiet surfaces, tall rows, monospace caps labels,
//! outlined status pills, one inverted primary button) and DockHand (cards, coloured tags, icon stats).
//! Light and dark palettes; the theme follows the system unless set in Settings.

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Sense, Stroke, Vec2, vec2};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    /// Primary button fill (inverted: near-white in dark mode, near-black in light mode) and its text.
    pub accent: Color32,
    pub on_accent: Color32,
    pub link: Color32,
    pub green: Color32,
    pub blue: Color32,
    pub orange: Color32,
    pub red: Color32,
    pub purple: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const DARK: Palette = Palette {
    bg: hex(0x111214),
    surface: hex(0x16171a),
    raised: hex(0x1d1e22),
    hover: hex(0x26282d),
    border: hex(0x2a2c31),
    text: hex(0xe9e9eb),
    dim: hex(0x9b9da4),
    faint: hex(0x8a8c93),
    accent: hex(0xf2f2f2),
    on_accent: hex(0x111214),
    link: hex(0x7cb0ff),
    green: hex(0x3ecf8e),
    blue: hex(0x60a5fa),
    orange: hex(0xf5a524),
    red: hex(0xf87171),
    purple: hex(0xa78bfa),
};

pub const LIGHT: Palette = Palette {
    bg: hex(0xf3f2ef),
    surface: hex(0xfafaf8),
    raised: hex(0xffffff),
    hover: hex(0xe9e7e2),
    border: hex(0xdcdad5),
    text: hex(0x17181b),
    dim: hex(0x66686e),
    faint: hex(0x6c6d72),
    accent: hex(0x141518),
    on_accent: hex(0xffffff),
    link: hex(0x2563eb),
    green: hex(0x15803d),
    blue: hex(0x2563eb),
    orange: hex(0xb45309),
    red: hex(0xdc2626),
    purple: hex(0x7c3aed),
};

pub fn pal(ctx: &egui::Context) -> Palette {
    if ctx.theme() == egui::Theme::Dark { DARK } else { LIGHT }
}

pub const SEMIBOLD: &str = "semibold";

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Font files for the UI, its semibold and monospace text, with their index in a collection.
type FontFiles = [Option<(std::path::PathBuf, u32)>; 3];

/// Segoe UI (regular + semibold) and Cascadia Mono from the Windows font folder.
#[cfg(windows)]
fn font_files() -> FontFiles {
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    ["segoeui.ttf", "seguisb.ttf", "CascadiaMono.ttf"].map(|f| Some((dir.join(f), 0)))
}

/// The system's sans-serif (regular + semibold) and monospace fonts, as fontconfig resolves them.
#[cfg(not(windows))]
fn font_files() -> FontFiles {
    let found = |pattern: &str| {
        let out = pa_core::cmd("fc-match").args(["-f", "%{file}\n%{index}", pattern]).output().ok().filter(|o| o.status.success())?;
        let text = String::from_utf8(out.stdout).ok()?;
        let (file, index) = text.split_once('\n')?;
        // The high 16 bits name a variable font's instance, which egui cannot select: keep the face only.
        Some((std::path::PathBuf::from(file), index.trim().parse::<u32>().unwrap_or(0) & 0xffff))
    };
    let [ui, sb, mono] = ["sans-serif", "sans-serif:weight=semibold", "monospace"].map(found);
    // fontconfig always answers: the sans-serif file again means there is no such face (a variable font has no
    // separate semibold, a system without a monospace font gets the sans-serif one), so egui's or regular then.
    let other = |f: Option<(std::path::PathBuf, u32)>| f.filter(|f| Some(f) != ui.as_ref());
    [ui.clone(), other(sb), other(mono)]
}

/// The system's UI and monospace fonts when present (`font_files`), else egui's fonts; Phosphor icons as a
/// fallback in every family.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let mut load = |name: &str, file: &Option<(std::path::PathBuf, u32)>| {
        let (path, index) = file.as_ref()?;
        let mut data = egui::FontData::from_owned(std::fs::read(path).ok()?);
        data.index = *index;
        fonts.font_data.insert(name.into(), data.into());
        Some(())
    };
    let [ui_file, sb_file, mono_file] = font_files();
    let ui = load("ui", &ui_file).is_some();
    let sb = load("ui-semibold", &sb_file).is_some();
    let mono = load("mono", &mono_file).is_some();
    let phosphor = "phosphor".to_string();
    let prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    if ui {
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "ui".into());
    }
    if mono {
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "mono".into());
    }
    // Icons inside monospace text (file paths, caps labels) too.
    let monos = fonts.families.entry(FontFamily::Monospace).or_default();
    if !monos.contains(&phosphor) {
        monos.push(phosphor.clone());
    }
    let mut semi = if sb { vec!["ui-semibold".to_string()] } else { Vec::new() };
    semi.extend(if ui { vec!["ui".to_string()] } else { prop });
    if !semi.contains(&phosphor) {
        semi.push(phosphor);
    }
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), semi);
    ctx.set_fonts(fonts);
}

fn visuals(p: Palette, dark: bool) -> egui::Visuals {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.raised;
    v.extreme_bg_color = if dark { hex(0x0c0d0f) } else { hex(0xffffff) };
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.raised;
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.link;
    v.selection.bg_fill = p.link.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.link);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 40 }) };
    v.popup_shadow = v.window_shadow;
    let r = CornerRadius::same(6);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = p.raised;
    w.inactive.weak_bg_fill = p.raised;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.faint);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.active.bg_fill = p.border;
    w.active.weak_bg_fill = p.border;
    w.active.bg_stroke = Stroke::new(1.0, p.dim);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.open = w.active;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = r;
        s.expansion = 0.0;
    }
    v
}

/// Apply both palettes and the spacing; `pref` is "system", "dark" or "light".
pub fn apply(ctx: &egui::Context, pref: &str) {
    ctx.set_visuals_of(egui::Theme::Dark, visuals(DARK, true));
    ctx.set_visuals_of(egui::Theme::Light, visuals(LIGHT, false));
    ctx.set_theme(match pref {
        "dark" => egui::ThemePreference::Dark,
        "light" => egui::ThemePreference::Light,
        _ => egui::ThemePreference::System,
    });
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 5.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.combo_height = 320.0;
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.window_margin = Margin::same(16);
        use egui::TextStyle::*;
        s.text_styles.insert(Body, FontId::proportional(14.0));
        s.text_styles.insert(Button, FontId::proportional(14.0));
        s.text_styles.insert(Small, FontId::proportional(12.0));
        s.text_styles.insert(Monospace, FontId::monospace(13.0));
        s.text_styles.insert(Heading, semibold(22.0));
    });
}

// --- widgets ------------------------------------------------------------------------------------------------

/// Monospace caps label with letter spacing (column headers, section titles, footer).
pub fn caps(text: impl AsRef<str>, color: Color32) -> RichText {
    RichText::new(text.as_ref().to_uppercase()).monospace().size(10.5).extra_letter_spacing(1.1).color(color)
}

/// An outlined pill: tinted fill, monospace caps text. Painted at its own size, so it never stretches to the row.
pub fn chip(ui: &mut egui::Ui, text: impl AsRef<str>, color: Color32) -> egui::Response {
    pill(ui, caps(text, color).size(10.0), color.gamma_multiply(0.10), Stroke::new(1.0, color.gamma_multiply(0.45)), vec2(6.0, 3.0))
}

/// A plain tag chip (DockHand style): soft fill, normal text.
pub fn tag(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    pill(ui, RichText::new(text).size(12.0).color(color), color.gamma_multiply(0.14), Stroke::NONE, vec2(6.0, 2.0))
}

fn pill(ui: &mut egui::Ui, text: RichText, fill: Color32, stroke: Stroke, pad: Vec2) -> egui::Response {
    let galley = egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
    let (rect, r) = ui.allocate_exact_size(galley.size() + pad * 2.0, Sense::hover());
    ui.painter().rect(rect, 4, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().galley(rect.min + pad, galley, Color32::PLACEHOLDER);
    r
}

/// Rounded square with the project's initials, tinted with `color`.
pub fn avatar(ui: &mut egui::Ui, name: &str, color: Color32, size: f32) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((size * 0.22) as u8), color.gamma_multiply(0.18));
    p.rect_stroke(rect, CornerRadius::same((size * 0.22) as u8), Stroke::new(1.0, color.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, initials(name), semibold(size * 0.38), color);
    r
}

/// "ChessClock" → "CC", "trailmix-web" → "TW", "weatherline" → "WE".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let caps: Vec<char> = name.chars().filter(|c| c.is_uppercase()).collect();
    let s: String = if words.len() >= 2 {
        words.iter().take(2).filter_map(|w| w.chars().next()).collect()
    } else if caps.len() >= 2 {
        caps.iter().take(2).collect()
    } else {
        name.chars().filter(|c| c.is_alphanumeric()).take(2).collect()
    };
    s.to_uppercase()
}

/// GitHub-like colours per language; others get a colour from their name.
pub fn lang_color(lang: &str) -> Color32 {
    match lang {
        "Rust" => hex(0xdea584),
        "Python" => hex(0x4b8bbe),
        "JavaScript" => hex(0xe8c547),
        "TypeScript" => hex(0x3178c6),
        "Svelte" => hex(0xff3e00),
        "Vue" => hex(0x41b883),
        "HTML" => hex(0xe34c26),
        "CSS" => hex(0x8e5cd9),
        "Go" => hex(0x00add8),
        "C" => hex(0x8a8a8a),
        "C++" => hex(0xf34b7d),
        "C#" => hex(0x178600),
        "Java" => hex(0xb07219),
        "Lua" => hex(0x6a7bd6),
        "Shell" => hex(0x89e051),
        "PowerShell" => hex(0x4d7ec8),
        "SQL" => hex(0xe38c00),
        "Apps Script" => hex(0x4285f4),
        other => hash_color(other),
    }
}

pub fn hash_color(s: &str) -> Color32 {
    let h = s.bytes().fold(0xcbf2_9ce4u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.55, 0.85, 1.0).into()
}

/// Thin stacked bar of language shares (top 5).
pub fn lang_bar(ui: &mut egui::Ui, langs: &[(String, u64)], width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 6.0), Sense::hover());
    let total: u64 = langs.iter().map(|l| l.1).sum();
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(3), pal(ui.ctx()).border);
    if total == 0 {
        return;
    }
    let mut x = rect.left();
    for (lang, n) in langs.iter().take(5) {
        let w = rect.width() * *n as f32 / total as f32;
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(x, rect.top()), vec2(w.max(1.0), rect.height())), 0, lang_color(lang));
        x += w;
    }
}

/// A coloured dot and the language name.
pub fn lang_dot(ui: &mut egui::Ui, lang: &str) {
    ui.spacing_mut().item_spacing.x = 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, lang_color(lang));
    ui.label(RichText::new(lang).size(13.0).color(pal(ui.ctx()).dim));
    ui.spacing_mut().item_spacing.x = 8.0;
}

/// A surface card.
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    let p = pal(ui.ctx());
    egui::Frame::new().fill(p.surface).stroke(Stroke::new(1.0, p.border)).corner_radius(10).inner_margin(Margin::same(16)).show(ui, add)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Inverted fill (one per view: the main action).
    Primary,
    /// Outlined.
    Secondary,
    /// No fill until hovered (icons, quiet actions).
    Ghost,
    /// Red `Ghost`, for actions that delete or forget something.
    DangerGhost,
    /// Sidebar entry or segment; `true` = the selected one.
    Nav(bool),
    Segment(bool),
}

/// Every button in the app: hover and press states fade in (~120 ms), pointer cursor, accessible label.
/// `text` may hold a second, dimmer part after a tab (`"ALL\t28"`), used by segments for their counts.
pub fn button(ui: &mut egui::Ui, text: &str, font: FontId, spacing: f32, kind: Kind, min: Vec2) -> egui::Response {
    button_sensing(ui, text, font, spacing, kind, min, Sense::click())
}

/// `button` with another sense (a sidebar favourite also drags).
fn button_sensing(ui: &mut egui::Ui, text: &str, font: FontId, spacing: f32, kind: Kind, min: Vec2, sense: Sense) -> egui::Response {
    let p = pal(ui.ctx());
    let pad = match kind {
        Kind::Ghost | Kind::DangerGhost => vec2(6.0, 4.0),
        Kind::Segment(_) => vec2(10.0, 4.0),
        _ => vec2(12.0, 5.0),
    };
    let (main, extra) = match text.split_once('\t') {
        Some((a, b)) => (a.to_string(), Some(b.to_string())),
        None => (text.to_string(), None),
    };
    // PLACEHOLDER: the colour is chosen at paint time (hover), not baked in by the theme's text colour override.
    let rich = |s: String, size: f32| RichText::new(s).font(FontId::new(size, font.family.clone())).extra_letter_spacing(spacing).color(Color32::PLACEHOLDER);
    let layout = |ui: &egui::Ui, t: RichText| egui::WidgetText::from(t).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button);
    let main_g = layout(ui, rich(main.clone(), font.size));
    let extra_g = extra.map(|e| layout(ui, rich(format!("  {e}"), (font.size - 0.5).max(9.0))));
    let text_w = main_g.size().x + extra_g.as_ref().map_or(0.0, |g| g.size().x);
    let size = vec2(text_w, main_g.size().y) + pad * 2.0;
    let size = size.max(min);
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &main));
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let h = ctx.animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
        let down = ctx.animate_bool_with_time(resp.id.with("down"), resp.is_pointer_button_down_on(), 0.06);
        let mix = |a: Color32, b: Color32, t: f32| a.lerp_to_gamma(b, t);
        let clear = Color32::TRANSPARENT;
        // (fill, stroke, text) at rest and fully hovered.
        let (rest, hover) = match kind {
            Kind::Primary => {
                let hov = mix(p.accent, p.dim, 0.18);
                ((p.accent, clear, p.on_accent), (hov, clear, p.on_accent))
            }
            Kind::Secondary => ((clear, p.border, p.text), (p.hover, p.faint, p.text)),
            Kind::Ghost => ((clear, clear, p.dim), (p.hover, clear, p.text)),
            Kind::DangerGhost => ((clear, clear, p.red), (p.red.gamma_multiply(0.10), clear, p.red)),
            Kind::Nav(true) => ((p.raised, p.border, p.text), (p.raised, p.faint, p.text)),
            Kind::Nav(false) => ((clear, clear, p.dim), (p.hover, clear, p.text)),
            Kind::Segment(true) => ((p.accent, clear, p.on_accent), (p.accent, clear, p.on_accent)),
            Kind::Segment(false) => ((clear, clear, p.dim), (p.hover, clear, p.text)),
        };
        let mut fill = mix(rest.0, hover.0, h);
        let stroke = mix(rest.1, hover.1, h);
        let fg = mix(rest.2, hover.2, h);
        if down > 0.0 {
            fill = mix(fill, if kind == Kind::Primary { p.faint } else { p.border }, down * 0.6);
        }
        let radius = if matches!(kind, Kind::Segment(_)) { 4 } else { 6 };
        let painter = ui.painter();
        painter.rect(rect, radius, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        // Nav entries read left to right; one too narrow for its padding (an icon toggle) stays centred.
        let left = matches!(kind, Kind::Nav(_)) && rect.width() > text_w + pad.x * 2.0;
        let x = if left { rect.left() + pad.x } else { rect.center().x - text_w / 2.0 };
        let y = rect.center().y - main_g.size().y / 2.0;
        let main_w = main_g.size().x;
        painter.galley(egui::pos2(x, y), main_g, fg);
        if let Some(g) = extra_g {
            let y = rect.center().y - g.size().y / 2.0;
            painter.galley(egui::pos2(x + main_w, y), g, fg.gamma_multiply(0.6));
        }
    }
    resp
}

/// The inverted primary button.
pub fn primary(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    button(ui, &text.into(), semibold(13.0), 0.0, Kind::Primary, vec2(0.0, 30.0))
}

/// Outlined secondary button.
pub fn secondary(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    button(ui, &text.into(), FontId::proportional(13.0), 0.0, Kind::Secondary, vec2(0.0, 30.0))
}

/// Quiet text button: no frame until hovered.
pub fn ghost(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    button(ui, &text.into(), FontId::proportional(13.0), 0.0, Kind::Ghost, vec2(0.0, 28.0))
}

/// Quiet red text button (`ghost` for deleting).
pub fn danger_ghost(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    button(ui, &text.into(), FontId::proportional(13.0), 0.0, Kind::DangerGhost, vec2(0.0, 28.0))
}

/// Borderless icon button with a tooltip.
pub fn icon_button(ui: &mut egui::Ui, icon: &str, tip: &str) -> egui::Response {
    button(ui, icon, FontId::proportional(16.0), 0.0, Kind::Ghost, vec2(28.0, 28.0)).on_hover_text(tip)
}

/// An icon button in a colour of its own (a favourite's star); hover fades in a background like `icon_button`.
pub fn color_icon(ui: &mut egui::Ui, icon: &str, color: Color32, tip: &str) -> egui::Response {
    let p = pal(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip);
    let h = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    ui.painter().rect_filled(rect, 6, Color32::TRANSPARENT.lerp_to_gamma(p.hover, h));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(16.0), color);
    resp
}

/// The right-aligned part of a toolbar row (a `ui.horizontal_wrapped`): laid out right to left after what is already
/// in the row, or on a row of its own when it would not fit there, instead of drawing over the left part. Its
/// width is remembered from the last frame under `salt`.
pub fn right<R>(ui: &mut egui::Ui, salt: &str, align: egui::Align, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let key = ui.id().with(("right", salt));
    let w: f32 = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let fits = w + ui.spacing().item_spacing.x * 3.0 <= ui.available_size_before_wrap().x;
    if !fits {
        ui.end_row();
    }
    let r = ui.with_layout(egui::Layout::right_to_left(align).with_main_wrap(!fits), add);
    ui.data_mut(|d| d.insert_temp(key, r.response.rect.width()));
    r.inner
}

/// A sidebar entry, full width.
pub fn nav(ui: &mut egui::Ui, text: impl Into<String>, on: bool) -> egui::Response {
    let w = ui.available_width();
    button(ui, &text.into(), FontId::proportional(14.0), 0.0, Kind::Nav(on), vec2(w, 34.0))
}

/// A sidebar entry that can also be dragged (favourites reorder).
pub fn nav_draggable(ui: &mut egui::Ui, text: impl Into<String>, on: bool) -> egui::Response {
    let w = ui.available_width();
    button_sensing(ui, &text.into(), FontId::proportional(14.0), 0.0, Kind::Nav(on), vec2(w, 34.0), Sense::click_and_drag())
}

/// Joined segmented control (ArtCraft's "ALL 13 | INSTALLED 4"): label, count (None = no count).
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut egui::Ui, items: &[(String, Option<usize>)], selected: usize) -> Option<usize> {
    let p = pal(ui.ctx());
    let mut clicked = None;
    egui::Frame::new().stroke(Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(Margin::same(2)).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        // Wraps onto more rows when the pane is too narrow, rather than running past its edge.
        ui.horizontal_wrapped(|ui| {
            for (i, (label, count)) in items.iter().enumerate() {
                let text = match count {
                    Some(n) => format!("{}\t{n}", label.to_uppercase()),
                    None => label.to_uppercase(),
                };
                if button(ui, &text, FontId::monospace(10.5), 1.0, Kind::Segment(i == selected), vec2(0.0, 26.0)).clicked() {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// Top-level tabs of a page (a project's sections): sentence case, count in faint, an underline under the selected
/// one, a hairline under the row. `segmented` is for the level below (sub-sections, views, filters).
/// Returns the index clicked, if any.
pub fn tabs(ui: &mut egui::Ui, items: &[(String, Option<usize>)], selected: usize) -> Option<usize> {
    let p = pal(ui.ctx());
    let mut clicked = None;
    let row = ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(20.0, 4.0);
        for (i, (label, count)) in items.iter().enumerate() {
            let on = i == selected;
            let font = if on { semibold(13.5) } else { FontId::proportional(13.5) };
            let label_g = ui.painter().layout_no_wrap(label.clone(), font, Color32::PLACEHOLDER);
            let count_g = count.map(|n| ui.painter().layout_no_wrap(n.to_string(), FontId::proportional(12.5), p.faint));
            let w = label_g.size().x + count_g.as_ref().map_or(0.0, |g| 6.0 + g.size().x) + 4.0;
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label));
            let h = hover_t(ui, &resp);
            let fg = if on { p.text } else { p.dim.lerp_to_gamma(p.text, h) };
            let y = rect.center().y - 1.0;
            let x = rect.left() + 2.0;
            let lw = label_g.size().x;
            ui.painter().galley(egui::pos2(x, y - label_g.size().y / 2.0), label_g, fg);
            if let Some(g) = count_g {
                ui.painter().galley(egui::pos2(x + lw + 6.0, y - g.size().y / 2.0), g, p.faint);
            }
            if on {
                ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom()), 0, p.accent);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    let r = row.response.rect;
    ui.painter().hline(r.left()..=ui.max_rect().right(), r.bottom(), Stroke::new(1.0, p.border));
    clicked
}

/// Every checkbox: a 16 px box with a faint outline (3:1 against the page), filled with the accent and a check when
/// on. `text` may be empty (a grid cell under a column header).
pub fn checkbox(ui: &mut egui::Ui, checked: &mut bool, text: impl Into<egui::WidgetText>) -> egui::Response {
    let p = pal(ui.ctx());
    let text: egui::WidgetText = text.into();
    let galley = (!text.is_empty()).then(|| text.into_galley(ui, None, (ui.available_width() - 24.0).max(40.0), egui::TextStyle::Body));
    let text_size = galley.as_ref().map_or(Vec2::ZERO, |g| g.size());
    let size = vec2(16.0 + if galley.is_some() { 8.0 + text_size.x } else { 0.0 }, text_size.y.max(20.0));
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let label = galley.as_ref().map_or(String::new(), |g| g.text().to_string());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *checked, &label));
    if ui.is_rect_visible(rect) {
        let enabled = ui.is_enabled();
        let h = hover_t(ui, &resp);
        let b = egui::Rect::from_center_size(egui::pos2(rect.left() + 8.0, rect.center().y), vec2(16.0, 16.0));
        let (fill, stroke) = match (enabled, *checked) {
            (false, _) => (p.hover, p.border),
            (true, true) => (p.accent, p.accent),
            (true, false) => (ui.visuals().extreme_bg_color, p.faint.lerp_to_gamma(p.text, h * 0.6)),
        };
        ui.painter().rect(b, 4, fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        if *checked {
            let c = if enabled { p.on_accent } else { p.faint };
            ui.painter().text(b.center(), Align2::CENTER_CENTER, egui_phosphor::regular::CHECK, FontId::proportional(12.0), c);
        }
        if let Some(g) = galley {
            let fg = if enabled { p.text } else { p.faint };
            ui.painter().galley(egui::pos2(b.right() + 8.0, rect.center().y - text_size.y / 2.0), g, fg);
        }
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Every drop-down: the field look (32 px, field fill, radius 6) with a chevron; `add` fills the menu.
pub fn select<R>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    selected: impl Into<egui::WidgetText>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        let field = ui.visuals().extreme_bg_color;
        ui.visuals_mut().widgets.inactive.weak_bg_fill = field;
        egui::ComboBox::from_id_salt(id)
            .width(width)
            .selected_text(selected)
            .icon(|ui, rect, _, _| {
                let c = pal(ui.ctx()).dim;
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, egui_phosphor::regular::CARET_DOWN, FontId::proportional(12.0), c);
            })
            .show_ui(ui, add)
    })
    .inner
}

/// Every modal dialog: the screen dimmed behind it, the title left and a close button right, then `body`.
/// Returns what `body` returns and whether the close button was clicked (Escape is the caller's, as is the footer:
/// a hint left, then Cancel and the main action on the right).
pub fn dialog<R>(ctx: &egui::Context, id: impl std::hash::Hash + std::fmt::Debug, title: &str, width: f32, body: impl FnOnce(&mut egui::Ui) -> R) -> (R, bool) {
    let p = pal(ctx);
    let frame = egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(10)
        .inner_margin(20)
        .shadow(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(0x33) });
    let mut closed = false;
    let r = egui::Modal::new(egui::Id::new(id)).frame(frame).backdrop_color(Color32::from_black_alpha(0x66)).show(ctx, |ui| {
        ui.set_width(width);
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).font(semibold(16.0)));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                closed = icon_button(ui, egui_phosphor::regular::X, "Close (Esc)").clicked();
            });
        });
        ui.add_space(12.0);
        body(ui)
    });
    (r.inner, closed)
}

/// A text field: fixed width (also inside grids, where `desired_width` alone collapses), comfortable padding.
pub fn field(text: &mut String, width: f32) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).desired_width(width).min_size(vec2(width, 32.0)).margin(vec2(10.0, 7.0)).vertical_align(egui::Align::Center)
}

/// Animated hover amount (0..1) for a custom clickable area (rows, cards).
pub fn hover_t(ui: &egui::Ui, resp: &egui::Response) -> f32 {
    ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12)
}

/// TODO text for display: markdown emphasis and code marks dropped (`**Orders:** see `x`` → `Orders: see x`).
pub fn plain_md(s: &str) -> String {
    s.replace("**", "").replace("__", "").replace('`', "")
}

/// "today", "yesterday", "5 days ago", "3 mo ago", "2 yr ago" for an ISO date.
pub fn relative(iso: &str) -> String {
    let Some(d) = pa_core::iso_days(iso) else { return String::new() };
    match pa_core::today_days() - d {
        ..=0 => "today".into(),
        1 => "yesterday".into(),
        n @ 2..=44 => format!("{n} days ago"),
        n @ 45..=364 => format!("{} mo ago", (n + 15) / 30),
        n => format!("{} yr ago", n / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("ChessClock"), "CC");
        assert_eq!(initials("trailmix-web"), "TW");
        assert_eq!(initials("weatherline"), "WE");
        assert_eq!(initials("grove.py"), "GP");
        assert_eq!(initials("VTOZ"), "VT");
    }
}
