/// Main entry point for the Hyprland recursive grid navigator.
///
/// This orchestrates the grid navigation logic with Wayland layer-shell integration.
mod backend;
mod config;
mod core;
mod geometry;
mod overlay;

use anyhow::{Context, Result};
use backend::{
    action_keys_from_config, apply_keybindings, build_cell_maps, init_wayland, keymap_from_env,
    run_event_loop, OverlayEvent,
};
use config::load_config;
use core::GridState;
use geometry::Rect;
use std::io;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    tracing::info!("Hyprland Recursive Grid Navigator starting");

    // Get screen dimensions
    let mut screen_rect = get_screen_rect().context("Failed to detect screen dimensions")?;

    tracing::info!(
        "Screen dimensions: {} x {}",
        screen_rect.width,
        screen_rect.height
    );

    // Initialize Wayland backend
    let cfg = load_config();
    let (rows, cols) = read_grid_size(&cfg);
    let (cell_keysyms, cell_keycodes, cell_labels) = build_cell_maps(rows, cols, &cfg);
    let keymap = keymap_from_env(apply_keybindings(Default::default(), &cfg));
    let action_keys = action_keys_from_config(&cfg);
    let (_conn, mut event_queue, mut app_state) = init_wayland(
        screen_rect.width as u32,
        screen_rect.height as u32,
        keymap,
        action_keys,
        cell_keysyms,
        cell_keycodes,
    )
    .context("Failed to initialize Wayland backend")?;

    let qh = event_queue.handle();

    // Wait for the layer surface to be configured
    tracing::info!("Waiting for layer surface configuration...");
    while !app_state.is_configured() {
        run_event_loop(&mut event_queue, &mut app_state, Some(100))?;
    }
    tracing::info!("Layer surface configured successfully");

    if app_state.width > 0 && app_state.height > 0 {
        screen_rect = Rect::new(0, 0, app_state.width as i32, app_state.height as i32);
    }

    // Create the grid state
    let mut grid = GridState::new_with_grid(screen_rect, rows, cols);
    let grid_color = read_color_from_config(&cfg);
    let mut overlay_state =
        overlay::OverlayState::new(screen_rect, rows, cols, cell_labels, grid_color);

    tracing::debug!("Grid initialized at root level");

    // Main navigation loop
    loop {
        // Update and render the overlay
        overlay_state.update(grid.current_rect());
        overlay_state.render(&mut app_state, &qh)?;

        // Poll for input events
        let events = run_event_loop(&mut event_queue, &mut app_state, None)?;

        // Process events
        for event in events {
            match event {
                OverlayEvent::SelectCell(cell) => {
                    grid.descend(cell);
                    tracing::debug!("Descended to {:?}", cell);
                }
                OverlayEvent::Ascend => {
                    if grid.ascend() {
                        tracing::debug!("Ascended to parent level");
                    } else {
                        tracing::debug!("Already at root level");
                    }
                }
                OverlayEvent::Cancel => {
                    tracing::info!("Navigation cancelled");
                    return Ok(());
                }
                OverlayEvent::Confirm => {
                    let (cx, cy) = grid.current_center();
                    println!("{} {}", cx, cy);
                    return Ok(());
                }
            }
        }

        // Small delay to prevent busy looping
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Detect the screen rectangle from the Wayland environment.
///
/// For MVP: Returns hardcoded dimensions if detection fails.
/// In a production version, this would query the Wayland output protocol.
fn get_screen_rect() -> Result<Rect> {
    // Try to detect from common environment variables or hardcode
    // For now, use a sensible default (1920x1080)
    // TODO: Implement proper Wayland output detection

    let width = std::env::var("HYPRLAND_RES")
        .ok()
        .and_then(|s| {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                parts[0].parse::<i32>().ok()
            } else {
                None
            }
        })
        .unwrap_or(1920);

    let height = std::env::var("HYPRLAND_RES")
        .ok()
        .and_then(|s| {
            let parts: Vec<&str> = s.split('x').collect();
            if parts.len() == 2 {
                parts[1].parse::<i32>().ok()
            } else {
                None
            }
        })
        .unwrap_or(1080);

    tracing::debug!("Detected screen resolution: {}x{}", width, height);
    Ok(Rect::new(0, 0, width, height))
}

fn read_grid_size(cfg: &config::Config) -> (usize, usize) {
    let mut rows = 2usize;
    let mut cols = 2usize;
    if let Some(gs) = &cfg.grid_size {
        rows = gs.rows.clamp(1, 10);
        cols = gs.cols.clamp(1, 10);
    }
    if let Ok(val) = std::env::var("HYPRRGN_GRID") {
        let parts: Vec<&str> = val.split('x').collect();
        if parts.len() == 2 {
            if let (Ok(r), Ok(c)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                rows = r.clamp(1, 10);
                cols = c.clamp(1, 10);
            }
        }
    }
    (rows, cols)
}

fn read_color_from_config(cfg: &config::Config) -> (f64, f64, f64) {
    if let Some(c) = &cfg.grid_color {
        let r = c.r.clamp(0.0, 1.0) as f64;
        let g = c.g.clamp(0.0, 1.0) as f64;
        let b = c.b.clamp(0.0, 1.0) as f64;
        return (r, g, b);
    }
    (1.0, 1.0, 1.0)
}
