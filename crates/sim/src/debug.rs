//! Text views of a world, for tests and debugging: print one in a failing assertion and the
//! lattice is right there in the test output.
//!
//! Each cell is drawn as a brick two text lines tall, one character wide, and odd columns start
//! a line lower, so the column-offset hex lattice keeps its true geometry: a level water surface
//! reads level, and a diagonal reads as a staircase of half steps.
//!
//! Glyphs: `.` air, `:` sand, `~` water, `#` slate, `@` bedrock; sand carrying a packet of water
//! draws `:` over `~`. With intents
//! (`ascii_intents`), the top line of a brick shows the way matter is moving, `↑ ↗ ↘ ↓ ↙ ↖`,
//! on both the giver (its offer) and the receiver (its acceptance).
//!
//! ```text
//!      012
//!    0 : .        a 3x2 world: sand at (0, 0), water at (1, 0),
//!      :~.        slate at (2, 1), air elsewhere
//!    1 .~#
//!      ..#
//!       .
//! ```

use crate::{Dir, Material, Rules, State, World};

/// The character a material draws as.
pub fn glyph(material: Material) -> char {
    match material {
        Material::Air => '.',
        Material::Sand => ':',
        Material::Water => '~',
        Material::Slate => '#',
        Material::Bedrock => '@',
    }
}

/// An arrow pointing the way `d` goes.
pub fn arrow(d: Dir) -> char {
    match d {
        Dir::Up => '↑',
        Dir::UpRight => '↗',
        Dir::DownRight => '↘',
        Dir::Down => '↓',
        Dir::DownLeft => '↙',
        Dir::UpLeft => '↖',
    }
}

/// The way matter is moving through a cell, if any: out along an offer, or in along an acceptance.
pub fn motion(s: State) -> Option<Dir> {
    s.offer.or(s.accept.map(Dir::opposite))
}

impl World {
    /// Rebuilds a world from raw cell words, e.g. a GPU readback.
    pub fn from_words(width: i32, height: i32, words: Vec<u32>) -> World {
        assert_eq!(words.len(), (width * height) as usize, "{width}x{height} lattice");
        World { width, height, cells: words, rules: Rules::default() }
    }

    /// Materials only.
    pub fn ascii(&self) -> String {
        self.ascii_with(|s| (glyph(s.material), glyph(s.sends())))
    }

    /// Materials, with the way matter is moving on the top line of each brick that has an intent.
    pub fn ascii_intents(&self) -> String {
        self.ascii_with(|s| (motion(s).map_or(glyph(s.material), arrow), glyph(s.sends())))
    }

    /// Draws each cell's brick with `cell`'s two characters, top line first, under a ruler of
    /// column numbers (mod 10) and beside row numbers.
    pub fn ascii_with(&self, cell: impl Fn(State) -> (char, char)) -> String {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut lines = vec![vec![' '; w]; 2 * h + 1];
        for y in 0..self.height {
            for x in 0..self.width {
                let (top, bottom) = cell(self.cell(x, y));
                let line = (2 * y + (x & 1)) as usize;
                lines[line][x as usize] = top;
                lines[line + 1][x as usize] = bottom;
            }
        }
        let ruler: String = (0..w).map(|x| char::from(b'0' + (x % 10) as u8)).collect();
        let mut out = format!("     {ruler}\n");
        for (i, line) in lines.iter().enumerate() {
            let label = if i % 2 == 0 && i / 2 < h { format!("{:>4} ", i / 2) } else { "     ".into() };
            let text: String = line.iter().collect();
            out += (label + &text).trim_end();
            out.push('\n');
        }
        out
    }
}

/// Where `got` differs from `want`: `None` when every word matches, else the first mismatch and
/// both worlds with intents, the differing cells marked `X` in a third view.
pub fn diff(want: &World, got: &World) -> Option<String> {
    assert_eq!((want.width, want.height), (got.width, got.height), "same lattice size");
    let first = want.cells.iter().zip(&got.cells).position(|(a, b)| a != b)?;
    let (x, y) = (first as i32 % want.width, first as i32 / want.width);
    let differs = World::from_words(
        want.width,
        want.height,
        want.cells.iter().zip(&got.cells).map(|(a, b)| u32::from(a != b)).collect(),
    );
    let marks = differs.ascii_with(|s| {
        let c = if s.material == Material::Sand { 'X' } else { '.' };
        (c, c)
    });
    let count = differs.cells.iter().filter(|&&d| d != 0).count();
    Some(format!(
        "{count} cells differ, first at ({x}, {y}): want {:?}, got {:?}\nwant:\n{}got:\n{}differ:\n{}",
        want.cell(x, y),
        got.cell(x, y),
        want.ascii_intents(),
        got.ascii_intents(),
        marks
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_ascii_draws_odd_columns_half_a_cell_lower() {
        let mut w = World::new(3, 2);
        w.paint(0, 0, Material::Sand);
        w.paint(1, 0, Material::Water);
        w.paint(2, 1, Material::Slate);
        let want = "     012\n   0 : .\n     :~.\n   1 .~#\n     ..#\n      .\n";
        assert_eq!(w.ascii(), want);
    }

    #[test]
    fn debug_intents_show_motion_on_both_ends() {
        let mut w = World::new(3, 3);
        w.rules.start = [1 << 16; 4]; // falls at once
        w.paint(1, 0, Material::Sand);
        w.step(); // offer
        w.step(); // accept
        let view = w.ascii_intents();
        assert_eq!(view.matches('↓').count(), 2, "{view}");
    }

    #[test]
    fn debug_diff_finds_the_first_mismatch() {
        let a = World::new(4, 4);
        assert_eq!(diff(&a, &a), None);
        let mut b = a.clone();
        b.paint(2, 1, Material::Water);
        b.paint(3, 3, Material::Water);
        let report = diff(&a, &b).expect("differs");
        assert!(report.starts_with("2 cells differ, first at (2, 1)"), "{report}");
    }
}
