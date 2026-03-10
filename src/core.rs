/// Core grid navigation state machine.
/// Pure logic with no Wayland dependencies - fully testable.
use crate::geometry::{CellIndex, Rect};
use std::collections::VecDeque;

/// The state of the grid navigator.
///
/// Tracks the root rectangle, current rectangle, and navigation history.
/// All operations are pure and return new state without mutation.
#[derive(Clone, Debug)]
pub struct GridState {
    /// The current active rectangle.
    current: Rect,
    /// History stack for backtracking (stores rectangles at each level).
    history: VecDeque<Rect>,
    /// Grid rows and columns.
    rows: usize,
    cols: usize,
}

impl GridState {
    /// Create a new grid state with configurable grid size.
    pub fn new_with_grid(root: Rect, rows: usize, cols: usize) -> Self {
        Self {
            current: root,
            history: VecDeque::new(),
            rows: rows.clamp(1, 10),
            cols: cols.clamp(1, 10),
        }
    }

    /// Get the current rectangle.
    pub fn current_rect(&self) -> Rect {
        self.current
    }

    /// Get the center of the current rectangle.
    pub fn current_center(&self) -> (i32, i32) {
        self.current.center()
    }

    /// Descend into a quadrant.
    ///
    /// This moves to the specified cell in the current subdivision
    /// and pushes the previous rectangle onto the history stack.
    pub fn descend(&mut self, cell: CellIndex) {
        // Save current position on history stack
        self.history.push_back(self.current);

        // Subdivide and select the appropriate quadrant
        self.current = self
            .current
            .cell_rect(self.rows, self.cols, cell.row, cell.col);
    }

    /// Ascend one level (pop from history).
    ///
    /// Returns true if ascent was successful, false if already at root.
    pub fn ascend(&mut self) -> bool {
        if let Some(prev) = self.history.pop_back() {
            self.current = prev;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let root = Rect::new(0, 0, 1920, 1080);
        let state = GridState::new_with_grid(root, 2, 2);
        assert_eq!(state.current_rect(), root);
    }

    #[test]
    fn test_descend_and_center() {
        let root = Rect::new(0, 0, 1000, 1000);
        let mut state = GridState::new_with_grid(root, 2, 2);

        state.descend(CellIndex { row: 1, col: 1 }); // bottom-right
        assert_eq!(state.current_rect(), Rect::new(500, 500, 500, 500));
        assert_eq!(state.current_center(), (750, 750));
    }

    #[test]
    fn test_ascend() {
        let root = Rect::new(0, 0, 1000, 1000);
        let mut state = GridState::new_with_grid(root, 2, 2);

        state.descend(CellIndex { row: 0, col: 0 });

        let ascended = state.ascend();
        assert!(ascended);
        assert_eq!(state.current_rect(), root);
    }

    #[test]
    fn test_cannot_ascend_at_root() {
        let root = Rect::new(0, 0, 1000, 1000);
        let mut state = GridState::new_with_grid(root, 2, 2);

        let ascended = state.ascend();
        assert!(!ascended);
    }

    #[test]
    fn test_multi_level_navigation() {
        let root = Rect::new(0, 0, 1000, 1000);
        let mut state = GridState::new_with_grid(root, 2, 2);

        state.descend(CellIndex { row: 1, col: 1 }); // bottom-right: (500, 500, 500, 500)
        state.descend(CellIndex { row: 0, col: 0 }); // top-left of that: (500, 500, 250, 250)

        assert_eq!(state.current_rect(), Rect::new(500, 500, 250, 250));

        state.ascend();
        assert_eq!(state.current_rect(), Rect::new(500, 500, 500, 500));
    }

    #[test]
    fn test_subdivisions() {
        let root = Rect::new(0, 0, 100, 100);
        let state = GridState::new_with_grid(root, 2, 2);

        let br = state.current.cell_rect(2, 2, 1, 1);
        assert_eq!(br, Rect::new(50, 50, 50, 50));
    }
}
