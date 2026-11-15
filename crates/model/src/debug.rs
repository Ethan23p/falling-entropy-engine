//! Text views of a world, for tests and dumps: print one in a failing assertion and the lattice is
//! right there in the output.
//!
//! Each cell is drawn as a brick two text lines tall and one character wide. On the hex lattice odd
//! columns start a line lower, true to the offset, as in v00's `debug`.
//!
//! Glyphs: `.` air, `~` water, `:` sand, `#` slate, `@` bedrock. Cells draw as the matter they own,
//! so a giver whose proposal was accepted draws as air ([`World::owned`]). With intents, a brick's
//! top line shows the way matter moves through the cell: out along its offer, else in along its
//! acceptance.

use crate::{Material, Shape, Slot, World};

pub fn glyph(material: Material) -> char {
    match material {
        Material::Air => '.',
        Material::Water => '~',
        Material::Sand => ':',
        Material::Slate => '#',
        Material::Bedrock => '@',
    }
}

/// An arrow pointing toward slot `s`.
pub fn arrow(shape: Shape, s: Slot) -> char {
    match shape {
        Shape::Hex => ['↑', '↗', '↘', '↓', '↙', '↖'][s],
        Shape::Square => ['↑', '→', '↓', '←'][s],
    }
}

impl World {
    /// The way matter moves through cell `i`: out along its offer, else in along its acceptance.
    pub fn motion(&self, i: usize) -> Option<Slot> {
        let s = self.cell(i);
        s.offers.iter().position(|&o| o).or(s.accepted().map(|a| self.shape().back(a)))
    }

    /// Materials, as owned.
    pub fn ascii(&self) -> String {
        self.ascii_with(|i| {
            let g = glyph(self.owned(i).material);
            (g, g)
        })
    }

    /// Materials as owned, with the way matter moves on the top line of each brick.
    pub fn ascii_intents(&self) -> String {
        self.ascii_with(|i| {
            let g = glyph(self.owned(i).material);
            (self.motion(i).map_or(g, |s| arrow(self.shape(), s)), g)
        })
    }

    /// Materials as shown rather than owned: matter in transit appears in both giver and receiver.
    pub fn ascii_shown(&self) -> String {
        self.ascii_with(|i| {
            let g = glyph(self.cell(i).matter.material);
            (g, g)
        })
    }

    /// Draws each cell's brick with `cell`'s two characters, top line first, under a ruler of column
    /// numbers (mod 10) and beside row numbers.
    pub fn ascii_with(&self, cell: impl Fn(usize) -> (char, char)) -> String {
        let (w, h) = (self.width() as usize, self.height() as usize);
        let offset = |x: i32| if self.shape() == Shape::Hex { (x & 1) as usize } else { 0 };
        let mut lines = vec![vec![' '; w]; 2 * h + 1];
        for i in 0..self.len() {
            let (x, y) = self.xy(i);
            let (top, bottom) = cell(i);
            let line = 2 * y as usize + offset(x);
            lines[line][x as usize] = top;
            lines[line + 1][x as usize] = bottom;
        }
        let ruler: String = (0..w).map(|x| char::from(b'0' + (x % 10) as u8)).collect();
        let mut out = format!("     {ruler}\n");
        for (i, line) in lines.iter().enumerate() {
            let label = if i % 2 == 0 && i / 2 < h { format!("{:>4} ", i / 2) } else { "     ".into() };
            let text: String = line.iter().collect();
            let text = (label + &text).trim_end().to_string();
            if !text.is_empty() {
                out += &text;
                out.push('\n');
            }
        }
        out
    }
}
