mod app;
mod browser;
mod config;
mod event;
mod model;
mod panel_server;
mod parser;
mod render;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::KeyCode,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::*;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use app::{App, AppConfig, ConnectionType, RenderMode, VizMode};
use browser::FileBrowser;
use model::selection::{ResidueColorSpec, resolve_residue_colors};

macro_rules! log {
    ($file:expr, $($arg:tt)*) => {
        if let Some(f) = $file.as_mut() {
            use std::io::Write;
            let _ = writeln!(f, $($arg)*);
            let _ = f.flush();
        }
    };
}

/// Terminal protein structure viewer
#[derive(Parser)]
#[command(name = "proteinview", version, about = "TUI protein structure viewer")]
struct Cli {
    /// Path to a PDB, mmCIF, or XYZ file, or a directory to browse
    file: Option<String>,

    /// Use HD rendering (HalfBlock over SSH, FullHD locally)
    #[arg(long)]
    hd: bool,

    /// Force full pixel graphics (Sixel/Kitty/iTerm2) regardless of SSH
    #[arg(long, alias = "pixel")]
    fullhd: bool,

    /// Render mode: braille, halfblock (or hd), hdplus (or hd+), fullhd (or pixel)
    /// [default: braille, or `defaults.render` from the config file]
    #[arg(long = "render", value_name = "MODE")]
    render_mode: Option<String>,

    /// Color scheme: structure, element, chain, plddt, bfactor, rainbow
    /// [default: structure, or `defaults.color` from the config file]
    #[arg(long)]
    color: Option<String>,

    /// Override one exact polymer residue color: CHAIN:RES[ICODE]=RRGGBB
    #[arg(long, value_name = "SELECTOR=RRGGBB")]
    residue_color: Vec<ResidueColorSpec>,

    /// Config file (TOML): colors, fog, startup defaults.
    /// Defaults to ~/.config/proteinview/config.toml when present
    #[arg(long, alias = "palette", value_name = "FILE")]
    config: Option<PathBuf>,

    /// Start on this named palette from the config file, rather than
    /// `defaults.palette`. `p` cycles between them in the TUI
    #[arg(long, value_name = "NAME")]
    palette_name: Option<String>,

    /// Visualization mode: cartoon, backbone, wireframe
    /// [default: cartoon, or `defaults.mode` from the config file]
    #[arg(long)]
    mode: Option<String>,

    /// Fetch structure from RCSB PDB by ID
    #[arg(long)]
    fetch: Option<String>,

    /// Run the persistent headless panel server over NDJSON stdin/stdout
    #[arg(long, requires = "output", conflicts_with = "snapshot")]
    panel_server: bool,

    /// PNG path atomically replaced by the panel server
    #[arg(long, value_name = "PNG", requires = "panel_server")]
    output: Option<PathBuf>,

    /// Initial panel framebuffer width in pixels
    #[arg(long, default_value_t = 960, value_name = "PX")]
    panel_width: u32,

    /// Initial panel framebuffer height in pixels
    #[arg(long, default_value_t = 540, value_name = "PX")]
    panel_height: u32,

    /// Render one FullHD pixel frame to a PNG and exit without starting the TUI
    #[arg(long, value_name = "PNG", conflicts_with = "panel_server")]
    snapshot: Option<PathBuf>,

    /// Snapshot width in pixels
    #[arg(
        long,
        default_value_t = render::snapshot::DEFAULT_SNAPSHOT_WIDTH,
        value_name = "PX"
    )]
    snapshot_width: u32,

    /// Snapshot height in pixels
    #[arg(
        long,
        default_value_t = render::snapshot::DEFAULT_SNAPSHOT_HEIGHT,
        value_name = "PX"
    )]
    snapshot_height: u32,

    /// Highlight the interface for this focus-chain ID in a snapshot
    #[arg(long, value_name = "CHAIN", requires = "snapshot")]
    snapshot_interface_chain: Option<String>,

    /// Overlay classified interface interaction lines in a snapshot
    #[arg(long, requires = "snapshot_interface_chain", requires = "snapshot")]
    snapshot_interactions: bool,

    /// Hide ligands and ions in a snapshot
    #[arg(long, requires = "snapshot")]
    snapshot_hide_ligands: bool,

    /// Write debug log to file (e.g. --log debug.log)
    #[arg(long)]
    log: Option<String>,

    /// Number of render threads (default: one per core)
    #[arg(long)]
    threads: Option<usize>,
}

/// Rebuild the sequence panel's wrapped layout for the current terminal size.
///
/// Cheap unless the width actually changed, so it is called both before and
/// after input handling.
fn viewport_width(term_cols: u16, browser: Option<&FileBrowser>, show_interface: bool) -> u16 {
    let file_panel_width = browser.map_or(0, |browser| {
        browser::panel_width(term_cols, browser.visible)
    });
    term_cols
        .saturating_sub(file_panel_width)
        .saturating_sub(if show_interface {
            ui::interface_panel::SIDEBAR_WIDTH
        } else {
            0
        })
        .max(1)
}

fn sync_sequence_viewport(app: &mut App, browser: Option<&FileBrowser>, fallback: (u16, u16)) {
    if !app.show_sequence {
        return;
    }
    let (cols, rows) = crossterm::terminal::size().unwrap_or(fallback);
    let panel_width = viewport_width(cols, browser, app.show_interface);
    let panel_height = ui::sequence_panel::height_for(app, rows);
    app.set_sequence_viewport(panel_width, panel_height);
}

fn load_structure_file(file: &Path) -> Result<model::protein::Protein> {
    let file_str = file
        .to_str()
        .with_context(|| format!("structure path is not valid UTF-8: '{}'", file.display()))?;
    if file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xyz"))
    {
        parser::xyz::load_xyz(file_str)
    } else {
        parser::pdb::load_structure(file_str)
    }
}

/// Keys the sequence panel owns while it is open.
///
/// Returns `true` when the key was consumed, so anything the panel does not
/// claim still reaches the normal bindings and the camera stays live.
fn handle_sequence_key(app: &mut App, key: crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyModifiers;

    // Shift plus an arrow extends the selection from where the cursor was, the
    // way a text editor does.
    let extend = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Left => app.seq_move_horizontal(-1, extend),
        KeyCode::Right => app.seq_move_horizontal(1, extend),
        KeyCode::Up => app.seq_move_vertical(-1, extend),
        KeyCode::Down => app.seq_move_vertical(1, extend),
        KeyCode::PageUp => app.seq_page(false, extend),
        KeyCode::PageDown => app.seq_page(true, extend),
        KeyCode::Home => app.seq_move_to_chain_edge(false, extend),
        KeyCode::End => app.seq_move_to_chain_edge(true, extend),
        KeyCode::Enter => app.seq_toggle_selection(),
        KeyCode::Char('A') => app.seq_toggle_chain_selection(),
        KeyCode::Char('x') => app.clear_selection(),
        KeyCode::Char('<') | KeyCode::Char(',') => app.resize_sequence_panel(-1),
        KeyCode::Char('>') | KeyCode::Char('.') => app.resize_sequence_panel(1),
        _ => return false,
    }
    true
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve the config before anything renders or reads a default from it.  A
    // bad config is a hard error rather than a silent fallback: ignoring a file
    // the user wrote is worse than refusing to start.
    config::init(cli.config.as_deref())?;
    // A palette named on the command line beats the one the file starts on.
    // Naming one that does not exist is an error rather than a silent fallback
    // to `default`: a snapshot cannot show you it was ignored.
    if let Some(name) = &cli.palette_name {
        if !config::set_palette(name) {
            let known: Vec<&str> = config::config()
                .palettes
                .iter()
                .map(|p| p.name.as_str())
                .collect();
            anyhow::bail!(
                "no palette named {name:?} in the config; this file has {}",
                known.join(", ")
            );
        }
    }
    let defaults = &config::config().defaults;

    // Rasterization splits the framebuffer into bands, so it scales with cores
    // until it becomes memory-bound.  Default to one thread per core: with the
    // shared-memory transport the terminal no longer has a frame to decompress
    // on every tick, so there is no longer a reason to hold cores back for it.
    // Cap at 16 -- beyond that the bands get too thin to be worth a thread.
    let num_threads = cli.threads.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(4, std::num::NonZeroUsize::get)
            .min(16)
    });
    let num_threads = num_threads.max(1);
    match rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global()
    {
        Ok(()) => {}
        Err(e) => eprintln!("Warning: failed to initialize rayon thread pool: {e}"),
    }

    // Resolve either one structure or a directory-backed interactive browser.
    // Fetched structures are temporary inputs and are removed immediately
    // after parsing, including when parsing fails.
    let fetched_path = cli
        .fetch
        .as_deref()
        .map(parser::fetch::fetch_pdb)
        .transpose()?;
    let (file_path, mut browser) = if let Some(file) = fetched_path.as_ref() {
        (PathBuf::from(file), None)
    } else if let Some(file) = &cli.file {
        let requested = PathBuf::from(file);
        if requested.is_dir() {
            if cli.panel_server || cli.snapshot.is_some() {
                anyhow::bail!("directory input is only available in the interactive TUI");
            }
            let browser = FileBrowser::open_directory(&requested)?;
            (browser.selected_path().to_path_buf(), Some(browser))
        } else {
            let browser = FileBrowser::alongside_file(&requested)?;
            (requested, Some(browser))
        }
    } else {
        eprintln!("Error: provide a file path or use --fetch <PDB_ID>");
        std::process::exit(1);
    };

    // Load protein structure (dispatch by file extension)
    let is_xyz = file_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xyz"));
    let protein_result = load_structure_file(&file_path);
    if let Some(file) = fetched_path {
        if let Err(error) = std::fs::remove_file(&file) {
            eprintln!(
                "Warning: failed to remove temporary fetched structure '{}': {error}",
                file
            );
        }
    }
    let protein = protein_result?;
    let residue_colors = resolve_residue_colors(&protein, &cli.residue_color)?;
    eprintln!(
        "Loaded: {} ({} chains, {} residues, {} atoms{})",
        protein.name,
        protein.chains.len(),
        protein.residue_count(),
        protein.atom_count(),
        if protein.ligands.is_empty() {
            String::new()
        } else {
            format!(", {} ligands", protein.ligand_count())
        },
    );

    // Open log file if requested
    let mut logfile: Option<std::fs::File> = match &cli.log {
        Some(path) => {
            let f = std::fs::File::create(path)
                .map_err(|e| anyhow::anyhow!("cannot create log file '{}': {}", path, e))?;
            Some(f)
        }
        None => None,
    };

    // Detect connection type
    let connection_type = ConnectionType::detect();
    log!(logfile, "connection type: {:?}", connection_type);

    // Determine render mode: an explicit flag first, then the config file, then
    // the built-in default.
    let render_mode = if let Some(mode_str) = &cli.render_mode {
        RenderMode::parse(mode_str).unwrap_or_else(|| {
            eprintln!("Warning: unknown render mode '{}', using default", mode_str);
            RenderMode::Braille
        })
    } else if cli.fullhd {
        // --fullhd / --pixel always forces FullHD regardless of SSH
        RenderMode::FullHD
    } else if cli.hd {
        // --hd is SSH-aware: FullHD locally, HalfBlock over SSH
        match connection_type {
            ConnectionType::Local => RenderMode::FullHD,
            ConnectionType::Ssh => RenderMode::HalfBlock,
        }
    } else {
        defaults.render.unwrap_or(RenderMode::Braille)
    };

    // Color scheme and visualization mode both resolve the same way: the flag if
    // it was given, else the config file, else the built-in default.  Whether
    // either was *chosen* -- by flag or by file -- is what the XYZ heuristic
    // below keys off, so a written-down preference is not silently overruled by
    // the file extension.
    let color_override = match &cli.color {
        Some(name) => Some(
            render::color::ColorSchemeType::parse(name).unwrap_or_else(|| {
                eprintln!("Warning: unknown color scheme '{name}', using structure");
                render::color::ColorSchemeType::Structure
            }),
        ),
        None => defaults.color,
    };
    let user_explicit_color = cli.color.is_some() || defaults.color.is_some();

    let user_explicit_mode = cli.mode.is_some() || defaults.mode.is_some();
    let viz_mode = match &cli.mode {
        Some(name) => VizMode::parse(name).unwrap_or_else(|| {
            eprintln!("Warning: unknown visualization mode '{name}', using cartoon");
            VizMode::Cartoon
        }),
        None => defaults.mode.unwrap_or(VizMode::Cartoon),
    };

    // XYZ files default to Element coloring + Wireframe mode unless overridden.
    let (color_override, viz_mode) = if is_xyz {
        let color = if user_explicit_color {
            color_override
        } else {
            Some(render::color::ColorSchemeType::Element)
        };
        let viz = if !user_explicit_mode {
            VizMode::Wireframe
        } else {
            viz_mode
        };
        (color, viz)
    } else {
        (color_override, viz_mode)
    };

    // The panel server owns a persistent, terminal-independent render session.
    // It must start before raw mode, terminal probing, alternate-screen setup,
    // or the crossterm input thread so its stdout remains strict NDJSON.
    if cli.panel_server {
        let output_path = cli
            .output
            .as_deref()
            .expect("clap enforces --output with --panel-server");
        let stdin = io::stdin();
        let stdout = io::stdout();
        panel_server::serve(
            protein,
            output_path,
            panel_server::PanelServerOptions {
                width: cli.panel_width,
                height: cli.panel_height,
                color_override,
                residue_colors: residue_colors.clone(),
                viz_mode,
                user_explicit_mode,
            },
            stdin.lock(),
            stdout.lock(),
        )?;
        return Ok(());
    }

    // A snapshot is the non-interactive form of ProteinView's FullHD pixel
    // renderer. Exit before raw mode, terminal probing, or alternate-screen
    // setup so it is safe to call from an agent tool running inside a TUI.
    if let Some(snapshot_path) = cli.snapshot.as_deref() {
        if cli.snapshot_interface_chain.is_some() && user_explicit_color {
            anyhow::bail!(
                "--snapshot-interface-chain uses ProteinView's interface palette and cannot be combined with --color"
            );
        }
        render::snapshot::save_png(
            protein,
            snapshot_path,
            render::snapshot::SnapshotOptions {
                width: cli.snapshot_width,
                height: cli.snapshot_height,
                color_override,
                residue_colors: residue_colors.clone(),
                viz_mode,
                user_explicit_mode,
                show_ligands: !cli.snapshot_hide_ligands,
                interface_chain: cli.snapshot_interface_chain.clone(),
                show_interactions: cli.snapshot_interactions,
            },
        )?;
        eprintln!(
            "Rendered FullHD PNG: {} ({}x{})",
            snapshot_path.display(),
            cli.snapshot_width,
            cli.snapshot_height
        );
        return Ok(());
    }

    // Get terminal dimensions before entering alternate screen
    let (term_cols, term_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    log!(logfile, "terminal size: {}x{}", term_cols, term_rows);

    // Install panic hook that restores the terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stderr(), LeaveAlternateScreen);
        render::kitty_shm::unlink_all();
        original_hook(info);
    }));

    // Setup terminal — must happen before Picker::from_query_stdio()
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Detect terminal graphics protocol (Sixel/Kitty/iTerm2) and font size.
    // Must be called after entering alternate screen but before spawning the
    // input thread (which reads from stdin).
    let picker = ratatui_image::picker::Picker::from_query_stdio()
        .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
    log!(
        logfile,
        "picker: protocol={:?} font_size={:?}",
        picker.protocol_type(),
        picker.font_size()
    );

    // Ask the terminal whether it can read pixels straight out of shared
    // memory.  Must happen here: after the picker's own query has drained its
    // responses, and before the input thread starts consuming stdin.  Over SSH
    // there is no shared filesystem to share memory through, so don't ask.
    let kitty_shm = picker.protocol_type() == ratatui_image::picker::ProtocolType::Kitty
        && connection_type == ConnectionType::Local
        && render::kitty_shm::probe(Duration::from_millis(500));
    log!(logfile, "kitty shared-memory transport: {}", kitty_shm);

    // Create the app against the actual 3D viewport, excluding a browser that
    // starts open for directory input.
    let initial_viewport_cols = viewport_width(term_cols, browser.as_ref(), false);
    let mut app = App::new(
        protein,
        AppConfig {
            render_mode,
            viz_mode,
            user_explicit_mode,
            color_override,
            residue_colors,
        },
        initial_viewport_cols,
        term_rows,
        picker,
    );
    app.kitty_shm = kitty_shm;
    log!(
        logfile,
        "app created: render_mode={:?} chains={} zoom={:.2}",
        app.render_mode,
        app.protein.chains.len(),
        app.camera.zoom
    );

    // Spawn dedicated input thread — decouples input from rendering so
    // quit always works even when HD rendering is slow
    let (input_rx, quit_flag) = event::spawn_input_thread();

    // Main loop
    let tick_rate = Duration::from_millis(33); // ~30 FPS
    let mut frame_count: u64 = 0;
    // Track how long the previous terminal.draw() took so we can skip frames
    // when rendering is too slow (prevents PTY buffer saturation & freezes).
    let mut last_draw_duration = Duration::ZERO;
    let mut frames_to_skip: u32 = 0;
    // Tracks the interaction state of the previous drawn frame, so the
    // transition back to a still view can trigger one full-resolution redraw.
    let mut was_interacting = false;

    loop {
        // Cursor movement is expressed in layout rows, so the layout has to be
        // current before keys are handled.
        sync_sequence_viewport(&mut app, browser.as_ref(), (term_cols, term_rows));

        // Drain all queued input from the dedicated input thread
        let mut had_input = false;
        while let Ok(app_event) = input_rx.try_recv() {
            had_input = true;
            match app_event {
                event::AppEvent::Resize(cols, rows) => {
                    log!(logfile, "resize: {}x{}", cols, rows);
                    let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                    app.recalculate_zoom(cols, rows);
                    app.mesh_dirty_flag();
                }
                event::AppEvent::Key(key) => {
                    log!(logfile, "key: {:?}", key.code);
                    let ctrl_c = key.code == KeyCode::Char('c')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL);
                    if key.code == KeyCode::Char('q') || ctrl_c {
                        app.should_quit = true;
                        continue;
                    }

                    if key.code == KeyCode::Char('?') {
                        app.show_help = !app.show_help;
                        continue;
                    }
                    if key.code == KeyCode::Esc && app.show_help {
                        app.show_help = false;
                        continue;
                    }

                    if key.code == KeyCode::Char('e') {
                        if let Some(file_browser) = browser.as_mut() {
                            file_browser.toggle();
                            let (cols, rows) =
                                crossterm::terminal::size().unwrap_or((term_cols, term_rows));
                            let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                            app.recalculate_zoom(cols, rows);
                            app.needs_clear = true;
                        }
                        continue;
                    }

                    if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
                        if let Some(file_browser) = browser.as_mut() {
                            file_browser.toggle_focus();
                        }
                        continue;
                    }

                    if browser.as_ref().is_some_and(|browser| browser.focused) {
                        let file_browser = browser.as_mut().expect("browser focus implies browser");
                        match key.code {
                            KeyCode::Char('j') | KeyCode::Down => file_browser.move_selection(1),
                            KeyCode::Char('k') | KeyCode::Up => file_browser.move_selection(-1),
                            KeyCode::PageDown => {
                                file_browser.page(1, usize::from(term_rows.saturating_sub(4)))
                            }
                            KeyCode::PageUp => {
                                file_browser.page(-1, usize::from(term_rows.saturating_sub(4)))
                            }
                            KeyCode::Home => file_browser.select_first(),
                            KeyCode::End => file_browser.select_last(),
                            KeyCode::Enter => {
                                let selected = file_browser.selected_path().to_path_buf();
                                if selected == file_browser.current {
                                    file_browser.focused = false;
                                    continue;
                                }
                                match load_structure_file(&selected).and_then(|protein| {
                                    let residue_colors =
                                        resolve_residue_colors(&protein, &cli.residue_color)?;
                                    Ok((protein, residue_colors))
                                }) {
                                    Ok((protein, residue_colors)) => {
                                        let (cols, rows) = crossterm::terminal::size()
                                            .unwrap_or((term_cols, term_rows));
                                        let cols = cols
                                            .saturating_sub(browser::panel_width(
                                                cols,
                                                file_browser.visible,
                                            ))
                                            .saturating_sub(if app.show_interface {
                                                ui::interface_panel::SIDEBAR_WIDTH
                                            } else {
                                                0
                                            })
                                            .max(1);
                                        app.replace_protein(protein, residue_colors, cols, rows);
                                        file_browser.mark_loaded(&selected);
                                        file_browser.focused = false;
                                        frames_to_skip = 0;
                                        was_interacting = false;
                                        log!(
                                            logfile,
                                            "loaded from browser: {}",
                                            selected.display()
                                        );
                                    }
                                    Err(error) => file_browser.set_error(format!("{error:#}")),
                                }
                            }
                            KeyCode::Esc => file_browser.focused = false,
                            _ => {}
                        }
                        continue;
                    }

                    // While the panel is open it owns the arrow keys and the
                    // selection keys; h/j/k/l keep driving the camera, so the
                    // view stays steerable with the sequence in front of you.
                    if app.show_sequence && handle_sequence_key(&mut app, key) {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') => app.should_quit = true,
                        KeyCode::Char('c')
                            if key
                                .modifiers
                                .contains(crossterm::event::KeyModifiers::CONTROL) =>
                        {
                            app.should_quit = true
                        }
                        // Every camera-moving key notes an interaction, which
                        // drops FullHD to its reduced resolution until the view
                        // settles again.
                        KeyCode::Char('h') | KeyCode::Left => {
                            app.camera.rotate_y(-1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('l') | KeyCode::Right => {
                            app.camera.rotate_y(1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            app.camera.rotate_x(1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            app.camera.rotate_x(-1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('u') => {
                            app.camera.rotate_z(-1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('i') => {
                            app.camera.rotate_z(1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('+') | KeyCode::Char('=') => {
                            app.camera.zoom_in();
                            app.note_camera_change();
                        }
                        KeyCode::Char('-') => {
                            app.camera.zoom_out();
                            app.note_camera_change();
                        }
                        KeyCode::Char('w') => {
                            app.camera.pan(0.0, 1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('s') => {
                            app.camera.pan(0.0, -1.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('a') => {
                            app.camera.pan(-1.0, 0.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('d') => {
                            app.camera.pan(1.0, 0.0);
                            app.note_camera_change();
                        }
                        KeyCode::Char('r') => {
                            let (cols, rows) =
                                crossterm::terminal::size().unwrap_or((term_cols, term_rows));
                            let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                            app.camera.reset();
                            app.recalculate_zoom(cols, rows);
                            app.note_camera_change();
                        }
                        KeyCode::Char('c') => app.cycle_color(),
                        // Named palettes from the config file.  Silent when the
                        // file defines none, since there is nothing to show.
                        KeyCode::Char('p') => {
                            app.cycle_palette(true);
                        }
                        KeyCode::Char('P') => {
                            app.cycle_palette(false);
                        }
                        KeyCode::Char('v') => app.cycle_viz_mode(),
                        KeyCode::Char('m') => {
                            let (cols, rows) =
                                crossterm::terminal::size().unwrap_or((term_cols, term_rows));
                            let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                            app.toggle_hd(cols, rows);
                        }
                        KeyCode::Char('M') => {
                            let (cols, rows) =
                                crossterm::terminal::size().unwrap_or((term_cols, term_rows));
                            let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                            app.toggle_fullhd(cols, rows);
                        }
                        KeyCode::Char('[') => app.prev_chain(),
                        KeyCode::Char(']') => app.next_chain(),
                        KeyCode::Char(' ') => app.camera.auto_rotate = !app.camera.auto_rotate,
                        KeyCode::Char('f') => {
                            app.toggle_interface();
                            let (cols, rows) =
                                crossterm::terminal::size().unwrap_or((term_cols, term_rows));
                            let cols = viewport_width(cols, browser.as_ref(), app.show_interface);
                            app.recalculate_zoom(cols, rows);
                            app.needs_clear = true;
                        }
                        KeyCode::Char('I') => app.toggle_interactions(),
                        KeyCode::Char('g') => app.toggle_ligands(),
                        KeyCode::Char('S') => app.toggle_sequence_panel(),
                        KeyCode::Char('b') => app.toggle_ball_stick(),
                        KeyCode::Char('z') => {
                            app.focus_on_selection();
                        }
                        KeyCode::Esc => {
                            if app.show_help {
                                app.show_help = false;
                            } else if app.show_sequence {
                                app.show_sequence = false;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }

        // Ensure ribbon mesh cache is fresh (rebuilds only when color scheme changes).
        // Must happen outside terminal.draw() since ribbon_mesh() needs &mut self.
        // Only rebuild when in Cartoon mode — Backbone/Wireframe don't use the
        // ribbon mesh, so skipping this preserves the lazy-mesh optimization for
        // large structures that start in a non-Cartoon mode.
        let mesh_was_rebuilt = app.viz_mode == VizMode::Cartoon && app.mesh_is_dirty();
        if app.viz_mode == VizMode::Cartoon {
            app.ribbon_mesh();
        }

        // Always poll the background interface thread, even during skipped
        // frames, so the result is absorbed as soon as it's available.
        let interface_was_pending = app.interface_pending();
        app.poll_background_interface();
        let interface_absorbed = interface_was_pending && !app.interface_pending();

        // Adaptive frame skipping: if the previous draw took longer than the
        // tick rate, skip frames proportionally.  User input always forces a
        // redraw so the UI stays responsive.
        //
        // Do NOT call app.tick() during skipped frames — that would advance
        // auto-rotate without a corresponding render, causing the protein to
        // "jump" when rendering resumes.  Instead we just sleep and let the
        // camera's dt-clamping handle the gap.
        if frames_to_skip > 0 && !had_input {
            frames_to_skip -= 1;
            // Reset the camera's tick timer so the next real tick doesn't see
            // a huge accumulated dt from the skipped frames.
            app.camera.reset_tick_timer();
            std::thread::sleep(tick_rate);
            continue;
        }

        // Nothing on screen changes unless input arrived, an animation is
        // running, or background state was just absorbed.  Redrawing anyway
        // would re-run the whole rasterize + transmit pipeline at every tick,
        // forever, for an image identical to the one already on screen.
        //
        // The one extra case is `settled`: FullHD renders at a reduced
        // resolution while the camera moves, so the frame after it comes to
        // rest must be drawn to replace it with the sharp one.
        let interacting = app.is_interacting();
        let settled = was_interacting && !interacting;
        was_interacting = interacting;

        let must_redraw = had_input
            || interacting
            || settled
            || app.ssh_hd_warning
            || app.needs_clear
            || mesh_was_rebuilt
            || interface_absorbed
            || frame_count < 2;
        if !must_redraw {
            app.tick();
            std::thread::sleep(tick_rate);
            continue;
        }

        // Render
        frame_count += 1;
        if frame_count <= 3 || frame_count % 300 == 0 {
            log!(
                logfile,
                "frame {} render start (render_mode={:?} viz={:?} interface={} last_draw={:?})",
                frame_count,
                app.render_mode,
                app.viz_mode,
                app.show_interface,
                last_draw_duration
            );
        }

        // After a render-mode switch, force ratatui to redraw every cell.
        // Without this, its diff-based rendering may leave stale characters
        // from the previous mode (e.g. braille dots under a FullHD image).
        if app.needs_clear {
            // Delete any Kitty graphics images that may be lingering from
            // a previous FullHD session.  Harmless no-op if there are none.
            let cleanup = render::kitty_png::KittyPngImage::cleanup_escape();
            execute!(terminal.backend_mut(), crossterm::style::Print(&cleanup))?;
            // Drop any shared memory object the terminal never got round to
            // reading, so switching modes cannot leave objects behind.
            render::kitty_shm::unlink_all();
            terminal.clear()?;
            app.needs_clear = false;
        }

        // ...and again after input, because the key that opened the panel or
        // resized it arrived after the first sync: the frame about to be drawn
        // must not be the one frame with a stale layout.
        sync_sequence_viewport(&mut app, browser.as_ref(), (term_cols, term_rows));

        let draw_start = Instant::now();
        terminal.draw(|frame| {
            let file_panel_width = browser.as_ref().map_or(0, |file_browser| {
                browser::panel_width(frame.area().width, file_browser.visible)
            });
            let protein_area = if file_panel_width > 0 {
                let horizontal = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Length(file_panel_width), Constraint::Min(20)])
                    .split(frame.area());
                ui::file_browser::render_file_browser(
                    frame,
                    horizontal[0],
                    browser.as_ref().expect("visible panel implies browser"),
                );
                horizontal[1]
            } else {
                frame.area()
            };

            // If interface is active, split horizontally: sidebar | main
            let main_area = if app.show_interface {
                let horiz = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(ui::interface_panel::SIDEBAR_WIDTH),
                        Constraint::Min(20),
                    ])
                    .split(protein_area);

                let summary = app.interface_analysis.summary(&app.protein);
                let chain_names = app.chain_names();
                let interaction_counts = app.interface_analysis.interaction_counts();
                ui::interface_panel::render_interface_panel(
                    frame,
                    horiz[0],
                    &summary,
                    app.current_chain,
                    &chain_names,
                    app.show_interactions,
                    interaction_counts,
                );
                horiz[1]
            } else {
                protein_area
            };

            let sequence_height = ui::sequence_panel::height_for(&app, main_area.height);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),               // Header
                    Constraint::Min(3),                  // Viewport
                    Constraint::Length(sequence_height), // Sequence panel
                    Constraint::Length(1),               // Mode and key hints
                ])
                .split(main_area);

            ui::header::render_header(frame, chunks[0], &app.protein.name);
            ui::viewport::render_viewport(frame, chunks[1], &app);
            if sequence_height > 0 {
                ui::sequence_panel::render_sequence_panel(frame, chunks[2], &app);
            }
            ui::statusbar::render_statusbar(frame, chunks[3], browser.as_ref());

            if app.show_help {
                ui::help_overlay::render_help_overlay(frame, frame.area());
            }
        })?;
        last_draw_duration = draw_start.elapsed();

        // If the draw took longer than two tick periods, skip some frames to
        // let the terminal catch up and avoid saturating the PTY write buffer.
        if last_draw_duration > tick_rate * 2 {
            // Skip 1-3 frames depending on how slow the draw was.
            frames_to_skip = ((last_draw_duration.as_millis() / tick_rate.as_millis()) as u32)
                .saturating_sub(1)
                .min(3);
        }

        app.tick();

        // Sleep for the remainder of the tick period to cap at ~30 FPS.
        // Account for the time already spent drawing so the frame rate stays
        // consistent regardless of render cost.
        let elapsed = draw_start.elapsed();
        if let Some(remaining) = tick_rate.checked_sub(elapsed) {
            std::thread::sleep(remaining);
        }
    }

    // Signal input thread to stop
    quit_flag.store(true, Ordering::Relaxed);

    // Restore terminal
    render::kitty_shm::unlink_all();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
