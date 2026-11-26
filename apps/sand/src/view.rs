//! The View's geometry (the doc's View entity): where the main lattice, the toy cases and the
//! options panel sit on screen, how a lattice is fitted and drawn as hexagons, and which cell a
//! screen point falls in. Pure functions of the screen, so they're tested natively.
//!
//! Landscape: toy cases on the left, the lattice centred, options on the right. Portrait: the
//! same, turned a quarter clockwise: toy cases on top, options at the bottom. Everything is
//! placed from an anchor of the screen and sized in ems or viewport units.

use engine::Screen;
use engine::glam::Vec2;

pub const SQRT3: f32 = 1.732_050_8;

/// A screen rectangle, in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub fn new(min: Vec2, size: Vec2) -> Rect {
        Rect { min, size: size.max(Vec2::ZERO) }
    }

    pub fn max(&self) -> Vec2 {
        self.min + self.size
    }

    pub fn center(&self) -> Vec2 {
        self.min + self.size * 0.5
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.cmpge(self.min).all() && p.cmplt(self.max()).all()
    }

    /// The overlap with `other`, as a scissor rectangle within a `target`-sized surface:
    /// x, y, width, height.
    pub fn clip(&self, other: &Rect, target: Vec2) -> [u32; 4] {
        let lo = self.min.max(other.min).max(Vec2::ZERO).floor();
        let hi = self.max().min(other.max()).min(target).ceil();
        let size = (hi - lo).max(Vec2::ZERO);
        [lo.x as u32, lo.y as u32, size.x as u32, size.y as u32]
    }
}

/// Where everything goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Portrait: the landscape layout turned a quarter clockwise.
    pub portrait: bool,
    pub toys: Rect,
    pub main: Rect,
    pub options: Rect,
}

/// The gap between areas, and around the screen's edge, in ems.
const GUTTER: f32 = 0.75;
/// Room kept clear for the config button in its bottom corner, in ems.
pub const CONFIG_BUTTON: f32 = 4.5;

impl Layout {
    pub fn of(s: Screen) -> Layout {
        let g = s.em(GUTTER);
        let (w, h) = (s.size.x, s.size.y);
        if w >= h {
            // Toy cases: a fifth of the width, at least 8 ems; options: room for a name and a swatch.
            let toys_w = s.vw(20.0).clamp(s.em(8.0), s.em(22.0));
            let options_w = s.vw(16.0).clamp(s.em(9.0), s.em(14.0));
            let toys = Rect::new(Vec2::splat(g), Vec2::new(toys_w, h - 2.0 * g));
            let options = Rect::new(Vec2::new(w - g - options_w, g), Vec2::new(options_w, h - 2.0 * g));
            let main_min = Vec2::new(toys.max().x + g, g);
            let main = Rect::new(main_min, Vec2::new(options.min.x - g - main_min.x, h - 2.0 * g));
            Layout { portrait: false, toys, main, options }
        } else {
            let toys_h = s.vh(15.0).clamp(s.em(6.0), s.em(12.0));
            let options_h = s.em(7.0);
            let toys = Rect::new(Vec2::splat(g), Vec2::new(w - 2.0 * g, toys_h));
            let options = Rect::new(Vec2::new(g, h - g - options_h), Vec2::new(w - 2.0 * g, options_h));
            let main_min = Vec2::new(g, toys.max().y + g);
            let main = Rect::new(main_min, Vec2::new(w - 2.0 * g, options.min.y - g - main_min.y));
            Layout { portrait: true, toys, main, options }
        }
    }
}

/// A lattice's extent in hexagon sizes (centre to corner): flat-topped hexes 1.5 apart across,
/// √3 apart down, odd columns half a cell lower. As square bricks (`bricks`), each cell is a
/// square 2 across, in the same column-offset pattern: the same topology, drawn differently.
pub fn extent(width: i32, height: i32, bricks: bool) -> Vec2 {
    if bricks {
        return Vec2::new(2.0 * width as f32, 2.0 * (height as f32 + 0.5));
    }
    Vec2::new(1.5 * (width - 1).max(0) as f32 + 2.0, SQRT3 * (height as f32 + 0.5))
}

/// How a lattice sits on screen: what the render shader's `View` takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// A hexagon's size, centre to corner, in virtual pixels.
    pub size: f32,
    /// Screen pixels per virtual pixel.
    pub texel: f32,
    /// The lattice's top-left, in screen pixels.
    pub origin: Vec2,
    /// Width and height on screen, in pixels.
    pub span: Vec2,
    /// Cells drawn as square bricks rather than hexagons.
    pub bricks: bool,
}

impl Fit {
    /// Fits a `width` x `height` lattice in `area`, centred, as large as fits but at most `scale`
    /// screen pixels per hexagon size (`None`: no limit). `hex_px` is the render resolution: virtual
    /// pixels across one hexagon, corner to corner; 0 draws at the screen's own resolution.
    pub fn new(width: i32, height: i32, area: Rect, scale: Option<f32>, hex_px: f32, bricks: bool) -> Fit {
        let e = extent(width, height, bricks);
        let mut s = (area.size / e).min_element().max(0.01);
        if let Some(limit) = scale {
            s = s.min(limit);
        }
        let span = e * s;
        let origin = (area.min + (area.size - span) * 0.5).floor();
        let (size, texel) = if hex_px > 0.0 { (hex_px * 0.5, s / (hex_px * 0.5)) } else { (s, 1.0) };
        Fit { size, texel, origin, span, bricks }
    }

    /// Screen pixels per hexagon size: what a toy case matches to the main lattice's cells.
    pub fn scale(&self) -> f32 {
        self.size * self.texel
    }

    pub fn rect(&self) -> Rect {
        Rect::new(self.origin, self.span)
    }

    /// The cell a screen point falls in, as the render shader finds it; it may be off the lattice.
    pub fn cell_at(&self, p: Vec2) -> (i32, i32) {
        let v = ((p - self.origin) / self.texel).floor() + 0.5;
        let s = self.size;
        if self.bricks {
            let x = (v.x / (2.0 * s)).floor() as i32;
            return (x, (v.y / (2.0 * s) - 0.5 * (x & 1) as f32).floor() as i32);
        }
        let p = v - Vec2::new(s, 0.5 * SQRT3 * s);
        let (q, r) = round_axial(p.x * 2.0 / (3.0 * s), (-p.x / 3.0 + p.y * SQRT3 / 3.0) / s);
        sim::from_axial(q, r)
    }
}

/// The axial cell nearest fractional axial `(q, r)`.
fn round_axial(q: f32, r: f32) -> (i32, i32) {
    let s = -q - r;
    let (mut rq, mut rr, rs) = (q.round(), r.round(), s.round());
    let (dq, dr, ds) = ((rq - q).abs(), (rr - r).abs(), (rs - s).abs());
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    (rq as i32, rr as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: f32, h: f32) -> Screen {
        Screen::new(Vec2::new(w, h), 2.0)
    }

    #[test]
    fn view_landscape_puts_toys_left_and_options_right() {
        let l = Layout::of(screen(2400.0, 1400.0));
        assert!(!l.portrait);
        assert!(l.toys.max().x < l.main.min.x && l.main.max().x < l.options.min.x);
        assert!(l.main.size.x > l.toys.size.x && l.main.size.x > l.options.size.x);
    }

    #[test]
    fn view_portrait_turns_it_clockwise() {
        let l = Layout::of(screen(1170.0, 2532.0));
        assert!(l.portrait);
        assert!(l.toys.max().y < l.main.min.y && l.main.max().y < l.options.min.y);
        assert!(l.main.size.y > l.toys.size.y && l.main.size.y > l.options.size.y);
    }

    #[test]
    fn view_lattice_is_centred_and_fits() {
        let area = Rect::new(Vec2::new(100.0, 50.0), Vec2::new(800.0, 600.0));
        let f = Fit::new(128, 128, area, None, 0.0, false);
        let r = f.rect();
        assert!(r.min.cmpge(area.min).all() && r.max().cmple(area.max() + 1.0).all());
        assert!((r.center() - area.center()).abs().max_element() <= 1.0);
    }

    #[test]
    fn view_cell_at_finds_each_cells_centre() {
        let area = Rect::new(Vec2::ZERO, Vec2::new(500.0, 500.0));
        for (hex_px, bricks) in [(0.0, false), (6.0, false), (20.0, false), (0.0, true), (6.0, true)] {
            let f = Fit::new(20, 20, area, None, hex_px, bricks);
            for (x, y) in [(0, 0), (1, 0), (3, 7), (19, 19), (10, 4)] {
                let down = y as f32 + 0.5 + 0.5 * (x & 1) as f32;
                let centre = if bricks {
                    Vec2::new(2.0 * x as f32 + 1.0, 2.0 * down)
                } else {
                    Vec2::new(1.5 * x as f32 + 1.0, SQRT3 * down)
                };
                let p = f.origin + centre * f.scale();
                assert_eq!(f.cell_at(p), (x, y), "hex_px {hex_px} bricks {bricks}");
            }
        }
    }

    #[test]
    fn view_resolution_is_separate_from_screen_size() {
        let area = Rect::new(Vec2::ZERO, Vec2::new(900.0, 900.0));
        let coarse = Fit::new(128, 128, area, None, 4.0, false);
        let fine = Fit::new(128, 128, area, None, 24.0, false);
        assert_eq!(coarse.span, fine.span, "same size on screen");
        assert!(coarse.texel > fine.texel, "fewer, larger pixels");
    }
}
