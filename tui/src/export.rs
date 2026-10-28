use std::fmt::Write as _;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::app::{App, Source, Subsystem};
use crate::page::{bytes, thousands};

pub fn dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(|home| PathBuf::from(home).join("rumb-exports"))
}

pub fn write(app: &App, dir: &Path, stamp: &str) -> io::Result<PathBuf> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let report = report(app);
    for attempt in 0..100 {
        let name = if attempt == 0 { format!("rumb-{stamp}.md") } else { format!("rumb-{stamp}-{attempt}.md") };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(mut file) => {
                file.write_all(report.as_bytes())?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "too many exports in one second"))
}

pub fn copy(text: &str) -> io::Result<()> {
    let (wayland, x11) = (std::env::var_os("WAYLAND_DISPLAY").is_some(), std::env::var_os("DISPLAY").is_some());
    let tools: [(&str, &[&str], bool); 3] = [("wl-copy", &[], wayland), ("xclip", &["-selection", "clipboard"], x11), ("xsel", &["--clipboard", "--input"], x11)];
    let found = |name: &str| std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()));
    let Some((program, args, _)) = tools.into_iter().find(|(program, _, usable)| *usable && found(program)) else {
        return Err(io::Error::other("needs wl-copy, xclip or xsel"));
    };
    let mut child = Command::new(program).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    child.stdin.take().ok_or_else(|| io::Error::other("no input pipe"))?.write_all(text.as_bytes())?;
    let status = child.wait()?;
    if status.success() { Ok(()) } else { Err(io::Error::other(format!("{program} {status}"))) }
}

fn signed(value: i64) -> String {
    format!("{}{}", if value < 0 { "-" } else { "+" }, bytes(value.unsigned_abs()))
}

fn clean(text: &str) -> String {
    text.chars().map(|ch| if ch.is_control() { ' ' } else { ch }).collect()
}

fn cell(text: &str) -> String {
    clean(text).replace('|', "\\|")
}

pub fn report(app: &App) -> String {
    let mut out = String::from("# rumb report\n\n");
    if let Some(anatomy) = &app.anatomy {
        let mut system: Vec<String> = [&anatomy.hostname, &anatomy.os, &anatomy.desktop].into_iter().filter(|part| !part.is_empty()).map(|part| clean(part)).collect();
        if !anatomy.kernel_running.is_empty() {
            system.push(format!("kernel {}", clean(&anatomy.kernel_running)));
        }
        let _ = writeln!(out, "{}\n", system.join(" · "));
    }
    if let Some(time) = &app.scan_time {
        let _ = writeln!(out, "- scanned: {}", clean(time));
    }
    if let Some(time) = &app.last_upgrade {
        let _ = writeln!(out, "- last full upgrade: {}", clean(time));
    }
    if let Some(time) = &app.since {
        let _ = writeln!(out, "- problems counted since: {}", clean(time));
    }
    if app.finished_at.is_some() {
        let _ = writeln!(out, "\n**{}**", app.verdict().0);
    } else {
        out.push_str("\n**scan still running, this report is partial**\n");
    }
    if let Some(error) = &app.error {
        let _ = writeln!(out, "\nscan failed: {}", clean(error));
    }

    out.push_str("\n## Before you update\n\n");
    if app.stage_done_at[0].is_some() {
        let _ = writeln!(out, "- high risk updates: {}", app.high());
    }
    if let Some(anatomy) = &app.anatomy {
        let after: Vec<String> = app.reboot_after().iter().map(|(what, update)| format!("{what} {} -> {}", clean(&update.old), clean(&update.new))).collect();
        let reboot = if anatomy.reboot_pending {
            format!("pending, kernel {} is installed", clean(&anatomy.kernel_installed))
        } else if !after.is_empty() {
            format!("after this update ({})", after.join(", "))
        } else {
            "not needed".into()
        };
        let _ = writeln!(out, "- reboot: {reboot}");
    }
    if let Some(preflight) = &app.preflight {
        let snapshot = match preflight.snapshot_by.as_str() {
            "snap-pac" => "snap-pac takes one before each pacman transaction",
            "omarchy-update" => "omarchy-update takes one first, pacman -Syu takes none",
            _ if preflight.snapper_configured => "snapper is set up, but nothing takes one before an update",
            _ => "none set up",
        };
        let _ = writeln!(out, "- snapshot: {snapshot}");
        let need = app.space_needed();
        let needed = if need > 0 { format!(", about {} needed", bytes(need)) } else { String::new() };
        let _ = writeln!(out, "- available on /: {}{needed}{}", bytes(preflight.free_bytes), if preflight.free_ok { "" } else { ", under 10 GiB" });
    }
    for error in app.update_errors.iter().chain(&app.problem_errors) {
        let _ = writeln!(out, "- could not run: {}", clean(error));
    }
    if app.news_failed {
        out.push_str("- could not fetch the Arch news\n");
    }
    if app.aur_unavailable {
        out.push_str("- AUR updates not checked\n");
    }

    let _ = writeln!(out, "\n## Updates ({})\n", app.updates.len());
    if app.updates.is_empty() {
        out.push_str(if app.stage_done_at[0].is_some() { "Nothing pending.\n" } else { "Not checked yet.\n" });
    } else {
        let _ = writeln!(out, "Download {}, installed size change {}.\n", bytes(app.download_bytes()), signed(app.installed_delta()));
        out.push_str("| risk | package | old | new | jump | part | source | download | reason |\n|---|---|---|---|---|---|---|---|---|\n");
        for update in app.sorted_updates() {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                cell(&update.risk),
                cell(&update.name),
                cell(&update.old),
                cell(&update.new),
                cell(&update.bump),
                Subsystem::of(&update.name).label(),
                cell(&update.source),
                update.download_bytes.map(bytes).unwrap_or_default(),
                cell(&update.reason)
            );
        }
    }

    let _ = writeln!(out, "\n## Problems ({})\n", app.problems.len());
    if app.problems.is_empty() {
        out.push_str(if app.stage_done_at[1].is_some() { "None found.\n" } else { "Not checked yet.\n" });
    } else {
        out.push_str("| count | kind | subject | owner | status |\n|---|---|---|---|---|\n");
        for problem in app.problems_by_count() {
            let status = match problem.status.as_str() {
                "possibly-fixed" => format!("owner has an update: {}", problem.update),
                other => other.to_string(),
            };
            let _ = writeln!(out, "| {} | {} | {} | {} | {} |", problem.count, cell(&problem.kind), cell(&problem.subject), cell(&problem.package), cell(&status));
        }
        if app.problems.iter().any(|problem| problem.status == "possibly-fixed") {
            out.push_str("\nAn update to the owner is a hint, not a verified fix.\n");
        }
    }

    if let Some(anatomy) = &app.anatomy {
        out.push_str("\n## Disk and packages\n\n");
        if let Some(preflight) = &app.preflight
            && preflight.disk_total_bytes > 0
        {
            let _ = writeln!(out, "- disk /: {:.0}% used, {} available", preflight.disk_used_bytes as f64 * 100.0 / preflight.disk_total_bytes as f64, bytes(preflight.free_bytes));
        }
        let _ = writeln!(out, "- installed: {}", thousands(anatomy.packages));
        let _ = writeln!(out, "- explicit: {}", thousands(anatomy.explicit));
        let _ = writeln!(out, "- foreign: {}", thousands(anatomy.repos.get("aur").copied().unwrap_or(0)));
        let _ = writeln!(out, "- orphans: {}", thousands(anatomy.orphans));
        let freeable = anatomy.cache_freeable_bytes.map(|freeable| format!(", {} freeable with paccache -rk1", bytes(freeable))).unwrap_or_default();
        let _ = writeln!(out, "- pacman cache: {}{freeable}", bytes(anatomy.cache_bytes));
    }

    if !app.news.is_empty() {
        out.push_str("\n## News\n\n");
        for news in &app.news {
            let manual = if news.manual_intervention { " (manual intervention)" } else { "" };
            let _ = writeln!(out, "- {} {}{manual} {}", clean(&news.date), clean(&news.title), clean(&news.link));
        }
    }

    let _ = writeln!(out, "\n## Live events ({}, newest first)\n", app.events.len());
    if app.events.is_empty() {
        out.push_str("None.\n");
    }
    for event in app.events.iter().rev() {
        let tag = if event.source == Source::Pacman { "P" } else { "J" };
        let when = if event.day.is_empty() { clean(&event.clock) } else { format!("{} {}", clean(&event.day), clean(&event.clock)) };
        let _ = writeln!(out, "    {when} {tag} {}: {}", clean(&event.ident), clean(&event.message));
    }
    out
}
