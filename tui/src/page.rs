use std::time::Instant;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use tiny_skia::Pixmap;

use crate::app::{App, Pane, Severity, Source, Subsystem, ease_out_cubic};
use crate::motion::{Motion, Signal, Target};
use crate::palette::{Palette, mix};

pub const MIN_WIDTH: u16 = 150;
pub const MIN_HEIGHT: u16 = 44;
const TILES_HEIGHT: u16 = 14;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const UNITS: [usize; 13] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000];

#[derive(Debug)]
pub enum Picture {
    Specimen { subsystem: Subsystem, signal: bool, glow: Option<(Color, f32)>, aim: f32 },
    Eye(Box<crate::ink::EyeSpec>),
}

pub struct Placement {
    pub rect: Rect,
    pub picture: Picture,
}

#[derive(Default)]
pub struct Scene {
    pub placements: Vec<Placement>,
    pub rows: [usize; 3],
}

pub fn paint(picture: &Picture, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (subsystem, signal, glow, aim) = match picture {
        Picture::Eye(spec) => return crate::ink::eye(spec, pal, width, height, cell_height),
        Picture::Specimen { subsystem, signal, glow, aim } => (subsystem, signal, glow, aim),
    };
    let mut tone = *pal;
    tone.panel = pal.page;
    if *signal {
        tone.ink = pal.teal;
    } else {
        tone.teal = pal.ink2;
    }
    let mut boost = 0.0;
    if let Some((color, strength)) = glow {
        let strength = strength.clamp(0.0, 1.0);
        let bright = mix(*color, pal.ink, 0.25);
        tone.ink = mix(tone.ink, bright, f64::from(strength));
        tone.teal = mix(tone.teal, bright, f64::from(strength));
        boost = strength;
    }
    crate::ink::specimen(*subsystem, &tone, width, height, cell_height, boost, (*aim > 0.01).then_some((pal.ink, *aim)))
}

pub fn moving(app: &App, motion: &Motion, now: Instant) -> bool {
    if app.skipped {
        return false;
    }
    let recent = |at: Option<Instant>, window: f64| at.is_some_and(|at| ms(now, at) < window && ms(now, at) > -100_000.0);
    let pending = |at: Option<Instant>| at.is_some_and(|at| at > now);
    motion.moving(now) || app.stage_done_at.iter().any(|at| recent(*at, 1800.0) || pending(*at)) || recent(app.anatomy_at, 1800.0) || recent(app.event_at, 1300.0)
}

pub fn draw(frame: &mut Frame, app: &App, motion: &Motion, pal: &Palette, now: Instant, cell: (u16, u16)) -> Scene {
    let area = frame.area();
    let buf = frame.buffer_mut();
    fill(buf, area, pal.page);
    let mut scene = Scene::default();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        draw_too_small(buf, area, pal);
        return scene;
    }
    draw_nerve(buf, area, app, motion, pal, now, cell, &mut scene);
    if app.overlay {
        scene.placements.clear();
        draw_overlay(buf, area, app, pal);
    }
    scene
}

fn draw_header(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let mut left = vec![Span::styled("rumb", Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))];
    if let Some(anatomy) = &app.anatomy {
        let omarchy = if anatomy.omarchy.is_empty() { String::new() } else { format!("Omarchy {}", anatomy.omarchy) };
        let kernel = if anatomy.kernel_running.is_empty() { String::new() } else { format!("kernel {}", anatomy.kernel_running) };
        let system: Vec<String> = [anatomy.hostname.clone(), anatomy.os.clone(), omarchy, anatomy.desktop.clone(), kernel].into_iter().filter(|part| !part.is_empty()).collect();
        left.push(Span::styled(format!("  {}", system.join(" · ")), Style::new().fg(pal.label)));
    }
    if app.demo {
        left.push(Span::styled("  demo data", Style::new().fg(pal.warn)));
    }
    let mut status = Vec::new();
    if let Some(error) = &app.error {
        status.push(Span::styled(format!("scan failed: {}   ", fit(error, 60)), Style::new().fg(pal.alert).add_modifier(Modifier::BOLD)));
    }
    let scan = match (&app.scan_time, app.finished_at) {
        (_, None) => {
            let step = app.steps.last().filter(|step| step.ended.is_none()).map(|step| fit(&step.label, 40)).unwrap_or_default();
            format!("{step} {}", SPINNER[(ms(now, app.started) / 90.0) as usize % SPINNER.len()]).trim_start().to_string()
        }
        (Some(time), Some(done)) => {
            let seconds = app.steps.first().map_or(0.0, |first| done.saturating_duration_since(first.started).as_secs_f64());
            format!("scanned {} · {seconds:.1} s", short_time(time))
        }
        (None, Some(_)) => "scanned".to_string(),
    };
    status.push(Span::styled(scan, Style::new().fg(pal.label)));
    if app.finished_at.is_some() && !app.demo {
        if app.live_lost {
            status.push(Span::styled("   × live feed stopped", Style::new().fg(pal.alert)));
        } else if app.transacting() {
            status.push(Span::styled("   ● pacman is running", Style::new().fg(pal.warn).add_modifier(Modifier::BOLD)));
        } else {
            status.push(Span::styled("   ● live", Style::new().fg(pal.ink2)));
        }
    }
    let used = width(&status);
    put(buf, area.x + 1, area.y, left, area.width.saturating_sub(used + 4));
    put_right(buf, area.right() - 1, area.y, status);
}

fn draw_tiles(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let count = 6u16;
    let width = area.width.saturating_sub(2) / count;
    for index in 0..count {
        let x = area.x + 1 + index * width;
        let w = if index + 1 == count { area.right() - 1 - x } else { width };
        let rect = Rect::new(x, area.y, w, area.height);
        match index {
            0 => tile_pending(buf, rect, app, pal, now),
            1 => tile_days(buf, rect, app, pal, now),
            2 => tile_since(buf, rect, app, pal, now),
            3 => tile_fixes(buf, rect, app, pal, now),
            4 => tile_disk(buf, rect, app, pal, now),
            _ => tile_checks(buf, rect, app, pal),
        }
    }
}

fn tile_pending(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let inner = frame(buf, area, "¹PENDING", false, pal);
    let Some(done) = app.stage_done_at[0] else {
        text(buf, inner.x, inner.y + 2, "checking…", Style::new().fg(pal.faint), inner.width);
        return;
    };
    let total = app.updates.len();
    let (high, medium) = (app.high(), app.medium());
    let low = total - high - medium;
    if total == 0 {
        let x = big(buf, inner.x, inner.y + 2, "0", Style::new().fg(pal.ink));
        text(buf, x, inner.y + 4, "up to date", Style::new().fg(pal.label), inner.right().saturating_sub(x));
        legend(buf, area, "nothing to install", pal);
        return;
    }
    let cols = (inner.width as usize).div_ceil(2);
    let rows = 7usize;
    let capacity = cols * rows;
    let squares = |unit: usize| high.div_ceil(unit) + medium.div_ceil(unit) + low.div_ceil(unit);
    let unit = UNITS.into_iter().find(|unit| squares(*unit) <= capacity).unwrap_or(10000);
    let colors: Vec<Color> = std::iter::repeat_n(pal.alert, high.div_ceil(unit))
        .chain(std::iter::repeat_n(pal.warn, medium.div_ceil(unit)))
        .chain(std::iter::repeat_n(pal.ink2, low.div_ceil(unit)))
        .collect();
    let shown = (reveal(app, now, Some(done), 0.0, 900.0) * colors.len() as f64).ceil() as usize;
    for k in 0..capacity {
        let (row, col) = (k / cols, k % cols);
        let color = if k < shown { colors.get(k).copied().unwrap_or(pal.grid) } else { pal.grid };
        text(buf, inner.x + col as u16 * 2, inner.y + 1 + row as u16, "■", Style::new().fg(color), 1);
    }
    let count = |value: usize, color: Color| Span::styled(value.to_string(), Style::new().fg(color).add_modifier(Modifier::BOLD));
    let word = |value: &str| Span::styled(value.to_string(), Style::new().fg(pal.label));
    let long = vec![count(high, pal.alert), word(" high  "), count(medium, pal.warn), word(" med  "), count(low, pal.ink2), word(" low")];
    let short = vec![count(high, pal.alert), word(" hi "), count(medium, pal.warn), word(" med "), count(low, pal.ink2), word(" lo")];
    put(buf, inner.x, inner.y + 9, if width(&long) <= inner.width { long } else { short }, inner.width);
    legend(buf, area, &format!("1 ■ = {unit} {}", if unit == 1 { "update" } else { "updates" }), pal);
}

fn tile_days(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let inner = frame(buf, area, "PROBLEMS / DAY", false, pal);
    let Some(done) = app.stage_done_at[1] else {
        text(buf, inner.x, inner.y + 2, "reading the journal…", Style::new().fg(pal.faint), inner.width);
        return;
    };
    let columns = (inner.width as usize).div_ceil(2);
    let start = app.activity.len().saturating_sub(columns);
    let mut days: Vec<(&str, u64, u64)> = app.activity[start..].iter().map(|day| (day.date.as_str(), day.crashes, day.errors)).collect();
    if let Some(last) = days.last_mut() {
        last.2 += app.journal_live as u64;
    }
    let rows = 7usize;
    let peak = days.iter().map(|(_, crashes, errors)| crashes + errors).max().unwrap_or(0) as usize;
    let unit = UNITS.into_iter().find(|unit| peak.div_ceil(*unit) <= rows).unwrap_or(10000);
    let upgrade_day = app.last_upgrade.as_deref().and_then(|time| time.get(..10));
    for (k, (date, crashes, errors)) in days.iter().enumerate() {
        let x = inner.x + k as u16 * 2;
        let filled = ((crashes + errors) as usize).div_ceil(unit).min(rows);
        let red = (*crashes as usize).div_ceil(unit).min(filled);
        let height = (filled as f64 * reveal(app, now, Some(done), k as f64 * 35.0, 450.0)).round() as usize;
        let marked = upgrade_day == Some(*date);
        for row in 0..rows {
            let color = if row >= height {
                pal.grid
            } else if row < red {
                pal.alert
            } else if marked {
                pal.ink
            } else {
                pal.ink2
            };
            text(buf, x, inner.y + rows as u16 - row as u16, "■", Style::new().fg(color), 1);
        }
        if marked {
            text(buf, x, inner.y, "▼", Style::new().fg(pal.ink), 1);
        }
    }
    if days.is_empty() {
        text(buf, inner.x, inner.y + 3, "no days to show", Style::new().fg(pal.faint), inner.width);
    }
    put(
        buf,
        inner.x,
        inner.y + 9,
        vec![
            Span::styled("■", Style::new().fg(pal.alert)),
            Span::styled(" crashes  ", Style::new().fg(pal.label)),
            Span::styled("▼", Style::new().fg(pal.ink)),
            Span::styled(" upgrade", Style::new().fg(pal.label)),
        ],
        inner.width,
    );
    legend(buf, area, &format!("1 ■ = {unit} {}", if unit == 1 { "error" } else { "errors" }), pal);
}

fn tile_since(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let inner = frame(buf, area, "SINCE UPGRADE", false, pal);
    let (Some(last), Some(scan)) = (app.last_upgrade.as_deref(), app.scan_time.as_deref()) else {
        let message = if app.start_at.is_some() { "no full upgrade logged" } else { "reading pacman.log…" };
        text(buf, inner.x, inner.y + 2, message, Style::new().fg(pal.faint), inner.width);
        return;
    };
    let (Some(last_epoch), Some(scan_epoch)) = (epoch(last), epoch(scan)) else { return };
    let live = app.start_at.map_or(0, |at| now.saturating_duration_since(at).as_secs() as i64);
    let elapsed = (scan_epoch - last_epoch + live).max(0);
    let (days, hours, minutes) = (elapsed / 86400, (elapsed % 86400) / 3600, (elapsed % 3600) / 60);
    let (number, rest) = if days > 0 {
        (days, format!("{} {hours} h", if days == 1 { "day" } else { "days" }))
    } else if hours > 0 {
        (hours, format!("h {minutes:02} min"))
    } else {
        (minutes, "min".to_string())
    };
    let x = big(buf, inner.x, inner.y + 1, &number.to_string(), Style::new().fg(pal.ink));
    text(buf, x, inner.y + 3, &rest, Style::new().fg(pal.label), inner.right().saturating_sub(x));
    let cols = (inner.width as usize).div_ceil(2);
    let lit = (days as usize).min(cols * 2);
    for k in 0..cols * 2 {
        let color = if k < lit { pal.ink2 } else { pal.grid };
        text(buf, inner.x + (k % cols) as u16 * 2, inner.y + 5 + (k / cols) as u16, "■", Style::new().fg(color), 1);
    }
    if let Some(when) = last.get(5..16) {
        text(buf, inner.x, inner.y + 8, &format!("on {}", when.replace('T', " ")), Style::new().fg(pal.label), inner.width);
    }
    if let Some(count) = upgraded_that_day(app, last, scan) {
        let detail = format!("{count} upgraded that day");
        let detail = if detail.chars().count() > inner.width as usize { format!("{count} that day") } else { detail };
        text(buf, inner.x, inner.y + 9, &detail, Style::new().fg(pal.faint), inner.width);
    }
    legend(buf, area, "1 ■ = 1 day", pal);
}

fn upgraded_that_day(app: &App, last: &str, scan: &str) -> Option<u64> {
    let anatomy = app.anatomy.as_ref()?;
    let day = |iso: &str| iso.get(..10).and_then(|date| epoch(&format!("{date}T00:00:00"))).map(|secs| secs.div_euclid(86400));
    let back = usize::try_from(day(scan)? - day(last)?).ok()?;
    let index = anatomy.upgrades_per_day.len().checked_sub(1 + back)?;
    anatomy.upgrades_per_day.get(index).copied()
}

fn tile_fixes(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let inner = frame(buf, area, "OWNER HAS UPDATE", false, pal);
    let (Some(updates), Some(problems)) = (app.stage_done_at[0], app.stage_done_at[1]) else {
        text(buf, inner.x, inner.y + 2, "linking…", Style::new().fg(pal.faint), inner.width);
        return;
    };
    let done = updates.max(problems);
    let mut fixes: Vec<_> = app.problems.iter().filter(|problem| problem.status == "possibly-fixed").collect();
    fixes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.subject.cmp(&b.subject)));
    let counted = (fixes.len() as f64 * reveal(app, now, Some(done), 0.0, 600.0)).round() as usize;
    let x = big(buf, inner.x, inner.y + 1, &counted.to_string(), Style::new().fg(if fixes.is_empty() { pal.ink2 } else { pal.teal }));
    let room = inner.right().saturating_sub(x);
    text(buf, x, inner.y + 2, &format!("of {}", app.problems.len()), Style::new().fg(pal.label), room);
    text(buf, x, inner.y + 3, if app.problems.len() == 1 { "problem" } else { "problems" }, Style::new().fg(pal.label), room);
    let (name_w, count_w) = (8u16, 5u16);
    let bar_w = inner.width.saturating_sub(name_w + 1 + count_w);
    let max = fixes.first().map_or(1, |problem| problem.count.max(1));
    for (k, problem) in fixes.iter().take(5).enumerate() {
        let y = inner.y + 5 + k as u16;
        text(buf, inner.x, y, &fit(problem.name(), name_w as usize), Style::new().fg(pal.ink), name_w);
        let share = problem.count as f64 / max as f64 * reveal(app, now, Some(done), k as f64 * 60.0, 500.0);
        let length = (f64::from(bar_w) * share).round().max(1.0) as usize;
        text(buf, inner.x + name_w + 1, y, &"━".repeat(length), Style::new().fg(pal.teal), bar_w);
        right(buf, inner.right(), y, &compact(problem.count), Style::new().fg(pal.ink2));
    }
    if fixes.is_empty() {
        text(buf, inner.x, inner.y + 5, "no owner has one", Style::new().fg(pal.faint), inner.width);
    }
    legend(buf, area, "bar = record count", pal);
}

fn tile_disk(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let inner = frame(buf, area, "DISK AND PACKAGES", false, pal);
    if let Some(preflight) = &app.preflight {
        let share = if preflight.disk_total_bytes > 0 { preflight.disk_used_bytes as f64 / preflight.disk_total_bytes as f64 } else { 0.0 };
        text(buf, inner.x, inner.y + 1, "disk /", Style::new().fg(pal.label), 6);
        let color = if preflight.free_ok { pal.ink2 } else { pal.alert };
        meter(buf, inner.x + 7, inner.y + 1, inner.width.saturating_sub(12), share * reveal(app, now, app.stage_done_at[3], 0.0, 700.0), color, pal);
        right(buf, inner.right(), inner.y + 1, &format!("{:.0}%", share * 100.0), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD));
        row(buf, inner, inner.y + 2, "free", &bytes(preflight.free_bytes), if preflight.free_ok { pal.ink } else { pal.alert }, pal);
    } else {
        text(buf, inner.x, inner.y + 1, "measuring…", Style::new().fg(pal.faint), inner.width);
    }
    let y = inner.y + 3;
    let Some(anatomy) = &app.anatomy else {
        text(buf, inner.x, y, "counting packages…", Style::new().fg(pal.faint), inner.width);
        return;
    };
    row(buf, inner, y, "installed", &thousands(anatomy.packages), pal.ink, pal);
    row(buf, inner, y + 1, "explicit", &thousands(anatomy.explicit), pal.ink, pal);
    row(buf, inner, y + 2, "from AUR", &thousands(anatomy.repos.get("aur").copied().unwrap_or(0)), pal.ink, pal);
    row(buf, inner, y + 3, "orphans", &thousands(anatomy.orphans), if anatomy.orphans > 0 { pal.warn } else { pal.ink }, pal);
    let cache = bytes(anatomy.cache_bytes);
    let cache_label = if 13 + cache.len() > inner.width as usize { "cache" } else { "pacman cache" };
    row(buf, inner, y + 4, cache_label, &cache, pal.ink, pal);
    match anatomy.cache_freeable_bytes {
        Some(freeable) if anatomy.cache_bytes > 0 => {
            let share = (freeable as f64 / anatomy.cache_bytes as f64).clamp(0.0, 1.0);
            let kept = (f64::from(inner.width) * (1.0 - share)).round() as u16;
            for i in 0..inner.width {
                text(buf, inner.x + i, y + 5, "■", Style::new().fg(if i < kept { pal.ink2 } else { pal.warn }), 1);
            }
            let (message, color) = if freeable > 0 { (format!("{} freeable", bytes(freeable)), pal.warn) } else { ("nothing to free".to_string(), pal.faint) };
            text(buf, inner.x, y + 6, &message, Style::new().fg(color), inner.width);
            let legend_style = Style::new().fg(pal.faint);
            legend_spans(
                buf,
                area,
                vec![Span::styled("■", Style::new().fg(pal.ink2)), Span::styled(" kept  ", legend_style), Span::styled("■", Style::new().fg(pal.warn)), Span::styled(" freeable", legend_style)],
                pal,
            );
        }
        _ => text(buf, inner.x, y + 6, "freeable: unknown", Style::new().fg(pal.faint), inner.width),
    }
}

fn tile_checks(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette) {
    let inner = frame(buf, area, "BEFORE YOU UPDATE", false, pal);
    let level = |ok: bool, bad: Severity| if ok { Severity::Ok } else { bad };
    let mut checks: Vec<(&str, String, Severity)> = Vec::new();
    if app.stage_done_at[0].is_some() {
        let high = app.high();
        checks.push(("risk", format!("{high} high"), level(high == 0, Severity::Bad)));
    }
    if let Some(anatomy) = &app.anatomy {
        let reboot = if anatomy.reboot_pending { "pending" } else { "not needed" };
        checks.push(("reboot", reboot.into(), level(!anatomy.reboot_pending, Severity::Bad)));
    }
    if let Some(preflight) = &app.preflight {
        let snapshot = if preflight.snapshot_before_update {
            "automatic"
        } else if preflight.snapper_configured {
            "not auto"
        } else {
            "none"
        };
        checks.push(("snapshot", snapshot.into(), level(preflight.snapshot_before_update, Severity::Warn)));
        checks.push(("space", bytes(preflight.free_bytes), level(preflight.free_ok, Severity::Bad)));
    }
    if app.stage_done_at[2].is_some() {
        let (news, severity) = if app.news_failed {
            ("failed".to_string(), Severity::Warn)
        } else if app.manual() > 0 {
            (format!("{} manual", app.manual()), Severity::Bad)
        } else if app.news.is_empty() {
            ("none new".to_string(), Severity::Ok)
        } else {
            (format!("{} new", app.news.len()), Severity::Ok)
        };
        checks.push(("news", news, severity));
    }
    if app.finished_at.is_some() {
        let failed = app.update_errors.len() + app.problem_errors.len() + usize::from(app.news_failed);
        checks.push(("checks", if failed > 0 { format!("{failed} failed") } else { "all ran".into() }, level(failed == 0, Severity::Warn)));
    }
    let tone = |severity: Severity| match severity {
        Severity::Bad => pal.alert,
        Severity::Warn => pal.warn,
        _ => pal.ink,
    };
    for (k, (name, value, severity)) in checks.iter().enumerate() {
        let y = inner.y + 1 + k as u16;
        text(buf, inner.x, y, "●", Style::new().fg(if *severity == Severity::Ok { pal.ink2 } else { tone(*severity) }), 1);
        text(buf, inner.x + 2, y, name, Style::new().fg(pal.label), 8);
        right(buf, inner.right(), y, &fit(value, inner.width.saturating_sub(11) as usize), Style::new().fg(tone(*severity)));
    }
    if app.finished_at.is_some() {
        let (verdict, severity) = app.verdict();
        let bg = match severity {
            Severity::Bad => pal.alert,
            Severity::Warn => pal.warn,
            Severity::Ok => pal.ink,
            Severity::Neutral => pal.ink2,
        };
        text(buf, inner.x, inner.y + 9, &format!(" {verdict} "), Style::new().fg(pal.page).bg(bg).add_modifier(Modifier::BOLD), inner.width);
    }
    let legend_style = Style::new().fg(pal.faint);
    legend_spans(
        buf,
        area,
        vec![
            Span::styled("●", Style::new().fg(pal.alert)),
            Span::styled(" act ", legend_style),
            Span::styled("●", Style::new().fg(pal.warn)),
            Span::styled(" look ", legend_style),
            Span::styled("●", Style::new().fg(pal.ink2)),
            Span::styled(" ok", legend_style),
        ],
        pal,
    );
}

fn list_frame(buf: &mut Buffer, area: Rect, app: &App, pane: Pane, title: &str, legend_text: &str, pal: &Palette) -> Rect {
    let focused = app.focus == pane;
    let inner = frame(buf, area, title, focused, pal);
    let list = &app.lists[pane.index()];
    let editing = focused && app.editing;
    if editing || !list.filter.is_empty() {
        let mut spans = vec![Span::styled("filter ", Style::new().fg(pal.label)), Span::styled(fit(&list.filter, 24), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD))];
        if editing {
            spans.push(Span::styled("▏", Style::new().fg(pal.ink)));
        }
        put_right(buf, inner.right(), area.y + 1, spans);
    }
    legend(buf, area, legend_text, pal);
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(3))
}

fn window(selected: usize, total: usize, rows: usize) -> (usize, usize) {
    let offset = (selected + 1).saturating_sub(rows);
    (offset, (offset + rows).min(total))
}

fn select_row(buf: &mut Buffer, area: Rect, focused: bool, pal: &Palette) {
    highlight(buf, area, if focused { pal.select } else { pal.kpi }, focused, pal);
}

fn highlight(buf: &mut Buffer, area: Rect, bg: Color, brighten: bool, pal: &Palette) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            if cell.fg == pal.grid {
                cell.set_fg(pal.rule);
            } else if brighten && (cell.fg == pal.label || cell.fg == pal.faint || cell.fg == pal.ink2) {
                cell.set_fg(pal.ink);
            }
            cell.set_bg(bg);
        }
    }
}

fn draw_updates(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette) -> usize {
    let updates = app.visible_updates();
    let download = app.download_bytes();
    let title = if download > 0 { format!("³UPDATES · {} · {} download", updates.len(), bytes(download)) } else { format!("³UPDATES · {}", updates.len()) };
    let body = list_frame(buf, area, app, Pane::Updates, &title, "sorted by risk · enter opens · / filters", pal);
    let name_w = body.width.saturating_sub(32).clamp(14, 24);
    let version_x = body.x + 5 + name_w + 1;
    let header = Style::new().fg(pal.faint);
    text(buf, body.x, body.y, "risk", header, 4);
    text(buf, body.x + 5, body.y, "package", header, name_w);
    text(buf, version_x, body.y, "old → new", header, 20);
    right(buf, body.right(), body.y, "size", header);
    let rows = body.height.saturating_sub(1) as usize;
    let selected = app.lists[0].selected.min(updates.len().saturating_sub(1));
    let (start, end) = window(selected, updates.len(), rows);
    for (row, index) in (start..end).enumerate() {
        let update = updates[index];
        let y = body.y + 1 + row as u16;
        let (level, color) = match update.risk.as_str() {
            "high" => (3, pal.alert),
            "medium" => (2, pal.warn),
            _ => (1, pal.ink2),
        };
        text(buf, body.x, y, &"■".repeat(level), Style::new().fg(color), 3);
        text(buf, body.x + level as u16, y, &"■".repeat(3 - level), Style::new().fg(pal.grid), 3);
        let chosen = index == selected;
        text(buf, body.x + 5, y, &fit(&update.name, name_w as usize), Style::new().fg(pal.ink).add_modifier(if chosen { Modifier::BOLD } else { Modifier::empty() }), name_w);
        let size = update.download_bytes.map(short_bytes).unwrap_or_else(|| "aur".into());
        let room = body.right().saturating_sub(version_x + 8);
        text(buf, version_x, y, &versions(&update.old, &update.new, room as usize), Style::new().fg(pal.ink2), room);
        if app.problems.iter().any(|problem| problem.package == update.name) {
            text(buf, body.right().saturating_sub(7), y, "●", Style::new().fg(pal.teal), 1);
        }
        right(buf, body.right(), y, &size, Style::new().fg(pal.label));
        if chosen {
            select_row(buf, Rect::new(body.x.saturating_sub(1), y, body.width + 2, 1), app.focus == Pane::Updates, pal);
        }
    }
    if updates.is_empty() {
        let message = if app.stage_done_at[0].is_none() {
            "checking…"
        } else if app.lists[0].filter.is_empty() {
            "nothing pending"
        } else {
            "no match"
        };
        text(buf, body.x, body.y + 1, message, Style::new().fg(pal.faint), body.width);
    }
    position(buf, body, selected, updates.len(), pal);
    rows
}

fn draw_problems(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette) -> usize {
    let problems = app.visible_problems();
    let since = app.since.as_deref().and_then(|since| since.get(5..10)).map(|date| format!(" since {}", date.replace('-', "/"))).unwrap_or_default();
    let title = format!("⁴PROBLEMS · {}{since}", problems.len());
    let body = list_frame(buf, area, app, Pane::Problems, &title, "→ = the owner has an update · j journal", pal);
    let header = Style::new().fg(pal.faint);
    text(buf, body.x, body.y, "count", header, 6);
    text(buf, body.x + 16, body.y, "subject · owner", header, 30);
    let rows = (body.height.saturating_sub(1) / 2) as usize;
    let selected = app.lists[1].selected.min(problems.len().saturating_sub(1));
    let (start, end) = window(selected, problems.len(), rows);
    let max = problems.iter().map(|problem| problem.count).max().unwrap_or(1).max(1) as f64;
    for (row, index) in (start..end).enumerate() {
        let problem = problems[index];
        let y = body.y + 1 + row as u16 * 2;
        right(buf, body.x + 5, y, &compact(problem.count), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD));
        let bar = ((problem.count as f64).ln_1p() / max.ln_1p() * 8.0).round().clamp(1.0, 8.0) as usize;
        let severe = problem.kind == "crash" || problem.kind == "failed-unit";
        text(buf, body.x + 6, y, &"■".repeat(bar), Style::new().fg(if severe { pal.alert } else { pal.ink2 }), 8);
        text(buf, body.x + 6 + bar as u16, y, &"■".repeat(8 - bar), Style::new().fg(pal.grid), 8);
        let (kind, kind_color) = match problem.kind.as_str() {
            "crash" => ("crash", pal.alert),
            "failed-unit" => ("unit", pal.alert),
            "config" => ("config", pal.warn),
            _ => ("error", pal.label),
        };
        let subject_w = body.width.saturating_sub(24);
        text(buf, body.x + 16, y, &fit(&problem.subject, subject_w as usize), Style::new().fg(pal.ink), subject_w);
        right(buf, body.right(), y, kind, Style::new().fg(kind_color));
        let (owner, color) = match problem.status.as_str() {
            "possibly-fixed" => (format!("{} → {}", problem.package, short_version(problem.fix_version())), pal.teal),
            "config" => ("fix in the Hyprland config".to_string(), pal.warn),
            "not-packaged" => ("not from a package".to_string(), pal.faint),
            "unknown-owner" => ("owner unknown".to_string(), pal.faint),
            _ => (format!("{} · no update pending", problem.package), pal.label),
        };
        let owner_w = body.width.saturating_sub(16);
        text(buf, body.x + 16, y + 1, &fit(&owner, owner_w as usize), Style::new().fg(color), owner_w);
        if index == selected {
            select_row(buf, Rect::new(body.x.saturating_sub(1), y, body.width + 2, 2), app.focus == Pane::Problems, pal);
        }
    }
    if problems.is_empty() {
        let message = if app.stage_done_at[1].is_none() {
            "reading crashes and the journal…"
        } else if app.lists[1].filter.is_empty() {
            "none found"
        } else {
            "no match"
        };
        text(buf, body.x, body.y + 1, message, Style::new().fg(pal.faint), body.width);
    }
    position(buf, body, selected, problems.len(), pal);
    rows
}

fn draw_events(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) -> usize {
    let events = app.visible_events();
    let state = if app.follow { "following" } else { "paused · Home follows" };
    let title = format!("⁵LIVE EVENTS · {state}");
    let body = list_frame(buf, area, app, Pane::Events, &title, "journal errors + pacman · newest first", pal);
    let ident_w = 12u16;
    let message_x = body.x + 11 + ident_w + 1;
    let header = Style::new().fg(pal.faint);
    text(buf, body.x, body.y, "time", header, 8);
    text(buf, body.x + 11, body.y, "source", header, ident_w);
    text(buf, message_x, body.y, "message", header, 10);
    let rows = body.height.saturating_sub(1) as usize;
    let selected = app.lists[2].selected.min(events.len().saturating_sub(1));
    let (start, end) = window(selected, events.len(), rows);
    for (row, index) in (start..end).enumerate() {
        let event = events[index];
        let y = body.y + 1 + row as u16;
        let (tag, color) = match event.source {
            Source::Journal => ("J", pal.label),
            Source::Pacman => ("P", pal.ink),
        };
        text(buf, body.x, y, &event.clock, Style::new().fg(pal.faint), 8);
        text(buf, body.x + 9, y, tag, Style::new().fg(color).add_modifier(Modifier::BOLD), 1);
        text(buf, body.x + 11, y, &fit(&event.ident, ident_w as usize), Style::new().fg(pal.ink2), ident_w);
        let room = body.right().saturating_sub(message_x);
        text(buf, message_x, y, &fit(&event.message, room as usize), Style::new().fg(pal.ink), room);
        let line = Rect::new(body.x.saturating_sub(1), y, body.width + 2, 1);
        let age = ms(now, event.at);
        if index == selected && app.focus == Pane::Events {
            select_row(buf, line, true, pal);
        } else if !app.skipped && age < 1300.0 {
            highlight(buf, line, mix(pal.select, pal.page, age / 1300.0), age < 650.0, pal);
        }
    }
    if events.is_empty() {
        let message = if app.lists[2].filter.is_empty() { "no errors in the journal yet" } else { "no match" };
        text(buf, body.x, body.y + 1, message, Style::new().fg(pal.faint), body.width);
    }
    position(buf, body, selected, events.len(), pal);
    rows
}

fn position(buf: &mut Buffer, body: Rect, selected: usize, total: usize, pal: &Palette) {
    if total > 0 {
        right(buf, body.right(), body.bottom(), &format!("{} of {}", selected + 1, thousands(total as u64)), Style::new().fg(pal.faint));
    }
}

fn draw_keys(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette, now: Instant) {
    let mut room = area.width.saturating_sub(2);
    if let Some((message, ok, at)) = &app.status
        && now.saturating_duration_since(*at).as_secs() < 8
    {
        let span = Span::styled(message.clone(), Style::new().fg(if *ok { pal.ink } else { pal.alert }).add_modifier(Modifier::BOLD));
        room = room.saturating_sub(span.width() as u16 + 3);
        put_right(buf, area.right() - 1, area.y, vec![span]);
    }
    let pairs: Vec<(&str, &str)> = if app.editing {
        vec![("type", "to filter"), ("Enter", "keep"), ("Esc", "clear")]
    } else if app.overlay {
        vec![("↑↓", "previous/next"), ("Esc", "close"), ("e", "export"), ("q", "quit")]
    } else {
        let mut pairs = vec![("Tab", "focus"), ("↑↓", "select"), ("PgUp/PgDn", "page"), ("Enter", "open"), ("/", "filter"), ("e", "export")];
        if app.focus == Pane::Problems {
            pairs.push(("j", "journal"));
        }
        pairs.extend([("r", "rescan"), ("q", "quit")]);
        pairs
    };
    let mut spans = Vec::new();
    if !app.editing && !app.overlay {
        for (index, on) in app.visible.iter().enumerate() {
            spans.push(Span::styled(format!("{}", index + 1), Style::new().fg(if *on { pal.ink } else { pal.faint }).add_modifier(Modifier::BOLD)));
        }
        spans.push(Span::styled(" show/hide   ", Style::new().fg(pal.label)));
    }
    let mut used = width(&spans);
    for (key, word) in pairs {
        let pair = [Span::styled(key.to_string(), Style::new().fg(pal.ink).add_modifier(Modifier::BOLD)), Span::styled(format!(" {word}   "), Style::new().fg(pal.label))];
        let size = width(&pair);
        if used + size > room + 3 {
            break;
        }
        used += size;
        spans.extend(pair);
    }
    put(buf, area.x + 1, area.y, spans, room);
}

fn draw_overlay(buf: &mut Buffer, area: Rect, app: &App, pal: &Palette) {
    let width = area.width.saturating_sub(20).min(96);
    let room = width.saturating_sub(18) as usize;
    let mut lines: Vec<(String, String, Color)> = Vec::new();
    let title;
    match app.focus {
        Pane::Updates => {
            let updates = app.visible_updates();
            let Some(update) = updates.get(app.lists[0].selected.min(updates.len().saturating_sub(1))) else { return };
            title = update.name.clone();
            lines.push(("version".into(), format!("{} → {}", update.old, update.new), pal.ink));
            let risk_color = match update.risk.as_str() {
                "high" => pal.alert,
                "medium" => pal.warn,
                _ => pal.ink,
            };
            lines.push(("risk".into(), format!("{} · {}", update.risk, if update.reason.is_empty() { "routine" } else { &update.reason }), risk_color));
            lines.push(("jump".into(), if update.bump.is_empty() { "other".into() } else { update.bump.clone() }, pal.ink));
            lines.push(("source".into(), update.source.clone(), pal.ink));
            lines.push(("part".into(), Subsystem::of(&update.name).label().to_string(), pal.ink));
            lines.push(("download".into(), update.download_bytes.map(bytes).unwrap_or_else(|| "unknown".into()), pal.ink));
            lines.push(("disk change".into(), update.installed_delta_bytes.map(signed_bytes).unwrap_or_else(|| "unknown".into()), pal.ink));
            if let Some(deps) = app.anatomy.as_ref().and_then(|anatomy| anatomy.deps.get(&update.name)) {
                lines.push(("depends on".into(), format!("{} packages", deps.depends), pal.ink));
                lines.push(("required by".into(), format!("{} packages", deps.required_by), pal.ink));
            }
            for problem in app.problems.iter().filter(|problem| problem.package == update.name) {
                lines.push(("problem".into(), format!("{} × {} · {}", thousands(problem.count), problem.kind, problem.subject), pal.teal));
            }
        }
        Pane::Problems => {
            let problems = app.visible_problems();
            let Some(problem) = problems.get(app.lists[1].selected.min(problems.len().saturating_sub(1))) else { return };
            title = problem.name().to_string();
            lines.push(("subject".into(), problem.subject.clone(), pal.ink));
            lines.push(("kind".into(), problem.kind.clone(), pal.ink));
            let since = app.since.as_deref().and_then(|since| since.get(5..16)).map(|since| format!(" since {}", since.replace('T', " "))).unwrap_or_default();
            lines.push(("count".into(), format!("{}{since}", thousands(problem.count)), pal.ink));
            lines.push(("owner".into(), if problem.package.is_empty() { "unknown".into() } else { problem.package.clone() }, pal.ink));
            let status = match problem.status.as_str() {
                "possibly-fixed" => format!("the owner has an update: {}", problem.update),
                "config" => "a configuration error, no package can fix it".into(),
                "not-packaged" => "the program is not from a package".into(),
                "unknown-owner" => "the journal did not record which program it was".into(),
                _ => "no update pending for the owner".into(),
            };
            lines.push(("status".into(), status, if problem.status == "possibly-fixed" { pal.teal } else { pal.ink }));
            lines.push(("note".into(), "an update to the owner is not a verified fix".into(), pal.faint));
            lines.push(("j".into(), format!("shows the live journal filtered to {}", problem.name()), pal.label));
        }
        Pane::Events => {
            let events = app.visible_events();
            let Some(event) = events.get(app.lists[2].selected.min(events.len().saturating_sub(1))) else { return };
            title = event.ident.clone();
            lines.push(("time".into(), event.clock.clone(), pal.ink));
            lines.push(("source".into(), if event.source == Source::Pacman { "pacman log".into() } else { "journal, priority error or higher".into() }, pal.ink));
            let mut current = String::new();
            let mut first = true;
            for word in event.message.split_whitespace() {
                if !current.is_empty() && current.chars().count() + word.chars().count() + 1 > room {
                    lines.push((if first { "message".into() } else { String::new() }, std::mem::take(&mut current), pal.ink));
                    first = false;
                }
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(word);
            }
            lines.push((if first { "message".into() } else { String::new() }, current, pal.ink));
        }
    }
    let height = (lines.len() as u16 + 4).min(area.height.saturating_sub(6));
    let rect = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    clear(buf, rect, pal);
    let inner = frame(buf, rect, &title, true, pal);
    for (k, (name, value, color)) in lines.iter().enumerate().take(inner.height.saturating_sub(1) as usize) {
        let y = inner.y + 1 + k as u16;
        text(buf, inner.x, y, name, Style::new().fg(pal.label), 13);
        text(buf, inner.x + 14, y, &fit(value, inner.width.saturating_sub(14) as usize), Style::new().fg(*color), inner.width.saturating_sub(14));
    }
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

fn frame(buf: &mut Buffer, area: Rect, title: &str, focused: bool, pal: &Palette) -> Rect {
    if area.width < 4 || area.height < 3 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let style = Style::new().fg(if focused { pal.ink } else { pal.rule });
    let right_x = area.right() - 1;
    let bottom_y = area.bottom() - 1;
    for x in area.x + 1..right_x {
        buf[(x, area.y)].set_symbol("─").set_style(style);
        buf[(x, bottom_y)].set_symbol("─").set_style(style);
    }
    for y in area.y + 1..bottom_y {
        buf[(area.x, y)].set_symbol("│").set_style(style);
        buf[(right_x, y)].set_symbol("│").set_style(style);
    }
    buf[(area.x, area.y)].set_symbol("┌").set_style(style);
    buf[(right_x, area.y)].set_symbol("┐").set_style(style);
    buf[(area.x, bottom_y)].set_symbol("└").set_style(style);
    buf[(right_x, bottom_y)].set_symbol("┘").set_style(style);
    let inner = Rect::new(area.x + 2, area.y + 1, area.width.saturating_sub(4), area.height.saturating_sub(2));
    let title_style = if focused { Style::new().fg(pal.ink).add_modifier(Modifier::BOLD) } else { Style::new().fg(pal.label) };
    text(buf, inner.x, inner.y, &format!(":: {title}"), title_style, inner.width);
    Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1))
}

fn legend(buf: &mut Buffer, area: Rect, legend_text: &str, pal: &Palette) {
    legend_spans(buf, area, vec![Span::styled(legend_text.to_string(), Style::new().fg(pal.faint))], pal);
}

fn legend_spans(buf: &mut Buffer, area: Rect, spans: Vec<Span<'static>>, pal: &Palette) {
    if area.height < 4 || area.width < 6 {
        return;
    }
    let y = area.bottom() - 2;
    for x in area.x + 1..area.right() - 1 {
        buf[(x, y)].set_symbol("─").set_fg(pal.grid);
    }
    let mut line = vec![Span::raw(" ")];
    line.extend(spans);
    line.push(Span::raw(" "));
    put(buf, area.x + 2, y, line, area.width.saturating_sub(4));
}

fn meter(buf: &mut Buffer, x: u16, y: u16, width: u16, share: f64, color: Color, pal: &Palette) {
    let filled = (f64::from(width) * share.clamp(0.0, 1.0)).round() as u16;
    for i in 0..width {
        text(buf, x + i, y, "■", Style::new().fg(if i < filled { color } else { pal.grid }), 1);
    }
}

fn row(buf: &mut Buffer, inner: Rect, y: u16, name: &str, value: &str, color: Color, pal: &Palette) {
    let room = inner.width.saturating_sub(value.chars().count() as u16 + 1);
    text(buf, inner.x, y, &fit(name, room as usize), Style::new().fg(pal.label), room);
    right(buf, inner.right(), y, value, Style::new().fg(color).add_modifier(Modifier::BOLD));
}

const DIGITS: [[&str; 5]; 10] = [
    ["111", "101", "101", "101", "111"],
    ["010", "110", "010", "010", "010"],
    ["111", "001", "111", "100", "111"],
    ["111", "001", "111", "001", "111"],
    ["101", "101", "111", "001", "001"],
    ["111", "100", "111", "001", "111"],
    ["111", "100", "111", "101", "111"],
    ["111", "001", "010", "010", "010"],
    ["111", "101", "111", "101", "111"],
    ["111", "101", "111", "001", "111"],
];

fn big(buf: &mut Buffer, x: u16, y: u16, value: &str, style: Style) -> u16 {
    let mut x = x;
    for digit in value.chars().filter_map(|ch| ch.to_digit(10)) {
        let rows = DIGITS[digit as usize];
        for cell_row in 0..3 {
            let mut line = String::new();
            for col in 0..3 {
                let top = rows[cell_row * 2].as_bytes()[col] == b'1';
                let bottom = cell_row * 2 + 1 < 5 && rows[cell_row * 2 + 1].as_bytes()[col] == b'1';
                line.push(match (top, bottom) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    _ => ' ',
                });
            }
            text(buf, x, y + cell_row as u16, &line, style, 3);
        }
        x += 4;
    }
    x
}

fn put(buf: &mut Buffer, x: u16, y: u16, spans: Vec<Span<'static>>, max: u16) {
    if y < buf.area.bottom() && x < buf.area.right() {
        buf.set_line(x, y, &Line::from(spans), max.min(buf.area.right() - x));
    }
}

fn put_right(buf: &mut Buffer, right_x: u16, y: u16, spans: Vec<Span<'static>>) {
    let w = width(&spans);
    put(buf, right_x.saturating_sub(w), y, spans, w);
}

fn text(buf: &mut Buffer, x: u16, y: u16, value: &str, style: Style, max: u16) {
    put(buf, x, y, vec![Span::styled(value.to_string(), style)], max);
}

fn right(buf: &mut Buffer, right_x: u16, y: u16, value: &str, style: Style) {
    put_right(buf, right_x, y, vec![Span::styled(value.to_string(), style)]);
}

fn fill(buf: &mut Buffer, area: Rect, color: Color) {
    buf.set_style(area.intersection(buf.area), Style::new().bg(color));
}

fn clear(buf: &mut Buffer, area: Rect, pal: &Palette) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.reset();
            cell.set_bg(pal.panel);
        }
    }
}

fn width(spans: &[Span]) -> u16 {
    spans.iter().map(|span| span.width() as u16).sum()
}

fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn ms(now: Instant, then: Instant) -> f64 {
    now.saturating_duration_since(then).as_secs_f64() * 1000.0
}

fn reveal(app: &App, now: Instant, at: Option<Instant>, delay: f64, duration: f64) -> f64 {
    let Some(at) = at else { return 0.0 };
    if app.skipped {
        return 1.0;
    }
    ease_out_cubic((ms(now, at) - delay) / duration)
}

pub fn bytes(value: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < units.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 || amount >= 100.0 { format!("{amount:.0} {}", units[unit]) } else { format!("{amount:.1} {}", units[unit]) }
}

fn short_bytes(value: u64) -> String {
    let units = ["B", "K", "M", "G", "T"];
    let mut amount = value as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < units.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 || amount >= 10.0 { format!("{amount:.0}{}", units[unit]) } else { format!("{amount:.1}{}", units[unit]) }
}

fn versions(old: &str, new: &str, room: usize) -> String {
    let (old, new) = (short_version(old), short_version(new));
    if old == new {
        return fit(&format!("{old} rebuild"), room);
    }
    let full = format!("{old} → {new}");
    if full.chars().count() <= room {
        return full;
    }
    let shared = old.chars().rev().zip(new.chars().rev()).take_while(|(a, b)| a == b).map(|(ch, _)| ch.len_utf8()).sum::<usize>();
    let (old, new) = match old[old.len() - shared..].find(['.', '-', '+', '_', ':']) {
        Some(index) if shared - index < old.len() && shared - index < new.len() => (&old[..old.len() - shared + index], &new[..new.len() - shared + index]),
        _ => (old, new),
    };
    let short = format!("{old} → {new}");
    let keep = room.saturating_sub(new.chars().count() + 3);
    if short.chars().count() <= room || keep == 0 {
        return fit(&short, room);
    }
    format!("{} → {new}", fit(old, keep))
}

fn signed_bytes(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "+" };
    format!("{sign}{}", bytes(value.unsigned_abs()))
}

pub fn thousands(value: u64) -> String {
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

fn compact(value: u64) -> String {
    match value {
        0..10_000 => thousands(value),
        10_000..1_000_000 => format!("{}k", value / 1000),
        _ => format!("{}M", value / 1_000_000),
    }
}

pub fn short_version(version: &str) -> &str {
    match version.rsplit_once('-') {
        Some((head, tail)) if tail.chars().all(|ch| ch.is_ascii_digit() || ch == '.') => head,
        _ => version,
    }
}

fn short_time(iso: &str) -> String {
    iso.get(11..16).map(str::to_string).unwrap_or_else(|| iso.to_string())
}

fn epoch(iso: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| iso.get(range).and_then(|part| part.parse::<i64>().ok());
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let offset = match iso.get(19..) {
        Some(zone) if zone.len() >= 6 => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            sign * (zone.get(1..3)?.parse::<i64>().ok()? * 3600 + zone.get(4..6)?.parse::<i64>().ok()? * 60)
        }
        _ => 0,
    };
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * m + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146097 + day_of_era - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second - offset)
}

fn weighted(up: u8, down: u8, left: u8, right: u8) -> &'static str {
    const TABLE: [&str; 81] = [
        " ", "─", "━", "─", "─", "╼", "━", "╾", "━", "│", "╭", "┍", "╮", "┬", "┮", "┑", "┭", "┯", "┃", "┎", "┏", "┒", "┰", "┲", "┓", "┱", "┳", "│", "╰", "┕", "╯", "┴", "┶", "┙", "┵", "┷", "│", "├", "┝", "┤", "┼", "┾", "┥", "┽",
        "┿", "╽", "┟", "┢", "┧", "╁", "╆", "┪", "╅", "╈", "┃", "┖", "┗", "┚", "┸", "┺", "┛", "┹", "┻", "╿", "┞", "┡", "┦", "╀", "╄", "┩", "╃", "╇", "┃", "┠", "┣", "┨", "╂", "╊", "┫", "╉", "╋",
    ];
    TABLE[usize::from(up.min(2)) * 27 + usize::from(down.min(2)) * 9 + usize::from(left.min(2)) * 3 + usize::from(right.min(2))]
}

type Plan = (usize, u16, Vec<(u16, u16)>);

struct Wire {
    cells: Vec<(u16, u16)>,
    start: u8,
    end: u8,
    target: (u16, u16),
    connector: Option<((u16, u16), &'static str)>,
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn route(points: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut cells: Vec<(u16, u16)> = Vec::new();
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let steps = x0.abs_diff(x1).max(y0.abs_diff(y1));
        for k in 0..=steps {
            let x = if x1 >= x0 { x0 + k.min(x1 - x0) } else { x0 - k.min(x0 - x1) };
            let y = if y1 >= y0 { y0 + k.min(y1 - y0) } else { y0 - k.min(y0 - y1) };
            if cells.last() != Some(&(x, y)) {
                cells.push((x, y));
            }
        }
    }
    cells
}

fn toward(from: (u16, u16), to: (u16, u16)) -> u8 {
    if to.1 < from.1 {
        UP
    } else if to.1 > from.1 {
        DOWN
    } else if to.0 < from.0 {
        LEFT
    } else {
        RIGHT
    }
}

fn light(mask: u8) -> &'static str {
    match mask {
        3 => "│",
        12 => "─",
        10 => "╭",
        6 => "╮",
        9 => "╰",
        5 => "╯",
        11 => "├",
        7 => "┤",
        14 => "┬",
        13 => "┴",
        15 => "┼",
        1 | 2 => "│",
        _ => "─",
    }
}

fn heavy(mask: u8) -> &'static str {
    match mask {
        3 => "┃",
        12 => "━",
        10 => "┏",
        6 => "┓",
        9 => "┗",
        5 => "┛",
        11 => "┣",
        7 => "┫",
        14 => "┳",
        13 => "┻",
        15 => "╋",
        1 | 2 => "┃",
        _ => "━",
    }
}

fn local_mask(wire: &Wire, index: usize) -> u8 {
    let cells = &wire.cells;
    let mut mask = 0;
    if index > 0 {
        mask |= toward(cells[index], cells[index - 1]);
    } else {
        mask |= wire.start;
    }
    if index + 1 < cells.len() {
        mask |= toward(cells[index], cells[index + 1]);
    } else {
        mask |= wire.end;
    }
    mask
}

fn eye_log(app: &App) -> (Vec<(String, String, usize)>, bool) {
    let mut lines: Vec<(String, String)> = Vec::new();
    let kind = |name: &str| app.problems.iter().filter(|problem| problem.kind == name).map(|problem| problem.count).sum::<u64>();
    if let Some(last) = &app.last_upgrade {
        lines.push(("› pacman.log".into(), crate::motion::upgrade_tail(Some(last))));
    }
    if app.stage_done_at[0].is_some() {
        let aur = app.updates.iter().filter(|update| update.source == "aur").count();
        lines.push(("› checkupdates".into(), format!("{} updates", apos((app.updates.len() - aur) as u64))));
        lines.push(("› yay -Qua".into(), format!("{aur} aur")));
    }
    if app.stage_done_at[1].is_some() {
        lines.push(("› coredumpctl list".into(), format!("{} crashes", apos(kind("crash")))));
        lines.push(("› journalctl -p err".into(), format!("{} errors", apos(kind("journal")))));
        lines.push(("› systemctl --failed".into(), format!("{} units", kind("failed-unit"))));
        if let Some(errors) = app.health.config_errors {
            lines.push(("› hyprctl configerrors".into(), errors.to_string()));
        }
    }
    if let Some(kernel) = &app.health.kernel {
        lines.push(("› uname -r".into(), kernel.running.clone()));
    }
    if let Some((loaded, _)) = &app.health.nvidia {
        lines.push(("› /sys/module/nvidia/version".into(), loaded.clone()));
    }
    if let Some((running, _)) = &app.health.hyprland {
        lines.push(("› hyprctl version".into(), format!("v{running}")));
    }
    if let Some(anatomy) = &app.anatomy {
        lines.push(("› pacman -Q".into(), format!("{} · {} orphans", apos(anatomy.packages), anatomy.orphans)));
        if let Some(freeable) = anatomy.cache_freeable_bytes {
            lines.push(("› paccache -dk1".into(), format!("{} freeable", bytes(freeable))));
        }
    }
    if app.stage_done_at[2].is_some() {
        let news = if app.news_failed { "failed".to_string() } else if app.news.is_empty() { "none new".to_string() } else { format!("{} new", app.news.len()) };
        lines.push(("› curl archlinux.org/feeds/news".into(), news));
    }
    if let Some(preflight) = &app.preflight {
        lines.push(("› df /".into(), format!("{} free", bytes(preflight.free_bytes))));
    }
    let mut typing = false;
    if app.finished_at.is_none() {
        if let Some(step) = app.steps.last().filter(|step| step.ended.is_none()) {
            lines.push((format!("› {}", crate::motion::command_of(&step.label)), String::new()));
            typing = true;
        }
    } else {
        lines.push(("› journalctl -f".into(), "following".into()));
        let events = app.visible_events();
        for event in events.iter().take(app.journal_live.min(4)).rev() {
            lines.push((fit(&event.ident, 14), event.message.clone()));
        }
    }
    (lines.into_iter().map(|(head, tail)| (head, tail, usize::MAX)).collect(), typing)
}

fn draw_lists(buf: &mut Buffer, rect: Rect, app: &App, pal: &Palette, now: Instant, scene: &mut Scene) -> Vec<(Pane, Rect)> {
    let panes: Vec<Pane> = Pane::ALL.into_iter().filter(|pane| app.visible[pane.panel()]).collect();
    let mut rects = Vec::new();
    if panes.is_empty() || rect.height < 6 {
        return rects;
    }
    let width = rect.width / panes.len() as u16;
    for (index, pane) in panes.iter().enumerate() {
        let x = rect.x + index as u16 * width;
        let w = if index + 1 == panes.len() { rect.right() - x } else { width };
        let area = Rect::new(x, rect.y, w, rect.height);
        scene.rows[pane.index()] = match pane {
            Pane::Updates => draw_updates(buf, area, app, pal),
            Pane::Problems => draw_problems(buf, area, app, pal),
            Pane::Events => draw_events(buf, area, app, pal, now),
        };
        rects.push((*pane, area));
    }
    rects
}

fn band_rows(area: Rect) -> u16 {
    let by_height = area.height.saturating_sub(35).clamp(15, 20);
    let by_width = (f32::from(area.width.saturating_sub(116)) / 3.6) as u16;
    by_height.min(by_width.max(15))
}

#[allow(clippy::too_many_arguments)]
fn draw_nerve(buf: &mut Buffer, area: Rect, app: &App, motion: &Motion, pal: &Palette, now: Instant, cell: (u16, u16), scene: &mut Scene) {
    use std::collections::{HashMap, HashSet};
    draw_header(buf, Rect::new(area.x, area.y, area.width, 1), app, pal, now);
    let keys = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let band_visible = app.visible[1];
    let rows = band_rows(area);
    let eye_w = ((f32::from(rows) * 3.6).round() as u16).min(area.width.saturating_sub(4));
    let eye_x = area.x + (area.width - eye_w) / 2;
    let (cw, ch) = (f32::from(cell.0.max(1)), f32::from(cell.1.max(1)));
    let (w, h) = (f32::from(eye_w) * cw, f32::from(rows) * ch);
    let hw = (w * 0.47).min(h * 0.47 / 0.608) * 0.96;
    let span_left = eye_x + ((w / 2.0 - hw) / cw).ceil() as u16;
    let span_right = eye_x + ((w / 2.0 + hw) / cw).floor() as u16;
    let eye_mid = eye_x + eye_w / 2;
    let inner = |x: u16| x >= span_left + 6 && x + 6 <= span_right;

    let tiles = app.visible[0].then(|| Rect::new(area.x, area.y + 1, area.width, TILES_HEIGHT));
    let tile_w = area.width.saturating_sub(2) / 6;
    let tile_rect = |index: u16| {
        let x = area.x + 1 + index * tile_w;
        let w = if index == 5 { area.right() - 1 - x } else { tile_w };
        Rect::new(x, area.y + 1, w, TILES_HEIGHT)
    };
    let centres: Vec<u16> = (0..6u16).map(|index| tile_rect(index).x + tile_rect(index).width / 2).collect();
    let straight: Vec<usize> = (0..6).filter(|index| inner(centres[*index])).collect();
    let low = straight.iter().map(|index| centres[*index]).min().unwrap_or(eye_mid);
    let high = straight.iter().map(|index| centres[*index]).max().unwrap_or(eye_mid);
    let left_ranks = (0..6).filter(|index| centres[*index] < low).count() as u16;
    let right_ranks = (0..6).filter(|index| centres[*index] > high).count() as u16;
    let gutter = if tiles.is_some() && band_visible { left_ranks.max(right_ranks).max(2) } else { 0 };

    let waiting = motion.enabled() && app.finished_at.is_none();
    let delivered = |target: Target| match target {
        Target::Tile(0) | Target::List(0) => app.stage_done_at[0].is_some(),
        Target::Tile(1 | 3) | Target::List(1) => app.stage_done_at[1].is_some(),
        Target::Tile(2) => app.start_at.is_some(),
        Target::Tile(4) | Target::List(2) => app.anatomy.is_some(),
        Target::Tile(_) => app.stage_done_at[2].is_some(),
        _ => true,
    };
    let held = |target: Target| motion.held(target, now) || (waiting && !delivered(target));
    let mut y = area.y + 1;
    if let Some(tiles) = tiles {
        draw_tiles(buf, tiles, app, pal, now);
        for index in 0..6u16 {
            if held(Target::Tile(index as u8)) {
                let rect = tile_rect(index);
                for row in rect.y + 2..rect.bottom() - 1 {
                    for x in rect.x + 1..rect.right() - 1 {
                        buf[(x, row)].set_symbol(" ");
                    }
                }
                text(buf, rect.x + 2, rect.y + 4, "checking…", Style::new().fg(pal.faint), rect.width.saturating_sub(4));
            }
        }
        y = tiles.bottom() + gutter;
    }
    let band = Rect::new(area.x, y, area.width, if band_visible { rows } else { 0 });
    let lists_top = if band_visible { band.bottom() + 1 } else { y };
    let lists_area = Rect::new(area.x, lists_top, area.width, keys.y.saturating_sub(lists_top));
    let lists = draw_lists(buf, lists_area, app, pal, now, scene);
    for (pane, rect) in &lists {
        if held(Target::List(pane.index() as u8)) {
            for row in rect.y + 2..rect.bottom().saturating_sub(2) {
                for x in rect.x + 1..rect.right() - 1 {
                    buf[(x, row)].set_symbol(" ").set_bg(pal.page);
                }
            }
            text(buf, rect.x + 2, rect.y + 3, "checking…", Style::new().fg(pal.faint), rect.width.saturating_sub(4));
        }
    }
    draw_keys(buf, keys, app, pal, now);
    if !band_visible {
        return;
    }

    let eye = Rect::new(eye_x, band.y, eye_w, band.height);
    let mut wires: Vec<(Target, Wire)> = Vec::new();
    let mut anchors_up: Vec<(Target, u16)> = Vec::new();
    let mut anchors_down: Vec<(Target, u16)> = Vec::new();
    if let Some(tiles) = tiles {
        let border_y = tiles.bottom() - 1;
        let top_cell = band.y - 1;
        let mut plans: Vec<Plan> = Vec::new();
        for (rank, index) in (0..6usize).rev().filter(|index| centres[*index] < low).enumerate() {
            let rank = rank as u16;
            let anchor = low.saturating_sub(3 * (rank + 1));
            let lane = tiles.bottom() + rank;
            plans.push((index, anchor, route(&[(anchor, top_cell), (anchor, lane), (centres[index], lane), (centres[index], border_y + 1)])));
        }
        for (rank, index) in (0..6usize).filter(|index| centres[*index] > high).enumerate() {
            let rank = rank as u16;
            let anchor = high + 3 * (rank + 1);
            let lane = tiles.bottom() + rank;
            plans.push((index, anchor, route(&[(anchor, top_cell), (anchor, lane), (centres[index], lane), (centres[index], border_y + 1)])));
        }
        for index in &straight {
            let x = centres[*index];
            plans.push((*index, x, route(&[(x, top_cell), (x, border_y + 1)])));
        }
        plans.sort_by_key(|(index, _, _)| *index);
        for (index, anchor, cells) in plans {
            let x = centres[index];
            let target = Target::Tile(index as u8);
            wires.push((target, Wire { cells, start: DOWN, end: UP, target: (x, border_y), connector: Some(((x, border_y), "┬")) }));
            anchors_up.push((target, anchor));
        }
    }

    let bottom_cell = band.bottom();
    let list_centres: Vec<(Pane, u16)> = lists.iter().map(|(pane, rect)| (*pane, rect.x + rect.width / 2)).collect();
    let middle: Vec<usize> = (0..list_centres.len()).filter(|index| inner(list_centres[*index].1)).collect();
    let low = middle.iter().map(|index| list_centres[*index].1).min().unwrap_or(eye_mid);
    let high = middle.iter().map(|index| list_centres[*index].1).max().unwrap_or(eye_mid);
    let mut plans: Vec<Plan> = Vec::new();
    for (rank, index) in (0..list_centres.len()).rev().filter(|index| list_centres[*index].1 < low).enumerate() {
        let anchor = low.saturating_sub(5 * (rank as u16 + 1));
        plans.push((index, anchor, route(&[(anchor, bottom_cell), (list_centres[index].1, bottom_cell)])));
    }
    for (rank, index) in (0..list_centres.len()).filter(|index| list_centres[*index].1 > high).enumerate() {
        let anchor = high + 5 * (rank as u16 + 1);
        plans.push((index, anchor, route(&[(anchor, bottom_cell), (list_centres[index].1, bottom_cell)])));
    }
    for index in &middle {
        let x = list_centres[*index].1;
        plans.push((*index, x, vec![(x, bottom_cell)]));
    }
    plans.sort_by_key(|(index, _, _)| *index);
    for (index, anchor, cells) in plans {
        let (pane, x) = list_centres[index];
        let target = Target::List(pane.index() as u8);
        wires.push((target, Wire { cells, start: UP, end: DOWN, target: (x, lists_area.y), connector: Some(((x, lists_area.y), "┴")) }));
        anchors_down.push((target, anchor));
    }

    let parts = subsystem_parts(app);
    let canthus_y = band.y + band.height / 2;
    let left_trunk = eye.x.saturating_sub(4);
    let right_trunk = eye.right() + 3;
    let tint = |kind: u8| match kind {
        2 => pal.alert,
        1 => pal.teal,
        _ => pal.ink,
    };
    let mut outer: Vec<(Target, Wire)> = Vec::new();
    let mut icon_slots: Vec<usize> = Vec::new();
    let mut texts: Vec<(u16, u16, Vec<Span<'static>>)> = Vec::new();
    let counted = app.stage_done_at[0].is_some();
    for (k, part) in parts.iter().enumerate() {
        let (subsystem, updates, high, problems) = *part;
        let (main, second, second_kind) = if counted || (k < 3 && part_known(app, k)) { part_lines(app, k, *part) } else { ("checking…".to_string(), String::new(), 0) };
        let signal = problems > 0;
        let (glow_kind, glow_strength) = motion.glow(k as u8, now);
        let glowing = (glow_strength > 0.01).then(|| (tint(glow_kind), glow_strength));
        let (flash_kind, flash_strength) = motion.flash(k as u8, now);
        let mut head = vec![Span::styled(subsystem.label().to_string(), Style::new().fg(if signal { pal.teal } else { pal.ink }).add_modifier(Modifier::BOLD))];
        if counted {
            head.push(Span::styled(format!(" · {updates} upd · "), Style::new().fg(pal.label)));
            head.push(Span::styled(format!("{high} high"), Style::new().fg(if high > 0 { pal.alert } else { pal.faint })));
        }
        let badge = Span::styled(format!(" {problems} "), Style::new().fg(pal.page).bg(pal.teal).add_modifier(Modifier::BOLD));
        let room = 30usize;
        let main = fit(&main, room);
        let tone = if main == "checking…" { pal.faint } else { mix(pal.ink, tint(flash_kind), f64::from(flash_strength)) };
        let mut fact_line = vec![Span::styled(main.clone(), Style::new().fg(tone))];
        if !second.is_empty() && main.chars().count() + 3 + second.chars().count() <= room {
            fact_line.push(Span::styled(" · ".to_string(), Style::new().fg(pal.faint)));
            fact_line.push(Span::styled(second, Style::new().fg(match second_kind {
                2 => pal.alert,
                1 => pal.warn,
                _ => pal.label,
            })));
        }
        if k < 4 {
            let y = band.y + k as u16 * (band.height - 3) / 3;
            let icon_x = left_trunk.saturating_sub(10);
            icon_slots.push(scene.placements.len());
            scene.placements.push(Placement { rect: Rect::new(icon_x, y, 8, 3), picture: Picture::Specimen { subsystem, signal, glow: glowing, aim: 0.0 } });
            if signal {
                head.push(Span::raw(" "));
                head.push(badge);
            }
            let right_edge = icon_x.saturating_sub(3);
            for (row, spans) in [(y, head), (y + 2, fact_line)] {
                let w = width(&spans);
                texts.push((right_edge.saturating_sub(w).max(area.x), row, spans));
            }
            let pad = right_edge.saturating_sub(room as u16 + 3).max(area.x);
            let cells = route(&[(eye.x - 1, canthus_y), (left_trunk, canthus_y), (left_trunk, y + 1), (icon_x + 8, y + 1)]);
            wires.push((Target::Part(k as u8), Wire { cells, start: RIGHT, end: LEFT, target: (icon_x + 4, y + 1), connector: None }));
            outer.push((Target::Edge(k as u8), Wire { cells: route(&[(icon_x - 1, y + 1), (pad + 1, y + 1)]), start: RIGHT, end: LEFT, target: (pad, y + 1), connector: Some(((pad, y + 1), "■")) }));
        } else {
            let slot = k as u16 - 4;
            let y = band.y + 1 + slot * (band.height - 5) / 2;
            let icon_x = right_trunk + 2;
            icon_slots.push(scene.placements.len());
            scene.placements.push(Placement { rect: Rect::new(icon_x, y, 8, 3), picture: Picture::Specimen { subsystem, signal, glow: glowing, aim: 0.0 } });
            if signal {
                head.insert(0, Span::raw(" "));
                head.insert(0, badge);
            }
            let left_edge = icon_x + 8 + 3;
            for (row, spans) in [(y, head), (y + 2, fact_line)] {
                texts.push((left_edge, row, spans));
            }
            let pad = (left_edge + room as u16 + 2).min(area.right() - 1);
            let cells = route(&[(eye.right(), canthus_y), (right_trunk, canthus_y), (right_trunk, y + 1), (icon_x - 1, y + 1)]);
            wires.push((Target::Part(k as u8), Wire { cells, start: LEFT, end: RIGHT, target: (icon_x + 4, y + 1), connector: None }));
            outer.push((Target::Edge(k as u8), Wire { cells: route(&[(icon_x + 8, y + 1), (pad - 1, y + 1)]), start: LEFT, end: RIGHT, target: (pad, y + 1), connector: Some(((pad, y + 1), "■")) }));
        }
    }
    wires.extend(outer);
    let wire_of = |target: Target| wires.iter().position(|(owner, _)| *owner == target);

    let signals: Vec<(usize, Signal)> = motion.signals(now).into_iter().filter_map(|signal| wire_of(signal.target).map(|index| (index, signal))).collect();
    let busy = |index: usize| signals.iter().any(|(link, _)| *link == index);
    let chosen = motion
        .look(now)
        .or_else(|| signals.iter().find(|(_, signal)| signal.inbound).or(signals.first()).map(|(_, signal)| signal.target.folded()))
        .filter(|target| wire_of(*target).is_some());
    let gaze = chosen.and_then(wire_of).map_or((0.0, 0.0), |index| {
        let wire = &wires[index].1;
        let (dx, dy) = ((f32::from(wire.target.0) - f32::from(eye_mid)) * cw, (f32::from(wire.target.1) - f32::from(canthus_y)) * ch);
        let length = (dx * dx + dy * dy).sqrt().max(1.0);
        (dx / length * 0.85, dy / length * 0.85)
    });
    let gaze = if motion.enabled() { motion.gaze(now, gaze) } else { gaze };
    let (log, typing, scroll) = if motion.enabled() {
        motion.pupil(now)
    } else {
        let (log, typing) = eye_log(app);
        (log, typing, 0.0)
    };
    let mut spec = fibers_spec(app);
    spec.gaze = gaze;
    spec.code = log;
    spec.typing = typing;
    spec.scroll = scroll;
    spec.columns = eye.width;
    spec.up = anchors_up.iter().map(|(target, x)| (x - eye.x, wire_of(*target).is_some_and(busy))).collect();
    spec.down = anchors_down.iter().map(|(target, x)| (x - eye.x, wire_of(*target).is_some_and(busy))).collect();
    let side_busy = |range: std::ops::Range<u8>| range.into_iter().any(|k| wire_of(Target::Part(k)).is_some_and(busy));
    spec.sides = Some((side_busy(0..4), side_busy(4..7)));

    let (current, since, previous, left_at) = motion.sight(now, chosen);
    let seconds = |from: Instant| now.saturating_duration_since(from).as_secs_f32();
    let ease = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    let lock = ease((seconds(since) - 0.32) / 0.14);
    let reaches: Vec<(usize, f32)> = [(previous, 1.0 - ease(seconds(left_at) / 0.18)), (current, ease(seconds(since) / 0.32))]
        .into_iter()
        .filter_map(|(target, reach)| target.filter(|target| !matches!(target, Target::Edge(_))).and_then(wire_of).filter(|_| reach > 0.0).map(|index| (index, reach)))
        .collect();
    let mut sight_bits: HashMap<(u16, u16), u8> = HashMap::new();
    let mut sight_done: Vec<usize> = Vec::new();
    for (index, reach) in &reaches {
        let (target, wire) = &wires[*index];
        let anchor = |anchors: &[(Target, u16)]| anchors.iter().find(|(owner, _)| owner == target).map(|(_, x)| x - eye.x).unwrap_or(0);
        let (side, column) = match target {
            Target::Tile(_) => (0u8, anchor(&anchors_up)),
            Target::List(_) => (1, anchor(&anchors_down)),
            Target::Part(k) if *k < 4 => (2, 0),
            _ => (3, 0),
        };
        spec.sight.push((side, column, (reach / 0.15).clamp(0.0, 1.0)));
        let len = wire.cells.len();
        let count = ((((reach - 0.15) / 0.85).clamp(0.0, 1.0)) * len as f32).ceil() as usize;
        for i in 0..count {
            let mut bits = if i > 0 { toward(wire.cells[i], wire.cells[i - 1]) } else { wire.start };
            if i + 1 < count {
                bits |= toward(wire.cells[i], wire.cells[i + 1]);
            } else if i + 1 == len {
                bits |= wire.end;
            }
            *sight_bits.entry(wire.cells[i]).or_insert(0) |= bits;
        }
        if count == len && *reach >= 1.0 {
            sight_done.push(*index);
        }
    }
    if let Some(Target::Part(k)) = current.filter(|_| lock > 0.0)
        && let Some(Placement { picture: Picture::Specimen { aim, .. }, .. }) = icon_slots.get(usize::from(k)).and_then(|slot| scene.placements.get_mut(*slot))
    {
        *aim = lock;
    }
    spec.turn = motion.turn();
    spec.rain = motion.rain(now).map(|rain| crate::ink::Rain {
        clock: rain.clock,
        level: rain.level,
        text: rain.text,
        before: rain.before,
        feed: rain.feed,
        previous: rain.previous,
        switch: rain.switch,
        tint: rain.tint.map(|(kind, strength)| (if kind == 2 { pal.alert } else { pal.teal }, strength)),
    });
    scene.placements.push(Placement { rect: eye, picture: Picture::Eye(Box::new(spec)) });

    let mut masks: HashMap<(u16, u16), u8> = HashMap::new();
    let mut active: HashSet<(u16, u16)> = HashSet::new();
    for (index, (_, wire)) in wires.iter().enumerate() {
        for k in 0..wire.cells.len() {
            *masks.entry(wire.cells[k]).or_insert(0) |= local_mask(wire, k);
            if busy(index) {
                active.insert(wire.cells[k]);
            }
        }
    }
    for (cell, mask) in &masks {
        if cell.0 < buf.area.right() && cell.1 < buf.area.bottom() {
            buf[*cell].set_symbol(light(*mask)).set_fg(if active.contains(cell) { pal.rule } else { pal.grid });
        }
    }
    let lit_by = |index: usize| signals.iter().find(|(link, signal)| *link == index && if signal.inbound { signal.progress > 0.75 } else { signal.progress > 0.9 }).map(|(_, signal)| signal.kind);
    for (index, (_, wire)) in wires.iter().enumerate() {
        if let Some((cell, symbol)) = wire.connector {
            let color = match lit_by(index) {
                Some(2) => pal.alert,
                Some(1) => pal.teal,
                Some(_) => pal.ink,
                None if symbol == "■" => pal.label,
                None => pal.rule,
            };
            buf[cell].set_symbol(symbol).set_fg(color);
        }
    }
    let sight_color = pal.ink2;
    for (cell, bits) in &sight_bits {
        if cell.0 >= buf.area.right() || cell.1 >= buf.area.bottom() {
            continue;
        }
        let mask = masks.get(cell).copied().unwrap_or(0) | bits;
        let weight = |bit: u8| if bits & bit != 0 { 2 } else { u8::from(mask & bit != 0) };
        buf[*cell].set_symbol(weighted(weight(UP), weight(DOWN), weight(LEFT), weight(RIGHT))).set_fg(sight_color);
    }
    for index in &sight_done {
        if let Some((cell, symbol)) = wires[*index].1.connector {
            let heavy_symbol = match symbol {
                "┬" => weighted(0, 2, 1, 1),
                "┴" => weighted(2, 0, 1, 1),
                other => other,
            };
            buf[cell].set_symbol(heavy_symbol);
            if lit_by(*index).is_none() {
                buf[cell].set_fg(sight_color);
            }
        }
    }
    let target_rect = match current.filter(|_| lock > 0.0) {
        Some(Target::Tile(index)) if tiles.is_some() => Some(tile_rect(u16::from(index))),
        Some(Target::List(pane)) => lists.iter().find(|(owner, _)| owner.index() == usize::from(pane)).map(|(_, rect)| *rect),
        _ => None,
    };
    if let Some(rect) = target_rect.filter(|rect| rect.width >= 6 && rect.height >= 4) {
        let mut mark = |x: u16, y: u16, symbol: &str| {
            if x < buf.area.right() && y < buf.area.bottom() {
                let base = buf[(x, y)].fg;
                buf[(x, y)].set_symbol(symbol).set_fg(mix(base, pal.ink, f64::from(lock)));
            }
        };
        let (left, right, top, bottom) = (rect.x, rect.right() - 1, rect.y, rect.bottom() - 1);
        for (x, y, corner, inward, down) in [(left, top, "┏", true, true), (right, top, "┓", false, true), (left, bottom, "┗", true, false), (right, bottom, "┛", false, false)] {
            mark(x, y, corner);
            for step in 1..=2u16 {
                mark(if inward { x + step } else { x - step }, y, "━");
            }
            mark(x, if down { y + 1 } else { y - 1 }, "┃");
        }
    }
    for (index, signal) in &signals {
        let wire = &wires[*index].1;
        let len = wire.cells.len();
        if len == 0 {
            continue;
        }
        let head = (signal.progress.clamp(0.0, 1.0) * (len - 1) as f32).round() as usize;
        let color = tint(signal.kind);
        for step in 1..=4usize {
            let at = if signal.inbound { head + step } else { head.wrapping_sub(step) };
            if at >= len {
                continue;
            }
            let fade = [0.25, 0.5, 0.7, 0.85][step - 1];
            let rest = if sight_bits.contains_key(&wire.cells[at]) { sight_color } else { pal.rule };
            buf[wire.cells[at]].set_symbol(heavy(local_mask(wire, at))).set_fg(mix(color, rest, fade));
        }
        buf[wire.cells[head]].set_symbol("●").set_fg(color);
    }
    for (x, y, spans) in texts {
        put(buf, x, y, spans, area.right().saturating_sub(x));
    }
}

fn part_known(app: &App, index: usize) -> bool {
    match index {
        0 => app.health.kernel.is_some(),
        1 => app.health.nvidia.is_some(),
        _ => app.health.hyprland.is_some(),
    }
}

fn part_lines(app: &App, index: usize, part: (Subsystem, usize, usize, usize)) -> (String, String, u8) {
    let release = |version: &str| version.rsplit_once('-').map_or(version, |(version, _)| version).to_string();
    let pair = |version: &str| version.split('.').take(2).collect::<Vec<_>>().join(".");
    match index {
        0 => {
            if let Some(kernel) = &app.health.kernel {
                let uptime = kernel.uptime_seconds.map_or_else(String::new, |seconds| if seconds >= 86_400 { format!("up {} d", seconds / 86_400) } else { format!("up {} h", seconds / 3600) });
                if kernel.reboot_pending {
                    return (format!("{} running", kernel.running), "reboot".into(), 2);
                }
                let installed = if kernel.installed.is_empty() { kernel.running.clone() } else { kernel.installed.clone() };
                return (format!("{installed} running ✓"), uptime, 0);
            }
        }
        1 => {
            if let Some((loaded, installed)) = &app.health.nvidia {
                let installed = release(installed);
                if installed.is_empty() || installed == *loaded {
                    let pending = app.updates.iter().find(|update| update.name == "nvidia-utils").map(|update| (format!("{} pending", pair(&update.new)), if update.risk == "high" { 2 } else { 1 }));
                    let (second, kind) = pending.unwrap_or_default();
                    return (format!("nvidia {} loaded ✓", pair(loaded)), second, kind);
                }
                return (format!("nvidia {} ≠ {}", pair(loaded), pair(&installed)), "reboot".into(), 2);
            }
        }
        2 => {
            if let Some((running, installed)) = &app.health.hyprland {
                let installed = release(installed);
                if installed.is_empty() || installed == *running {
                    let errors = app.health.config_errors.map_or_else(String::new, |errors| format!("{errors} config errors"));
                    let kind = u8::from(app.health.config_errors.is_some_and(|errors| errors > 0));
                    return (format!("Hyprland {running} running ✓"), errors, kind);
                }
                return (format!("Hyprland {running} ≠ {installed}"), "relogin".into(), 2);
            }
        }
        _ => {}
    }
    let (subsystem, _, _, problems) = part;
    if problems > 0
        && let Some(problem) = app.problems.iter().filter(|problem| !problem.package.is_empty() && Subsystem::of(&problem.package) == subsystem).max_by_key(|problem| problem.count)
    {
        let kind = if problem.kind == "crash" { "crashes" } else { "errors" };
        return (format!("{} {} {kind}", problem.name(), apos(problem.count)), if problems == 1 { "1 problem".to_string() } else { format!("{problems} problems") }, 0);
    }
    match app.updates.iter().filter(|update| Subsystem::of(&update.name) == subsystem).min_by_key(|update| (update.risk_rank(), update.name.clone())) {
        Some(update) => (format!("{} → {}", update.name, short_version(&update.new)), update.risk.clone(), if update.risk == "high" { 2 } else if update.risk == "medium" { 1 } else { 0 }),
        None => ("nothing pending".into(), String::new(), 0),
    }
}

pub fn apos(value: u64) -> String {
    thousands(value).replace(',', "'")
}

fn subsystem_parts(app: &App) -> Vec<(Subsystem, usize, usize, usize)> {
    Subsystem::ALL
        .into_iter()
        .map(|subsystem| {
            let (updates, high) = app.updates.iter().filter(|update| Subsystem::of(&update.name) == subsystem).fold((0, 0), |(count, high), update| (count + 1, high + usize::from(update.risk == "high")));
            let problems = app.problems.iter().filter(|problem| !problem.package.is_empty() && Subsystem::of(&problem.package) == subsystem).count();
            (subsystem, updates, high, problems)
        })
        .collect()
}

fn fibers_spec(app: &App) -> crate::ink::EyeSpec {
    use std::f32::consts::TAU;
    let group = |update: &crate::app::Update| Subsystem::ALL.iter().position(|s| *s == Subsystem::of(&update.name)).unwrap_or(Subsystem::ALL.len());
    let mut updates: Vec<&crate::app::Update> = if app.stage_done_at[0].is_some() { app.updates.iter().collect() } else { Vec::new() };
    updates.sort_by(|a, b| group(a).cmp(&group(b)).then(a.name.cmp(&b.name)));
    let mut groups: Vec<usize> = updates.iter().map(|update| group(update)).collect();
    groups.dedup();
    let gap = if groups.len() > 1 { (TAU * 0.2 / groups.len() as f32).min(0.07) } else { 0.0 };
    let step = (TAU - gap * groups.len() as f32) / updates.len().max(1) as f32;
    let fixes = |name: &str| app.stage_done_at[1].is_some() && app.problems.iter().any(|problem| problem.status == "possibly-fixed" && problem.package == name);
    let mut fibers = Vec::new();
    let mut sectors = Vec::new();
    let mut angle = -TAU / 4.0;
    for g in &groups {
        if groups.len() > 1 {
            sectors.push(angle + gap / 2.0);
            angle += gap;
        }
        for update in updates.iter().filter(|update| group(update) == *g) {
            fibers.push(crate::ink::Fiber { angle: angle + step / 2.0, risk: update.risk_rank(), fixes: fixes(&update.name) });
            angle += step;
        }
    }
    crate::ink::EyeSpec { fibers, sectors, ..crate::ink::EyeSpec::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_shorten_to_the_changed_part_and_sizes_stay_short() {
        assert_eq!(versions("610.57.04-1", "615.78.08-1", 30), "610.57.04 → 615.78.08");
        assert_eq!(versions("610.57.04-1", "615.78.08-1", 18), "610.5… → 615.78.08");
        assert_eq!(versions("6.17.2.arch1-1", "6.17.4.arch1-1", 18), "6.17.2 → 6.17.4");
        assert_eq!(versions("1:25.2.4-1", "1:25.2.4-2", 18), "1:25.2.4 rebuild");
        assert_eq!(versions("1:1.2.74-1", "1:1.2.75-1", 18), "1:1.2.… → 1:1.2.75");
        assert_eq!(versions("141.0.7390.54-1", "142.0.7444.59-1", 18), "1… → 142.0.7444.59");
        assert_eq!(versions("1.0ä-1", "2.0ö-1", 6).chars().count(), 6);
        assert_eq!(versions("a.b-1", "c.b-1", 4).chars().count(), 4);
        assert_eq!(short_bytes(517 * 1024), "517K");
        assert_eq!(short_bytes(6_815_744), "6.5M");
        assert_eq!(short_bytes(1023), "1023B");
        assert_eq!(compact(12_345), "12k");
    }
}
