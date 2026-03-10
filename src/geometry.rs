/// Pure geometry module for recursive grid navigation.
/// All types here are immutable and testable with no dependencies on Wayland.
use std::fmt;

/// A rectangle defined by integer coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// Create a new rectangle.
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Get the center point of this rectangle.
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.width / 2, self.y + self.height / 2)
    }

    /// Get a sub-rectangle by row/col in a grid.
    pub fn cell_rect(&self, rows: usize, cols: usize, row: usize, col: usize) -> Rect {
        let rows = rows.max(1);
        let cols = cols.max(1);
        let row = row.min(rows - 1);
        let col = col.min(cols - 1);

        let row_heights = split_lengths(self.height, rows);
        let col_widths = split_lengths(self.width, cols);

        let y = self.y + row_heights.iter().take(row).sum::<i32>();
        let x = self.x + col_widths.iter().take(col).sum::<i32>();
        Rect::new(x, y, col_widths[col], row_heights[row])
    }
}

impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Rect(x={}, y={}, w={}, h={})",
            self.x, self.y, self.width, self.height
        )
    }
}

/// Represents a grid cell selection (row/col).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellIndex {
    pub row: usize,
    pub col: usize,
}

impl fmt::Display for CellIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cell({}, {})", self.row, self.col)
    }
}

/// Split a length into `parts` segments with remainders distributed to the front.
pub fn split_lengths(total: i32, parts: usize) -> Vec<i32> {
    let parts = parts.max(1);
    let base = total / parts as i32;
    let rem = total % parts as i32;
    let mut v = Vec::with_capacity(parts);
    for i in 0..parts {
        let extra = if (i as i32) < rem { 1 } else { 0 };
        v.push(base + extra);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_center() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(r.center(), (50, 50));

        let r2 = Rect::new(10, 20, 50, 60);
        assert_eq!(r2.center(), (35, 50));
    }

    #[test]
    fn test_cell_rect() {
        let r = Rect::new(0, 0, 100, 100);
        let br = r.cell_rect(2, 2, 1, 1);
        assert_eq!(br, Rect::new(50, 50, 50, 50));
    }
}
