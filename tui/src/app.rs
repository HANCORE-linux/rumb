use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::source::Event;

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub name: String,
    pub old: String,
    pub new: String,
    pub source: String,
    #[serde(default)]
    pub bump: String,
    pub risk: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub download_bytes: Option<u64>,
    #[serde(default)]
    pub installed_delta_bytes: Option<i64>,
}

impl Update {
    pub fn risk_rank(&self) -> u8 {
        match self.risk.as_str() {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    Kernel,
    Graphics,
    Compositor,
    Toolkit,
    Shell,
    Base,
    Apps,
}

impl Subsystem {
    pub const ALL: [Subsystem; 7] = [
        Subsystem::Kernel,
        Subsystem::Graphics,
        Subsystem::Compositor,
        Subsystem::Toolkit,
        Subsystem::Shell,
        Subsystem::Base,
        Subsystem::Apps,
    ];

    /// Which part of the system a package belongs to, by name.
    pub fn of(name: &str) -> Self {
        let starts = |prefix: &str| name.starts_with(prefix);
        if name == "linux" || starts("linux-") || name == "kernel" {
            Subsystem::Kernel
        } else if ["mesa", "lib32-mesa", "nvidia", "lib32-nvidia", "vulkan", "lib32-vulkan", "libva", "intel-media"].iter().any(|p| starts(p)) {
            Subsystem::Graphics
        } else if starts("hypr") || matches!(name, "aquamarine" | "xdg-desktop-portal-hyprland" | "uwsm" | "sddm" | "Hyprland") {
            Subsystem::Compositor
        } else if ["qt5-", "qt6-", "gtk", "libadwaita"].iter().any(|p| starts(p)) {
            Subsystem::Toolkit
        } else if starts("omarchy") || starts("walker") || starts("elephant") || matches!(name, "quickshell" | "waybar" | "mako" | "swayosd") {
            Subsystem::Shell
        } else if starts("limine")
            || matches!(name, "systemd" | "systemd-libs" | "glibc" | "lib32-glibc" | "pacman" | "mkinitcpio" | "bash" | "coreutils" | "openssl" | "util-linux" | "filesystem" | "dbus-broker" | "bluez")
        {
            Subsystem::Base
        } else {
            Subsystem::Apps
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Subsystem::Kernel => "kernel",
            Subsystem::Graphics => "graphics",
            Subsystem::Compositor => "compositor",
            Subsystem::Toolkit => "toolkit",
            Subsystem::Shell => "shell",
            Subsystem::Base => "base",
            Subsystem::Apps => "other",
        }
    }
}

pub fn problem_part(kind: &str, subject: &str, package: &str) -> Subsystem {
    if kind == "config" {
        Subsystem::Compositor
    } else if kind == "journal" && subject == "kernel" {
        Subsystem::Kernel
    } else if package.is_empty() {
        Subsystem::Apps
    } else {
        Subsystem::of(package)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Deps {
    pub depends: u64,
    pub required_by: u64,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub required_names: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JournalLine {
    pub time: String,
    #[serde(default)]
    pub day: String,
    pub ident: String,
    #[serde(default, deserialize_with = "crate::source::or_empty")]
    pub program: String,
    pub message: String,
    #[serde(default, deserialize_with = "crate::source::or_empty")]
    pub priority: String,
    #[serde(default, deserialize_with = "crate::source::or_empty")]
    pub unit: String,
}

/// Facts about the installed software as a whole.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Anatomy {
    pub packages: u64,
    pub explicit: u64,
    pub orphans: u64,
    pub cache_bytes: u64,
    pub kernel_running: String,
    #[serde(default)]
    pub kernel_installed: String,
    #[serde(default)]
    pub kernel_package: String,
    #[serde(default)]
    pub cache_freeable_bytes: Option<u64>,
    #[serde(default)]
    pub reboot_pending: bool,
    #[serde(default)]
    pub upgrades_per_day: Vec<u64>,
    #[serde(default)]
    pub repos: BTreeMap<String, u64>,
    #[serde(default)]
    pub deps: HashMap<String, Deps>,
    #[serde(default)]
    pub journal: Vec<JournalLine>,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub desktop: String,
    #[serde(default)]
    pub omarchy: String,
    #[serde(default)]
    pub orphan_names: Vec<String>,
    #[serde(default)]
    pub uptime_seconds: Option<u64>,
    #[serde(default)]
    pub nvidia_loaded: String,
    #[serde(default)]
    pub nvidia_installed: String,
    #[serde(default)]
    pub hyprland_running: String,
    #[serde(default)]
    pub hyprland_installed: String,
}

#[derive(Debug, Clone, Default)]
pub struct Health {
    pub kernel: Option<KernelHealth>,
    pub nvidia: Option<(String, String)>,
    pub hyprland: Option<(String, String)>,
    pub config_errors: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct KernelHealth {
    pub running: String,
    pub installed: String,
    pub package: String,
    pub uptime_seconds: Option<u64>,
    pub reboot_pending: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Problem {
    pub kind: String,
    pub subject: String,
    pub count: u64,
    #[serde(default)]
    pub package: String,
    pub status: String,
    #[serde(default)]
    pub update: String,
    #[serde(default)]
    pub signal: String,
    #[serde(default)]
    pub frames: String,
    #[serde(default)]
    pub last: String,
    #[serde(default)]
    pub message: String,
}

impl Problem {
    pub fn part(&self) -> Subsystem {
        problem_part(&self.kind, &self.subject, &self.package)
    }

    pub fn name(&self) -> &str {
        if self.kind == "journal" && self.subject == "kernel" {
            return "kernel";
        }
        if self.kind == "config" {
            return "Hyprland";
        }
        self.subject.rsplit('/').next().unwrap_or(&self.subject)
    }

    /// The version the fixing update brings, from "pkg old -> new".
    pub fn fix_version(&self) -> &str {
        self.update.rsplit("-> ").next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct OmarchyNotes {
    pub releases: u64,
    pub fixes: u64,
    pub improvements: u64,
    pub security: u64,
    #[serde(default)]
    pub per_release: Vec<OmarchyRelease>,
    #[serde(default)]
    pub titles: Vec<String>,
    #[serde(default)]
    pub security_titles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OmarchyRelease {
    pub tag: String,
    pub fixes: u64,
    pub improvements: u64,
    pub security: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct News {
    pub date: String,
    pub title: String,
    #[serde(default)]
    pub link: String,
    pub manual_intervention: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Activity {
    pub date: String,
    pub crashes: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Preflight {
    pub free_bytes: u64,
    pub free_ok: bool,
    pub snapper_configured: bool,
    #[serde(default)]
    pub snapshot_by: String,
    #[serde(default)]
    pub disk_total_bytes: u64,
    #[serde(default)]
    pub disk_used_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Waiting,
    Running,
    Done,
}

pub const STAGES: [&str; 4] = ["updates", "problems", "news", "preflight"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Warn,
    Bad,
    Neutral,
}

pub struct Step {
    pub label: String,
    pub started: Instant,
    pub ended: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Updates,
    Problems,
    Events,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Updates, Pane::Problems, Pane::Events];

    pub fn index(self) -> usize {
        match self {
            Pane::Updates => 0,
            Pane::Problems => 1,
            Pane::Events => 2,
        }
    }

    pub fn panel(self) -> usize {
        self.index() + 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Journal,
    Pacman,
}

pub struct EventLine {
    pub at: Instant,
    pub clock: String,
    pub day: String,
    pub source: Source,
    pub ident: String,
    pub program: String,
    pub message: String,
    pub priority: String,
    pub unit: String,
}

#[derive(Default)]
pub struct ListState {
    pub selected: usize,
    pub filter: String,
}

pub const MAX_EVENTS: usize = 10_000;

pub fn ease_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub struct App {
    pub demo: bool,
    pub started: Instant,
    pub stages: [StageState; 4],
    pub stage_done_at: [Option<Instant>; 4],
    pub steps: Vec<Step>,
    pub scan_time: Option<String>,
    pub last_upgrade: Option<String>,
    pub last_upgraded: Vec<String>,
    pub omarchy: bool,
    pub since: Option<String>,
    pub updates: Vec<Update>,
    pub update_errors: Vec<String>,
    pub aur_unavailable: bool,
    pub problems: Vec<Problem>,
    pub problem_errors: Vec<String>,
    pub activity: Vec<Activity>,
    pub news: Vec<News>,
    pub news_failed: bool,
    pub omarchy_notes: Option<OmarchyNotes>,
    pub preflight: Option<Preflight>,
    pub anatomy: Option<Anatomy>,
    pub anatomy_at: Option<Instant>,
    pub journal_live: usize,
    pub events: Vec<EventLine>,
    pub event_at: Option<Instant>,
    pub start_at: Option<Instant>,
    pub transaction: Option<(Instant, Option<Instant>)>,
    pub applied: Vec<(Update, Instant)>,
    pub applied_total: usize,
    pub full_upgrade: bool,
    pub full_upgrade_at: Option<Instant>,
    pub visible: [bool; 5],
    pub focus: Pane,
    pub lists: [ListState; 3],
    pub editing: bool,
    pub overlay: bool,
    pub tile: Option<usize>,
    pub last_tile: usize,
    pub part: Option<usize>,
    pub last_part: usize,
    pub updates_map: bool,
    pub map_fits: bool,
    pub pictures: bool,
    pub follow: bool,
    pub status: Option<(String, bool, Instant)>,
    pub finished_at: Option<Instant>,
    pub error: Option<String>,
    pub live_lost: bool,
    pub skipped: bool,
    pub health: Health,
}

impl App {
    pub fn new(demo: bool, animate: bool, now: Instant) -> Self {
        App {
            demo,
            started: now,
            stages: [StageState::Waiting; 4],
            stage_done_at: [None; 4],
            steps: Vec::new(),
            scan_time: None,
            last_upgrade: None,
            last_upgraded: Vec::new(),
            omarchy: false,
            since: None,
            updates: Vec::new(),
            update_errors: Vec::new(),
            aur_unavailable: false,
            problem_errors: Vec::new(),
            problems: Vec::new(),
            activity: Vec::new(),
            news: Vec::new(),
            news_failed: false,
            omarchy_notes: None,
            preflight: None,
            anatomy: None,
            anatomy_at: None,
            journal_live: 0,
            events: Vec::new(),
            event_at: None,
            start_at: None,
            transaction: None,
            applied: Vec::new(),
            applied_total: 0,
            full_upgrade: false,
            full_upgrade_at: None,
            visible: [true; 5],
            focus: Pane::Updates,
            lists: Default::default(),
            editing: false,
            overlay: false,
            tile: None,
            last_tile: 0,
            part: None,
            last_part: 0,
            updates_map: true,
            map_fits: false,
            pictures: true,
            follow: true,
            status: None,
            finished_at: None,
            error: None,
            live_lost: false,
            skipped: !animate,
            health: Health::default(),
        }
    }

    pub fn apply(&mut self, event: Event, now: Instant) {
        match event {
            Event::Start { last_upgrade, problems_since, now: scan_time, omarchy, last_upgraded } => {
                self.start_at.get_or_insert(now);
                self.omarchy = omarchy;
                self.last_upgraded = last_upgraded;
                self.scan_time = scan_time;
                self.last_upgrade = last_upgrade;
                self.since = Some(problems_since);
            }
            Event::Step { stage, label } => {
                if let Some(index) = stage_index(&stage)
                    && self.stages[index] == StageState::Waiting
                {
                    self.stages[index] = StageState::Running;
                }
                self.end_step(now);
                self.steps.push(Step { label, started: now, ended: None });
            }
            Event::Stage { stage, data } => {
                if let Err(error) = self.stage(&stage, data, now) {
                    self.error = Some(format!("rumb-scan sent unreadable {stage} data: {error}"));
                    self.finish(now);
                }
            }
            Event::Done { label, value, .. } => self.done(&label, &value),
            Event::Result {} => self.finish(now),
            Event::Activity { line } => self.activity(&line, now),
            Event::Journal { time, day, ident, program, message, priority, unit } => self.journal(JournalLine { time, day, ident, program, message, priority, unit }, now),
        }
    }

    fn done(&mut self, label: &str, value: &Value) {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        match label {
            "reading the running kernel" => {
                self.health.kernel = Some(KernelHealth { running: text("running"), installed: text("installed"), package: text("package"), uptime_seconds: value["uptime_seconds"].as_u64(), reboot_pending: value["reboot_pending"].as_bool().unwrap_or(false) });
            }
            "reading the loaded nvidia module" => self.health.nvidia = Some((text("loaded"), text("installed"))),
            "reading the running Hyprland" => self.health.hyprland = Some((text("running"), text("installed"))),
            "checking Hyprland config" => self.health.config_errors = value.as_u64(),
            _ => {}
        }
    }

    fn stage(&mut self, stage: &str, data: Value, now: Instant) -> Result<(), serde_json::Error> {
        if stage == "anatomy" {
            let anatomy: Anatomy = serde_json::from_value(data)?;
            if self.health.kernel.is_none() && !anatomy.kernel_running.is_empty() {
                self.health.kernel = Some(KernelHealth { running: anatomy.kernel_running.clone(), installed: anatomy.kernel_installed.clone(), package: anatomy.kernel_package.clone(), uptime_seconds: anatomy.uptime_seconds, reboot_pending: anatomy.reboot_pending });
            }
            if self.health.nvidia.is_none() && !anatomy.nvidia_loaded.is_empty() {
                self.health.nvidia = Some((anatomy.nvidia_loaded.clone(), anatomy.nvidia_installed.clone()));
            }
            if self.health.hyprland.is_none() && !anatomy.hyprland_running.is_empty() {
                self.health.hyprland = Some((anatomy.hyprland_running.clone(), anatomy.hyprland_installed.clone()));
            }
            if self.events.is_empty() {
                for line in &anatomy.journal {
                    self.events.push(EventLine { at: now, clock: line.time.clone(), day: line.day.clone(), source: Source::Journal, ident: line.ident.clone(), program: line.program.clone(), message: line.message.trim().to_string(), priority: line.priority.clone(), unit: line.unit.clone() });
                }
            }
            self.anatomy = Some(anatomy);
            self.anatomy_at = Some(now);
            return Ok(());
        }
        let Some(index) = stage_index(stage) else { return Ok(()) };
        match index {
            0 => {
                self.updates = field(&data, "updates")?;
                self.update_errors = field(&data, "update_check_errors")?;
                self.aur_unavailable = field(&data, "aur_unavailable")?;
            }
            1 => {
                self.problems = field(&data, "problems")?;
                self.activity = field(&data, "activity")?;
                self.problem_errors = field(&data, "problem_check_errors")?;
                if let Some(errors) = data.get("config_errors").and_then(Value::as_u64) {
                    self.health.config_errors = Some(errors);
                }
                self.journal_live = 0;
            }
            2 => {
                self.news = field(&data, "news")?;
                self.news_failed = field(&data, "news_check_failed")?;
                self.omarchy_notes = data.get("omarchy_notes").cloned().and_then(|notes| serde_json::from_value(notes).ok());
            }
            _ => {
                self.preflight = Some(serde_json::from_value(data)?);
            }
        }
        self.stages[index] = StageState::Done;
        self.stage_done_at[index] = Some(now);
        Ok(())
    }

    fn journal(&mut self, line: JournalLine, now: Instant) {
        if self.finished_at.is_some() {
            self.journal_live += 1;
        }
        self.push_event(EventLine { at: now, clock: line.time, day: line.day, source: Source::Journal, ident: line.ident, program: line.program, message: line.message.trim().to_string(), priority: line.priority, unit: line.unit }, now);
    }

    fn push_event(&mut self, event: EventLine, now: Instant) {
        if !self.follow && self.shows_event(&event) {
            self.lists[Pane::Events.index()].selected += 1;
        }
        self.events.push(event);
        let excess = self.events.len().saturating_sub(MAX_EVENTS);
        self.events.drain(..excess);
        self.event_at = Some(now);
    }

    fn shows_event(&self, event: &EventLine) -> bool {
        let filter = self.lists[Pane::Events.index()].filter.to_lowercase();
        filter.is_empty() || format!("{} {} {}", event.ident, event.program, event.message).to_lowercase().contains(&filter)
    }

    fn activity(&mut self, line: &str, now: Instant) {
        let line = line.trim();
        let mut words = line.splitn(3, ' ');
        let (action, name, rest) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
        let versions = rest.trim_matches(|c| c == '(' || c == ')');
        match (action, name) {
            ("transaction", "started") => {
                self.transaction = Some((now, None));
                self.applied.clear();
                self.applied_total = self.updates.len();
            }
            ("starting", "full") => self.full_upgrade = true,
            ("transaction", "completed") => {
                if let Some((_, end)) = &mut self.transaction {
                    *end = Some(now);
                }
                if std::mem::take(&mut self.full_upgrade) {
                    self.full_upgrade_at = Some(now);
                }
            }
            ("upgraded" | "downgraded" | "installed" | "removed" | "reinstalled", _) => {
                if let Some(position) = self.updates.iter().position(|update| update.name == name) {
                    let installed = versions.split_once(" -> ").map_or(versions, |(_, new)| new);
                    match action {
                        "upgraded" | "installed" => {
                            let mut update = self.updates.remove(position);
                            update.new = installed.to_string();
                            self.applied.push((update, now));
                        }
                        "removed" => {
                            self.updates.remove(position);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let clock = clock_now();
        self.push_event(EventLine { at: now, clock, day: today(), source: Source::Pacman, ident: "pacman".into(), program: String::new(), message: line.to_string(), priority: String::new(), unit: String::new() }, now);
    }

    pub fn transacting(&self) -> bool {
        self.transaction.is_some_and(|(_, end)| end.is_none())
    }

    fn end_step(&mut self, now: Instant) {
        if let Some(step) = self.steps.last_mut()
            && step.ended.is_none()
        {
            step.ended = Some(now);
        }
    }

    pub fn fail(&mut self, message: String, now: Instant) {
        if self.finished_at.is_none() {
            self.error = Some(message);
            self.finish(now);
        }
    }

    fn finish(&mut self, now: Instant) {
        if self.finished_at.is_some() {
            return;
        }
        self.end_step(now);
        self.finished_at = Some(now);
    }

    pub fn high(&self) -> usize {
        self.updates.iter().filter(|update| update.risk == "high").count()
    }

    pub fn medium(&self) -> usize {
        self.updates.iter().filter(|update| update.risk == "medium").count()
    }

    pub fn manual(&self) -> usize {
        self.news.iter().filter(|news| news.manual_intervention).count()
    }

    pub fn download_bytes(&self) -> u64 {
        self.updates.iter().filter_map(|update| update.download_bytes).sum()
    }

    pub fn installed_delta(&self) -> i64 {
        self.updates.iter().filter_map(|update| update.installed_delta_bytes).sum()
    }

    pub fn aur_updates(&self) -> usize {
        self.updates.iter().filter(|update| update.source == "aur").count()
    }

    pub fn space_needed(&self) -> u64 {
        self.download_bytes() + self.installed_delta().max(0).unsigned_abs()
    }

    pub fn space_severity(&self) -> Option<Severity> {
        let preflight = self.preflight.as_ref()?;
        Some(if preflight.free_bytes < self.space_needed() {
            Severity::Bad
        } else if !preflight.free_ok {
            Severity::Warn
        } else {
            Severity::Ok
        })
    }

    pub fn reboot_after(&self) -> Vec<(&'static str, &Update)> {
        let mut parts = Vec::new();
        let kernel = self.health.kernel.as_ref().map(|kernel| kernel.package.as_str()).filter(|name| !name.is_empty());
        if let Some(update) = kernel.and_then(|name| self.updates.iter().find(|update| update.name == name)) {
            parts.push(("kernel", update));
        }
        if self.health.nvidia.is_some()
            && let Some(update) = self.updates.iter().find(|update| update.name == "nvidia-utils")
        {
            parts.push(("nvidia", update));
        }
        parts
    }

    pub fn live_feed(&self) -> bool {
        self.finished_at.is_some() && !self.live_lost && self.error.is_none()
    }

    pub fn incomplete(&self) -> bool {
        !self.update_errors.is_empty() || !self.problem_errors.is_empty() || self.news_failed
    }

    pub fn sorted_updates(&self) -> Vec<&Update> {
        let mut updates: Vec<&Update> = self.updates.iter().collect();
        updates.sort_by(|a, b| a.risk_rank().cmp(&b.risk_rank()).then_with(|| a.name.cmp(&b.name)));
        updates
    }

    /// Verdict headline and its severity.
    pub fn verdict(&self) -> (String, Severity) {
        if self.error.is_some() {
            return ("SCAN FAILED".into(), Severity::Bad);
        }
        if self.high() > 0 || self.manual() > 0 {
            let severity = if self.high() > 0 { Severity::Bad } else { Severity::Warn };
            return ("REVIEW FIRST".into(), severity);
        }
        if self.incomplete() {
            return ("INCOMPLETE".into(), Severity::Warn);
        }
        if self.updates.is_empty() {
            return ("UP TO DATE".into(), Severity::Neutral);
        }
        ("READY TO UPDATE".into(), Severity::Ok)
    }
}

impl App {
    pub fn visible_updates(&self) -> Vec<&Update> {
        let filter = self.lists[0].filter.to_lowercase();
        self.sorted_updates()
            .into_iter()
            .filter(|update| filter.is_empty() || format!("{} {} {} {} {} {} {}", update.name, update.old, update.new, update.risk, update.bump, update.source, Subsystem::of(&update.name).label()).to_lowercase().contains(&filter))
            .collect()
    }

    pub fn problems_by_count(&self) -> Vec<&Problem> {
        let mut problems: Vec<&Problem> = self.problems.iter().collect();
        problems.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.subject.cmp(&b.subject)));
        problems
    }

    pub fn visible_problems(&self) -> Vec<&Problem> {
        let filter = self.lists[1].filter.to_lowercase();
        self.problems_by_count()
            .into_iter()
            .filter(|problem| filter.is_empty() || format!("{} {} {} {} {}", problem.kind, problem.subject, problem.package, problem.status, problem.update).to_lowercase().contains(&filter))
            .collect()
    }

    pub fn visible_events(&self) -> Vec<&EventLine> {
        self.events.iter().rev().filter(|event| self.shows_event(event)).collect()
    }

    pub fn rows(&self, pane: Pane) -> usize {
        match pane {
            Pane::Updates => self.visible_updates().len(),
            Pane::Problems => self.visible_problems().len(),
            Pane::Events => self.visible_events().len(),
        }
    }

    pub fn select(&mut self, pane: Pane, target: isize) {
        let rows = self.rows(pane);
        let selected = target.clamp(0, rows.saturating_sub(1) as isize) as usize;
        self.lists[pane.index()].selected = selected;
        if pane == Pane::Events {
            self.follow = selected == 0;
        }
    }

    pub fn toggle(&mut self, panel: usize) {
        self.visible[panel] = !self.visible[panel];
        if panel == 0 && !self.visible[0] {
            self.leave_tiles();
        }
        if panel == 1 && !self.visible[1] {
            self.leave_band();
        }
        if !self.visible[self.focus.panel()]
            && let Some(next) = Pane::ALL.into_iter().find(|pane| self.visible[pane.panel()])
        {
            self.focus = next;
        }
    }

    pub fn lists_shown(&self) -> bool {
        self.visible[2..].iter().any(|on| *on)
    }

    pub fn list_focused(&self, pane: Pane) -> bool {
        self.tile.is_none() && self.part.is_none() && self.focus == pane
    }

    pub fn cycle_focus(&mut self, forward: bool, stops: bool) {
        #[derive(Clone, Copy, PartialEq)]
        enum Stop {
            Tiles,
            Band,
            List(Pane),
        }
        let mut order = Vec::new();
        if stops && self.visible[0] {
            order.push(Stop::Tiles);
        }
        if stops && self.visible[1] {
            order.push(Stop::Band);
        }
        order.extend(Pane::ALL.into_iter().filter(|pane| self.visible[pane.panel()]).map(Stop::List));
        self.overlay = false;
        if order.is_empty() {
            return;
        }
        let current = if self.tile.is_some() {
            Stop::Tiles
        } else if self.part.is_some() {
            Stop::Band
        } else {
            Stop::List(self.focus)
        };
        let next = match order.iter().position(|stop| *stop == current) {
            Some(position) => order[if forward { (position + 1) % order.len() } else { (position + order.len() - 1) % order.len() }],
            None => order.iter().copied().find(|stop| matches!(stop, Stop::List(_))).unwrap_or(order[0]),
        };
        if next == current {
            return;
        }
        match next {
            Stop::Tiles => self.enter_tiles(),
            Stop::Band => self.enter_band(),
            Stop::List(pane) => {
                self.leave_tiles();
                self.leave_band();
                self.focus = pane;
            }
        }
    }

    pub fn press_panel(&mut self, panel: usize) {
        let updates = Pane::Updates.panel();
        if panel == updates && self.visible[panel] && self.updates_map && self.map_fits {
            self.updates_map = false;
            return;
        }
        let hiding = self.visible[panel];
        self.toggle(panel);
        if panel == updates && hiding {
            self.updates_map = true;
        }
    }

    pub fn enter_tiles(&mut self) {
        if self.visible[0] {
            self.leave_band();
            self.tile = Some(self.last_tile);
            self.overlay = false;
        }
    }

    pub fn leave_tiles(&mut self) {
        if let Some(tile) = self.tile.take() {
            self.last_tile = tile;
        }
        self.overlay = false;
    }

    pub fn move_tile(&mut self, forward: bool) {
        if let Some(tile) = self.tile {
            self.tile = Some(if forward { (tile + 1) % 6 } else { (tile + 5) % 6 });
        }
    }

    pub fn enter_band(&mut self) {
        if self.visible[1] {
            self.leave_tiles();
            self.part = Some(self.last_part);
            self.overlay = false;
        }
    }

    pub fn leave_band(&mut self) {
        if let Some(part) = self.part.take() {
            self.last_part = part;
        }
        self.overlay = false;
    }

    pub fn move_part(&mut self, forward: bool) {
        let count = Subsystem::ALL.len();
        if let Some(part) = self.part {
            self.part = Some(if forward { (part + 1) % count } else { (part + count - 1) % count });
        }
    }
}

fn local_now() -> libc::tm {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    tm
}

pub fn clock_now() -> String {
    let tm = local_now();
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn today() -> String {
    let tm = local_now();
    format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)
}

pub fn local_stamp() -> String {
    let tm = local_now();
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn stage_index(stage: &str) -> Option<usize> {
    STAGES.iter().position(|name| *name == stage)
}

fn field<T: DeserializeOwned + Default>(data: &Value, key: &str) -> Result<T, serde_json::Error> {
    match data.get(key) {
        Some(value) => serde_json::from_value(value.clone()),
        None => Ok(T::default()),
    }
}
