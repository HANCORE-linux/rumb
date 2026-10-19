use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use ratatui::style::Color;

use crate::palette::{Palette, mix};

const BUNDLED: &str = include_str!("../themes.json");

pub struct Theme {
    pub palette: Palette,
    pub generation: u64,
    live: Option<Live>,
}

struct Live {
    colors: PathBuf,
    name: PathBuf,
    stamp: Option<(SystemTime, String)>,
    checked: Instant,
}

impl Theme {
    fn fixed(palette: Palette) -> Self {
        Theme { palette, generation: 0, live: None }
    }

    pub fn refresh(&mut self, now: Instant) -> bool {
        let Some(live) = &mut self.live else { return false };
        if now.saturating_duration_since(live.checked) < Duration::from_secs(1) {
            return false;
        }
        live.checked = now;
        let stamp = stamp(live);
        if stamp.is_none() || stamp == live.stamp {
            return false;
        }
        let Some(palette) = read_live(live) else { return false };
        live.stamp = stamp;
        self.palette = palette;
        self.generation += 1;
        true
    }
}

fn bundled() -> BTreeMap<String, HashMap<String, String>> {
    serde_json::from_str(BUNDLED).unwrap_or_default()
}

pub fn names() -> Vec<String> {
    let mut names: Vec<String> = ["labra", "paper"].into_iter().map(String::from).chain(bundled().into_keys()).collect();
    names.sort();
    names.dedup();
    names
}

pub fn pick(requested: Option<&str>) -> Result<Theme, String> {
    match requested {
        Some("omarchy") => live().ok_or_else(|| "no Omarchy theme found".to_string()),
        Some(name) => named(name).map(Theme::fixed).ok_or_else(|| format!("unknown theme: {name} (rumb --themes lists them)")),
        None => Ok(live().unwrap_or_else(|| Theme::fixed(Palette::labra()))),
    }
}

fn named(name: &str) -> Option<Palette> {
    match name {
        "labra" => Some(Palette::labra()),
        "paper" => Some(Palette::paper()),
        _ => derive(bundled().get(name)?),
    }
}

fn live() -> Option<Theme> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let current = [".local/state/omarchy/current", ".config/omarchy/current"]
        .into_iter()
        .map(|dir| home.join(dir))
        .find(|dir| dir.join("theme/colors.toml").is_file())?;
    let mut live = Live { colors: current.join("theme/colors.toml"), name: current.join("theme.name"), stamp: None, checked: Instant::now() };
    live.stamp = stamp(&live);
    let palette = read_live(&live)?;
    Some(Theme { palette, generation: 0, live: Some(live) })
}

fn stamp(live: &Live) -> Option<(SystemTime, String)> {
    let modified = std::fs::metadata(&live.colors).and_then(|meta| meta.modified()).ok()?;
    let name = std::fs::read_to_string(&live.name).unwrap_or_default().trim().to_string();
    Some((modified, name))
}

fn read_live(live: &Live) -> Option<Palette> {
    if std::fs::read_to_string(&live.name).is_ok_and(|name| name.trim().eq_ignore_ascii_case("labra")) {
        return Some(Palette::labra());
    }
    let text = std::fs::read_to_string(&live.colors).ok()?;
    let light = live.colors.with_file_name("light.mode").is_file();
    derive(&resolve(parse(&text), light))
}

fn parse(text: &str) -> HashMap<String, String> {
    let mut colors = HashMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let key: String = key.chars().filter(|c| !matches!(c, '"' | '\'' | ' ' | '\t')).collect();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        let value = match value.find(['"', '\'']) {
            Some(start) => {
                let rest = &value[start + 1..];
                rest[..rest.find(['"', '\'']).unwrap_or(rest.len())].to_string()
            }
            None => value.trim().to_string(),
        };
        colors.insert(key, value);
    }
    colors
}

fn resolve(mut c: HashMap<String, String>, light_file: bool) -> HashMap<String, String> {
    c.retain(|_, value| !value.is_empty());
    let alias = |c: &mut HashMap<String, String>, key: &str, from: &str| {
        if !c.contains_key(key)
            && let Some(value) = c.get(from).cloned()
        {
            c.insert(key.to_string(), value);
        }
    };
    for (key, short) in [
        ("background", "bg"),
        ("foreground", "fg"),
        ("lighter_background", "lighter_bg"),
        ("dark_foreground", "dark_fg"),
        ("light_foreground", "light_fg"),
        ("bright_foreground", "bright_fg"),
    ] {
        alias(&mut c, key, short);
    }
    alias(&mut c, "background", "color0");
    alias(&mut c, "foreground", "color7");
    for (key, from) in [("color0", "background"), ("color7", "foreground")] {
        if let Some(value) = c.get(from).cloned() {
            c.insert(key.to_string(), value);
        }
    }
    for (key, from) in [("red", "color1"), ("green", "color2"), ("yellow", "color3"), ("blue", "color4"), ("magenta", "color5"), ("cyan", "color6"), ("magenta", "purple")] {
        alias(&mut c, key, from);
    }
    for (key, chain) in [
        ("light_foreground", &["color7", "foreground"][..]),
        ("bright_foreground", &["color15", "foreground"]),
        ("lighter_background", &["color0", "background"]),
        ("dark_foreground", &["color8", "foreground"]),
        ("muted", &["color8", "dark_foreground"]),
        ("selection", &["selection_background", "color8", "color0", "background"]),
    ] {
        for from in chain {
            alias(&mut c, key, from);
        }
    }
    if !c.contains_key("mode") {
        let light = light_file
            || c.get("background").and_then(|value| hex(value)).is_some_and(|color| match color {
                Color::Rgb(r, g, b) => u32::from(r) + u32::from(g) + u32::from(b) > 382,
                _ => false,
            });
        let mode = c.get("theme_type").cloned().unwrap_or_else(|| if light { "light" } else { "dark" }.to_string());
        c.insert("mode".to_string(), mode);
    }
    c
}

fn derive(c: &HashMap<String, String>) -> Option<Palette> {
    let color = |key: &str| c.get(key).and_then(|value| hex(value));
    let (page, ink) = (color("background")?, color("foreground")?);
    let dark = c.get("mode").is_none_or(|mode| mode != "light");
    let panel = if dark { mix(page, ink, 0.035) } else { mix(page, Color::Rgb(255, 255, 255), 0.35) };
    let ramp = |t: f64| mix(panel, ink, t);
    let accent = legible(color("accent").or(color("blue")).unwrap_or(ink), panel, ink);
    Some(Palette {
        dark,
        page,
        panel,
        kpi: ramp(0.05),
        bar: ramp(0.095),
        bar_text: color("light_foreground").unwrap_or(ink),
        ink,
        ink2: ramp(0.7),
        label: ramp(0.51),
        faint: ramp(0.39),
        rule: ramp(0.24),
        grid: ramp(0.095),
        teal: accent,
        teal_mid: mix(accent, page, 0.1),
        teal_light: mix(panel, accent, 0.2),
        teal_fill: mix(panel, color("green").unwrap_or(accent), 0.45),
        alert: legible(color("red").unwrap_or(accent), panel, ink),
    })
}

fn hex(value: &str) -> Option<Color> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let rgb = u32::from_str_radix(digits, 16).ok()?;
    Some(Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
}

fn luminance(color: Color) -> f64 {
    let Color::Rgb(r, g, b) = color else { return 0.0 };
    let channel = |value: u8| {
        let s = f64::from(value) / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

fn contrast(a: Color, b: Color) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn legible(color: Color, against: Color, toward: Color) -> Color {
    (0..=10).map(|step| mix(color, toward, f64::from(step) / 10.0)).find(|candidate| contrast(*candidate, against) >= 3.0).unwrap_or(toward)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_gives_a_palette() {
        let themes = bundled();
        assert!(themes.len() >= 50);
        for (name, colors) in &themes {
            assert!(derive(colors).is_some(), "{name}");
        }
        for name in names() {
            assert!(named(&name).is_some(), "{name}");
        }
    }

    #[test]
    fn resolves_like_omarchy() {
        let ansi = resolve(parse("background = \"#101010\"\ncolor1 = '#aa0000'\ncolor8 = \"#555555\" # muted\n"), false);
        assert_eq!(ansi["red"], "#aa0000");
        assert_eq!(ansi["muted"], "#555555");
        assert_eq!(ansi["dark_foreground"], "#555555");
        assert_eq!(ansi["mode"], "dark");
        let named = resolve(parse("background = \"#f0f0f0\"\nforeground = \"#202020\"\naccent = \"#3366cc\"\n"), false);
        assert_eq!(named["mode"], "light");
        assert_eq!(named["light_foreground"], "#202020");
        assert_eq!(named["lighter_background"], "#f0f0f0");
    }
}
