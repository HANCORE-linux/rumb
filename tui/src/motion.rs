use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::app::{App, Pane, Subsystem};
use crate::source::Event;

const TYPE: Duration = Duration::from_millis(30);
const LIVE_TYPE: Duration = Duration::from_millis(12);
const GAP: Duration = Duration::from_millis(260);
const SETTLE: Duration = Duration::from_millis(240);
const LOOK: Duration = Duration::from_millis(500);
const GLANCE: Duration = Duration::from_millis(520);
const IDLE: Duration = Duration::from_secs(8);
const REST: Duration = Duration::from_secs(4);
const BACKLOG: Duration = Duration::from_secs(12);
const LINES: usize = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Tile(u8),
    List(u8),
    Part(u8),
    Edge(u8),
}

impl Target {
    pub fn folded(self) -> Target {
        match self {
            Target::Edge(part) => Target::Part(part),
            other => other,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Signal {
    pub target: Target,
    pub progress: f32,
    pub inbound: bool,
    pub kind: u8,
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
    pub tint: Option<(u8, f32)>,
}

type Spring = (Instant, (f32, f32), (f32, f32));
pub type Sight = (Option<Target>, Instant, Option<Target>, Instant);

struct Comet {
    start: Instant,
    target: Target,
    inbound: bool,
    kind: u8,
    duration: Duration,
}

impl Comet {
    fn end(&self) -> Instant {
        self.start + self.duration
    }
}

struct Line {
    start: Instant,
    head: String,
    tail: String,
    result: Option<Instant>,
    speed: Duration,
}

impl Line {
    fn typed(&self) -> Instant {
        self.start + self.speed * self.head.chars().count() as u32
    }
}

enum Act {
    Apply(Box<Event>, Instant),
    Done(usize, Instant),
    Feed(Arc<Vec<char>>),
}

struct Turn {
    start: Instant,
    from: f32,
    to: f32,
    length: Duration,
    hold: Duration,
}

pub struct Motion {
    enabled: bool,
    cursor: Instant,
    open: Option<usize>,
    lines: Vec<Line>,
    acts: Vec<(Instant, u64, Act)>,
    sequence: u64,
    comets: Vec<Comet>,
    holds: Vec<(Target, Instant)>,
    glows: Vec<(u8, Instant, u8)>,
    flashes: Vec<(u8, Instant, u8)>,
    last_activity: Instant,
    omarchy: bool,
    attention: Option<(Instant, Target)>,
    last_step: Instant,
    clock: f32,
    level: f32,
    feed: (u64, Arc<Vec<char>>),
    before: (u64, Arc<Vec<char>>),
    feeds: u64,
    switch: f32,
    turn: Turn,
    angle: f32,
    side: f32,
    turns: u32,
    pending_select: Option<(Pane, usize, Instant)>,
    shown_select: [Option<usize>; 3],
    gaze: Cell<Option<Spring>>,
    sight: Cell<Option<Sight>>,
}

fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn chance(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    (x & 0xFFFF) as f32 / 65535.0
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

pub fn command_of(label: &str) -> &str {
    match label {
        "checking repository updates" => "checkupdates",
        "checking AUR updates" => "yay -Qua",
        "reading crash reports" => "coredumpctl list",
        "reading journal errors" => "journalctl -p err",
        "checking failed units" => "systemctl --failed",
        "checking Hyprland config" => "hyprctl configerrors",
        "reading the running kernel" => "uname -r",
        "reading the loaded nvidia module" => "/sys/module/nvidia/version",
        "reading the running Hyprland" => "hyprctl version",
        "listing installed packages" => "pacman -Q",
        "measuring the package cache" => "paccache -dk1",
        "fetching Arch news" => "curl archlinux.org/feeds/news",
        "fetching Omarchy release notes" => "curl omacom/omarchy/releases",
        "checking disk space and snapshots" => "df /",
        other => other,
    }
}

pub fn result_of(label: &str, value: &Value) -> String {
    let number = value.as_u64();
    let count = |one: &str, many: &str| number.map_or_else(|| "failed".to_string(), |n| format!("{} {}", crate::page::thousands(n), if n == 1 { one } else { many }));
    match label {
        "checking repository updates" => count("update", "updates"),
        "checking AUR updates" => count("aur", "aur"),
        "reading crash reports" => number.map_or_else(|| "failed".to_string(), |n| format!("{} {}", crate::page::thousands(n), if n == 1 { "crash" } else { "crashes" })),
        "reading journal errors" => count("error", "errors"),
        "checking failed units" => count("unit", "units"),
        "checking Hyprland config" => number.map_or_else(|| "failed".to_string(), |n| n.to_string()),
        "reading the running kernel" => value["running"].as_str().unwrap_or_default().to_string(),
        "reading the loaded nvidia module" => value["loaded"].as_str().unwrap_or_default().to_string(),
        "reading the running Hyprland" => value["running"].as_str().map(|version| format!("v{version}")).unwrap_or_default(),
        "listing installed packages" => {
            let orphans = value["orphans"].as_u64().unwrap_or(0);
            format!("{} · {orphans} {}", crate::page::thousands(value["packages"].as_u64().unwrap_or(0)), if orphans == 1 { "orphan" } else { "orphans" })
        }
        "measuring the package cache" => number.map_or_else(|| "no paccache".to_string(), |bytes| format!("{} freeable", crate::page::bytes(bytes))),
        "fetching Arch news" => match number {
            None => "failed".to_string(),
            Some(0) => "none new".to_string(),
            Some(n) => format!("{n} new"),
        },
        "fetching Omarchy release notes" => number.map_or_else(|| "failed".to_string(), |n| format!("{n} {}", if n == 1 { "release" } else { "releases" })),
        "checking disk space and snapshots" => number.map_or_else(String::new, |bytes| format!("{} available", crate::page::bytes(bytes))),
        _ => match value {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        },
    }
}

fn short(name: &str) -> String {
    if name.chars().count() > 14 { format!("{}…", name.chars().take(13).collect::<String>()) } else { name.to_string() }
}

pub fn upgrade_tail(iso: Option<&str>) -> String {
    match iso.filter(|iso| iso.len() >= 16) {
        Some(iso) => format!("upgrade {} {}", &iso[5..10], &iso[11..16]),
        None => "no upgrade found".to_string(),
    }
}

impl Motion {
    pub fn new(enabled: bool, now: Instant) -> Self {
        let empty: Arc<Vec<char>> = Arc::new(Vec::new());
        Motion {
            enabled,
            cursor: now,
            open: None,
            lines: Vec::new(),
            acts: Vec::new(),
            sequence: 0,
            comets: Vec::new(),
            holds: Vec::new(),
            glows: Vec::new(),
            flashes: Vec::new(),
            last_activity: now,
            omarchy: false,
            attention: None,
            last_step: now,
            clock: 0.0,
            level: 0.2,
            feed: (0, empty.clone()),
            before: (0, empty),
            feeds: 0,
            switch: 0.0,
            turn: Turn { start: now, from: 0.0, to: 0.0, length: ms(1), hold: ms(600) },
            angle: 0.0,
            side: 1.0,
            turns: 0,
            pending_select: None,
            shown_select: [None; 3],
            gaze: Cell::new(None),
            sight: Cell::new(None),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn act(&mut self, at: Instant, act: Act) {
        self.sequence += 1;
        self.acts.push((at, self.sequence, act));
    }

    fn send(&mut self, start: Instant, target: Target, kind: u8, duration: u64, hold: bool) -> Instant {
        let duration = ms(duration);
        self.comets.push(Comet { start, target, inbound: false, kind, duration });
        if hold {
            self.holds.push((target, start + duration));
        }
        start + duration
    }

    fn receive(&mut self, start: Instant, target: Target, kind: u8, duration: u64) -> Instant {
        let duration = ms(duration);
        self.comets.push(Comet { start, target, inbound: true, kind, duration });
        start + duration
    }

    fn line(&mut self, start: Instant, head: String, tail: String, result: Option<Instant>, speed: Duration) -> usize {
        let index = self.lines.partition_point(|line| line.start <= start);
        self.lines.insert(index, Line { start, head, tail, result, speed });
        if let Some(open) = self.open.as_mut()
            && *open >= index
        {
            *open += 1;
        }
        index
    }

    fn close_open(&mut self, now: Instant) {
        if let Some(open) = self.open.take()
            && let Some(line) = self.lines.get_mut(open)
        {
            let result = line.typed().max(now);
            line.result = Some(result);
            self.cursor = result + GAP;
        }
    }

    fn free(&self, now: Instant) -> Instant {
        self.cursor.max(now)
    }

    fn feed_from(&mut self, at: Instant, raw: &str) {
        if raw.is_empty() {
            return;
        }
        self.act(at, Act::Feed(Arc::new(raw.chars().filter(|ch| !ch.is_control()).collect())));
    }

    pub fn push(&mut self, event: Event, raw: &str, app: &mut App, now: Instant) {
        if !self.enabled {
            app.apply(event, now);
            return;
        }
        self.last_activity = now;
        match event {
            Event::Start { ref last_upgrade, omarchy, .. } => {
                self.omarchy = omarchy;
                let tail = upgrade_tail(last_upgrade.as_deref());
                let start = self.free(now);
                let head = "› pacman.log".to_string();
                let result = start + TYPE * head.chars().count() as u32 + ms(280);
                self.line(start, head, tail, Some(result), TYPE);
                self.cursor = result + GAP;
                self.send(result, Target::Tile(2), 0, 700, true);
                self.feed_from(start, raw);
                self.act(now, Act::Apply(Box::new(event), now));
            }
            Event::Step { ref label, .. } => {
                self.close_open(now);
                let start = self.free(now);
                let head = format!("› {}", command_of(label));
                let index = self.line(start, head, String::new(), None, TYPE);
                self.open = Some(index);
                self.last_step = start;
                self.act(start, Act::Apply(Box::new(event), now));
            }
            Event::Done { ref label, ref value, .. } => {
                let tail = result_of(label, value);
                let Some(open) = self.open.take() else {
                    self.act(now, Act::Apply(Box::new(event), now));
                    return;
                };
                let typed = self.lines[open].typed();
                let base = typed.max(now);
                let result = match label.as_str() {
                    "reading the running kernel" => {
                        self.send(base, Target::Part(0), 0, 360, false);
                        self.glows.push((0, base + ms(360), 0));
                        self.send(base + ms(360), Target::Edge(0), 0, 420, false);
                        self.receive(base + ms(780), Target::Edge(0), 0, 420);
                        self.flashes.push((0, base + ms(990), 0));
                        self.glows.push((0, base + ms(1200), 0));
                        self.receive(base + ms(1200), Target::Part(0), 0, 360);
                        base + ms(1600)
                    }
                    "reading the loaded nvidia module" | "reading the running Hyprland" => {
                        let part = if label == "reading the loaded nvidia module" { 1 } else { 2 };
                        self.receive(base, Target::Edge(part), 0, 420);
                        self.flashes.push((part, base + ms(210), 0));
                        self.glows.push((part, base + ms(420), 0));
                        self.receive(base + ms(420), Target::Part(part), 0, 360);
                        base + ms(820)
                    }
                    _ => (typed + SETTLE).max(now),
                };
                let line = &mut self.lines[open];
                line.tail = tail;
                line.result = Some(result);
                self.cursor = result + GAP;
                self.act(result, Act::Apply(Box::new(event), now));
            }
            Event::Stage { ref stage, ref data } => {
                self.close_open(now);
                let at = self.lines.last().and_then(|line| line.result).unwrap_or(now).max(now);
                let stage = stage.clone();
                let effects = self.stage_effects(&stage, data, at);
                self.feed_from(at, raw);
                self.act(at, Act::Apply(Box::new(event), now));
                if let Some((index, arrival)) = effects {
                    self.act(at, Act::Done(index, arrival));
                }
                self.cursor = self.cursor.max(at + GAP);
            }
            Event::Result {} => {
                self.close_open(now);
                let arrival = self.comets.iter().filter(|comet| !comet.inbound).map(Comet::end).max().unwrap_or(now).max(self.cursor).max(now);
                self.act(arrival, Act::Apply(Box::new(event), now));
                let start = arrival + ms(300);
                let head = "› journalctl -f".to_string();
                let result = start + TYPE * head.chars().count() as u32 + ms(150);
                self.line(start, head, "following".into(), Some(result), TYPE);
                self.cursor = result + GAP + ms(900);
            }
            Event::Journal { ref ident, ref message, .. } => {
                let (head, tail) = (short(ident), message.trim().to_string());
                let part = Subsystem::ALL.iter().position(|part| *part == Subsystem::of(ident)).unwrap_or(6) as u8;
                self.live(event_box(event), raw, part, 1, head, tail, now, app);
            }
            Event::Activity { ref line } => {
                let name = line.split_whitespace().nth(1).unwrap_or("");
                let part = Subsystem::ALL.iter().position(|part| *part == Subsystem::of(name)).unwrap_or(6) as u8;
                let tail = line.trim().to_string();
                self.live(event_box(event), raw, part, 0, "pacman".into(), tail, now, app);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn live(&mut self, event: Box<Event>, raw: &str, part: u8, kind: u8, head: String, tail: String, now: Instant, app: &mut App) {
        if self.cursor > now + BACKLOG {
            app.apply(*event, now);
            return;
        }
        let base = self.free(now);
        self.receive(base, Target::Edge(part), kind, 520);
        self.flashes.push((part, base + ms(260), kind));
        self.glows.push((part, base + ms(520), kind));
        self.receive(base + ms(520), Target::Part(part), kind, 380);
        let start = base + ms(900);
        let result = start + LIVE_TYPE * head.chars().count() as u32;
        self.line(start, head, tail, Some(result), LIVE_TYPE);
        self.send(result, Target::List(2), kind, 620, false);
        self.send(result, Target::Tile(1), kind, 720, false);
        self.feed_from(base, raw);
        self.act(result + ms(620), Act::Apply(event, now));
        self.cursor = result + ms(1300);
    }

    fn stage_effects(&mut self, stage: &str, data: &Value, at: Instant) -> Option<(usize, Instant)> {
        match stage {
            "updates" => {
                let arrival = self.send(at, Target::Tile(0), 0, 760, true);
                self.send(at, Target::List(0), 0, 700, true);
                let mut high = [0usize; 7];
                for update in data["updates"].as_array().into_iter().flatten() {
                    let part = Subsystem::ALL.iter().position(|part| *part == Subsystem::of(update["name"].as_str().unwrap_or_default())).unwrap_or(6);
                    high[part] += usize::from(update["risk"] == "high");
                }
                for (part, count) in high.iter().enumerate() {
                    let kind = if *count > 0 { 2 } else { 0 };
                    let reach = self.send(at, Target::Part(part as u8), kind, 420, false);
                    self.glows.push((part as u8, reach, kind));
                }
                Some((0, arrival))
            }
            "problems" => {
                let arrival = self.send(at, Target::Tile(1), 1, 720, true);
                if !self.omarchy {
                    self.send(at, Target::Tile(3), 0, 760, true);
                }
                self.send(at, Target::List(1), 2, 700, true);
                let mut hit = [false; 7];
                for problem in data["problems"].as_array().into_iter().flatten() {
                    let field = |key: &str| problem[key].as_str().unwrap_or_default();
                    let owner = crate::app::problem_part(field("kind"), field("subject"), field("package"));
                    let part = Subsystem::ALL.iter().position(|part| *part == owner).unwrap_or(6);
                    hit[part] = true;
                }
                for part in (0..7).filter(|part| hit[*part]) {
                    let reach = self.send(at, Target::Part(part as u8), 1, 420, false);
                    self.glows.push((part as u8, reach, 1));
                }
                Some((1, arrival))
            }
            "anatomy" => {
                self.send(at, Target::Tile(4), 0, 760, true);
                self.send(at, Target::List(2), 1, 700, true);
                None
            }
            "news" => {
                if self.omarchy {
                    self.send(at, Target::Tile(3), 0, 760, true);
                }
                Some((2, self.send(at, Target::Tile(5), 0, 760, true)))
            }
            "preflight" => {
                self.send(at, Target::Tile(4), 0, 760, false);
                Some((3, self.send(at, Target::Tile(5), 0, 760, false)))
            }
            _ => None,
        }
    }

    pub fn select(&mut self, app: &App, now: Instant) {
        if !self.enabled {
            return;
        }
        if let Some(tile) = app.tile {
            self.pending_select = None;
            self.attention = Some((now, Target::Tile(tile as u8)));
            self.last_activity = now;
            return;
        }
        if let Some(part) = app.part {
            self.pending_select = None;
            self.attention = Some((now, Target::Part(part as u8)));
            self.last_activity = now;
            return;
        }
        if matches!(self.attention, Some((_, Target::Tile(_) | Target::Part(_)))) {
            self.attention = Some((now, Target::List(app.focus.index() as u8)));
            self.last_activity = now;
        }
        let pane = app.focus;
        let row = app.lists[pane.index()].selected;
        if self.shown_select[pane.index()] == Some(row) && self.pending_select.is_none() {
            return;
        }
        self.pending_select = Some((pane, row, now));
    }

    fn fire_select(&mut self, app: &App, now: Instant) {
        let Some((pane, row, at)) = self.pending_select else { return };
        if now < at + ms(150) {
            return;
        }
        self.pending_select = None;
        if app.focus != pane || app.lists[pane.index()].selected != row {
            return;
        }
        self.shown_select[pane.index()] = Some(row);
        let (head, tail) = match pane {
            Pane::Updates => match app.visible_updates().get(row) {
                Some(update) => (format!("▸ {}", update.name), format!("{} → {} · {}", crate::page::short_version(&update.old), crate::page::short_version(&update.new), update.risk)),
                None => return,
            },
            Pane::Problems => match app.visible_problems().get(row) {
                Some(problem) => {
                    let (one, many) = match problem.kind.as_str() {
                        "crash" => ("crash", "crashes"),
                        "failed-unit" => ("failed unit", "failed units"),
                        "config" => ("config error", "config errors"),
                        _ => ("error", "errors"),
                    };
                    (format!("▸ {}", short(problem.subject.rsplit('/').next().unwrap_or(&problem.subject))), format!("{} {}", crate::page::thousands(problem.count), if problem.count == 1 { one } else { many }))
                }
                None => return,
            },
            Pane::Events => match app.visible_events().get(row) {
                Some(event) => (format!("▸ {}", short(&event.ident)), event.message.clone()),
                None => return,
            },
        };
        self.last_activity = now;
        self.attention = Some((now, Target::List(pane.index() as u8)));
        self.line(now, head, tail, Some(now), ms(1));
    }

    pub fn step(&mut self, app: &mut App, now: Instant, dt: f32) {
        if !self.enabled {
            return;
        }
        self.fire_select(app, now);
        if self.acts.iter().any(|(at, _, _)| *at <= now) {
            self.acts.sort_by_key(|(at, sequence, _)| (*at, *sequence));
            let due = self.acts.partition_point(|(at, _, _)| *at <= now);
            for (_, _, act) in self.acts.drain(..due).collect::<Vec<_>>() {
                match act {
                    Act::Apply(event, arrived) => app.apply(*event, arrived),
                    Act::Done(index, arrival) => {
                        if app.stage_done_at[index].is_some() {
                            app.stage_done_at[index] = Some(arrival);
                        }
                    }
                    Act::Feed(text) => {
                        self.feeds += 1;
                        self.before = std::mem::replace(&mut self.feed, (self.feeds, text));
                        self.switch = self.clock;
                    }
                }
            }
        }
        let running = self.typing(now) || self.open.is_some();
        let flying = self.comets.iter().any(|comet| now >= comet.start && now <= comet.end());
        if running || flying {
            self.last_activity = self.last_activity.max(now);
        }
        let idle = now.saturating_duration_since(self.last_activity) > IDLE;
        let goal = if running {
            1.0
        } else if flying {
            0.7
        } else if idle {
            0.0
        } else {
            0.2
        };
        let dt = dt.clamp(0.0, 0.25);
        self.level += (goal - self.level) * (1.0 - (-dt / 0.6).exp());
        if self.level < 0.004 && goal == 0.0 {
            self.level = 0.0;
        }
        self.clock += dt * (2.5 + 6.0 * self.level);
        while now >= self.turn.start + self.turn.length + self.turn.hold {
            let begin = self.turn.start + self.turn.length + self.turn.hold;
            let thinking = self.typing(begin) || self.open.is_some() || self.comets.iter().any(|comet| begin >= comet.start && begin <= comet.end());
            let resting = now.saturating_duration_since(self.last_activity) > IDLE;
            self.turns = self.turns.wrapping_add(1);
            let seed = self.turns.wrapping_mul(7);
            self.side = if chance(seed + 1) < 0.8 { -self.side } else { self.side };
            let (amplitude, hold) = if thinking {
                (0.16 + 0.30 * chance(seed + 2), 140 + (chance(seed + 3) * 380.0) as u64)
            } else if resting {
                (0.0, 1200)
            } else {
                (0.02 + 0.04 * chance(seed + 2), 900 + (chance(seed + 3) * 1400.0) as u64)
            };
            let to = self.side * amplitude;
            let length = ((420.0 + (to - self.turn.to).abs() * 1100.0) * (0.85 + 0.3 * chance(seed + 4))) as u64;
            self.turn = Turn { start: begin.max(now.checked_sub(ms(1)).unwrap_or(now)), from: self.turn.to, to, length: ms(length.max(1)), hold: ms(hold) };
        }
        let x = (now.saturating_duration_since(self.turn.start).as_secs_f32() / self.turn.length.as_secs_f32()).clamp(0.0, 1.0);
        self.angle = self.turn.from + (self.turn.to - self.turn.from) * x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
        self.comets.retain(|comet| comet.end() + LOOK > now || comet.start > now);
        self.holds.retain(|(_, release)| *release > now);
        self.glows.retain(|(_, at, _)| *at > now || now.saturating_duration_since(*at) < Duration::from_secs(4));
        self.flashes.retain(|(_, at, _)| *at > now || now.saturating_duration_since(*at) < Duration::from_secs(3));
        if self.lines.len() > LINES * 2 {
            let excess = self.lines.len() - LINES;
            let ready = self.lines.iter().take(excess).all(|line| line.result.is_some_and(|result| result <= now));
            if ready && self.open.is_none_or(|open| open >= excess) {
                self.lines.drain(..excess);
                if let Some(open) = self.open.as_mut() {
                    *open -= excess;
                }
            }
        }
    }

    fn resting(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_activity) > IDLE + REST
    }

    fn typing(&self, now: Instant) -> bool {
        self.lines.iter().rev().take(3).any(|line| line.start <= now && line.result.is_none_or(|result| now < result))
    }

    pub fn signals(&self, now: Instant) -> Vec<Signal> {
        self.comets
            .iter()
            .filter(|comet| now >= comet.start && now <= comet.end())
            .map(|comet| {
                let x = now.saturating_duration_since(comet.start).as_secs_f32() / comet.duration.as_secs_f32().max(0.001);
                let eased = ease(x);
                Signal { target: comet.target, progress: if comet.inbound { 1.0 - eased } else { eased }, inbound: comet.inbound, kind: comet.kind }
            })
            .collect()
    }

    pub fn held(&self, target: Target, now: Instant) -> bool {
        self.enabled && self.holds.iter().any(|(held, release)| *held == target && now < *release)
    }

    fn decay(list: &[(u8, Instant, u8)], part: u8, now: Instant, tau: f32) -> (u8, f32) {
        let mut best = (0u8, 0.0f32);
        for (who, at, kind) in list {
            if *who == part && now >= *at {
                let strength = (-now.saturating_duration_since(*at).as_secs_f32() * 1000.0 / tau).exp();
                if strength > best.1 {
                    best = (*kind, strength);
                }
            }
        }
        best
    }

    pub fn glow(&self, part: u8, now: Instant) -> (u8, f32) {
        Self::decay(&self.glows, part, now, 520.0)
    }

    pub fn flash(&self, part: u8, now: Instant) -> (u8, f32) {
        Self::decay(&self.flashes, part, now, 380.0)
    }

    pub fn look(&self, now: Instant) -> Option<Target> {
        let mut look: Option<(Instant, Target)> = self.attention.filter(|(start, _)| now >= *start && now <= *start + GLANCE + LOOK);
        for comet in &self.comets {
            if now >= comet.start && now <= comet.end() + LOOK && look.is_none_or(|(start, _)| comet.start > start) {
                look = Some((comet.start, comet.target));
            }
        }
        look.map(|(_, target)| target.folded())
    }

    pub fn gaze(&self, now: Instant, target: (f32, f32)) -> (f32, f32) {
        let next = match self.gaze.get() {
            Some((then, mut position, mut velocity)) if now > then && now.duration_since(then).as_secs_f32() < 1.0 => {
                let mut left = now.duration_since(then).as_secs_f32();
                let omega = 11.0f32;
                while left > 0.0 {
                    let step = left.min(1.0 / 240.0);
                    let pull = |x: f32, v: f32, goal: f32| omega * omega * (goal - x) - 2.0 * omega * v;
                    velocity.0 += pull(position.0, velocity.0, target.0) * step;
                    velocity.1 += pull(position.1, velocity.1, target.1) * step;
                    position.0 += velocity.0 * step;
                    position.1 += velocity.1 * step;
                    left -= step;
                }
                (now, position, velocity)
            }
            Some((then, position, velocity)) if now == then => (now, position, velocity),
            _ => (now, target, (0.0, 0.0)),
        };
        self.gaze.set(Some(next));
        next.1
    }

    fn gaze_moving(&self, now: Instant) -> bool {
        self.gaze.get().is_some_and(|(then, _, velocity)| now.saturating_duration_since(then) < Duration::from_secs(1) && velocity.0.hypot(velocity.1) > 0.002)
    }

    pub fn sight(&self, now: Instant, chosen: Option<Target>) -> Sight {
        let settled = now.checked_sub(Duration::from_secs(2)).unwrap_or(now);
        let next = match self.sight.get() {
            Some((current, since, before, left)) if now >= since => {
                if current == chosen { (current, since, before, left) } else { (chosen, now, current, now) }
            }
            _ => (chosen, settled, None, settled),
        };
        self.sight.set(Some(next));
        next
    }

    fn sight_moving(&self, now: Instant) -> bool {
        self.sight.get().is_some_and(|(_, since, before, left)| now.saturating_duration_since(since) < ms(500) || (before.is_some() && now.saturating_duration_since(left) < ms(200)))
    }

    pub fn pupil(&self, now: Instant) -> (Vec<(String, String, usize)>, bool, f32) {
        let shown: Vec<&Line> = self.lines.iter().filter(|line| line.start <= now).collect();
        let mut caret = false;
        let mut out = Vec::new();
        for line in shown.iter().skip(shown.len().saturating_sub(LINES)) {
            let complete = line.result.is_some_and(|result| now >= result);
            if complete {
                out.push((line.head.clone(), line.tail.clone(), usize::MAX));
                caret = self.resting(now) || (now.saturating_duration_since(line.start).as_millis() / 530).is_multiple_of(2);
            } else {
                let typed = (now.saturating_duration_since(line.start).as_millis() / line.speed.as_millis().max(1)) as usize;
                out.push((line.head.clone(), String::new(), typed.min(line.head.chars().count())));
                caret = true;
            }
        }
        let newest = shown.last().map(|line| line.start);
        let since = newest.map_or(1.0, |start| now.saturating_duration_since(start).as_secs_f32() / 0.18);
        let scroll = if since < 1.0 { 1.0 - ease(since) } else { 0.0 };
        (out, caret, scroll)
    }

    pub fn turn(&self) -> f32 {
        self.angle
    }

    pub fn rain(&self, now: Instant) -> Option<Rain> {
        if !self.enabled || self.level <= 0.0 || self.feed.1.is_empty() {
            return None;
        }
        let mut tint: Option<(u8, f32)> = None;
        for comet in self.comets.iter().filter(|comet| comet.inbound && comet.kind > 0 && matches!(comet.target, Target::Part(_) | Target::Tile(_) | Target::List(_))) {
            let arrival = comet.end();
            if now >= arrival {
                let strength = (-now.saturating_duration_since(arrival).as_secs_f32() * 1000.0 / 600.0).exp();
                if strength > 0.02 && tint.is_none_or(|(_, best)| strength > best) {
                    tint = Some((comet.kind, strength));
                }
            }
        }
        Some(Rain { clock: self.clock, level: self.level, text: self.feed.1.clone(), before: self.before.1.clone(), feed: self.feed.0, previous: self.before.0, switch: self.switch, tint })
    }

    pub fn moving(&self, now: Instant) -> bool {
        if !self.enabled {
            return false;
        }
        self.level > 0.0
            || self.acts.iter().any(|(at, _, _)| *at <= now + ms(40))
            || self.pending_select.is_some()
            || self.comets.iter().any(|comet| now <= comet.end() + LOOK)
            || self.glows.iter().chain(&self.flashes).any(|(_, at, _)| now < *at + Duration::from_millis(1600))
            || self.typing(now)
            || self.lines.last().is_some_and(|line| now.saturating_duration_since(line.start) < ms(400))
            || (now < self.turn.start + self.turn.length && (self.turn.to - self.turn.from).abs() > 1e-4)
            || self.gaze_moving(now)
            || self.sight_moving(now)
    }

    pub fn wake(&self, now: Instant) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        let mut next: Option<Instant> = self.acts.iter().map(|(at, _, _)| *at).min();
        let mut consider = |at: Instant| {
            if at > now {
                next = Some(next.map_or(at, |next: Instant| next.min(at)));
            }
        };
        consider(self.turn.start + self.turn.length + self.turn.hold);
        if self.resting(now) {
            consider(self.last_activity + IDLE + REST);
        } else if let Some(line) = self.lines.iter().rev().find(|line| line.start <= now) {
            let elapsed = now.saturating_duration_since(line.start).as_millis() as u64;
            consider(now + ms(530 - elapsed % 530));
        }
        if let Some(line) = self.lines.iter().find(|line| line.start > now) {
            consider(line.start);
        }
        for comet in &self.comets {
            consider(comet.start);
        }
        next.map(|at| at.saturating_duration_since(now))
    }
}

fn event_box(event: Event) -> Box<Event> {
    Box::new(event)
}
