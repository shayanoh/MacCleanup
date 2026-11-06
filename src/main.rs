use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use maccleanup::accounting::Accounting;
use maccleanup::app::{Action, App};
use maccleanup::cli::Cli;
use maccleanup::glyphs::{GlyphMode, Glyphs};
use maccleanup::safety::Policy;
use maccleanup::scan::Scanner;
use maccleanup::softdelete::Holding;
use maccleanup::{input, perms, ui, volume};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event};
use ratatui::crossterm::execute;

/// ~20 frames per second: one animation tick per frame.
const FRAME: Duration = Duration::from_millis(50);

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("maccleanup: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| !h.as_os_str().is_empty())
        .context("the HOME environment variable is not set")?;
    let home = home
        .canonicalize()
        .with_context(|| format!("cannot read your home folder {}", home.display()))?;

    let root = cli
        .root()
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let root = root
        .canonicalize()
        .with_context(|| format!("cannot read {}", root.display()))?;
    if !root.is_dir() {
        bail!("{} is not a folder", root.display());
    }
    let holding_dir = cli
        .holding_dir
        .clone()
        .unwrap_or_else(|| home.join("ToBeDeleted"));
    if holding_dir.exists() && !holding_dir.is_dir() {
        bail!("{} exists but is not a folder", holding_dir.display());
    }
    // Check this before creating anything on disk.
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        bail!("MacCleanup needs an interactive terminal");
    }

    let holding = Holding::open(holding_dir.clone())
        .with_context(|| format!("cannot create {}", holding_dir.display()))?;
    let policy = Policy::new(&home, &holding_dir);
    let scanner = Scanner::new(Scanner::default_threads());
    let root_node = scanner.scan_root(&root);
    let free = volume::free_space(&root).context("cannot read the free disk space")?;
    let glyphs = Glyphs::new(if cli.ascii {
        GlyphMode::Ascii
    } else {
        GlyphMode::Nerd
    });

    let mut app = App::new(
        scanner,
        root_node,
        policy,
        holding,
        Accounting::new(free),
        glyphs,
        !cli.no_animation,
    );
    app.fda_missing = perms::full_disk_access_missing(&home);
    let result = run_tui(&mut app);

    // The terminal is restored now: say what this session did.
    let free_now = volume::free_space(&root).unwrap_or(app.free_now);
    match app.accounting.summary(free_now) {
        Some(summary) => println!("{summary}"),
        None => println!("MacCleanup: nothing was deleted this session."),
    }
    result
}

/// Set up the terminal, run the app, and always put the terminal back.
fn run_tui(app: &mut App) -> Result<()> {
    // Turn mouse capture off even when we panic; ratatui's own hook (installed
    // by `init`) restores raw mode and the alternate screen and then calls this.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture);
        previous(info);
    }));

    let mut terminal = ratatui::init();
    if let Err(e) = execute!(io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(e).context("cannot enable mouse support");
    }
    let result = event_loop(&mut terminal, app);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut next_tick = Instant::now() + FRAME;
    let mut dirty = true;
    while !app.should_quit {
        if dirty {
            terminal.draw(|frame| ui::draw(frame, app))?;
            dirty = false;
        }
        let timeout = next_tick.saturating_duration_since(Instant::now());
        if event::poll(timeout)? {
            // Handle everything that is queued before drawing again.
            loop {
                match event::read()? {
                    Event::Key(key) => {
                        if let Some(action) = input::map_key(key, app) {
                            app.update(action);
                            dirty = true;
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Some(action) = input::map_mouse(mouse, app) {
                            app.update(action);
                            dirty = true;
                        }
                    }
                    Event::Resize(..) => dirty = true,
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        if Instant::now() >= next_tick {
            app.update(Action::Tick);
            next_tick = Instant::now() + FRAME;
            dirty = true;
        }
    }
    Ok(())
}
