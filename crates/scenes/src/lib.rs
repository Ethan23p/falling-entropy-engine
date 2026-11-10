//! Preset scenes, shared by the game (the Scene choice in the config panel, or `?scene=` in the
//! URL) and the harness (strips and the feel benchmark).
//!
//! A scene builds a starting world sized to whatever lattice it is given, and may script brush
//! input over time. It is built either by code here or from a painting: a world painted in the
//! game and copied out as text with Copy scene (see [`Painting`]), saved under `painted/`.

pub mod run;

use sim::{Dir, Material, World};

/// One brush stamp, in cell units, as `World::stamp` takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub material: Material,
}

/// Brush input over time: the stamps to apply just before `phase`, on a `width` x `height` lattice.
pub type Script = fn(phase: u32, width: i32, height: i32, out: &mut Vec<Stamp>);

#[derive(Clone, Copy)]
pub enum Build {
    Code(fn(&mut World)),
    /// A painting's text.
    Painted(&'static str),
}

#[derive(Clone, Copy)]
pub struct Scene {
    pub name: &'static str,
    /// A few words: what's in it.
    pub about: &'static str,
    pub build: Build,
    pub script: Option<Script>,
}

impl Scene {
    /// Builds the scene into `world`, which should be freshly made.
    pub fn build(&self, world: &mut World) {
        match self.build {
            Build::Code(f) => f(world),
            Build::Painted(text) => Painting::parse(text).expect("a valid painting").paint_into(world),
        }
    }

    /// Appends the scripted stamps for `phase`.
    pub fn stamps(&self, phase: u32, width: i32, height: i32, out: &mut Vec<Stamp>) {
        if let Some(script) = self.script {
            script(phase, width, height, out);
        }
    }
}

/// Every scene, in the order the game lists them. The first is the blank lattice.
pub const ALL: &[Scene] = &[
    Scene { name: "empty", about: "a blank lattice", build: Build::Code(|_| {}), script: None },
    Scene { name: "funnel", about: "sand and water into a slate funnel", build: Build::Code(funnel), script: None },
    Scene { name: "dam", about: "a column of water lets go", build: Build::Code(dam), script: None },
    Scene { name: "pour", about: "sand poured onto a floor", build: Build::Code(|_| {}), script: Some(pour) },
    Scene {
        name: "tap",
        about: "water run down a ramp into a basin",
        build: Build::Code(tap),
        script: Some(tap_script),
    },
];

pub fn find(name: &str) -> Option<&'static Scene> {
    ALL.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

pub fn names() -> Vec<&'static str> {
    ALL.iter().map(|s| s.name).collect()
}

/// Paints slate from `(x, y)` stepping toward `d` while in bounds and `go` holds. Walking
/// neighbors leaves no holes, which a drawn line would on this lattice.
fn wall(world: &mut World, (mut x, mut y): (i32, i32), d: Dir, go: impl Fn(i32, i32) -> bool) {
    while world.in_bounds(x, y) && go(x, y) {
        world.paint(x, y, Material::Slate);
        (x, y) = sim::neighbor(x, y, d);
    }
}

/// Two slate ramps sloping in toward a gap, under a scattering of sand, a block of sand and a
/// block of water.
fn funnel(world: &mut World) {
    let (w, h) = (world.width(), world.height());
    for (start, d) in [(w / 2 + 3, Dir::UpRight), (w / 2 - 3, Dir::UpLeft)] {
        wall(world, (start, h * 2 / 3), d, |x, _| (x - w / 2).abs() < w / 2 - 2);
    }
    for y in 0..h / 2 {
        for x in 0..w {
            let block = (w / 3..w / 2).contains(&x) && y < h / 4;
            let pool = (w * 3 / 5..w * 4 / 5).contains(&x) && y < h / 3;
            if pool {
                world.paint(x, y, Material::Water);
            } else if block || sim::hash(x, y, 0, 41).is_multiple_of(4) {
                world.paint(x, y, Material::Sand);
            }
        }
    }
}

/// Water filling the left quarter, free to spread across the floor.
fn dam(world: &mut World) {
    let (w, h) = (world.width(), world.height());
    for y in h / 4..h {
        for x in 0..w / 4 {
            world.paint(x, y, Material::Water);
        }
    }
}

/// A trickle of sand onto the middle of the floor, for 20 s at 60 phases/s.
fn pour(phase: u32, w: i32, _: i32, out: &mut Vec<Stamp>) {
    if phase < 1200 && phase.is_multiple_of(3) {
        out.push(Stamp { x: w as f32 / 2.0, y: 1.0, radius: 0.8, material: Material::Sand });
    }
}

/// A slate ramp falling from the upper left into a walled basin on the right.
fn tap(world: &mut World) {
    let (w, h) = (world.width(), world.height());
    wall(world, (w / 12, h / 3), Dir::DownRight, |x, _| x < w / 2);
    let floor = (w / 2, h - 1 - h / 10);
    wall(world, floor, Dir::Up, |_, y| y > h / 2);
    wall(world, (w / 2, h - 1), Dir::Up, |_, y| y > floor.1);
    wall(world, (w - 1, h - 1), Dir::Up, |_, y| y > h / 2);
}

/// Water run onto the top of the ramp for 15 s.
fn tap_script(phase: u32, w: i32, h: i32, out: &mut Vec<Stamp>) {
    if phase < 900 && phase.is_multiple_of(2) {
        out.push(Stamp { x: w as f32 / 12.0 + 2.0, y: h as f32 / 3.0 - 6.0, radius: 1.2, material: Material::Water });
    }
}

/// A world's materials as text, so a scene painted in the game can be pasted back as a preset.
///
/// ```text
/// fse-scene 1
/// size 120 80
/// about two ramps and a pool
/// 40.3#77.
/// 12~2:
/// ```
///
/// After the header, one line per row, top first: runs of a count (omitted when 1) and a glyph
/// from `sim::debug::glyph`. Trailing air and missing rows are air.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Painting {
    pub width: i32,
    pub height: i32,
    pub about: String,
    cells: Vec<Material>,
}

const HEADER: &str = "fse-scene 1";
const MATERIALS: [Material; 5] = [Material::Air, Material::Sand, Material::Water, Material::Slate, Material::Bedrock];

impl Painting {
    pub fn of(world: &World, about: &str) -> Painting {
        let (width, height) = (world.width(), world.height());
        let cells =
            (0..height).flat_map(|y| (0..width).map(move |x| (x, y))).map(|(x, y)| world.material(x, y)).collect();
        Painting { width, height, about: about.trim().into(), cells }
    }

    pub fn material(&self, x: i32, y: i32) -> Material {
        self.cells[(y * self.width + x) as usize]
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{HEADER}\nsize {} {}\n", self.width, self.height);
        if !self.about.is_empty() {
            out += &format!("about {}\n", self.about);
        }
        for y in 0..self.height {
            let row: Vec<Material> = (0..self.width).map(|x| self.material(x, y)).collect();
            let end = row.iter().rposition(|&m| m != Material::Air).map_or(0, |i| i + 1);
            let mut x = 0;
            while x < end {
                let run = row[x..end].iter().take_while(|&&m| m == row[x]).count();
                if run > 1 {
                    out += &run.to_string();
                }
                out.push(sim::debug::glyph(row[x]));
                x += run;
            }
            out.push('\n');
        }
        out
    }

    pub fn parse(text: &str) -> Result<Painting, String> {
        let mut lines = text.lines().map(str::trim);
        if lines.next() != Some(HEADER) {
            return Err(format!("expected `{HEADER}` first"));
        }
        let size = lines.next().and_then(|l| l.strip_prefix("size ")).ok_or("expected `size W H`")?;
        let mut dims = size.split_whitespace().map(str::parse::<i32>);
        let (Some(Ok(width)), Some(Ok(height)), None) = (dims.next(), dims.next(), dims.next()) else {
            return Err(format!("bad size `{size}`"));
        };
        if width < 1 || height < 1 {
            return Err(format!("bad size `{size}`"));
        }
        let mut about = String::new();
        let mut cells = vec![Material::Air; (width * height) as usize];
        let mut y = 0;
        for line in lines {
            if let Some(a) = line.strip_prefix("about") {
                about = a.trim().into();
                continue;
            }
            if y >= height {
                return Err(format!("more than {height} rows"));
            }
            let (mut x, mut count) = (0, 0usize);
            for c in line.chars() {
                if let Some(d) = c.to_digit(10) {
                    count = count * 10 + d as usize;
                    continue;
                }
                let m =
                    *MATERIALS.iter().find(|&&m| sim::debug::glyph(m) == c).ok_or(format!("unknown glyph `{c}`"))?;
                for _ in 0..count.max(1) {
                    if x >= width {
                        return Err(format!("row {y} is wider than {width}"));
                    }
                    cells[(y * width + x) as usize] = m;
                    x += 1;
                }
                count = 0;
            }
            y += 1;
        }
        Ok(Painting { width, height, about, cells })
    }

    /// Paints into `world`, scaled to its size cell by nearest cell.
    pub fn paint_into(&self, world: &mut World) {
        let (w, h) = (world.width(), world.height());
        for y in 0..h {
            for x in 0..w {
                let m = self.material(x * self.width / w, y * self.height / h);
                if m != Material::Air {
                    world.paint(x, y, m);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenes_have_unique_names_and_build_at_any_size() {
        let mut names = names();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
        for scene in ALL {
            for (w, h) in [(24, 16), (97, 61)] {
                let mut world = World::new(w, h);
                scene.build(&mut world);
                let mut stamps = Vec::new();
                for phase in 0..120 {
                    scene.stamps(phase, w, h, &mut stamps);
                }
                for s in stamps {
                    world.stamp(s.x, s.y, s.radius, s.material);
                }
            }
        }
    }

    #[test]
    fn painting_round_trips_through_text() {
        let mut world = World::new(30, 20);
        find("funnel").unwrap().build(&mut world);
        world.paint(0, 19, Material::Bedrock);
        let painting = Painting::of(&world, "  the funnel ");
        let text = painting.to_text();
        assert!(text.starts_with("fse-scene 1\nsize 30 20\nabout the funnel\n"), "{text}");
        assert_eq!(Painting::parse(&text), Ok(painting.clone()));

        let mut again = World::new(30, 20);
        painting.paint_into(&mut again);
        for y in 0..20 {
            for x in 0..30 {
                assert_eq!(again.material(x, y), world.material(x, y), "({x},{y})");
            }
        }
    }

    #[test]
    fn painting_scales_to_a_bigger_lattice() {
        let text = "fse-scene 1\nsize 2 2\n:~\n#\n";
        let p = Painting::parse(text).unwrap();
        let mut world = World::new(4, 4);
        p.paint_into(&mut world);
        assert_eq!(world.material(0, 0), Material::Sand);
        assert_eq!(world.material(3, 1), Material::Water);
        assert_eq!(world.material(1, 3), Material::Slate);
        assert_eq!(world.material(3, 3), Material::Air);
    }

    #[test]
    fn painting_rejects_bad_text() {
        assert!(Painting::parse("hello").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2\n").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2 1\n3:\n").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2 1\n?\n").is_err());
    }
}
