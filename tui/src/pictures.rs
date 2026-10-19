use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbaImage};
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use tiny_skia::Pixmap;

use crate::ui::{Picture, Scene};

const SETTLE: Duration = Duration::from_millis(250);

pub struct Pictures {
    picker: Picker,
    slots: HashMap<Rect, Slot>,
}

struct Slot {
    key: u64,
    at: Instant,
    protocol: Protocol,
}

impl Pictures {
    pub fn query() -> Option<Self> {
        let picker = Picker::from_query_stdio_with_options(QueryStdioOptions { kitty_compression: true, ..QueryStdioOptions::default() }).ok()?;
        (picker.protocol_type() != ProtocolType::Halfblocks).then(|| Pictures { picker, slots: HashMap::new() })
    }

    pub fn draw(&mut self, frame: &mut Frame, scene: &Scene, salt: (u64, &str), now: Instant, paint: impl Fn(&Picture, u32, u32, f32) -> Option<Pixmap>) {
        let font = self.picker.font_size();
        let mut shown = Vec::new();
        for placement in &scene.placements {
            let rect = placement.rect;
            let key = fingerprint(&placement.picture, salt);
            let (width, height) = (u32::from(rect.width) * u32::from(font.width), u32::from(rect.height) * u32::from(font.height));
            let settle = SETTLE + Duration::from_micros(u64::from(width) * u64::from(height));
            let settled = self.slots.get(&rect).is_some_and(|slot| slot.key == key || now.saturating_duration_since(slot.at) < settle);
            if !settled && let Some(protocol) = paint(&placement.picture, width, height, f32::from(font.height)).and_then(|pixmap| self.encode(&pixmap, rect)) {
                self.slots.insert(rect, Slot { key, at: now, protocol });
            }
            if let Some(slot) = self.slots.get(&rect) {
                frame.render_widget(Image::new(&slot.protocol), rect);
                shown.push(rect);
            }
        }
        self.slots.retain(|rect, _| shown.contains(rect));
    }

    fn encode(&self, pixmap: &Pixmap, rect: Rect) -> Option<Protocol> {
        let pixels = pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| {
                let color = pixel.demultiply();
                [color.red(), color.green(), color.blue(), color.alpha()]
            })
            .collect();
        let image = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixels)?;
        self.picker.new_protocol(DynamicImage::ImageRgba8(image), Size::new(rect.width, rect.height), Resize::Fit(None)).ok()
    }
}

fn fingerprint(picture: &Picture, (generation, astrolabe): (u64, &str)) -> u64 {
    let mut hasher = DefaultHasher::new();
    generation.hash(&mut hasher);
    match picture {
        Picture::Astrolabe => astrolabe.hash(&mut hasher),
        other => format!("{other:?}").hash(&mut hasher),
    }
    hasher.finish()
}
