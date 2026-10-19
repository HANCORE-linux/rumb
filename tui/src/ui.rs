//! The dashboard: a btop-style overview of the installed software in a paper
//! and ink look. Every text on screen is either data or a key hint.

use std::collections::{HashMap, HashSet};
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::{App, LogLine, OmarchyMark, Severity, Subsystem, Update, ease_out_cubic, plural};
use crate::ink::{Callout, DIAL, Edge, EyePort, EyeSpec, Fiber, Model, ScreenLine, Side};
use crate::palette::{Palette, mix};

pub const MIN_WIDTH: u16 = 150;
pub const MIN_HEIGHT: u16 = 44;
/// Cell width divided by cell height, used until the terminal reports its own.
pub const CELL_ASPECT: f64 = 10.0 / 21.0;

/// An image the terminal shows over a cell rectangle.
#[derive(Debug)]
pub enum Picture {
    Astrolabe,
    Eye(EyeSpec),
    Mark(OmarchyMark),
    Specimen(Subsystem),
    Gauge { fraction: f32, label: String, reveal: f32 },
    Molecule { depends: u64, required_by: u64, seed: u32, reveal: f32 },
}

pub struct Placement {
    pub rect: Rect,
    pub picture: Picture,
}

#[derive(Default)]
pub struct Scene {
    pub placements: Vec<Placement>,
    pub callouts: Vec<Callout>,
}

#[allow(clippy::too_many_arguments)]
pub fn paint(picture: &Picture, scene: &Scene, model: &Model, anim: &crate::ink::Anim, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<tiny_skia::Pixmap> {
    match picture {
        Picture::Astrolabe => crate::ink::astrolabe(model, anim, &scene.callouts, pal, width, height, cell_height),
        Picture::Eye(spec) => crate::ink::eye(spec, pal, width, height, cell_height),
        Picture::Mark(kind) => {
            let file = match kind {
                OmarchyMark::Logo => "omarchy-logo.png",
                OmarchyMark::Wordmark => "omarchy-wordmark.png",
            };
            let mut sources: Vec<std::path::PathBuf> = std::env::var_os("RUMB_BRAND_DIR").map(|dir| std::path::Path::new(&dir).join(file)).into_iter().collect();
            if *kind == OmarchyMark::Logo {
                sources.extend(["/usr/share/omarchy/icon.png", "/usr/share/pixmaps/omarchy.png"].map(std::path::PathBuf::from));
            }
            let source = sources.iter().find_map(|path| tiny_skia::Pixmap::load_png(path).ok())?;
            crate::ink::mark(&source, pal, width, height)
        }
        Picture::Specimen(subsystem) => crate::ink::specimen(*subsystem, pal, width, height, cell_height),
        Picture::Gauge { fraction, label, reveal } => crate::ink::gauge(*fraction, label, pal, width, height, cell_height, *reveal),
        Picture::Molecule { depends, required_by, seed, reveal } => crate::ink::molecule(*depends, *required_by, *seed, pal, width, height, cell_height, *reveal),
    }
}

// --- Timing and text helpers ---

fn ms(now: Instant, then: Instant) -> f64 {
    now.saturating_duration_since(then).as_secs_f64() * 1000.0
}

fn after(start: Option<Instant>, delay: u64) -> Option<Instant> {
    start.map(|t| t + Duration::from_millis(delay))
}

/// 0..1 eased progress of something that starts `delay` ms after `start`.
fn progress(app: &App, now: Instant, start: Option<Instant>, delay: f64, duration: f64) -> f64 {
    let Some(start) = start else { return 0.0 };
    if app.skipped {
        return 1.0;
    }
    ease_out_cubic((ms(now, start) - delay) / duration)
}

const GLYPHS: [char; 16] = ['⠁', '⠂', '⠄', '⡀', '⠈', '⠐', '⠠', '⢀', '⠃', '⠅', '⠉', '⠒', '⠤', '⠑', '⠌', '⠡'];

fn hash64(value: u64) -> u64 {
    let mut x = value.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^ (x >> 29)
}

pub fn hash_str(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Text that resolves from sparse braille glyphs into its letters.
fn decode(text: &str, style: Style, app: &App, now: Instant, start: Option<Instant>, delay: f64, pal: &Palette) -> Vec<Span<'static>> {
    let Some(start) = start else { return Vec::new() };
    if app.skipped {
        return vec![Span::styled(text.to_string(), style)];
    }
    let elapsed = ms(now, start) - delay;
    let bucket = (ms(now, app.started) / 70.0) as u64;
    let noise = Style::new().fg(pal.teal_mid);
    let mut spans = Vec::new();
    let mut settled = String::new();
    for (index, ch) in text.chars().enumerate() {
        let reveal = index as f64 * 10.0;
        if ch == ' ' || elapsed >= reveal + 160.0 {
            settled.push(ch);
            continue;
        }
        if !settled.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut settled), style));
        }
        if elapsed >= reveal {
            let glyph = GLYPHS[(hash64(index as u64 * 7919 + bucket) % GLYPHS.len() as u64) as usize];
            spans.push(Span::styled(glyph.to_string(), noise));
        } else {
            spans.push(Span::raw(" "));
        }
    }
    if !settled.is_empty() {
        spans.push(Span::styled(settled, style));
    }
    spans
}

fn width(spans: &[Span]) -> u16 {
    spans.iter().map(|span| span.width() as u16).sum()
}

fn put(buf: &mut Buffer, x: u16, y: u16, spans: Vec<Span<'static>>, max: u16) {
    buf.set_line(x, y, &Line::from(spans), max);
}

fn put_right(buf: &mut Buffer, right: u16, y: u16, spans: Vec<Span<'static>>) {
    let w = width(&spans);
    buf.set_line(right.saturating_sub(w), y, &Line::from(spans), w);
}

fn fill(buf: &mut Buffer, area: Rect, color: Color) {
    buf.set_style(area, Style::new().bg(color));
}

fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn bytes(value: u64) -> String {
    let mib = value as f64 / 1_048_576.0;
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else if mib >= 1.0 {
        format!("{mib:.0} MiB")
    } else {
        format!("{:.0} KiB", value as f64 / 1024.0)
    }
}

fn signed_bytes(value: i64) -> String {
    let sign = if value >= 0 { "+" } else { "−" };
    format!("{sign}{}", bytes(value.unsigned_abs()))
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn short_date(iso: &str) -> String {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = iso.get(5..7).and_then(|m| m.parse::<usize>().ok()).unwrap_or(1);
    let day = iso.get(8..10).unwrap_or("??");
    format!("{} {day}", MONTHS[month.clamp(1, 12) - 1])
}

fn short_time(iso: &str) -> String {
    format!("{} {}", short_date(iso), iso.get(11..16).unwrap_or(""))
}

fn days_between(earlier: &str, later: &str) -> Option<i64> {
    let day_number = |iso: &str| -> Option<i64> {
        let y: i64 = iso.get(0..4)?.parse().ok()?;
        let m: i64 = iso.get(5..7)?.parse().ok()?;
        let d: i64 = iso.get(8..10)?.parse().ok()?;
        let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
        Some(365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + d)
    };
    Some(day_number(later)? - day_number(earlier)?)
}

fn short_version(version: &str) -> &str {
    version.rsplit_once('-').map_or(version, |(v, _)| v)
}

const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

fn sparkline(values: &[u64], color: Color, grow: f64, pal: &Palette) -> Vec<Span<'static>> {
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    values
        .iter()
        .map(|&value| {
            if value == 0 {
                Span::styled("▁", Style::new().fg(pal.grid))
            } else {
                let level = ((value as f64 / max as f64 * 7.0 * grow).round() as usize).min(7);
                Span::styled(BARS[level], Style::new().fg(color))
            }
        })
        .collect()
}

/// A rounded label: tinted background with caps drawn in the same tint.
fn pill(text: &str, fg: Color, bg: Color, bold: bool) -> Vec<Span<'static>> {
    let mut style = Style::new().fg(fg).bg(bg);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    vec![
        Span::styled("\u{e0b6}", Style::new().fg(bg)),
        Span::styled(text.to_string(), style),
        Span::styled("\u{e0b4}", Style::new().fg(bg)),
    ]
}

fn risk_pill(update: &Update, pal: &Palette) -> Vec<Span<'static>> {
    match update.risk.as_str() {
        "high" => pill("HIGH", pal.panel, pal.ink, true),
        "medium" => pill("MED ", pal.ink, pal.kpi, true),
        _ => vec![Span::styled(" low  ", Style::new().fg(pal.faint))],
    }
}

/// Old version dim, new version with the changed part in strong ink.
fn version_ruler(update: &Update, room: usize, pal: &Palette) -> Vec<Span<'static>> {
    let (old, new) = (short_version(&update.old), short_version(&update.new));
    let dim = Style::new().fg(pal.faint);
    let arrow = Span::styled(" → ", Style::new().fg(pal.faint));
    let strong = Style::new().fg(pal.ink).add_modifier(Modifier::BOLD);
    let plain = Style::new().fg(pal.ink2);
    if old == new {
        let rel = |v: &str| v.rsplit_once('-').map_or(String::new(), |(_, r)| r.to_string());
        let (from, to) = (format!("{old}-{}", rel(&update.old)), format!("-{}", rel(&update.new)));
        if from.chars().count() + 3 + to.chars().count() <= room {
            return vec![Span::styled(from, dim), arrow, Span::styled(to, strong)];
        }
        let lead = Span::styled("→ ", Style::new().fg(pal.faint));
        if old.chars().count() + 2 + to.chars().count() <= room {
            return vec![lead, Span::styled(old.to_string(), plain), Span::styled(to, strong)];
        }
        return vec![lead, Span::styled(to, strong)];
    }
    let old_parts: Vec<&str> = old.split('.').collect();
    let new_parts: Vec<&str> = new.split('.').collect();
    let changed = old_parts.iter().zip(&new_parts).position(|(a, b)| a != b).unwrap_or(old_parts.len().min(new_parts.len()));
    let kept = new_parts[..changed].join(".");
    let rest = new_parts[changed..].join(".");
    let mut new_spans = Vec::new();
    if !kept.is_empty() {
        new_spans.push(Span::styled(format!("{kept}."), plain));
    }
    new_spans.push(Span::styled(rest, strong));
    if old.chars().count() + 3 + new.chars().count() <= room {
        let mut spans = vec![Span::styled(old.to_string(), dim), arrow];
        spans.extend(new_spans);
        spans
    } else {
        let mut spans = vec![Span::styled("→ ", Style::new().fg(pal.faint))];
        if new.chars().count() + 2 <= room {
            spans.extend(new_spans);
        } else {
            spans.push(Span::styled(fit(new, room.saturating_sub(2)), strong));
        }
        spans
    }
}

// --- Frames ---

const ROUND: [&str; 6] = ["─", "│", "╭", "╮", "╰", "╯"];
const HEAVY: [&str; 6] = ["━", "┃", "┏", "┓", "┗", "┛"];

fn border(buf: &mut Buffer, area: Rect, focused: bool, pal: &Palette) {
    if focused {
        outline(buf, area, HEAVY, pal.ink);
    } else {
        outline(buf, area, ROUND, pal.rule);
    }
}

fn outline(buf: &mut Buffer, area: Rect, [h, v, tl, tr, bl, br]: [&str; 6], color: Color) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    for x in area.x + 1..area.right() - 1 {
        buf[(x, area.y)].set_symbol(h).set_fg(color);
        buf[(x, area.bottom() - 1)].set_symbol(h).set_fg(color);
    }
    for y in area.y + 1..area.bottom() - 1 {
        buf[(area.x, y)].set_symbol(v).set_fg(color);
        buf[(area.right() - 1, y)].set_symbol(v).set_fg(color);
    }
    buf[(area.x, area.y)].set_symbol(tl).set_fg(color);
    buf[(area.right() - 1, area.y)].set_symbol(tr).set_fg(color);
    buf[(area.x, area.bottom() - 1)].set_symbol(bl).set_fg(color);
    buf[(area.right() - 1, area.bottom() - 1)].set_symbol(br).set_fg(color);
}

fn focus_tag(buf: &mut Buffer, area: Rect, pal: &Palette) {
    let tag = vec![Span::styled(" ⏎ details ", Style::new().fg(pal.panel).bg(pal.ink).add_modifier(Modifier::BOLD))];
    put_right(buf, area.right() - 2, area.y, tag);
}

/// A bordered tile with its title in the first row; returns the content area.
#[allow(clippy::too_many_arguments)]
fn tile(buf: &mut Buffer, area: Rect, focused: bool, title: &str, right: Vec<Span<'static>>, app: &App, pal: &Palette, now: Instant) -> Rect {
    fill(buf, area, pal.panel);
    border(buf, area, focused, pal);
    let style = Style::new().fg(pal.ink).add_modifier(Modifier::BOLD);
    put(buf, area.x + 2, area.y + 1, decode(title, style, app, now, Some(app.started), 120.0, pal), area.width.saturating_sub(4));
    if !right.is_empty() {
        put_right(buf, area.right() - 2, area.y + 1, right);
    }
    if focused {
        focus_tag(buf, area, pal);
    }
    Rect::new(area.x + 2, area.y + 2, area.width.saturating_sub(4), area.height.saturating_sub(3))
}

/// A tile with a black title bar, for the lists along the bottom.
#[allow(clippy::too_many_arguments)]
fn bar_tile(buf: &mut Buffer, area: Rect, focused: bool, title: &str, right: Vec<Span<'static>>, app: &App, pal: &Palette, now: Instant) -> Rect {
    fill(buf, area, pal.panel);
    border(buf, area, focused, pal);
    let bar = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1);
    fill(buf, bar, pal.bar);
    let style = Style::new().fg(pal.bar_text).add_modifier(Modifier::BOLD);
    put(buf, bar.x + 1, bar.y, decode(title, style, app, now, Some(app.started), 120.0, pal), bar.width.saturating_sub(2));
    if !right.is_empty() {
        put_right(buf, bar.right() - 1, bar.y, right);
    }
    if focused {
        focus_tag(buf, area, pal);
    }
    Rect::new(area.x + 2, area.y + 2, area.width.saturating_sub(4), area.height.saturating_sub(3))
}

// --- Views and layout ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Panel {
    Pending,
    Journal,
    Installed,
    News,
    Preflight,
    Subsystems,
    Scan,
    Problems,
    Activity,
    Repos,
    Deps,
    Eye,
    Log,
}

/// The views, switched with the number keys like the presets in btop.
pub const VIEWS: [&str; 5] = ["overview", "updates", "problems", "packages", "live"];

/// The tiles of a view in reading order; tab walks them in this order.
pub fn panels_of(view: usize) -> &'static [Panel] {
    use Panel::*;
    match view {
        0 => &[Pending, Problems, Preflight, News, Installed, Journal],
        1 => &[Scan, Pending, News, Preflight],
        2 => &[Problems, Activity, Journal],
        3 => &[Subsystems, Installed, Repos, Deps],
        _ => &[Eye, Log],
    }
}

/// Direction bits of a wire cell.
const L: u8 = 1;
const R: u8 = 2;
const U: u8 = 4;
const D: u8 = 8;

/// A continuous run of wire cells, with the directions it leaves its first
/// and last cell in.
#[derive(Clone)]
struct Run {
    cells: Vec<(u16, u16)>,
    start: u8,
    end: u8,
}

/// A wire from the eye to a tile, then from tile to tile.
pub struct Wire {
    panel: Panel,
    runs: Vec<Run>,
    /// Border cells where the wire passes into tiles, with their glyph directions.
    ports: Vec<((u16, u16), u8)>,
}

impl Wire {
    fn cells(&self) -> Vec<(u16, u16)> {
        self.runs.iter().flat_map(|run| run.cells.iter().copied()).collect()
    }
}

pub struct Layout {
    pub title: Rect,
    pub panels: Vec<(Panel, Rect)>,
    /// The eye the wires start from: on the life line, or the big eye in the live view.
    pub eye: Rect,
    /// The life line across the overview, and where things go on it.
    pub band: Option<Rect>,
    pub plan: Option<BandPlan>,
    pub footer: Rect,
    pub wires: Vec<Wire>,
}

/// `n` columns across `w` with a one-cell gap between them.
fn columns(x: u16, w: u16, n: u16, y: u16, h: u16) -> Vec<Rect> {
    let each = w.saturating_sub(n - 1) / n;
    (0..n)
        .map(|i| {
            let cx = x + i * (each + 1);
            let cw = if i == n - 1 { (x + w).saturating_sub(cx) } else { each };
            Rect::new(cx, y, cw, h)
        })
        .collect()
}

pub fn layout(app: &App, area: Rect) -> Layout {
    let view = app.view;
    let x = area.x + 1;
    let w = area.width.saturating_sub(2);
    let title = Rect::new(x, area.y + 1, w, 3);
    let footer = Rect::new(x, area.bottom().saturating_sub(1), w, 1);
    if view == 0 {
        return life_layout(app, area, title, footer);
    }
    let top = area.y + 5;
    let height = footer.y.saturating_sub(top + 1);
    let full = |y: u16, h: u16| Rect::new(x, y, w, h);
    let rects: Vec<Rect> = match view {
        1 => {
            let low = 8.min(height / 3);
            let high = height.saturating_sub(low + 1);
            let scan_w = w * 3 / 5;
            let mut rects = vec![Rect::new(x, top, scan_w, high), Rect::new(x + scan_w + 1, top, w.saturating_sub(scan_w + 1), high)];
            rects.extend(columns(x, w, 2, top + high + 1, low));
            rects
        }
        2 => {
            let high = height * 9 / 20;
            let mut rects = columns(x, w, 2, top, high);
            rects.push(full(top + high + 1, height.saturating_sub(high + 1)));
            rects
        }
        3 => {
            let left_w = w * 3 / 5;
            let (right_x, right_w) = (x + left_w + 1, w.saturating_sub(left_w + 1));
            let repos = app.anatomy.as_ref().map_or(4, |a| a.repos.len()).clamp(4, 8) as u16;
            let installed_h = 9.min(height / 3);
            let repos_h = (repos + 3).min(height / 4);
            let deps_y = top + installed_h + repos_h + 2;
            vec![
                Rect::new(x, top, left_w, height),
                Rect::new(right_x, top, right_w, installed_h),
                Rect::new(right_x, top + installed_h + 1, right_w, repos_h),
                Rect::new(right_x, deps_y, right_w, (top + height).saturating_sub(deps_y)),
            ]
        }
        _ => {
            let eye_w = w * 58 / 100;
            vec![Rect::new(x, top, eye_w, height), Rect::new(x + eye_w + 1, top, w.saturating_sub(eye_w + 1), height)]
        }
    };
    let panels: Vec<(Panel, Rect)> = panels_of(view).iter().copied().zip(rects).collect();
    let eye = panels.iter().find(|(panel, _)| *panel == Panel::Eye).map_or(Rect::default(), |(_, area)| big_eye(*area));
    let wires = wires(eye, &panels, top - 1);
    Layout { title, panels, eye, band: None, plan: None, footer, wires }
}

/// The overview: tiles above and below the life line, no number strip. Wide
/// tiles for the long lists, a narrow column where the checks sit over the news.
fn life_layout(app: &App, area: Rect, title: Rect, footer: Rect) -> Layout {
    let x = area.x + 1;
    let w = area.width.saturating_sub(2);
    let top = area.y + 5;
    let height = footer.y.saturating_sub(top + 1);
    let band_h = 8;
    let above_h = height.saturating_sub(band_h + 2) / 2;
    let below_h = height.saturating_sub(band_h + 2 + above_h);
    let band = Rect::new(x, top + above_h + 1, w, band_h);
    let pending_w = w * 36 / 100;
    let problems_w = w * 37 / 100;
    let side_x = x + pending_w + 1 + problems_w + 1;
    let side_w = (x + w).saturating_sub(side_x);
    let checks_h = above_h.saturating_sub(1) / 2;
    let news_h = above_h.saturating_sub(checks_h + 1);
    let below_y = band.bottom() + 1;
    let installed_w = w * 36 / 100;
    let rects = vec![
        Rect::new(x, top, pending_w, above_h),
        Rect::new(x + pending_w + 1, top, problems_w, above_h),
        Rect::new(side_x, top, side_w, checks_h),
        Rect::new(side_x, top + checks_h + 1, side_w, news_h),
        Rect::new(x, below_y, installed_w, below_h),
        Rect::new(x + installed_w + 1, below_y, w.saturating_sub(installed_w + 1), below_h),
    ];
    let panels: Vec<(Panel, Rect)> = panels_of(0).iter().copied().zip(rects).collect();
    let eye_w = 11.min(w);
    let eye = Rect::new(band.right().saturating_sub(eye_w + 1), band.y + 3, eye_w, 3);
    let plan = band_plan(app, band, eye);
    let wires = band_wires(app, &plan, band, &panels);
    Layout { title, panels, eye, band: Some(band), plan: Some(plan), footer, wires }
}

/// The big eye inside its tile, with room for the legend below.
fn big_eye(area: Rect) -> Rect {
    let mut w = area.width.saturating_sub(2);
    if w.is_multiple_of(2) {
        w = w.saturating_sub(1);
    }
    Rect::new(area.x + 1, area.y + 1, w, area.height.saturating_sub(5))
}

/// Columns after `from` up to and including `to`.
fn steps(from: u16, to: u16) -> Vec<u16> {
    if to > from { (from + 1..=to).collect() } else { (to..from).rev().collect() }
}

/// Every cell along a path of horizontal and vertical legs.
fn route(points: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut cells = vec![points[0]];
    for leg in points.windows(2) {
        let (from, to) = (leg[0], leg[1]);
        if from.1 == to.1 {
            cells.extend(steps(from.0, to.0).into_iter().map(|cx| (cx, from.1)));
        } else {
            cells.extend(steps(from.1, to.1).into_iter().map(|cy| (from.0, cy)));
        }
    }
    cells
}

fn overlap(a: Rect, b: Rect) -> u16 {
    a.right().min(b.right()).saturating_sub(a.x.max(b.x))
}

fn wires(eye: Rect, panels: &[(Panel, Rect)], bus: u16) -> Vec<Wire> {
    let mut wires: Vec<Wire> = Vec::new();
    if eye.width < 3 || eye.height < 3 {
        return wires;
    }
    // The trunk: up from the top of the big eye to the bus
    let start = (eye.x + eye.width / 2, eye.y.saturating_sub(1));
    let to = |column: u16| route(&[start, (start.0, bus), (column, bus)]);
    let mut placed: Vec<(Rect, usize)> = Vec::new();
    for (panel, area) in panels {
        if *panel == Panel::Eye || area.width < 3 || area.height < 3 {
            continue;
        }
        if area.y == bus + 1 {
            let column = area.x + area.width / 2;
            wires.push(Wire {
                panel: *panel,
                runs: vec![Run { cells: to(column), start: D, end: D }],
                ports: vec![((column, area.y), L | R | U)],
            });
        } else {
            // Below another tile: one short link through the gap between them,
            // from the tile above that covers it most, the most central of
            // those that cover it about as much
            let center = i32::from(area.x + area.width / 2);
            let distance = |r: &Rect| (i32::from(r.x + r.width / 2) - center).abs();
            let above: Vec<&(Rect, usize)> = placed.iter().filter(|(r, _)| r.bottom() + 1 == area.y && overlap(*r, *area) > 2).collect();
            let most = above.iter().map(|(r, _)| overlap(*r, *area)).max().unwrap_or(0);
            let Some(&&(above, index)) = above.iter().filter(|(r, _)| overlap(*r, *area) * 10 >= most * 9).min_by_key(|(r, _)| distance(r)) else {
                continue;
            };
            let link = above.x.max(area.x) + overlap(above, *area) / 2;
            let mut runs = wires[index].runs.clone();
            runs.push(Run { cells: vec![(link, area.y - 1)], start: U, end: D });
            let mut ports = wires[index].ports.clone();
            ports.extend([((link, above.bottom() - 1), L | R | D), ((link, area.y), L | R | U)]);
            wires.push(Wire { panel: *panel, runs, ports });
        }
        placed.push((*area, wires.len() - 1));
    }
    wires
}

// --- The life line (overview) ---

/// The band measures what happened since the last full upgrade: problems per
/// day above the axis, packages upgraded per day below it, the axis running
/// from the last full upgrade (◆) to now, where the eye sits. Rails above and
/// below carry each tile's link to the marks that are that tile's past.
pub struct BandPlan {
    pub axis: u16,
    top_rail: u16,
    bottom_rail: u16,
    slot: u16,
    /// Days shown back from today; at least two weeks, longer when the last
    /// full upgrade is older.
    days: i64,
    first_x: u16,
    eye_column: u16,
}

impl BandPlan {
    fn slot_x(&self, ago: i64) -> Option<u16> {
        (0..=self.days).contains(&ago).then(|| self.first_x + (self.days - ago) as u16 * self.slot)
    }

    fn center(&self, ago: i64) -> Option<u16> {
        self.slot_x(ago).map(|x| x + self.slot / 2)
    }

    fn bar_x(&self, ago: i64) -> Option<u16> {
        self.slot_x(ago).map(|x| x + self.slot.saturating_sub(BAR_W) / 2)
    }
}

/// Width of the row labels at the start of the band, and of a day's bar.
const LABEL_W: u16 = 16;
const BAR_W: u16 = 3;

fn today(app: &App) -> Option<String> {
    app.scan_time.as_deref().map(|iso| iso.get(..10).unwrap_or(iso).to_string())
}

/// Days before today, for a date.
fn days_ago(app: &App, date: &str) -> Option<i64> {
    days_between(date, &today(app)?)
}

/// Days since the last full upgrade: from the package log, or none once a
/// full upgrade has completed while rumb watched.
fn since_full_upgrade(app: &App) -> Option<i64> {
    if app.full_upgrade_at.is_some() {
        return Some(0);
    }
    app.last_upgrade.as_deref().and_then(|d| days_ago(app, d))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The date `ago` days before today, as "oct 10".
fn date_ago(app: &App, ago: i64) -> Option<String> {
    let today = today(app)?;
    let part = |range: std::ops::Range<usize>| today.get(range).and_then(|s| s.parse::<i64>().ok());
    let (y, m, d) = civil_from_days(days_from_civil(part(0..4)?, part(5..7)?, part(8..10)?) - ago);
    Some(short_date(&format!("{y:04}-{m:02}-{d:02}")))
}

fn band_plan(app: &App, band: Rect, eye: Rect) -> BandPlan {
    let start = band.x + LABEL_W;
    let end = eye.x.saturating_sub(2);
    let covered = app.anatomy.as_ref().map_or(29, |a| a.upgrades_per_day.len().saturating_sub(1).max(14) as i64);
    let fits = i64::from(end.saturating_sub(start) / 3).saturating_sub(1).max(1);
    let days = since_full_upgrade(app).unwrap_or(14).max(14).min(covered).min(fits);
    let slot = (end.saturating_sub(start) / (days as u16 + 1)).clamp(3, 12);
    let first_x = end.saturating_sub(slot * (days as u16 + 1));
    BandPlan { axis: band.y + 4, top_rail: band.y, bottom_rail: band.bottom() - 1, slot, days, first_x, eye_column: eye.x + eye.width / 2 }
}

/// Every tile lands on the band once, on the marks that are its own past:
/// problems on the bars, installed software at the last full upgrade, the
/// checks and news at the eye from above, the journal at the eye from below,
/// pending changes on the top rail that carries updates into the eye.
fn band_wires(app: &App, plan: &BandPlan, band: Rect, panels: &[(Panel, Rect)]) -> Vec<Wire> {
    let eye = plan.eye_column;
    let near = |area: &Rect| area.bottom() + 1 == band.y || area.y == band.bottom() + 1;
    let landing = |panel: Panel, area: &Rect| -> u16 {
        let wanted = match panel {
            Panel::Problems => plan.first_x + plan.slot * (plan.days as u16 + 1) / 2,
            Panel::Installed => since_full_upgrade(app).and_then(|ago| plan.center(ago)).unwrap_or(plan.first_x),
            Panel::News | Panel::Preflight | Panel::Journal => eye,
            _ => area.x + area.width / 2,
        };
        wanted.clamp(area.x + 2, area.right().saturating_sub(3))
    };
    let mut wires: Vec<Wire> = Vec::new();
    let mut placed: Vec<(Rect, usize)> = Vec::new();
    for (panel, area) in panels.iter().filter(|(_, area)| near(area)) {
        let column = landing(*panel, area);
        let (cells, start, end, port) = if area.bottom() <= band.y {
            (route(&[(eye, plan.axis - 2), (eye, plan.top_rail), (column, plan.top_rail), (column, band.y - 1)]), D, U, ((column, area.bottom() - 1), L | R | D))
        } else {
            (route(&[(eye, plan.axis + 2), (eye, plan.bottom_rail), (column, plan.bottom_rail), (column, band.bottom())]), U, D, ((column, area.y), L | R | U))
        };
        wires.push(Wire { panel: *panel, runs: vec![Run { cells, start, end }], ports: vec![port] });
        placed.push((*area, wires.len() - 1));
    }
    for (panel, area) in panels.iter().filter(|(_, area)| !near(area)) {
        let above = area.bottom() <= band.y;
        let touching = |r: &Rect| if above { r.y == area.bottom() + 1 } else { r.bottom() + 1 == area.y };
        let Some(&(next, index)) = placed.iter().filter(|(r, _)| touching(r) && overlap(*r, *area) > 2).max_by_key(|(r, _)| overlap(*r, *area)) else { continue };
        let own = wires[index].runs.first().and_then(|run| run.cells.last()).map(|cell| cell.0);
        let link = own.filter(|x| *x >= area.x + 2 && *x + 3 <= area.right()).unwrap_or(next.x.max(area.x) + overlap(next, *area) / 2);
        let mut runs = wires[index].runs.clone();
        let mut ports = wires[index].ports.clone();
        if above {
            runs.push(Run { cells: vec![(link, area.bottom())], start: D, end: U });
            ports.extend([((link, next.y), L | R | U), ((link, area.bottom() - 1), L | R | D)]);
        } else {
            runs.push(Run { cells: vec![(link, area.y - 1)], start: U, end: D });
            ports.extend([((link, next.bottom() - 1), L | R | D), ((link, area.y), L | R | U)]);
        }
        wires.push(Wire { panel: *panel, runs, ports });
    }
    wires
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Freshness {
    Collecting,
    Current,
    Stale,
    Failed,
}

const STALE_MS: f64 = 30.0 * 60.0 * 1000.0;

/// How fresh a tile's data is: still being collected, current, old, or the
/// check failed.
fn freshness(app: &App, panel: Panel, now: Instant) -> Freshness {
    let from = |done: Option<Instant>, failed: bool| match done {
        _ if failed => Freshness::Failed,
        None => Freshness::Collecting,
        Some(at) if ms(now, at) > STALE_MS => Freshness::Stale,
        Some(_) => Freshness::Current,
    };
    match panel {
        Panel::Pending | Panel::Scan | Panel::Subsystems => from(app.stage_done_at[0], !app.update_errors.is_empty()),
        Panel::Problems | Panel::Activity => from(app.stage_done_at[1], false),
        Panel::News => from(app.stage_done_at[2], app.news_failed),
        Panel::Preflight => from(app.stage_done_at[3], false),
        Panel::Installed | Panel::Repos | Panel::Deps | Panel::Journal => from(app.anatomy_at, false),
        Panel::Log | Panel::Eye => Freshness::Current,
    }
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn freshness_glyph(state: Freshness, app: &App, now: Instant, pal: &Palette) -> (&'static str, Color) {
    match state {
        Freshness::Collecting => (SPINNER[(ms(now, app.started) / 90.0) as usize % SPINNER.len()], pal.teal_mid),
        Freshness::Current => ("▪", pal.teal),
        Freshness::Stale => ("▫", pal.label),
        Freshness::Failed => ("×", pal.alert),
    }
}

/// A mark at the start of a title bar when that tile's data is not current:
/// still being collected, old, or its check failed.
fn draw_freshness(buf: &mut Buffer, layout: &Layout, app: &App, pal: &Palette, now: Instant) {
    let area = buf.area;
    for (panel, rect) in &layout.panels {
        let cell = (rect.x + 1, rect.y + 1);
        let state = freshness(app, *panel, now);
        if state == Freshness::Current || rect.width < 4 || rect.height < 3 || cell.0 >= area.right() || cell.1 >= area.bottom() {
            continue;
        }
        let (glyph, color) = freshness_glyph(state, app, now, pal);
        buf[cell].set_symbol(glyph).set_fg(color);
    }
}

/// What the eye is doing, written under it.
fn eye_status(app: &App) -> (&'static str, bool) {
    match app.transaction {
        Some((_, None)) => ("now · updating", true),
        _ if app.finished_at.is_none() => ("now · scanning", true),
        _ => ("now · monitoring", false),
    }
}

/// The band: problems per day, the axis from the last full upgrade to now,
/// packages upgraded per day, dates, and the eye at now.
#[allow(clippy::too_many_arguments)]
fn draw_band(buf: &mut Buffer, layout: &Layout, plan: &BandPlan, band: Rect, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    let eye = layout.eye;
    let axis = plan.axis;
    let label = Style::new().fg(pal.label);
    let number = Style::new().fg(pal.ink);
    let since = since_full_upgrade(app);

    for x in band.x..band.right() {
        buf[(x, plan.top_rail)].set_symbol("─").set_fg(pal.grid);
        buf[(x, plan.bottom_rail)].set_symbol("─").set_fg(pal.grid);
    }
    let (problem_row, upgrade_row) = (axis - 3, axis + 1);
    put(buf, band.x, problem_row, vec![Span::styled("problems / day", label)], LABEL_W);
    put(buf, band.x, upgrade_row, vec![Span::styled("upgrades / day", label)], LABEL_W);
    let axis_start = plan.center(plan.days).unwrap_or(plan.first_x);
    for x in axis_start..eye.x {
        buf[(x, axis)].set_symbol("━").set_fg(pal.ink2);
    }
    // While scanning, a light runs along the axis towards now
    if app.finished_at.is_none() && !app.skipped {
        let span = eye.x.saturating_sub(axis_start).max(1);
        let head = axis_start + ((ms(now, app.started) / 1600.0).fract() * f64::from(span)) as u16;
        for x in head.saturating_sub(4)..=head.min(eye.x.saturating_sub(1)) {
            buf[(x, axis)].set_fg(pal.teal);
        }
    }

    // Problems per day; each journal error that comes in grows today's bar
    if let Some(done) = app.stage_done_at[1] {
        let per_day = problems_per_day(app, now);
        let max = per_day.iter().filter(|(ago, _)| **ago <= plan.days).map(|(_, (c, e))| c + e).max().unwrap_or(1).max(1) as f64;
        let grow = progress(app, now, Some(done), 0.0, 900.0);
        let mut totals = Vec::new();
        for (ago, (crashes, errors)) in &per_day {
            let Some(x) = plan.bar_x(*ago) else { continue };
            totals.push((*ago, ((crashes + errors) as f64 * grow).round() as u64));
            let total = (((crashes + errors) as f64 / max) * 16.0 * grow).round() as u16;
            let crash = ((*crashes as f64 / max) * 16.0 * grow).round() as u16;
            for row in 0..2u16 {
                let floor = row * 8;
                if total <= floor {
                    break;
                }
                let fill = (total - floor).min(8);
                let crash_fill = crash.saturating_sub(floor).min(8);
                // Crashes sit at the bottom of the bar; where both share a full
                // cell, the crash part is the glyph and the errors its background
                let (glyph, fg, bg) = if crash_fill >= fill {
                    (fill, pal.ink, None)
                } else if crash_fill > 0 && fill == 8 {
                    (crash_fill, pal.ink, Some(pal.teal_mid))
                } else if crash_fill * 2 >= fill {
                    (fill, pal.ink, None)
                } else {
                    (fill, pal.teal_mid, None)
                };
                for dx in 0..BAR_W {
                    let cell = &mut buf[(x + dx, axis - 1 - row)];
                    cell.set_symbol(BARS[glyph as usize - 1]).set_fg(fg);
                    if let Some(bg) = bg {
                        cell.set_bg(bg);
                    }
                }
            }
        }
        draw_counts(buf, plan, problem_row, &totals, number);
    }

    // Packages upgraded per day; a running transaction counts for today
    // until the next scan
    if let Some(anatomy) = &app.anatomy {
        let rescanned = app.anatomy_at.zip(app.transaction.and_then(|(_, end)| end)).is_some_and(|(at, end)| at > end);
        let live = if rescanned { 0 } else { app.applied.len() as u64 };
        let mut totals: Vec<(i64, u64)> = anatomy.upgrades_per_day.iter().rev().enumerate().map(|(ago, count)| (ago as i64, *count)).collect();
        match totals.first_mut() {
            Some((0, today)) => *today += live,
            _ => totals.push((0, live)),
        }
        draw_counts(buf, plan, upgrade_row, &totals, number);
    }

    // A package confirmed installed rides the top rail from the pending tile
    // into the eye
    if let Some(start) = layout.wires.iter().find(|wire| wire.panel == Panel::Pending).and_then(|wire| wire.runs.first()).and_then(|run| run.cells.last()).map(|cell| cell.0) {
        for (_, at) in &app.applied {
            let t = ms(now, *at) / 600.0;
            if app.skipped || !(0.0..1.0).contains(&t) {
                continue;
            }
            let x = start + ((f64::from(plan.eye_column.saturating_sub(start)) * ease_out_cubic(t)) as u16);
            buf[(x, plan.top_rail)].set_symbol("●").set_fg(pal.ink);
        }
    }

    // Under the axis: the last full upgrade and news by name, dates every
    // week, or the line the package manager just wrote; under the eye, what
    // it is doing
    let row = axis + 2;
    let (status, active) = eye_status(app);
    let status_w = status.chars().count() as u16;
    let status_x = plan.eye_column.saturating_sub(status_w + 1);
    put(buf, status_x, row, vec![Span::styled(status, Style::new().fg(if active { pal.teal } else { pal.label }))], status_w);
    let mut taken: Vec<(u16, u16)> = vec![(status_x.saturating_sub(2), band.right())];
    // Names of marks may move left of something in their way; dates may not,
    // or they would sit under the wrong day
    let place = |buf: &mut Buffer, x: u16, spans: Vec<Span<'static>>, taken: &mut Vec<(u16, u16)>, shift: u16| {
        let w = width(&spans);
        let mut at = x;
        while let Some(from) = taken.iter().filter(|(from, to)| at <= *to && at + w + 1 >= *from).map(|(from, _)| *from).min() {
            let left = from.saturating_sub(w + 2);
            if left >= at || x - left > shift {
                return;
            }
            at = left;
        }
        if at < band.x {
            return;
        }
        taken.push((at, at + w));
        put(buf, at, row, spans, w);
    };
    if let Some(ago) = since {
        let when = if ago == 0 && app.full_upgrade_at.is_some() { "just now".to_string() } else { format!("{} · {ago} d", date_ago(app, ago).unwrap_or_default()) };
        let (x, mark) = plan.center(ago).map_or((axis_start, "◂ ◆ "), |x| (x, "◆ "));
        place(buf, x, vec![Span::styled(mark, Style::new().fg(pal.ink)), Span::styled(format!("last full upgrade · {when}"), label)], &mut taken, u16::MAX);
    }
    let caption = match app.transaction {
        Some((_, ended)) if ended.is_none_or(|end| ms(now, end) < 4000.0) => app.feed.iter().rev().find(|(at, _)| *at <= now).map(|(_, line)| readable(line, false)),
        _ if app.finished_at.is_none() => app.steps.last().map(|step| format!("{}…", step.label)),
        _ => None,
    };
    if let Some(text) = caption {
        let text = fit(&text, 60);
        let x = status_x.saturating_sub(text.chars().count() as u16 + 4);
        place(buf, x, vec![Span::styled(text, Style::new().fg(pal.ink2))], &mut taken, 0);
    }
    for news in &app.news {
        if let Some((ago, x)) = days_ago(app, &news.date).and_then(|ago| Some((ago, plan.center(ago)?))) {
            let spans = vec![Span::styled("◇ ", Style::new().fg(pal.ink2)), Span::styled(format!("news · {ago} d"), label)];
            let shift = width(&spans).saturating_sub(2);
            place(buf, x, spans, &mut taken, shift);
        }
    }
    let mut ago = 7;
    while let Some(x) = plan.center(ago) {
        if let Some(date) = date_ago(app, ago) {
            place(buf, x.saturating_sub(3), vec![Span::styled(date, label)], &mut taken, 0);
        }
        ago += 7;
    }

    // Marks on the axis: the last full upgrade, news
    if let Some(x) = since.and_then(|ago| plan.center(ago)) {
        buf[(x, axis)].set_symbol("◆").set_fg(pal.ink);
    }
    for news in &app.news {
        if let Some(x) = days_ago(app, &news.date).and_then(|ago| plan.center(ago)) {
            buf[(x, axis)].set_symbol("◇").set_fg(pal.ink2);
        }
    }

    scene.placements.push(Placement { rect: eye, picture: Picture::Eye(eye_spec(app, layout, eye, false, now)) });
}

fn draw_counts(buf: &mut Buffer, plan: &BandPlan, row: u16, totals: &[(i64, u64)], style: Style) {
    let mut days: Vec<(u16, String)> = totals.iter().filter(|(_, count)| *count > 0).filter_map(|(ago, count)| Some((plan.center(*ago)?, compact(*count)))).collect();
    days.sort_by_key(|(center, _)| *center);
    let mut free = 0;
    for (center, text) in days {
        let w = text.chars().count() as u16;
        let x = center.saturating_sub(w.saturating_sub(1) / 2);
        if x < free {
            continue;
        }
        put(buf, x, row, vec![Span::styled(text, style)], w);
        free = x + w + 1;
    }
}

fn compact(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=9_999 => format!("{}.{}k", value / 1000, value % 1000 / 100),
        10_000..=999_999 => format!("{}k", value / 1000),
        _ => format!("{}M", value / 1_000_000),
    }
}

fn problems_per_day(app: &App, now: Instant) -> HashMap<i64, (u64, u64)> {
    let mut per_day: HashMap<i64, (u64, u64)> = HashMap::new();
    for day in &app.activity {
        if let Some(ago) = days_ago(app, &day.date) {
            per_day.insert(ago, (day.crashes, day.errors));
        }
    }
    let late = journal_after_scan(app, now) as u64;
    if late > 0 {
        per_day.entry(0).or_insert((0, 0)).1 += late;
    }
    per_day
}

fn journal_after_scan(app: &App, now: Instant) -> usize {
    let total = app.anatomy.as_ref().map_or(0, |a| a.journal.len());
    if !app.demo {
        return app.journal_live;
    }
    if app.journal_at.is_none() {
        return 0;
    }
    live_journal_count(app, now).min(total).saturating_sub(total.saturating_sub(3))
}

/// A package manager line in plain words: "upgraded hyprland 0.55.2 → 0.56.0".
fn readable(line: &str, both: bool) -> String {
    let short = |version: &str| version.rsplit_once('-').map_or(version, |(v, _)| v).to_string();
    let mut words = line.splitn(3, ' ');
    let (action, name, rest) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
    match rest.trim_matches(|c| c == '(' || c == ')').split_once(" -> ") {
        Some((old, new)) if both => format!("{action} {name} {} → {}", short(old), short(new)),
        Some((_, new)) => format!("{action} {name} → {}", short(new)),
        None => line.trim_end_matches("...").replace('\'', ""),
    }
}

// --- Wires and signals ---

fn toward(from: (u16, u16), to: (u16, u16)) -> u8 {
    if to.0 < from.0 {
        L
    } else if to.0 > from.0 {
        R
    } else if to.1 < from.1 {
        U
    } else {
        D
    }
}

fn glyph(dirs: u8, heavy: bool) -> &'static str {
    let set = if heavy {
        ["━", "┃", "┏", "┓", "┗", "┛", "┳", "┻", "┣", "┫", "╋"]
    } else {
        ["─", "│", "╭", "╮", "╰", "╯", "┬", "┴", "├", "┤", "┼"]
    };
    let index = match dirs {
        x if x == L | R || x == L || x == R => 0,
        x if x == U | D || x == U || x == D => 1,
        x if x == R | D => 2,
        x if x == L | D => 3,
        x if x == R | U => 4,
        x if x == L | U => 5,
        x if x == L | R | D => 6,
        x if x == L | R | U => 7,
        x if x == U | D | R => 8,
        x if x == U | D | L => 9,
        _ => 10,
    };
    set[index]
}

/// Joined directions of every wire cell, so shared cells become junctions.
fn wire_dirs(wires: &[Wire]) -> HashMap<(u16, u16), u8> {
    let mut map = HashMap::new();
    for wire in wires {
        for run in &wire.runs {
            let last = run.cells.len().saturating_sub(1);
            for (i, &cell) in run.cells.iter().enumerate() {
                let back = if i == 0 { run.start } else { toward(cell, run.cells[i - 1]) };
                let on = if i == last { run.end } else { toward(cell, run.cells[i + 1]) };
                *map.entry(cell).or_insert(0) |= back | on;
            }
        }
    }
    map
}

const IMAGE_MS: f64 = 220.0;
const CELLS_PER_MS: f64 = 0.11;
const FLASH_MS: f64 = 400.0;
const GLANCE_MS: f64 = 1100.0;
const GAZE_MS: f64 = 380.0;
const BLINK_EVERY_MS: f64 = 6800.0;
const RETRACT_MS: f64 = 700.0;

/// When the demo journal received its latest line, up to `now`.
fn journal_arrival(app: &App, now: Instant) -> Option<Instant> {
    if !app.demo {
        return app.journal_last.filter(|at| !app.skipped && *at <= now);
    }
    let at = app.journal_at?;
    if app.skipped || now < at {
        return None;
    }
    let total = app.anatomy.as_ref().map_or(0, |a| a.journal.len());
    let k = ((ms(now, at) / 2500.0) as u64).min(total.min(3) as u64);
    (k > 0).then(|| at + Duration::from_millis(k * 2500))
}

/// When a tile last received data.
fn last_event(app: &App, panel: Panel, now: Instant) -> Option<Instant> {
    let stage = |i: usize| app.stage_done_at[i];
    let applied = app.applied.iter().map(|(_, at)| *at).filter(|at| *at <= now).max();
    let times = match panel {
        Panel::Pending | Panel::Scan => vec![stage(0), applied],
        Panel::News => vec![stage(2)],
        Panel::Journal => vec![app.anatomy_at, journal_arrival(app, now)],
        Panel::Installed | Panel::Repos | Panel::Deps => vec![app.anatomy_at],
        Panel::Preflight => vec![stage(3)],
        Panel::Subsystems => vec![stage(0), stage(1), app.spotlight_since(now), applied],
        Panel::Problems | Panel::Activity => vec![stage(1)],
        Panel::Log => vec![feed(app, now).last().map(|(at, _)| *at)],
        Panel::Eye => Vec::new(),
    };
    times.into_iter().flatten().filter(|at| *at <= now).max()
}

/// Where the latest signal on a wire is: inside the picture, out on the
/// cells, or arrived and flashing at the port.
struct Signal {
    image: Option<f32>,
    head: Option<f64>,
    flash: bool,
}

fn signal(app: &App, wire: &Wire, now: Instant) -> Signal {
    let none = Signal { image: None, head: None, flash: false };
    if app.skipped {
        return none;
    }
    let Some(at) = last_event(app, wire.panel, now) else { return none };
    let elapsed = ms(now, at);
    if elapsed < IMAGE_MS {
        return Signal { image: Some((elapsed / IMAGE_MS) as f32), head: None, flash: false };
    }
    let head = (elapsed - IMAGE_MS) * CELLS_PER_MS;
    let len = wire.runs.iter().map(|run| run.cells.len()).sum::<usize>() as f64;
    let arrived = IMAGE_MS + len / CELLS_PER_MS;
    Signal { image: None, head: (head < len + 2.0).then_some(head), flash: elapsed >= arrived && elapsed < arrived + FLASH_MS }
}

fn focused_panel(app: &App, layout: &Layout) -> Option<(Panel, Rect)> {
    if app.detail.is_some() {
        return None;
    }
    layout.panels.get(app.focus).copied()
}

/// The wire that leads to the selected tile.
fn focused_target(app: &App, layout: &Layout) -> Option<Panel> {
    focused_panel(app, layout).map(|(panel, _)| panel).filter(|panel| *panel != Panel::Eye)
}

fn draw_wires(buf: &mut Buffer, layout: &Layout, dirs: &HashMap<(u16, u16), u8>, app: &App, pal: &Palette) {
    let focus = focused_target(app, layout);
    let lit: HashSet<(u16, u16)> = layout.wires.iter().filter(|wire| Some(wire.panel) == focus).flat_map(|wire| wire.cells()).collect();
    let area = buf.area;
    for (cell, bits) in dirs {
        if cell.0 >= area.right() || cell.1 >= area.bottom() {
            continue;
        }
        let color = if lit.contains(cell) { pal.teal } else { pal.grid };
        buf[*cell].set_symbol(glyph(*bits, false)).set_fg(color);
    }
}

fn on_border(area: Rect, cell: (u16, u16)) -> bool {
    cell.0 >= area.x && cell.0 < area.right() && (cell.1 == area.y || cell.1 + 1 == area.bottom())
}

fn draw_ports(buf: &mut Buffer, layout: &Layout, app: &App, pal: &Palette, now: Instant) {
    let focus = focused_target(app, layout);
    let heavy_area = focused_panel(app, layout).filter(|(panel, _)| *panel != Panel::Eye).map(|(_, area)| area);
    // Per border cell: joined directions, the strongest state, heavy or not
    let mut ports: HashMap<(u16, u16), (u8, u8, bool)> = HashMap::new();
    for wire in &layout.wires {
        let on_path = Some(wire.panel) == focus;
        let flash = signal(app, wire, now).flash;
        let last = wire.ports.len().saturating_sub(1);
        for (index, (cell, dirs)) in wire.ports.iter().enumerate() {
            let border = heavy_area.is_some_and(|area| on_border(area, *cell));
            let arrived = flash && index == last;
            let rank = if on_path || arrived {
                3
            } else if border {
                2
            } else {
                1
            };
            let entry = ports.entry(*cell).or_insert((0, 0, false));
            entry.0 |= dirs;
            entry.1 = entry.1.max(rank);
            entry.2 |= border || arrived;
        }
    }
    let area = buf.area;
    for (cell, (dirs, rank, heavy)) in ports {
        if cell.0 >= area.right() || cell.1 >= area.bottom() {
            continue;
        }
        let color = match rank {
            3 => pal.teal,
            2 => pal.ink,
            _ => pal.rule,
        };
        buf[cell].set_symbol(glyph(dirs, heavy)).set_fg(color);
    }
}

fn draw_signals(buf: &mut Buffer, layout: &Layout, dirs: &HashMap<(u16, u16), u8>, app: &App, pal: &Palette, now: Instant) {
    let area = buf.area;
    for wire in &layout.wires {
        let Some(head) = signal(app, wire, now).head else { continue };
        let cells = wire.cells();
        let head = head.floor() as i64;
        for i in (head - 2).max(0)..=head {
            let Some(&cell) = cells.get(i as usize) else { continue };
            if cell.0 >= area.right() || cell.1 >= area.bottom() {
                continue;
            }
            buf[cell].set_symbol(glyph(dirs.get(&cell).copied().unwrap_or(0), true)).set_fg(pal.teal);
        }
    }
}

// --- The eye's attention ---

/// Moments the eye glances at a tile because new data arrived there.
fn glances(app: &App, layout: &Layout, t: Instant) -> Vec<(Instant, Rect)> {
    let find = |kinds: &[Panel]| kinds.iter().find_map(|kind| layout.panels.iter().find(|(panel, _)| panel == kind).map(|(_, area)| *area));
    [
        (app.anatomy_at, find(&[Panel::Installed, Panel::Journal])),
        (app.stage_done_at[0], find(&[Panel::Pending, Panel::Scan])),
        (app.stage_done_at[1], find(&[Panel::Journal, Panel::Problems, Panel::Subsystems])),
        (app.stage_done_at[2], find(&[Panel::News])),
        (app.stage_done_at[3], find(&[Panel::Preflight])),
        (journal_arrival(app, t), find(&[Panel::Journal, Panel::Log])),
    ]
    .into_iter()
    .filter_map(|(at, area)| Some((at?, area?)))
    .collect()
}

/// What the eye looks at, at time `t`: a tile that just got data, else the
/// selected tile; `None` is straight ahead.
fn gaze_target(app: &App, layout: &Layout, t: Instant) -> Option<Rect> {
    let glance = glances(app, layout, t).into_iter().filter(|(at, _)| *at <= t && ms(t, *at) < GLANCE_MS).max_by_key(|(at, _)| *at);
    if let Some((_, area)) = glance {
        return Some(area);
    }
    if app.detail.is_some() {
        return None;
    }
    let focus = match app.focus_since {
        Some(since) if t < since => app.focus_prev,
        _ => app.focus,
    };
    layout.panels.get(focus).filter(|(panel, _)| *panel != Panel::Eye).map(|(_, area)| *area)
}

fn gaze_vector(layout: &Layout, target: Option<Rect>) -> (f32, f32) {
    let center = |r: Rect| (f32::from(r.x) + f32::from(r.width) / 2.0, f32::from(r.y) + f32::from(r.height) / 2.0);
    let Some(area) = target else { return (0.0, 0.0) };
    let (point, eye) = (center(area), center(layout.eye));
    let (dx, dy) = ((point.0 - eye.0) * CELL_ASPECT as f32, point.1 - eye.1);
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.5 { (0.0, 0.0) } else { (dx / length, dy / length) }
}

/// Where the eye looks now, easing from where it looked before.
fn gaze(app: &App, layout: &Layout, now: Instant) -> (f32, f32) {
    let current = gaze_vector(layout, gaze_target(app, layout, now));
    if app.skipped {
        return current;
    }
    let mut switches: Vec<Instant> = app.focus_since.into_iter().collect();
    for (at, _) in glances(app, layout, now) {
        switches.push(at);
        switches.push(at + Duration::from_millis(GLANCE_MS as u64));
    }
    let Some(switch) = switches.into_iter().filter(|at| *at <= now).max() else { return current };
    let t = ease_out_cubic(ms(now, switch) / GAZE_MS) as f32;
    if t >= 1.0 {
        return current;
    }
    let before = gaze_vector(layout, gaze_target(app, layout, switch - Duration::from_millis(1)));
    (before.0 + (current.0 - before.0) * t, before.1 + (current.1 - before.1) * t)
}

/// 1 open, 0 closed: the eye opens when the monitor starts and blinks now and then.
fn eye_open(app: &App, now: Instant) -> f32 {
    if app.skipped {
        return 1.0;
    }
    let since_start = ms(now, app.started);
    let wake = if since_start < 120.0 { 0.0 } else { ease_out_cubic((since_start - 120.0) / 450.0) };
    let blink = match app.finished_at {
        Some(done) if ms(now, done) > 1000.0 => {
            let phase = ms(now, done) % BLINK_EVERY_MS;
            if phase < 170.0 { (phase - 85.0).abs() / 85.0 } else { 1.0 }
        }
        _ => 1.0,
    };
    wake.min(blink) as f32
}

/// Whether anything on screen moves: signals on the wires, the eye turning,
/// blinking or working.
pub fn moving(app: &App, now: Instant) -> bool {
    if app.skipped {
        return false;
    }
    if app.working(now) > 0.0 {
        return true;
    }
    let mut events = vec![app.anatomy_at, app.start_at, app.finished_at, app.focus_since, app.spotlight_since(now), journal_arrival(app, now)];
    events.extend(app.stage_done_at);
    events.extend(app.applied.iter().map(|(_, at)| Some(*at)));
    events.push(app.transaction.and_then(|(_, end)| end));
    if events.into_iter().flatten().any(|at| at <= now && ms(now, at) < 2200.0) {
        return true;
    }
    app.finished_at.is_some_and(|done| {
        let phase = ms(now, done) % BLINK_EVERY_MS;
        ms(now, done) > 1000.0 && (phase < 250.0 || phase > BLINK_EVERY_MS - 600.0)
    })
}

/// Lines on the pupil screen: the newest at the bottom, older ones rising
/// and fading as new ones arrive.
fn screen_lines(app: &App, now: Instant) -> Vec<ScreenLine> {
    let began = app.transaction.map(|(began, _)| began);
    let shown: Vec<&LogLine> = app.log.iter().filter(|line| line.at <= now && began.is_some_and(|began| line.at >= began)).collect();
    let shown = &shown[shown.len().saturating_sub(12)..];
    let eases: Vec<f32> = shown.iter().map(|line| if app.skipped { 1.0 } else { ease_out_cubic(ms(now, line.at) / 260.0) as f32 }).collect();
    shown
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let y = eases[index + 1..].iter().sum::<f32>() - (1.0 - eases[index]);
            ScreenLine { text: line.text.clone(), y, alpha: (1.0 - y / 7.0).clamp(0.2, 1.0) }
        })
        .collect()
}

fn eye_spec(app: &App, layout: &Layout, picture: Rect, live: bool, now: Instant) -> EyeSpec {
    let group = |update: &Update| Subsystem::ALL.iter().position(|s| *s == Subsystem::of(&update.name)).unwrap_or(6);
    // Updates installed during this transaction keep their place while their
    // fibers withdraw, so the others do not jump
    let applied_at = |name: &str| app.applied.iter().find(|(update, _)| update.name == name).map(|(_, at)| *at);
    let mut updates: Vec<&Update> = app.updates.iter().chain(app.applied.iter().map(|(update, _)| update)).collect();
    updates.sort_by(|a, b| group(a).cmp(&group(b)).then(a.risk_rank().cmp(&b.risk_rank())).then(a.name.cmp(&b.name)));
    if layout.band.is_some() && !live {
        updates.clear();
    }
    let mut groups: Vec<usize> = updates.iter().map(|u| group(u)).collect();
    groups.dedup();
    let gap = if groups.len() > 1 { (TAU * 0.2 / groups.len() as f32).min(0.07) } else { 0.0 };
    let step = (TAU - gap * groups.len() as f32) / updates.len().max(1) as f32;
    let per_fiber = (1500.0 / updates.len().max(1) as f64).min(70.0);
    let problems_known = app.stage_done_at[1].is_some();
    let fixes = |name: &str| problems_known && app.problems.iter().any(|p| p.status == "possibly-fixed" && p.package == name);
    let mut fibers = Vec::new();
    let mut sectors = Vec::new();
    let mut angle = -TAU / 4.0;
    for g in &groups {
        if groups.len() > 1 {
            sectors.push(angle + gap / 2.0);
            angle += gap;
        }
        for update in updates.iter().filter(|u| group(u) == *g) {
            let delay = 150.0 + fibers.len() as f64 * per_fiber;
            fibers.push(Fiber {
                angle: angle + step / 2.0,
                risk: update.risk_rank(),
                fixes: fixes(&update.name),
                reveal: progress(app, now, app.stage_done_at[0], delay, 600.0) as f32,
                retract: applied_at(&update.name).map_or(0.0, |at| if app.skipped { 1.0 } else { ease_out_cubic(ms(now, at) / RETRACT_MS) as f32 }),
            });
            angle += step;
        }
    }

    let focus = focused_target(app, layout);
    let ports: Vec<EyePort> = if layout.band.is_some() && !live {
        let upward = |wire: &&Wire| wire.runs.first().and_then(|run| run.cells.first()).is_some_and(|cell| cell.1 < picture.y);
        let mut ports = vec![EyePort { edge: Edge::Left, cell: picture.height / 2, focused: false, heavy: true, pulse: None }];
        for (edge, up) in [(Edge::Top, true), (Edge::Bottom, false)] {
            let mine: Vec<&Wire> = layout.wires.iter().filter(|wire| upward(wire) == up).collect();
            ports.push(EyePort {
                edge,
                cell: picture.width / 2,
                focused: mine.iter().any(|wire| Some(wire.panel) == focus),
                heavy: false,
                pulse: mine.iter().filter_map(|wire| signal(app, wire, now).image).reduce(f32::max),
            });
        }
        ports
    } else {
        vec![EyePort {
            edge: Edge::Top,
            cell: picture.width / 2,
            focused: focus.is_some(),
            heavy: false,
            pulse: layout.wires.iter().filter_map(|wire| signal(app, wire, now).image).reduce(f32::max),
        }]
    };
    let angle = (ms(now, app.started) / 1000.0 * std::f64::consts::TAU / 3.4) as f32;
    let sweep = match app.finished_at {
        _ if app.skipped => None,
        None => Some((angle, 1.0)),
        Some(done) => {
            let strength = 1.0 - ms(now, done) / 900.0;
            (strength > 0.0).then_some((angle, strength as f32))
        }
    };
    let work = app.working(now) as f32;
    let (gx, gy) = gaze(app, layout, now);
    EyeSpec {
        cols: picture.width,
        rows: picture.height,
        fibers,
        sectors,
        gaze: (gx * (1.0 - work), gy * (1.0 - work)),
        open: if work > 0.0 { 1.0 } else { eye_open(app, now) },
        ring: progress(app, now, Some(app.started), 0.0, 1400.0) as f32,
        sweep,
        ports,
        work,
        screen: if work > 0.0 { screen_lines(app, now) } else { Vec::new() },
        cursor: ((ms(now, app.started) / 400.0) as u64).is_multiple_of(2),
    }
}

/// Everything that happened, in order: scan steps, package manager lines and
/// journal errors as they came in.
fn feed(app: &App, now: Instant) -> Vec<(Instant, String)> {
    let mut lines: Vec<(Instant, String)> = app.steps.iter().filter(|step| step.started <= now).map(|step| (step.started, step.label.clone())).collect();
    lines.extend(app.feed.iter().filter(|(at, _)| *at <= now).cloned());
    lines.extend(app.journal_feed.iter().filter(|(at, _)| *at <= now).cloned());
    if let (true, Some(at), Some(anatomy), Some(latest)) = (app.demo, app.journal_at, &app.anatomy, journal_arrival(app, now)) {
        let total = anatomy.journal.len();
        let base = total.saturating_sub(3);
        let arrived = (ms(latest, at) / 2500.0).round() as usize;
        for k in 1..=arrived {
            if let Some(line) = anatomy.journal.get(base + k - 1) {
                lines.push((at + Duration::from_millis(k as u64 * 2500), format!("{}: {}", line.ident, line.message)));
            }
        }
    }
    lines.sort_by_key(|(at, _)| *at);
    lines
}

/// Wall clock time of a moment, counted from the scan's own timestamp.
fn clock(app: &App, at: Instant) -> String {
    let base = app.scan_time.as_deref().and_then(|iso| {
        let part = |range: std::ops::Range<usize>| iso.get(range).and_then(|s| s.parse::<u64>().ok());
        Some(part(11..13)? * 3600 + part(14..16)? * 60 + part(17..19)?)
    });
    match (base, app.start_at) {
        (Some(base), Some(start)) => {
            let t = base + at.saturating_duration_since(start).as_secs();
            format!("{:02}:{:02}:{:02}", t / 3600 % 24, t / 60 % 60, t % 60)
        }
        _ => format!("+{:.1}s", ms(at, app.started) / 1000.0),
    }
}

// --- Drawing ---

pub fn draw(frame: &mut Frame, app: &App, model: &Model, pal: &Palette, now: Instant) -> Scene {
    let area = frame.area();
    let buf = frame.buffer_mut();
    fill(buf, area, pal.page);
    let mut scene = Scene::default();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(buf, area, pal);
        return scene;
    }
    let layout = layout(app, area);
    if let Some(index) = app.detail {
        let panel = layout.panels.get(index).map_or(Panel::Pending, |(panel, _)| *panel);
        if panel == Panel::Pending {
            draw_pending_detail(buf, area, app, pal, now, &mut scene);
        } else {
            let inner = Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), area.height.saturating_sub(3));
            draw_panel(buf, panel, inner, false, &layout, app, model, pal, now, &mut scene);
            draw_footer(buf, layout.footer, app, pal, layout.band.is_some(), now);
        }
        return scene;
    }
    let dirs = wire_dirs(&layout.wires);
    draw_title(buf, &layout, app, pal, now, &mut scene);
    if let (Some(band), Some(plan)) = (layout.band, layout.plan.as_ref()) {
        draw_band(buf, &layout, plan, band, app, pal, now, &mut scene);
    }
    draw_wires(buf, &layout, &dirs, app, pal);
    for (index, (panel, rect)) in layout.panels.iter().enumerate() {
        draw_panel(buf, *panel, *rect, app.focus == index, &layout, app, model, pal, now, &mut scene);
    }
    draw_ports(buf, &layout, app, pal, now);
    draw_freshness(buf, &layout, app, pal, now);
    draw_signals(buf, &layout, &dirs, app, pal, now);
    draw_footer(buf, layout.footer, app, pal, layout.band.is_some(), now);
    scene
}

fn draw_too_small(buf: &mut Buffer, area: Rect, pal: &Palette) {
    let ink = Style::new().fg(pal.ink);
    let size = |value: u16, min: u16| Span::styled(value.to_string(), if value < min { ink.fg(pal.alert) } else { ink });
    let lines = [
        vec![Span::styled("Terminal size too small:", ink)],
        vec![Span::styled("Width = ", ink), size(area.width, MIN_WIDTH), Span::styled(" Height = ", ink), size(area.height, MIN_HEIGHT)],
        vec![],
        vec![Span::styled("Needed:", ink)],
        vec![Span::styled(format!("Width = {MIN_WIDTH} Height = {MIN_HEIGHT}"), ink)],
    ];
    let top = area.y + (area.height / 2).saturating_sub(2);
    for (y, spans) in (top..area.bottom()).zip(lines) {
        let x = area.x + area.width.saturating_sub(width(&spans)) / 2;
        put(buf, x, y, spans, area.right() - x);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_panel(buf: &mut Buffer, panel: Panel, area: Rect, focused: bool, layout: &Layout, app: &App, model: &Model, pal: &Palette, now: Instant, scene: &mut Scene) {
    match panel {
        Panel::Pending => draw_pending(buf, area, focused, app, pal, now),
        Panel::Journal => draw_journal(buf, area, focused, app, pal, now),
        Panel::Installed => draw_installed(buf, area, focused, !layout.panels.iter().any(|(other, _)| *other == Panel::Repos), app, pal, now, scene),
        Panel::News => draw_news(buf, area, focused, layout.band.is_none(), app, pal, now),
        Panel::Preflight => draw_preflight(buf, area, focused, app, pal, now),
        Panel::Subsystems => draw_subsystems(buf, area, focused, app, pal, now, scene),
        Panel::Scan => draw_scan(buf, area, focused, app, model, pal, now, scene),
        Panel::Problems => draw_problems(buf, area, focused, app, pal, now),
        Panel::Activity => draw_activity(buf, area, focused, app, pal, now),
        Panel::Repos => draw_repos(buf, area, focused, app, pal, now),
        Panel::Deps => draw_deps(buf, area, focused, app, pal, now),
        Panel::Eye => draw_big_eye(buf, area, focused, layout, app, pal, now, scene),
        Panel::Log => draw_log(buf, area, focused, app, pal, now),
    }
}

/// The live view: the eye large, its pupil a screen while the system changes.
#[allow(clippy::too_many_arguments)]
fn draw_big_eye(buf: &mut Buffer, area: Rect, focused: bool, layout: &Layout, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    if area.width < 8 || area.height < 8 {
        return;
    }
    let (color, h) = if focused { (pal.ink, "━") } else { (pal.rule, "─") };
    let corners = if focused { ["┏", "┓", "┗", "┛"] } else { ["╭", "╮", "╰", "╯"] };
    let (left, right, top, bottom) = (area.x, area.right() - 1, area.y, area.bottom() - 1);
    for (cell, symbol) in [((left, top), corners[0]), ((right, top), corners[1]), ((left, bottom), corners[2]), ((right, bottom), corners[3])] {
        buf[cell].set_symbol(symbol).set_fg(color);
    }
    for cell in [(left + 1, top), (right - 1, top), (left + 1, bottom), (right - 1, bottom)] {
        buf[cell].set_symbol(h).set_fg(color);
    }
    let picture = big_eye(area);
    scene.placements.push(Placement { rect: picture, picture: Picture::Eye(eye_spec(app, layout, picture, true, now)) });

    let label = Style::new().fg(pal.label);
    let legend = vec![
        Span::styled("━ ", Style::new().fg(pal.ink2)),
        Span::styled("pending update   ", label),
        Span::styled("reaching deeper: riskier   ", label),
        Span::styled("━ ", Style::new().fg(pal.teal)),
        Span::styled("may fix a problem", label),
    ];
    let legend_y = picture.bottom() + 1;
    put(buf, area.x + area.width.saturating_sub(width(&legend)) / 2, legend_y, legend, area.width);
    if let Some((_, ended)) = app.transaction
        && ended.is_none_or(|end| ms(now, end) < 4000.0)
    {
        let headline = if ended.is_none() { "applying updates" } else { "updates applied" };
        let line = vec![
            Span::styled(headline, Style::new().fg(pal.ink).add_modifier(Modifier::BOLD)),
            Span::styled(format!("   {} of {}", app.applied.len(), app.applied_total), label),
        ];
        put(buf, area.x + area.width.saturating_sub(width(&line)) / 2, legend_y + 2, line, area.width);
    }
}

/// Everything that happened, newest at the bottom.
fn draw_log(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let mut lines = feed(app, now);
    if app.working(now) > 0.0
        && let Some((began, _)) = app.transaction
    {
        let in_eye: Vec<Instant> = app.feed.iter().filter(|(at, _)| *at <= now && *at >= began).rev().take(3).map(|(at, _)| *at).collect();
        lines.retain(|(at, _)| !in_eye.contains(at));
    }
    let right = vec![Span::styled(format!("{} {}", lines.len(), plural(lines.len(), "event", "events")), Style::new().fg(pal.bar_text))];
    let content = bar_tile(buf, area, focused, "ACTIVITY", right, app, pal, now);
    let rows = content.height as usize;
    let start = lines.len().saturating_sub(rows);
    for (offset, (at, text)) in lines[start..].iter().enumerate() {
        let y = content.y + offset as u16;
        if start + offset + 1 == lines.len() {
            fill(buf, Rect::new(content.x.saturating_sub(1), y, content.width + 2, 1), pal.teal_light);
        }
        put(
            buf,
            content.x,
            y,
            vec![Span::styled(format!("{}  ", clock(app, *at)), Style::new().fg(pal.label)), Span::styled(fit(text, content.width.saturating_sub(10) as usize), Style::new().fg(pal.ink2))],
            content.width,
        );
    }
}

/// The problems found since the last upgrade, and whether a pending update
/// may fix them.
fn draw_problems(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let known = app.stage_done_at[1];
    let right = if known.is_some() {
        vec![Span::styled(format!("{} · {} may be fixed", app.problems.len(), app.fixes()), Style::new().fg(pal.bar_text))]
    } else {
        Vec::new()
    };
    let content = bar_tile(buf, area, focused, "CURRENT PROBLEMS", right, app, pal, now);
    let Some(done) = known else { return };
    let problems = app.sorted_problems();
    let rows = content.height as usize;
    let shown = if problems.len() > rows { rows.saturating_sub(1) } else { problems.len() };
    for (index, problem) in problems.iter().take(shown).enumerate() {
        if progress(app, now, after(Some(done), 200 + index as u64 * 70), 0.0, 300.0) <= 0.0 {
            continue;
        }
        let y = content.y + index as u16;
        let kind = match problem.kind.as_str() {
            "crash" => "crash",
            "failed-unit" => "unit",
            "config" => "config",
            _ => "error",
        };
        let installed = app.applied.iter().any(|(update, _)| update.name == problem.package);
        let (status, color) = match problem.status.as_str() {
            "possibly-fixed" if installed => (format!("✓ {} {}", problem.package, short_version(problem.fix_version())), pal.ink2),
            "possibly-fixed" => (format!("↗ {} {}", problem.package, short_version(problem.fix_version())), pal.teal),
            "config" => ("config error".to_string(), pal.alert),
            "not-packaged" => ("not from a package".to_string(), pal.faint),
            _ => ("no update pending".to_string(), pal.faint),
        };
        put(
            buf,
            content.x,
            y,
            vec![
                Span::styled(format!("{kind:<7}"), Style::new().fg(pal.label)),
                Span::styled(format!("{:<18}", fit(problem.name(), 17)), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:>6}×  ", thousands(problem.count)), Style::new().fg(pal.ink2)),
                Span::styled(fit(&status, content.width.saturating_sub(34) as usize), Style::new().fg(color)),
            ],
            content.width,
        );
    }
    if problems.len() > shown {
        put(buf, content.x, content.y + shown as u16, vec![Span::styled(format!("+{} more · ⏎ details", problems.len() - shown), Style::new().fg(pal.label))], content.width);
    }
}

/// Crashes and errors per day as stacked bars.
fn draw_activity(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let known = app.stage_done_at[1];
    let per_day = problems_per_day(app, now);
    let days = per_day.keys().max().map_or(0, |oldest| oldest + 1);
    let right = known.filter(|_| days > 0).map(|_| {
        let total: u64 = per_day.values().map(|(crashes, errors)| crashes + errors).sum();
        vec![Span::styled(format!("{} in {days} days", thousands(total)), Style::new().fg(pal.bar_text))]
    });
    let content = bar_tile(buf, area, focused, "PROBLEMS PER DAY", right.unwrap_or_default(), app, pal, now);
    let Some(done) = known else { return };
    if content.height < 5 || days == 0 {
        return;
    }
    let chart_h = content.height - 3;
    let shown = days.min(i64::from(content.width / 3)).max(1);
    // Each day gets an equal slot across the tile; bars leave a gap
    let slot = (content.width / shown as u16).clamp(3, 6);
    let bar = slot - 1;
    let max = per_day.iter().filter(|(ago, _)| **ago < shown).map(|(_, (c, e))| c + e).max().unwrap_or(1).max(1) as f64;
    let grow = progress(app, now, Some(done), 0.0, 900.0);
    let eighths = |value: u64| ((value as f64 / max) * f64::from(chart_h) * 8.0 * grow).round() as u16;
    let bottom = content.y + chart_h;
    let mut free = content.x;
    for index in 0..shown {
        let ago = shown - 1 - index;
        let (crashes, errors) = per_day.get(&ago).copied().unwrap_or((0, 0));
        let x = content.x + index as u16 * slot;
        let (crash, total) = (eighths(crashes), eighths(crashes + errors));
        for row in 0..chart_h {
            let floor = row * 8;
            if total <= floor {
                break;
            }
            let fill = (total - floor).min(8);
            let color = if crash >= floor + 8 { pal.ink } else { pal.teal_mid };
            for dx in 0..bar {
                buf[(x + dx, bottom - row)].set_symbol(BARS[fill as usize - 1]).set_fg(color);
            }
        }
        let value = ((crashes + errors) as f64 * grow).round() as u64;
        if value == 0 {
            continue;
        }
        let text = compact(value);
        let w = text.chars().count() as u16;
        let tx = (x + bar / 2).saturating_sub(w / 2).max(content.x);
        if tx >= free && tx + w <= content.right() {
            put(buf, tx, bottom - total.div_ceil(8).min(chart_h), vec![Span::styled(text, Style::new().fg(pal.ink))], w);
            free = tx + w + 1;
        }
    }
    let label = Style::new().fg(pal.label);
    let y = bottom + 2;
    if let (Some(first), Some(last)) = (date_ago(app, shown - 1), date_ago(app, 0)) {
        put(buf, content.x, y, vec![Span::styled(first, label)], content.width);
        put_right(buf, content.x + (shown as u16 - 1) * slot + bar, y, vec![Span::styled(last, label)]);
        let legend = vec![
            Span::styled("█ ", Style::new().fg(pal.ink)),
            Span::styled("crashes   ", label),
            Span::styled("█ ", Style::new().fg(pal.teal_mid)),
            Span::styled("errors", label),
        ];
        let legend_x = content.x + content.width.saturating_sub(width(&legend)) / 2;
        put(buf, legend_x, y, legend, content.width);
    }
}

fn draw_repos(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let repos: Vec<(&String, &u64)> = app.anatomy.as_ref().map(|a| a.repos.iter().collect()).unwrap_or_default();
    let right = if repos.is_empty() { Vec::new() } else { vec![Span::styled(format!("{} repositories", repos.len()), Style::new().fg(pal.bar_text))] };
    let content = bar_tile(buf, area, focused, "REPOSITORIES", right, app, pal, now);
    let mut repos = repos;
    repos.sort_by(|a, b| b.1.cmp(a.1));
    let max = repos.first().map_or(1, |(_, count)| **count).max(1);
    let reveal = progress(app, now, app.anatomy_at, 200.0, 900.0);
    for (index, (name, count)) in repos.iter().take(content.height as usize).enumerate() {
        let y = content.y + index as u16;
        let bar_w = content.width.saturating_sub(20);
        let filled = ((f64::from(bar_w) * **count as f64 / max as f64) * reveal).round() as u16;
        put(buf, content.x, y, vec![Span::styled(fit(name, 10), Style::new().fg(pal.ink2))], 11);
        for i in 0..bar_w {
            buf[(content.x + 11 + i, y)].set_symbol("━").set_fg(if i < filled { pal.teal_mid } else { pal.grid });
        }
        put_right(buf, content.right(), y, vec![Span::styled(thousands(**count), Style::new().fg(pal.ink))]);
    }
}

/// Pending updates by how many installed packages rely on them.
fn draw_deps(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let (title, hint) = ("DEPENDENCIES OF PENDING UPDATES", "longer bar: more rely on it");
    let fits = (title.len() + hint.len() + 6) as u16 <= area.width;
    let right = vec![Span::styled(hint, Style::new().fg(pal.bar_text))];
    let content = bar_tile(buf, area, focused, title, if app.stage_done_at[0].is_some() && fits { right } else { Vec::new() }, app, pal, now);
    let (Some(anatomy), Some(done)) = (&app.anatomy, app.stage_done_at[0]) else { return };
    let mut rows: Vec<(&Update, u64, u64)> = app
        .updates
        .iter()
        .map(|u| {
            let deps = anatomy.deps.get(&u.name).cloned().unwrap_or_default();
            (u, deps.depends, deps.required_by)
        })
        .collect();
    rows.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.name.cmp(&b.0.name)));
    let max = rows.first().map_or(1, |r| r.2).max(1);
    let reveal = progress(app, now, Some(done), 300.0, 900.0);
    let height = (content.height as usize).max(1);
    let two = rows.len() > height && content.width >= 2 * (24 + 16 + 8) + 3;
    let per_column = if two { rows.len().div_ceil(2).min(height) } else { height };
    let column_w = if two { content.width.saturating_sub(3) / 2 } else { content.width };
    let capacity = if two { per_column * 2 } else { height };
    for (index, (update, depends, required_by)) in rows.iter().enumerate() {
        let (column, row) = (index / per_column, index % per_column);
        if column > usize::from(two) {
            break;
        }
        let x = content.x + column as u16 * (column_w + 3);
        let y = content.y + row as u16;
        if rows.len() > capacity && index + 1 == capacity {
            put(buf, x, y, vec![Span::styled(format!("+{} more", rows.len() - index), Style::new().fg(pal.label))], column_w);
            break;
        }
        let bar_w = column_w.saturating_sub(24 + 16);
        let filled = ((f64::from(bar_w) * *required_by as f64 / max as f64) * reveal).round() as u16;
        put(buf, x, y, vec![Span::styled(format!("{:<14}", fit(&update.name, 13)), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))], 14);
        put(buf, x + 14, y, vec![Span::styled(format!("{depends:>3} deps"), Style::new().fg(pal.label))], 9);
        for i in 0..bar_w {
            buf[(x + 24 + i, y)].set_symbol("━").set_fg(if i < filled { pal.teal } else { pal.grid });
        }
        put_right(buf, x + column_w, y, vec![Span::styled(format!("{required_by} require it"), Style::new().fg(pal.teal))]);
    }
}

const LETTERS: [(char, [&str; 3]); 9] = [
    ('O', ["┏━┓", "┃ ┃", "┗━┛"]),
    ('M', ["┏┳┓", "┃┃┃", "╹╹╹"]),
    ('A', ["┏━┓", "┣━┫", "╹ ╹"]),
    ('R', ["┏━┓", "┣┳┛", "╹┗╸"]),
    ('C', ["┏━╸", "┃  ", "┗━╸"]),
    ('H', ["╻ ╻", "┣━┫", "╹ ╹"]),
    ('Y', ["╻ ╻", "┗┳┛", " ╹ "]),
    ('U', ["╻ ╻", "┃ ┃", "┗━┛"]),
    ('B', ["┏━┓", "┣━┫", "┗━┛"]),
];

const DIGITS: [[&str; 3]; 10] = [
    ["┏━┓", "┃ ┃", "┗━┛"],
    ["╺┓ ", " ┃ ", "╺┻╸"],
    ["╺━┓", "┏━┛", "┗━╸"],
    ["╺━┓", "╺━┫", "╺━┛"],
    ["╻ ╻", "┗━┫", "  ╹"],
    ["┏━╸", "┗━┓", "╺━┛"],
    ["┏━╸", "┣━┓", "┗━┛"],
    ["╺━┓", "  ┃", "  ╹"],
    ["┏━┓", "┣━┫", "┗━┛"],
    ["┏━┓", "┗━┫", "╺━┛"],
];

fn big_text(text: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (index, ch) in text.chars().enumerate() {
        let glyph = if let Some(digit) = ch.to_digit(10) {
            DIGITS[digit as usize]
        } else {
            LETTERS.iter().find(|(c, _)| *c == ch).map(|(_, g)| *g).unwrap_or(["   ", "   ", "   "])
        };
        for (row, out) in rows.iter_mut().enumerate() {
            if index > 0 {
                out.push(' ');
            }
            out.push_str(glyph[row]);
        }
    }
    rows
}

/// The header: the name, the scan state and the machine.
fn draw_title(buf: &mut Buffer, layout: &Layout, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    let area = layout.title;
    let reveal = progress(app, now, Some(app.started), 0.0, 700.0);
    let ink = mix(pal.page, pal.ink, reveal);
    let rows = big_text("RUMB");
    for (row, text) in rows.iter().enumerate() {
        put(buf, area.x, area.y + row as u16, vec![Span::styled(text.clone(), Style::new().fg(ink))], area.width);
    }
    let x = area.x + rows[0].chars().count() as u16 + 3;
    let mut first = decode("SOFTWARE MONITOR", Style::new().fg(pal.ink).add_modifier(Modifier::BOLD), app, now, Some(app.started), 200.0, pal);
    if app.demo {
        first.push(Span::styled("   demo data", Style::new().fg(pal.alert)));
    }
    put(buf, x, area.y, first, area.width);
    let status = match (&app.scan_time, app.finished_at) {
        (Some(time), Some(_)) => format!("scanned {}", short_time(time)),
        (_, None) => "scanning…".to_string(),
        (None, Some(_)) => "scanned".to_string(),
    };
    // Scan time and length, then the verdict; everything else lives in the tiles
    let seconds = app.steps.first().map(|first| app.finished_at.unwrap_or(now).saturating_duration_since(first.started).as_secs_f64());
    let scan = match seconds {
        Some(s) => format!("{status} · {s:.1} s"),
        None => status,
    };
    let mut line = pill(&scan, pal.teal, pal.teal_light, true);
    if app.finished_at.is_some() {
        let (verdict, severity) = app.verdict();
        let (fg, bg) = match severity {
            Severity::Bad => (pal.panel, pal.alert),
            Severity::Ok => (pal.panel, pal.teal),
            _ => (pal.panel, pal.ink2),
        };
        line.push(Span::raw("  "));
        line.extend(pill(&verdict, fg, bg, true));
    }
    put(buf, x, area.y + 1, line, area.width);

    if let Some(anatomy) = &app.anatomy {
        // On Omarchy its own mark stands at the right, with the version beside it
        let mut right = area.right();
        let mut version = Vec::new();
        if !anatomy.omarchy.is_empty() {
            let strong = Style::new().fg(pal.ink).add_modifier(Modifier::BOLD);
            match app.omarchy_mark {
                OmarchyMark::Logo => {
                    scene.placements.push(Placement { rect: Rect::new(area.right().saturating_sub(6), area.y, 6, 3), picture: Picture::Mark(OmarchyMark::Logo) });
                    right = area.right().saturating_sub(8);
                    version = vec![Span::styled(format!("   {}", anatomy.omarchy), strong)];
                }
                OmarchyMark::Wordmark => {
                    scene.placements.push(Placement { rect: Rect::new(area.right().saturating_sub(18), area.y, 18, 2), picture: Picture::Mark(OmarchyMark::Wordmark) });
                    put_right(buf, area.right(), area.y + 2, vec![Span::styled(anatomy.omarchy.clone(), strong)]);
                    right = area.right().saturating_sub(20);
                }
            }
        }
        let mut first = vec![Span::styled(anatomy.hostname.to_uppercase(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))];
        let system: Vec<String> = [anatomy.os.clone(), anatomy.desktop.clone()].into_iter().filter(|part| !part.is_empty()).collect();
        if !system.is_empty() {
            first.push(Span::styled(format!("   {}", system.join(" · ")), Style::new().fg(pal.label)));
        }
        first.extend(version);
        put_right(buf, right, area.y, first);
        let mut second = vec![Span::styled(format!("kernel {}", anatomy.kernel_running), Style::new().fg(pal.label))];
        if anatomy.reboot_pending {
            second.push(Span::styled("  reboot pending", Style::new().fg(pal.alert).add_modifier(Modifier::BOLD)));
        }
        put_right(buf, right, area.y + 1, second);
    }
}

fn draw_footer(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, life: bool, now: Instant) {
    let key = |k: &str| Span::styled(k.to_string(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD));
    let text = |t: &str| Span::styled(t.to_string(), Style::new().fg(pal.label));
    let mut hints = Vec::new();
    if app.detail.is_some() {
        hints.extend([key("esc"), text(" back   "), key("↑↓"), text(" select   "), key("q"), text(" quit")]);
    } else {
        // The views, like the presets in btop; the current one stands out
        for (index, name) in VIEWS.iter().enumerate() {
            if index == app.view {
                hints.push(Span::styled(format!(" {} {name} ", index + 1), Style::new().fg(pal.panel).bg(pal.ink).add_modifier(Modifier::BOLD)));
            } else {
                hints.extend([key(&format!(" {}", index + 1)), text(&format!(" {name} "))]);
            }
        }
        hints.extend([text("   "), key("tab"), text(" next   "), key("⏎"), text(" details   "), key("r"), text(" rescan   "), key("q"), text(" quit")]);
    }
    let used = width(&hints);
    put(buf, area.x, area.y, hints, area.width);

    let mark = |glyph: &str, color: Color, label: &str| vec![Span::styled(format!("{glyph} "), Style::new().fg(color)), text(&format!("{label}  "))];
    let mut status: Vec<Vec<Span<'static>>> = Vec::new();
    if app.detail.is_none() {
        if life {
            status = vec![mark("▆", pal.teal_mid, "errors"), mark("▆", pal.ink, "crashes")];
        }
        // The freshness beads that are on screen right now
        let states: Vec<Freshness> = panels_of(app.view).iter().map(|panel| freshness(app, *panel, now)).collect();
        for (state, label) in [(Freshness::Collecting, "collecting"), (Freshness::Stale, "stale"), (Freshness::Failed, "failed")] {
            if states.contains(&state) {
                let (glyph, color) = freshness_glyph(state, app, now, pal);
                status.push(mark(glyph, color, label));
            }
        }
    }
    // Drop the leftmost items when they would run into the key hints
    while !status.is_empty() && used + 2 + status.iter().map(|item| width(item)).sum::<u16>() > area.width {
        status.remove(0);
    }
    put_right(buf, area.right(), area.y, status.into_iter().flatten().collect());
}

// --- Tiles ---

#[allow(clippy::too_many_arguments)]
fn draw_scan(buf: &mut Buffer, area: Rect, focused: bool, app: &App, model: &Model, pal: &Palette, now: Instant, scene: &mut Scene) {
    let right = if app.stage_done_at[0].is_some() { vec![Span::styled("riskier changes orbit closer to the core", Style::new().fg(pal.label))] } else { Vec::new() };
    let content = tile(buf, area, focused, "UPDATE SCAN", right, app, pal, now);
    if content.height < 8 {
        return;
    }
    let spotlight = app.spotlight(now);

    // Left: subsystems and problem activity
    let left = Rect::new(content.x, content.y + 1, 22.min(content.width / 4), content.height.saturating_sub(1));
    put(buf, left.x, left.y, vec![Span::styled("SUBSYSTEM", Style::new().fg(pal.label))], left.width);
    put_right(buf, left.right(), left.y, vec![Span::styled("upd · prob", Style::new().fg(pal.faint))]);
    for (y, subsystem) in (left.y + 1..left.y + 8).zip(Subsystem::ALL) {
        let updates = app.subsystem_updates(subsystem).len();
        let problems = app.subsystem_problems(subsystem).len();
        let active = app.finished_at.is_some() && subsystem == spotlight;
        let (marker, style) = if active {
            ("◉ ", Style::new().fg(pal.teal).add_modifier(Modifier::BOLD))
        } else if updates + problems > 0 {
            ("○ ", Style::new().fg(pal.ink))
        } else {
            ("· ", Style::new().fg(pal.faint))
        };
        put(buf, left.x, y, vec![Span::styled(marker, style), Span::styled(subsystem.label().to_string(), style)], left.width);
        if app.stage_done_at[1].is_some() {
            put_right(buf, left.right(), y, vec![Span::styled(format!("{updates} · {problems}"), style)]);
        }
    }
    if let Some(done) = app.stage_done_at[1]
        && left.height >= 14
    {
        let y = left.bottom().saturating_sub(6);
        put(buf, left.x, y, vec![Span::styled("PROBLEMS PER DAY", Style::new().fg(pal.label))], left.width);
        let days: Vec<u64> = app.activity.iter().rev().take(left.width as usize).rev().map(|d| d.crashes + d.errors).collect();
        put(buf, left.x, y + 1, sparkline(&days, pal.ink2, progress(app, now, Some(done), 0.0, 800.0), pal), left.width);
        let count = |kind: &str| app.problems.iter().filter(|p| p.kind == kind).map(|p| p.count).sum::<u64>();
        for (offset, (label, kind)) in [("crashes", "crash"), ("errors", "journal"), ("failed units", "failed-unit")].iter().enumerate() {
            put(buf, left.x, y + 2 + offset as u16, vec![Span::styled(format!("• {label}"), Style::new().fg(pal.ink2))], left.width);
            put_right(buf, left.right(), y + 2 + offset as u16, vec![Span::styled(count(kind).to_string(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))]);
        }
    }

    // Right: how big the version jumps are
    let right_w = 22.min(content.width / 4);
    let bumps = Rect::new(content.right().saturating_sub(right_w), content.y + 1, right_w, content.height.saturating_sub(1));
    put(buf, bumps.x, bumps.y, vec![Span::styled("CHANGES BY JUMP", Style::new().fg(pal.label))], bumps.width);
    if let Some(done) = app.stage_done_at[0] {
        let max = ["major", "minor", "patch", "rebuild"]
            .iter()
            .map(|group| app.updates.iter().filter(|u| bump_group(&u.bump) == *group).count())
            .max()
            .unwrap_or(1)
            .max(1);
        for (row, group) in ["major", "minor", "patch", "rebuild"].iter().enumerate() {
            let y = bumps.y + 1 + row as u16 * 2;
            let members: Vec<&Update> = app.updates.iter().filter(|u| bump_group(&u.bump) == *group).collect();
            let high = members.iter().filter(|u| u.risk == "high").count();
            put(buf, bumps.x, y, vec![Span::styled(group.to_string(), Style::new().fg(pal.ink2))], 8);
            put_right(buf, bumps.right(), y, vec![Span::styled(members.len().to_string(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))]);
            let bar_w = bumps.width;
            let reveal = progress(app, now, Some(done), 200.0 + row as f64 * 90.0, 700.0);
            let filled = ((f64::from(bar_w) * members.len() as f64 / max as f64) * reveal).round() as u16;
            let high_w = if members.is_empty() { 0 } else { ((f64::from(filled) * high as f64 / members.len() as f64).round()) as u16 };
            for i in 0..bar_w {
                let color = if i < high_w {
                    pal.ink
                } else if i < filled {
                    pal.teal_mid
                } else {
                    pal.grid
                };
                buf[(bumps.x + i, y + 1)].set_symbol("━").set_fg(color);
            }
        }
        let legend_y = bumps.y + 10;
        if legend_y + 1 < bumps.bottom() {
            put(
                buf,
                bumps.x,
                legend_y,
                vec![
                    Span::styled("━ ", Style::new().fg(pal.ink)),
                    Span::styled("high risk  ", Style::new().fg(pal.label)),
                    Span::styled("━ ", Style::new().fg(pal.teal_mid)),
                    Span::styled("other", Style::new().fg(pal.label)),
                ],
                bumps.width,
            );
        }
    }
    let matrix = bumps;

    // Middle: the astrolabe with labels beside it
    let mid_x = left.right() + 2;
    let mid = Rect::new(mid_x, content.y, matrix.x.saturating_sub(mid_x + 2), content.height.saturating_sub(1));
    let label_w = 13;
    let mut hero_h = mid.height;
    let mut hero_w = (f64::from(hero_h) / CELL_ASPECT).round() as u16;
    if hero_w + 2 * label_w > mid.width {
        hero_w = mid.width.saturating_sub(2 * label_w);
        hero_h = (f64::from(hero_w) * CELL_ASPECT).round() as u16;
    }
    let hero = Rect::new(mid.x + (mid.width - hero_w) / 2, mid.y + (mid.height - hero_h) / 2, hero_w, hero_h);
    scene.placements.push(Placement { rect: hero, picture: Picture::Astrolabe });
    scene.callouts = place_callouts(app, model, hero, now);
    for callout in &scene.callouts {
        draw_callout(buf, callout, model, hero, mid, pal);
    }
    let legend = vec![
        Span::styled("● ", Style::new().fg(pal.ink)),
        Span::styled("high   ", Style::new().fg(pal.label)),
        Span::styled("◉ ", Style::new().fg(pal.ink2)),
        Span::styled("medium   ", Style::new().fg(pal.label)),
        Span::styled("○ ", Style::new().fg(pal.label)),
        Span::styled("low   ", Style::new().fg(pal.label)),
        Span::styled("◌ ", Style::new().fg(pal.teal)),
        Span::styled("may fix a problem", Style::new().fg(pal.label)),
    ];
    let w = width(&legend);
    put(buf, mid.x + mid.width.saturating_sub(w) / 2, content.bottom().saturating_sub(1), legend, mid.width);
}

fn bump_group(bump: &str) -> &'static str {
    match bump {
        "major" | "epoch" => "major",
        "minor" => "minor",
        "rebuild" => "rebuild",
        _ => "patch",
    }
}

fn draw_callout(buf: &mut Buffer, callout: &Callout, model: &Model, hero: Rect, mid: Rect, pal: &Palette) {
    let Some(node) = model.nodes.get(callout.node) else { return };
    let alpha = f64::from(callout.alpha);
    if alpha <= 0.0 {
        return;
    }
    let (x, w) = match callout.side {
        Side::Left => (mid.x, hero.x.saturating_sub(mid.x + 1)),
        Side::Right => (hero.right() + 1, mid.right().saturating_sub(hero.right() + 1)),
    };
    let y = hero.y + callout.row;
    let (fg, bg) = if node.fixes > 0 {
        (pal.panel, mix(pal.panel, pal.teal, alpha))
    } else {
        (pal.panel, mix(pal.panel, pal.ink, alpha))
    };
    let title = format!(" {} ", fit(&node.name, w.saturating_sub(2) as usize));
    let detail = if node.fixes > 0 { fit(&format!("may fix {}×", node.fixes), w as usize) } else { String::new() };
    let title_w = title.chars().count() as u16;
    let detail_w = detail.chars().count() as u16;
    let (tx, dx) = if callout.side == Side::Left {
        (x + w.saturating_sub(title_w), x + w.saturating_sub(detail_w))
    } else {
        (x, x)
    };
    put(buf, tx, y, vec![Span::styled(title, Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD))], w);
    put(buf, dx, y + 1, vec![Span::styled(detail, Style::new().fg(mix(pal.panel, pal.ink2, alpha)))], w);
}

/// Picks which nodes get a label: high-risk changes and changes that may fix
/// a problem, plus the subsystem in the spotlight.
fn place_callouts(app: &App, model: &Model, hero: Rect, now: Instant) -> Vec<Callout> {
    if hero.height < 6 {
        return Vec::new();
    }
    let spotlight = app.finished_at.map(|_| app.spotlight(now));
    let spot_alpha = progress(app, now, app.spotlight_since(now), 0.0, 600.0) as f32;
    let mut wanted: Vec<(usize, f32)> = model
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.risk == 0 || node.fixes > 0 || Some(node.subsystem) == spotlight)
        .map(|(index, node)| {
            let (stage, delay) = if node.fixes > 0 { (1, 500.0) } else { (0, 700.0) };
            let mut alpha = progress(app, now, app.stage_done_at[stage], delay, 600.0) as f32;
            if node.risk != 0 && node.fixes == 0 {
                alpha *= spot_alpha;
            }
            (index, alpha)
        })
        .collect();
    wanted.truncate(7);

    let mut callouts = Vec::new();
    for side in [Side::Left, Side::Right] {
        let mut members: Vec<(usize, f32, i32)> = wanted
            .iter()
            .filter(|(index, _)| (model.nodes[*index].x < 0.0) == (side == Side::Left))
            .map(|(index, alpha)| {
                let ideal = ((0.5 + model.nodes[*index].y * DIAL / 2.0) * f32::from(hero.height)) as i32 - 1;
                (*index, *alpha, ideal)
            })
            .collect();
        members.sort_by_key(|member| member.2);
        let mut next_free = 0;
        for (index, alpha, ideal) in members {
            let row = ideal.max(next_free);
            if row + 2 > i32::from(hero.height) {
                continue;
            }
            next_free = row + 3;
            callouts.push(Callout { node: index, side, row: row as u16, alpha });
        }
    }
    callouts
}

pub fn astrolabe_anim(app: &App, model: &Model, now: Instant) -> crate::ink::Anim {
    let angle = (ms(now, app.started) / 1000.0 * std::f64::consts::TAU / 3.4) as f32;
    let sweep = match app.finished_at {
        _ if app.skipped => None,
        None => Some((angle, 1.0)),
        Some(done) => {
            let strength = 1.0 - ms(now, done) / 900.0;
            (strength > 0.0).then_some((angle, strength as f32))
        }
    };
    crate::ink::Anim {
        dial: progress(app, now, Some(app.started), 0.0, 1500.0) as f32,
        orbits: progress(app, now, Some(app.started), 250.0, 1900.0) as f32,
        nodes: (0..model.nodes.len())
            .map(|index| progress(app, now, app.stage_done_at[0], 150.0 + index as f64 * 70.0, 600.0) as f32)
            .collect(),
        links: progress(app, now, app.stage_done_at[1], 250.0, 1000.0) as f32,
        spotlight: app.finished_at.map(|_| app.spotlight(now)),
        spot_alpha: progress(app, now, app.spotlight_since(now), 0.0, 600.0) as f32,
        sweep,
    }
}

/// The seven subsystems, each with its riskiest update and its problems.
fn draw_subsystems(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    let known = app.stage_done_at[0].is_some();
    let busy = Subsystem::ALL
        .iter()
        .filter(|s| !app.subsystem_updates(**s).is_empty() || !app.subsystem_problems(**s).is_empty())
        .count();
    let right = if app.stage_done_at[1].is_some() {
        vec![Span::styled(format!("{busy} of 7 with changes or problems"), Style::new().fg(pal.bar_text))]
    } else {
        Vec::new()
    };
    let content = bar_tile(buf, area, focused, "SUBSYSTEMS", right, app, pal, now);
    let spotlight = app.spotlight(now);
    let problems_known = app.stage_done_at[1].is_some();
    let count = Subsystem::ALL.len() as u16;
    let per_row = 4u16;
    let card_w = ((content.width + 1) / per_row).saturating_sub(1);
    let card_h = ((content.height + 1) / count.div_ceil(per_row)).saturating_sub(1);
    for (index, subsystem) in Subsystem::ALL.iter().enumerate() {
        let i = index as u16;
        let (row, column) = (i / per_row, i % per_row);
        let in_row = (count - row * per_row).min(per_row);
        let offset = content.width.saturating_sub(in_row * (card_w + 1) - 1) / 2;
        let card = Rect::new(content.x + offset + column * (card_w + 1), content.y + row * (card_h + 1), card_w, card_h);
        if card.width < 8 || card.height < 4 || card.bottom() > content.bottom() {
            continue;
        }
        let alpha = if known { progress(app, now, app.stage_done_at[0], 200.0 + index as f64 * 90.0, 500.0) } else { 1.0 };
        let fade = |color: Color| mix(pal.panel, color, alpha);
        let focus = app.finished_at.is_some() && *subsystem == spotlight;
        let updates = app.subsystem_updates(*subsystem);
        let problems = app.subsystem_problems(*subsystem);
        let quiet = known && updates.is_empty() && problems.is_empty();
        outline(buf, card, ROUND, fade(if focus { pal.teal } else { pal.rule }));
        let text_y = card.bottom().saturating_sub(8).max(card.y + 1);
        let icon = Rect::new(card.x + 1, card.y + 1, card.width - 2, text_y.saturating_sub(card.y + 2));
        if icon.height >= 2 {
            scene.placements.push(Placement { rect: icon, picture: Picture::Specimen(*subsystem) });
        }
        let (text_x, text_w) = (card.x + 2, card.width.saturating_sub(3));
        let name = if focus {
            pal.teal
        } else if quiet {
            pal.faint
        } else {
            pal.ink
        };
        let mut lines = vec![(0u16, vec![Span::styled(subsystem.label().to_uppercase(), Style::new().fg(fade(name)).add_modifier(Modifier::BOLD))])];
        if known && alpha >= 0.5 {
            let label = Style::new().fg(fade(pal.label));
            let tally = match updates.len() {
                0 => "no updates".to_string(),
                1 => "1 update".to_string(),
                n => format!("{n} updates"),
            };
            lines.push((1, vec![Span::styled(tally, label)]));
            if let Some(update) = updates.first() {
                let mut spans = vec![Span::styled(format!("{} ", fit(&update.name, 11)), Style::new().fg(fade(pal.ink)).add_modifier(Modifier::BOLD))];
                let used = width(&spans) as usize;
                spans.extend(version_ruler(update, (text_w as usize).saturating_sub(used), pal));
                lines.push((2, spans));
                let risk = format!("{} · {} risk", bump_group(&update.bump), update.risk);
                let risk = if risk.chars().count() <= text_w as usize { risk } else { format!("{} · {}", bump_group(&update.bump), update.risk) };
                lines.push((3, vec![Span::styled(risk, label)]));
            }
            if problems_known {
                let total: u64 = problems.iter().map(|p| p.count).sum();
                let fixes: u64 = problems.iter().filter(|p| p.status == "possibly-fixed").map(|p| p.count).sum();
                let text = if total == 0 { "no problems".to_string() } else { format!("{total} {}", plural(total as usize, "problem", "problems")) };
                lines.push((5, vec![Span::styled(text, if total == 0 { label } else { Style::new().fg(fade(pal.ink2)) })]));
                if fixes > 0 {
                    lines.push((6, vec![Span::styled(format!("↗ {fixes} may be fixed"), Style::new().fg(fade(pal.teal)).add_modifier(Modifier::BOLD))]));
                }
            }
        }
        for (k, spans) in lines {
            if text_y + k + 1 < card.bottom() {
                put(buf, text_x, text_y + k, spans, text_w);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_installed(buf: &mut Buffer, area: Rect, focused: bool, repos: bool, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    let right = app
        .anatomy
        .as_ref()
        .map(|a| vec![Span::styled(format!("{} packages", thousands(a.packages)), Style::new().fg(pal.bar_text))])
        .unwrap_or_default();
    let content = bar_tile(buf, area, focused, "INSTALLED SOFTWARE", right, app, pal, now);
    let Some(anatomy) = &app.anatomy else { return };
    let reveal = progress(app, now, app.anatomy_at.max(app.stage_done_at[0]), 0.0, 1100.0);
    let updates_known = app.stage_done_at[0].is_some();
    let current = if anatomy.packages > 0 {
        1.0 - app.updates.len() as f32 / anatomy.packages as f32
    } else {
        1.0
    };
    let gauge = Rect::new(content.x, content.y, 12, content.height.saturating_sub(1).min(5));
    let (fraction, label) = if updates_known { (current, format!("{:.1}", current * 100.0)) } else { (0.0, String::new()) };
    scene.placements.push(Placement { rect: gauge, picture: Picture::Gauge { fraction, label, reveal: reveal as f32 } });
    put(buf, content.x, gauge.bottom(), vec![Span::styled("% up to date", Style::new().fg(pal.label))], 12);

    let x = content.x + 14;
    let column = content.width.saturating_sub(14).min(24);
    let rows = [
        ("explicit", thousands(anatomy.explicit)),
        ("as dependency", thousands(anatomy.packages.saturating_sub(anatomy.explicit))),
        ("orphans", thousands(anatomy.orphans)),
        ("cache", bytes(anatomy.cache_bytes)),
    ];
    for (index, (label, value)) in rows.iter().enumerate() {
        let y = content.y + index as u16;
        if y >= content.bottom() {
            break;
        }
        put(buf, x, y, vec![Span::styled(label.to_string(), Style::new().fg(pal.label))], column);
        put_right(buf, x + column, y, vec![Span::styled(value.clone(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))]);
    }
    if !repos {
        return;
    }

    let mut repos: Vec<(&String, &u64)> = anatomy.repos.iter().collect();
    repos.sort_by(|a, b| b.1.cmp(a.1));
    let repo_x = x + column + 3;
    let repo_w = content.right().saturating_sub(repo_x);
    if repo_w >= 14 {
        put(buf, repo_x, content.y, vec![Span::styled("REPOSITORY", Style::new().fg(pal.label))], repo_w);
        for (index, (name, count)) in repos.iter().take(content.height.saturating_sub(1) as usize).enumerate() {
            let y = content.y + 1 + index as u16;
            put(buf, repo_x, y, vec![Span::styled(fit(name, 9), Style::new().fg(pal.ink2))], repo_w);
            put_right(buf, content.right(), y, vec![Span::styled(thousands(**count), Style::new().fg(pal.ink))]);
        }
    } else if content.height > rows.len() as u16 {
        let line = repos.iter().map(|(name, count)| format!("{name} {}", thousands(**count))).collect::<Vec<_>>().join(" · ");
        put(buf, x, content.y + rows.len() as u16, vec![Span::styled(fit(&line, column as usize), Style::new().fg(pal.ink2))], column);
    }
}

fn draw_news(buf: &mut Buffer, area: Rect, focused: bool, dated: bool, app: &App, pal: &Palette, now: Instant) {
    let right = app
        .last_upgrade
        .as_deref()
        .filter(|_| dated)
        .map(|last| vec![Span::styled(format!("since {}", short_date(last)), Style::new().fg(pal.bar_text))])
        .unwrap_or_default();
    let content = bar_tile(buf, area, focused, "ARCH NEWS", right, app, pal, now);
    let Some(done) = app.stage_done_at[2] else { return };
    if app.news_failed {
        put(buf, content.x, content.y, vec![Span::styled("archlinux.org did not answer", Style::new().fg(pal.alert))], content.width);
        return;
    }
    if app.news.is_empty() {
        put(buf, content.x, content.y, vec![Span::styled("no news since the last upgrade", Style::new().fg(pal.label))], content.width);
        return;
    }
    let mut y = content.y;
    for (index, news) in app.news.iter().enumerate() {
        let start = after(Some(done), 150 + index as u64 * 120);
        let mut head = if dated { vec![Span::styled(format!("{}  ", short_date(&news.date)), Style::new().fg(pal.label))] } else { Vec::new() };
        if news.manual_intervention {
            head.extend(pill("ACTION NEEDED", pal.panel, pal.alert, true));
        }
        put(buf, content.x, y, head, content.width);
        y += 1;
        for line in wrap(&news.title, content.width as usize) {
            if y >= content.bottom() {
                return;
            }
            put(buf, content.x, y, decode(&line, Style::new().fg(pal.ink), app, now, start, 0.0, pal), content.width);
            y += 1;
        }
        y += 1;
    }
}

fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn update_row(update: &Update, w: u16, pal: &Palette) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let mut spans = risk_pill(update, pal);
    spans.push(Span::raw(" "));
    let name_w = 13;
    spans.push(Span::styled(format!("{:<name_w$}", fit(&update.name, name_w)), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD)));
    spans.extend(version_ruler(update, (w as usize).saturating_sub(7 + name_w + 10), pal));
    let size = update.download_bytes.map(bytes).unwrap_or_else(|| update.source.clone());
    (spans, vec![Span::styled(size, Style::new().fg(pal.label))])
}

fn draw_pending(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let right = if app.stage_done_at[0].is_some() {
        let sized = app.updates.iter().any(|update| update.download_bytes.is_some());
        let text = if sized { format!("{} · {} · {} on disk", app.updates.len(), bytes(app.download_bytes()), signed_bytes(app.installed_delta())) } else { app.updates.len().to_string() };
        vec![Span::styled(text, Style::new().fg(pal.bar_text))]
    } else {
        Vec::new()
    };
    let content = bar_tile(buf, area, focused, "PENDING CHANGES", right, app, pal, now);
    let Some(done) = app.stage_done_at[0] else { return };
    let updates = app.sorted_updates();
    let rows = content.height as usize;
    let shown = if updates.len() > rows { rows.saturating_sub(1) } else { updates.len() };
    for (index, update) in updates.iter().take(shown).enumerate() {
        let alpha = progress(app, now, after(Some(done), 300 + index as u64 * 80), 0.0, 400.0);
        if alpha <= 0.0 {
            continue;
        }
        let (left, right) = update_row(update, content.width, pal);
        let y = content.y + index as u16;
        put(buf, content.x, y, left, content.width);
        put_right(buf, content.right(), y, right);
    }
    if updates.len() > shown {
        put(buf, content.x, content.y + shown as u16, vec![Span::styled(format!("+{} more · ⏎ for the full list", updates.len() - shown), Style::new().fg(pal.label))], content.width);
    }
}

fn draw_journal(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let right = vec![Span::styled("● ", Style::new().fg(pal.teal_mid)), Span::styled("errors as they happen", Style::new().fg(pal.bar_text))];
    let content = bar_tile(buf, area, focused, "LIVE JOURNAL", right, app, pal, now);

    let Some(anatomy) = &app.anatomy else { return };
    let lines = &anatomy.journal;
    let visible = live_journal_count(app, now).min(lines.len());
    let rows = content.height as usize;
    let start = visible.saturating_sub(rows);
    for (offset, line) in lines[start..visible].iter().enumerate() {
        let y = content.y + offset as u16;
        let newest = start + offset + 1 == visible;
        if newest {
            fill(buf, Rect::new(content.x.saturating_sub(1), y, content.width + 2, 1), pal.teal_light);
        }
        put(
            buf,
            content.x,
            y,
            vec![
                Span::styled(format!("[{}] ", line.time.get(..5).unwrap_or(&line.time)), Style::new().fg(pal.label)),
                Span::styled(format!("{:<12}", fit(&line.ident, 11)), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD)),
                Span::styled(fit(&line.message, content.width.saturating_sub(21) as usize), Style::new().fg(pal.ink2)),
            ],
            content.width,
        );
    }
}

/// How many journal lines have "arrived": the demo feeds them in over time.
fn live_journal_count(app: &App, now: Instant) -> usize {
    let Some(at) = app.journal_at else { return 0 };
    let total = app.anatomy.as_ref().map_or(0, |a| a.journal.len());
    if app.skipped || !app.demo {
        return total;
    }
    let base = total.saturating_sub(3);
    base + (ms(now, at) / 2500.0) as usize
}

fn draw_preflight(buf: &mut Buffer, area: Rect, focused: bool, app: &App, pal: &Palette, now: Instant) {
    let ok = app.preflight.as_ref().is_some_and(|p| p.free_ok && p.snapper_configured);
    let right = match &app.preflight {
        Some(_) => vec![Span::styled(if ok { "ready" } else { "check" }, Style::new().fg(pal.bar_text).add_modifier(Modifier::BOLD))],
        None => Vec::new(),
    };
    let content = bar_tile(buf, area, focused, "BEFORE YOU UPDATE", right, app, pal, now);
    let Some(preflight) = &app.preflight else { return };
    let reveal = progress(app, now, app.stage_done_at[3], 100.0, 1000.0);
    let share = if preflight.disk_total_bytes > 0 { preflight.disk_used_bytes as f64 / preflight.disk_total_bytes as f64 } else { 0.0 };
    let bar_w = content.width.saturating_sub(10);
    let filled = (f64::from(bar_w) * share * reveal).round() as u16;
    put(buf, content.x, content.y, vec![Span::styled("disk", Style::new().fg(pal.label))], 6);
    for i in 0..bar_w {
        buf[(content.x + 5 + i, content.y)].set_symbol("━").set_fg(if i < filled { if preflight.free_ok { pal.teal_mid } else { pal.alert } } else { pal.grid });
    }
    put_right(buf, content.right(), content.y, vec![Span::styled(format!("{:.0}%", share * 100.0), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))]);
    let rows = [
        (
            "free space",
            format!("{} of {}", bytes(preflight.free_bytes), bytes(preflight.disk_total_bytes)),
            if preflight.free_ok { pal.ink } else { pal.alert },
        ),
        (
            "snapshot",
            if preflight.snapper_configured { "snapper ✓".to_string() } else { "none — no rollback".to_string() },
            if preflight.snapper_configured { pal.teal } else { pal.alert },
        ),
    ];
    for (index, (label, value, color)) in rows.iter().enumerate() {
        let y = content.y + 1 + index as u16;
        if y >= content.bottom() {
            break;
        }
        put(buf, content.x, y, vec![Span::styled(label.to_string(), Style::new().fg(pal.label))], content.width);
        put_right(buf, content.right(), y, vec![Span::styled(value.clone(), Style::new().fg(*color))]);
    }
}

// --- Detail: pending changes ---

fn draw_pending_detail(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) {
    let x = area.x + 2;
    let w = area.width.saturating_sub(4);
    let updates = app.sorted_updates();
    let rows = big_text(&updates.len().to_string());
    for (row, text) in rows.iter().enumerate() {
        put(buf, x, area.y + 1 + row as u16, vec![Span::styled(text.clone(), Style::new().fg(pal.ink))], w);
    }
    let tx = x + rows[0].chars().count() as u16 + 3;
    put(buf, tx, area.y + 1, vec![Span::styled("PENDING CHANGES", Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))], w);
    let low = updates.len() - app.high() - app.medium();
    put(
        buf,
        tx,
        area.y + 2,
        vec![
            Span::styled(format!("{} download · {} on disk", bytes(app.download_bytes()), signed_bytes(app.installed_delta())), Style::new().fg(pal.ink2)),
            Span::styled(format!("   {} high · {} medium · {low} low", app.high(), app.medium()), Style::new().fg(pal.label)),
        ],
        w,
    );

    let table_w = w * 60 / 100;
    let table = Rect::new(x, area.y + 5, table_w, area.height.saturating_sub(8));
    fill(buf, table, pal.panel);
    border(buf, table, true, pal);
    let inner = Rect::new(table.x + 2, table.y + 1, table.width.saturating_sub(4), table.height.saturating_sub(2));
    let header = Style::new().fg(pal.label);
    let columns: [(&str, u16); 6] = [("RISK", 0), ("PACKAGE", 7), ("SUBSYSTEM", 22), ("VERSION", 34), ("DOWNLOAD", 0), ("DISK", 0)];
    for (name, offset) in columns.iter().take(4) {
        put(buf, inner.x + offset, inner.y, vec![Span::styled(name.to_string(), header)], 12);
    }
    put_right(buf, inner.right().saturating_sub(10), inner.y, vec![Span::styled("DOWNLOAD", header)]);
    put_right(buf, inner.right(), inner.y, vec![Span::styled("DISK", header)]);
    for i in 0..inner.width {
        buf[(inner.x + i, inner.y + 1)].set_symbol("─").set_fg(pal.rule);
    }
    let selected = app.selected.min(updates.len().saturating_sub(1));
    for (index, update) in updates.iter().enumerate() {
        let y = inner.y + 2 + index as u16;
        if y >= inner.bottom() {
            break;
        }
        if index == selected {
            fill(buf, Rect::new(inner.x.saturating_sub(1), y, inner.width + 2, 1), pal.teal_light);
        }
        put(buf, inner.x, y, risk_pill(update, pal), 6);
        put(buf, inner.x + 7, y, vec![Span::styled(fit(&update.name, 14), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))], 14);
        put(buf, inner.x + 22, y, vec![Span::styled(Subsystem::of(&update.name).label().to_string(), Style::new().fg(pal.label))], 11);
        put(buf, inner.x + 34, y, version_ruler(update, inner.width.saturating_sub(34 + 22) as usize, pal), inner.width.saturating_sub(34 + 21));
        let download = update.download_bytes.map(bytes).unwrap_or_else(|| "aur".into());
        put_right(buf, inner.right().saturating_sub(10), y, vec![Span::styled(download, Style::new().fg(pal.ink2))]);
        let delta = update.installed_delta_bytes.map(signed_bytes).unwrap_or_default();
        put_right(buf, inner.right(), y, vec![Span::styled(delta, Style::new().fg(pal.label))]);
    }

    // Download by package, largest first
    let chart_y = (inner.y + 4 + updates.len() as u16).min(inner.bottom().saturating_sub(5));
    put(buf, inner.x, chart_y, vec![Span::styled("DOWNLOAD BY PACKAGE", header)], inner.width);
    let mut sized: Vec<&&Update> = updates.iter().filter(|u| u.download_bytes.is_some()).collect();
    sized.sort_by_key(|u| std::cmp::Reverse(u.download_bytes.unwrap_or(0)));
    let max = sized.first().and_then(|u| u.download_bytes).unwrap_or(1).max(1);
    let reveal = progress(app, now, app.stage_done_at[0], 300.0, 900.0);
    for (index, update) in sized.iter().take(inner.bottom().saturating_sub(chart_y + 1) as usize).enumerate() {
        let y = chart_y + 1 + index as u16;
        let bar_w = inner.width.saturating_sub(26);
        let filled = ((f64::from(bar_w) * update.download_bytes.unwrap_or(0) as f64 / max as f64) * reveal).round() as u16;
        put(buf, inner.x, y, vec![Span::styled(fit(&update.name, 12), Style::new().fg(pal.ink2))], 13);
        for i in 0..bar_w {
            buf[(inner.x + 13 + i, y)].set_symbol("━").set_fg(if i < filled { pal.teal_mid } else { pal.grid });
        }
        put_right(buf, inner.right(), y, vec![Span::styled(bytes(update.download_bytes.unwrap_or(0)), Style::new().fg(pal.ink))]);
    }

    // Dossier of the selected package
    let dossier = Rect::new(table.right() + 2, table.y, w.saturating_sub(table_w + 2), table.height);
    fill(buf, dossier, pal.panel);
    border(buf, dossier, false, pal);
    let Some(update) = updates.get(selected) else { return };
    let d = Rect::new(dossier.x + 2, dossier.y + 1, dossier.width.saturating_sub(4), dossier.height.saturating_sub(2));
    let subsystem = Subsystem::of(&update.name);
    scene.placements.push(Placement { rect: Rect::new(d.x, d.y, 12, 4), picture: Picture::Specimen(subsystem) });
    put(buf, d.x + 14, d.y, vec![Span::styled(update.name.clone(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))], d.width);
    let mut tags = risk_pill(update, pal);
    tags.push(Span::styled(format!("  {} · {} · {}", subsystem.label(), update.source, bump_group(&update.bump)), Style::new().fg(pal.label)));
    put(buf, d.x + 14, d.y + 1, tags, d.width.saturating_sub(14));
    put(buf, d.x + 14, d.y + 2, version_ruler(update, d.width.saturating_sub(14) as usize, pal), d.width.saturating_sub(14));
    if !update.reason.is_empty() {
        put(buf, d.x + 14, d.y + 3, vec![Span::styled(update.reason.clone(), Style::new().fg(pal.label))], d.width.saturating_sub(14));
    }
    let mut y = d.y + 5;
    let facts = [
        ("download", update.download_bytes.map(bytes).unwrap_or_else(|| "unknown (aur)".into())),
        ("disk change", update.installed_delta_bytes.map(signed_bytes).unwrap_or_else(|| "unknown".into())),
    ];
    for (label, value) in facts {
        put(buf, d.x, y, vec![Span::styled(label.to_string(), Style::new().fg(pal.label))], d.width);
        put_right(buf, d.right(), y, vec![Span::styled(value, Style::new().fg(pal.ink))]);
        y += 1;
    }
    let deps = app.anatomy.as_ref().and_then(|a| a.deps.get(&update.name)).cloned().unwrap_or_default();
    y += 1;
    put(buf, d.x, y, vec![Span::styled("DEPENDENCIES", Style::new().fg(pal.label))], d.width);
    scene.placements.push(Placement {
        rect: Rect::new(d.x, y + 1, d.width.min(36), 10),
        picture: Picture::Molecule { depends: deps.depends, required_by: deps.required_by, seed: hash_str(&update.name), reveal: 1.0 },
    });
    let facts_x = d.x + d.width.min(36) + 2;
    put(buf, facts_x, y + 2, vec![Span::styled(format!("{} it depends on", deps.depends), Style::new().fg(pal.ink2))], d.right().saturating_sub(facts_x));
    put(buf, facts_x, y + 3, vec![Span::styled(format!("{} require it", deps.required_by), Style::new().fg(pal.teal))], d.right().saturating_sub(facts_x));
    y += 12;
    put(buf, d.x, y, vec![Span::styled("LINKED PROBLEMS", Style::new().fg(pal.label))], d.width);
    let linked: Vec<_> = app.problems.iter().filter(|p| p.package == update.name).collect();
    if linked.is_empty() {
        put(buf, d.x, y + 1, vec![Span::styled("none on this system", Style::new().fg(pal.faint))], d.width);
    }
    for (index, problem) in linked.iter().enumerate() {
        let kind = if problem.kind == "crash" { "crashes" } else { "errors" };
        let fixed = problem.status == "possibly-fixed";
        put(
            buf,
            d.x,
            y + 1 + index as u16,
            vec![Span::styled(
                format!("{} {} {kind}{}", if fixed { "↗" } else { "·" }, problem.count, if fixed { " — may be fixed by this update" } else { "" }),
                Style::new().fg(if fixed { pal.teal } else { pal.ink2 }),
            )],
            d.width,
        );
    }

    draw_footer(buf, Rect::new(area.x + 1, area.bottom() - 1, area.width.saturating_sub(2), 1), app, pal, false, now);
}
