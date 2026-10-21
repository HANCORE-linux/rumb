use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use image::{DynamicImage, RgbaImage};
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::protocol::kitty::Kitty;
use ratatui_image::protocol::sixel::Sixel;
use ratatui_image::{Image, Resize};
use tiny_skia::Pixmap;

use crate::page::{Picture, Scene};
use crate::palette::Palette;
use crate::sixel::Encoder;

const MAX_PENDING: usize = 4;
pub const FRAME: Duration = Duration::from_millis(33);

pub type Paint = Box<dyn FnOnce(u32, u32, f32) -> Option<Pixmap> + Send>;

pub struct Pictures {
    picker: Picker,
    slots: HashMap<Rect, Slot>,
    pending: HashMap<Rect, u64>,
    ids: HashMap<Rect, u32>,
    free: Vec<u32>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
}

struct Slot {
    key: u64,
    next: Instant,
    since: Instant,
    protocol: Protocol,
}

struct Job {
    rect: Rect,
    key: u64,
    id: u32,
    width: u32,
    height: u32,
    cell_height: f32,
    palette: Palette,
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
        Some(Self::start(picker))
    }

    fn start(picker: Picker) -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (outbox, done) = mpsc::channel();
        let worker = picker.clone();
        thread::spawn(move || {
            let mut encoder: Option<Encoder> = None;
            for job in inbox {
                let started = Instant::now();
                let protocol = (job.paint)(job.width, job.height, job.cell_height).and_then(|pixmap| {
                    if worker.protocol_type() == ProtocolType::Sixel {
                        let encoder = match encoder.as_mut() {
                            Some(encoder) if encoder.fits(&job.palette) => encoder,
                            _ => encoder.insert(Encoder::new(&job.palette)),
                        };
                        Some(sixel(encoder, &pixmap, job.rect))
                    } else {
                        encode(&worker, &pixmap, job.rect, job.id)
                    }
                });
                if outbox.send(Done { rect: job.rect, key: job.key, cost: started.elapsed(), protocol }).is_err() {
                    return;
                }
            }
        });
        Pictures { picker, slots: HashMap::new(), pending: HashMap::new(), ids: HashMap::new(), free: Vec::new(), jobs, done }
    }

    pub fn cell(&self) -> (u16, u16) {
        let font = self.picker.font_size();
        (font.width, font.height)
    }

    pub fn draw(&mut self, frame: &mut Frame, scene: Scene, palette: &Palette, generation: u64, now: Instant, painter: impl Fn(Picture) -> Paint) {
        while let Ok(done) = self.done.try_recv() {
            self.pending.remove(&done.rect);
            if let Some(protocol) = done.protocol {
                self.slots.insert(done.rect, Slot { key: done.key, next: now + done.cost.saturating_sub(FRAME), since: now, protocol });
            }
        }
        let font = self.picker.font_size();
        let mut placements: Vec<(Rect, u64, Option<Picture>)> = scene.placements.into_iter().map(|placement| (placement.rect, fingerprint(&placement.picture, generation), Some(placement.picture))).collect();
        let mut stale: Vec<usize> = (0..placements.len())
            .filter(|&index| {
                let (rect, key, _) = &placements[index];
                self.slots.get(rect).is_none_or(|slot| slot.key != *key && now >= slot.next) && !self.pending.contains_key(rect)
            })
            .collect();
        stale.sort_by_key(|&index| (!matches!(placements[index].2, Some(Picture::Eye(_))), self.slots.get(&placements[index].0).map(|slot| slot.since)));
        for index in stale.into_iter().take(MAX_PENDING.saturating_sub(self.pending.len())) {
            let (rect, key, picture) = &mut placements[index];
            let Some(picture) = picture.take() else { continue };
            let (width, height) = (u32::from(rect.width) * u32::from(font.width), u32::from(rect.height) * u32::from(font.height));
            let job = Job { rect: *rect, key: *key, id: self.id(*rect), width, height, cell_height: f32::from(font.height), palette: *palette, paint: painter(picture) };
            if self.jobs.send(job).is_ok() {
                self.pending.insert(*rect, *key);
            }
        }
        let mut shown = Vec::new();
        for (rect, _, _) in &placements {
            if let Some(slot) = self.slots.get(rect) {
                frame.render_widget(Image::new(&slot.protocol), *rect);
                shown.push(*rect);
            }
        }
        self.slots.retain(|rect, _| shown.contains(rect));
        let (pending, free) = (&self.pending, &mut self.free);
        self.ids.retain(|rect, id| {
            let keep = shown.contains(rect) || pending.contains_key(rect);
            if !keep {
                free.push(*id);
            }
            keep
        });
    }

    fn id(&mut self, rect: Rect) -> u32 {
        let next = (std::process::id() << 8) + (self.ids.len() + self.free.len()) as u32;
        *self.ids.entry(rect).or_insert_with(|| self.free.pop().unwrap_or(next))
    }
}

fn encode(picker: &Picker, pixmap: &Pixmap, rect: Rect, id: u32) -> Option<Protocol> {
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    let image = DynamicImage::ImageRgba8(RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixels)?);
    let size = Size::new(rect.width, rect.height);
    if picker.protocol_type() == ProtocolType::Kitty {
        let compress = picker.capabilities().contains(&Capability::KittyCompression);
        return Kitty::new(image, size, id, picker.tmux_detected(), compress).ok().map(Protocol::Kitty);
    }
    picker.new_protocol(image, size, Resize::Fit(None)).ok()
}

fn sixel(encoder: &mut Encoder, pixmap: &Pixmap, rect: Rect) -> Protocol {
    let mut data = String::new();
    if rect.height == 1 {
        data.push_str(&format!("\x1b[{}X", rect.width));
    } else {
        for _ in 0..rect.height {
            data.push_str(&format!("\x1b[{}X\x1b[1B", rect.width));
        }
        data.push_str(&format!("\x1b[{}A", rect.height));
    }
    data.push_str(&encoder.encode(pixmap));
    Protocol::Sixel(Sixel { data, size: Size::new(rect.width, rect.height), is_tmux: false })
}

fn fingerprint(picture: &Picture, generation: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    generation.hash(&mut hasher);
    match picture {
        Picture::Eye(spec) => {
            1u8.hash(&mut hasher);
            spec.fingerprint(&mut hasher);
        }
        Picture::Fan(spec) => {
            3u8.hash(&mut hasher);
            spec.hash(&mut hasher);
        }
        Picture::Orbit(spec) => {
            4u8.hash(&mut hasher);
            spec.hash(&mut hasher);
        }
        Picture::Specimen { subsystem, signal, glow, aim } => {
            (2u8, subsystem.label(), signal, glow.map(|(color, strength)| (format!("{color:?}"), strength.to_bits())), aim.to_bits()).hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::Subsystem;
    use crate::page::Placement;

    fn specimen() -> Picture {
        Picture::Specimen { subsystem: Subsystem::Kernel, signal: false, glow: None, aim: 0.0 }
    }

    #[test]
    fn kitty_pictures_reuse_their_image_ids() {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut pictures = Pictures::start(picker);
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        let (eye, icon, moved) = (Rect::new(1, 1, 12, 4), Rect::new(20, 1, 8, 3), Rect::new(20, 5, 8, 3));
        let pal = Palette::labra();
        let started = Instant::now();
        let mut sent: HashMap<Rect, Vec<u32>> = HashMap::new();
        for step in 0..9u64 {
            let rects = match step {
                0..3 => vec![eye, icon],
                3..6 => vec![eye],
                _ => vec![eye, moved],
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let scene = Scene { placements: rects.iter().map(|&rect| Placement { rect, picture: specimen() }).collect(), ..Scene::default() };
                terminal.draw(|frame| pictures.draw(frame, scene, &pal, step, started + Duration::from_secs(step), |_| Box::new(|width: u32, height: u32, _: f32| Pixmap::new(width, height)))).unwrap();
                for &rect in &rects {
                    let symbol = terminal.backend().buffer()[(rect.x, rect.y)].symbol();
                    sent.entry(rect).or_default().extend(symbol.split("q=2,i=").skip(1).filter_map(|rest| rest.split(',').next()?.parse::<u32>().ok()));
                }
                let key = fingerprint(&specimen(), step);
                if rects.iter().all(|rect| pictures.slots.get(rect).is_some_and(|slot| slot.key == key)) {
                    break;
                }
                assert!(Instant::now() < deadline, "the pictures of step {step} were not drawn");
                thread::sleep(Duration::from_millis(2));
            }
        }
        assert_eq!(sent[&eye].len(), 9);
        assert!(sent[&eye].iter().all(|id| *id == sent[&eye][0]));
        assert_eq!((sent[&icon].len(), sent[&moved].len()), (3, 3));
        assert!(sent[&icon].iter().chain(&sent[&moved]).all(|id| *id == sent[&icon][0]));
        assert_ne!(sent[&eye][0], sent[&icon][0]);
        assert_eq!(pictures.ids.len() + pictures.free.len(), 2);
    }

    #[test]
    fn busy_icons_do_not_starve_the_eye() {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut pictures = Pictures::start(picker);
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).unwrap();
        let icons: Vec<Rect> = (0..4).map(|k| Rect::new(1, 1 + 2 * k, 8, 2)).collect();
        let eye = Rect::new(20, 1, 30, 8);
        let pal = Palette::labra();
        let started = Instant::now();
        let mut seen = Vec::new();
        for step in 0..25u64 {
            let scene = Scene { placements: icons.iter().chain([&eye]).map(|&rect| Placement { rect, picture: specimen() }).collect(), ..Scene::default() };
            terminal.draw(|frame| pictures.draw(frame, scene, &pal, step, started + Duration::from_secs(step), |_| Box::new(|width: u32, height: u32, _: f32| Pixmap::new(width, height)))).unwrap();
            if let Some(slot) = pictures.slots.get(&eye)
                && seen.last() != Some(&slot.key)
            {
                seen.push(slot.key);
            }
            thread::sleep(Duration::from_millis(40));
        }
        assert!(seen.len() >= 4, "the eye was redrawn {} times in 25 busy frames", seen.len());
    }

    #[test]
    fn the_eye_keeps_its_frame_rate_while_icons_glow() {
        let mut picker = Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut pictures = Pictures::start(picker);
        let mut terminal = Terminal::new(TestBackend::new(60, 20)).unwrap();
        let icons: Vec<Rect> = (0..7).map(|k| Rect::new(1, 1 + 2 * k, 8, 2)).collect();
        let eye = Rect::new(20, 1, 30, 8);
        let pal = Palette::labra();
        let started = Instant::now();
        let mut seen = Vec::new();
        for step in 0..25u64 {
            let mut placements: Vec<Placement> = icons.iter().map(|&rect| Placement { rect, picture: specimen() }).collect();
            placements.push(Placement { rect: eye, picture: Picture::Eye(Box::default()) });
            let scene = Scene { placements, ..Scene::default() };
            terminal.draw(|frame| pictures.draw(frame, scene, &pal, step, started + Duration::from_secs(step), |_| Box::new(|width: u32, height: u32, _: f32| Pixmap::new(width, height)))).unwrap();
            if let Some(slot) = pictures.slots.get(&eye)
                && seen.last() != Some(&slot.key)
            {
                seen.push(slot.key);
            }
            thread::sleep(Duration::from_millis(40));
        }
        assert!(seen.len() >= 20, "the eye was redrawn {} times in 25 frames with seven glowing icons", seen.len());
    }
}
