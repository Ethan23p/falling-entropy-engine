//! The brush's placement (the doc's Brush entity): pressing shows a ghost, a random tetrahex
//! under the pointer that follows it; releasing confirms it, stamping it once; holding, after a
//! short delay, confirms the same shape again and again; a second press anywhere, a right click
//! or the platform taking the pointer cancels it. The shapes themselves are `sim::Placed`.

use sim::{Placed, to_axial};

#[derive(Default)]
pub struct Brush {
    press: Option<Press>,
    /// Counts presses, so each picks its own shape.
    seed: u32,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    id: u64,
    cell: (i32, i32),
    placed: Placed,
    /// When it went down, in seconds, and how many times holding has confirmed it.
    since: f32,
    repeats: u32,
}

/// The ghost moved from cell `from` to cell `to`: the same shape, translated.
fn shifted(p: Placed, from: (i32, i32), to: (i32, i32)) -> Placed {
    let (a, b) = (to_axial(from.0, from.1), to_axial(to.0, to.1));
    Placed { centres: p.centres.map(|(q, r)| (q + b.0 - a.0, r + b.1 - a.1)), ..p }
}

impl Brush {
    /// A pointer went down: over cell `cell` of the lattice, or elsewhere (`None`). A press while
    /// another is pending cancels it instead.
    pub fn down(&mut self, id: u64, cell: Option<(i32, i32)>, radius: i32, now: f32) {
        if self.press.is_some() {
            self.press = None;
            return;
        }
        if let Some(cell) = cell {
            self.seed = self.seed.wrapping_add(1);
            let placed = Placed::random(self.seed, radius, cell);
            self.press = Some(Press { id, cell, placed, since: now, repeats: 0 });
        }
    }

    pub fn moved(&mut self, id: u64, cell: (i32, i32)) {
        if let Some(p) = self.press.as_mut().filter(|p| p.id == id) {
            p.placed = shifted(p.placed, p.cell, cell);
            p.cell = cell;
        }
    }

    /// The pointer came up: the ghost, if still pending, is confirmed.
    pub fn up(&mut self, id: u64) -> Option<Placed> {
        let p = self.press.filter(|p| p.id == id)?;
        self.press = None;
        Some(p.placed)
    }

    /// A right click, or the platform taking the pointer: the ghost goes, unstamped.
    pub fn cancel(&mut self) {
        self.press = None;
    }

    /// While held, the confirmations due by `now`: one after `delay` seconds, then one every
    /// `every` seconds, each the same shape where the ghost is.
    pub fn held(&mut self, now: f32, delay: f32, every: f32) -> Vec<Placed> {
        let Some(p) = self.press.as_mut() else { return Vec::new() };
        let held = now - p.since - delay;
        if held < 0.0 {
            return Vec::new();
        }
        let due = (held / every.max(0.01)).floor() as u32 + 1;
        let n = due.saturating_sub(p.repeats).min(8);
        p.repeats = due;
        vec![p.placed; n as usize]
    }

    pub fn ghost(&self) -> Option<Placed> {
        self.press.map(|p| p.placed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brush_release_confirms_the_ghost_once() {
        let mut b = Brush::default();
        b.down(1, Some((10, 10)), 1, 0.0);
        let ghost = b.ghost().expect("a ghost on press");
        assert!(b.held(0.1, 0.35, 0.1).is_empty(), "no repeat before the delay");
        assert_eq!(b.up(1), Some(ghost));
        assert_eq!((b.ghost(), b.up(1)), (None, None));
    }

    #[test]
    fn brush_ghost_follows_the_pointer_keeping_its_shape() {
        let mut b = Brush::default();
        b.down(1, Some((10, 10)), 1, 0.0);
        let before = b.ghost().unwrap();
        b.moved(1, (13, 10));
        let after = b.ghost().unwrap();
        let cells = |p: Placed| {
            let mut c = p.cells();
            c.sort();
            c
        };
        assert_eq!(cells(after).len(), cells(before).len());
        assert_ne!(after, before);
        b.moved(1, (10, 10));
        assert_eq!(b.ghost(), Some(before));
    }

    #[test]
    fn brush_holding_repeats_the_first_shape() {
        let mut b = Brush::default();
        b.down(1, Some((10, 10)), 0, 0.0);
        let ghost = b.ghost().unwrap();
        assert_eq!(b.held(0.4, 0.35, 0.1), vec![ghost]);
        assert_eq!(b.held(0.66, 0.35, 0.1), vec![ghost; 3]);
        assert_eq!(b.up(1), Some(ghost));
    }

    #[test]
    fn brush_second_press_or_right_click_cancels() {
        let mut b = Brush::default();
        b.down(1, Some((10, 10)), 1, 0.0);
        b.down(2, None, 1, 0.1);
        assert_eq!((b.ghost(), b.up(1), b.up(2)), (None, None, None));
        b.down(3, Some((5, 5)), 1, 1.0);
        b.cancel();
        assert_eq!(b.up(3), None);
    }

    #[test]
    fn brush_presses_draw_different_shapes() {
        let mut b = Brush::default();
        let mut seen = Vec::new();
        for i in 0..20 {
            b.down(i, Some((20, 20)), 0, 0.0);
            let mut c = b.up(i).unwrap().cells();
            c.sort();
            seen.push(c);
        }
        seen.sort();
        seen.dedup();
        assert!(seen.len() > 5, "{} distinct", seen.len());
    }
}
