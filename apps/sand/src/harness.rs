//! The harness, native only.
//!
//! `strip` runs scenes on the CPU reference (which the parity test proves matches the shaders),
//! renders chosen moments through the game's own render shader, and stacks them into a vertical
//! strip under a text header: what to look at after a sim or render change.
//!
//! `bench` measures feel on the CPU reference (how steep sand piles, how fast it settles) and
//! compares with the last run, so a change to a rule shows up as numbers.

use std::path::{Path, PathBuf};

use engine::{Tuner, wgpu};
use image::{Rgba, RgbaImage};
use scenes::Scene;
use scenes::run::{Run, Trigger, pile_angle};

use crate::Config;
use crate::gpu::{Lattice, Pipelines};

const USAGE: &str = "\
usage:
  cargo run -p sand -- strip <scene|all>... [options]
      --note WORDS      a few words on what this run is for, shown in the header
      --at 0,1,3,8      capture at these times, in seconds (the default)
      --when TRIGGERS   also capture when each first holds, comma-separated:
                        settled, or a metric test like moving<10, sand>=500
                        (metrics: sand, moving, still)
      --until SECONDS   run at most this long (default: the last --at, or 30 with --when)
      --set ID=VALUE    a config value for this run, as Copy values writes it (repeatable)
      --size WxH        lattice size in cells (default 160x100)
      --cell PX         pixels per cell (default 3)
      --tag NAME        written as <scene>-<tag>.png, to keep runs side by side
      --out DIR         where strips go (default target/strips)
  cargo run -p sand -- bench
      measures feel on the CPU reference; compares with target/bench.txt and updates it";

pub fn main(args: &[String]) -> i32 {
    let result = match args[0].as_str() {
        "strip" => strip(&args[1..]),
        _ => bench(),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            2
        }
    }
}

// Strips

struct StripArgs {
    scenes: Vec<&'static Scene>,
    note: String,
    at: Vec<f32>,
    when: Vec<Trigger>,
    until: Option<f32>,
    set: Vec<String>,
    size: (i32, i32),
    cell: u32,
    tag: Option<String>,
    out: PathBuf,
}

fn parse_strip(args: &[String]) -> Result<StripArgs, String> {
    let mut a = StripArgs {
        scenes: Vec::new(),
        note: String::new(),
        at: vec![0.0, 1.0, 3.0, 8.0],
        when: Vec::new(),
        until: None,
        set: Vec::new(),
        size: (160, 100),
        cell: 3,
        tag: None,
        out: PathBuf::from("target/strips"),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--note" => a.note = value()?,
            "--at" => {
                a.at = value()?
                    .split(',')
                    .map(|t| t.trim().parse().map_err(|_| format!("bad time `{t}`")))
                    .collect::<Result<_, _>>()?
            }
            "--when" => a.when = value()?.split(',').map(Trigger::parse).collect::<Result<_, _>>()?,
            "--until" => a.until = Some(value()?.parse().map_err(|_| "bad --until")?),
            "--set" => a.set.push(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or(format!("bad size `{v}`"))?;
                a.size = (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?);
            }
            "--cell" => a.cell = value()?.parse().map_err(|_| "bad --cell")?,
            "--tag" => a.tag = Some(value()?),
            "--out" => a.out = PathBuf::from(value()?),
            "all" => a.scenes.extend(scenes::ALL.iter().filter(|s| s.name != "empty")),
            name if !name.starts_with("--") => {
                let known = scenes::names().join(", ");
                a.scenes.push(scenes::find(name).ok_or(format!("no scene `{name}`; scenes: {known}"))?);
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    if a.scenes.is_empty() {
        return Err("name a scene, or all".into());
    }
    Ok(a)
}

/// One captured moment.
struct Capture {
    phase: u32,
    seconds: f32,
    /// The trigger that caught it, if it wasn't a set time.
    trigger: Option<String>,
    metrics: String,
    image: RgbaImage,
}

fn strip(args: &[String]) -> Result<(), String> {
    let a = parse_strip(args)?;
    let mut tuner: Tuner<Config> = Tuner::unsaved();
    let set: String = a.set.iter().map(|s| s.replacen('=', " = ", 1) + "\n").collect();
    tuner.apply_text(&set);
    let rate = tuner.current.sim_rate;
    let renderer = Renderer::new();
    let font = Font::new();
    std::fs::create_dir_all(&a.out).map_err(|e| e.to_string())?;

    for &scene in &a.scenes {
        let mut run = Run::new(scene, a.size.0, a.size.1);
        let mut times: Vec<(u32, f32)> = a.at.iter().map(|&t| ((t * rate).round() as u32, t)).collect();
        times.sort_by_key(|&(p, _)| p);
        let default_end = if a.when.is_empty() { 0.0 } else { 30.0 };
        let until = a.until.unwrap_or(a.at.iter().copied().fold(default_end, f32::max));
        let end = (until * rate).round() as u32;
        let mut fired: Vec<Option<f32>> = vec![None; a.when.len()];
        let mut captures = Vec::new();

        loop {
            let m = run.metrics();
            let seconds = run.phase as f32 / rate;
            let mut caught: Vec<Option<String>> = Vec::new();
            if times.iter().any(|&(p, _)| p == run.phase) {
                caught.push(None);
            }
            if run.phase > 0 {
                for (i, t) in a.when.iter().enumerate() {
                    if fired[i].is_none() && t.holds(&m) {
                        fired[i] = Some(seconds);
                        caught.push(Some(t.name()));
                    }
                }
            }
            if !caught.is_empty() {
                let image = renderer.render(&run.world, a.cell);
                // A set time and a trigger on the same phase share one frame.
                let trigger = caught.into_iter().flatten().collect::<Vec<_>>().join(", ");
                let trigger = (!trigger.is_empty()).then_some(trigger);
                captures.push(Capture { phase: run.phase, seconds, trigger, metrics: m.line(), image });
            }
            let times_done = times.iter().all(|&(p, _)| p <= run.phase);
            if run.phase >= end || (times_done && fired.iter().all(Option::is_some)) {
                break;
            }
            run.step();
        }

        let mut header = vec![
            (Style::Title, format!("{} — {}", scene.name, scene.about)),
            (Style::Note, a.note.clone()),
            (Style::Detail, {
                let mut d = format!("{}×{} cells · {} phases/s", a.size.0, a.size.1, rate);
                if !a.set.is_empty() {
                    d += &format!(" · {}", a.set.join(", "));
                }
                d
            }),
        ];
        for (t, at) in a.when.iter().zip(&fired) {
            let line = match at {
                Some(s) => format!("{}: at {s:.1} s", t.name()),
                None => format!("{}: not by {until:.0} s", t.name()),
            };
            header.push((Style::Detail, line));
        }
        header.retain(|(_, text)| !text.is_empty());

        let name = match &a.tag {
            Some(tag) => format!("{}-{tag}", scene.name),
            None => scene.name.to_string(),
        };
        let png = a.out.join(format!("{name}.png"));
        compose(&font, &header, &captures).save(&png).map_err(|e| e.to_string())?;
        let mut txt: String = header.iter().map(|(_, t)| format!("{t}\n")).collect();
        for c in &captures {
            let label = c.trigger.as_deref().map(|t| format!(" [{t}]")).unwrap_or_default();
            txt += &format!("{:>6.1} s  phase {:>5}{label}  {}\n", c.seconds, c.phase, c.metrics);
        }
        std::fs::write(png.with_extension("txt"), &txt).map_err(|e| e.to_string())?;
        print!("{}{txt}", format_args!("wrote {}\n", png.display()));
    }
    Ok(())
}

// Rendering a world through the game's render shader

struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipelines: Pipelines,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl Renderer {
    fn new() -> Renderer {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).expect("a GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).expect("a device");
        let pipelines = Pipelines::new(&device, FORMAT);
        Renderer { device, queue, pipelines }
    }

    fn render(&self, world: &sim::World, cell: u32) -> RgbaImage {
        let (width, height) = (world.width() as u32 * cell, (world.height() as u32 * 2 + 1) * cell / 2);
        let lattice = Lattice::new(&self.device, &self.pipelines, world);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("strip frame"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // Rows in a texture-to-buffer copy are padded to 256 bytes.
        let row = width * 4;
        let padded = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("strip readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let px = cell as f32;
        lattice.render(&self.queue, &self.pipelines, &mut encoder, &view, [0.0, 0.0], [px, px]);
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
        let data = slice.get_mapped_range().expect("mapped");
        let mut pixels = Vec::with_capacity((row * height) as usize);
        for y in 0..height as usize {
            pixels.extend_from_slice(&data[y * padded as usize..][..row as usize]);
        }
        RgbaImage::from_raw(width, height, pixels).expect("frame size")
    }
}

// Composing the strip

#[derive(Clone, Copy)]
enum Style {
    Title,
    Note,
    Detail,
}

const PANEL: [u8; 3] = [0x1b, 0x1f, 0x2a];
const BAND: [u8; 3] = [0x14, 0x17, 0x1f];
const BACK: [u8; 3] = [0x0c, 0x0e, 0x13];
const WHITE: [u8; 3] = [0xee, 0xee, 0xf2];
const NOTE: [u8; 3] = [0xf2, 0xc9, 0x6b];
const GRAY: [u8; 3] = [0x9a, 0xa1, 0xb3];
const TRIGGER: [u8; 3] = [0x7f, 0xc8, 0xf5];
const PAD: u32 = 14;
const GAP: u32 = 8;
const BAND_HEIGHT: u32 = 32;

impl Style {
    fn px(self) -> f32 {
        match self {
            Style::Title => 26.0,
            Style::Note => 21.0,
            Style::Detail => 18.0,
        }
    }

    fn color(self) -> [u8; 3] {
        match self {
            Style::Title => WHITE,
            Style::Note => NOTE,
            Style::Detail => GRAY,
        }
    }
}

/// The header patch on top, then each capture as a caption band over its frame.
fn compose(font: &Font, header: &[(Style, String)], captures: &[Capture]) -> RgbaImage {
    let frame_w = captures.iter().map(|c| c.image.width()).max().unwrap_or(0);
    let width = (frame_w + 2 * PAD).max(560);
    let line = |s: Style| (s.px() * 1.45) as u32;
    let header_h = PAD * 2 + header.iter().map(|(s, _)| line(*s)).sum::<u32>();
    let frames_h: u32 = captures.iter().map(|c| BAND_HEIGHT + c.image.height() + GAP).sum();
    let mut img = RgbaImage::from_pixel(width, header_h + GAP + frames_h, rgba(BACK));

    fill(&mut img, 0, 0, width, header_h, PANEL);
    let mut y = PAD;
    for (style, text) in header {
        let h = line(*style);
        font.draw(&mut img, PAD as f32, (y + h) as f32 - style.px() * 0.38, style.px(), text, style.color());
        y += h;
    }

    let mut y = header_h + GAP;
    for c in captures {
        fill(&mut img, 0, y, width, BAND_HEIGHT, BAND);
        let base = (y + BAND_HEIGHT) as f32 - 10.0;
        let mut x = PAD as f32;
        x += font.draw(&mut img, x, base, 20.0, &format!("{:.1} s", c.seconds), WHITE) + 12.0;
        if let Some(t) = &c.trigger {
            font.draw(&mut img, x, base, 18.0, t, TRIGGER);
        }
        let right = font.width(&c.metrics, 16.0);
        font.draw(&mut img, width as f32 - PAD as f32 - right, base, 16.0, &c.metrics, GRAY);
        y += BAND_HEIGHT;
        image::imageops::replace(&mut img, &c.image, ((width - c.image.width()) / 2) as i64, y as i64);
        y += c.image.height() + GAP;
    }
    img
}

fn rgba(c: [u8; 3]) -> Rgba<u8> {
    Rgba([c[0], c[1], c[2], 255])
}

fn fill(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
    for yy in y..(y + h).min(img.height()) {
        for xx in x..(x + w).min(img.width()) {
            img.put_pixel(xx, yy, rgba(c));
        }
    }
}

/// Text, rasterized with fontdue from the engine's font.
struct Font(fontdue::Font);

impl Font {
    fn new() -> Font {
        let bytes = include_bytes!("../assets/Fredoka-SemiBold.ttf") as &[u8];
        Font(fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).expect("font"))
    }

    fn width(&self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.0.metrics(c, px).advance_width).sum()
    }

    /// Draws `text` with its baseline at `baseline`, blending over what's there. Returns its width.
    fn draw(&self, img: &mut RgbaImage, x: f32, baseline: f32, px: f32, text: &str, color: [u8; 3]) -> f32 {
        let mut pen = x;
        for c in text.chars() {
            let (m, coverage) = self.0.rasterize(c, px);
            let left = pen.round() as i64 + m.xmin as i64;
            let top = baseline.round() as i64 - m.height as i64 - m.ymin as i64;
            for gy in 0..m.height {
                for gx in 0..m.width {
                    let a = coverage[gy * m.width + gx] as u32;
                    let (px_, py) = (left + gx as i64, top + gy as i64);
                    if a == 0 || px_ < 0 || py < 0 || px_ >= img.width() as i64 || py >= img.height() as i64 {
                        continue;
                    }
                    let p = img.get_pixel_mut(px_ as u32, py as u32);
                    for (channel, &c) in p.0.iter_mut().zip(&color) {
                        *channel = ((c as u32 * a + *channel as u32 * (255 - a)) / 255) as u8;
                    }
                }
            }
            pen += m.advance_width;
        }
        pen - x
    }
}

// The feel benchmark

/// One measurement of feel: a scene run on a fixed lattice at the shipped sim rate and rules.
struct Case {
    name: &'static str,
    unit: &'static str,
    measure: fn() -> Option<f64>,
}

const SIZE: (i32, i32) = (120, 80);

/// The shipped sim rate, in phases per second.
fn rate() -> f64 {
    Config::default().sim_rate as f64
}

fn run_of(name: &str) -> Run {
    Run::new(scenes::find(name).expect("bench scene"), SIZE.0, SIZE.1)
}

/// Steps `run` until `done` holds or `seconds` pass; returns the seconds taken, if it held.
fn seconds_until(run: &mut Run, seconds: f64, done: impl Fn(&Run) -> bool) -> Option<f64> {
    let start = run.phase;
    let end = start + (seconds * rate()) as u32;
    while run.phase < end {
        run.step();
        if done(run) {
            return Some((run.phase - start) as f64 / rate());
        }
    }
    None
}

const CASES: &[Case] = &[
    Case {
        name: "pour: pile angle",
        unit: "°",
        measure: || {
            let mut run = run_of("pour");
            seconds_until(&mut run, 40.0, |r| r.phase > 20 * scenes::RATE && r.metrics().still >= scenes::RATE)?;
            pile_angle(&run.world)
        },
    },
    Case {
        name: "pour: settles after pouring",
        unit: "s",
        measure: || {
            let mut run = run_of("pour");
            seconds_until(&mut run, 20.0, |_| false);
            seconds_until(&mut run, 60.0, |r| r.metrics().still >= scenes::RATE)
        },
    },
];

fn bench() -> Result<(), String> {
    let path = Path::new("target/bench.txt");
    // Each case's last result: a number, or `never` when it didn't happen.
    let last: Vec<(String, Option<f64>)> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, v) = l.rsplit_once(" = ")?;
            Some((k.to_string(), v.parse().ok()))
        })
        .collect();
    println!("feel, on the CPU reference at {}x{} cells and {} phases/s:\n", SIZE.0, SIZE.1, rate());
    println!("  {:<34} {:>10} {:>10} {:>10}", "", "now", "last", "change");
    let mut saved = String::new();
    for case in CASES {
        let now = (case.measure)();
        let before = last.iter().find(|(k, _)| k == case.name).map(|&(_, v)| v);
        let show = |v: Option<f64>| v.map_or("never".into(), |v| format!("{v:.1} {}", case.unit));
        let change = match (now, before) {
            (Some(n), Some(Some(b))) if (n - b).abs() > 1e-9 => format!("{:+.1}", n - b),
            (_, Some(b)) if b == now => "=".into(),
            (_, Some(_)) => "changed".into(),
            (_, None) => String::new(),
        };
        let before = before.map_or("—".into(), show);
        println!("  {:<34} {:>10} {:>10} {:>10}", case.name, show(now), before, change);
        saved += &format!("{} = {}\n", case.name, now.map_or("never".into(), |n| n.to_string()));
    }
    std::fs::write(path, saved).map_err(|e| e.to_string())?;
    println!("\nsaved to {}", path.display());
    Ok(())
}
