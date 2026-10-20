mod app;
mod export;
mod ink;
mod motion;
mod page;
mod palette;
mod pictures;
mod sixel;
mod source;
mod theme;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};

use app::{App, Pane};
use motion::Motion;
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

Keys: 1-5 show or hide the instruments, subsystems, updates, problems and
live events; tab or left/right moves the focus between the lists; up/down,
PgUp/PgDn and Home/End select; enter opens the selection; / filters the
focused list; j shows the journal of the selected problem; e exports a
Markdown report to ~/rumb-exports; esc closes or clears; r rescans; q quits.

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

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Nothing,
    Quit,
    Rescan,
    Export,
}

fn on_key(app: &mut App, key: KeyEvent, page: [usize; 3]) -> Action {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }
    let focus = app.focus;
    let list = focus.index();
    if app.editing {
        match key.code {
            KeyCode::Esc => {
                app.lists[list].filter.clear();
                app.editing = false;
            }
            KeyCode::Enter => app.editing = false,
            KeyCode::Backspace => {
                app.lists[list].filter.pop();
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => app.lists[list].filter.push(ch),
            _ => return Action::Nothing,
        }
        app.select(focus, 0);
        return Action::Nothing;
    }
    let selected = app.lists[list].selected as isize;
    let step = page[list].max(1) as isize;
    let shown = app.visible[focus.panel()];
    match key.code {
        KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('e') => return Action::Export,
        KeyCode::Char('r') if app.finished_at.is_some() => return Action::Rescan,
        KeyCode::Esc if app.overlay => app.overlay = false,
        KeyCode::Esc if !app.lists[list].filter.is_empty() => {
            app.lists[list].filter.clear();
            app.select(focus, 0);
        }
        KeyCode::Tab | KeyCode::Right => app.cycle_focus(true),
        KeyCode::BackTab | KeyCode::Left => app.cycle_focus(false),
        KeyCode::Char(digit @ '1'..='5') => {
            app.toggle(digit as usize - '1' as usize);
            app.overlay = false;
        }
        KeyCode::Down => app.select(focus, selected + 1),
        KeyCode::Up => app.select(focus, selected - 1),
        KeyCode::PageDown => app.select(focus, selected + step),
        KeyCode::PageUp => app.select(focus, selected - step),
        KeyCode::Home => app.select(focus, 0),
        KeyCode::End => app.select(focus, isize::MAX),
        KeyCode::Enter if shown && app.rows(focus) > 0 => app.overlay = true,
        KeyCode::Char('/' | 'f') if shown => {
            app.overlay = false;
            app.editing = true;
        }
        KeyCode::Char('j') if shown && focus == Pane::Problems => {
            if let Some(name) = app.visible_problems().get(app.lists[list].selected).map(|problem| problem.name().to_string()) {
                app.lists[Pane::Events.index()].filter = name;
                app.visible[Pane::Events.panel()] = true;
                app.focus = Pane::Events;
                app.overlay = false;
                app.select(Pane::Events, 0);
            }
        }
        _ => {}
    }
    Action::Nothing
}

fn tilde(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => format!("~{}", &text[home.len()..]),
        _ => text,
    }
}

fn run(terminal: &mut DefaultTerminal, options: &Options, script: Option<PathBuf>, theme: &mut Theme, pictures: &mut Option<pictures::Pictures>) -> io::Result<bool> {
    let (mut child, mut rx) = start_source(options, script.as_ref())?;
    let mut app = App::new(options.demo, !options.no_anim, Instant::now());
    let mut motion = Motion::new(!options.no_anim, Instant::now());
    let mut source_open = true;
    let mut rows = [10; 3];
    let mut last_frame = Instant::now();

    loop {
        if SIGNAL.load(Ordering::Relaxed) != 0 {
            break;
        }
        let now = Instant::now();
        while source_open {
            match rx.try_recv() {
                Ok(Msg::Event(event, raw)) => motion.push(event, &raw, &mut app, now),
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
        motion.step(&mut app, now, now.saturating_duration_since(last_frame).as_secs_f32());
        last_frame = now;
        let cell = pictures.as_ref().map_or((10, 21), pictures::Pictures::cell);
        execute!(io::stdout(), BeginSynchronizedUpdate)?;
        let drawn = terminal.draw(|frame| {
            let scene = page::draw(frame, &app, &motion, &theme.palette, now, cell);
            rows = scene.rows;
            if let Some(pictures) = pictures.as_mut() {
                let pal = theme.palette;
                pictures.draw(frame, scene, &pal, theme.generation, now, |picture| Box::new(move |width, height, cell_height| page::paint(&picture, &pal, width, height, cell_height)));
            }
        });
        execute!(io::stdout(), EndSynchronizedUpdate)?;
        drawn?;

        let timeout = if page::moving(&app, &motion, now) {
            Duration::from_millis(33)
        } else if app.finished_at.is_none() {
            Duration::from_millis(100)
        } else {
            motion.wake(now).map_or(Duration::from_millis(500), |wake| wake.clamp(Duration::from_millis(10), Duration::from_millis(500)))
        };

        if !event::poll(timeout)? {
            continue;
        }
        let before = (app.focus, app.lists[app.focus.index()].selected);
        match event::read()? {
            TermEvent::Key(key) if key.kind == KeyEventKind::Press => match on_key(&mut app, key, rows) {
                Action::Nothing => {
                    if (app.focus, app.lists[app.focus.index()].selected) != before {
                        motion.select(&app, Instant::now());
                    }
                }
                Action::Quit => break,
                Action::Rescan => {
                    drop(child.take());
                    (child, rx) = start_source(options, script.as_ref())?;
                    motion = Motion::new(!options.no_anim, Instant::now());
                    let mut fresh = App::new(options.demo, !options.no_anim, Instant::now());
                    fresh.visible = app.visible;
                    fresh.focus = app.focus;
                    for (list, old) in fresh.lists.iter_mut().zip(&app.lists) {
                        list.filter.clone_from(&old.filter);
                    }
                    app = fresh;
                    source_open = true;
                }
                Action::Export => {
                    let result = export::dir().ok_or_else(|| io::Error::other("HOME is not set")).and_then(|dir| export::write(&app, &dir, &app::local_stamp()));
                    app.status = Some(match result {
                        Ok(path) => (format!("exported {}", tilde(&path)), true, Instant::now()),
                        Err(error) => (format!("export failed: {error}"), false, Instant::now()),
                    });
                }
            },
            TermEvent::Resize(..) => {}
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
        write_frame(dir, name, (width, height), &app, &Motion::new(false, started), &pal, now, (10, 21));
    }

    #[allow(clippy::too_many_arguments)]
    fn write_frame(dir: &std::path::Path, name: &str, size: (u16, u16), app: &App, motion: &Motion, pal: &palette::Palette, now: Instant, cell: (u16, u16)) {
        let (width, height) = size;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut scene = None;
        terminal.draw(|frame| scene = Some(page::draw(frame, app, motion, pal, now, cell))).unwrap();
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

        let (cell_w, cell_h) = (u32::from(cell.0), u32::from(cell.1));
        let mut images = Vec::new();
        for (index, placement) in scene.placements.iter().enumerate() {
            let (w, h) = (u32::from(placement.rect.width) * cell_w, u32::from(placement.rect.height) * cell_h);
            let pixmap = page::paint(&placement.picture, pal, w, h, cell_h as f32).or_else(|| tiny_skia::Pixmap::new(w, h));
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

    fn render(app: &App, width: u16, height: u16, now: Instant) -> (ratatui::buffer::Buffer, page::Scene) {
        let pal = palette::Palette::paper();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut scene = None;
        let motion = Motion::new(false, now);
        terminal.draw(|frame| scene = Some(page::draw(frame, app, &motion, &pal, now, (10, 21)))).unwrap();
        (terminal.backend().buffer().clone(), scene.unwrap())
    }

    fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
        let (buffer, _) = render(app, width, height, app.started + Duration::from_secs(60));
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(app: &mut App, codes: &[KeyCode]) -> Action {
        let mut last = Action::Nothing;
        for code in codes {
            last = on_key(app, key(*code), [10; 3]);
        }
        last
    }

    fn typed(text: &str) -> Vec<KeyCode> {
        text.chars().map(KeyCode::Char).collect()
    }

    #[test]
    fn small_terminal_shows_needed_size() {
        let pal = palette::Palette::paper();
        let now = Instant::now();
        let app = App::new(true, false, now);
        for (width, height) in [(100, 30), (149, 44), (150, 43)] {
            let (buffer, scene) = render(&app, width, height, now);
            let rows: Vec<String> = (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect();
            let current = format!("Width = {width} Height = {height}");
            let (y, x) = rows.iter().enumerate().find_map(|(y, row)| row.find(&current).map(|x| (y as u16, x as u16))).expect("current size shown");
            assert!(rows.iter().any(|row| row.contains("Terminal size too small:")), "{width}x{height}");
            assert!(rows.iter().any(|row| row.contains("Width = 150 Height = 44")), "{width}x{height}");
            assert!(scene.placements.is_empty(), "{width}x{height}");
            let fg = |offset: u16| buffer[(x + offset, y)].fg;
            assert_eq!(fg(8), if width < page::MIN_WIDTH { pal.alert } else { pal.ink }, "{width}x{height}");
            assert_eq!(fg(21), if height < page::MIN_HEIGHT { pal.alert } else { pal.ink }, "{width}x{height}");
        }
        render(&app, 20, 1, now);
        render(&app, 1, 1, now);
        let (buffer, scene) = render(&app, 150, 44, now);
        assert!(!buffer.content().iter().map(|cell| cell.symbol()).collect::<String>().contains("Terminal size too small:"));
        assert_eq!(scene.placements.len(), 8);
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
        let now = Instant::now();
        let mut app = crowded_app(now);
        let later = now + Duration::from_secs(60);
        let mut failures = Vec::new();
        for (width, height) in [(150, 44), (200, 60)] {
            for mask in 0..32u8 {
                for focus in Pane::ALL {
                    for (overlay, editing) in [(false, false), (true, false), (false, true)] {
                        for selected in [0, 10_000] {
                            for filter in ["", "pkg-4", "zzz"] {
                                app.visible = std::array::from_fn(|panel| mask & (1 << panel) != 0);
                                app.focus = focus;
                                app.overlay = overlay;
                                app.editing = editing;
                                for list in &mut app.lists {
                                    list.filter = filter.into();
                                    list.selected = selected;
                                }
                                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(render(&app, width, height, later))));
                                if outcome.is_err() {
                                    failures.push(format!("{width}x{height} mask {mask:05b} {focus:?} overlay {overlay} editing {editing} selected {selected} filter {filter:?}"));
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(failures.is_empty(), "panicked: {failures:#?}");
    }

    #[test]
    fn list_scrolls_to_the_selection_and_shows_its_position() {
        let mut app = crowded_app(Instant::now());
        for selected in [0usize, 300, 449] {
            app.select(Pane::Updates, selected as isize);
            let name = app.visible_updates()[selected].name[..7].to_string();
            let rows = screen(&app, 150, 44);
            assert!(rows.iter().any(|row| row.contains(&name)), "selection {selected} not listed");
            assert!(rows.iter().any(|row| row.contains(&format!("{} of 450", selected + 1))), "no position for {selected}");
        }
    }

    #[test]
    fn keys_move_filter_open_and_never_quit_on_escape() {
        let mut app = crowded_app(Instant::now());
        assert_eq!(press(&mut app, &[KeyCode::Esc, KeyCode::Esc]), Action::Nothing);
        press(&mut app, &[KeyCode::Down, KeyCode::Down, KeyCode::PageDown]);
        assert_eq!(app.lists[0].selected, 12);
        press(&mut app, &[KeyCode::End]);
        assert_eq!(app.lists[0].selected, 449);
        press(&mut app, &[KeyCode::Down, KeyCode::Home, KeyCode::Up]);
        assert_eq!(app.lists[0].selected, 0);

        press(&mut app, &[KeyCode::Char('/')]);
        assert!(app.editing);
        press(&mut app, &typed("pkg-04"));
        assert_eq!(press(&mut app, &[KeyCode::Char('q'), KeyCode::Backspace, KeyCode::Enter]), Action::Nothing);
        assert!(!app.editing);
        assert_eq!(app.lists[0].filter, "pkg-04");
        assert_eq!(app.rows(Pane::Updates), 10);
        assert!(screen(&app, 150, 44).iter().any(|row| row.contains("UPDATES · 10")));

        press(&mut app, &[KeyCode::Enter]);
        assert!(app.overlay);
        press(&mut app, &[KeyCode::Esc]);
        assert!(!app.overlay);
        assert_eq!(app.lists[0].filter, "pkg-04");
        press(&mut app, &[KeyCode::Esc]);
        assert!(app.lists[0].filter.is_empty());
        assert_eq!(press(&mut app, &[KeyCode::Esc]), Action::Nothing);

        press(&mut app, &[KeyCode::Char('3')]);
        assert!(!app.visible[2]);
        assert_eq!(app.focus, Pane::Problems);
        press(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, Pane::Events);
        press(&mut app, &[KeyCode::Right]);
        assert_eq!(app.focus, Pane::Problems);

        let name = app.visible_problems()[0].name().to_string();
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.focus, Pane::Events);
        assert_eq!(app.lists[2].filter, name);

        assert_eq!(press(&mut app, &[KeyCode::Char('e')]), Action::Export);
        assert_eq!(press(&mut app, &[KeyCode::Char('r')]), Action::Rescan);
        assert_eq!(press(&mut app, &[KeyCode::Char('q')]), Action::Quit);
        assert_eq!(on_key(&mut app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), [10; 3]), Action::Quit);
    }

    #[test]
    fn paused_events_keep_the_selected_line_when_new_ones_arrive() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        app.focus = Pane::Events;
        press(&mut app, &[KeyCode::Down, KeyCode::Down]);
        assert!(!app.follow);
        let chosen = app.visible_events()[2].ident.clone();
        app.apply(source::Event::Journal { time: "12:00:01".into(), ident: "late".into(), message: "boom".into() }, now);
        assert_eq!(app.visible_events()[app.lists[2].selected].ident, chosen);
        app.lists[2].filter = "ident-1".into();
        app.select(Pane::Events, 1);
        let chosen = app.visible_events()[1].ident.clone();
        app.apply(source::Event::Journal { time: "12:00:02".into(), ident: "other".into(), message: "boom".into() }, now);
        assert_eq!(app.visible_events()[app.lists[2].selected].ident, chosen);
        press(&mut app, &[KeyCode::Home]);
        assert!(app.follow);
    }

    #[test]
    fn live_journal_lines_count_until_the_next_problem_stage() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Stage { stage: "problems".into(), data: json!({"problems": [], "activity": []}) }, now);
        app.apply(source::Event::Result {}, now);
        for _ in 0..3 {
            app.apply(source::Event::Journal { time: "12:00:00".into(), ident: "x".into(), message: "y".into() }, now);
        }
        assert_eq!(app.journal_live, 3);
        app.apply(source::Event::Stage { stage: "problems".into(), data: json!({"problems": [], "activity": []}) }, now);
        assert_eq!(app.journal_live, 0);
    }

    #[test]
    fn export_writes_a_private_markdown_report_without_control_characters() {
        use std::os::unix::fs::PermissionsExt;
        let now = Instant::now();
        let mut app = crowded_app(now);
        app.apply(source::Event::Journal { time: "12:00:03".into(), ident: "evil|ident".into(), message: "red \u{1b}[31mtext\u{7}".into() }, now);
        let dir = std::env::temp_dir().join(format!("rumb-export-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = export::write(&app, &dir, "20261019-120000").unwrap();
        let second = export::write(&app, &dir, "20261019-120000").unwrap();
        assert_eq!(first.file_name().unwrap(), "rumb-20261019-120000.md");
        assert_eq!(second.file_name().unwrap(), "rumb-20261019-120000-1.md");
        assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o077, 0);
        let text = std::fs::read_to_string(&first).unwrap();
        for part in ["# rumb report", "## Updates (450)", "## Problems (120)", "## Disk and packages", "## Live events", "pkg-000-with-a-rather-long-name", "freeable with paccache -rk1"] {
            assert!(text.contains(part), "missing {part}");
        }
        assert!(!text.chars().any(|ch| ch.is_control() && ch != '\n'), "control character in the report");
        assert!(text.contains("red  [31mtext "));
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(app.events.iter().filter(|event| event.source == app::Source::Pacman).count(), 4);
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

    #[test]
    fn mockup_frames() {
        let Some(dir) = std::env::var_os("RUMB_DUMP") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        dump(&dir, "page", 170, 50, 8_500, false, |_| {});
        dump(&dir, "page-after-upgrade", 170, 50, 30_000, false, |_| {});
        dump(&dir, "page-small", 150, 44, 8_500, false, |_| {});
        dump(&dir, "page-working", 170, 50, 3_000, true, |_| {});
        dump(&dir, "page-overlay", 170, 50, 8_500, false, |app| {
            app.focus = Pane::Problems;
            app.overlay = true;
        });
        dump(&dir, "page-filter", 170, 50, 8_500, false, |app| {
            app.focus = Pane::Events;
            app.editing = true;
            app.lists[2].filter = "kernel".into();
        });
        dump(&dir, "page-lists", 170, 50, 8_500, false, |app| app.visible = [false, false, true, true, true]);
    }

    #[test]
    fn motion_frames() {
        let Some(dir) = std::env::var_os("RUMB_VIDEO") else { return };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fps = 30u64;
        let number = |name: &str, default: u64| std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default);
        let size = (number("RUMB_COLUMNS", 170) as u16, number("RUMB_ROWS", 50) as u16);
        let cell = (number("RUMB_CELL_WIDTH", 10) as u16, number("RUMB_CELL_HEIGHT", 21) as u16);
        let pal = theme::pick(Some(&std::env::var("RUMB_THEME").unwrap_or_else(|_| "labra".into()))).expect("known theme").palette;
        let recorded = std::env::var_os("RUMB_EVENTS").map(|path| std::fs::read_to_string(path).expect("readable RUMB_EVENTS file"));
        let mut at = 0u64;
        let stream: Vec<(u64, String)> = recorded
            .as_deref()
            .unwrap_or(include_str!("../demo.ndjson"))
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let value: Value = serde_json::from_str(line).unwrap();
                at += value["_delay"].as_u64().unwrap_or(0);
                (at, line.to_string())
            })
            .collect();
        let selections: Vec<(u64, usize)> = std::env::var("RUMB_SELECT").ok().map(|text| text.split(',').filter_map(|pair| pair.split_once(':')).filter_map(|(time, row)| Some((time.parse().ok()?, row.parse().ok()?))).collect()).unwrap_or_default();
        let only: Option<Vec<u64>> = std::env::var("RUMB_ONLY").ok().map(|text| text.split(',').filter_map(|frame| frame.trim().parse().ok()).collect());
        let started = Instant::now();
        let mut app = App::new(false, true, started);
        let mut motion = Motion::new(true, started);
        let (mut next, mut chosen, mut last) = (0usize, 0usize, started);
        let mut costs: Vec<f64> = Vec::new();
        for frame in 0..=number("RUMB_SECONDS", 40) * fps {
            let millis = frame * 1000 / fps;
            let now = started + Duration::from_millis(millis);
            while next < stream.len() && stream[next].0 <= millis {
                let raw = &stream[next].1;
                motion.push(serde_json::from_str(raw).unwrap(), raw, &mut app, now);
                next += 1;
            }
            while chosen < selections.len() && selections[chosen].0 <= millis {
                app.focus = Pane::Updates;
                app.select(Pane::Updates, selections[chosen].1 as isize);
                motion.select(&app, now);
                chosen += 1;
            }
            motion.step(&mut app, now, now.saturating_duration_since(last).as_secs_f32());
            last = now;
            if only.as_ref().is_none_or(|only| only.contains(&frame)) {
                let begun = Instant::now();
                write_frame(&dir, &format!("video-{frame:04}"), size, &app, &motion, &pal, now, cell);
                costs.push(begun.elapsed().as_secs_f64() * 1000.0);
            } else {
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                terminal.draw(|f| drop(page::draw(f, &app, &motion, &pal, now, cell))).unwrap();
            }
        }
        costs.sort_by(f64::total_cmp);
        println!("frames written {} median {:.1} ms", costs.len(), costs.get(costs.len() / 2).copied().unwrap_or(0.0));
    }
}
