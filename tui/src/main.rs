mod app;
mod ink;
mod palette;
mod pictures;
mod source;
mod theme;
mod ui;

use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};

use app::App;
use source::Msg;
use theme::Theme;

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

Without a terminal of at least 150x44 (or with NO_COLOR set) it prints the
plain text report instead.";

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
    let fits = ratatui::crossterm::terminal::size()
        .map(|(width, height)| width >= ui::MIN_WIDTH && height >= ui::MIN_HEIGHT)
        .unwrap_or(false);
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());

    if !interactive || !fits || no_color {
        if options.demo {
            eprintln!("--demo needs an interactive terminal of at least {}x{}", ui::MIN_WIDTH, ui::MIN_HEIGHT);
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
    let mut pictures = pictures::Pictures::query();
    let result = run(&mut terminal, &options, script, &mut theme, &mut pictures);
    ratatui::restore();

    match result {
        Ok(true) => ExitCode::from(1),
        Ok(false) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

fn start_source(options: &Options, script: Option<&PathBuf>) -> io::Result<(Option<Child>, Receiver<Msg>)> {
    if options.demo {
        return Ok((None, source::spawn_demo()));
    }
    let script = script.expect("checked in main");
    let (child, rx) = source::spawn_script(script, &options.args)?;
    Ok((Some(child), rx))
}

fn stop(child: &mut Option<Child>, running: bool) {
    if let Some(mut child) = child.take() {
        if running {
            let _ = child.kill();
        }
        let _ = child.wait();
    }
}

fn run(terminal: &mut DefaultTerminal, options: &Options, script: Option<PathBuf>, theme: &mut Theme, pictures: &mut Option<pictures::Pictures>) -> io::Result<bool> {
    let (mut child, mut rx) = start_source(options, script.as_ref())?;
    let mut app = App::new(options.demo, !options.no_anim, Instant::now());
    let mut source_open = true;

    loop {
        let now = Instant::now();
        while source_open {
            match rx.try_recv() {
                Ok(Msg::Event(event)) => app.apply(event, now),
                Ok(Msg::Invalid) => {}
                Ok(Msg::Eof { stderr }) => {
                    source_open = false;
                    let status = child.as_mut().and_then(|child| child.wait().ok());
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
            let scene = ui::draw(frame, &app, &model, &theme.palette, now);
            if let Some(pictures) = pictures.as_mut() {
                let anim = ui::astrolabe_anim(&app, &model, now);
                let astrolabe = format!("{anim:?}{:?}{}", scene.callouts, model.nodes.len());
                pictures.draw(frame, &scene, (theme.generation, &astrolabe), now, |picture, width, height, cell_height| {
                    ui::paint(picture, &scene, &model, &anim, &theme.palette, width, height, cell_height)
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
                        stop(&mut child, source_open);
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

    stop(&mut child, source_open);
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
            let pixmap = ui::paint(&placement.picture, &scene, &model, &anim, &pal, w, h, cell_h as f32).or_else(|| tiny_skia::Pixmap::new(w, h));
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
