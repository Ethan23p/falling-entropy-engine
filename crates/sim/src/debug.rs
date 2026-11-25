//! Text views of a world, for tests and debugging: print one in a failing assertion and the
//! lattice is right there in the test output.
//!
//! Each cell is drawn as a brick two text lines tall, one character wide, and odd columns start
//! a line lower, so the column-offset hex lattice keeps its true geometry: a level surface reads
//! level, and a diagonal reads as a staircase of half steps.
//!
//! Glyphs: `.` air, `:` sand, `@` bedrock, `o` vacuum.
//!
//! ```text
//!      012
//!    0 : .        a 3x2 world: sand at (0, 0) and (1, 0),
//!      ::.        bedrock at (2, 1), air elsewhere
//!    1 .:@
//!      ..@
//!       .
//! ```

use crate::{Material, State, WORDS, World};

/// The character a material draws as.
pub fn glyph(material: Material) -> char {
    match material {
        Material::Air => '.',
        Material::Sand => ':',
        Material::Bedrock => '@',
        Material::Vacuum => 'o',
    }
}

impl World {
    /// Materials only.
    pub fn ascii(&self) -> String {
        self.ascii_with(|s| (glyph(s.material), glyph(s.material)))
    }

    /// Draws each cell's brick with `cell`'s two characters, top line first, under a ruler of
    /// column numbers (mod 10) and beside row numbers. `cell` gets what the cell publishes.
    pub fn ascii_with(&self, cell: impl Fn(State) -> (char, char)) -> String {
        let (w, h) = (self.width() as usize, self.height() as usize);
        let mut lines = vec![vec![' '; w]; 2 * h + 1];
        for y in 0..self.height() {
            for x in 0..self.width() {
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
/// both worlds, the differing cells marked `X` in a third view.
pub fn diff(want: &World, got: &World) -> Option<String> {
    assert_eq!((want.width(), want.height()), (got.width(), got.height()), "same lattice size");
    let (w, h) = (want.width(), want.height());
    let differs: Vec<usize> = (0..(w * h) as usize)
        .filter(|&i| want.words()[WORDS * i..WORDS * (i + 1)] != got.words()[WORDS * i..WORDS * (i + 1)])
        .collect();
    let first = *differs.first()?;
    let (x, y) = (first as i32 % w, first as i32 / w);
    let mut marked = World::new(w, h);
    for &i in &differs {
        marked.paint(i as i32 % w, i as i32 / w, Material::Sand);
    }
    let marks = marked.ascii_with(|s| {
        let c = if s.material == Material::Sand { 'X' } else { '.' };
        (c, c)
    });
    Some(format!(
        "{} cells differ, first at ({x}, {y}): want {:?} at phase {}, got {:?} at phase {}
want:
{}got:
{}differ:
{}",
        differs.len(),
        want.cell(x, y),
        want.phase(x, y),
        got.cell(x, y),
        got.phase(x, y),
        want.ascii(),
        got.ascii(),
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
        w.paint(1, 0, Material::Sand);
        w.paint(2, 1, Material::Bedrock);
        let want = "     012\n   0 : .\n     ::.\n   1 .:@\n     ..@\n      .\n";
        assert_eq!(w.ascii(), want);
    }

    #[test]
    fn debug_diff_finds_the_first_mismatch() {
        let a = World::new(4, 4);
        assert_eq!(diff(&a, &a), None);
        let mut b = a.clone();
        b.paint(2, 1, Material::Sand);
        b.paint(3, 3, Material::Sand);
        let report = diff(&a, &b).expect("differs");
        assert!(report.starts_with("2 cells differ, first at (2, 1)"), "{report}");
    }
}
