use ratatui::style::Color;

/// Paper and ink, after the scientific-dashboard look: monochrome with a
/// single teal signal.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub page: Color,
    pub panel: Color,
    pub kpi: Color,
    pub bar: Color,
    pub bar_text: Color,
    pub ink: Color,
    pub ink2: Color,
    pub label: Color,
    pub faint: Color,
    pub rule: Color,
    pub grid: Color,
    pub teal: Color,
    pub select: Color,
    pub alert: Color,
    pub warn: Color,
    pub relation: Color,
    pub ok: Color,
    pub options: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    pub const fn paper() -> Self {
        Palette {
            page: rgb(0xe4e4df),
            panel: rgb(0xebebe6),
            kpi: rgb(0xdbe0da),
            bar: rgb(0x292926),
            bar_text: rgb(0xebebe6),
            ink: rgb(0x1b1b19),
            ink2: rgb(0x3d3d39),
            label: rgb(0x6f706a),
            faint: rgb(0x9d9e98),
            rule: rgb(0xb4b5af),
            grid: rgb(0xd3d4ce),
            teal: rgb(0x3a7778),
            select: rgb(0xc8c8c4),
            alert: rgb(0xa8463c),
            warn: rgb(0x8a6d12),
            relation: rgb(0x5f6a12),
            ok: rgb(0x5f6a12),
            options: rgb(0xa8463c),
        }
    }

    /// The Labra theme, with the roles its btop theme gives its colors:
    /// pink highlight, olive and cream on near black.
    pub const fn labra() -> Self {
        Palette {
            page: rgb(0x040704),
            panel: rgb(0x0a0e0a),
            kpi: rgb(0x141814),
            bar: rgb(0x1d201d),
            bar_text: rgb(0xdaddc0),
            ink: rgb(0xd3d7b5),
            ink2: rgb(0x969b83),
            label: rgb(0x717669),
            faint: rgb(0x585d58),
            rule: rgb(0x3b403b),
            grid: rgb(0x1d201d),
            teal: rgb(0xe33d84),
            select: rgb(0x1e211b),
            alert: rgb(0xee585d),
            warn: rgb(0xd3dd2c),
            relation: rgb(0x747f11),
            ok: rgb(0x747f11),
            options: rgb(0xff676a),
        }
    }
}

/// Linear blend between two RGB colors; `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color, b: Color, t: f64) -> Color {
    match (a, b) {
        (Color::Rgb(ar, ag, ab), Color::Rgb(br, bg, bb)) => {
            let t = t.clamp(0.0, 1.0);
            let lerp = |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * t).round() as u8;
            Color::Rgb(lerp(ar, br), lerp(ag, bg), lerp(ab, bb))
        }
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}
