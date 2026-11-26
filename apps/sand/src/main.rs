//! Falling Entropy Engine: air, water, sand, slate and bedrock in vacuum on a synchronized hex
//! lattice, moved by contracts biased by the physics distribution. The lattices live on the GPU.
//!
//! The View: a 128 by 128 lattice drawn as hexagons at a chosen render resolution, centred; the
//! toy cases beside it, each in its own lattice; the options panel on the other side (`view`,
//! `toys`, `options`). The Brush stamps random tetrahexes: press, release, hold to repeat (`brush`).
//! The config panel tunes the physics, the rates and the view, and picks the main lattice's scene.
//!
//! Native: `cargo run -p sand [-- --scene NAME]`. Web: `trunk serve` from this folder, with
//! `?scene=NAME` to start in a scene. The harness: `cargo run -p sand -- strip|bench ...`.

mod brush;
mod gpu;
#[cfg(not(target_arch = "wasm32"))]
mod harness;
mod options;
mod toys;
mod view;

use std::sync::LazyLock;

use engine::glam::Vec2;
use engine::{App, Frame, Gpu, Input, Phase, Schema, Screen, Shapes, Tunable, Tuner, wgpu};
use gpu::{Lattice, Look, Pipelines, Readback, Shape, Stamp};
use scenes::Scene;
use sim::World;
use view::{Fit, Layout, Rect};

static SCENE_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(scenes::names);

#[derive(Clone)]
pub struct Config {
    pub sim_rate: f32,
    pub max_phases: i32,
    pub lattice_size: i32,
    pub hex_px: f32,
    pub hex_edges: f32,
    pub cell_shape: usize,
    pub scene: usize,
    pub brush_size: i32,
    pub brush_hold: f32,
    pub brush_repeat: f32,
    pub brush_offset: f32,
    pub tool: usize,
    pub gravity: i32,
    pub ceiling: i32,
    pub request_odds: i32,
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
            lattice_size: 128,
            hex_px: 0.0,
            hex_edges: 0.22,
            cell_shape: 0,
            scene: 0,
            brush_size: 1,
            brush_hold: 1.0,
            brush_repeat: 1.0,
            brush_offset: 5.0,
            tool: 0,
            gravity: p.gravity as i32,
            ceiling: p.ceiling as i32,
            request_odds: p.request_odds as i32,
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
        let mut p = sim::Physics {
            gravity: u(self.gravity),
            ceiling: u(self.ceiling),
            request_odds: u(self.request_odds),
            ..sim::Physics::default()
        };
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
            .int("request_odds", "Requests: one proposal in this many (0: none)", "", 0..=64, |c| &mut c.request_odds)
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
        s.group("View")
            .int("lattice_size", "Lattice: cells across and down (rebuilds)", "", 8..=512, |c| &mut c.lattice_size)
            .float("hex_px", "Render resolution: pixels per hexagon (0: the screen's)", "px", 0.0..=64.0, 1.0, |c| {
                &mut c.hex_px
            })
            .float("hex_edges", "Cell edges: how dark", "", 0.0..=1.0, 0.01, |c| &mut c.hex_edges)
            .choice("cell_shape", "Cells drawn as", &["Hexagons", "Square bricks"], |c| &mut c.cell_shape)
            .choice("scene", "Scene (rebuilds)", &SCENE_NAMES, |c| &mut c.scene);
        s.group("Brush")
            .choice("tool", "Tool", &["Sand", "Water", "Air", "Slate", "Erase"], |c| &mut c.tool)
            .int("brush_size", "Size: the radius of each hex", "cells", 0..=16, |c| &mut c.brush_size)
            .float("brush_hold", "Hold before repeating", "s", 0.05..=4.0, 0.05, |c| &mut c.brush_hold)
            .float("brush_repeat", "Repeat every", "s", 0.02..=3.0, 0.01, |c| &mut c.brush_repeat)
            .float("brush_offset", "Ghost above the pointer", "em", 0.0..=12.0, 0.25, |c| &mut c.brush_offset);
    }
}

struct Game {
    tuner: Tuner<Config>,
    pipelines: Option<Pipelines>,
    lattice: Option<Lattice>,
    /// What the lattice was built with, and whether a rebuild was asked for.
    built: (i32, usize),
    rebuild: bool,
    /// Phases owed since the last render frame, and phases run since the scene was built.
    pending: u32,
    scene_phase: u32,
    stamps: Vec<Stamp>,
    script: Vec<scenes::Stamp>,
    brush: brush::Brush,
    toys: Vec<toys::Toy>,
    /// The toy strip's scroll, in pixels, and where a drag on it last was.
    scroll: f32,
    scrolling: Option<Vec2>,
    /// The layout and the main lattice's fit, as last drawn.
    screen: Screen,
    layout: Layout,
    fit: Fit,
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

/// The backdrop around the lattices: bedrock's colour, as outside a lattice reads as bedrock.
const BACKDROP: wgpu::Color = wgpu::Color { r: 0.031, g: 0.035, b: 0.047, a: 1.0 };

impl Game {
    fn new(tuner: Tuner<Config>) -> Self {
        let screen = Screen::new(Vec2::new(800.0, 600.0), 1.0);
        let layout = Layout::of(screen);
        Self {
            tuner,
            pipelines: None,
            lattice: None,
            built: (0, 0),
            rebuild: false,
            pending: 0,
            scene_phase: 0,
            stamps: Vec::new(),
            script: Vec::new(),
            brush: brush::Brush::default(),
            toys: toys::all(),
            scroll: 0.0,
            scrolling: None,
            screen,
            layout,
            fit: Fit::new(128, 128, layout.main, None, 0.0, false),
            readback: None,
            panel_open: false,
            frames_since_read: 0,
            scene_text: None,
            copied: 0,
        }
    }

    fn bricks(&self) -> bool {
        self.tuner.current.cell_shape == 1
    }

    /// Where the ghost goes for a pointer at `p`: a little above it, clear of the finger.
    fn aim(&self, p: Vec2) -> Vec2 {
        p - Vec2::new(0.0, self.screen.em(self.tuner.current.brush_offset))
    }

    fn size(&self) -> i32 {
        self.tuner.current.lattice_size.max(8)
    }

    /// Lays everything out for this frame's screen.
    fn arrange(&mut self, frame: &Frame) {
        self.screen = frame.screen();
        self.layout = Layout::of(self.screen);
        let n = self.size();
        self.fit = Fit::new(n, n, self.layout.main, None, self.tuner.current.hex_px, self.bricks());
    }

    fn toy_slots(&self) -> Vec<(Fit, Rect)> {
        let l = &self.layout;
        toys::arrange(
            &self.toys,
            l.toys,
            l.portrait,
            self.screen,
            self.fit.scale(),
            self.tuner.current.hex_px,
            self.bricks(),
            self.scroll,
        )
    }

    /// The cell the ghost aims at for a press at `p`, if the press is on the main lattice. The
    /// aimed cell itself may be past the lattice's top; the stamp then paints what lands on it.
    fn cell_at(&self, p: Vec2) -> Option<(i32, i32)> {
        let (x, y) = self.fit.cell_at(p);
        let n = self.size();
        let on = self.layout.main.contains(p) && (0..n).contains(&x) && (0..n).contains(&y);
        on.then(|| self.fit.cell_at(self.aim(p)))
    }

    fn material(&self) -> sim::Material {
        options::TOOLS[self.tuner.current.tool.min(options::TOOLS.len() - 1)].1
    }

    fn stamp(&mut self, placed: sim::Placed) {
        self.stamps.push(Stamp { shape: Shape::Tetrahex(placed), material: self.material() });
    }

    fn option(&mut self, control: options::Control) {
        let c = &mut self.tuner.current;
        match control {
            options::Control::Tool(i) => c.tool = i,
            options::Control::Smaller => c.brush_size = (c.brush_size - 1).max(0),
            options::Control::Larger => c.brush_size = (c.brush_size + 1).min(16),
            options::Control::Clear => self.rebuild = true,
            options::Control::Size => {}
        }
    }
}

impl App for Game {
    fn step(&self) -> f32 {
        1.0 / self.tuner.current.sim_rate
    }

    fn input(&mut self, input: Input, frame: &Frame) {
        let l = self.layout;
        match input {
            Input::Pointer(p) => match p.phase {
                Phase::Down => {
                    let cell = self.cell_at(p.pos);
                    self.brush.down(p.id, cell, self.tuner.current.brush_size, frame.time);
                }
                Phase::Move => {
                    self.brush.moved(p.id, self.fit.cell_at(self.aim(p.pos)));
                }
                Phase::Up => {
                    if let Some(placed) = self.brush.up(p.id) {
                        self.stamp(placed);
                    }
                }
                Phase::Cancel => self.brush.cancel(),
            },
            Input::Secondary(_) => self.brush.cancel(),
            Input::Tap(p) => {
                if let Some(c) = options::at(l.options, l.portrait, self.screen, p) {
                    self.option(c);
                } else if l.toys.contains(p) {
                    let slots = self.toy_slots();
                    if let Some(i) = slots.iter().position(|(fit, _)| fit.rect().contains(p)) {
                        self.toys[i].playing = !self.toys[i].playing;
                    }
                }
            }
            Input::DragStart(p) if l.toys.contains(p) => self.scrolling = Some(p),
            Input::Drag(p) => {
                if let Some(last) = self.scrolling {
                    let d = if l.portrait { last.x - p.x } else { last.y - p.y };
                    let slots = self.toy_slots();
                    let room = toys::scroll_room(&slots, l.toys, l.portrait, self.scroll);
                    self.scroll = (self.scroll + d).clamp(0.0, room);
                    self.scrolling = Some(p);
                }
            }
            Input::DragEnd(_) => self.scrolling = None,
            _ => {}
        }
    }

    fn update(&mut self, _dt: f32) {
        self.pending += 1;
    }

    fn frame(&mut self, frame: &Frame, shapes: &mut Shapes) {
        self.arrange(frame);
        let l = self.layout;
        let c = &self.tuner.current;
        options::draw(shapes, l.options, l.portrait, self.screen, c.tool, c.brush_size);
        let slots = self.toy_slots();
        toys::draw(shapes, &self.toys, &slots, l.toys, self.screen);
    }

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
        self.arrange(frame);
        let Some(pipelines) = self.pipelines.as_ref() else { return false };
        let c = self.tuner.current.clone();
        let n = self.size();
        let wanted = (n, c.scene);
        if self.lattice.is_none() || self.rebuild || wanted != self.built {
            let mut world = World::new(n, n);
            c.scene().build(&mut world);
            self.lattice = Some(Lattice::new(gpu.device, pipelines, &world));
            self.built = wanted;
            self.rebuild = false;
            self.scene_phase = 0;
            self.stamps.clear();
            self.scene_text = None;
        }
        for toy in &mut self.toys {
            if toy.lattice.is_none() {
                let (w, h) = toy.scene.size;
                let mut world = World::new(w, h);
                toy.scene.build(&mut world);
                toy.lattice = Some(Lattice::new(gpu.device, pipelines, &world));
            }
        }

        // Holding the brush confirms its ghost again and again.
        let material = self.material();
        for placed in self.brush.held(frame.time, c.brush_hold, c.brush_repeat) {
            self.stamps.push(Stamp { shape: Shape::Tetrahex(placed), material });
        }

        let phases = self.pending.min(c.max_phases.max(1) as u32);
        self.pending = 0;
        let physics = c.physics();
        let disc =
            |s: &scenes::Stamp| Stamp { shape: Shape::Disc { x: s.x, y: s.y, radius: s.radius }, material: s.material };

        // The main lattice: the scene's script for the phases this frame runs, with the player's stamps.
        let lattice = self.lattice.as_ref().expect("lattice");
        self.script.clear();
        for p in self.scene_phase..self.scene_phase + phases {
            c.scene().stamps(p, lattice.width, lattice.height, &mut self.script);
        }
        self.scene_phase += phases;
        self.stamps.extend(self.script.iter().map(disc));
        lattice.simulate(gpu.queue, pipelines, encoder, &self.stamps, &physics, phases);
        self.stamps.clear();
        let size = frame.size;
        let whole = Rect::new(Vec2::ZERO, size);
        let look = |fit: &Fit, ghost| Look {
            size: fit.size,
            texel: fit.texel,
            origin: fit.origin.into(),
            edges: c.hex_edges,
            bricks: fit.bricks,
            ghost,
        };
        let ghost = self.brush.ghost().map(|g| (g, self.material()));
        let clip = self.fit.rect().clip(&whole, size);
        lattice.render(gpu.queue, pipelines, encoder, target, &look(&self.fit, ghost), clip, Some(BACKDROP));

        // The toy cases: each plays only once tapped.
        let slots = self.toy_slots();
        for (toy, (fit, _)) in self.toys.iter_mut().zip(&slots) {
            let lattice = toy.lattice.as_ref().expect("toy lattice");
            if toy.playing {
                self.script.clear();
                for p in toy.phase..toy.phase + phases {
                    toy.scene.stamps(p, lattice.width, lattice.height, &mut self.script);
                }
                toy.phase += phases;
                let stamps: Vec<Stamp> = self.script.iter().map(disc).collect();
                lattice.simulate(gpu.queue, pipelines, encoder, &stamps, &physics, phases);
            }
            let clip = fit.rect().clip(&self.layout.toys, size);
            if clip[2] > 0 && clip[3] > 0 {
                lattice.render(gpu.queue, pipelines, encoder, target, &look(fit, None), clip, None);
            }
        }
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
