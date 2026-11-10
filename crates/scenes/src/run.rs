//! Running a scene on the CPU reference, with measurements: what the strip harness and the feel
//! benchmark watch. The GPU parity test proves the shaders compute the same worlds, so a run here
//! shows exactly what the game would.

use sim::{Material, World, height};

use crate::Scene;

/// A scene being run: its world, its phase and its script.
pub struct Run {
    pub scene: &'static Scene,
    pub world: World,
    pub phase: u32,
    /// Each cell's material a phase ago, for `moving`.
    before: Vec<Material>,
    /// Cells whose material changed in the last phase.
    moving: usize,
    /// Phases in a row with nothing moving.
    still: u32,
    /// Phases in a row with water level.
    level_for: u32,
    stamps: Vec<crate::Stamp>,
}

impl Run {
    pub fn new(scene: &'static Scene, width: i32, height: i32) -> Run {
        let mut world = World::new(width, height);
        scene.build(&mut world);
        let before = materials(&world);
        Run { scene, world, phase: 0, before, moving: 0, still: 0, level_for: 0, stamps: Vec::new() }
    }

    /// The scene's scripted stamps for this phase, then one phase.
    pub fn step(&mut self) {
        let (w, h) = (self.world.width(), self.world.height());
        self.stamps.clear();
        self.scene.stamps(self.phase, w, h, &mut self.stamps);
        for s in &self.stamps {
            self.world.stamp(s.x, s.y, s.radius, s.material);
        }
        self.world.step();
        self.phase += 1;
        let now = materials(&self.world);
        self.moving = now.iter().zip(&self.before).filter(|(a, b)| a != b).count();
        self.still = if self.moving == 0 && self.stamps.is_empty() { self.still + 1 } else { 0 };
        self.before = now;
        let level = water_level(&self.world).is_some_and(|l| l <= 2);
        self.level_for = if level { self.level_for + 1 } else { 0 };
    }

    pub fn metrics(&self) -> Metrics {
        Metrics {
            sand: self.world.count(Material::Sand),
            water: self.world.count(Material::Water),
            moving: self.moving,
            still: self.still,
            level: water_level(&self.world),
            level_for: self.level_for,
        }
    }
}

fn materials(world: &World) -> Vec<Material> {
    (0..world.height()).flat_map(|y| (0..world.width()).map(move |x| world.material(x, y))).collect()
}

/// What a run looks like at one phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub sand: usize,
    pub water: usize,
    /// Cells whose material changed in the last phase: two per transfer.
    pub moving: usize,
    /// Phases in a row with nothing moving and no brush.
    pub still: u32,
    /// How far water's resting surface is from level, in half cells (see `water_level`).
    pub level: Option<i32>,
    /// Phases in a row with `level` at most 2.
    pub level_for: u32,
}

impl Metrics {
    /// A short readout for captions.
    pub fn line(&self) -> String {
        let mut parts =
            vec![format!("sand {}", self.sand), format!("water {}", self.water), format!("moving {}", self.moving)];
        if let Some(l) = self.level {
            parts.push(format!("level {l}"));
        }
        parts.join(" · ")
    }

    /// A named metric, for triggers.
    pub fn get(&self, name: &str) -> Option<f64> {
        Some(match name {
            "sand" => self.sand as f64,
            "water" => self.water as f64,
            "moving" => self.moving as f64,
            "still" => self.still as f64,
            "level" => self.level? as f64,
            _ => return None,
        })
    }
}

/// The spread of water's resting surface: the highest column top minus the lowest, in half
/// cells, over columns holding resting water. Water rests when matter runs unbroken from it down
/// to the floor or to slate, so drops and blocks in flight don't count. A level surface reads 1
/// (the lattice's half-cell offset); a partial layer of drops skating on it reads 2. `None` while
/// more than a tenth of the water is in flight, or there is none. Meant for one basin: separate
/// pools at different heights read as not level.
pub fn water_level(world: &World) -> Option<i32> {
    let (w, h) = (world.width(), world.height());
    let mut resting = 0;
    let mut tops = Vec::new();
    for x in 0..w {
        // Up the column from the floor, while matter is unbroken or slate holds it up.
        let mut top = None;
        let mut supported = true;
        for y in (0..h).rev() {
            supported = match world.material(x, y) {
                Material::Air => false,
                Material::Slate | Material::Bedrock => true,
                Material::Sand | Material::Water => supported,
            };
            if supported && world.material(x, y) == Material::Water {
                top = Some(height(x, y));
                resting += 1;
            }
        }
        tops.extend(top);
    }
    if resting * 10 < world.count(Material::Water) * 9 {
        return None;
    }
    Some(tops.iter().max()? - tops.iter().min()?)
}

/// The slope of the sand pile, in degrees from horizontal: the tallest column's rise above the
/// floor over the half-width of the pile's base, measured as drawn (cells one wide, one tall).
pub fn pile_angle(world: &World) -> Option<f64> {
    let (w, h) = (world.width(), world.height());
    let column = |x: i32| (0..h).find(|&y| world.material(x, y) == Material::Sand).map(|y| h - y);
    let heights: Vec<(i32, i32)> = (0..w).filter_map(|x| column(x).map(|c| (x, c))).collect();
    let (&(first, _), &(last, _)) = (heights.first()?, heights.last()?);
    let peak = heights.iter().map(|&(_, c)| c).max()?;
    let half = (last - first + 1) as f64 / 2.0;
    Some((peak as f64 / half).atan().to_degrees())
}

/// A condition to capture a frame at: the first phase it holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    /// Nothing has moved for a second's worth of phases.
    Settled,
    /// Water's resting surface has been level (`level` at most 2) for a second's worth of phases.
    Level,
    /// A metric compared with a value, e.g. `moving<10` or `water>=500`.
    Compare { metric: String, op: Op, value: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Less,
    LessEq,
    Greater,
    GreaterEq,
}

impl Trigger {
    pub fn parse(text: &str) -> Result<Trigger, String> {
        let text = text.trim();
        match text {
            "settled" => return Ok(Trigger::Settled),
            "level" => return Ok(Trigger::Level),
            _ => {}
        }
        for (sym, op) in [("<=", Op::LessEq), (">=", Op::GreaterEq), ("<", Op::Less), (">", Op::Greater)] {
            if let Some((metric, value)) = text.split_once(sym) {
                let value = value.trim().parse().map_err(|_| format!("bad number in `{text}`"))?;
                let metric = metric.trim().to_string();
                let probe = Metrics { sand: 0, water: 0, moving: 0, still: 0, level: Some(0), level_for: 0 };
                if probe.get(&metric).is_none() {
                    return Err(format!("unknown metric `{metric}`: sand, water, moving, still or level"));
                }
                return Ok(Trigger::Compare { metric, op, value });
            }
        }
        Err(format!("unknown trigger `{text}`: settled, level, or e.g. moving<10"))
    }

    pub fn holds(&self, m: &Metrics) -> bool {
        match self {
            Trigger::Settled => m.still >= crate::RATE,
            Trigger::Level => m.level_for >= crate::RATE,
            Trigger::Compare { metric, op, value } => m.get(metric).is_some_and(|v| match op {
                Op::Less => v < *value,
                Op::LessEq => v <= *value,
                Op::Greater => v > *value,
                Op::GreaterEq => v >= *value,
            }),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Trigger::Settled => "settled".into(),
            Trigger::Level => "level".into(),
            Trigger::Compare { metric, op, value } => {
                let sym = match op {
                    Op::Less => "<",
                    Op::LessEq => "<=",
                    Op::Greater => ">",
                    Op::GreaterEq => ">=",
                };
                format!("{metric}{sym}{value}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find;

    #[test]
    fn run_counts_motion_and_stillness() {
        let mut run = Run::new(find("funnel").unwrap(), 40, 30);
        run.step();
        run.step();
        run.step();
        assert!(run.metrics().moving > 0);
        let mut empty = Run::new(find("empty").unwrap(), 10, 10);
        for _ in 0..crate::RATE + 10 {
            empty.step();
        }
        assert!(Trigger::Settled.holds(&empty.metrics()));
    }

    #[test]
    fn run_applies_the_script() {
        let mut run = Run::new(find("pour").unwrap(), 30, 20);
        for _ in 0..30 {
            run.step();
        }
        assert!(run.metrics().sand > 0);
    }

    #[test]
    fn water_level_reads_one_when_level() {
        let mut w = World::new(12, 8);
        for y in 5..8 {
            for x in 0..12 {
                w.paint(x, y, Material::Water);
            }
        }
        assert_eq!(water_level(&w), Some(1));
        assert_eq!(water_level(&World::new(4, 4)), None);
    }

    #[test]
    fn pile_angle_of_a_triangle() {
        // A pile 4 cells tall on a base 8 wide: atan(4 / 4) = 45 degrees.
        let mut w = World::new(12, 6);
        for (x, c) in [(2, 1), (3, 2), (4, 3), (5, 4), (6, 4), (7, 3), (8, 2), (9, 1)] {
            for y in 6 - c..6 {
                w.paint(x, y, Material::Sand);
            }
        }
        assert!((pile_angle(&w).unwrap() - 45.0).abs() < 1e-9);
    }

    #[test]
    fn triggers_parse_and_hold() {
        let m = Metrics { sand: 5, water: 0, moving: 3, still: 0, level: None, level_for: 0 };
        assert!(Trigger::parse("moving<10").unwrap().holds(&m));
        assert!(!Trigger::parse("moving >= 4").unwrap().holds(&m));
        assert!(!Trigger::Level.holds(&m));
        assert_eq!(Trigger::parse("sand>=2").unwrap().name(), "sand>=2");
        assert!(Trigger::parse("speed<1").is_err());
        assert!(Trigger::parse("wobbly").is_err());
    }
}
