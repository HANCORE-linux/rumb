use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
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

const MAX_PENDING: usize = 4;

pub type Paint = Box<dyn FnOnce(u32, u32, f32) -> Option<Pixmap> + Send>;

pub struct Pictures {
    picker: Picker,
    slots: HashMap<Rect, Slot>,
    pending: HashMap<Rect, u64>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

struct Slot {
    key: u64,
    next: Instant,
    protocol: Protocol,
}

struct Job {
    rect: Rect,
    key: u64,
    width: u32,
    height: u32,
    cell_height: f32,
    paint: Paint,
}

struct Done {
    rect: Rect,
    key: u64,
    cost: Duration,
    protocol: Option<Protocol>,
}

impl Pictures {
    pub fn query() -> Option<Self> {
        let picker = Picker::from_query_stdio_with_options(QueryStdioOptions { kitty_compression: true, ..QueryStdioOptions::default() }).ok()?;
        if picker.protocol_type() == ProtocolType::Halfblocks {
            return None;
        }
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, done) = mpsc::channel();
        let worker = picker.clone();
        thread::spawn(move || {
            for job in inbox {
                let started = Instant::now();
                let protocol = (job.paint)(job.width, job.height, job.cell_height).and_then(|pixmap| encode(&worker, &pixmap, job.rect));
                if outbox.send(Done { rect: job.rect, key: job.key, cost: started.elapsed(), protocol }).is_err() {
                    return;
                }
            }
        });
        Some(Pictures { picker, slots: HashMap::new(), pending: HashMap::new(), jobs, done })
    }

    pub fn draw(&mut self, frame: &mut Frame, scene: Scene, salt: (u64, &str), now: Instant, painter: impl Fn(Picture) -> Paint) {
        while let Ok(done) = self.done.try_recv() {
            self.pending.remove(&done.rect);
            if let Some(protocol) = done.protocol {
                self.slots.insert(done.rect, Slot { key: done.key, next: now + done.cost, protocol });
            }
        }
        let font = self.picker.font_size();
        let mut shown = Vec::new();
        for placement in scene.placements {
            let rect = placement.rect;
            let key = fingerprint(&placement.picture, salt);
            let slot = self.slots.get(&rect);
            let stale = slot.is_none_or(|slot| slot.key != key && now >= slot.next);
            if stale && !self.pending.contains_key(&rect) && self.pending.len() < MAX_PENDING {
                let (width, height) = (u32::from(rect.width) * u32::from(font.width), u32::from(rect.height) * u32::from(font.height));
                let job = Job { rect, key, width, height, cell_height: f32::from(font.height), paint: painter(placement.picture) };
                if self.jobs.send(job).is_ok() {
                    self.pending.insert(rect, key);
                }
            }
            if let Some(slot) = self.slots.get(&rect) {
                frame.render_widget(Image::new(&slot.protocol), rect);
                shown.push(rect);
            }
        }
        self.slots.retain(|rect, _| shown.contains(rect));
    }
}

fn encode(picker: &Picker, pixmap: &Pixmap, rect: Rect) -> Option<Protocol> {
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    let image = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixels)?;
    picker.new_protocol(DynamicImage::ImageRgba8(image), Size::new(rect.width, rect.height), Resize::Fit(None)).ok()
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
