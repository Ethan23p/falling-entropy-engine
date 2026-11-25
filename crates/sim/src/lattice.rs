//! The lattice: the hexagonal structure cells are positioned relative to. A column-offset layout,
//! `y` growing downward, odd columns half a cell lower than even ones.

/// The six neighbor directions of a column-offset hexagonal lattice, clockwise from up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Dir {
    Up = 0,
    UpRight = 1,
    DownRight = 2,
    Down = 3,
    DownLeft = 4,
    UpLeft = 5,
}

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Up, Dir::UpRight, Dir::DownRight, Dir::Down, Dir::DownLeft, Dir::UpLeft];

    pub fn opposite(self) -> Dir {
        Dir::ALL[(self as usize + 3) % 6]
    }

    /// The step toward this neighbor, in half cells, `y` growing downward: what velocity is
    /// measured against. Up and down are 2 long; the diagonals 2 across and 1 up or down.
    pub fn step(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -2),
            Dir::UpRight => (2, -1),
            Dir::DownRight => (2, 1),
            Dir::Down => (0, 2),
            Dir::DownLeft => (-2, 1),
            Dir::UpLeft => (-2, -1),
        }
    }
}

/// The neighbor of `(x, y)` toward `d`. Odd columns sit half a cell lower than even ones,
/// and `y` grows downward.
pub fn neighbor(x: i32, y: i32, d: Dir) -> (i32, i32) {
    let odd = x & 1;
    match d {
        Dir::Up => (x, y - 1),
        Dir::Down => (x, y + 1),
        Dir::UpRight => (x + 1, y - 1 + odd),
        Dir::DownRight => (x + 1, y + odd),
        Dir::DownLeft => (x - 1, y + odd),
        Dir::UpLeft => (x - 1, y - 1 + odd),
    }
}
