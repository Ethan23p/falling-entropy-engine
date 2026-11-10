//! Falling Sand Engine, v00.03: sand, water, slate and air. The lattice lives on the GPU; the
//! brush paints sand, water or slate into air and erases; the config panel tunes the rates, the
//! cell size and the brush.
//!
//! Native: `cargo run -p sand`, or `cargo run -p sand -- --snapshot out.png` for a headless
//! strip of a seeded scene settling. Web: `trunk serve` from this folder.

mod gpu;

use engine::glam::Vec2;
use engine::{App, Frame, Gpu, Input, Phase, Schema, Shapes, Tunable, Tuner, wgpu};
use gpu::{Lattice, Pipelines, Stamp};
use sim::{Material, World};

#[derive(Clone)]
struct Config {
    sim_rate: f32,
    max_phases: i32,
    cell_size: f32,
    brush_radius: f32,
    tool: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { sim_rate: 60.0, max_phases: 8, cell_size: 4.0, brush_radius: 3.0, tool: 0 }
    }
}

impl Tunable for Config {
    const KEY: &'static str = "sand";

    fn schema(s: &mut Schema<Self>) {
        s.group("Rates").float("sim_rate", "Sim rate", "phases/s", 1.0..=960.0, 1.0, |c| &mut c.sim_rate).int(
            "max_phases",
            "Most phases per render frame",
            "",
            1..=64,
            |c| &mut c.max_phases,
        );
        s.group("Lattice").float("cell_size", "Cell size (rebuilds)", "px", 1.0..=32.0, 0.5, |c| &mut c.cell_size);
        s.group("Brush").choice("tool", "Tool", &["Sand", "Water", "Slate", "Erase"], |c| &mut c.tool).float(
            "brush_radius",
            "Radius",
            "cells",
            0.5..=40.0,
            0.5,
            |c| &mut c.brush_radius,
        );
    }
}

struct Game {
    tuner: Tuner<Config>,
    pipelines: Option<Pipelines>,
    lattice: Option<Lattice>,
    /// The cell size the lattice was built for, and whether a rebuild was asked for.
    built_cell_size: f32,
    rebuild: bool,
    /// Phases owed since the last render frame.
    pending: u32,
    stamps: Vec<Stamp>,
    /// Where each held pointer last painted, in cell units.
    strokes: Vec<(u64, Vec2)>,
    /// Top-left and size of one cell, in pixels, as last drawn.
    origin: Vec2,
    cell: Vec2,
    /// Fills a freshly built lattice.
    seed: fn(&mut World),
}

impl Game {
    fn new(tuner: Tuner<Config>, seed: fn(&mut World)) -> Self {
        Self {
            tuner,
            pipelines: None,
            lattice: None,
            built_cell_size: 0.0,
            rebuild: false,
            pending: 0,
            stamps: Vec::new(),
            strokes: Vec::new(),
            origin: Vec2::ZERO,
            cell: Vec2::ONE,
            seed,
        }
    }

    /// A pointer position in cell units, matching the brush shader's cell centres.
    fn to_cells(&self, p: Vec2) -> Vec2 {
        (p - self.origin) / self.cell - 0.5
    }

    fn stamp(&mut self, at: Vec2) {
        let c = &self.tuner.current;
        let material = [Material::Sand, Material::Water, Material::Slate, Material::Air][c.tool.min(3)];
        self.stamps.push(Stamp { x: at.x, y: at.y, radius: c.brush_radius, material });
    }

    /// Stamps along a stroke, close enough that it leaves no gaps.
    fn stroke(&mut self, from: Vec2, to: Vec2) {
        let spacing = (self.tuner.current.brush_radius * 0.5).max(0.5);
        let steps = ((to - from).length() / spacing).ceil().max(1.0) as u32;
        for i in 1..=steps {
            self.stamp(from.lerp(to, i as f32 / steps as f32));
        }
    }
}

impl App for Game {
    fn step(&self) -> f32 {
        1.0 / self.tuner.current.sim_rate
    }

    fn input(&mut self, input: Input, _: &Frame) {
        let Input::Pointer(p) = input else { return };
        let at = self.to_cells(p.pos);
        let held = self.strokes.iter().position(|&(id, _)| id == p.id);
        match (p.phase, held) {
            (Phase::Down, _) => {
                self.stamp(at);
                self.strokes.retain(|&(id, _)| id != p.id);
                self.strokes.push((p.id, at));
            }
            (Phase::Move, Some(i)) => {
                let from = self.strokes[i].1;
                self.stroke(from, at);
                self.strokes[i].1 = at;
            }
            (Phase::Up | Phase::Cancel, Some(i)) => {
                self.strokes.remove(i);
            }
            _ => {}
        }
    }

    fn update(&mut self, _dt: f32) {
        self.pending += 1;
    }

    fn frame(&mut self, _: &Frame, _: &mut Shapes) {}

    fn gpu_init(&mut self, gpu: &Gpu) {
        self.pipelines = Some(Pipelines::new(gpu.device, gpu.format));
    }

    fn gpu_frame(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        frame: &Frame,
    ) -> bool {
        let Some(pipelines) = self.pipelines.as_ref() else { return false };
        let cell_size = self.tuner.current.cell_size;
        if self.lattice.is_none() || self.rebuild || cell_size != self.built_cell_size {
            // The lattice is sized once, to fill the screen at this cell size; later
            // resizes scale it to fit rather than rebuilding it.
            let px = cell_size * frame.scale;
            let width = (frame.size.x / px) as i32;
            let height = (frame.size.y / px - 0.5) as i32;
            if width < 8 || height < 8 {
                // Not laid out yet (a hidden or collapsed page reports a tiny viewport): wait,
                // rather than build a lattice a few cells across that later resizes only stretch.
                return false;
            }
            let mut world = World::new(width, height);
            (self.seed)(&mut world);
            self.lattice = Some(Lattice::new(gpu.device, pipelines, &world));
            self.built_cell_size = cell_size;
            self.rebuild = false;
            self.stamps.clear();
        }
        let lattice = self.lattice.as_ref().expect("lattice");

        let span = Vec2::new(lattice.width as f32, lattice.height as f32 + 0.5);
        let px = (frame.size / span).min_element();
        self.cell = Vec2::splat(px);
        self.origin = ((frame.size - span * px) * 0.5).floor();

        let phases = self.pending.min(self.tuner.current.max_phases.max(1) as u32);
        self.pending = 0;
        lattice.simulate(gpu.queue, pipelines, encoder, &self.stamps, phases);
        self.stamps.clear();
        lattice.render(gpu.queue, pipelines, encoder, target, self.origin.into(), self.cell.into());
        true
    }

    fn config(&mut self) -> Option<&mut dyn engine::tune::Config> {
        Some(&mut self.tuner)
    }

    fn config_actions(&mut self, ui: &mut engine::egui::Ui) {
        if ui.button("Clear").clicked() {
            self.rebuild = true;
        }
    }
}

/// The snapshot scene: a slate funnel under a scattering of sand, a solid block of sand and a
/// block of water, left to settle.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn scene(world: &mut World) {
    let (w, h) = (world.width(), world.height());
    // Two slate ramps sloping in toward a gap in the middle, two thirds of the way down. Each
    // walks from the gap along up-diagonal neighbors, so it has no holes.
    for (start, d) in [(w / 2 + 3, sim::Dir::UpRight), (w / 2 - 3, sim::Dir::UpLeft)] {
        let (mut x, mut y) = (start, h * 2 / 3);
        while world.in_bounds(x, y) && (x - w / 2).abs() < w / 2 - 2 {
            world.paint(x, y, Material::Slate);
            (x, y) = sim::neighbor(x, y, d);
        }
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

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--snapshot") {
            let path = args.get(i + 1).map_or("snapshot.png", String::as_str);
            let mut tuner: Tuner<Config> = Tuner::unsaved();
            tuner.current.max_phases = i32::MAX;
            let mut app = Game::new(tuner, scene);
            engine::headless::snapshot_strip(&mut app, 240, 360, &[0.0, 1.0, 3.0, 8.0], path.as_ref());
            println!("wrote {path}");
            return;
        }
    }
    engine::run(Game::new(Tuner::new(), |_| {}));
}
