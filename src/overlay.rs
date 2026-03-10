/// Overlay rendering module.
///
/// Handles displaying the recursive grid to a Wayland layer-shell surface using Cairo.
use crate::backend::AppState;
use crate::geometry::{split_lengths, Rect};
use anyhow::Result;
use cairo::{Context, Format, ImageSurface};
use wayland_client::QueueHandle;

/// Represents the visual state of the overlay.
#[derive(Clone, Debug)]
pub struct OverlayState {
    /// The root rectangle (screen bounds).
    pub root: Rect,
    /// Current target rectangle.
    pub current: Rect,
    /// Grid size.
    pub rows: usize,
    pub cols: usize,
    /// Labels for cells (row-major).
    pub labels: Vec<String>,
    /// Grid color (RGB).
    pub grid_color: (f64, f64, f64),
}

impl OverlayState {
    /// Create a new overlay state.
    pub fn new(
        root: Rect,
        rows: usize,
        cols: usize,
        labels: Vec<String>,
        grid_color: (f64, f64, f64),
    ) -> Self {
        Self {
            root,
            current: root,
            rows,
            cols,
            labels,
            grid_color,
        }
    }

    /// Update the overlay state based on navigation.
    pub fn update(&mut self, current: Rect) {
        self.current = current;
    }

    /// Render the overlay to the Wayland surface.
    pub fn render(&self, app_state: &mut AppState, qh: &QueueHandle<AppState>) -> Result<()> {
        if !app_state.is_configured() {
            return Ok(());
        }

        let width = app_state.width;
        let height = app_state.height;

        let wl_surface = app_state.surface.clone();
        let buffer = app_state.get_buffer(width, height, qh)?;
        if buffer.busy {
            return Ok(());
        }

        // Clear buffer to transparent to avoid ghosting from previous frames
        buffer.mmap.fill(0);

        // Create a Cairo surface over the SHM buffer
        let surface = unsafe {
            ImageSurface::create_for_data_unsafe(
                buffer.mmap.as_mut_ptr(),
                Format::ARgb32,
                width as i32,
                height as i32,
                buffer.stride as i32,
            )?
        };
        let cr = Context::new(&surface)?;

        // Ensure we start from a clean state
        cr.set_operator(cairo::Operator::Over);

        // Draw the grid overlay
        self.draw_grid(&cr, width as f64, height as f64)?;

        // Draw labels for the cells
        self.draw_labels(&cr, width as f64, height as f64)?;

        surface.flush();

        if let Some(wl_surface) = wl_surface {
            wl_surface.attach(Some(&buffer.buffer), 0, 0);
            wl_surface.damage(0, 0, width as i32, height as i32);
            wl_surface.commit();
            buffer.busy = true;
        }

        Ok(())
    }

    /// Draw the recursive grid lines.
    fn draw_grid(&self, cr: &Context, width: f64, height: f64) -> Result<()> {
        // Set up grid line style
        cr.set_source_rgba(self.grid_color.0, self.grid_color.1, self.grid_color.2, 0.8);
        cr.set_line_width(2.0);

        // Convert current rect to overlay coordinates
        let scale_x = width / self.root.width as f64;
        let scale_y = height / self.root.height as f64;

        let x = self.current.x as f64 * scale_x;
        let y = self.current.y as f64 * scale_y;
        let w = self.current.width as f64 * scale_x;
        let h = self.current.height as f64 * scale_y;

        // Outline current rect
        cr.rectangle(x, y, w, h);
        cr.stroke()?;

        // Draw grid lines within current rect
        let row_heights = split_lengths(self.current.height, self.rows);
        let col_widths = split_lengths(self.current.width, self.cols);

        let mut x_cursor = self.current.x;
        for c in 0..(self.cols.saturating_sub(1)) {
            x_cursor += col_widths[c];
            let lx = x_cursor as f64 * scale_x;
            cr.move_to(lx, y);
            cr.line_to(lx, y + h);
        }

        let mut y_cursor = self.current.y;
        for r in 0..(self.rows.saturating_sub(1)) {
            y_cursor += row_heights[r];
            let ly = y_cursor as f64 * scale_y;
            cr.move_to(x, ly);
            cr.line_to(x + w, ly);
        }
        cr.stroke()?;

        Ok(())
    }

    /// Draw labels for the grid cells.
    fn draw_labels(&self, cr: &Context, width: f64, height: f64) -> Result<()> {
        // Label positions (quarters of the current rect)
        let scale_x = width / self.root.width as f64;
        let scale_y = height / self.root.height as f64;

        let w = self.current.width as f64 * scale_x;
        let h = self.current.height as f64 * scale_y;

        let min_dim = w.min(h);
        let font_size = (min_dim * 0.12).clamp(10.0, 48.0);

        cr.set_source_rgba(self.grid_color.0, self.grid_color.1, self.grid_color.2, 0.9);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(font_size);

        let row_heights = split_lengths(self.current.height, self.rows);
        let col_widths = split_lengths(self.current.width, self.cols);

        for r in 0..self.rows {
            for c in 0..self.cols {
                let idx = r * self.cols + c;
                let label = self.labels.get(idx).map(|s| s.as_str()).unwrap_or("");
                if label.is_empty() {
                    continue;
                }
                let cell_x = self.current.x + col_widths.iter().take(c).sum::<i32>();
                let cell_y = self.current.y + row_heights.iter().take(r).sum::<i32>();
                let cell_w = col_widths[c];
                let cell_h = row_heights[r];

                let cx = (cell_x as f64 + cell_w as f64 / 2.0) * scale_x;
                let cy = (cell_y as f64 + cell_h as f64 / 2.0) * scale_y;

                let extents = cr.text_extents(label)?;
                cr.set_source_rgba(self.grid_color.0, self.grid_color.1, self.grid_color.2, 0.9);
                cr.move_to(cx - extents.width() / 2.0, cy + extents.height() / 2.0);
                cr.show_text(label)?;
            }
        }

        // Draw instructions at the bottom
        cr.set_font_size(16.0);
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.8);
        let instructions = "Backspace: Up • Enter: Confirm • Esc: Cancel";
        let extents = cr.text_extents(instructions)?;

        let instr_x = width / 2.0;
        let instr_y = height - 30.0;

        // Background for instructions
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
        cr.rectangle(
            instr_x - extents.width() / 2.0 - 10.0,
            instr_y - extents.height() - 5.0,
            extents.width() + 20.0,
            extents.height() + 10.0,
        );
        cr.fill()?;

        // Instructions text
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        cr.move_to(instr_x - extents.width() / 2.0, instr_y);
        cr.show_text(instructions)?;

        Ok(())
    }
}
