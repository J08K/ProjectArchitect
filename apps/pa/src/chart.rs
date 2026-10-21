//! Small charts painted with egui's painter (no plotting crate): lines over days, grouped bars per week, and
//! sparklines. Colours come from the theme palette; text is caps or faint like the rest of the app.

use crate::theme::{Palette, caps};
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};

/// One line: label, colour, and `(day, value)` points in day order.
pub struct Series {
    pub label: String,
    pub color: Color32,
    pub points: Vec<(i64, f32)>,
}

/// `Oct 19` for days since 1970-01-01.
pub fn day_label(day: i64) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, m, d) = pa_core::civil_from_days(day);
    format!("{} {d}", MONTHS[(m - 1).clamp(0, 11) as usize])
}

fn legend(ui: &mut egui::Ui, items: &[(String, Color32)], p: Palette) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (label, color) in items {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, *color);
            ui.label(RichText::new(label).size(12.0).color(p.dim));
            ui.add_space(8.0);
        }
    });
}

/// Allocates a chart of `height` with a 0 / half / max grid and the max on the left; returns the plot area.
fn frame(ui: &mut egui::Ui, height: f32, max: f32, p: Palette) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter();
    let plot = Rect::from_min_max(rect.min + vec2(30.0, 4.0), rect.max - vec2(4.0, 4.0));
    for f in [0.0, 0.5, 1.0] {
        let y = plot.bottom() - f * plot.height();
        painter.hline(plot.x_range(), y, Stroke::new(1.0, p.border));
    }
    let top = if max >= 10.0 { format!("{max:.0}") } else { format!("{max:.1}").trim_end_matches(".0").to_string() };
    painter.text(pos2(rect.left(), plot.top()), egui::Align2::LEFT_CENTER, top, egui::FontId::monospace(10.5), p.faint);
    painter.text(pos2(rect.left(), plot.bottom()), egui::Align2::LEFT_CENTER, "0", egui::FontId::monospace(10.5), p.faint);
    plot
}

/// Lines over days on one y axis from 0, with a legend (label and last value) above and the first and last day
/// under it. `unit` follows the values in the legend ("%" or "").
pub fn lines(ui: &mut egui::Ui, series: &[Series], height: f32, unit: &str, p: Palette) {
    let all = || series.iter().flat_map(|s| s.points.iter());
    let (Some(first), Some(last)) = (all().map(|x| x.0).min(), all().map(|x| x.0).max()) else {
        ui.label(RichText::new("No history yet.").color(p.dim));
        return;
    };
    // Shares always run to 100 %; counts to their largest value.
    let max = if unit == "%" { 100.0 } else { all().map(|x| x.1).fold(1.0f32, f32::max) };
    let items: Vec<(String, Color32)> =
        series.iter().map(|s| (format!("{}  {}{unit}", s.label, s.points.last().map_or(0.0, |x| x.1).round()), s.color)).collect();
    legend(ui, &items, p);
    let plot = frame(ui, height, max, p);
    let span = (last - first).max(1) as f32;
    let at = |day: i64, v: f32| pos2(plot.left() + (day - first) as f32 / span * plot.width(), plot.bottom() - v / max * plot.height());
    let painter = ui.painter();
    for s in series {
        let pts: Vec<Pos2> = s.points.iter().map(|&(d, v)| at(d, v)).collect();
        if pts.len() == 1 {
            painter.circle_filled(pts[0], 3.0, s.color);
        } else {
            painter.add(egui::Shape::line(pts, Stroke::new(2.0, s.color)));
        }
    }
    ui.horizontal(|ui| {
        ui.add_space(30.0);
        ui.label(caps(day_label(first), p.faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(caps(day_label(last), p.faint)));
    });
}

/// Grouped bars, one group per week (oldest first, the last is this week), a bar per series; totals in the legend.
pub fn weekly_bars(ui: &mut egui::Ui, series: &[(&str, Color32, &[usize])], height: f32, p: Palette) {
    let weeks = series.iter().map(|s| s.2.len()).max().unwrap_or(0);
    if weeks == 0 {
        return;
    }
    let items: Vec<(String, Color32)> = series.iter().map(|s| (format!("{}  {}", s.0, s.2.iter().sum::<usize>()), s.1)).collect();
    legend(ui, &items, p);
    let max = series.iter().flat_map(|s| s.2.iter()).copied().max().unwrap_or(0).max(1) as f32;
    let plot = frame(ui, height, max, p);
    let group = plot.width() / weeks as f32;
    let bar = (group * 0.8 / series.len() as f32).min(18.0);
    let painter = ui.painter();
    for w in 0..weeks {
        let x0 = plot.left() + w as f32 * group + (group - bar * series.len() as f32) / 2.0;
        for (k, (_, color, values)) in series.iter().enumerate() {
            let v = values.get(w).copied().unwrap_or(0) as f32;
            if v > 0.0 {
                let h = v / max * plot.height();
                let r = Rect::from_min_size(pos2(x0 + k as f32 * bar, plot.bottom() - h), vec2(bar - 2.0, h));
                painter.rect_filled(r, 2, *color);
            }
        }
    }
    ui.horizontal(|ui| {
        ui.add_space(30.0);
        ui.label(caps(format!("{weeks} weeks ago"), p.faint));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(caps("this week", p.faint)));
    });
}

/// A small line of `values` (evenly spaced), scaled to its own range.
pub fn spark(ui: &mut egui::Ui, values: &[f32], size: Vec2, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if values.len() >= 2 {
        let (lo, hi) = values.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let range = (hi - lo).max(1.0);
        let pts: Vec<Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| pos2(rect.left() + i as f32 / (values.len() - 1) as f32 * rect.width(), rect.bottom() - (v - lo) / range * rect.height()))
            .collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5, color)));
    }
    resp
}
