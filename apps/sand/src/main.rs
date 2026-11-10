//! Falling Sand Engine, v00.03: sand, water, slate and air. The lattice lives on the GPU; the
//! brush paints sand, water or slate into air and erases; the config panel tunes the rates, the
//! cell size and the brush, and picks a preset scene.
//!
//! Native: `cargo run -p sand [-- --scene NAME]`. Web: `trunk serve` from this folder, with
//! `?scene=NAME` to start in a scene. The harness: `cargo run -p sand -- strip|bench ...`.

mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod harness;

use std::sync::LazyLock;

use engine::glam::Vec2;
use engine::{App, Frame, Gpu, Input, Phase, Schema, Shapes, Tunable, Tuner, wgpu};
use gpu::{Lattice, Pipelines, Stamp};
use scenes::Scene;
use sim::{Material, World};

static SCENE_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(scenes::names);

#[derive(Clone)]
pub struct Config {
    pub sim_rate: f32,
    pub max_phases: i32,
    pub cell_size: f32,
    pub scene: usize,
    pub brush_radius: f32,
    pub tool: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self { sim_rate: 60.0, max_phases: 8, cell_size: 4.0, scene: 0, brush_radius: 3.0, tool: 0 }
    }
}

impl Config {
    pub fn scene(&self) -> &'static Scene {
        &scenes::ALL[self.scene.min(scenes::ALL.len() - 1)]
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
        s.group("Lattice")
            .float("cell_size", "Cell size (rebuilds)", "px", 1.0..=32.0, 0.5, |c| &mut c.cell_size)
            .choice("scene", "Scene (rebuilds)", &SCENE_NAMES, |c| &mut c.scene);
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
    /// What the lattice was built with, and whether a rebuild was asked for.
    built: (f32, usize),
    rebuild: bool,
    /// Phases owed since the last render frame, and phases run since the scene was built.
    pending: u32,
    scene_phase: u32,
    stamps: Vec<Stamp>,
    script: Vec<scenes::Stamp>,
    /// Where each held pointer last painted, in cell units.
    strokes: Vec<(u64, Vec2)>,
    /// Top-left and size of one cell, in pixels, as last drawn.
    origin: Vec2,
    cell: Vec2,
}

impl Game {
    fn new(tuner: Tuner<Config>) -> Self {
        Self {
            tuner,
            pipelines: None,
            lattice: None,
            built: (0.0, 0),
            rebuild: false,
            pending: 0,
            scene_phase: 0,
            stamps: Vec::new(),
            script: Vec::new(),
            strokes: Vec::new(),
            origin: Vec2::ZERO,
            cell: Vec2::ONE,
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
        let c = &self.tuner.current;
        let wanted = (c.cell_size, c.scene);
        if self.lattice.is_none() || self.rebuild || wanted != self.built {
            // The lattice is sized once, to fill the screen at this cell size; later
            // resizes scale it to fit rather than rebuilding it.
            let px = c.cell_size * frame.scale;
            let width = (frame.size.x / px) as i32;
            let height = (frame.size.y / px - 0.5) as i32;
            if width < 8 || height < 8 {
                // Not laid out yet (a hidden or collapsed page reports a tiny viewport): wait,
                // rather than build a lattice a few cells across that later resizes only stretch.
                return false;
            }
            let mut world = World::new(width, height);
            c.scene().build(&mut world);
            self.lattice = Some(Lattice::new(gpu.device, pipelines, &world));
            self.built = wanted;
            self.rebuild = false;
            self.scene_phase = 0;
            self.stamps.clear();
        }
        let lattice = self.lattice.as_ref().expect("lattice");

        let span = Vec2::new(lattice.width as f32, lattice.height as f32 + 0.5);
        let px = (frame.size / span).min_element();
        self.cell = Vec2::splat(px);
        self.origin = ((frame.size - span * px) * 0.5).floor();

        let phases = self.pending.min(self.tuner.current.max_phases.max(1) as u32);
        self.pending = 0;
        // The scene's script for the phases this frame runs, applied with the player's strokes.
        self.script.clear();
        let scene = self.tuner.current.scene();
        for p in self.scene_phase..self.scene_phase + phases {
            scene.stamps(p, lattice.width, lattice.height, &mut self.script);
        }
        self.scene_phase += phases;
        self.stamps.extend(self.script.iter().map(|s| Stamp {
            x: s.x,
            y: s.y,
            radius: s.radius,
            material: s.material,
        }));
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

/// The scene named in the page's `?scene=` query, if any.
#[cfg(target_arch = "wasm32")]
fn url_scene() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search.trim_start_matches('?').split('&').find_map(|kv| kv.strip_prefix("scene=")).map(str::to_string)
}

fn main() {
    let mut tuner: Tuner<Config> = Tuner::new();
    #[cfg(not(target_arch = "wasm32"))]
    let named = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if matches!(args.first().map(String::as_str), Some("strip" | "bench")) {
            std::process::exit(harness::main(&args));
        }
        args.iter().position(|a| a == "--scene").and_then(|i| args.get(i + 1).cloned())
    };
    #[cfg(target_arch = "wasm32")]
    let named = url_scene();
    if let Some(name) = named {
        match scenes::ALL.iter().position(|s| s.name.eq_ignore_ascii_case(&name)) {
            Some(i) => tuner.current.scene = i,
            None => log_unknown(&name),
        }
    }
    engine::run(Game::new(tuner));
}

fn log_unknown(name: &str) {
    let known = scenes::names().join(", ");
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("no scene `{name}`; scenes: {known}");
    #[cfg(target_arch = "wasm32")]
    web_sys::console::warn_1(&format!("no scene `{name}`; scenes: {known}").into());
}
