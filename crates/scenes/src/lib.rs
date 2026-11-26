//! Preset scenes, shared by the game (the Scene choice in the config panel, or `?scene=` in the
//! URL) and the harness (strips and the feel benchmark).
//!
//! A scene builds a starting world sized to whatever lattice it is given, and may script brush
//! input over time. It is built either by code here or from a painting: a world painted in the
//! game and copied out as text with Copy scene (see [`Painting`]), saved under `painted/`.

pub mod run;

/// The sim rate, in phases per second, that scripts and measures are written for: the shipped one.
pub const RATE: u32 = 240;

use sim::{Material, World};

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
    Scene { name: "packed", about: "a lattice packed with sand and air", build: Build::Code(packed), script: None },
    Scene {
        name: "packed-vacuum",
        about: "a lattice packed with sand and air and one cell of vacuum",
        build: Build::Code(packed_vacuum),
        script: None,
    },
    Scene { name: "pour", about: "sand poured onto a floor", build: Build::Code(|_| {}), script: Some(pour) },
    Scene {
        name: "silo",
        about: "a slate silo full of sand with a small leak in the bottom, open air outside",
        build: Build::Code(|w| silo(w, Leak::BottomSmall, Outside::Air)),
        script: None,
    },
    Scene {
        name: "silo-wide",
        about: "the silo with a large leak in the bottom",
        build: Build::Code(|w| silo(w, Leak::BottomLarge, Outside::Air)),
        script: None,
    },
    Scene {
        name: "silo-side",
        about: "the silo with a leak in its side",
        build: Build::Code(|w| silo(w, Leak::Side, Outside::Air)),
        script: None,
    },
    Scene {
        name: "silo-vacuum",
        about: "the silo with a small leak, vacuum outside",
        build: Build::Code(|w| silo(w, Leak::BottomSmall, Outside::Vacuum)),
        script: None,
    },
    Scene {
        name: "silo-limited-air",
        about: "the silo with a small leak, vacuum outside but for a layer of air on the floor",
        build: Build::Code(|w| silo(w, Leak::BottomSmall, Outside::LimitedAir)),
        script: None,
    },
    Scene {
        name: "silo-water",
        about: "the silo with a small leak, standing in water",
        build: Build::Code(|w| silo(w, Leak::BottomSmall, Outside::Water)),
        script: None,
    },
    Scene {
        name: "water-drop",
        about: "one water in the center of an all-vacuum lattice",
        build: Build::Code(water_drop),
        script: None,
    },
    Scene {
        name: "packed-water",
        about: "a lattice packed with water and many cells of vacuum",
        build: Build::Code(packed_water),
        script: None,
    },
    Scene {
        name: "pour-water",
        about: "water poured into an all-vacuum lattice",
        build: Build::Code(fill_vacuum),
        script: Some(pour_water),
    },
];

pub fn find(name: &str) -> Option<&'static Scene> {
    ALL.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

pub fn names() -> Vec<&'static str> {
    ALL.iter().map(|s| s.name).collect()
}

/// Every cell holds sand or air, hashed half and half: the first toy case.
fn packed(world: &mut World) {
    let (w, h) = (world.width(), world.height());
    for y in 0..h {
        for x in 0..w {
            let m = if sim::hash(x, y, 0, 41) & 1 == 1 { Material::Sand } else { Material::Air };
            world.paint(x, y, m);
        }
    }
}

/// The packed lattice with one cell of vacuum in the middle: the second toy case.
fn packed_vacuum(world: &mut World) {
    packed(world);
    let (w, h) = (world.width(), world.height());
    world.paint(w / 2, h / 2, Material::Vacuum);
}

/// Where the silo leaks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leak {
    BottomSmall,
    BottomLarge,
    Side,
}

/// What surrounds the silo.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outside {
    Air,
    Vacuum,
    /// Vacuum, but for a layer of air on the floor.
    LimitedAir,
    /// Air, with a pool of water on the floor.
    Water,
}

/// The silo toy case: a slate silo filled with sand, held up in the upper middle of the lattice
/// (slate is static), with a leak.
fn silo(world: &mut World, leak: Leak, outside: Outside) {
    let (w, h) = (world.width(), world.height());
    let (left, right) = (w / 2 - w / 6, w / 2 + w / 6);
    let (top, bottom) = (h / 10, h / 2);
    for y in 0..h {
        for x in 0..w {
            let fill = match outside {
                Outside::Air => Some(Material::Air),
                Outside::Vacuum => None,
                Outside::LimitedAir => (y >= h - h / 8).then_some(Material::Air),
                Outside::Water => Some(if y >= h - h / 5 { Material::Water } else { Material::Air }),
            };
            if let Some(m) = fill {
                world.paint(x, y, m);
            }
        }
    }
    for y in top..=bottom {
        world.paint(left, y, Material::Vacuum);
        world.paint(right, y, Material::Vacuum);
        world.paint(left, y, Material::Slate);
        world.paint(right, y, Material::Slate);
    }
    for x in left..=right {
        world.paint(x, bottom, Material::Vacuum);
        world.paint(x, bottom, Material::Slate);
    }
    for y in top..bottom {
        for x in left + 1..right {
            world.paint(x, y, Material::Vacuum);
            world.paint(x, y, Material::Sand);
        }
    }
    let mid = w / 2;
    let holes: Vec<(i32, i32)> = match leak {
        Leak::BottomSmall => vec![(mid, bottom)],
        Leak::BottomLarge => (mid - 2..=mid + 2).map(|x| (x, bottom)).collect(),
        Leak::Side => vec![(right, bottom - 2), (right, bottom - 1)],
    };
    for (x, y) in holes {
        world.paint(x, y, Material::Vacuum);
        world.paint(x, y, Material::Sand);
    }
}

fn fill_vacuum(world: &mut World) {
    for y in 0..world.height() {
        for x in 0..world.width() {
            world.paint(x, y, Material::Vacuum);
        }
    }
}

/// One water in the center of an all-vacuum lattice.
fn water_drop(world: &mut World) {
    fill_vacuum(world);
    let (w, h) = (world.width(), world.height());
    world.paint(w / 2, h / 2, Material::Water);
}

/// Water everywhere, with one cell in eight vacuum, hashed.
fn packed_water(world: &mut World) {
    for y in 0..world.height() {
        for x in 0..world.width() {
            let m = if sim::hash(x, y, 0, 43).is_multiple_of(8) { Material::Vacuum } else { Material::Water };
            world.paint(x, y, m);
        }
    }
}

/// The sand pour's trickle, in water.
fn pour_water(phase: u32, w: i32, _: i32, out: &mut Vec<Stamp>) {
    if phase < 20 * RATE && phase.is_multiple_of(RATE / 20) {
        out.push(Stamp { x: w as f32 / 2.0, y: 1.0, radius: 0.8, material: Material::Water });
    }
}

/// A trickle of sand onto the middle of the floor, 20 grains a second for 20 s.
fn pour(phase: u32, w: i32, _: i32, out: &mut Vec<Stamp>) {
    if phase < 20 * RATE && phase.is_multiple_of(RATE / 20) {
        out.push(Stamp { x: w as f32 / 2.0, y: 1.0, radius: 0.8, material: Material::Sand });
    }
}

/// A world's materials as text, so a scene painted in the game can be pasted back as a preset.
///
/// ```text
/// fse-scene 1
/// size 120 80
/// about a pile on a ledge
/// 40.3@77.
/// 12:2.:
/// ```
///
/// After the header, one line per row, top first: runs of a count (omitted when 1) and a glyph
/// from `sim::debug::glyph`. Trailing vacuum and missing rows are vacuum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Painting {
    pub width: i32,
    pub height: i32,
    pub about: String,
    cells: Vec<Material>,
}

const HEADER: &str = "fse-scene 1";
const MATERIALS: [Material; 6] = Material::ALL;

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
            let end = row.iter().rposition(|&m| m != Material::Vacuum).map_or(0, |i| i + 1);
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
        let mut cells = vec![Material::Vacuum; (width * height) as usize];
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
                if m != Material::Vacuum {
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
    fn conservation_every_scene_keeps_its_matter_but_for_its_brush() {
        for scene in ALL {
            let mut run = run::Run::new(scene, 48, 36);
            for _ in 0..400 {
                let before = run.world.conserved();
                let painted = run.step();
                assert!(painted || run.world.conserved() == before, "{}", scene.name);
            }
        }
    }

    #[test]
    fn painting_round_trips_through_text() {
        let mut world = World::new(30, 20);
        find("packed").unwrap().build(&mut world);
        world.paint(0, 19, Material::Vacuum);
        world.paint(0, 19, Material::Bedrock);
        let painting = Painting::of(&world, "  the packed lattice ");
        let text = painting.to_text();
        assert!(text.starts_with("fse-scene 1\nsize 30 20\nabout the packed lattice\n"), "{text}");
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
        let text = "fse-scene 1\nsize 2 2\n:.\n@\n";
        let p = Painting::parse(text).unwrap();
        let mut world = World::new(4, 4);
        p.paint_into(&mut world);
        assert_eq!(world.material(0, 0), Material::Sand);
        assert_eq!(world.material(3, 1), Material::Air);
        assert_eq!(world.material(1, 3), Material::Bedrock);
        assert_eq!(world.material(3, 3), Material::Vacuum, "missing cells are vacuum");
    }

    #[test]
    fn painting_rejects_bad_text() {
        assert!(Painting::parse("hello").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2\n").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2 1\n3:\n").is_err());
        assert!(Painting::parse("fse-scene 1\nsize 2 1\n?\n").is_err());
    }
}
