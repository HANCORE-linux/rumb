use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

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
            Subsystem::Apps => "apps",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Deps {
    pub depends: u64,
    pub required_by: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JournalLine {
    pub time: String,
    pub ident: String,
    pub message: String,
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
}

impl Problem {
    pub fn name(&self) -> &str {
        if self.kind == "journal" && self.subject == "kernel" {
            return "kernel";
        }
        if self.kind == "config" {
            return "Hyprland";
        }
        self.subject.rsplit('/').next().unwrap_or(&self.subject)
    }

    pub fn rank(&self) -> (u8, u8) {
        let status = match self.status.as_str() {
            "possibly-fixed" => 0,
            "config" => 1,
            _ => 2,
        };
        let kind = match self.kind.as_str() {
            "failed-unit" => 0,
            "crash" => 1,
            _ => 2,
        };
        (status, kind)
    }

    /// The version the fixing update brings, from "pkg old -> new".
    pub fn fix_version(&self) -> &str {
        self.update.rsplit("-> ").next().unwrap_or("")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct News {
    pub date: String,
    pub title: String,
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

/// A line shown inside the eye while the system changes.
pub struct LogLine {
    pub text: String,
    pub at: Instant,
}

/// How Omarchy is shown when the system runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OmarchyMark {
    Logo,
    #[allow(dead_code)]
    Wordmark,
}

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
    pub since: Option<String>,
    pub updates: Vec<Update>,
    pub update_errors: Vec<String>,
    pub problems: Vec<Problem>,
    pub activity: Vec<Activity>,
    pub news: Vec<News>,
    pub news_failed: bool,
    pub preflight: Option<Preflight>,
    pub anatomy: Option<Anatomy>,
    pub anatomy_at: Option<Instant>,
    pub journal_at: Option<Instant>,
    pub journal_live: usize,
    pub journal_last: Option<Instant>,
    pub journal_feed: Vec<(Instant, String)>,
    /// Tile that has keyboard focus, and the tile opened in detail view.
    pub focus: usize,
    /// The tile focused before, and when focus moved on from it.
    pub focus_prev: usize,
    pub focus_since: Option<Instant>,
    pub detail: Option<usize>,
    pub start_at: Option<Instant>,
    /// The package manager at work: when its transaction began and ended.
    pub transaction: Option<(Instant, Option<Instant>)>,
    /// Updates applied during the current transaction, and when.
    pub applied: Vec<(Update, Instant)>,
    /// How many updates were pending when the transaction began.
    pub applied_total: usize,
    pub full_upgrade: bool,
    pub full_upgrade_at: Option<Instant>,
    pub log: Vec<LogLine>,
    /// Package manager lines as they came in, for the ticker and the log.
    pub feed: Vec<(Instant, String)>,
    pub omarchy_mark: OmarchyMark,
    /// Which view is shown, like the presets in btop.
    pub view: usize,
    pub selected: usize,
    pub finished_at: Option<Instant>,
    pub error: Option<String>,
    /// Animations are shown at their final state.
    pub skipped: bool,
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
            since: None,
            updates: Vec::new(),
            update_errors: Vec::new(),
            problems: Vec::new(),
            activity: Vec::new(),
            news: Vec::new(),
            news_failed: false,
            preflight: None,
            anatomy: None,
            anatomy_at: None,
            journal_at: None,
            journal_live: 0,
            journal_last: None,
            journal_feed: Vec::new(),
            focus: 0,
            focus_prev: 0,
            focus_since: None,
            detail: None,
            start_at: None,
            transaction: None,
            applied: Vec::new(),
            applied_total: 0,
            full_upgrade: false,
            full_upgrade_at: None,
            log: Vec::new(),
            feed: Vec::new(),
            omarchy_mark: OmarchyMark::Logo,
            view: 0,
            selected: 0,
            finished_at: None,
            error: None,
            skipped: !animate,
        }
    }

    pub fn apply(&mut self, event: Event, now: Instant) {
        match event {
            Event::Start { last_upgrade, problems_since, now: scan_time } => {
                self.start_at = Some(now);
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
                if stage == "anatomy" {
                    self.anatomy = serde_json::from_value(data).ok();
                    self.anatomy_at = Some(now);
                    self.journal_at.get_or_insert(now);
                    return;
                }
                let Some(index) = stage_index(&stage) else { return };
                match index {
                    0 => {
                        self.updates = field(&data, "updates");
                        self.update_errors = field(&data, "update_check_errors");
                    }
                    1 => {
                        self.problems = field(&data, "problems");
                        self.activity = field(&data, "activity");
                    }
                    2 => {
                        self.news = field(&data, "news");
                        self.news_failed = data.get("news_check_failed").and_then(Value::as_bool).unwrap_or(false);
                    }
                    _ => {
                        self.preflight = serde_json::from_value(data).ok();
                    }
                }
                self.stages[index] = StageState::Done;
                self.stage_done_at[index] = Some(now);
            }
            Event::Result {} => self.finish(now),
            Event::Activity { line } => self.activity(&line, now),
            Event::Journal { time, ident, message } => self.journal(JournalLine { time, ident, message }, now),
        }
    }

    fn journal(&mut self, line: JournalLine, now: Instant) {
        let Some(anatomy) = &mut self.anatomy else { return };
        self.journal_feed.push((now, format!("{}: {}", line.ident, line.message)));
        let excess = self.journal_feed.len().saturating_sub(200);
        self.journal_feed.drain(..excess);
        anatomy.journal.push(line);
        let excess = anatomy.journal.len().saturating_sub(200);
        anatomy.journal.drain(..excess);
        if self.finished_at.is_some() {
            self.journal_live += 1;
        }
        self.journal_last = Some(now);
    }

    /// Follows the package manager's log: a transaction starts, packages get
    /// upgraded one by one, hooks run, the transaction completes.
    fn activity(&mut self, line: &str, now: Instant) {
        let line = line.trim();
        self.feed.push((now, line.to_string()));
        let excess = self.feed.len().saturating_sub(200);
        self.feed.drain(..excess);
        let mut words = line.splitn(3, ' ');
        let (action, name, rest) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
        let versions = rest.trim_matches(|c| c == '(' || c == ')');
        let short = |version: &str| version.rsplit_once('-').map_or(version, |(v, _)| v).to_string();
        // Lines stay short enough to fit across the pupil
        let clip = |text: &str| text.to_uppercase().chars().take(13).collect::<String>();
        let mut shown = Vec::new();
        match (action, name) {
            ("transaction", "started") => {
                self.transaction = Some((now, None));
                self.applied.clear();
                self.applied_total = self.updates.len();
                shown.extend(["TRANSACTION".to_string(), "STARTED".to_string()]);
            }
            ("starting", "full") => {
                self.full_upgrade = true;
                shown.extend(["FULL SYSTEM".to_string(), "UPGRADE".to_string()]);
            }
            ("transaction", "completed") => {
                if let Some((_, end)) = &mut self.transaction {
                    *end = Some(now);
                }
                if std::mem::take(&mut self.full_upgrade) {
                    self.full_upgrade_at = Some(now);
                }
                shown.extend(["TRANSACTION".to_string(), "COMPLETED".to_string()]);
            }
            ("upgraded" | "downgraded" | "installed" | "removed" | "reinstalled", _) => {
                if let Some(position) = self.updates.iter().position(|update| update.name == name) {
                    let update = self.updates.remove(position);
                    self.applied.push((update, now));
                }
                shown.extend([clip(action), clip(name)]);
                let change = match versions.split_once(" -> ") {
                    Some((old, new)) if short(old) == short(new) => new.to_string(),
                    Some((old, new)) if short(old).len() + short(new).len() < 13 => format!("{}>{}", short(old), short(new)),
                    Some((_, new)) => short(new),
                    None => short(versions),
                };
                if !change.is_empty() {
                    shown.push(clip(&change));
                }
            }
            ("running", hook) => {
                let hook = hook.trim_matches(|c| c == '\'' || c == '.').trim_end_matches(".hook");
                let hook = hook.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
                shown.extend(["HOOK".to_string(), clip(hook)]);
            }
            _ => shown.push(clip(line)),
        }
        for text in shown {
            self.log.push(LogLine { text, at: now });
        }
        let excess = self.log.len().saturating_sub(40);
        self.log.drain(..excess);
    }

    /// 0 idle, 1 working: the package manager is busy, easing in and out.
    pub fn working(&self, now: Instant) -> f64 {
        let Some((began, ended)) = self.transaction else { return 0.0 };
        if self.skipped {
            return if ended.is_some() { 0.0 } else { 1.0 };
        }
        let rise = ease_out_cubic(now.saturating_duration_since(began).as_secs_f64() / 0.45);
        match ended {
            None => rise,
            Some(end) => {
                let held = now.saturating_duration_since(end).as_secs_f64();
                rise * (1.0 - ease_out_cubic((held - 1.2) / 0.45))
            }
        }
    }

    pub fn set_view(&mut self, view: usize, now: Instant) {
        if view != self.view {
            self.view = view;
            self.detail = None;
            self.focus_prev = 0;
            self.focus = 0;
            self.focus_since = Some(now);
        }
    }

    pub fn set_focus(&mut self, index: usize, now: Instant) {
        if index != self.focus {
            self.focus_prev = self.focus;
            self.focus = index;
            self.focus_since = Some(now);
        }
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

    /// Jumps every animation to its final state.
    pub fn skip(&mut self) {
        self.skipped = true;
    }

    pub fn animating(&self, now: Instant) -> bool {
        if self.skipped {
            return false;
        }
        let recent = |at: Instant| now.saturating_duration_since(at) < Duration::from_millis(3000);
        let spotlight_moving = self.spotlight_since(now).is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(700));
        spotlight_moving
            || self.finished_at.is_none_or(recent)
            || self.stage_done_at.iter().flatten().any(|done| recent(*done))
    }

    pub fn high(&self) -> usize {
        self.updates.iter().filter(|update| update.risk == "high").count()
    }

    pub fn medium(&self) -> usize {
        self.updates.iter().filter(|update| update.risk == "medium").count()
    }

    pub fn fixes(&self) -> usize {
        self.problems.iter().filter(|problem| problem.status == "possibly-fixed").count()
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

    pub fn incomplete(&self) -> bool {
        !self.update_errors.is_empty() || self.news_failed
    }

    pub fn sorted_updates(&self) -> Vec<&Update> {
        let mut updates: Vec<&Update> = self.updates.iter().collect();
        updates.sort_by(|a, b| a.risk_rank().cmp(&b.risk_rank()).then_with(|| a.name.cmp(&b.name)));
        updates
    }

    pub fn sorted_problems(&self) -> Vec<&Problem> {
        let mut problems: Vec<&Problem> = self.problems.iter().collect();
        problems.sort_by(|a, b| a.rank().cmp(&b.rank()).then_with(|| b.count.cmp(&a.count)));
        problems
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
    pub fn subsystem_updates(&self, subsystem: Subsystem) -> Vec<&Update> {
        self.sorted_updates().into_iter().filter(|update| Subsystem::of(&update.name) == subsystem).collect()
    }

    pub fn subsystem_problems(&self, subsystem: Subsystem) -> Vec<&Problem> {
        self.sorted_problems()
            .into_iter()
            .filter(|problem| {
                let name = if problem.package.is_empty() { problem.name() } else { &problem.package };
                Subsystem::of(name) == subsystem
            })
            .collect()
    }

    /// Subsystems that have something to show, in display order.
    pub fn active_subsystems(&self) -> Vec<Subsystem> {
        let active: Vec<Subsystem> = Subsystem::ALL
            .into_iter()
            .filter(|s| !self.subsystem_updates(*s).is_empty() || !self.subsystem_problems(*s).is_empty())
            .collect();
        if active.is_empty() { Subsystem::ALL[..4].to_vec() } else { active }
    }

    /// The subsystem in the spotlight; it moves on every few seconds once the
    /// scan has finished, like an instrument stepping through its channels.
    pub fn spotlight(&self, now: Instant) -> Subsystem {
        let active = self.active_subsystems();
        let Some(done) = self.finished_at else { return active[0] };
        let step = (now.saturating_duration_since(done).as_millis() / 5000) as usize;
        active[step % active.len()]
    }

    pub fn spotlight_since(&self, now: Instant) -> Option<Instant> {
        let done = self.finished_at?;
        let step = now.saturating_duration_since(done).as_millis() / 5000;
        Some(done + Duration::from_millis((step * 5000) as u64))
    }
}

pub fn plural<'a>(count: usize, one: &'a str, many: &'a str) -> &'a str {
    if count == 1 { one } else { many }
}

fn stage_index(stage: &str) -> Option<usize> {
    STAGES.iter().position(|name| *name == stage)
}

fn field<T: DeserializeOwned + Default>(data: &Value, key: &str) -> T {
    data.get(key)
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}
