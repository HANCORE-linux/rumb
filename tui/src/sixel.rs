use std::fmt::Write;

use ratatui::style::Color;
use tiny_skia::Pixmap;

use crate::palette::{Palette, mix};

pub struct Encoder {
    key: [u32; 15],
    colors: Vec<[u8; 3]>,
    lookup: Vec<u16>,
}

fn rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    }
}

fn key_of(pal: &Palette) -> [u32; 15] {
    let pack = |color: Color| {
        let [r, g, b] = rgb(color);
        u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    };
    [pal.page, pal.panel, pal.kpi, pal.bar, pal.bar_text, pal.ink, pal.ink2, pal.label, pal.faint, pal.rule, pal.grid, pal.teal, pal.select, pal.alert, pal.warn].map(pack)
}

impl Encoder {
    pub fn new(pal: &Palette) -> Self {
        let grounds = [pal.page, mix(pal.page, pal.ink, 0.04), mix(pal.page, pal.ink, 0.09), pal.ink];
        let inks = [pal.ink, pal.ink2, pal.label, pal.faint, pal.rule, pal.grid, pal.alert, pal.warn, pal.teal];
        let mut colors: Vec<[u8; 3]> = Vec::new();
        let mut add = |color: Color| {
            let value = rgb(color);
            if !colors.contains(&value) {
                colors.push(value);
            }
        };
        for ground in grounds {
            add(ground);
        }
        for ink in inks {
            for step in 1..=20 {
                add(mix(pal.page, ink, f64::from(step) / 20.0));
            }
        }
        for ink in [pal.alert, pal.warn, pal.label, pal.ink, pal.teal] {
            for step in 1..=6 {
                add(mix(mix(pal.page, pal.ink, 0.09), ink, f64::from(step) / 7.0));
            }
        }
        for (a, b) in [(pal.alert, pal.ink), (pal.teal, pal.ink), (pal.warn, pal.ink)] {
            for step in 1..=4 {
                add(mix(a, b, f64::from(step) / 5.0));
            }
        }
        colors.truncate(256);
        Encoder { key: key_of(pal), colors, lookup: vec![0; 1 << 18] }
    }

    pub fn fits(&self, pal: &Palette) -> bool {
        self.key == key_of(pal)
    }

    fn index(&mut self, r: u8, g: u8, b: u8) -> u8 {
        let cell = (usize::from(r >> 2) << 12) | (usize::from(g >> 2) << 6) | usize::from(b >> 2);
        let known = self.lookup[cell];
        if known != 0 {
            return (known - 1) as u8;
        }
        let (r, g, b) = (i32::from(r & !3) + 2, i32::from(g & !3) + 2, i32::from(b & !3) + 2);
        let mut best = (0usize, i32::MAX);
        for (index, [pr, pg, pb]) in self.colors.iter().enumerate() {
            let (dr, dg, db) = (r - i32::from(*pr), g - i32::from(*pg), b - i32::from(*pb));
            let distance = 3 * dr * dr + 4 * dg * dg + 2 * db * db;
            if distance < best.1 {
                best = (index, distance);
            }
        }
        self.lookup[cell] = best.0 as u16 + 1;
        best.0 as u8
    }

    pub fn encode(&mut self, pixmap: &Pixmap) -> String {
        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        let pixels = pixmap.data();
        let mut indices = vec![0u8; width * height];
        for (slot, pixel) in indices.iter_mut().zip(pixels.as_chunks::<4>().0) {
            *slot = self.index(pixel[0], pixel[1], pixel[2]);
        }
        let mut used = [false; 256];
        for index in &indices {
            used[usize::from(*index)] = true;
        }
        let mut out = String::with_capacity(width * height / 2);
        let _ = write!(out, "\x1bP0;1;0q\"1;1;{width};{height}");
        for (index, [r, g, b]) in self.colors.iter().enumerate() {
            if used[index] {
                let percent = |value: u8| (u32::from(value) * 100 + 127) / 255;
                let _ = write!(out, "#{index};2;{};{};{}", percent(*r), percent(*g), percent(*b));
            }
        }
        let mut bits = vec![0u8; 256 * width];
        let mut present: Vec<usize> = Vec::with_capacity(64);
        let mut seen = [false; 256];
        for band in (0..height).step_by(6) {
            if band > 0 {
                out.push('-');
            }
            for color in present.drain(..) {
                seen[color] = false;
            }
            for row in 0..6.min(height - band) {
                let line = &indices[(band + row) * width..(band + row + 1) * width];
                for (x, index) in line.iter().enumerate() {
                    let color = usize::from(*index);
                    if !seen[color] {
                        seen[color] = true;
                        present.push(color);
                    }
                    bits[color * width + x] |= 1 << row;
                }
            }
            for (number, color) in present.iter().enumerate() {
                if number > 0 {
                    out.push('$');
                }
                let _ = write!(out, "#{color}");
                let row = &mut bits[color * width..(color + 1) * width];
                let last = row.iter().rposition(|value| *value != 0).map_or(0, |at| at + 1);
                let mut x = 0;
                while x < last {
                    let value = row[x];
                    let mut run = 1;
                    while x + run < last && row[x + run] == value {
                        run += 1;
                    }
                    let symbol = char::from(63 + value);
                    if run > 3 {
                        let _ = write!(out, "!{run}{symbol}");
                    } else {
                        for _ in 0..run {
                            out.push(symbol);
                        }
                    }
                    x += run;
                }
                row.fill(0);
            }
        }
        out.push_str("\x1b\\");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_every_pixel_once_per_band() {
        let pal = Palette::labra();
        let mut pixmap = Pixmap::new(13, 14).unwrap();
        pixmap.fill(tiny_skia::Color::from_rgba8(4, 7, 4, 255));
        let mut encoder = Encoder::new(&pal);
        let text = encoder.encode(&pixmap);
        assert!(text.starts_with("\x1bP0;1;0q\"1;1;13;14"));
        assert!(text.ends_with("\x1b\\"));
        assert_eq!(text.matches('-').count(), 2);
        let again = encoder.encode(&pixmap);
        assert_eq!(text, again);
    }

    #[test]
    fn palette_holds_the_theme_colors() {
        let pal = Palette::labra();
        let mut encoder = Encoder::new(&pal);
        for color in [pal.page, pal.ink, pal.alert, pal.warn, pal.teal] {
            let [r, g, b] = rgb(color);
            let index = encoder.index(r, g, b);
            let [pr, pg, pb] = encoder.colors[usize::from(index)];
            assert!(r.abs_diff(pr) <= 3 && g.abs_diff(pg) <= 3 && b.abs_diff(pb) <= 3, "{color:?}");
        }
        assert!(encoder.colors.len() <= 256);
    }
}
