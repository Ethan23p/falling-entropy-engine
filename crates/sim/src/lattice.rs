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

/// A cell's axial coordinates `(q, r)`: `q` is its column and `r` leans with the lattice, so each of
/// the six neighbors is a unit step: up `(0, -1)`, up-right `(1, -1)`, down-right `(1, 0)`, down
/// `(0, 1)`, down-left `(-1, 1)`, up-left `(-1, 0)`. Shapes on the lattice are built in these.
pub fn to_axial(x: i32, y: i32) -> (i32, i32) {
    (x, y - (x - (x & 1)) / 2)
}

/// The cell at axial `(q, r)`, as `(x, y)`.
pub fn from_axial(q: i32, r: i32) -> (i32, i32) {
    (q, r + (q - (q & 1)) / 2)
}

/// Steps between two cells, in axial coordinates.
pub fn hex_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    let (dq, dr) = (a.0 - b.0, a.1 - b.1);
    (dq.abs() + dr.abs() + (dq + dr).abs()) / 2
}
