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

use std::io::{self, IsTerminal, Write};
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
  --theme <name>  Colors to use; by default the theme chosen with esc, else
                  the current Omarchy theme, live, and labra without Omarchy
  --themes        List the theme names: the official Omarchy themes and the
                  HANCORE themes
  --version       Print the version
  --since <time>  Passed on to rumb-scan
  --no-aur        Passed on to rumb-scan

Keys: 1-5 show or hide the instruments, subsystems, updates, problems and
live events; tab and shift+tab move the focus between the instruments, the
subsystems and the lists; left/right selects an instrument or a part; up/down,
PgUp/PgDn and Home/End select a row; enter opens the selection; / filters the
focused list; j shows the journal of the selected problem; c copies the text
of the open window; e exports a Markdown report to ~/rumb-exports; esc closes
or clears, and with nothing open shows the options in the eye, where left/right
change the theme that rumb remembers and typing filters the themes; r rescans;
q quits.

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
                println!("{}", theme::choices().join("\n"));
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

    let default_hook = std::panic::take_hook();
    let mut terminal = ratatui::init();
    std::panic::set_hook(Box::new(move |info| {
        let _ = ratatui::try_restore();
        default_hook(info);
    }));
    catch_signals();
    watch_terminal();
    let mut pictures = pictures::Pictures::query();
    let result = run(&mut terminal, &options, script, &mut theme, &mut pictures);
    let _ = ratatui::try_restore();

    let signal = SIGNAL.load(Ordering::Relaxed);
    if signal != 0 {
        return ExitCode::from((128 + signal).clamp(0, 255) as u8);
    }
    match result {
        Ok(true) => ExitCode::from(1),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr(), "{error}");
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
    Copy,
    Theme(bool),
    ThemeFilter,
}

fn on_key(app: &mut App, key: KeyEvent, page: [usize; 3], now: Instant) -> Action {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }
    if app.options {
        match key.code {
            KeyCode::Left => return Action::Theme(false),
            KeyCode::Right => return Action::Theme(true),
            KeyCode::Esc if !app.theme_filter.is_empty() => app.theme_filter.clear(),
            KeyCode::Esc => app.toggle_options(now),
            KeyCode::Backspace => {
                app.theme_filter.pop();
                return Action::ThemeFilter;
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.theme_filter.push(ch);
                return Action::ThemeFilter;
            }
            _ => {}
        }
        return Action::Nothing;
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
    if app.tile.is_some() {
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('e') => return Action::Export,
            KeyCode::Char('c') if app.overlay => return Action::Copy,
            KeyCode::Char('r') if app.finished_at.is_some() => return Action::Rescan,
            KeyCode::Esc if app.overlay => app.overlay = false,
            KeyCode::Esc if app.eye_shown() => app.toggle_options(now),
            KeyCode::Left => app.move_tile(false),
            KeyCode::Right => app.move_tile(true),
            KeyCode::Tab => app.cycle_focus(true),
            KeyCode::BackTab => app.cycle_focus(false),
            KeyCode::Enter => app.overlay = true,
            KeyCode::Char(digit @ '1'..='5') => {
                app.press_panel(digit as usize - '1' as usize);
                app.overlay = false;
            }
            _ => {}
        }
        return Action::Nothing;
    }
    if app.part.is_some() {
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('e') => return Action::Export,
            KeyCode::Char('c') if app.overlay => return Action::Copy,
            KeyCode::Char('r') if app.finished_at.is_some() => return Action::Rescan,
            KeyCode::Esc if app.overlay => app.overlay = false,
            KeyCode::Esc if app.eye_shown() => app.toggle_options(now),
            KeyCode::Left => app.move_part(false),
            KeyCode::Right => app.move_part(true),
            KeyCode::Tab => app.cycle_focus(true),
            KeyCode::BackTab => app.cycle_focus(false),
            KeyCode::Enter => app.overlay = true,
            KeyCode::Char(digit @ '1'..='5') => {
                app.press_panel(digit as usize - '1' as usize);
                app.overlay = false;
            }
            _ => {}
        }
        return Action::Nothing;
    }
    let selected = app.lists[list].selected as isize;
    let step = page[list].max(1) as isize;
    let shown = app.visible[focus.panel()];
    match key.code {
        KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('e') => return Action::Export,
        KeyCode::Char('c') if app.overlay => return Action::Copy,
        KeyCode::Char('r') if app.finished_at.is_some() => return Action::Rescan,
        KeyCode::Esc if app.overlay => app.overlay = false,
        KeyCode::Esc if !app.lists[list].filter.is_empty() => {
            app.lists[list].filter.clear();
            app.select(focus, 0);
        }
        KeyCode::Esc if app.eye_shown() => app.toggle_options(now),
        KeyCode::Tab => app.cycle_focus(true),
        KeyCode::BackTab => app.cycle_focus(false),
        KeyCode::Char(digit @ '1'..='5') => {
            app.press_panel(digit as usize - '1' as usize);
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

fn step_theme(theme: &mut Theme, forward: bool, filter: &str) {
    let choices = theme::matching(filter);
    let count = choices.len();
    let index = choices.iter().position(|name| *name == theme.name);
    for step in 1..=count {
        let next = match index {
            Some(index) => (index + if forward { step } else { count - step }) % count,
            None if forward => step - 1,
            None => count - step,
        };
        if Some(next) != index && theme.switch(&choices[next]) {
            return;
        }
    }
}

fn filter_theme(theme: &mut Theme, filter: &str) {
    if theme::matches(&theme.name, filter) {
        return;
    }
    for name in theme::matching(filter) {
        if theme.switch(&name) {
            return;
        }
    }
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
    app.pictures = pictures.is_some();
    app.theme.clone_from(&theme.name);
    let mut motion = Motion::new(!options.no_anim, Instant::now());
    let mut source_open = true;
    let mut rows = [10; 3];
    let mut map_fits = false;
    let mut last_frame = Instant::now();
    let (copy_done, copies) = std::sync::mpsc::channel();
    let mut copying = 0;

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
        while let Ok((lines, result)) = copies.try_recv() {
            copying -= 1;
            app.status = Some(match result {
                Ok(()) => (format!("copied {} {}", page::thousands(lines as u64), if lines == 1 { "line" } else { "lines" }), true, Instant::now()),
                Err(error) => (format!("copy failed: {error}"), false, Instant::now()),
            });
        }

        theme.refresh(now);
        motion.step(&mut app, now, now.saturating_duration_since(last_frame).as_secs_f32());
        last_frame = now;
        let size = terminal.size()?;
        app.fit_page(page::fits(&app, size.width, size.height));
        let cell = pictures.as_ref().map_or((10, 21), pictures::Pictures::cell);
        execute!(io::stdout(), BeginSynchronizedUpdate)?;
        let drawn = terminal.draw(|frame| {
            let scene = page::draw(frame, &app, &motion, &theme.palette, now, cell);
            rows = scene.rows;
            map_fits = scene.map_fits;
            if let Some(pictures) = pictures.as_mut() {
                let pal = theme.palette;
                pictures.draw(frame, scene, &pal, theme.generation, now, |picture| Box::new(move |width, height, cell_height| page::paint(&picture, &pal, width, height, cell_height)));
            }
        });
        execute!(io::stdout(), EndSynchronizedUpdate)?;
        drawn?;
        app.map_fits = map_fits;

        let timeout = if page::moving(&app, &motion, now) {
            pictures::FRAME
        } else if app.finished_at.is_none() || copying > 0 {
            Duration::from_millis(100)
        } else {
            motion.wake(now).map_or(Duration::from_millis(500), |wake| wake.clamp(Duration::from_millis(10), Duration::from_millis(500)))
        };

        if !event::poll(timeout)? {
            continue;
        }
        let before = (app.focus, app.lists[app.focus.index()].selected, app.tile, app.part);
        match event::read()? {
            TermEvent::Key(key) if key.kind == KeyEventKind::Press => match on_key(&mut app, key, rows, Instant::now()) {
                Action::Nothing => {
                    if (app.focus, app.lists[app.focus.index()].selected, app.tile, app.part) != before {
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
                    fresh.pictures = app.pictures;
                    fresh.theme.clone_from(&theme.name);
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
                action @ (Action::Theme(_) | Action::ThemeFilter) => {
                    match action {
                        Action::Theme(forward) => step_theme(theme, forward, &app.theme_filter),
                        _ => filter_theme(theme, &app.theme_filter),
                    }
                    app.theme.clone_from(&theme.name);
                    if let Err(error) = theme::save(&theme.name) {
                        app.status = Some((format!("theme not saved: {error}"), false, Instant::now()));
                    }
                }
                Action::Copy => {
                    if let Some(text) = page::overlay_text(&app, &theme.palette) {
                        let (lines, done) = (text.lines().count(), copy_done.clone());
                        std::thread::spawn(move || done.send((lines, export::copy(text))));
                        copying += 1;
                    }
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
        for (index, placement) in scene.placements.iter().filter(|_| app.pictures).enumerate() {
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
            last = on_key(app, key(*code), [10; 3], Instant::now());
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
            assert_eq!(fg(8), if width < 150 { pal.alert } else { pal.ink }, "{width}x{height}");
            assert_eq!(fg(21), if height < 44 { pal.alert } else { pal.ink }, "{width}x{height}");
        }
        render(&app, 20, 1, now);
        render(&app, 1, 1, now);
        let (buffer, scene) = render(&app, 150, 44, now);
        assert!(!buffer.content().iter().map(|cell| cell.symbol()).collect::<String>().contains("Terminal size too small:"));
        assert_eq!(scene.placements.len(), 8);
    }

    #[test]
    fn hidden_panels_lower_the_needed_size() {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        assert_eq!(page::min_size(&app), (150, 44));
        app.visible = [false, true, false, false, false];
        assert_eq!(page::min_size(&app), (150, 17));
        let (buffer, scene) = render(&app, 150, 17, now);
        assert!(!buffer.content().iter().map(|cell| cell.symbol()).collect::<String>().contains("Terminal size too small:"));
        assert_eq!(scene.placements.len(), 8);
        app.visible = [true, false, false, false, false];
        assert_eq!(page::min_size(&app), (150, 16));
        app.visible = [false, false, true, true, false];
        assert_eq!(page::min_size(&app), (100, 13));
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
                    "reason": "critical package, major version jump", "download_bytes": 1000 * i, "installed_delta_bytes": 10 * i})
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
            app.apply(source::Event::Journal { time: "12:00:00".into(), day: String::new(), ident: format!("ident-{i}"), program: String::new(), message: "x".repeat(300), priority: String::new(), unit: String::new() }, now);
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
        for list in &mut app.lists {
            list.filter.clear();
            list.selected = 0;
        }
        app.editing = false;
        for (width, height) in [(150, 44), (200, 60), (150, 17)] {
            for mask in 0..32u8 {
                app.visible = std::array::from_fn(|panel| mask & (1 << panel) != 0);
                for overlay in [false, true] {
                    app.overlay = overlay;
                    for stop in 0..13usize {
                        (app.tile, app.part) = if stop < 6 { (Some(stop), None) } else { (None, Some(stop - 6)) };
                        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(render(&app, width, height, later))));
                        if outcome.is_err() {
                            failures.push(format!("{width}x{height} mask {mask:05b} overlay {overlay} tile {:?} part {:?}", app.tile, app.part));
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
        assert_eq!((app.part, app.tile, app.focus), (None, None, Pane::Updates));

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
        assert!(app.options);
        assert_eq!(press(&mut app, &[KeyCode::Esc]), Action::Nothing);
        assert!(!app.options);

        press(&mut app, &[KeyCode::Char('3')]);
        assert!(!app.visible[2]);
        assert_eq!(app.focus, Pane::Problems);
        press(&mut app, &[KeyCode::Tab]);
        assert_eq!(app.focus, Pane::Events);
        press(&mut app, &[KeyCode::BackTab]);
        assert_eq!(app.focus, Pane::Problems);

        let name = app.visible_problems()[0].name().to_string();
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.focus, Pane::Events);
        assert_eq!(app.lists[2].filter, name);

        assert_eq!(press(&mut app, &[KeyCode::Char('e')]), Action::Export);
        assert_eq!(press(&mut app, &[KeyCode::Char('r')]), Action::Rescan);
        assert_eq!(press(&mut app, &[KeyCode::Char('q')]), Action::Quit);
        assert_eq!(on_key(&mut app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), [10; 3], Instant::now()), Action::Quit);
    }

    #[test]
    fn escape_opens_the_options_in_the_eye() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let keys = |app: &App| screen(app, 170, 50).last().unwrap().trim().to_string();
        assert!(keys(&app).ends_with("q quit   Esc options"), "{}", keys(&app));
        press(&mut app, &[KeyCode::Esc]);
        assert!(app.options);
        assert_eq!(keys(&app), "type to filter   ←→ change   Esc close");
        assert_eq!(press(&mut app, &[KeyCode::Char('3'), KeyCode::Down, KeyCode::Char('q')]), Action::ThemeFilter);
        assert!(app.visible[2] && app.lists[0].selected == 0);
        assert_eq!((app.theme_filter.as_str(), keys(&app).as_str()), ("3q", "type to filter   ←→ change   Esc clear"));
        assert_eq!(press(&mut app, &[KeyCode::Backspace]), Action::ThemeFilter);
        assert_eq!(app.theme_filter, "3");
        assert_eq!(press(&mut app, &[KeyCode::Left]), Action::Theme(false));
        assert_eq!(press(&mut app, &[KeyCode::Right]), Action::Theme(true));
        press(&mut app, &[KeyCode::Esc]);
        assert!(app.options && app.theme_filter.is_empty());
        press(&mut app, &[KeyCode::Esc]);
        assert!(!app.options);

        app.lists[0].filter = "pkg".into();
        assert!(!keys(&app).contains("Esc options"));
        press(&mut app, &[KeyCode::Esc]);
        assert!(!app.options && app.lists[0].filter.is_empty());
        press(&mut app, &[KeyCode::Enter, KeyCode::Esc]);
        assert!(!app.options && !app.overlay);
        press(&mut app, &[KeyCode::BackTab, KeyCode::BackTab, KeyCode::Esc]);
        assert!(app.tile.is_some() && app.options);
        press(&mut app, &[KeyCode::Esc]);

        for hide in [|app: &mut App| app.pictures = false, |app: &mut App| app.visible[1] = false] {
            let mut app = crowded_app(now);
            hide(&mut app);
            assert!(!keys(&app).contains("Esc options"));
            press(&mut app, &[KeyCode::Esc]);
            assert!(!app.options);
        }
    }

    #[test]
    fn typed_letters_pick_and_step_through_the_matching_themes() {
        assert_eq!(theme::matching("BLACK"), ["blackgold", "blackmoney", "blackturq", "matte-black", "vantablack"]);
        let mut theme = theme::pick(Some("labra")).unwrap();
        filter_theme(&mut theme, "black");
        assert_eq!(theme.name, "blackgold");
        step_theme(&mut theme, true, "black");
        assert_eq!(theme.name, "blackmoney");
        step_theme(&mut theme, false, "black");
        step_theme(&mut theme, false, "black");
        assert_eq!(theme.name, "vantablack");
        filter_theme(&mut theme, "anta");
        assert_eq!(theme.name, "vantablack");
        filter_theme(&mut theme, "xyz");
        step_theme(&mut theme, true, "xyz");
        assert_eq!(theme.name, "vantablack");
    }

    #[test]
    fn a_window_too_small_for_the_eye_keeps_the_options_shut() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        assert!(!page::fits(&app, 80, 24) && page::fits(&app, 170, 50));
        app.fit_page(page::fits(&app, 80, 24));
        assert!(screen(&app, 80, 24).iter().any(|row| row.contains("Terminal size too small:")));
        assert_eq!(press(&mut app, &[KeyCode::Esc]), Action::Nothing);
        assert!(!app.options);
        assert_eq!(press(&mut app, &[KeyCode::Char('q')]), Action::Quit);

        app.fit_page(page::fits(&app, 170, 50));
        press(&mut app, &[KeyCode::Esc]);
        assert!(app.options);
        app.fit_page(page::fits(&app, 80, 24));
        assert!(!app.options && app.dilation(now) == 0.0);
        assert_eq!(press(&mut app, &[KeyCode::Char('q')]), Action::Quit);
        app.fit_page(page::fits(&app, 170, 50));
        assert!(!app.options && screen(&app, 170, 50).last().unwrap().trim().ends_with("q quit   Esc options"));
    }

    #[test]
    fn the_pupil_opens_and_closes_in_one_calm_motion() {
        let now = Instant::now();
        let mut app = App::new(true, true, now);
        assert_eq!(app.dilation(now), 0.0);
        app.toggle_options(now);
        let at = |ms: u64| app.dilation(now + Duration::from_millis(ms));
        let opening: Vec<f32> = (0..=12).map(|step| at(step * 30)).collect();
        assert!(opening.windows(2).all(|pair| pair[1] >= pair[0]) && opening[0] == 0.0 && at(360) == 1.0, "{opening:?}");
        assert!(app.dilating(now + Duration::from_millis(300)) && !app.dilating(now + Duration::from_millis(400)));
        let half = at(180);
        app.toggle_options(now + Duration::from_millis(180));
        assert!((app.dilation(now + Duration::from_millis(180)) - half).abs() < 1e-6);
        assert_eq!(app.dilation(now + Duration::from_millis(540)), 0.0);
        let mut still = App::new(true, false, now);
        still.toggle_options(now);
        assert_eq!(still.dilation(now), 1.0);
    }

    #[test]
    fn update_details_put_facts_left_and_dependencies_right() {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (_, event) in demo_events() {
            app.apply(event, now);
        }
        app.focus = Pane::Updates;
        app.overlay = true;
        let (buffer, scene) = render(&app, 170, 50, now);
        assert!(scene.placements.iter().any(|placement| matches!(placement.picture, page::Picture::Fan(_))));
        let rows: Vec<String> = (0..50).map(|y| (0..170).map(|x| buffer[(x, y)].symbol()).collect()).collect();
        let x = rows.iter().find_map(|row| row.find("version ").map(|at| row[..at].chars().count() as u16)).expect("facts shown");
        assert!((0..50).filter(|y| buffer[(x + 47, *y)].symbol() == "│").count() >= 3);
    }

    #[test]
    fn a_selection_is_a_glance_not_a_stare() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let mut motion = Motion::new(true, now);
        app.focus = Pane::Problems;
        motion.select(&app, now);
        motion.step(&mut app, now + Duration::from_millis(200), 0.2);
        assert_eq!(motion.look(now + Duration::from_millis(400)), Some(motion::Target::List(1)));
        assert_eq!(motion.look(now + Duration::from_secs(3)), None);
    }

    #[test]
    fn choosing_a_part_turns_the_eye_to_it() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let mut motion = Motion::new(true, now);
        app.part = Some(1);
        motion.select(&app, now);
        assert_eq!(motion.look(now + Duration::from_millis(300)), Some(motion::Target::Part(1)));
        assert_eq!(motion.look(now + Duration::from_secs(3)), None);
        app.part = None;
        motion.select(&app, now + Duration::from_millis(400));
        assert_eq!(motion.look(now + Duration::from_millis(600)), Some(motion::Target::List(0)));
    }

    #[test]
    fn the_iris_shows_only_risk_colours() {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events().into_iter().filter(|(time, _)| *time <= 8_500) {
            app.apply(event, now + Duration::from_millis(time));
        }
        assert!(app.problems.iter().any(|problem| problem.status == "possibly-fixed" && app.updates.iter().any(|update| update.name == problem.package)));
        let pal = palette::Palette::paper();
        let (_, scene) = render(&app, 170, 50, now + Duration::from_secs(60));
        let eye = scene.placements.iter().find(|placement| matches!(placement.picture, page::Picture::Eye(_))).expect("the eye is drawn");
        let pixmap = page::paint(&eye.picture, &pal, u32::from(eye.rect.width) * 10, u32::from(eye.rect.height) * 21, 21.0).expect("the eye paints");
        let Color::Rgb(r, g, b) = pal.teal else { panic!("the accent is not rgb") };
        let accent = pixmap.pixels().iter().map(|pixel| pixel.demultiply()).filter(|pixel| pixel.red().abs_diff(r) <= 30 && pixel.green().abs_diff(g) <= 30 && pixel.blue().abs_diff(b) <= 30).count();
        assert_eq!(accent, 0);
    }

    #[test]
    fn the_header_names_the_running_version() {
        let now = Instant::now();
        let version = env!("CARGO_PKG_VERSION");
        let empty = App::new(true, false, now);
        let header = screen(&empty, 170, 50)[0].clone();
        assert!(header.contains(&format!("rumb {version}")) && !header.contains(&format!("rumb {version} ·")), "{header}");
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events() {
            app.apply(event, now + Duration::from_millis(time));
        }
        let header = screen(&app, 170, 50)[0].clone();
        assert!(header.contains(&format!("rumb {version} · machine")), "{header}");
    }

    #[test]
    fn a_quiet_feed_keeps_the_eye_alive() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let mut motion = Motion::new(true, now);
        let start = source::Event::Start { last_upgrade: None, problems_since: "2026-10-21T20:00:00+02:00".into(), now: None, omarchy: false, last_upgraded: Vec::new() };
        motion.push(start, r#"{"event":"start","problems_since":"2026-10-21T20:00:00+02:00"}"#, &mut app, now);
        let mut looked = Vec::new();
        for frame in 1..=30 * 24 {
            let at = now + Duration::from_millis(frame * 1000 / 30);
            motion.step(&mut app, at, 1.0 / 30.0);
            if let Some(target) = motion.look(at)
                && looked.last() != Some(&target)
            {
                looked.push(target);
            }
        }
        let rain = motion.rain(now + Duration::from_secs(24)).expect("the white of the eye keeps its code");
        assert!(rain.level > 0.1);
        assert_eq!(rain.pupil, 1.0);
        assert!(looked.len() >= 3, "the eye looked at {looked:?} in a quiet feed");
    }

    #[test]
    fn three_switches_the_updates_between_map_rows_and_hidden() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let (_, scene) = render(&app, 358, 85, now);
        assert!(scene.map_fits);
        assert!(scene.placements.iter().any(|placement| matches!(placement.picture, page::Picture::Orbit(_))));
        app.map_fits = scene.map_fits;
        press(&mut app, &[KeyCode::Char('3')]);
        assert!(app.visible[2] && !app.updates_map);
        let (_, scene) = render(&app, 358, 85, now);
        assert!(!scene.placements.iter().any(|placement| matches!(placement.picture, page::Picture::Orbit(_))));
        press(&mut app, &[KeyCode::Char('3')]);
        assert!(!app.visible[2]);
        press(&mut app, &[KeyCode::Char('3')]);
        assert!(app.visible[2] && app.updates_map);
        let (_, scene) = render(&app, 170, 50, now);
        assert!(!scene.map_fits);
        app.map_fits = scene.map_fits;
        press(&mut app, &[KeyCode::Char('3')]);
        assert!(!app.visible[2]);
    }

    #[test]
    fn keys_reach_the_instrument_tiles() {
        let mut app = crowded_app(Instant::now());
        press(&mut app, &[KeyCode::BackTab, KeyCode::BackTab]);
        assert_eq!(app.tile, Some(0));
        press(&mut app, &[KeyCode::Right, KeyCode::Right]);
        assert_eq!(app.tile, Some(2));
        press(&mut app, &[KeyCode::Left, KeyCode::Left, KeyCode::Left]);
        assert_eq!(app.tile, Some(5));
        press(&mut app, &[KeyCode::Enter]);
        assert!(app.overlay);
        assert!(screen(&app, 150, 44).iter().any(|row| row.contains(":: BEFORE YOU UPDATE")));
        press(&mut app, &[KeyCode::Esc]);
        assert!(!app.overlay);
        assert_eq!(app.tile, Some(5));
        press(&mut app, &[KeyCode::Tab]);
        assert_eq!((app.tile, app.part), (None, Some(0)));
        press(&mut app, &[KeyCode::Tab]);
        assert_eq!((app.tile, app.part, app.focus), (None, None, Pane::Updates));
        press(&mut app, &[KeyCode::BackTab, KeyCode::BackTab]);
        assert_eq!(app.tile, Some(5));
        press(&mut app, &[KeyCode::Char('1')]);
        assert_eq!(app.tile, None);
        press(&mut app, &[KeyCode::BackTab]);
        assert_eq!((app.tile, app.part), (None, Some(0)));
        press(&mut app, &[KeyCode::BackTab]);
        assert_eq!((app.tile, app.part, app.focus), (None, None, Pane::Events));
    }

    #[test]
    fn arrow_keys_never_leave_the_focused_panel() {
        let mut app = crowded_app(Instant::now());
        for _ in 0..5 {
            let stop = (app.tile.is_some(), app.part.is_some(), app.focus);
            for code in [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right, KeyCode::PageDown, KeyCode::End, KeyCode::PageUp, KeyCode::Home, KeyCode::Up, KeyCode::Esc] {
                press(&mut app, &[code]);
                assert_eq!((app.tile.is_some(), app.part.is_some(), app.focus), stop, "{code:?} left {stop:?}");
            }
            press(&mut app, &[KeyCode::Tab]);
        }
    }

    fn done(label: &str, value: Value) -> source::Event {
        source::Event::Done { label: label.into(), value }
    }

    fn stage(name: &str, data: Value) -> source::Event {
        source::Event::Stage { stage: name.into(), data }
    }

    fn find(rows: &[String], text: &str) -> Option<(u16, u16)> {
        rows.iter().enumerate().find_map(|(y, row)| row.find(text).map(|at| (row[..at].chars().count() as u16, y as u16)))
    }

    fn system_app(now: Instant) -> App {
        let gib = 1024 * 1024 * 1024u64;
        let mut app = App::new(false, false, now);
        app.apply(done("reading the running kernel", json!({"running": "7.2.5-3-omarchy", "installed": "7.2.5-3", "package": "linux-omarchy", "uptime_seconds": 2_188_986, "reboot_pending": false})), now);
        app.apply(done("reading the loaded nvidia module", json!({"loaded": "610.57.04", "installed": "610.57.04-1"})), now);
        app.apply(done("reading the running Hyprland", json!({"running": "0.56.2", "installed": "0.56.2-2"})), now);
        let updates = json!([
            {"name": "linux-omarchy", "old": "7.2.5-3", "new": "7.2.8-9", "source": "repo", "bump": "patch", "risk": "medium", "reason": "critical package", "download_bytes": 6 * gib, "installed_delta_bytes": gib / 2},
            {"name": "nvidia-utils", "old": "610.57.04-1", "new": "615.78.08-1", "source": "repo", "bump": "major", "risk": "high", "reason": "critical package, major version jump", "download_bytes": gib, "installed_delta_bytes": 0},
            {"name": "netwatch-tui", "old": "0.4.0-1", "new": "0.5.0-1", "source": "aur", "bump": "minor", "risk": "low"}
        ]);
        app.apply(stage("updates", json!({"updates": updates})), now);
        app.apply(stage("problems", json!({"problems": [
            {"kind": "journal", "subject": "/usr/bin/conmon", "count": 1915, "package": "conmon", "status": "no-update"},
            {"kind": "journal", "subject": "hancore", "count": 3, "package": "", "status": "unknown-owner"}
        ], "activity": [], "config_errors": 0})), now);
        app.apply(stage("anatomy", json!({"packages": 1342, "explicit": 300, "orphans": 12, "cache_bytes": 1000, "kernel_running": "7.2.5-3-omarchy", "kernel_installed": "7.2.5-3", "kernel_package": "linux-omarchy", "hostname": "box"})), now);
        app.apply(stage("preflight", json!({"free_bytes": 167 * gib, "free_ok": true, "snapper_configured": true, "snapshot_before_update": true, "snapshot_by": "omarchy-update", "disk_total_bytes": 465 * gib, "disk_used_bytes": 289 * gib})), now);
        app.apply(source::Event::Result {}, now);
        app
    }

    #[test]
    fn the_band_never_drops_a_red_or_yellow_fact() {
        let now = Instant::now();
        let pal = palette::Palette::paper();
        for (width, height) in [(150, 44), (170, 50), (358, 85)] {
            let mut app = App::new(false, false, now);
            app.apply(done("reading the running kernel", json!({"running": "7.2.5-3-omarchy", "installed": "7.2.8-9", "package": "linux-omarchy", "uptime_seconds": 90_000, "reboot_pending": true})), now);
            app.apply(done("reading the loaded nvidia module", json!({"loaded": "610.57.04", "installed": "615.78.08-1"})), now);
            app.apply(done("reading the running Hyprland", json!({"running": "0.56.2", "installed": "0.56.3-1"})), now);
            let (buffer, _) = render(&app, width, height, now);
            let rows: Vec<String> = (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect();
            for word in ["· reboot", "· relogin"] {
                let (x, y) = find(&rows, word).unwrap_or_else(|| panic!("{word} missing at {width}x{height}"));
                assert_eq!(buffer[(x + 2, y)].fg, pal.alert, "{word} at {width}x{height}");
            }
            assert_eq!(rows.iter().filter(|row| row.contains("· reboot")).count(), 2, "{width}x{height}");

            let mut app = App::new(false, false, now);
            app.apply(done("reading the loaded nvidia module", json!({"loaded": "610.57.04", "installed": "610.57.04-1"})), now);
            app.apply(done("reading the running Hyprland", json!({"running": "0.56.2", "installed": "0.56.2-2"})), now);
            app.apply(stage("updates", json!({"updates": [{"name": "nvidia-utils", "old": "610.57.04-1", "new": "615.78.08-1", "source": "repo", "bump": "major", "risk": "high"}]})), now);
            app.apply(stage("problems", json!({"problems": [], "activity": [], "config_errors": 2})), now);
            let rows = screen(&app, width, height);
            assert!(find(&rows, "· 615.78 pending").is_some(), "nvidia at {width}x{height}");
            assert!(find(&rows, "· 2 config errors").is_some(), "config at {width}x{height}");
        }
    }

    #[test]
    fn before_you_update_says_what_really_happens() {
        let now = Instant::now();
        let mut app = system_app(now);
        app.tile = Some(5);
        app.overlay = true;
        let rows = screen(&app, 358, 85);
        for line in ["omarchy-update takes one first · pacman -Syu takes none", "after this update · kernel 7.2.5-3 → 7.2.8-9 · nvidia 610.57 → 615.78", "167 GiB available · ≈ 7.5 GiB needed", "nvidia-utils  610.57.04 → 615.78.08 · critical package, major version jump"] {
            assert!(find(&rows, line).is_some(), "missing {line:?}");
        }
        app.overlay = false;
        let value = |rows: &[String], name: &str| {
            let row = rows.iter().find(|row| row.contains(&format!("● {name}"))).unwrap_or_else(|| panic!("no {name} row"));
            row.split(&format!("● {name}")).nth(1).and_then(|tail| tail.split('│').next()).unwrap_or_default().trim().to_string()
        };
        let wide = screen(&app, 358, 85);
        assert_eq!([value(&wide, "risk"), value(&wide, "reboot"), value(&wide, "snapshot"), value(&wide, "space")], ["high", "after update", "omarchy-update", "ok"]);
        let narrow = screen(&app, 150, 44);
        assert_eq!([value(&narrow, "reboot"), value(&narrow, "snapshot")], ["after update", "omarchy"]);
        assert!(find(&wide, "³UPDATES · 3 · 7.0 GiB download + 1 AUR").is_some());
    }

    #[test]
    fn space_needed_is_download_plus_growth() {
        let now = Instant::now();
        let mut app = system_app(now);
        assert_eq!(app.space_needed(), 7 * 1024 * 1024 * 1024 + 512 * 1024 * 1024);
        assert_eq!(app.space_severity(), Some(app::Severity::Ok));
        if let Some(preflight) = app.preflight.as_mut() {
            preflight.free_bytes = 7 * 1024 * 1024 * 1024;
        }
        assert_eq!(app.space_severity(), Some(app::Severity::Bad));
        for update in &mut app.updates {
            update.installed_delta_bytes = Some(-(1 << 40));
        }
        assert_eq!(app.space_needed(), 7 * 1024 * 1024 * 1024);
    }

    #[test]
    fn windows_open_over_what_they_explain() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        app.tile = Some(0);
        app.overlay = true;
        let rows = screen(&app, 358, 85);
        assert!(rows[2].contains(":: PENDING"), "{}", rows[2]);
        app.tile = None;
        app.focus = Pane::Problems;
        app.select(Pane::Problems, 3);
        app.overlay = false;
        let (_, scene) = render(&app, 358, 85, app.started + Duration::from_secs(60));
        let row = scene.chosen[1].expect("selected problem drawn");
        app.overlay = true;
        let name = app.visible_problems()[3].name().to_string();
        let rows = screen(&app, 358, 85);
        let (_, y) = find(&rows, &format!(":: {name}")).expect("problem window");
        assert_eq!(y, row.y - 1);
    }

    #[test]
    fn events_from_earlier_days_show_their_date() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Journal { time: "13:45:10".into(), day: "2000-01-02".into(), ident: "earlier".into(), program: String::new(), message: "old".into(), priority: String::new(), unit: String::new() }, now);
        app.apply(source::Event::Journal { time: "14:00:00".into(), day: app::today(), ident: "today".into(), program: String::new(), message: "new".into(), priority: String::new(), unit: String::new() }, now);
        let rows = screen(&app, 150, 44);
        assert!(find(&rows, "01-02 13:45").is_some());
        assert!(find(&rows, "14:00:00").is_some());
        assert!(!rows.iter().any(|row| row.contains("following")), "the feed is not live yet");
        app.apply(source::Event::Result {}, now);
        assert!(screen(&app, 150, 44).iter().any(|row| row.contains("LIVE EVENTS · following")));
    }

    #[test]
    fn the_map_shows_the_position_too() {
        let app = crowded_app(Instant::now());
        assert!(screen(&app, 358, 85).iter().any(|row| row.contains("1 of 450")));
    }

    #[test]
    fn the_map_head_line_calls_a_rebuild_a_rebuild() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        app.apply(stage("updates", json!({"updates": [{"name": "mesa", "old": "1:25.2.4-1", "new": "1:25.2.4-2", "source": "repo", "bump": "rebuild", "risk": "low"}]})), now);
        let rows = screen(&app, 358, 85);
        assert!(rows.iter().any(|row| row.contains("▸ mesa  1:25.2.4 rebuild · low")));
        assert!(!rows.iter().any(|row| row.contains("1:25.2.4 → 1:25.2.4")));
    }

    #[test]
    fn problems_without_an_owner_belong_to_other() {
        use app::{Subsystem, problem_part};
        assert_eq!(problem_part("journal", "hancore", ""), Subsystem::Apps);
        assert_eq!(problem_part("config", "Hyprland config errors", ""), Subsystem::Compositor);
        assert_eq!(problem_part("journal", "kernel", ""), Subsystem::Kernel);
        assert_eq!(problem_part("crash", "/usr/bin/Hyprland", "hyprland"), Subsystem::Compositor);
        assert_eq!(Subsystem::Apps.label(), "other");
        let rows = screen(&system_app(Instant::now()), 358, 85);
        assert!(find(&rows, " 2  other").is_some() || find(&rows, " 2 other").is_some(), "badge counts the unowned problem");
    }

    #[test]
    fn the_eye_takes_the_room_of_hidden_lists() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        let eye = |app: &App, width: u16, height: u16| render(app, width, height, now).1.placements.iter().find_map(|placement| matches!(placement.picture, page::Picture::Eye(_)).then_some(placement.rect)).expect("eye drawn");
        assert_eq!(eye(&app, 358, 85).height, 20);
        app.visible = [true, true, false, false, false];
        let grown = eye(&app, 358, 85);
        assert_eq!(grown.height, 40);
        assert!(grown.y > 16 && grown.bottom() < 84, "{grown:?}");
        assert_eq!(eye(&app, 170, 50).height, 15);
        app.visible = [false, true, false, false, false];
        assert_eq!(eye(&app, 358, 85).height, 40);
    }

    #[test]
    fn the_subsystems_are_a_stop_between_tiles_and_lists() {
        let mut app = crowded_app(Instant::now());
        press(&mut app, &[KeyCode::BackTab]);
        assert_eq!(app.part, Some(0));
        assert!(screen(&app, 358, 85).iter().any(|row| row.contains("←→ part")));
        press(&mut app, &[KeyCode::Left]);
        assert_eq!(app.part, Some(6));
        press(&mut app, &[KeyCode::Right, KeyCode::Right]);
        assert_eq!(app.part, Some(1));
        press(&mut app, &[KeyCode::Enter]);
        assert!(app.overlay);
        assert!(screen(&app, 358, 85).iter().any(|row| row.contains(":: graphics")));
        press(&mut app, &[KeyCode::Right]);
        assert_eq!((app.overlay, app.part), (true, Some(2)));
        press(&mut app, &[KeyCode::Esc]);
        assert_eq!((app.overlay, app.part), (false, Some(2)));
        press(&mut app, &[KeyCode::Tab]);
        assert_eq!((app.part, app.focus), (None, Pane::Updates));
        press(&mut app, &[KeyCode::BackTab, KeyCode::BackTab, KeyCode::Tab]);
        assert_eq!((app.tile, app.part), (None, Some(2)));
        press(&mut app, &[KeyCode::Char('2')]);
        assert_eq!(app.part, None);
    }

    #[test]
    fn the_kernel_window_names_the_reboot_this_update_brings() {
        let now = Instant::now();
        let mut app = system_app(now);
        app.part = Some(0);
        app.overlay = true;
        let rows = screen(&app, 358, 85);
        assert!(find(&rows, ":: kernel").is_some());
        assert!(find(&rows, "after this update · kernel 7.2.5-3 → 7.2.8-9").is_some());
        assert!(find(&rows, "linux-omarchy  7.2.5 → 7.2.8").is_some());
        app.part = Some(6);
        let rows = screen(&app, 358, 85);
        assert!(find(&rows, "1,915 × error · /usr/bin/conmon").is_some());
        assert!(find(&rows, "3 × error · hancore").is_some());
    }

    #[test]
    fn problem_windows_say_only_what_is_known() {
        let now = Instant::now();
        let mut app = system_app(now);
        app.focus = Pane::Problems;
        app.select(Pane::Problems, 1);
        app.overlay = true;
        let rows = screen(&app, 358, 85);
        assert!(find(&rows, "the journal has no program path for it").is_some());
        assert!(find(&rows, "kind          error").is_some());
        for gone in ["owner         unknown", "not a verified fix"] {
            assert!(!rows.iter().any(|row| row.contains(gone)), "{gone} shown");
        }
        assert!(rows.iter().any(|row| row.contains("j journal") && row.contains("c copy") && row.contains("r rescan")));
    }

    #[test]
    fn c_copies_the_open_window_as_text_in_one_piece() {
        let pal = palette::Palette::labra();
        let started = Instant::now();
        let mut app = App::new(true, false, started);
        for (time, event) in demo_events() {
            if time <= 30_000 {
                app.apply(event, started + Duration::from_millis(time));
            }
        }
        app.focus = Pane::Problems;
        app.select(Pane::Problems, 0);
        assert_eq!(press(&mut app, &[KeyCode::Char('c')]), Action::Nothing);
        assert_eq!(page::overlay_text(&app, &pal), None);
        app.overlay = true;
        assert_eq!(press(&mut app, &[KeyCode::Char('c')]), Action::Copy);
        assert_eq!(
            page::overlay_text(&app, &pal).unwrap().lines().collect::<Vec<_>>(),
            [
                "dbus-broker-launch",
                "subject       /usr/bin/dbus-broker-launch",
                "kind          error",
                "count         151 since 10-03 21:10",
                "last          10-17 23:38",
                "message       Activation request for 'org.freedesktop.home1' failed: The systemd unit 'dbus-org.freedesktop.home1.service' could not be found.",
                "owner         dbus-broker",
                "status        no update pending for the owner",
            ]
        );
        app.focus = Pane::Updates;
        app.select(Pane::Updates, 0);
        let text = page::overlay_text(&app, &pal).unwrap();
        for line in ["risk          high · critical package, minor version jump", "+ new dependency    ↑ updates now"] {
            assert!(text.lines().any(|row| row == line), "{line} missing in\n{text}");
        }
        assert!(text.lines().any(|row| row.starts_with("it will depend on 24: hyprwire +, mesa ↑, ")), "{text}");
        app.overlay = false;
        app.tile = Some(5);
        assert_eq!(press(&mut app, &[KeyCode::Char('c')]), Action::Nothing);
        app.overlay = true;
        assert_eq!(press(&mut app, &[KeyCode::Char('c')]), Action::Copy);
        assert!(page::overlay_text(&app, &pal).unwrap().starts_with("BEFORE YOU UPDATE\nrisk          2 high\n"));
        app.tile = None;
        app.part = Some(0);
        assert_eq!(press(&mut app, &[KeyCode::Char('c')]), Action::Copy);
        assert!(page::overlay_text(&app, &pal).unwrap().starts_with("kernel\nrunning       6.17.2-arch1-1\n"));
    }

    #[test]
    fn j_finds_journal_lines_by_program_when_the_identifier_differs() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Stage { stage: "problems".into(), data: json!({"problems": [{"kind": "journal", "subject": "/usr/bin/conmon", "count": 1915, "package": "conmon", "status": "no-update"}], "activity": []}) }, now);
        app.apply(source::Event::Journal { time: "14:17:39".into(), day: String::new(), ident: "peaceful_golick".into(), program: "conmon".into(), message: "Compiling rumb".into(), priority: String::new(), unit: String::new() }, now);
        app.apply(source::Event::Journal { time: "14:17:40".into(), day: String::new(), ident: "playerctld".into(), program: "playerctld".into(), message: "lost the bus".into(), priority: String::new(), unit: String::new() }, now);
        app.visible = [true; 5];
        app.focus = Pane::Problems;
        press(&mut app, &[KeyCode::Char('j')]);
        assert_eq!(app.focus, Pane::Events);
        let shown: Vec<&str> = app.visible_events().iter().map(|event| event.ident.as_str()).collect();
        assert_eq!(shown, vec!["peaceful_golick"]);
    }

    #[test]
    fn a_live_journal_event_shows_its_priority_and_unit() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Journal { time: "14:20:00".into(), day: String::new(), ident: "bluetoothd".into(), program: "bluetoothd".into(), message: "Failed to set mode".into(), priority: "2".into(), unit: "bluetooth.service".into() }, now);
        app.visible = [false, false, false, false, true];
        app.focus = Pane::Events;
        app.overlay = true;
        let rows = screen(&app, 170, 50);
        let row = |label: &str| rows.iter().find(|row| row.trim_start_matches(['│', ' ']).starts_with(label)).cloned().unwrap_or_default();
        assert!(row("source").contains("journal"), "{rows:#?}");
        assert!(row("priority").contains("critical"));
        assert!(row("unit").contains("bluetooth.service"));
        assert!(!rows.iter().any(|row| row.contains("priority error or higher")));
    }

    #[test]
    fn a_journal_problem_shows_its_newest_message() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        app.apply(source::Event::Stage { stage: "problems".into(), data: json!({"problems": [{"kind": "journal", "subject": "/usr/lib/bluetooth/bluetoothd", "count": 36, "package": "bluez", "status": "no-update", "last": "10-17 23:52", "message": "src/adapter.c: Failed to clear UUIDs: Busy (0x0a)"}], "activity": []}) }, now);
        app.visible = [false, false, false, true, false];
        app.focus = Pane::Problems;
        app.overlay = true;
        let rows = screen(&app, 170, 50);
        assert!(rows.iter().any(|row| row.contains("last") && row.contains("10-17 23:52")), "{rows:#?}");
        assert!(rows.iter().any(|row| row.contains("message") && row.contains("src/adapter.c: Failed to clear UUIDs: Busy (0x0a)")));
    }

    fn demo_detail(name: &str) -> App {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events() {
            if time <= 26_000 {
                app.apply(event, now + Duration::from_millis(time));
            }
        }
        app.focus = Pane::Updates;
        let index = app.visible_updates().iter().position(|update| update.name == name).unwrap();
        app.select(Pane::Updates, index as isize);
        app.overlay = true;
        app
    }

    #[test]
    fn an_update_shows_the_dependencies_of_its_new_version() {
        let rows = screen(&demo_detail("hyprland"), 170, 50);
        let all = rows.join("\n");
        assert!(all.contains("it will depend on 24"), "{all}");
        assert!(all.contains("hyprwire +"));
        assert!(rows.iter().any(|row| row.contains("dropped deps") && row.contains("libxcursor")));
        assert_eq!(all.matches("libxcursor").count(), 1);
        assert!(all.contains("+ new dependency"));
    }

    #[test]
    fn a_new_dependency_with_a_problem_keeps_the_problem_colour() {
        let app = demo_detail("quickshell");
        let (buffer, _) = render(&app, 170, 50, app.started + Duration::from_secs(60));
        let found = (0..50u16).find_map(|y| {
            let row: String = (0..170u16).map(|x| buffer[(x, y)].symbol()).collect();
            row.find("bluez +").map(|at| (row[..at].chars().count() as u16, y))
        });
        let (x, y) = found.expect("bluez is listed as a new dependency");
        assert_eq!(buffer[(x, y)].fg, palette::Palette::paper().teal);
    }

    fn demo_map(columns: u16, rows: u16) -> (u32, u32, ink::OrbitSpec) {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events() {
            if time <= 26_000 {
                app.apply(event, now + Duration::from_millis(time));
            }
        }
        app.visible = [false, false, true, false, false];
        app.focus = Pane::Updates;
        let (_, scene) = render(&app, columns, rows, app.started + Duration::from_secs(60));
        scene
            .placements
            .into_iter()
            .find_map(|placement| match placement.picture {
                page::Picture::Orbit(spec) => Some((u32::from(placement.rect.width) * 10, u32::from(placement.rect.height) * 21, *spec)),
                _ => None,
            })
            .expect("the update map is drawn")
    }

    #[test]
    fn the_map_links_end_at_the_ports_outside_the_dial() {
        let (width, height, spec) = demo_map(170, 50);
        assert_eq!(spec.links.iter().filter(|link| link.left).count(), 24);
        let pal = palette::Palette::paper();
        let with = ink::orbit(&spec, &pal, width, height, 21.0).unwrap();
        let without = ink::orbit(&ink::OrbitSpec { links: Vec::new(), ..spec }, &pal, width, height, 21.0).unwrap();
        let (cx, cy) = (width as f32 / 2.0, height as f32 / 2.0);
        let radius = ink::dial(width as f32, height as f32, 1.0);
        let changed: Vec<f32> = with
            .pixels()
            .iter()
            .zip(without.pixels())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| (((index as u32 % width) as f32 - cx).powi(2) + ((index as u32 / width) as f32 - cy).powi(2)).sqrt())
            .collect();
        assert!(!changed.is_empty());
        let inside = changed.iter().filter(|distance| **distance < radius * 1.08).count();
        assert_eq!(inside, 0, "{inside} of {} link pixels inside the dial", changed.len());
    }

    #[test]
    fn the_map_leaves_room_for_the_links() {
        let pal = palette::Palette::paper();
        for (columns, rows) in [(110, 36), (130, 40), (170, 50)] {
            let (width, height, spec) = demo_map(columns, rows);
            let with = ink::orbit(&spec, &pal, width, height, 21.0).unwrap();
            let without = ink::orbit(&ink::OrbitSpec { links: Vec::new(), ..spec }, &pal, width, height, 21.0).unwrap();
            let reach = with
                .pixels()
                .iter()
                .zip(without.pixels())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(index, _)| index as u32 % width)
                .filter(|x| *x < width / 2)
                .max()
                .unwrap();
            assert!(reach >= 66, "the links reach only {reach} px into a {width} px map at {columns}x{rows}");
        }
    }

    #[test]
    fn the_map_labels_stay_inside_the_picture() {
        let pal = palette::Palette::paper();
        for (columns, rows) in [(92, 31), (130, 40), (170, 50)] {
            let (width, height, spec) = demo_map(columns, rows);
            let labelled = ink::orbit(&spec, &pal, width, height, 21.0).unwrap();
            let bare = ink::orbit(&ink::OrbitSpec { sectors: spec.sectors.iter().map(|(_, count)| (String::new(), *count)).collect(), ..spec }, &pal, width, height, 21.0).unwrap();
            let label: Vec<(u32, u32)> = labelled.pixels().iter().zip(bare.pixels()).enumerate().filter(|(_, (a, b))| a != b).map(|(index, _)| (index as u32 % width, index as u32 / width)).collect();
            assert!(!label.is_empty());
            let on_edge = label.iter().filter(|(x, y)| *x < 1 || *x + 1 >= width || *y < 1 || *y + 1 >= height).count();
            assert_eq!(on_edge, 0, "{on_edge} label pixels on the edge at {columns}x{rows}");
        }
    }

    fn draw_map(app: &App, motion: &Motion, now: Instant) -> (ink::OrbitSpec, String) {
        let pal = palette::Palette::paper();
        let mut terminal = Terminal::new(TestBackend::new(170, 50)).unwrap();
        let mut scene = None;
        terminal.draw(|frame| scene = Some(page::draw(frame, app, motion, &pal, now, (10, 21)))).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text = (0..50u16).map(|y| (0..170u16).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n");
        let spec = scene
            .unwrap()
            .placements
            .into_iter()
            .find_map(|placement| match placement.picture {
                page::Picture::Orbit(spec) => Some(*spec),
                _ => None,
            })
            .expect("the update map is drawn");
        (spec, text)
    }

    fn demo_until(millis: u64, start: Instant) -> App {
        let mut app = App::new(true, true, start);
        for (time, event) in demo_events() {
            if time <= millis {
                app.apply(event, start);
            }
        }
        app.visible = [false, false, true, false, false];
        app.focus = Pane::Updates;
        app
    }

    #[test]
    fn the_map_brings_new_updates_in_ring_by_ring() {
        let start = Instant::now();
        let mut app = demo_until(3_100, start);
        let mut motion = Motion::new(true, start);
        motion.step(&mut app, start, 0.0);
        let (first, _) = draw_map(&app, &motion, start + Duration::from_millis(10));
        assert!(!first.nodes.is_empty());
        assert!(first.nodes.iter().all(|node| node.ring == 0 && node.faint), "{:?}", first.nodes);
        let (settled, _) = draw_map(&app, &motion, start + Duration::from_secs(1));
        assert_eq!(settled.nodes.len(), app.updates.len());
        assert!(settled.nodes.iter().all(|node| !node.faint));
    }

    #[test]
    fn a_new_selection_brings_its_names_in_from_the_ports() {
        let start = Instant::now();
        let mut app = demo_until(26_000, start);
        let mut motion = Motion::new(true, start);
        motion.step(&mut app, start, 0.0);
        let later = start + Duration::from_secs(3);
        app.select(Pane::Updates, 1);
        motion.step(&mut app, later, 0.0);
        let chosen = app.visible_updates()[1].name.clone();
        let names = app.anatomy.as_ref().unwrap().deps[&chosen].depends_on.clone();
        let shown = |after: Duration| {
            let (_, text) = draw_map(&app, &motion, later + after);
            names.iter().filter(|name| text.contains(&format!(" {name} "))).count()
        };
        let (early, full) = (shown(Duration::from_millis(20)), shown(Duration::from_secs(1)));
        assert_eq!(full, names.len().min(23));
        assert!(early < full / 2, "{early} of {full} names right after the selection");
    }

    #[test]
    fn a_rescan_brings_the_map_names_in_again() {
        let start = Instant::now();
        let mut app = demo_until(26_000, start);
        let mut motion = Motion::new(true, start);
        motion.step(&mut app, start, 0.0);
        let rescan = start + Duration::from_secs(5);
        let anatomy = demo_events().into_iter().map(|(_, event)| event).find(|event| matches!(event, source::Event::Stage { stage, .. } if stage == "anatomy")).unwrap();
        app.apply(anatomy, rescan);
        motion.step(&mut app, rescan, 0.0);
        let chosen = app.visible_updates()[0].name.clone();
        let names = app.anatomy.as_ref().unwrap().deps[&chosen].depends_on.clone();
        let shown = |after: Duration| {
            let (_, text) = draw_map(&app, &motion, rescan + after);
            names.iter().filter(|name| text.contains(&format!(" {name} "))).count()
        };
        let (early, full) = (shown(Duration::from_millis(20)), shown(Duration::from_secs(1)));
        assert_eq!(full, names.len());
        assert!(early < full / 2, "{early} of {full} names right after the rescan");
    }

    #[test]
    fn scan_results_show_as_soon_as_they_arrive() {
        let start = Instant::now();
        let mut app = App::new(false, true, start);
        let mut motion = Motion::new(true, start);
        let mut at = 0;
        for line in include_str!("../demo.ndjson").lines().filter(|line| !line.trim().is_empty()) {
            let value: Value = serde_json::from_str(line).unwrap();
            at += value["_delay"].as_u64().unwrap_or(0);
            let now = start + Duration::from_millis(at);
            motion.push(serde_json::from_str(line).unwrap(), line, &mut app, now);
            motion.step(&mut app, now, 0.0);
            if value["event"] == "stage" {
                match value["stage"].as_str() {
                    Some("updates") => assert!(!app.updates.is_empty(), "no updates right after they arrived at {at} ms"),
                    Some("anatomy") => assert!(app.anatomy.is_some(), "no dependencies right after they arrived at {at} ms"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn each_scan_result_is_revealed_once() {
        let start = Instant::now();
        let mut app = App::new(false, true, start);
        let mut motion = Motion::new(true, start);
        let mut stream = Vec::new();
        let mut at = 0;
        for line in include_str!("../demo.ndjson").lines().filter(|line| !line.trim().is_empty()) {
            let value: Value = serde_json::from_str(line).unwrap();
            at += value["_delay"].as_u64().unwrap_or(0);
            stream.push((at, line));
        }
        let (mut next, mut first) = (0, [None; 4]);
        for millis in (0..22_000).step_by(33) {
            let now = start + Duration::from_millis(millis);
            while next < stream.len() && stream[next].0 <= millis {
                motion.push(serde_json::from_str(stream[next].1).unwrap(), stream[next].1, &mut app, now);
                next += 1;
            }
            motion.step(&mut app, now, 0.033);
            for (index, stamp) in app.stage_done_at.into_iter().enumerate() {
                if let Some(stamp) = stamp {
                    assert_eq!(*first[index].get_or_insert(stamp), stamp, "stage {index} was revealed again at {millis} ms");
                }
            }
        }
        assert!(first.iter().all(Option::is_some));
    }

    #[test]
    fn an_idle_glance_at_the_map_lets_its_names_light_up() {
        let start = Instant::now();
        let mut app = demo_until(26_000, start);
        let mut motion = Motion::new(true, start);
        let mut now = start;
        let mut glow = None;
        while glow.is_none() && now < start + Duration::from_secs(60) {
            now += Duration::from_millis(33);
            motion.step(&mut app, now, 0.033);
            if motion.map_glow(now).is_some() {
                glow = Some(now);
            }
        }
        let glow = glow.expect("an idle glance at the update list lights the names");
        let pal = palette::Palette::paper();
        let colours = |at: Instant| {
            let mut terminal = Terminal::new(TestBackend::new(170, 50)).unwrap();
            terminal.draw(|frame| drop(page::draw(frame, &app, &motion, &pal, at, (10, 21)))).unwrap();
            terminal.backend().buffer().content().iter().map(|cell| cell.fg).collect::<std::collections::HashSet<_>>().len()
        };
        assert!(colours(glow + Duration::from_millis(300)) > colours(glow + Duration::from_secs(6)));
    }

    #[test]
    fn paused_events_keep_the_selected_line_when_new_ones_arrive() {
        let now = Instant::now();
        let mut app = crowded_app(now);
        app.focus = Pane::Events;
        press(&mut app, &[KeyCode::Down, KeyCode::Down]);
        assert!(!app.follow);
        let chosen = app.visible_events()[2].ident.clone();
        app.apply(source::Event::Journal { time: "12:00:01".into(), day: String::new(), ident: "late".into(), program: String::new(), message: "boom".into(), priority: String::new(), unit: String::new() }, now);
        assert_eq!(app.visible_events()[app.lists[2].selected].ident, chosen);
        app.lists[2].filter = "ident-1".into();
        app.select(Pane::Events, 1);
        let chosen = app.visible_events()[1].ident.clone();
        app.apply(source::Event::Journal { time: "12:00:02".into(), day: String::new(), ident: "other".into(), program: String::new(), message: "boom".into(), priority: String::new(), unit: String::new() }, now);
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
            app.apply(source::Event::Journal { time: "12:00:00".into(), day: String::new(), ident: "x".into(), program: String::new(), message: "y".into(), priority: String::new(), unit: String::new() }, now);
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
        app.apply(source::Event::Journal { time: "12:00:03".into(), day: String::new(), ident: "evil|ident".into(), program: String::new(), message: "red \u{1b}[31mtext\u{7}".into(), priority: String::new(), unit: String::new() }, now);
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
    fn journal_lines_without_a_program_still_load() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        let line = json!({"time": "20:29:22", "day": "2026-10-21", "ident": "kernel", "program": null, "message": "virt/tdx: TDX not supported by the host platform"});
        app.apply(source::Event::Stage { stage: "anatomy".into(), data: json!({"packages": 1, "explicit": 1, "orphans": 0, "cache_bytes": 0, "kernel_running": "7.2.8-10-omarchy", "journal": [line]}) }, now);
        assert_eq!(app.error, None);
        assert_eq!(app.anatomy.as_ref().map(|anatomy| anatomy.journal[0].program.as_str()), Some(""));
        let live: source::Event = serde_json::from_str(r#"{"event": "journal", "time": "20:29:24", "ident": "kernel", "program": null, "message": "Bluetooth: hci0: Reading supported features failed (-16)"}"#).unwrap();
        assert!(matches!(live, source::Event::Journal { ref program, .. } if program.is_empty()));
    }

    #[test]
    fn a_selected_part_keeps_its_problem_badge() {
        let pal = palette::Palette::paper();
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events() {
            app.apply(event, now + Duration::from_millis(time));
        }
        app.enter_band();
        let mut inside = 0;
        for part in 0..app::Subsystem::ALL.len() {
            app.part = Some(part);
            let (buffer, _) = render(&app, 170, 50, now + Duration::from_secs(60));
            for y in 0..50 {
                for x in 2..168 {
                    let cell = &buffer[(x, y)];
                    if cell.fg != pal.page || cell.symbol() == " " {
                        continue;
                    }
                    assert_ne!(cell.bg, pal.select, "part {part} at {x},{y}");
                    if buffer[(x - 2, y)].bg == pal.select || buffer[(x + 2, y)].bg == pal.select {
                        inside += 1;
                    }
                }
            }
        }
        assert!(inside > 0);
    }

    #[test]
    fn a_terminal_without_pictures_says_so_in_place_of_the_eye() {
        let now = Instant::now();
        let mut app = App::new(true, false, now);
        for (time, event) in demo_events() {
            app.apply(event, now + Duration::from_millis(time));
        }
        let note = |app: &App| screen(app, 170, 50).iter().filter(|row| row.contains("this terminal shows no pictures") || row.contains("foot and kitty show the eye")).count();
        assert_eq!(note(&app), 0);
        app.pictures = false;
        assert_eq!(note(&app), 2);
    }

    #[test]
    fn a_later_start_moves_the_window_but_not_the_intro() {
        let now = Instant::now();
        let mut app = App::new(false, false, now);
        let start = |since: &str| source::Event::Start { last_upgrade: Some(since.into()), problems_since: since.into(), now: None, omarchy: false, last_upgraded: Vec::new() };
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
        let mut theme = theme::pick(Some(&std::env::var("RUMB_THEME").unwrap_or_else(|_| "labra".into()))).expect("known theme");
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
        let keys: Vec<(u64, KeyCode)> = std::env::var("RUMB_KEYS")
            .ok()
            .map(|text| {
                text.split(',')
                    .filter_map(|pair| pair.split_once(':'))
                    .filter_map(|(time, name)| {
                        let code = match name {
                            "esc" => KeyCode::Esc,
                            "left" => KeyCode::Left,
                            "right" => KeyCode::Right,
                            "enter" => KeyCode::Enter,
                            "tab" => KeyCode::Tab,
                            "backtab" => KeyCode::BackTab,
                            other => KeyCode::Char(other.chars().next()?),
                        };
                        Some((time.parse().ok()?, code))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let only: Option<Vec<u64>> = std::env::var("RUMB_ONLY").ok().map(|text| text.split(',').filter_map(|frame| frame.trim().parse().ok()).collect());
        let started = Instant::now();
        let mut app = App::new(false, true, started);
        app.theme.clone_from(&theme.name);
        app.pictures = std::env::var_os("RUMB_NO_PICTURES").is_none();
        if let Ok(panels) = std::env::var("RUMB_PANELS") {
            for (index, on) in panels.chars().enumerate().take(app.visible.len()) {
                app.visible[index] = on == '1';
            }
        }
        let mut motion = Motion::new(true, started);
        let (mut next, mut chosen, mut pressed, mut last) = (0usize, 0usize, 0usize, started);
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
            while pressed < keys.len() && keys[pressed].0 <= millis {
                let before = (app.focus, app.lists[app.focus.index()].selected, app.tile, app.part);
                match on_key(&mut app, key(keys[pressed].1), [10; 3], now) {
                    Action::Theme(forward) => {
                        step_theme(&mut theme, forward, &app.theme_filter);
                        app.theme.clone_from(&theme.name);
                    }
                    Action::ThemeFilter => {
                        filter_theme(&mut theme, &app.theme_filter);
                        app.theme.clone_from(&theme.name);
                    }
                    _ if (app.focus, app.lists[app.focus.index()].selected, app.tile, app.part) != before => motion.select(&app, now),
                    _ => {}
                }
                pressed += 1;
            }
            motion.step(&mut app, now, now.saturating_duration_since(last).as_secs_f32());
            last = now;
            if only.as_ref().is_none_or(|only| only.contains(&frame)) {
                let begun = Instant::now();
                write_frame(&dir, &format!("video-{frame:04}"), size, &app, &motion, &theme.palette, now, cell);
                costs.push(begun.elapsed().as_secs_f64() * 1000.0);
            } else {
                let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
                terminal.draw(|f| drop(page::draw(f, &app, &motion, &theme.palette, now, cell))).unwrap();
            }
        }
        costs.sort_by(f64::total_cmp);
        println!("frames written {} median {:.1} ms", costs.len(), costs.get(costs.len() / 2).copied().unwrap_or(0.0));
    }
}
