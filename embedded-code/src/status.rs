//! Application state module.
//!
//! Tracks the GPS step counter state including:
//! - Current position (x, y) in meters
//! - Total steps taken
//! - Current view page
use comm_protocol::ViewPage;

/// Screen dimensions
pub const SCREEN_WIDTH: i16 = 128;
pub const SCREEN_HEIGHT: i16 = 80;

/// Starting position (center of screen)
pub const START_X: i16 = SCREEN_WIDTH / 2;
pub const START_Y: i16 = SCREEN_HEIGHT / 2;

/// Number of positions the path history can hold
pub const MAX_PATH_POINTS: usize = 512;

/// Application status
#[derive(Debug, Clone)]
pub struct Status {
    /// Current X position in meters (also used as pixel coordinate)
    pub x: i16,
    /// Current Y position in meters (also used as pixel coordinate)
    pub y: i16,
    /// Total number of steps taken
    pub steps: u32,
    /// Current view page
    pub current_view: ViewPage,
    /// Path history for map view (stores visited positions)
    /// Using a simple array with fixed size to avoid heap allocation
    pub path: PathHistory,
}

impl Status {
    /// Reset state to initial values
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Record a step that moves by (dx, dy), clamped to the screen
    pub fn record_step(&mut self, dx: i16, dy: i16) {
        self.steps = self.steps.saturating_add(1);
        self.x = self.x.saturating_add(dx).clamp(0, SCREEN_WIDTH - 1);
        self.y = self.y.saturating_add(dy).clamp(0, SCREEN_HEIGHT - 1);
        self.path.add(self.x, self.y);
    }

    /// Switch to a different view
    pub fn switch_view(&mut self, page: ViewPage) {
        self.current_view = page;
    }
}

impl Default for Status {
    /// Start in the middle of the screen, which is also the first point of the path
    fn default() -> Self {
        let mut path = PathHistory::new();
        path.add(START_X, START_Y);
        Status {
            x: START_X,
            y: START_Y,
            steps: 0,
            current_view: ViewPage::Map,
            path,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PathHistory {
    points: [(i16, i16); MAX_PATH_POINTS],
    len: usize,
}

impl PathHistory {
    pub const fn new() -> Self {
        PathHistory {
            points: [(0, 0); MAX_PATH_POINTS],
            len: 0,
        }
    }

    /// Add a point to the path
    pub fn add(&mut self, x: i16, y: i16) {
        if self.len < MAX_PATH_POINTS {
            self.points[self.len] = (x, y);
            self.len += 1;
        }
        // If buffer is full, we just stop recording (no crash)
    }

    /// Iterate over recorded points
    pub fn iter(&self) -> impl Iterator<Item = &(i16, i16)> {
        self.points[..self.len].iter()
    }

    // /// Get number of recorded points
    // pub fn len(&self) -> usize {
    //     self.len
    // }

    // /// Check if empty
    // pub fn is_empty(&self) -> bool {
    //     self.len == 0
    // }
}
