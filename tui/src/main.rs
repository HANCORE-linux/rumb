mod app;
mod ink;
mod palette;
mod pictures;
mod source;
mod theme;
mod ui;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};

use app::App;
use source::{Msg, Scan};
use theme::Theme;

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::Relaxed);
}

fn catch_signals() {
    let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
    for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
        unsafe { libc::signal(signal, handler) };
    }
}

fn watch_terminal() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(200));
        let mut input = libc::pollfd { fd: 0, events: 0, revents: 0 };
        let ready = unsafe { libc::poll(&mut input, 1, 0) };
        if ready > 0 && input.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            source::terminate_scan();
            std::process::exit(129);
        }
    });
}

const USAGE: &str = "rumb — Track software health. Spot trouble early.

Usage: rumb [--demo] [--no-anim] [--theme <name>] [--since <time>] [--no-aur]

  --demo          Play bundled demo data instead of checking this system
  --no-anim       Show the results without animation
  --theme <name>  Colors to use; by default the current Omarchy theme, live,
                  and labra without Omarchy
  --themes        List the theme names
  --version       Print the version
  --since <time>  Passed on to rumb-scan
  --no-aur        Passed on to rumb-scan

Keys: 1-5 switch views (overview, updates, problems, packages, live),
tab / shift-tab move between tiles, enter opens details, esc goes back,
r rescans, q quits.

The dashboard needs a terminal of at least 150x44; a smaller one shows the
size it needs. Without a terminal (or with NO_COLOR set) rumb prints the plain
text report instead.";

struct Options {
    demo: bool,
    no_anim: bool,
    theme: Option<String>,
    args: Vec<String>,
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options { demo: false, no_anim: false, theme: None, args: Vec::new() };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = true,
            "--no-anim" => options.no_anim = true,
            "--no-aur" => options.args.push(arg),
            "--theme" => options.theme = Some(args.next().ok_or("--theme needs a name")?),
            "--themes" => {
                println!("omarchy\n{}", theme::names().join("\n"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("rumb {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--since" => {
                let value = args.next().ok_or("--since needs a value")?;
                options.args.push(arg);
                options.args.push(value);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("Unknown option: {other}")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            return ExitCode::from(1);
        }
    };

    let script = source::find_script();
    if !options.demo && script.is_none() {
        eprintln!("rumb-scan not found; set RUMB_SCAN to its path");
        return ExitCode::from(1);
    }

    let requested = options.theme.clone().or_else(|| std::env::var("RUMB_THEME").ok()).or_else(|| std::env::var("RUMB_PALETTE").ok());
    let mut theme = match theme::pick(requested.as_deref()) {
        Ok(theme) => theme,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(1);
        }
    };

    let interactive = io::stdout().is_terminal() && io::stdin().is_terminal();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());

    if !interactive || no_color {
        if options.demo {
            eprintln!("--demo needs an interactive terminal without NO_COLOR");
            return ExitCode::from(1);
        }
        let script = script.expect("checked above");
        return match Command::new(&script).args(&options.args).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8),
            Err(error) => {
                eprintln!("Could not run {}: {error}", script.display());
                ExitCode::from(1)
            }
        };
    }

    let mut terminal = ratatui::init();
    catch_signals();
    watch_terminal();
    let mut pictures = pictures::Pictures::query();
    let result = run(&mut terminal, &options, script, &mut theme, &mut pictures);
    ratatui::restore();

    let signal = SIGNAL.load(Ordering::Relaxed);
    if signal != 0 {
        return ExitCode::from((128 + signal).clamp(0, 255) as u8);
    }
    match result {
        Ok(true) => ExitCode::from(1),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn start_source(options: &Options, script: Option<&PathBuf>) -> io::Result<(Option<Scan>, Receiver<Msg>)> {
    if options.demo {
        return Ok((None, source::spawn_demo()));
    }
    let script = script.expect("checked in main");
    let (scan, rx) = source::spawn_script(script, &options.args)?;
    Ok((Some(scan), rx))
}

fn run(terminal: &mut DefaultTerminal, options: &Options, script: Option<PathBuf>, theme: &mut Theme, pictures: &mut Option<pictures::Pictures>) -> io::Result<bool> {
    let (mut child, mut rx) = start_source(options, script.as_ref())?;
    let mut app = App::new(options.demo, !options.no_anim, Instant::now());
    let mut source_open = true;

    loop {
        if SIGNAL.load(Ordering::Relaxed) != 0 {
            break;
        }
        let now = Instant::now();
        while source_open {
            match rx.try_recv() {
                Ok(Msg::Event(event)) => app.apply(event, now),
                Ok(Msg::Invalid) => {}
                Ok(Msg::Eof { stderr }) => {
                    source_open = false;
                    let status = child.as_mut().and_then(Scan::exited);
                    if child.is_some() && app.finished_at.is_some() {
                        app.live_lost = true;
                    }
                    if app.finished_at.is_none() {
                        let detail = stderr
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .map(str::to_string)
                            .or_else(|| status.map(|status| format!("exited with {status}")))
                            .unwrap_or_else(|| "the scan stopped unexpectedly".into());
                        app.fail(detail, now);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    source_open = false;
                    app.fail("the scan stopped unexpectedly".into(), now);
                }
            }
        }

        theme.refresh(now);
        terminal.draw(|frame| {
            let model = ink::Model::new(&app);
            let mut scene = ui::draw(frame, &app, &model, &theme.palette, now);
            if let Some(pictures) = pictures.as_mut() {
                let anim = ui::astrolabe_anim(&app, &model, now);
                let astrolabe = format!("{anim:?}{:?}{}", scene.callouts, model.nodes.len());
                let (model, anim, callouts, pal) = (Arc::new(model), Arc::new(anim), Arc::new(std::mem::take(&mut scene.callouts)), theme.palette);
                pictures.draw(frame, scene, (theme.generation, &astrolabe), now, |picture| {
                    let (model, anim, callouts) = (model.clone(), anim.clone(), callouts.clone());
                    Box::new(move |width, height, cell_height| ui::paint(&picture, &callouts, &model, &anim, &pal, width, height, cell_height))
                });
            }
        })?;

        let timeout = if app.animating(now) || ui::moving(&app, now) {
            Duration::from_millis(33)
        } else if app.finished_at.is_none() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };

        if !event::poll(timeout)? {
            continue;
        }
        match event::read()? {
            TermEvent::Key(key) if key.kind == KeyEventKind::Press => {
                let ctrl_c = key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    _ if ctrl_c => break,
                    KeyCode::Char('q') => break,
                    KeyCode::Esc if app.detail.is_some() => app.detail = None,
                    KeyCode::Esc => break,
                    KeyCode::Tab | KeyCode::BackTab if app.detail.is_none() => {
                        let count = ui::panels_of(app.view).len().max(1);
                        let step = if key.code == KeyCode::Tab { 1 } else { count - 1 };
                        app.set_focus((app.focus + step) % count, Instant::now());
                    }
                    KeyCode::Char(digit @ '1'..='5') => app.set_view(digit as usize - '1' as usize, Instant::now()),
                    KeyCode::Enter if app.detail.is_none() => {
                        app.detail = Some(app.focus);
                        app.selected = 0;
                    }
                    KeyCode::Down if app.detail.is_some() => app.selected = (app.selected + 1).min(app.updates.len().saturating_sub(1)),
                    KeyCode::Up if app.detail.is_some() => app.selected = app.selected.saturating_sub(1),
                    KeyCode::Char('r') if app.finished_at.is_some() => {
                        drop(child.take());
                        (child, rx) = start_source(options, script.as_ref())?;
                        let (view, focus, detail) = (app.view, app.focus, app.detail);
                        app = App::new(options.demo, !options.no_anim, Instant::now());
                        (app.view, app.focus, app.detail) = (view, focus, detail);
                        source_open = true;
                    }
                    _ => app.skip(),
                }
            }
            TermEvent::Resize(..) => app.skip(),
            _ => {}
        }
    }

    drop(child);
    Ok(app.error.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier};
    use serde_json::{Value, json};

    fn demo_events() -> Vec<(u64, source::Event)> {
        let mut at = 0;
        let recorded = std::env::var_os("RUMB_EVENTS").map(|path| std::fs::read_to_string(path).expect("readable RUMB_EVENTS file"));
        recorded
            .as_deref()
            .unwrap_or(include_str!("../demo.ndjson"))
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                at += value["_delay"].as_u64().unwrap_or(0);
                (at, serde_json::from_str(line).unwrap())
            })
            .collect()
    }

    fn hex(color: Color) -> Value {
        match color {
            Color::Rgb(r, g, b) => json!(format!("#{r:02x}{g:02x}{b:02x}")),
            _ => Value::Null,
        }
    }

    /// Renders the demo at `at_ms` and writes the cell grid as JSON plus every
    /// image as PNG, for the mockup renderer.
    fn dump(dir: &std::path::Path, name: &str, width: u16, height: u16, at_ms: u64, animate: bool, setup: impl Fn(&mut App)) {
        let requested = std::env::var("RUMB_THEME").or_else(|_| std::env::var("RUMB_PALETTE")).unwrap_or_else(|_| "paper".into());
        let pal = theme::pick(Some(&requested)).expect("known theme").palette;
        let started = Instant::now();
        let mut app = App::new(std::env::var_os("RUMB_EVENTS").is_none(), animate, started);
        for (time, event) in demo_events() {
            if time <= at_ms {
                app.apply(event, started + Duration::from_millis(time));
            }
        }
        setup(&mut app);
        let now = started + Duration::from_millis(at_ms);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let model = ink::Model::new(&app);
        let mut scene = None;
        terminal.draw(|frame| scene = Some(ui::draw(frame, &app, &model, &pal, now))).unwrap();
        let scene = scene.unwrap();

        let buffer = terminal.backend().buffer();
        let mut rows = Vec::new();
        for y in 0..height {
            let mut runs: Vec<Value> = Vec::new();
            let mut current: Option<(String, Value, Value, bool)> = None;
            for x in 0..width {
                let cell = &buffer[(x, y)];
                let key = (hex(cell.fg), hex(cell.bg), cell.modifier.contains(Modifier::BOLD));
                match &mut current {
                    Some((text, fg, bg, bold)) if (fg.clone(), bg.clone(), *bold) == key => text.push_str(cell.symbol()),
                    _ => {
                        if let Some((text, fg, bg, bold)) = current.take() {
                            runs.push(json!([text, fg, bg, bold]));
                        }
                        current = Some((cell.symbol().to_string(), key.0, key.1, key.2));
                    }
                }
            }
            if let Some((text, fg, bg, bold)) = current {
                runs.push(json!([text, fg, bg, bold]));
            }
            rows.push(Value::Array(runs));
        }

        let (cell_w, cell_h) = (10u32, 21u32);
        let anim = ui::astrolabe_anim(&app, &model, now);
        let mut images = Vec::new();
        for (index, placement) in scene.placements.iter().enumerate() {
            let (w, h) = (u32::from(placement.rect.width) * cell_w, u32::from(placement.rect.height) * cell_h);
            let pixmap = ui::paint(&placement.picture, &scene.callouts, &model, &anim, &pal, w, h, cell_h as f32).or_else(|| tiny_skia::Pixmap::new(w, h));
            let file = format!("{name}-image-{index}.png");
            pixmap.unwrap().save_png(dir.join(&file)).unwrap();
            images.push(json!({"x": placement.rect.x, "y": placement.rect.y, "png": file}));
        }

        let frame = json!({
            "width": width,
            "height": height,
            "cell": [cell_w, cell_h],
            "bg": hex(pal.page),
            "fg": hex(pal.ink),
            "rows": rows,
            "images": images,
        });
        std::fs::write(dir.join(format!("{name}.json")), frame.to_string()).unwrap();
    }

    #[test]
    fn small_terminal_shows_needed_size() {
        let pal = palette::Palette::paper();
        let now = Instant::now();
        let app = App::new(true, false, now);
        let model = ink::Model::new(&app);
        let render = |width: u16, height: u16| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut scene = None;
            terminal.draw(|frame| scene = Some(ui::draw(frame, &app, &model, &pal, now))).unwrap();
            let buffer = terminal.backend().buffer().clone();
            (buffer, scene.unwrap())
        };
        for (width, height) in [(100, 30), (149, 44), (150, 43)] {
            let (buffer, scene) = render(width, height);
            let rows: Vec<String> = (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect();
            let current = format!("Width = {width} Height = {height}");
            let (y, x) = rows.iter().enumerate().find_map(|(y, row)| row.find(&current).map(|x| (y as u16, x as u16))).expect("current size shown");
            assert!(rows.iter().any(|row| row.contains("Terminal size too small:")), "{width}x{height}");
            assert!(rows.iter().any(|row| row.contains("Width = 150 Height = 44")), "{width}x{height}");
            assert!(scene.placements.is_empty(), "{width}x{height}");
            let fg = |offset: u16| buffer[(x + offset, y)].fg;
            assert_eq!(fg(8), if width < ui::MIN_WIDTH { pal.alert } else { pal.ink }, "{width}x{height}");
            assert_eq!(fg(21), if height < ui::MIN_HEIGHT { pal.alert } else { pal.ink }, "{width}x{height}");
        }
        render(20, 1);
        render(1, 1);
        let (buffer, _) = render(150, 44);
        assert!(!buffer.content().iter().map(|cell| cell.symbol()).collect::<String>().contains("Terminal size too small:"));
    }

    fn crowded_app(now: Instant) -> App {
        let mut app = App::new(true, false, now);
        for (_, event) in demo_events() {
            app.apply(event, now);
        }
        let long = |i: usize| format!("pkg-{i:03}-with-a-rather-long-name");
        let updates: Vec<Value> = (0..500)
            .map(|i| {
                let (source, risk) = (if i % 7 == 0 { "aur" } else { "repo" }, ["high", "medium", "low"][i % 3]);
                json!({"name": long(i), "old": "1.0-1", "new": "2.0-1", "source": source, "bump": "major", "risk": risk,
                    "reason": "core package, major version jump", "download_bytes": 1000 * i, "installed_delta_bytes": 10 * i})
            })
            .collect();
        let mut problems: Vec<Value> = (0..60)
            .map(|i| {
                json!({"kind": "failed-unit", "subject": format!("unit-{i}.service"), "count": 1, "package": long(0),
                    "status": "possibly-fixed", "update": format!("{} 1.0-1 -> 2.0-1", long(0))})
            })
            .collect();
        problems.extend((0..60).map(|i| json!({"kind": "journal", "subject": format!("/usr/bin/tool-{i}"), "count": i + 1, "package": format!("tool-{i}"), "status": "no-update", "update": ""})));
        let news: Vec<Value> = (0..40)
            .map(|i| json!({"date": "2026-10-01", "title": format!("News item {i} with a rather long title that needs truncation"), "link": "https://archlinux.org/news/x/", "manual_intervention": i % 5 == 0}))
            .collect();
        app.apply(source::Event::Stage { stage: "updates".into(), data: json!({"updates": updates, "update_check_errors": []}) }, now);
        app.apply(source::Event::Stage { stage: "problems".into(), data: json!({"problems": problems, "activity": []}) }, now);
        app.apply(source::Event::Stage { stage: "news".into(), data: json!({"news": news, "news_check_failed": false}) }, now);
        app.apply(source::Event::Activity { line: "transaction started".into() }, now);
        for i in 450..500 {
            app.apply(source::Event::Activity { line: format!("upgraded {} (1.0-1 -> 2.0-1)", long(i)) }, now);
        }
        for i in 0..300 {
            app.apply(source::Event::Journal { time: "12:00:00".into(), ident: format!("ident-{i}"), message: "x".repeat(300) }, now);
        }
        app
    }

    #[test]
    fn crowded_data_never_panics() {
        let pal = palette::Palette::paper();
        let mut failures = Vec::new();
        for (width, height) in [(150, 44), (200, 60)] {
            for view in 0..ui::VIEWS.len() {
                for detail in std::iter::once(None).chain((0..ui::panels_of(view).len()).map(Some)) {
                    for selected in [0, 449] {
                        let now = Instant::now();
                        let mut app = crowded_app(now);
                        app.view = view;
                        app.detail = detail;
                        app.selected = selected;
                        let model = ink::Model::new(&app);
                        let later = now + Duration::from_secs(60);
                        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                            terminal.draw(|frame| drop(ui::draw(frame, &app, &model, &pal, later))).unwrap();
                        }));
                        if outcome.is_err() {
                            failures.push(format!("{width}x{height} view {view} detail {detail:?} selected {selected}"));
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "panicked: {failures:#?}");
    }

    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let pal = palette::Palette::paper();
        let model = ink::Model::new(app);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| drop(ui::draw(frame, app, &model, &pal, app.started + Duration::from_secs(60)))).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
    }

    #[test]
    fn full_list_scrolls_to_the_selection_without_overlap() {
        let mut app = crowded_app(Instant::now());
        app.detail = Some(0);
        for selected in [0, 300, 449] {
            app.selected = selected;
            let name = app.sorted_updates()[selected].name[..8].to_string();
            let rows = screen(&app, 150, 44);
            let chart = rows.iter().position(|row| row.contains("DOWNLOAD BY PACKAGE")).expect("chart title");
            let listed: Vec<usize> = (0..chart).filter(|&y| rows[y].contains("pkg-")).collect();
            assert!(listed.iter().any(|&y| rows[y].contains(&name)), "selection {selected} not listed");
            assert!(!rows[chart - 1].contains("pkg-"), "chart touches the table");
            assert!(rows.iter().any(|row| row.contains(" of 450 ")), "no position shown");
        }
    }

    #[test]
    fn only_real_upgrades_count_as_applied() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        let update = |name: &str| json!({"name": name, "old": "1.0-1", "new": "2.0-1", "source": "repo", "risk": "low"});
        app.apply(source::Event::Stage { stage: "updates".into(), data: json!({"updates": [update("kept"), update("newer"), update("gone"), update("back")]}) }, now);
        for line in ["reinstalled kept (1.0-1)", "upgraded newer (1.0-1 -> 2.1-1)", "removed gone (1.0-1)", "downgraded back (1.0-1 -> 0.9-1)"] {
            app.apply(source::Event::Activity { line: line.into() }, now);
        }
        let pending: Vec<&str> = app.updates.iter().map(|u| u.name.as_str()).collect();
        let applied: Vec<(&str, &str)> = app.applied.iter().map(|(u, _)| (u.name.as_str(), u.new.as_str())).collect();
        assert_eq!(pending, ["kept", "back"]);
        assert_eq!(applied, [("newer", "2.1-1")]);
    }

    #[test]
    fn unreadable_stage_data_fails_the_scan_with_its_reason() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Stage { stage: "updates".into(), data: json!({"updates": [{"name": "broken"}]}) }, now);
        assert_eq!(app.verdict().0, "SCAN FAILED");
        let error = app.error.clone().unwrap_or_default();
        assert!(error.contains("updates"), "{error}");
        app.error = Some("Missing required command: checkupdates".into());
        assert!(screen(&app, 170, 50).iter().any(|row| row.contains("Missing required command: checkupdates")));
    }

    #[test]
    fn a_later_start_moves_the_window_but_not_the_intro() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        let start = |since: &str| source::Event::Start { last_upgrade: Some(since.into()), problems_since: since.into(), now: None };
        app.apply(start("2026-10-01T10:00:00+02:00"), now);
        let first = app.start_at;
        app.apply(start("2026-10-19T12:00:00+02:00"), now + Duration::from_secs(90));
        assert_eq!(app.start_at, first);
        assert_eq!(app.since.as_deref(), Some("2026-10-19T12:00:00+02:00"));
        assert_eq!(app.last_upgrade.as_deref(), Some("2026-10-19T12:00:00+02:00"));
    }

    fn on_omarchy(app: &mut App, mark: app::OmarchyMark) {
        if let Some(anatomy) = app.anatomy.as_mut() {
            anatomy.omarchy = "4.0.4".into();
        }
        app.omarchy_mark = mark;
    }

    #[test]
    fn mockup_frames() {
        let Some(dir) = std::env::var_os("RUMB_DUMP") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for view in 0..5 {
            dump(&dir, &format!("view-{}", view + 1), 170, 50, 8_500, false, |app| app.view = view);
        }
        dump(&dir, "live-working", 170, 50, 20_900, true, |app| app.view = 4);
        dump(&dir, "overview-working", 170, 50, 20_900, true, |_| {});
        dump(&dir, "omarchy-wordmark", 170, 50, 8_500, false, |app| on_omarchy(app, app::OmarchyMark::Wordmark));
        if std::env::var_os("RUMB_DUMP_VIDEO").is_some() {
            let script: [(u64, usize, usize); 8] = [(0, 0, 0), (8_000, 1, 0), (11_000, 2, 0), (14_000, 3, 0), (17_000, 4, 0), (26_000, 0, 0), (29_000, 0, 1), (30_500, 0, 2)];
            for frame in 0..(24 * 32) {
                let at = frame * 1000 / 24;
                dump(&dir, &format!("video-{frame:04}"), 170, 50, at, true, |app| {
                    let step = script.iter().rposition(|(from, _, _)| *from <= at).unwrap_or(0);
                    let (from, view, focus) = script[step];
                    app.view = view;
                    app.focus = focus;
                    app.focus_prev = if step > 0 && script[step - 1].1 == view { script[step - 1].2 } else { 0 };
                    if from > 0 {
                        app.focus_since = Some(app.started + Duration::from_millis(from));
                    }
                });
            }
        }
    }
}
