//! Ink drawings for the dashboard: the astrolabe and small specimen plates.
//! Rendered with tiny-skia at the terminal's real pixel size and shown through
//! its image protocol (sixel in foot, kitty in ghostty).

use std::f32::consts::TAU;

use ratatui::style::Color;
use tiny_skia::{Color as SkColor, FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, Stroke, StrokeDash, Transform};

use crate::app::{App, Subsystem};
use crate::palette::{Palette, mix};

/// Share of half the image size used by the dial radius.
pub const DIAL: f32 = 0.84;

struct Orbit {
    rx: f32,
    ry: f32,
    rotation: f32,
    phase: f32,
}

const ORBITS: [Orbit; 3] = [
    Orbit { rx: 0.36, ry: 0.15, rotation: -0.28, phase: 0.6 },
    Orbit { rx: 0.58, ry: 0.25, rotation: 0.38, phase: 2.1 },
    Orbit { rx: 0.80, ry: 0.34, rotation: -0.12, phase: 3.7 },
];

pub struct Node {
    pub name: String,
    /// 0 high, 1 medium, 2 low
    pub risk: u8,
    pub subsystem: Subsystem,
    /// Position in dial units: the dial has radius 1, y points down.
    pub x: f32,
    pub y: f32,
    pub fixes: u64,
}

pub struct Model {
    pub nodes: Vec<Node>,
}

impl Model {
    pub fn new(app: &App) -> Self {
        let updates = app.sorted_updates();
        let mut nodes = Vec::new();
        for (orbit_index, orbit) in ORBITS.iter().enumerate() {
            let members: Vec<_> = updates.iter().filter(|update| update.risk_rank() as usize == orbit_index).collect();
            let count = members.len().max(1) as f32;
            for (k, update) in members.iter().enumerate() {
                let angle = orbit.phase + k as f32 * TAU / count;
                let (px, py) = (orbit.rx * angle.cos(), orbit.ry * angle.sin());
                let (sin, cos) = orbit.rotation.sin_cos();
                let fixes = app
                    .problems
                    .iter()
                    .filter(|problem| problem.status == "possibly-fixed" && problem.package == update.name)
                    .map(|problem| problem.count)
                    .sum();
                nodes.push(Node {
                    name: update.name.clone(),
                    risk: orbit_index as u8,
                    subsystem: Subsystem::of(&update.name),
                    x: px * cos - py * sin,
                    y: px * sin + py * cos,
                    fixes,
                });
            }
        }
        Model { nodes }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// A label beside the image; its leader line runs from the node to the image
/// edge at the label's first row.
#[derive(Debug)]
pub struct Callout {
    pub node: usize,
    pub side: Side,
    pub row: u16,
    pub alpha: f32,
}

#[derive(Debug)]
pub struct Anim {
    pub dial: f32,
    pub orbits: f32,
    pub nodes: Vec<f32>,
    pub links: f32,
    pub spotlight: Option<Subsystem>,
    pub spot_alpha: f32,
    /// Angle and strength of the scan sweep while the scan runs.
    pub sweep: Option<(f32, f32)>,
}

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

/// Deterministic pseudo-random numbers for decoration.
fn noise(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

fn canvas(width: u32, height: u32, background: Color, cell_height: f32) -> Option<(Pixmap, f32)> {
    let mut pixmap = Pixmap::new(width.max(1), height.max(1))?;
    pixmap.fill(sk(background, 1.0));
    Some((pixmap, (cell_height / 21.0).max(0.6)))
}

/// The astrolabe: every pending package is a node on an orbit, riskier
/// changes orbit closer to the core.
pub fn astrolabe(model: &Model, anim: &Anim, callouts: &[Callout], pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, pal.panel, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let c = (w / 2.0, h / 2.0);
    let r = w.min(h) / 2.0 * DIAL;
    let at = |x: f32, y: f32| (c.0 + x * r, c.1 + y * r);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let ink = |alpha: f32| sk(pal.ink, alpha);

    // Dust drifting around the instrument
    for i in 0..170u32 {
        let angle = noise(i * 3) * TAU;
        let distance = 0.45 + noise(i * 3 + 1) * 0.75;
        let (x, y) = at(angle.cos() * distance * 1.25, angle.sin() * distance * 0.8);
        let size = 0.5 + noise(i * 3 + 2) * 1.4;
        pen.disc(x, y, size * unit, ink(0.12 + noise(i * 7) * 0.45 * anim.dial));
    }

    // Plate: almucantar arcs and hour lines, engraved faintly
    for k in 0..7 {
        let offset = r * 0.04 * k as f32;
        let radius = r * (0.88 - 0.1 * k as f32);
        pen.circle(c.0, c.1 + offset, radius, ink(0.10 * anim.orbits), 0.7, None);
    }
    for k in 0..24 {
        let angle = k as f32 * TAU / 24.0;
        let (sin, cos) = angle.sin_cos();
        pen.line((c.0 + cos * r * 0.22, c.1 + sin * r * 0.22), (c.0 + cos * r * 0.92, c.1 + sin * r * 0.92), ink(0.06 * anim.orbits), 0.6);
    }

    // Ecliptic ring, offset like on an astrolabe rete
    let ecliptic = (c.0, c.1 - r * 0.2);
    pen.circle(ecliptic.0, ecliptic.1, r * 0.6, ink(0.35 * anim.orbits), 0.8, None);
    pen.circle(ecliptic.0, ecliptic.1, r * 0.66, ink(0.25 * anim.orbits), 0.6, None);
    for k in 0..72 {
        let angle = k as f32 * TAU / 72.0;
        let (sin, cos) = angle.sin_cos();
        let inner = if k % 6 == 0 { 0.6 } else { 0.63 };
        pen.line(
            (ecliptic.0 + cos * r * inner, ecliptic.1 + sin * r * inner),
            (ecliptic.0 + cos * r * 0.66, ecliptic.1 + sin * r * 0.66),
            ink(0.3 * anim.orbits),
            0.6,
        );
    }

    // Limb: double ring with a degree scale that sweeps in from the top
    pen.circle(c.0, c.1, r, ink(0.85 * anim.dial), 1.0, None);
    pen.circle(c.0, c.1, r * 1.035, ink(0.85 * anim.dial), 1.0, None);
    pen.circle(c.0, c.1, r * 0.93, ink(0.3 * anim.dial), 0.7, None);
    let ticks = 180;
    for k in 0..ticks {
        if k as f32 / ticks as f32 > anim.dial {
            break;
        }
        let angle = -TAU / 4.0 + k as f32 * TAU / ticks as f32;
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
        pen.line((x - arm, y), (x + arm, y), ink(0.7 * anim.dial), 0.8);
        pen.line((x, y - arm), (x, y + arm), ink(0.7 * anim.dial), 0.8);
    }

    // Orbits
    for (index, orbit) in ORBITS.iter().enumerate() {
        let reveal = ((anim.orbits - index as f32 * 0.18) / 0.64).clamp(0.0, 1.0);
        if reveal > 0.0 {
            let dash = if index == 1 { Some((3.0, 2.5)) } else { None };
            pen.ellipse(c, orbit.rx * r, orbit.ry * r, orbit.rotation, orbit.phase, reveal, ink(0.55), 0.9, dash);
        }
    }

    // Scan sweep
    if let Some((angle, strength)) = anim.sweep {
        for i in 0..24 {
            let a = angle - i as f32 * 0.04;
            let (sin, cos) = a.sin_cos();
            let alpha = strength * 0.35 * (1.0 - i as f32 / 24.0).powi(2);
            pen.line(c, (c.0 + cos * r, c.1 + sin * r), sk(pal.teal, alpha), 1.0);
        }
    }

    // Core: an engraved sphere with hatching
    let core = r * 0.07;
    for k in -6..=6 {
        let offset = k as f32 / 7.0 * core;
        let half = (core * core - offset * offset).max(0.0).sqrt();
        pen.line((c.0 - half + offset * 0.4, c.1 + offset), (c.0 + half + offset * 0.4, c.1 + offset), ink(0.55), 0.6);
    }
    pen.circle(c.0, c.1, core, ink(0.9), 1.0, None);
    pen.circle(c.0, c.1, core * 2.0, ink(0.35), 0.7, Some((2.0, 2.0)));

    // Fix links out to the rim
    for (index, node) in model.nodes.iter().enumerate() {
        if node.fixes == 0 || anim.links <= 0.0 {
            continue;
        }
        let alpha = anim.links * anim.nodes.get(index).copied().unwrap_or(0.0);
        let length = (node.x * node.x + node.y * node.y).sqrt().max(0.001);
        let (ux, uy) = (node.x / length, node.y / length);
        let reach = length + (1.08 - length) * anim.links;
        pen.dashed(at(node.x, node.y), at(ux * reach, uy * reach), sk(pal.teal, alpha), 1.1, (3.0, 2.5));
    }

    // Leader lines to the labels
    for callout in callouts {
        let Some(node) = model.nodes.get(callout.node) else { continue };
        if callout.alpha <= 0.0 {
            continue;
        }
        let from = at(node.x, node.y);
        let edge_y = (f32::from(callout.row) + 0.5) * cell_height;
        let edge_x = if callout.side == Side::Left { 0.0 } else { w };
        let elbow_x = from.0 + (edge_x - from.0) * 0.3;
        pen.line(from, (elbow_x, edge_y), ink(0.8 * callout.alpha), 0.8);
        pen.line((elbow_x, edge_y), (edge_x, edge_y), ink(0.8 * callout.alpha), 0.8);
    }

    // Nodes
    for (index, node) in model.nodes.iter().enumerate() {
        let alpha = anim.nodes.get(index).copied().unwrap_or(0.0);
        if alpha <= 0.0 {
            continue;
        }
        let (x, y) = at(node.x, node.y);
        pen.disc(x, y, 6.5 * unit, sk(pal.panel, alpha));
        match node.risk {
            0 => {
                pen.disc(x, y, 4.2 * unit, ink(alpha));
                pen.circle(x, y, 7.5 * unit, ink(alpha * 0.8), 0.9, None);
            }
            1 => {
                pen.disc(x, y, 3.4 * unit, sk(pal.ink2, alpha));
                pen.disc(x, y, 1.2 * unit, sk(pal.panel, alpha));
            }
            _ => pen.circle(x, y, 3.0 * unit, sk(pal.label, alpha), 1.0, None),
        }
        if anim.spotlight == Some(node.subsystem) && anim.spot_alpha > 0.0 {
            pen.circle(x, y, 11.0 * unit, sk(pal.teal, alpha * anim.spot_alpha), 1.4, None);
        }
        if node.fixes > 0 {
            pen.circle(x, y, 15.0 * unit, sk(pal.teal, alpha * anim.links * 0.8), 0.9, Some((2.0, 2.0)));
        }
    }

    Some(pixmap)
}

/// A small specimen plate for a subsystem, in the same engraved line work.
pub fn specimen(subsystem: Subsystem, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, pal.panel, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let ink = |alpha: f32| sk(pal.ink, alpha);
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

    Some(pixmap)
}

/// Seven-segment digits drawn as strokes, for numbers inside images.
fn segments(pen: &mut Pen, text: &str, x: f32, y: f32, height: f32, color: SkColor) {
    let w = height * 0.5;
    let mut cursor = x;
    for ch in text.chars() {
        let on: &[u8] = match ch {
            '0' => &[0, 1, 2, 4, 5, 6],
            '1' => &[2, 5],
            '2' => &[0, 2, 3, 4, 6],
            '3' => &[0, 2, 3, 5, 6],
            '4' => &[1, 2, 3, 5],
            '5' => &[0, 1, 3, 5, 6],
            '6' => &[0, 1, 3, 4, 5, 6],
            '7' => &[0, 2, 5],
            '8' => &[0, 1, 2, 3, 4, 5, 6],
            '9' => &[0, 1, 2, 3, 5, 6],
            '.' => {
                pen.disc(cursor + w * 0.15, y + height, height * 0.06, color);
                cursor += w * 0.45;
                continue;
            }
            _ => {
                cursor += w * 0.8;
                continue;
            }
        };
        let (l, r, t, m, b) = (cursor, cursor + w, y, y + height / 2.0, y + height);
        let lines = [((l, t), (r, t)), ((l, t), (l, m)), ((r, t), (r, m)), ((l, m), (r, m)), ((l, m), (l, b)), ((r, m), (r, b)), ((l, b), (r, b))];
        for &segment in on {
            let (a, z) = lines[segment as usize];
            pen.line(a, z, color, 1.3);
        }
        cursor += w * 1.45;
    }
}

/// Ring gauge with the share of packages that are up to date.
pub fn gauge(fraction: f32, label: &str, pal: &Palette, width: u32, height: u32, cell_height: f32, reveal: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, pal.panel, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = w.min(h) * 0.4;
    pen.circle(cx, cy, r, sk(pal.grid, 1.0), 3.2, None);
    let sweep = fraction.clamp(0.0, 1.0) * reveal;
    if sweep > 0.0 {
        let mut builder = PathBuilder::new();
        let steps = 160;
        for s in 0..=((steps as f32 * sweep) as usize) {
            let angle = -TAU / 4.0 + s as f32 * TAU / steps as f32;
            let (x, y) = (cx + angle.cos() * r, cy + angle.sin() * r);
            if s == 0 {
                builder.move_to(x, y);
            } else {
                builder.line_to(x, y);
            }
        }
        pen.stroke(builder, sk(pal.teal_mid, 1.0), 3.2, None);
    }
    for k in 0..40 {
        let angle = k as f32 * TAU / 40.0;
        let (sin, cos) = angle.sin_cos();
        pen.line((cx + cos * r * 1.18, cy + sin * r * 1.18), (cx + cos * r * 1.26, cy + sin * r * 1.26), sk(pal.ink, 0.35), 0.6);
    }
    let digit_h = r * 0.5;
    let text_w = label.chars().map(|c| if c == '.' { 0.45 } else { 1.45 }).sum::<f32>() * digit_h * 0.5;
    segments(&mut pen, label, cx - text_w / 2.0 + digit_h * 0.1, cy - digit_h / 2.0, digit_h, sk(pal.teal, reveal));
    Some(pixmap)
}

/// Dependency molecule: what a package depends on (left) and what requires it
/// (right), as a small node graph.
#[allow(clippy::too_many_arguments)]
pub fn molecule(depends: u64, required_by: u64, seed: u32, pal: &Palette, width: u32, height: u32, cell_height: f32, reveal: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, pal.panel, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };
    let (cx, cy) = (w / 2.0, h / 2.0);
    for (count, side, color) in [(depends, -1.0f32, pal.ink2), (required_by, 1.0, pal.teal)] {
        let shown = count.min(22) as u32;
        for i in 0..shown {
            let t = (i as f32 + 0.5) / shown.max(1) as f32;
            let angle = (t - 0.5) * 2.4;
            let distance = 0.55 + noise(seed + i * 5 + (side > 0.0) as u32 * 97) * 0.4;
            let (x, y) = (cx + side * angle.cos() * distance * w * 0.45, cy + angle.sin() * distance * h * 0.42);
            let alpha = ((reveal * shown as f32 - i as f32) / 2.0).clamp(0.0, 1.0);
            pen.line((cx, cy), (x, y), sk(color, 0.35 * alpha), 0.6);
            pen.disc(x, y, (1.4 + noise(seed + i) * 1.4) * unit, sk(color, 0.85 * alpha));
        }
    }
    pen.disc(cx, cy, 4.5 * unit, sk(pal.ink, reveal.max(0.3)));
    pen.circle(cx, cy, 8.0 * unit, sk(pal.ink, 0.5), 0.8, None);
    Some(pixmap)
}

// --- The eye ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    #[allow(dead_code)]
    Right,
    Top,
    Bottom,
}

/// Where a line leaves the picture: from a corner of the eye to the left or
/// right edge, or from the top of the lid up to the top edge.
#[derive(Debug)]
pub struct EyePort {
    pub edge: Edge,
    /// Cell index along that edge.
    pub cell: u16,
    pub focused: bool,
    /// Drawn as heavy as the life line it continues.
    pub heavy: bool,
    /// A signal on its way out: 0 at the eye, 1 at the picture edge.
    pub pulse: Option<f32>,
}

/// One pending update.
#[derive(Debug)]
pub struct Fiber {
    pub angle: f32,
    /// 0 high, 1 medium, 2 low
    pub risk: u8,
    pub fixes: bool,
    pub reveal: f32,
    /// 1 once the update is installed and its fiber has withdrawn.
    pub retract: f32,
}

/// A line on the screen the pupil becomes while the system changes.
#[derive(Debug)]
pub struct ScreenLine {
    pub text: String,
    /// Lines above the newest one; the newest slides in from -1 to 0.
    pub y: f32,
    pub alpha: f32,
}

#[derive(Debug)]
pub struct EyeSpec {
    pub cols: u16,
    pub rows: u16,
    pub fibers: Vec<Fiber>,
    /// Angles between the subsystem groups of fibers.
    pub sectors: Vec<f32>,
    /// Where the eye looks, as a unit vector; (0, 0) is straight ahead.
    pub gaze: (f32, f32),
    /// 1 open, 0 closed.
    pub open: f32,
    /// Reveal when the monitor starts.
    pub ring: f32,
    pub sweep: Option<(f32, f32)>,
    pub ports: Vec<EyePort>,
    /// 0 idle, 1 working: the pupil widens into a screen.
    pub work: f32,
    pub screen: Vec<ScreenLine>,
    pub cursor: bool,
}

fn arc(center: (f32, f32), radius: f32, from: f32, to: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|s| {
            let angle = from + (to - from) * s as f32 / steps as f32;
            (center.0 + radius * angle.cos(), center.1 + radius * angle.sin())
        })
        .collect()
}

/// A lid from the left corner to the right one; its apex sits `depth` above
/// (negative) or below (positive) the centre.
fn lid(c: (f32, f32), half_width: f32, depth: f32, steps: usize) -> Vec<(f32, f32)> {
    let bulge = depth.abs().max(0.01);
    let radius = (half_width * half_width + bulge * bulge) / (2.0 * bulge);
    let half = (half_width / radius).clamp(-1.0, 1.0).asin();
    if depth < 0.0 {
        arc((c.0, c.1 - bulge + radius), radius, -TAU / 4.0 - half, -TAU / 4.0 + half, steps)
    } else {
        arc((c.0, c.1 + bulge - radius), radius, TAU / 4.0 + half, TAU / 4.0 - half, steps)
    }
}

fn length(points: &[(f32, f32)]) -> f32 {
    points.windows(2).map(|p| ((p[1].0 - p[0].0).powi(2) + (p[1].1 - p[0].1).powi(2)).sqrt()).sum()
}

/// The part of a polyline between two distances from its start.
fn segment(points: &[(f32, f32)], from: f32, to: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut walked = 0.0;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        if len <= 0.0 {
            continue;
        }
        let (lo, hi) = (from.max(walked), to.min(walked + len));
        if lo < hi {
            let at = |d: f32| (a.0 + (b.0 - a.0) * (d - walked) / len, a.1 + (b.1 - a.1) * (d - walked) / len);
            if out.is_empty() {
                out.push(at(lo));
            }
            out.push(at(hi));
        }
        walked += len;
    }
    out
}

/// The eye. Its iris fibers are the pending updates; the riskier a change,
/// the deeper its fiber reaches towards the pupil. It fills the picture: a
/// small emblem in the header, or large in the live view.
pub fn eye(spec: &EyeSpec, pal: &Palette, width: u32, height: u32, cell_height: f32) -> Option<Pixmap> {
    let (mut pixmap, unit) = canvas(width, height, pal.page, cell_height)?;
    let (w, h) = (width as f32, height as f32);
    let cell_w = w / f32::from(spec.cols.max(1));
    let cell_h = h / f32::from(spec.rows.max(1));
    let c = (w / 2.0, h / 2.0);
    let fit = if w > 400.0 { 0.8 } else { 0.96 };
    let half_width = (w * 0.47).min(h * 0.47 / 0.608) * fit;
    let (upper_depth, lower_depth) = (half_width * 0.608, half_width * 0.5);
    let ring = spec.ring.clamp(0.0, 1.0);
    let ink = |alpha: f32| sk(pal.ink, alpha);
    let mut pen = Pen { pixmap: &mut pixmap, unit, mask: None };

    // Lids that hug the iris
    let open = spec.open.clamp(0.0, 1.0);
    let upper = lid(c, half_width, -(upper_depth * open).max(half_width * 0.01), 72);
    let lower = lid(c, half_width, (lower_depth * open).max(half_width * 0.01), 72);
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
    if let Some(path) = builder.finish() {
        let mut paint = Paint::default();
        paint.set_color(sk(pal.panel, 1.0));
        paint.anti_alias = true;
        pen.pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        pen.mask = Mask::new(width, height).map(|mut mask| {
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
            mask
        });
    }

    // Stipple shading towards the corners
    let dots = (half_width * upper_depth / 18.0).clamp(40.0, 900.0) as u32;
    for i in 0..dots {
        let x = c.0 + (noise(i * 5 + 11) * 2.0 - 1.0) * half_width;
        let y = c.1 + (noise(i * 5 + 12) * 2.0 - 1.0) * upper_depth;
        let edge = ((x - c.0).abs() / half_width).powi(2);
        pen.disc(x, y, (0.45 + noise(i * 5 + 13) * 0.6) * unit, ink((0.04 + 0.24 * edge) * ring));
    }

    // Iris; the pupil widens while the system changes
    let work = spec.work.clamp(0.0, 1.0);
    let ri = half_width * 0.54;
    let rp = ri * (0.34 + 0.56 * work);
    let ic = (c.0 + spec.gaze.0 * half_width * 0.27, c.1 + spec.gaze.1 * half_width * 0.07);
    pen.disc(ic.0, ic.1, ri, sk(mix(pal.panel, pal.teal_fill, 0.6), 1.0));
    let base = (ri * 2.4).clamp(60.0, 300.0) as u32;
    for k in 0..base {
        let angle = k as f32 / base as f32 * TAU + (noise(k * 3 + 1) - 0.5) * 0.02;
        let bend = (noise(k * 3 + 2) - 0.5) * 0.09;
        let (r0, r1) = (rp * 1.15, ri * (0.88 + 0.1 * noise(k * 3 + 3)));
        let mid = (r0 + r1) / 2.0;
        let mut fiber = PathBuilder::new();
        fiber.move_to(ic.0 + angle.cos() * r0, ic.1 + angle.sin() * r0);
        fiber.quad_to(ic.0 + (angle + bend).cos() * mid, ic.1 + (angle + bend).sin() * mid, ic.0 + angle.cos() * r1, ic.1 + angle.sin() * r1);
        pen.stroke(fiber, ink((0.12 + 0.16 * noise(k * 3 + 4)) * ring), 0.55, None);
    }
    let collarette: Vec<(f32, f32)> = (0..=160u32)
        .map(|s| {
            let angle = s as f32 / 160.0 * TAU;
            let radius = ri * 0.56 + (angle * 11.0 + 0.7).sin() * ri * 0.022 + (noise((s % 160) * 7 + 5) - 0.5) * ri * 0.025;
            (ic.0 + angle.cos() * radius, ic.1 + angle.sin() * radius)
        })
        .collect();
    pen.polyline(&collarette, ink(0.42 * ring), 0.8);

    // Pending updates as fibers
    for fiber in &spec.fibers {
        if fiber.reveal <= 0.0 || fiber.retract >= 1.0 {
            continue;
        }
        let (sin, cos) = fiber.angle.sin_cos();
        let outer = ri * 0.97;
        let inner = match fiber.risk {
            0 => ri * 0.36,
            1 => ri * 0.55,
            _ => ri * 0.74,
        }
        .max(rp * 1.02);
        let reach = outer - (outer - inner) * fiber.reveal * (1.0 - fiber.retract);
        let (color, width) = match fiber.risk {
            0 => (pal.ink, 2.4),
            1 => (pal.ink2, 1.8),
            _ => (pal.label, 1.2),
        };
        let color = if fiber.fixes { pal.teal } else { color };
        pen.line((ic.0 + cos * outer, ic.1 + sin * outer), (ic.0 + cos * reach, ic.1 + sin * reach), sk(color, 0.95), width);
        if fiber.fixes {
            pen.disc(ic.0 + cos * ri * 1.08, ic.1 + sin * ri * 1.08, (ri / 50.0).clamp(1.2, 3.0) * unit, sk(pal.teal, fiber.reveal));
        }
    }
    if !spec.fibers.is_empty() {
        for angle in &spec.sectors {
            let (sin, cos) = angle.sin_cos();
            pen.line((ic.0 + cos * ri * 0.9, ic.1 + sin * ri * 0.9), (ic.0 + cos * ri * 1.04, ic.1 + sin * ri * 1.04), ink(0.75), 0.8);
        }
    }

    pen.circle(ic.0, ic.1, ri, ink(0.9 * ring), 1.5, None);
    pen.circle(ic.0, ic.1, ri * 1.025, ink(0.25 * ring), 0.6, None);
    pen.circle(ic.0, ic.1, rp * 1.14, ink(0.45), 0.6, None);
    // The pupil stays the darkest thing in the eye on any page, the
    // catchlight the brightest
    let (pupil, light) = if pal.dark { (pal.page, pal.ink) } else { (pal.ink, pal.panel) };
    pen.disc(ic.0, ic.1, rp, sk(pupil, 1.0));
    let catchlight = 1.0 - work;
    pen.disc(ic.0 - rp * 0.34, ic.1 - rp * 0.38, rp * 0.24 * catchlight, sk(light, catchlight));
    pen.disc(ic.0 + rp * 0.36, ic.1 + rp * 0.3, rp * 0.08 * catchlight, sk(light, 0.85 * catchlight));

    // The screen: log lines rising through the widened pupil, when it is
    // large enough to read
    if work > 0.0 && rp * 0.9 >= 36.0 {
        let saved = pen.mask.clone();
        if let (Some(mask), Some(circle)) = (pen.mask.as_mut(), PathBuilder::from_circle(ic.0, ic.1, rp * 0.9)) {
            mask.intersect_path(&circle, FillRule::Winding, true, Transform::identity());
        }
        let dot = (ri / 60.0).round().max(1.0);
        let line_h = 9.0 * dot;
        let baseline = ic.1 + rp * 0.58;
        let last = spec.screen.len().saturating_sub(1);
        for (index, line) in spec.screen.iter().enumerate() {
            let color = match (index == last, pal.dark) {
                (true, true) => pal.ink,
                (false, true) => pal.ink2,
                (true, false) => pal.panel,
                (false, false) => pal.teal_light,
            };
            let y = baseline - line.y * line_h;
            let end = dot_text(&mut pen, &line.text, ic.0, y, dot, sk(color, line.alpha * work));
            if index == last && spec.cursor {
                pen.fill_rect(end + dot, (y - 7.0 * dot).round(), 5.0 * dot, 7.0 * dot, sk(if pal.dark { pal.teal } else { pal.teal_light }, work));
            }
        }
        pen.mask = saved;
    }
    if work > 0.0 {
        pen.circle(ic.0, ic.1, rp * 1.03, sk(pal.teal, 0.6 * work), 0.9, None);
    }

    if let Some((angle, strength)) = spec.sweep {
        for i in 0..24 {
            let a = angle - i as f32 * 0.04;
            let (sin, cos) = a.sin_cos();
            let alpha = strength * 0.45 * (1.0 - i as f32 / 24.0).powi(2);
            pen.line((ic.0 + cos * rp, ic.1 + sin * rp), (ic.0 + cos * ri, ic.1 + sin * ri), sk(pal.teal, alpha), 1.2);
        }
    }
    pen.mask = None;

    // Lids and creases
    pen.polyline(&upper, ink(0.95 * ring), 1.5);
    pen.polyline(&lower, ink(0.8 * ring), 1.1);
    let offset = |points: &[(f32, f32)], lift: f32, span: (f32, f32)| -> Vec<(f32, f32)> {
        let last = (points.len() - 1) as f32;
        points
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let t = i as f32 / last;
                (t >= span.0 && t <= span.1).then(|| (p.0, p.1 + lift * (std::f32::consts::PI * t).sin()))
            })
            .collect()
    };
    pen.polyline(&offset(&upper, -half_width * 0.1, (0.12, 0.88)), ink(0.32 * ring), 0.8);
    pen.polyline(&offset(&lower, half_width * 0.06, (0.22, 0.78)), ink(0.18 * ring), 0.7);

    // Lines out of the eye: from its corners to the side edges, or from the
    // top of the lid to the top edge. A heavy line sits where a terminal
    // draws its heavy box line, two pixels around the cell's middle.
    for port in &spec.ports {
        let index = f32::from(port.cell);
        let across = |size: f32| if port.heavy { index * size + (size / 2.0).floor() } else { index * size + (size / 2.0).floor() + 0.5 };
        let path = match port.edge {
            Edge::Left => {
                let y = across(cell_h);
                vec![(c.0 - half_width, c.1), (c.0 - half_width - 4.0 * unit, y), (0.0, y)]
            }
            Edge::Right => {
                let y = across(cell_h);
                vec![(c.0 + half_width, c.1), (c.0 + half_width + 4.0 * unit, y), (w, y)]
            }
            Edge::Top => {
                let x = across(cell_w);
                vec![(x, c.1 - upper_depth * open), (x, 0.0)]
            }
            Edge::Bottom => {
                let x = across(cell_w);
                vec![(x, c.1 + lower_depth * open), (x, h)]
            }
        };
        let color = if port.focused {
            pal.teal
        } else if port.heavy {
            pal.ink2
        } else {
            pal.grid
        };
        pen.polyline(&path, sk(color, 1.0), if port.heavy { 2.0 / unit } else { 1.0 });
        if let Some(t) = port.pulse {
            let head = length(&path) * t.clamp(0.0, 1.0);
            let piece = segment(&path, (head - 12.0 * unit).max(0.0), head);
            if piece.len() >= 2 {
                pen.polyline(&piece, sk(pal.teal, 1.0), 2.4);
            }
        }
    }

    Some(pixmap)
}

/// A 5x7 dot-matrix font for the screen in the pupil; rows top to bottom,
/// five bits each, the highest bit on the left.
const FONT: [(char, [u8; 7]); 56] = [
    ('A', [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('B', [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E]),
    ('C', [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E]),
    ('D', [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C]),
    ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
    ('F', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10]),
    ('G', [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F]),
    ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
    ('I', [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('J', [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C]),
    ('K', [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11]),
    ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
    ('M', [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11]),
    ('N', [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11]),
    ('O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('P', [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10]),
    ('Q', [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D]),
    ('R', [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11]),
    ('S', [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E]),
    ('T', [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04]),
    ('U', [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
    ('V', [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A]),
    ('X', [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11]),
    ('Y', [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04]),
    ('Z', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F]),
    ('0', [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E]),
    ('1', [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E]),
    ('2', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F]),
    ('3', [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E]),
    ('4', [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02]),
    ('5', [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E]),
    ('6', [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E]),
    ('7', [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08]),
    ('8', [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E]),
    ('9', [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C]),
    ('.', [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C]),
    (',', [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08]),
    ('-', [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00]),
    ('>', [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08]),
    ('<', [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02]),
    ('(', [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02]),
    (')', [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08]),
    (':', [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00]),
    ('/', [0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x00]),
    ('_', [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F]),
    ('+', [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00]),
    ('[', [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E]),
    (']', [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E]),
    ('=', [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00]),
    ('\'', [0x0C, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00]),
    ('?', [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04]),
    ('!', [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04]),
    ('#', [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A]),
    ('*', [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00]),
    ('%', [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03]),
];

/// Draws dot-matrix text centred on `center_x` with its baseline at `baseline`;
/// returns where the text ends.
fn dot_text(pen: &mut Pen, text: &str, center_x: f32, baseline: f32, dot: f32, color: SkColor) -> f32 {
    let advance = 6.0 * dot;
    let width = text.chars().count() as f32 * advance - dot;
    let x0 = (center_x - width / 2.0).round();
    let y0 = (baseline - 7.0 * dot).round();
    for (index, ch) in text.chars().enumerate() {
        let Some((_, rows)) = FONT.iter().find(|(c, _)| *c == ch.to_ascii_uppercase()) else { continue };
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5u8 {
                if bits & (0x10 >> col) != 0 {
                    pen.fill_rect(x0 + index as f32 * advance + f32::from(col) * dot, y0 + row as f32 * dot, dot, dot, color);
                }
            }
        }
    }
    x0 + width
}

/// A brand mark from a picture, drawn in ink on the page.
pub fn mark(source: &Pixmap, pal: &Palette, width: u32, height: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(width.max(1), height.max(1))?;
    out.fill(sk(pal.page, 1.0));
    let mut layer = Pixmap::new(width.max(1), height.max(1))?;
    let scale = (width as f32 / source.width() as f32).min(height as f32 / source.height() as f32);
    let (drawn_w, drawn_h) = (source.width() as f32 * scale, source.height() as f32 * scale);
    let transform = Transform::from_row(scale, 0.0, 0.0, scale, ((width as f32 - drawn_w) / 2.0).round(), ((height as f32 - drawn_h) / 2.0).round());
    let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..tiny_skia::PixmapPaint::default() };
    layer.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
    let mask = Mask::from_pixmap(layer.as_ref(), tiny_skia::MaskType::Alpha);
    let mut ink = Paint::default();
    ink.set_color(sk(pal.ink, 1.0));
    out.fill_rect(tiny_skia::Rect::from_xywh(0.0, 0.0, width as f32, height as f32)?, &ink, Transform::identity(), Some(&mask));
    Some(out)
}
