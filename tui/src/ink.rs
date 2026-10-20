//! Ink drawings for the dashboard: the eye and small specimen plates.
//! Rendered with tiny-skia at the terminal's real pixel size and shown through
//! its image protocol (sixel in foot, kitty in ghostty).

use std::cell::RefCell;
use std::f32::consts::TAU;
use std::sync::Arc;

use ratatui::style::Color;
use tiny_skia::{Color as SkColor, FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

use crate::app::Subsystem;
use crate::palette::{Palette, mix};

fn sk(color: Color, alpha: f32) -> SkColor {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
    match color {
        Color::Rgb(r, g, b) => SkColor::from_rgba8(r, g, b, a),
        _ => SkColor::from_rgba8(0, 0, 0, a),
    }
}

/// Small drawing helper bound to one pixmap; `mask` clips everything drawn.
struct Pen<'a> {
    pixmap: &'a mut Pixmap,
    unit: f32,
    mask: Option<Mask>,
}

impl Pen<'_> {
    fn stroke(&mut self, builder: PathBuilder, color: SkColor, width: f32, dash: Option<(f32, f32)>) {
        let Some(path) = builder.finish() else { return };
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let mut stroke = Stroke { width: width * self.unit, line_cap: LineCap::Butt, ..Stroke::default() };
        if let Some((on, off)) = dash {
            stroke.dash = StrokeDash::new(vec![on * self.unit, off * self.unit], 0.0);
        }
        self.pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), self.mask.as_ref());
    }

    fn line(&mut self, from: (f32, f32), to: (f32, f32), color: SkColor, width: f32) {
        let mut builder = PathBuilder::new();
        builder.move_to(from.0, from.1);
        builder.line_to(to.0, to.1);
        self.stroke(builder, color, width, None);
    }

    fn dashed(&mut self, from: (f32, f32), to: (f32, f32), color: SkColor, width: f32, dash: (f32, f32)) {
        let mut builder = PathBuilder::new();
        builder.move_to(from.0, from.1);
        builder.line_to(to.0, to.1);
        self.stroke(builder, color, width, Some(dash));
    }

    fn polyline(&mut self, points: &[(f32, f32)], color: SkColor, width: f32) {
        let mut builder = PathBuilder::new();
        for (index, (x, y)) in points.iter().enumerate() {
            if index == 0 {
                builder.move_to(*x, *y);
            } else {
                builder.line_to(*x, *y);
            }
        }
        self.stroke(builder, color, width, None);
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, color: SkColor, width: f32, dash: Option<(f32, f32)>) {
        let mut builder = PathBuilder::new();
        builder.push_circle(x, y, radius);
        self.stroke(builder, color, width, dash);
    }

    fn disc(&mut self, x: f32, y: f32, radius: f32, color: SkColor) {
        if let Some(path) = PathBuilder::from_circle(x, y, radius) {
            let mut paint = Paint::default();
            paint.set_color(color);
            paint.anti_alias = true;
            self.pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), self.mask.as_ref());
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: SkColor, width: f32) {
        let mut builder = PathBuilder::new();
        builder.move_to(x, y);
        builder.line_to(x + w, y);
        builder.line_to(x + w, y + h);
        builder.line_to(x, y + h);
        builder.close();
        self.stroke(builder, color, width, None);
    }

    fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: SkColor) {
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
            let mut paint = Paint::default();
            paint.set_color(color);
            self.pixmap.fill_rect(rect, &paint, Transform::identity(), self.mask.as_ref());
        }
    }

    /// Partial ellipse from `phase`, covering `reveal` of a full turn.
    #[allow(clippy::too_many_arguments)]
    fn ellipse(&mut self, c: (f32, f32), rx: f32, ry: f32, rotation: f32, phase: f32, reveal: f32, color: SkColor, width: f32, dash: Option<(f32, f32)>) {
        let segments = 240;
        let (sin, cos) = rotation.sin_cos();
        let mut builder = PathBuilder::new();
        for s in 0..=((segments as f32 * reveal) as usize) {
            let angle = phase + s as f32 * TAU / segments as f32;
            let (px, py) = (rx * angle.cos(), ry * angle.sin());
            let (x, y) = (c.0 + px * cos - py * sin, c.1 + px * sin + py * cos);
            if s == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        self.stroke(builder, color, width, dash);
    }
}

#[derive(Debug, Hash)]
pub struct FanSpec {
    pub left: Vec<u8>,
    pub right: Vec<u8>,
    pub panel: bool,
}

pub const DIAL: f32 = 0.84;

#[derive(Debug, Hash)]
pub struct OrbitNode {
    pub ring: u8,
    pub sector: u8,
    pub slot: u16,
    pub count: u16,
    pub fixes: bool,
    pub selected: bool,
    pub related: bool,
}

#[derive(Debug, Hash)]
pub struct OrbitLink {
    pub left: bool,
    pub row: u16,
    pub kind: u8,
}

#[derive(Debug, Hash)]
pub struct OrbitSpec {
    pub nodes: Vec<OrbitNode>,
    pub links: Vec<OrbitLink>,
    pub sectors: Vec<(String, u16)>,
}

fn sector_spans(sectors: &[(String, u16)]) -> Vec<(f32, f32)> {
    let total: f32 = sectors.iter().map(|(_, count)| f32::from(*count)).sum::<f32>().max(1.0);
    let shown = sectors.iter().filter(|(_, count)| *count > 0).count().max(1) as f32;
    let floor = 0.045f32;
    let free = (1.0 - floor * shown).max(0.0);
    let share = |count: u16| if count == 0 { 0.0 } else { floor + free * f32::from(count) / total };
    let last = sectors.last().map_or(0.0, |(_, count)| share(*count) * TAU);
    let mut start = TAU / 4.0 + last / 2.0;
    sectors
        .iter()
        .map(|(_, count)| {
            let span = share(*count) * TAU;
            let range = (start, span);
            start += span;
            range
        })
        .collect()
}

fn ring_position(node: &OrbitNode, spans: &[(f32, f32)]) -> (f32, f32) {
    let (start, span) = spans.get(usize::from(node.sector)).copied().unwrap_or((0.0, TAU));
    let gap = 0.035f32.min(span * 0.2);
    let (start, span) = (start + gap, (span - 2.0 * gap).max(0.001));
    let count = f32::from(node.count.max(1));
    let slot = f32::from(node.slot);
    let (angle, radius) = match node.ring {
        0 => (start + (slot + 0.5) / count * span, 0.3),
        1 => (start + (slot + 0.5) / count * span, 0.52),
        _ => {
            let capacity = (span * 0.79 / 0.022).max(1.0);
            let rows = (count / capacity).ceil().max(1.0);
            let row = (slot % rows).floor();
            let column = (slot / rows).floor();
            let columns = (count / rows).ceil().max(1.0);
            (start + (column + 0.5) / columns * span, 0.7 + (row + 0.5) / rows * 0.18)
        }
    };
    (angle.cos() * radius, angle.sin() * radius)
}

pub fn orbit(spec: &OrbitSpec, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let ground = pal.page;
    let (mut pixmap, unit) = canvas(width, height, ground, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let c = (w / 2.0, h / 2.0);
    let r = w.min(h) / 2.0 * DIAL;
    let at = |x: f32, y: f32| (c.0 + x * r, c.1 + y * r);
    let ink = |alpha: f32| sk(pal.ink, alpha);
    let spans = sector_spans(&spec.sectors);
    {
        let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
        for i in 0..170u32 {
            let angle = noise(i * 3) * TAU;
            let distance = 0.45 + noise(i * 3 + 1) * 0.75;
            let (x, y) = at(angle.cos() * distance * 1.25, angle.sin() * distance * 0.8);
            pen.disc(x, y, (0.5 + noise(i * 3 + 2) * 1.4) * unit, ink(0.12 + noise(i * 7) * 0.45));
        }
        for radius in [0.3f32, 0.52, 0.7, 0.88] {
            pen.circle(c.0, c.1, r * radius, ink(0.12), 0.7, Some((2.0, 3.0)));
        }
        for k in 0..24 {
            let angle = k as f32 * TAU / 24.0;
            let (sin, cos) = angle.sin_cos();
            pen.line((c.0 + cos * r * 0.18, c.1 + sin * r * 0.18), (c.0 + cos * r * 0.22, c.1 + sin * r * 0.22), ink(0.3), 0.6);
        }
        pen.circle(c.0, c.1, r, ink(0.85), 1.0, None);
        pen.circle(c.0, c.1, r * 1.035, ink(0.85), 1.0, None);
        pen.circle(c.0, c.1, r * 0.93, ink(0.3), 0.7, None);
        for k in 0..180 {
            let angle = -TAU / 4.0 + k as f32 * TAU / 180.0;
            let (sin, cos) = angle.sin_cos();
            let (from, to, alpha) = if k % 15 == 0 {
                (0.93, 1.08, 0.9)
            } else if k % 5 == 0 {
                (1.0, 1.06, 0.75)
            } else {
                (1.0, 1.035, 0.55)
            };
            pen.line((c.0 + cos * r * from, c.1 + sin * r * from), (c.0 + cos * r * to, c.1 + sin * r * to), ink(alpha), 0.7);
        }
        for (dx, dy) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)] {
            let (x, y) = (c.0 + dx * r * 1.15, c.1 + dy * r * 1.15);
            let arm = 5.0 * unit;
            pen.line((x - arm, y), (x + arm, y), ink(0.7), 0.8);
            pen.line((x, y - arm), (x, y + arm), ink(0.7), 0.8);
        }
        for (start, span) in spans.iter().filter(|(_, span)| *span > 0.0) {
            for angle in [*start, start + span] {
                let (sin, cos) = angle.sin_cos();
                pen.line((c.0 + cos * r * 0.24, c.1 + sin * r * 0.24), (c.0 + cos * r * 0.93, c.1 + sin * r * 0.93), ink(0.22), 0.7);
            }
        }
        let core = r * 0.07;
        for k in -6..=6 {
            let offset = k as f32 / 7.0 * core;
            let half = (core * core - offset * offset).max(0.0).sqrt();
            pen.line((c.0 - half + offset * 0.4, c.1 + offset), (c.0 + half + offset * 0.4, c.1 + offset), ink(0.55), 0.6);
        }
        pen.circle(c.0, c.1, core, ink(0.9), 1.0, None);
        let place = |node: &OrbitNode| ring_position(node, &spans);
        for node in spec.nodes.iter().filter(|node| node.fixes) {
            let (x, y) = place(node);
            let length = (x * x + y * y).sqrt().max(0.001);
            pen.dashed(at(x, y), at(x / length * 1.08, y / length * 1.08), sk(pal.teal, 1.0), 1.1, (3.0, 2.5));
        }
        if let Some(chosen) = spec.nodes.iter().find(|node| node.selected) {
            let (sx, sy) = place(chosen);
            let target = at(sx, sy);
            for link in &spec.links {
                let y = (f32::from(link.row) + 0.5) * cell_height;
                let x = if link.left { 4.0 * unit } else { w - 4.0 * unit };
                let side = if link.left { 1.0 } else { -1.0 };
                let (color, alpha) = match link.kind {
                    2 => (pal.teal, 0.9),
                    3 => (pal.relation, 0.35),
                    _ => (pal.relation, 0.85),
                };
                let mut path = PathBuilder::new();
                path.move_to(x, y);
                path.cubic_to(x + side * w * 0.22, y, target.0 - side * w * 0.12, target.1, target.0, target.1);
                pen.stroke(path, sk(color, alpha), if link.kind == 3 { 0.6 } else { 0.9 }, None);
                let dot = match link.kind {
                    2 => pal.teal,
                    1 => pal.ink,
                    3 => pal.faint,
                    _ => pal.ink2,
                };
                pen.disc(x, y, if link.kind == 3 { 1.2 } else { 2.0 } * unit, sk(dot, 0.95));
            }
        }
        for node in &spec.nodes {
            let (x, y) = at(place(node).0, place(node).1);
            match node.ring {
                0 => {
                    pen.disc(x, y, 6.5 * unit, sk(ground, 1.0));
                    pen.disc(x, y, 4.2 * unit, ink(1.0));
                    pen.circle(x, y, 7.5 * unit, ink(0.8), 0.9, None);
                }
                1 => {
                    pen.disc(x, y, 4.6 * unit, sk(ground, 1.0));
                    pen.disc(x, y, 3.4 * unit, sk(pal.ink2, 1.0));
                    pen.disc(x, y, 1.2 * unit, sk(ground, 1.0));
                }
                _ => pen.disc(x, y, 1.3 * unit, sk(pal.label, 0.95)),
            }
            if node.fixes {
                pen.circle(x, y, 15.0 * unit, sk(pal.teal, 0.8), 0.9, Some((2.0, 2.0)));
            }
            if node.related {
                let ring = match node.ring {
                    0 => 9.5,
                    1 => 6.5,
                    _ => 3.2,
                };
                pen.circle(x, y, ring * unit, sk(pal.relation, 1.0), 1.0, None);
            }
            if node.selected {
                pen.circle(x, y, 11.0 * unit, ink(1.0), 1.6, None);
            }
        }
    }
    if let Some(atlas) = atlas(cell_height) {
        let advance = atlas.advance as f32;
        for ((name, count), (start, span)) in spec.sectors.iter().zip(&spans) {
            if *count == 0 {
                continue;
            }
            let (sin, cos) = (start + span / 2.0).sin_cos();
            let (x, y) = (c.0 + cos * r * 1.2, c.1 + sin * r * 1.2);
            let line: Vec<(char, SkColor)> = name.chars().map(|ch| (ch, sk(pal.label, 1.0))).collect();
            let length = line.len() as f32 * advance;
            let left = (x - length / 2.0).clamp(2.0, (w - length - 2.0).max(2.0));
            type_line(&mut pixmap, atlas, left, y + atlas.ascent as f32 / 2.0, &line, &|_, _| 1.0);
        }
    }
    Some(pixmap)
}

pub fn fan(spec: &FanSpec, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, if spec.panel { pal.panel } else { pal.page }, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let rows = spec.left.len().max(spec.right.len()).max(1);
    let cell = h / rows as f32;
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let (cx, cy) = (w / 2.0, h / 2.0);
    for (leaves, side) in [(&spec.left, -1.0f32), (&spec.right, 1.0)] {
        let offset = (rows - leaves.len()) / 2;
        for (index, kind) in leaves.iter().enumerate() {
            let y = (offset + index) as f32 * cell + cell / 2.0;
            let x = if side < 0.0 { 5.0 * unit } else { w - 5.0 * unit };
            let (color, line) = match kind {
                2 => (pal.teal, 0.75),
                1 => (pal.ink, 0.6),
                3 => (pal.faint, 0.3),
                _ => (pal.ink2, 0.4),
            };
            let mut path = PathBuilder::new();
            path.move_to(cx + side * 9.0 * unit, cy);
            path.cubic_to(cx + side * w * 0.2, cy, x - side * w * 0.2, y, x, y);
            pen.stroke(path, sk(color, line), 0.7, None);
            pen.disc(x, y, if *kind == 3 { 1.2 } else { 2.0 } * unit, sk(color, 0.95));
        }
    }
    pen.disc(cx, cy, 4.5 * unit, sk(pal.ink, 1.0));
    pen.circle(cx, cy, 8.5 * unit, sk(pal.ink, 0.5), 0.8, None);
    Some(pixmap)
}

fn canvas(width: u32, height: u32, background: Color, cell_height: f32) -> Option<(Pixmap, f32)> {
    let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
    pixmap.fill(sk(background, 1.0));
    Some((pixmap, (cell_height / 21.0).max(0.6)))
}

/// A small specimen plate for a subsystem, in the same engraved line work.
pub fn specimen(subsystem: Subsystem, pal: &Palette, width: u32, height: u32, cell_height: f32, boost: f32, aim: Option<(Color, f32)>) -> Option<Pixmap> {
    let (mut pixmap, plain) = canvas(width, height, pal.panel, cell_height)?;
    let unit = plain * (1.0 + 0.9 * boost.clamp(0.0, 1.0));
    let (w, h) = (width as f32, height as f32);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let ink = |alpha: f32| sk(pal.ink, alpha + (1.0 - alpha) * boost.clamp(0.0, 1.0));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let s = h.min(w) * 0.42;

    match subsystem {
        Subsystem::Kernel => {
            pen.rect(cx - s * 0.62, cy - s * 0.62, s * 1.24, s * 1.24, ink(0.9), 1.0);
            pen.rect(cx - s * 0.36, cy - s * 0.36, s * 0.72, s * 0.72, ink(0.6), 0.8);
            for k in 0..9 {
                let t = -0.52 + k as f32 * 0.13;
                for (ax, ay, bx, by) in [
                    (cx + t * s, cy - s * 0.62, cx + t * s, cy - s * 0.82),
                    (cx + t * s, cy + s * 0.62, cx + t * s, cy + s * 0.82),
                    (cx - s * 0.62, cy + t * s, cx - s * 0.82, cy + t * s),
                    (cx + s * 0.62, cy + t * s, cx + s * 0.82, cy + t * s),
                ] {
                    pen.line((ax, ay), (bx, by), ink(0.75), 0.8);
                }
            }
            for i in 0..4 {
                for j in 0..4 {
                    pen.disc(cx - s * 0.2 + i as f32 * s * 0.13, cy - s * 0.2 + j as f32 * s * 0.13, 0.9 * unit, ink(0.6));
                }
            }
            pen.circle(cx - s * 0.48, cy - s * 0.48, 1.6 * unit, ink(0.8), 0.8, None);
        }
        Subsystem::Graphics => {
            pen.circle(cx, cy, s * 0.8, ink(0.9), 1.0, None);
            for k in 1..5 {
                pen.ellipse((cx, cy), s * 0.8, s * 0.8 * (0.2 + 0.2 * k as f32).min(0.95), 0.0, 0.0, 1.0, ink(0.35), 0.7, None);
                pen.ellipse((cx, cy), s * 0.8 * (0.2 + 0.2 * k as f32).min(0.95), s * 0.8, 0.0, 0.0, 1.0, ink(0.35), 0.7, None);
            }
            pen.line((cx - s * 0.8, cy), (cx + s * 0.8, cy), ink(0.5), 0.7);
            pen.line((cx, cy - s * 0.8), (cx, cy + s * 0.8), ink(0.5), 0.7);
        }
        Subsystem::Compositor => {
            for k in (0..3).rev() {
                let offset = k as f32 * s * 0.26;
                let (x, y) = (cx - s * 0.85 + offset, cy - s * 0.65 + offset * 0.7);
                let alpha = 0.45 + 0.25 * (2 - k) as f32;
                pen.fill_rect(x, y, s * 1.2, s * 0.85, sk(pal.panel, 1.0));
                pen.rect(x, y, s * 1.2, s * 0.85, ink(alpha), 0.9);
                pen.line((x, y + s * 0.16), (x + s * 1.2, y + s * 0.16), ink(alpha), 0.7);
                for d in 0..3 {
                    pen.disc(x + s * 0.08 + d as f32 * s * 0.08, y + s * 0.08, 0.9 * unit, ink(alpha));
                }
            }
        }
        Subsystem::Toolkit => {
            for i in 0..3 {
                for j in 0..2 {
                    let (x, y) = (cx - s * 0.9 + i as f32 * s * 0.62, cy - s * 0.6 + j as f32 * s * 0.66);
                    pen.rect(x, y, s * 0.5, s * 0.5, ink(0.75), 0.8);
                    pen.line((x + s * 0.08, y + s * 0.25), (x + s * 0.42, y + s * 0.25), ink(0.5), 0.7);
                    pen.disc(x + s * 0.08 + (i + j) as f32 * s * 0.1, y + s * 0.25, 1.4 * unit, ink(0.8));
                }
            }
        }
        Subsystem::Shell => {
            pen.rect(cx - s * 0.95, cy - s * 0.65, s * 1.9, s * 1.3, ink(0.9), 1.0);
            pen.line((cx - s * 0.95, cy - s * 0.42), (cx + s * 0.95, cy - s * 0.42), ink(0.6), 0.7);
            pen.line((cx - s * 0.75, cy - s * 0.2), (cx - s * 0.6, cy - s * 0.08), ink(0.85), 1.0);
            pen.line((cx - s * 0.6, cy - s * 0.08), (cx - s * 0.75, cy + s * 0.04), ink(0.85), 1.0);
            for (k, len) in [0.9, 0.6, 0.75].iter().enumerate() {
                let y = cy - s * 0.08 + k as f32 * s * 0.18;
                pen.line((cx - s * 0.48, y), (cx - s * 0.48 + s * len, y), ink(0.45), 0.8);
            }
            pen.fill_rect(cx + s * 0.35, cy + s * 0.3, s * 0.12, s * 0.18, sk(pal.teal, 0.9));
        }
        Subsystem::Base => {
            let teeth = 10;
            let mut builder = PathBuilder::new();
            for k in 0..=teeth * 4 {
                let angle = k as f32 * TAU / (teeth * 4) as f32;
                let radius = if (k / 2) % 2 == 0 { s * 0.8 } else { s * 0.64 };
                let (x, y) = (cx + angle.cos() * radius, cy + angle.sin() * radius);
                if k == 0 {
                    builder.move_to(x, y);
                } else {
                    builder.line_to(x, y);
                }
            }
            pen.stroke(builder, ink(0.85), 0.9, None);
            pen.circle(cx, cy, s * 0.3, ink(0.7), 0.9, None);
            pen.circle(cx, cy, s * 0.12, ink(0.5), 0.8, None);
        }
        Subsystem::Apps => {
            for i in 0..3 {
                for j in 0..3 {
                    let (x, y) = (cx - s * 0.75 + i as f32 * s * 0.52, cy - s * 0.75 + j as f32 * s * 0.52);
                    pen.rect(x, y, s * 0.38, s * 0.38, ink(0.55 + 0.1 * ((i + j) % 3) as f32), 0.8);
                }
            }
        }
    }

    if let Some((color, strength)) = aim.filter(|(_, strength)| *strength > 0.01) {
        pen.unit = 1.0;
        let tone = sk(color, strength.clamp(0.0, 1.0));
        let (_, heavy) = line_widths(w / 8.0, cell_height);
        let half = heavy as f32 / 2.0;
        let (inset, arm) = ((3.0 * plain).round() + half, (11.0 * plain).round());
        let (left, right, top, bottom) = (inset, w - inset, inset, h - inset);
        for (x, y, sx, sy) in [(left, top, 1.0, 1.0), (right, top, -1.0, 1.0), (left, bottom, 1.0, -1.0), (right, bottom, -1.0, -1.0)] {
            pen.polyline(&[(x + sx * arm, y), (x, y), (x, y + sy * arm)], tone, heavy as f32);
        }
    }

    Some(pixmap)
}


fn noise(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

pub fn line_widths(cell_width: f32, cell_height: f32) -> (u32, u32) {
    let base = ((0.04 * cell_width.hypot(cell_height) * 96.0 / 72.0) as u32).max(1);
    (base, base * 3)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fiber {
    pub angle: f32,
    pub risk: u8,
    pub fixes: bool,
}

#[derive(Debug, Default)]
pub struct EyeSpec {
    pub fibers: Vec<Fiber>,
    pub sectors: Vec<f32>,
    pub gaze: (f32, f32),
    pub code: Vec<(String, String, usize)>,
    pub typing: bool,
    pub scroll: f32,
    pub columns: u16,
    pub up: Vec<(u16, bool)>,
    pub down: Vec<(u16, bool)>,
    pub sides: Option<(bool, bool)>,
    pub turn: f32,
    pub rain: Option<Rain>,
    pub sight: Vec<(u8, u16, f32)>,
}

#[derive(Clone)]
pub struct Rain {
    pub clock: f32,
    pub level: f32,
    pub text: Arc<Vec<char>>,
    pub before: Arc<Vec<char>>,
    pub feed: u64,
    pub previous: u64,
    pub switch: f32,
    pub tint: Option<(Color, f32)>,
}

impl std::fmt::Debug for Rain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rain({}, {}, {}, {}, {}, {:?})", self.clock.to_bits(), self.level.to_bits(), self.feed, self.previous, self.switch.to_bits(), self.tint.map(|(color, strength)| (color, strength.to_bits())))
    }
}

struct Atlas {
    size: u32,
    advance: u32,
    height: u32,
    ascent: u32,
    codes: Vec<u32>,
    data: &'static [u8],
}

impl Atlas {
    fn glyph(&self, ch: char) -> Option<&'static [u8]> {
        let index = self.codes.iter().position(|code| *code == ch as u32)?;
        let size = (self.advance * self.height) as usize;
        self.data.get(index * size..(index + 1) * size)
    }
}

fn atlases() -> &'static [Atlas] {
    static ATLASES: std::sync::OnceLock<Vec<Atlas>> = std::sync::OnceLock::new();
    ATLASES.get_or_init(|| {
        let bytes: &'static [u8] = include_bytes!("../assets/glyphs.bin");
        let mut atlases = Vec::new();
        let mut at = 1usize;
        for _ in 0..bytes.first().copied().unwrap_or(0) {
            let Some(head) = bytes.get(at..at + 6) else { break };
            let (size, advance, height, ascent) = (u32::from(head[0]), u32::from(head[1]), u32::from(head[2]), u32::from(head[3]));
            let count = usize::from(u16::from_le_bytes([head[4], head[5]]));
            let codes: Vec<u32> = (0..count).filter_map(|k| bytes.get(at + 6 + 4 * k..at + 10 + 4 * k)).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            let start = at + 6 + 4 * count;
            let end = start + (advance * height) as usize * count;
            let Some(data) = bytes.get(start..end) else { break };
            atlases.push(Atlas { size, advance, height, ascent, codes, data });
            at = end;
        }
        atlases
    })
}

fn atlas(cell_height: f32) -> Option<&'static Atlas> {
    let target = 10.0 * cell_height / 21.0;
    atlases().iter().min_by(|a, b| (a.size as f32 - target).abs().total_cmp(&(b.size as f32 - target).abs()))
}

type Glyphs = Vec<(char, SkColor)>;

struct Break {
    end: usize,
    next: usize,
    level: u8,
}

fn breaks(chars: &[(char, SkColor)], split: Option<usize>) -> Vec<Break> {
    let mut out = Vec::new();
    for (i, (ch, _)) in chars.iter().enumerate() {
        if *ch == ' ' {
            if i > 0 && chars[i - 1].0 == ' ' {
                continue;
            }
            let next = chars[i..].iter().position(|(other, _)| *other != ' ').map_or(chars.len(), |gap| i + gap);
            let level = if Some(i) == split {
                0
            } else if i > 0 && chars[i - 1].0 == '›' {
                3
            } else if split.is_some_and(|split| i > split) {
                2
            } else {
                1
            };
            out.push(Break { end: i, next, level });
        } else if *ch == '/' && i > 0 && chars.get(i + 1).is_some_and(|(other, _)| *other != ' ') {
            out.push(Break { end: i + 1, next: i + 1, level: 1 });
        }
    }
    out
}

fn wrap(chars: &[(char, SkColor)], rooms: &[usize], breaks: &[Break], level: u8) -> (Vec<(usize, usize)>, bool) {
    let skip = |mut at: usize| {
        while at < chars.len() && chars[at].0 == ' ' {
            at += 1;
        }
        at
    };
    let mut spans = Vec::new();
    let mut start = skip(0);
    for &room in rooms {
        if start >= chars.len() {
            break;
        }
        if chars.len() - start <= room {
            spans.push((start, chars.len()));
            start = chars.len();
            break;
        }
        let limit = start + room;
        let best = breaks.iter().filter(|candidate| candidate.level <= level && candidate.end > start && candidate.end <= limit).map(|candidate| (candidate.end, candidate.next)).next_back();
        let (end, next) = match best {
            Some(found) => found,
            None if level >= 4 => (limit, limit),
            None => return (spans, false),
        };
        spans.push((start, end));
        start = skip(next);
    }
    (spans, start >= chars.len())
}

fn type_line(pixmap: &mut Pixmap, atlas: &Atlas, x: f32, baseline: f32, line: &[(char, SkColor)], clip: &dyn Fn(f32, f32) -> f32) {
    let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
    let data = pixmap.data_mut();
    let top = (baseline - atlas.ascent as f32).round() as i32;
    let mut left = x.round() as i32;
    for (ch, color) in line {
        let alpha = color.alpha();
        if alpha > 0.0
            && let Some(bitmap) = atlas.glyph(*ch)
        {
            for j in 0..atlas.height as i32 {
                let py = top + j;
                if py < 0 || py >= height {
                    continue;
                }
                for i in 0..atlas.advance as i32 {
                    let coverage = bitmap[(j as u32 * atlas.advance + i as u32) as usize];
                    let px = left + i;
                    if coverage == 0 || px < 0 || px >= width {
                        continue;
                    }
                    let keep = clip(px as f32 + 0.5, py as f32 + 0.5);
                    if keep <= 0.0 {
                        continue;
                    }
                    let a = f32::from(coverage) / 255.0 * alpha * keep.min(1.0);
                    let offset = ((py * width + px) * 4) as usize;
                    for (channel, source) in [color.red(), color.green(), color.blue()].into_iter().enumerate() {
                        let dst = f32::from(data[offset + channel]);
                        data[offset + channel] = (source * 255.0 * a + dst * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        left += atlas.advance as i32;
    }
}

fn lid(c: (f32, f32), half_width: f32, depth: f32, steps: usize) -> Vec<(f32, f32)> {
    let bulge = depth.abs().max(0.01);
    let radius = (half_width * half_width + bulge * bulge) / (2.0 * bulge);
    let half = (half_width / radius).clamp(-1.0, 1.0).asin();
    let (center, from, to) = if depth < 0.0 {
        ((c.0, c.1 - bulge + radius), -TAU / 4.0 - half, -TAU / 4.0 + half)
    } else {
        ((c.0, c.1 + bulge - radius), TAU / 4.0 + half, TAU / 4.0 - half)
    };
    (0..=steps)
        .map(|s| {
            let angle = from + (to - from) * s as f32 / steps as f32;
            (center.0 + radius * angle.cos(), center.1 + radius * angle.sin())
        })
        .collect()
}

struct Base {
    key: (u32, u32, u32, Color, Color),
    pixmap: Pixmap,
    mask: Option<Mask>,
}

thread_local! {
    static BASE: RefCell<Option<Base>> = const { RefCell::new(None) };
}

fn base_layer(pal: &Palette, width: u32, height: u32, center_y: f32, cell_height: f32, half_width: f32, depths: (f32, f32)) -> Option<(Pixmap, Option<Mask>)> {
    let key = (width, height, center_y.to_bits(), pal.page, pal.ink);
    if let Some(found) = BASE.with(|base| base.borrow().as_ref().filter(|base| base.key == key).map(|base| (base.pixmap.clone(), base.mask.clone()))) {
        return Some(found);
    }
    let (mut pixmap, unit) = canvas(width, height, pal.page, cell_height)?;
    let c = (width as f32 / 2.0, center_y);
    let upper = lid(c, half_width, -depths.0, 72);
    let lower = lid(c, half_width, depths.1, 72);
    let mut builder = PathBuilder::new();
    for (index, (x, y)) in upper.iter().enumerate() {
        if index == 0 {
            builder.move_to(*x, *y);
        } else {
            builder.line_to(*x, *y);
        }
    }
    for (x, y) in lower.iter().rev() {
        builder.line_to(*x, *y);
    }
    builder.close();
    let path = builder.finish()?;
    let mut paint = Paint::default();
    paint.set_color(sk(mix(pal.page, pal.ink, 0.04), 1.0));
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    let mask = Mask::new(width, height).map(|mut mask| {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        mask
    });
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: mask.clone() };
    let dots = (half_width * depths.0 / 18.0).clamp(40.0, 900.0) as u32;
    for i in 0..dots {
        let x = c.0 + (noise(i * 5 + 11) * 2.0 - 1.0) * half_width;
        let y = c.1 + (noise(i * 5 + 12) * 2.0 - 1.0) * depths.0;
        let edge = ((x - c.0).abs() / half_width).powi(2);
        pen.disc(x, y, (0.45 + noise(i * 5 + 13) * 0.6) * unit, sk(pal.ink, 0.04 + 0.24 * edge));
    }
    BASE.with(|base| *base.borrow_mut() = Some(Base { key, pixmap: pixmap.clone(), mask: mask.clone() }));
    Some((pixmap, mask))
}

fn stroke_all(pixmap: &mut Pixmap, builder: PathBuilder, color: SkColor, width: f32, mask: Option<&Mask>) {
    let Some(path) = builder.finish() else { return };
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = true;
    let stroke = Stroke { width, line_cap: LineCap::Butt, ..Stroke::default() };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), mask);
}

fn fill_box(pixmap: &mut Pixmap, x: i32, y: i32, w: i32, h: i32, color: SkColor) {
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(pixmap.width() as i32), (y + h).min(pixmap.height() as i32));
    if x1 > x0
        && y1 > y0
        && let Some(rect) = tiny_skia::Rect::from_ltrb(x0 as f32, y0 as f32, x1 as f32, y1 as f32)
    {
        let mut paint = Paint::default();
        paint.set_color(color);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

pub fn eye(spec: &EyeSpec, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (w, h) = (width as f32, height as f32);
    let rows = (h / cell_height).round().max(1.0);
    let cell_width = if spec.columns > 0 { (width / u32::from(spec.columns)) as f32 } else { cell_height * 0.5 };
    let (light, heavy) = line_widths(cell_width, cell_height);
    let canthus_row = (rows as u32 / 2) as f32;
    let center_y = if spec.columns > 0 { canthus_row * cell_height + ((cell_height as u32 - light) / 2) as f32 + light as f32 / 2.0 } else { h / 2.0 };
    let c = (w / 2.0, center_y);
    let half_width = (w * 0.47).min(h * 0.47 / 0.608) * 0.96;
    let (upper_depth, lower_depth) = (half_width * 0.608, half_width * 0.5);
    let (mut pixmap, mask) = base_layer(pal, width, height, center_y, cell_height, half_width, (upper_depth, lower_depth))?;
    let unit = (cell_height / 21.0).max(0.6);
    let ink = |alpha: f32| sk(pal.ink, alpha);
    let glyphs = atlas(cell_height);

    let ri = half_width * 0.54;
    let rp = ri * 0.52;
    let ic = (c.0 + spec.gaze.0 * half_width * 0.27, c.1 + spec.gaze.1 * half_width * 0.07);
    if let (Some(rain), Some(atlas)) = (&spec.rain, glyphs) {
        let (pitch_x, pitch_y) = (atlas.advance as f32 + (3.0 * unit).round(), (12.0 * unit).round().max(6.0));
        let circle = |bulge: f32, below: bool| {
            let radius = (half_width * half_width + bulge * bulge) / (2.0 * bulge);
            (if below { c.1 - bulge + radius } else { c.1 + bulge - radius }, radius)
        };
        let (upper_y, upper_r) = circle(upper_depth, true);
        let (lower_y, lower_r) = circle(lower_depth, false);
        let depth = |x: f32, y: f32| (upper_r - (x - c.0).hypot(y - upper_y)).min(lower_r - (x - c.0).hypot(y - lower_y));
        let clip = |px: f32, py: f32| {
            let edge = ((depth(px, py) - 3.0 * unit) / (9.0 * unit)).clamp(0.0, 1.0);
            let iris = (((px - ic.0).hypot(py - ic.1) - ri * 1.025 - 3.0 * unit) / (8.0 * unit)).clamp(0.0, 1.0);
            edge * iris
        };
        let level = rain.level.clamp(0.0, 1.0);
        let columns = (2.0 * half_width / pitch_x).floor() as u32;
        let left = c.0 - columns as f32 * pitch_x / 2.0 + (1.5 * unit).round();
        let top = c.1 - upper_depth;
        let rows = ((upper_depth + lower_depth) / pitch_y).ceil() as u32;
        for column in 0..columns {
            let presence = ((level + 0.15 - noise(column * 17 + 3)) / 0.18 + 0.5).clamp(0.0, 1.0);
            if presence <= 0.0 {
                continue;
            }
            let speed = 0.75 + 0.6 * noise(column * 17 + 5);
            let period = 12.0 + 12.0 * noise(column * 17 + 7);
            let phase = noise(column * 17 + 11) * period;
            let travelled = rain.clock * speed + phase;
            let x = left + column as f32 * pitch_x;
            for row in 0..rows {
                let behind = travelled - row as f32;
                if behind < 0.0 {
                    continue;
                }
                let drop = (behind / period).floor();
                let distance = behind - drop * period;
                let seed = column.wrapping_mul(7919) ^ (drop as u32).wrapping_mul(104_729);
                let length = (6.0 + 9.0 * noise(seed)).min(period - 2.0);
                if distance > length {
                    continue;
                }
                let baseline = top + (row as f32 + 1.0) * pitch_y;
                let middle = (x + atlas.advance as f32 / 2.0, baseline - atlas.ascent as f32 / 2.0);
                if depth(middle.0, middle.1) < -6.0 * unit || (middle.0 - ic.0).hypot(middle.1 - ic.1) < ri - 6.0 * unit {
                    continue;
                }
                let rise = (distance / 0.4).clamp(0.0, 1.0);
                let trail = (1.0 - distance / length).max(0.0).powf(1.6);
                let spark = (-(distance - 0.4).max(0.0) * 2.2).exp();
                let alpha = rise * (0.34 * trail + 0.5 * spark) * presence * (0.55 + 0.45 * level);
                if alpha < 0.01 {
                    continue;
                }
                let born = (drop * period - phase) / speed;
                let source = if born < rain.switch { &rain.before } else { &rain.text };
                if source.is_empty() {
                    continue;
                }
                let ch = source[((noise(seed ^ 0x5bd1) * source.len() as f32) as usize + row as usize) % source.len()];
                if ch.is_whitespace() {
                    continue;
                }
                let tone = match rain.tint {
                    Some((tint, strength)) => mix(pal.ink, tint, f64::from((strength * spark).clamp(0.0, 1.0))),
                    None => pal.ink,
                };
                type_line(&mut pixmap, atlas, x, baseline, &[(ch, sk(tone, alpha))], &clip);
            }
        }
    }

    let mut pen = Pen { pixmap: &mut pixmap, unit, mask };
    pen.disc(ic.0, ic.1, ri, sk(mix(pal.page, pal.ink, 0.09), 1.0));
    let base = (ri * 2.4).clamp(60.0, 300.0) as u32;
    let mut texture: [PathBuilder; 4] = Default::default();
    for k in 0..base {
        let angle = k as f32 / base as f32 * TAU + (noise(k * 3 + 1) - 0.5) * 0.02 + spec.turn;
        let bend = (noise(k * 3 + 2) - 0.5) * 0.09;
        let (r0, r1) = (rp * 1.15, ri * (0.88 + 0.1 * noise(k * 3 + 3)));
        let mid = (r0 + r1) / 2.0;
        let bucket = ((noise(k * 3 + 4) * 4.0) as usize).min(3);
        let fiber = &mut texture[bucket];
        fiber.move_to(ic.0 + angle.cos() * r0, ic.1 + angle.sin() * r0);
        fiber.quad_to(ic.0 + (angle + bend).cos() * mid, ic.1 + (angle + bend).sin() * mid, ic.0 + angle.cos() * r1, ic.1 + angle.sin() * r1);
    }
    for (bucket, builder) in texture.into_iter().enumerate() {
        stroke_all(pen.pixmap, builder, ink(0.12 + 0.16 * (bucket as f32 + 0.5) / 4.0), 0.55 * unit, pen.mask.as_ref());
    }
    let collarette: Vec<(f32, f32)> = (0..=160u32)
        .map(|s| {
            let angle = s as f32 / 160.0 * TAU;
            let ring = rp + (ri - rp) * 0.34;
            let radius = ring + (angle * 11.0 + 0.7).sin() * ri * 0.022 + (noise((s % 160) * 7 + 5) - 0.5) * ri * 0.025;
            let angle = angle + spec.turn;
            (ic.0 + angle.cos() * radius, ic.1 + angle.sin() * radius)
        })
        .collect();
    pen.polyline(&collarette, ink(0.42), 0.8);

    let mut strokes: Vec<(Color, f32, PathBuilder)> = Vec::new();
    let mut fixes: Vec<(f32, f32)> = Vec::new();
    for fiber in &spec.fibers {
        let (sin, cos) = (fiber.angle + spec.turn).sin_cos();
        let outer = ri * 0.97;
        let inner = match fiber.risk {
            0 => rp * 1.04,
            1 => rp + (ri - rp) * 0.38,
            _ => rp + (ri - rp) * 0.66,
        };
        let (color, width) = match fiber.risk {
            0 => (pal.alert, 2.4),
            1 => (pal.warn, 1.8),
            _ => (pal.label, 1.2),
        };
        let color = if fiber.fixes { pal.teal } else { color };
        let slot = match strokes.iter().position(|(owner, size, _)| *owner == color && *size == width) {
            Some(slot) => slot,
            None => {
                strokes.push((color, width, PathBuilder::new()));
                strokes.len() - 1
            }
        };
        strokes[slot].2.move_to(ic.0 + cos * outer, ic.1 + sin * outer);
        strokes[slot].2.line_to(ic.0 + cos * inner, ic.1 + sin * inner);
        if fiber.fixes {
            fixes.push((ic.0 + cos * ri * 1.08, ic.1 + sin * ri * 1.08));
        }
    }
    for (color, width, builder) in strokes {
        stroke_all(pen.pixmap, builder, sk(color, 0.95), width * unit, pen.mask.as_ref());
    }
    for (x, y) in fixes {
        pen.disc(x, y, (ri / 50.0).clamp(1.2, 3.0) * unit, sk(pal.teal, 1.0));
    }
    if !spec.fibers.is_empty() && !spec.sectors.is_empty() {
        let mut ticks = PathBuilder::new();
        for angle in &spec.sectors {
            let (sin, cos) = (angle + spec.turn).sin_cos();
            ticks.move_to(ic.0 + cos * ri * 0.9, ic.1 + sin * ri * 0.9);
            ticks.line_to(ic.0 + cos * ri * 1.04, ic.1 + sin * ri * 1.04);
        }
        stroke_all(pen.pixmap, ticks, ink(0.75), 0.8 * unit, pen.mask.as_ref());
    }

    pen.circle(ic.0, ic.1, ri, ink(0.9), 1.5, None);
    pen.circle(ic.0, ic.1, ri * 1.025, ink(0.25), 0.6, None);
    pen.circle(ic.0, ic.1, rp * 1.14, ink(0.45), 0.6, None);
    let dark = brightness(pal.page) < brightness(pal.ink);
    let pupil = if dark { pal.page } else { pal.ink };
    pen.disc(ic.0, ic.1, rp, sk(pupil, 1.0));
    pen.mask = None;
    if let Some(atlas) = glyphs {
        let line_height = atlas.height as f32 - 1.0;
        let advance = atlas.advance as f32;
        let pad = (8.0 * unit).round();
        let mut rows: Vec<(f32, usize)> = Vec::new();
        let mut baseline = ic.1 + rp * 0.56;
        while baseline - atlas.ascent as f32 > ic.1 - rp + 3.0 * unit {
            let middle = baseline - atlas.ascent as f32 / 2.0;
            let chord = (rp * rp - (middle - ic.1).powi(2)).max(0.0).sqrt();
            let room = ((2.0 * chord - 2.0 * pad) / advance).floor().max(0.0) as usize;
            if room >= 3 {
                rows.push((baseline, room));
            }
            baseline -= line_height;
        }
        let count = spec.code.len();
        let mut entries: Vec<(Glyphs, Option<usize>, Option<usize>)> = Vec::new();
        for (index, (head, tail, shown)) in spec.code.iter().enumerate() {
            let age = count - 1 - index;
            let fade = if age == 0 { 1.0 } else { (0.82 - 0.12 * age as f32).max(0.26) };
            let mut chars: Vec<(char, SkColor)> = head.chars().map(|ch| (ch, sk(pal.ink2, fade))).collect();
            let split = (!tail.is_empty()).then_some(chars.len());
            if !tail.is_empty() {
                chars.extend("  ".chars().map(|ch| (ch, sk(pal.ink, fade))));
                chars.extend(tail.chars().map(|ch| (ch, sk(pal.ink, fade))));
            }
            if chars.len() > 34 {
                let tone = chars[33].1;
                chars.truncate(33);
                chars.push(('…', tone));
            }
            let visible = (*shown).min(chars.len());
            for item in chars.iter_mut().skip(visible) {
                item.1 = sk(pal.ink, 0.0);
            }
            entries.push((chars, (age == 0 && spec.typing).then_some(visible), split));
        }
        let mut placed: Vec<(usize, usize, Glyphs)> = Vec::new();
        let mut slot = 0usize;
        let mut newest_rows = 1usize;
        for (index, (chars, cursor, split)) in entries.iter().rev().enumerate() {
            if slot >= rows.len() {
                break;
            }
            let breaks = breaks(chars, *split);
            let top = if index == 0 { 4 } else { 2 };
            let mut fitted = None;
            'fit: for used in 1..=rows.len() - slot {
                let rooms: Vec<usize> = (slot..slot + used).rev().map(|k| rows[k].1).collect();
                for level in 0..=top {
                    let (spans, complete) = wrap(chars, &rooms, &breaks, level);
                    if complete || (level == 4 && slot + used == rows.len()) {
                        fitted = Some((used, spans));
                        break 'fit;
                    }
                }
            }
            let Some((used, spans)) = fitted else { break };
            for (k, (start, end)) in (slot..slot + used).rev().zip(spans.iter().copied()) {
                let mut line: Vec<(char, SkColor)> = chars[start..end].to_vec();
                match cursor {
                    Some(at) if (start..end).contains(at) => line[at - start] = ('▌', sk(pal.ink, 1.0)),
                    Some(at) if *at == chars.len() && end == chars.len() => line.push(('▌', sk(pal.ink, 1.0))),
                    _ => {}
                }
                placed.push((k, end - start, line));
            }
            if slot == 0 {
                newest_rows = used;
            }
            slot += used;
        }
        let shift = spec.scroll.clamp(0.0, 1.0) * newest_rows as f32 * line_height;
        let clip = |px: f32, py: f32| (((rp - 2.0 * unit) - ((px - ic.0).powi(2) + (py - ic.1).powi(2)).sqrt()) / (5.0 * unit)).clamp(0.0, 1.0);
        for (row, span, line) in &placed {
            let (y, _) = rows[*row];
            let x = (ic.0 - *span as f32 * advance / 2.0).round();
            type_line(pen.pixmap, atlas, x, y + shift, line, &clip);
        }
    }

    let upper = lid(c, half_width, -upper_depth, 72);
    let lower = lid(c, half_width, lower_depth, 72);
    if spec.columns > 0 {
        let cell = cell_width as i32;
        let line_x = |column: u16, thick: u32| i32::from(column) * cell + (cell - thick as i32) / 2;
        let lid_y = |x: f32, depth: f32| {
            let bulge = depth.abs();
            let radius = (half_width * half_width + bulge * bulge) / (2.0 * bulge);
            let dx = (x - c.0).clamp(-half_width, half_width);
            let rise = (radius * radius - dx * dx).max(0.0).sqrt() - (radius - bulge);
            if depth < 0.0 { c.1 - rise } else { c.1 + rise }
        };
        let tone = |active: bool| if active { sk(pal.rule, 1.0) } else { sk(pal.grid, 1.0) };
        let row_y = |thick: u32| (canthus_row * cell_height) as i32 + (cell_height as i32 - thick as i32) / 2;
        let left_end = (c.0 - half_width).round() as i32;
        let right_start = (c.0 + half_width).round() as i32;
        for (column, active) in &spec.up {
            let x = line_x(*column, light);
            let end = lid_y(x as f32 + light as f32 / 2.0, -upper_depth).round() as i32;
            fill_box(pen.pixmap, x, 0, light as i32, end, tone(*active));
        }
        for (column, active) in &spec.down {
            let x = line_x(*column, light);
            let start = lid_y(x as f32 + light as f32 / 2.0, lower_depth).round() as i32;
            fill_box(pen.pixmap, x, start, light as i32, height as i32 - start, tone(*active));
        }
        if let Some((left, right)) = spec.sides {
            let y = row_y(light);
            fill_box(pen.pixmap, 0, y, left_end, light as i32, tone(left));
            fill_box(pen.pixmap, right_start, y, width as i32 - right_start, light as i32, tone(right));
        }
        let sight = sk(pal.ink2, 1.0);
        for (side, column, reach) in &spec.sight {
            let reach = reach.clamp(0.0, 1.0);
            if reach <= 0.0 {
                continue;
            }
            match side {
                0 => {
                    let x = line_x(*column, heavy);
                    let end = lid_y(x as f32 + heavy as f32 / 2.0, -upper_depth).round() as i32;
                    let top = (end as f32 * (1.0 - reach)).round() as i32;
                    fill_box(pen.pixmap, x, top, heavy as i32, end - top, sight);
                }
                1 => {
                    let x = line_x(*column, heavy);
                    let start = lid_y(x as f32 + heavy as f32 / 2.0, lower_depth).round() as i32;
                    let length = ((height as i32 - start) as f32 * reach).round() as i32;
                    fill_box(pen.pixmap, x, start, heavy as i32, length, sight);
                }
                2 => {
                    let start = (left_end as f32 * (1.0 - reach)).round() as i32;
                    fill_box(pen.pixmap, start, row_y(heavy), left_end - start, heavy as i32, sight);
                }
                _ => {
                    let length = ((width as i32 - right_start) as f32 * reach).round() as i32;
                    fill_box(pen.pixmap, right_start, row_y(heavy), length, heavy as i32, sight);
                }
            }
        }
    }
    pen.polyline(&upper, ink(0.95), 1.5);
    pen.polyline(&lower, ink(0.8), 1.1);
    Some(pixmap)
}

fn brightness(color: Color) -> f32 {
    match color {
        Color::Rgb(r, g, b) => 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b),
        _ => 0.0,
    }
}

impl EyeSpec {
    pub fn fingerprint(&self, hasher: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        for fiber in &self.fibers {
            (fiber.angle.to_bits(), fiber.risk, fiber.fixes).hash(hasher);
        }
        self.sectors.iter().map(|angle| angle.to_bits()).collect::<Vec<_>>().hash(hasher);
        (self.gaze.0.to_bits(), self.gaze.1.to_bits(), self.typing, self.scroll.to_bits(), self.columns, self.turn.to_bits()).hash(hasher);
        self.code.hash(hasher);
        self.up.hash(hasher);
        self.down.hash(hasher);
        self.sides.hash(hasher);
        for (side, column, reach) in &self.sight {
            (side, column, reach.to_bits()).hash(hasher);
        }
        format!("{:?}", self.rain).hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, split: Option<usize>, rooms: &[usize]) -> Vec<String> {
        let chars: Vec<(char, SkColor)> = text.chars().map(|ch| (ch, SkColor::BLACK)).collect();
        let breaks = breaks(&chars, split);
        for level in 0..=4 {
            let (spans, complete) = wrap(&chars, rooms, &breaks, level);
            if complete {
                return spans.iter().map(|(start, end)| chars[*start..*end].iter().map(|(ch, _)| *ch).collect()).collect();
            }
        }
        Vec::new()
    }

    #[test]
    fn sectors_fill_the_circle_and_nodes_stay_inside() {
        let sectors: Vec<(String, u16)> = [("kernel", 20u16), ("graphics", 12), ("compositor", 0), ("apps", 429)].into_iter().map(|(name, count)| (name.to_string(), count)).collect();
        let spans = sector_spans(&sectors);
        let total: f32 = spans.iter().map(|(_, span)| span).sum();
        assert!((total - TAU).abs() < 1e-3, "{total}");
        assert_eq!(spans[2].1, 0.0);
        for (ring, count) in [(0u8, 3u16), (1, 40), (2, 400)] {
            for slot in 0..count {
                let (x, y) = ring_position(&OrbitNode { ring, sector: 3, slot, count, fixes: false, selected: false, related: false }, &spans);
                let radius = x.hypot(y);
                assert!((0.25..0.9).contains(&radius), "{ring} {slot} {radius}");
            }
        }
    }

    #[test]
    fn pupil_lines_break_between_words() {
        assert_eq!(lines("› systemctl --failed  1 units", Some(20), &[12, 12, 12]), ["› systemctl", "--failed", "1 units"]);
        assert_eq!(lines("› yay -Qua  2 aur", Some(10), &[12, 12]), ["› yay -Qua", "2 aur"]);
        assert_eq!(lines("› /sys/module/nvidia/version", None, &[16, 16]), ["› /sys/module/", "nvidia/version"]);
        assert_eq!(lines("› uname -r  7.2.5-3-omarchy", Some(10), &[30]), ["› uname -r  7.2.5-3-omarchy"]);
    }
}
