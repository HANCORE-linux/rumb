use std::fmt::Write;

use ratatui::style::Color;
use tiny_skia::Pixmap;

use crate::palette::{Palette, mix};

pub struct Encoder {
    key: [u32; 17],
    colors: Vec<[u8; 3]>,
    lookup: Vec<u16>,
}

fn rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [0, 0, 0],
    }
}

fn key_of(pal: &Palette) -> [u32; 17] {
    let pack = |color: Color| {
        let [r, g, b] = rgb(color);
        u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
    };
    [pal.page, pal.panel, pal.kpi, pal.bar, pal.bar_text, pal.ink, pal.ink2, pal.label, pal.faint, pal.rule, pal.grid, pal.teal, pal.select, pal.alert, pal.warn, pal.relation, pal.options].map(pack)
}

impl Encoder {
    pub fn new(pal: &Palette) -> Self {
        let grounds = [pal.page, mix(pal.page, pal.ink, 0.04), mix(pal.page, pal.ink, 0.09), pal.ink];
        let inks = [pal.ink, pal.ink2, pal.label, pal.faint, pal.rule, pal.grid, pal.alert, pal.warn, pal.teal, pal.relation];
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
        for step in 1..=10 {
            add(mix(pal.page, pal.options, f64::from(step) / 10.0));
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
        let mut spans = [(0usize, 0usize); 256];
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
                    if seen[color] {
                        let span = &mut spans[color];
                        span.0 = span.0.min(x);
                        span.1 = span.1.max(x);
                    } else {
                        seen[color] = true;
                        present.push(color);
                        spans[color] = (x, x);
                    }
                    bits[color * width + x] |= 1 << row;
                }
            }
            for (number, color) in present.iter().enumerate() {
                if number > 0 {
                    out.push('$');
                }
                out.push('#');
                decimal(&mut out, *color);
                let (first, last) = spans[*color];
                let row = &mut bits[color * width..(color + 1) * width];
                run(&mut out, first, 0);
                let mut x = first;
                while x <= last {
                    let value = row[x];
                    let mut count = 1;
                    while x + count <= last && row[x + count] == value {
                        count += 1;
                    }
                    run(&mut out, count, value);
                    x += count;
                }
                row[first..=last].fill(0);
            }
        }
        out.push_str("\x1b\\");
        out
    }
}

fn run(out: &mut String, count: usize, value: u8) {
    let symbol = char::from(63 + value);
    if count > 3 {
        out.push('!');
        decimal(out, count);
        out.push(symbol);
    } else {
        for _ in 0..count {
            out.push(symbol);
        }
    }
}

fn decimal(out: &mut String, mut value: usize) {
    let mut digits = [0u8; 20];
    let mut length = 0;
    loop {
        digits[length] = b'0' + (value % 10) as u8;
        length += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in digits[..length].iter().rev() {
        out.push(char::from(*digit));
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

    fn decode(text: &str, width: usize, height: usize) -> Vec<u8> {
        let mut out = vec![u8::MAX; width * height];
        let bytes = text.as_bytes();
        let mut i = text.find('#').unwrap();
        let (mut x, mut band, mut color) = (0usize, 0usize, 0usize);
        let number = |i: &mut usize| {
            let start = *i;
            while *i < bytes.len() && bytes[*i].is_ascii_digit() {
                *i += 1;
            }
            text[start..*i].parse::<usize>().unwrap_or(0)
        };
        let mut paint = |x: usize, band: usize, color: usize, value: u8| {
            for bit in 0..6 {
                let y = band * 6 + bit;
                if value & (1 << bit) != 0 && x < width && y < height {
                    out[y * width + x] = color as u8;
                }
            }
        };
        while i < bytes.len() {
            match bytes[i] {
                0x1b => break,
                b'#' => {
                    i += 1;
                    color = number(&mut i);
                    while i < bytes.len() && (bytes[i] == b';' || bytes[i].is_ascii_digit()) {
                        i += 1;
                    }
                }
                b'!' => {
                    i += 1;
                    let count = number(&mut i);
                    for _ in 0..count {
                        paint(x, band, color, bytes[i] - 63);
                        x += 1;
                    }
                    i += 1;
                }
                b'$' => {
                    x = 0;
                    i += 1;
                }
                b'-' => {
                    x = 0;
                    band += 1;
                    i += 1;
                }
                value @ b'?'..=b'~' => {
                    paint(x, band, color, value - 63);
                    x += 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        out
    }

    #[test]
    fn every_pixel_decodes_to_its_palette_entry() {
        let pal = Palette::labra();
        let mut encoder = Encoder::new(&pal);
        let (width, height) = (41usize, 23usize);
        let mut pixmap = Pixmap::new(width as u32, height as u32).unwrap();
        let tones = [pal.page, pal.ink, pal.alert, pal.warn, pal.teal, mix(pal.page, pal.ink, 0.3)];
        for (i, pixel) in pixmap.pixels_mut().iter_mut().enumerate() {
            let (x, y) = (i % width, i / width);
            let tone = if x == 0 || x == width - 1 {
                tones[(y / 4) % tones.len()]
            } else if (5..9).contains(&x) && (2..11).contains(&y) {
                tones[2]
            } else if (30..33).contains(&x) && y > 12 {
                tones[3 + (y % 3)]
            } else {
                tones[0]
            };
            let [r, g, b] = rgb(tone);
            *pixel = tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, 255).unwrap();
        }
        let text = encoder.encode(&pixmap);
        let decoded = decode(&text, width, height);
        for (i, pixel) in pixmap.pixels().iter().enumerate() {
            assert_eq!(decoded[i], encoder.index(pixel.red(), pixel.green(), pixel.blue()), "pixel {i}");
        }
    }

    #[test]
    fn palette_holds_the_theme_colors() {
        for name in crate::theme::names() {
            let pal = crate::theme::pick(Some(&name)).unwrap().palette;
            let mut encoder = Encoder::new(&pal);
            for color in [pal.page, pal.ink, pal.alert, pal.warn, pal.teal, pal.relation, pal.options] {
                let [r, g, b] = rgb(color);
                let index = encoder.index(r, g, b);
                let [pr, pg, pb] = encoder.colors[usize::from(index)];
                assert!(r.abs_diff(pr) <= 3 && g.abs_diff(pg) <= 3 && b.abs_diff(pb) <= 3, "{name} {color:?}");
            }
            assert!(encoder.colors.len() <= 256);
        }
    }
}
