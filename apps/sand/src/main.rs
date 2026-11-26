//! Falling Entropy Engine, v01.05: air, water, sand, slate and bedrock in vacuum on a synchronized hex
//! lattice, moved by contracts biased by the physics distribution. The lattice lives on the GPU; the
//! brush paints sand, water, air or slate and erases to vacuum; the config panel tunes the physics, the rates, the cell size and the
//! brush, and picks a preset scene.
//!
//! Native: `cargo run -p sand [-- --scene NAME]`. Web: `trunk serve` from this folder, with
//! `?scene=NAME` to start in a scene. The harness: `cargo run -p sand -- strip|bench ...`.

mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod harness;

use std::sync::LazyLock;

use engine::glam::Vec2;
use engine::{App, Frame, Gpu, Input, Phase, Schema, Shapes, Tunable, Tuner, wgpu};
use gpu::{Lattice, Pipelines, Readback, Stamp};
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
    pub gravity: i32,
    pub ceiling: i32,
    pub mass_air: i32,
    pub mass_water: i32,
    pub mass_sand: i32,
    pub into_air: i32,
    pub into_water: i32,
    pub into_sand: i32,
    pub into_vacuum: i32,
    pub deflect_air: i32,
    pub deflect_water: i32,
    pub deflect_sand: i32,
}

impl Default for Config {
    fn default() -> Self {
        use sim::Material::{Air, Sand, Vacuum, Water};
        let p = sim::Physics::default();
        let of = |m| p.of(m);
        Self {
            sim_rate: 240.0,
            max_phases: 16,
            cell_size: 4.0,
            scene: 0,
            brush_radius: 3.0,
            tool: 0,
            gravity: p.gravity as i32,
            ceiling: p.ceiling as i32,
            mass_air: of(Air).mass as i32,
            mass_water: of(Water).mass as i32,
            mass_sand: of(Sand).mass as i32,
            into_air: of(Air).into as i32,
            into_water: of(Water).into as i32,
            into_sand: of(Sand).into as i32,
            into_vacuum: of(Vacuum).into as i32,
            deflect_air: of(Air).deflect as i32,
            deflect_water: of(Water).deflect as i32,
            deflect_sand: of(Sand).deflect as i32,
        }
    }
}

impl Config {
    /// The physics distribution's data, as the sim takes it: the defaults, with the tuned values.
    pub fn physics(&self) -> sim::Physics {
        use sim::Material::{Air, Sand, Vacuum, Water};
        let u = |v: i32| v.max(0) as u32;
        let mut p = sim::Physics { gravity: u(self.gravity), ceiling: u(self.ceiling), ..sim::Physics::default() };
        p.of_mut(Air).mass = u(self.mass_air);
        p.of_mut(Water).mass = u(self.mass_water);
        p.of_mut(Sand).mass = u(self.mass_sand);
        p.of_mut(Air).into = u(self.into_air);
        p.of_mut(Water).into = u(self.into_water);
        p.of_mut(Sand).into = u(self.into_sand);
        p.of_mut(Vacuum).into = u(self.into_vacuum);
        p.of_mut(Air).deflect = u(self.deflect_air);
        p.of_mut(Water).deflect = u(self.deflect_water);
        p.of_mut(Sand).deflect = u(self.deflect_sand);
        p
    }

    pub fn scene(&self) -> &'static Scene {
        &scenes::ALL[self.scene.min(scenes::ALL.len() - 1)]
    }
}

impl Tunable for Config {
    const KEY: &'static str = "sand";

    fn schema(s: &mut Schema<Self>) {
        s.group("Physics")
            .int("gravity", "Gravity: impulse per move, per mass", "", 0..=1024, |c| &mut c.gravity)
            .int("ceiling", "Velocity ceiling: the magnitude impulses approach", "", 1..=8192, |c| &mut c.ceiling)
            .int("mass_sand", "Mass of sand", "", 0..=16, |c| &mut c.mass_sand)
            .int("mass_water", "Mass of water", "", 0..=16, |c| &mut c.mass_water)
            .int("mass_air", "Mass of air", "", 0..=16, |c| &mut c.mass_air)
            .int("into_air", "Weight of moving into air", "", 0..=255, |c| &mut c.into_air)
            .int("into_water", "Weight of moving into water", "", 0..=255, |c| &mut c.into_water)
            .int("into_sand", "Weight of moving into sand", "", 0..=255, |c| &mut c.into_sand)
            .int("into_vacuum", "Weight of moving into vacuum", "", 0..=255, |c| &mut c.into_vacuum)
            .int("deflect_water", "Deflection of water on rejection", "", 0..=1024, |c| &mut c.deflect_water)
            .int("deflect_sand", "Deflection of sand on rejection", "", 0..=1024, |c| &mut c.deflect_sand)
            .int("deflect_air", "Deflection of air on rejection", "", 0..=1024, |c| &mut c.deflect_air);
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
        s.group("Brush").choice("tool", "Tool", &["Sand", "Water", "Air", "Slate", "Erase"], |c| &mut c.tool).float(
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
    /// Copy scene: while the config panel is open, the world is read back every few frames
    /// and kept as a painting's text. A phone's clipboard accepts text only during the tap,
    /// so the tap copies text that is already there.
    readback: Option<Readback>,
    panel_open: bool,
    frames_since_read: u32,
    scene_text: Option<String>,
    /// Frames left to show "Copied".
    copied: u32,
}

/// How often Copy scene's text is refreshed while the panel is open, in frames.
const READ_EVERY: u32 = 15;

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
            readback: None,
            panel_open: false,
            frames_since_read: 0,
            scene_text: None,
            copied: 0,
        }
    }

    /// A pointer position in cell units, matching the brush shader's cell centres.
    fn to_cells(&self, p: Vec2) -> Vec2 {
        (p - self.origin) / self.cell - 0.5
    }

    fn stamp(&mut self, at: Vec2) {
        let c = &self.tuner.current;
        let material =
            [Material::Sand, Material::Water, Material::Air, Material::Slate, Material::Vacuum][c.tool.min(4)];
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
            self.scene_text = None;
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
        let physics = self.tuner.current.physics();
        lattice.simulate(gpu.queue, pipelines, encoder, &self.stamps, &physics, phases);
        self.stamps.clear();
        lattice.render(gpu.queue, pipelines, encoder, target, self.origin.into(), self.cell.into());
        self.read_scene(gpu, encoder);
        true
    }

    fn config(&mut self) -> Option<&mut dyn engine::tune::Config> {
        Some(&mut self.tuner)
    }

    fn config_actions(&mut self, ui: &mut engine::egui::Ui) {
        self.panel_open = true;
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.rebuild = true;
            }
            let label = if self.copied > 0 { "Copied" } else { "Copy scene" };
            let copy = ui.add_enabled(self.scene_text.is_some(), engine::egui::Button::new(label));
            if copy.clicked()
                && let Some(text) = &self.scene_text
            {
                ui.ctx().copy_text(text.clone());
                self.copied = 60;
            }
            // v1 is deployed beside the game, at /phosphor/ (see .github/workflows/pages.yml).
            #[cfg(target_arch = "wasm32")]
            if ui.button("Phosphor Sand").clicked()
                && let Some(window) = web_sys::window()
            {
                let _ = window.location().set_href(PHOSPHOR_SAND);
            }
        });
    }
}

impl Game {
    /// Keeps `scene_text` fresh while the config panel is open (see `readback`).
    fn read_scene(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder) {
        let lattice = self.lattice.as_ref().expect("lattice");
        let open = std::mem::take(&mut self.panel_open);
        self.copied = self.copied.saturating_sub(1);
        self.frames_since_read += 1;
        let readback = self.readback.get_or_insert_with(|| Readback::new(gpu.device, lattice));
        if let Some(words) = readback.poll(gpu.device) {
            let world = World::from_words(readback.width, readback.height, words);
            let about = match self.tuner.current.scene().name {
                "empty" => "painted in the game".to_string(),
                name => format!("painted in the game, from {name}"),
            };
            self.scene_text = Some(scenes::Painting::of(&world, &about).to_text());
        }
        if !open || !readback.idle() || self.frames_since_read < READ_EVERY {
            return;
        }
        if !readback.fits(lattice) {
            *readback = Readback::new(gpu.device, lattice);
        }
        readback.start(encoder, lattice);
        self.frames_since_read = 0;
    }
}

/// Where the config panel's Phosphor Sand button goes: v1, deployed beside the game.
#[cfg(target_arch = "wasm32")]
const PHOSPHOR_SAND: &str = "phosphor/";

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
