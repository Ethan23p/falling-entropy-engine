//! Running a scene on the CPU reference, with measurements: what the strip harness and the feel
//! benchmark watch. The GPU parity test proves the shaders compute the same worlds, so a run here
//! shows exactly what the game would.

use sim::{Material, World};

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
    stamps: Vec<crate::Stamp>,
}

impl Run {
    pub fn new(scene: &'static Scene, width: i32, height: i32) -> Run {
        let mut world = World::new(width, height);
        scene.build(&mut world);
        let before = materials(&world);
        Run { scene, world, phase: 0, before, moving: 0, still: 0, stamps: Vec::new() }
    }

    /// The scene's scripted stamps for this phase, then one phase. Returns whether the script
    /// stamped anything.
    pub fn step(&mut self) -> bool {
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
        !self.stamps.is_empty()
    }

    pub fn metrics(&self) -> Metrics {
        Metrics { sand: self.world.count(Material::Sand), moving: self.moving, still: self.still }
    }
}

fn materials(world: &World) -> Vec<Material> {
    (0..world.height()).flat_map(|y| (0..world.width()).map(move |x| world.material(x, y))).collect()
}

/// What a run looks like at one phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub sand: usize,
    /// Cells whose material changed in the last phase.
    pub moving: usize,
    /// Phases in a row with nothing moving and no brush.
    pub still: u32,
}

impl Metrics {
    /// A short readout for captions.
    pub fn line(&self) -> String {
        format!("sand {} · moving {}", self.sand, self.moving)
    }

    /// A named metric, for triggers.
    pub fn get(&self, name: &str) -> Option<f64> {
        Some(match name {
            "sand" => self.sand as f64,
            "moving" => self.moving as f64,
            "still" => self.still as f64,
            _ => return None,
        })
    }
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
    /// A metric compared with a value, e.g. `moving<10` or `sand>=500`.
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
        if text == "settled" {
            return Ok(Trigger::Settled);
        }
        for (sym, op) in [("<=", Op::LessEq), (">=", Op::GreaterEq), ("<", Op::Less), (">", Op::Greater)] {
            if let Some((metric, value)) = text.split_once(sym) {
                let value = value.trim().parse().map_err(|_| format!("bad number in `{text}`"))?;
                let metric = metric.trim().to_string();
                let probe = Metrics { sand: 0, moving: 0, still: 0 };
                if probe.get(&metric).is_none() {
                    return Err(format!("unknown metric `{metric}`: sand, moving or still"));
                }
                return Ok(Trigger::Compare { metric, op, value });
            }
        }
        Err(format!("unknown trigger `{text}`: settled, or e.g. moving<10"))
    }

    pub fn holds(&self, m: &Metrics) -> bool {
        match self {
            Trigger::Settled => m.still >= crate::RATE,
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
    fn run_counts_stillness() {
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
        let m = Metrics { sand: 5, moving: 3, still: 0 };
        assert!(Trigger::parse("moving<10").unwrap().holds(&m));
        assert!(!Trigger::parse("moving >= 4").unwrap().holds(&m));
        assert!(!Trigger::Settled.holds(&m));
        assert_eq!(Trigger::parse("sand>=2").unwrap().name(), "sand>=2");
        assert!(Trigger::parse("speed<1").is_err());
        assert!(Trigger::parse("level").is_err());
    }
}
