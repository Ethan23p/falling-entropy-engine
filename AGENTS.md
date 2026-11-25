# AGENTS.md

Falling Sand Engine is v2 of Ethan's falling-sand game, in Rust + WebGPU. It grows one mechanic (or feature: the same thing) at a time on cellular-automaton primitives, from a minimal base toward field-mediated matter. It will be served by Ethan's games platform, [contra-ethos-games](https://github.com/Ethan23p/contra-ethos-games), but lives in its own repo. v1 is [Phosphor-Sand](https://github.com/Ethan23p/Phosphor-Sand) (plain JS + WGSL, at sand.contra-ethos.com); borrow its dynamics freely.

This file is for orientation and standing rules: how we work, the commands and the architecture. It holds no status or history: what is being built lives in the design doc, and what has been built lives in the git log.

## Sources of truth

1. **The design doc is the authority:** Ethan's Logseq page [[Falling Entropy Engine]] (graph Logseq-DB-Aurelius). It describes the whole self-consistent system Ethan is aiming for; each version builds a piece of it, as its Policies section says. Read it through a Remote Control session on Ethan's PC (folder `C:\Caesar-General`) with the logseq-interface skill, pulling the full depth (`--level 50`). Its sections:
   - **Scope**: the Entities, Capabilities and Policies, including what constrains the current version. Names in the code and tests come from here: lattice, cell, particle, material, mass, velocity, phase, published state, contract (offer, request), selection, physics distribution; the update's stages read, resolve, evaluate, evolve, publish, propose.
   - **Technical Specification**: the toy cases.
   - If the code and the design doc disagree, the doc wins. If it is silent, ask Ethan.
2. **The git log is the progress tracker.** Each commit is prefixed with the version it belongs to, e.g. `v01.00: a column of sand falls and rests`.
3. **The Claude Doc** [Understanding Cellular Automata](https://claude.ai/artifact/QzVdJFygvriiRbGSj1xnau) is for collaboration and coordination, not a source of truth: notes that don't fit here, batches of questions for Ethan. Use it with the Claude Docs connector (load the docs skill first).

## Roles and decisions

Ethan is the designer and owns the design doc. Claude is the engineer and owns the code, tooling and verification.

- **Every design decision is open until settled with Ethan.** That covers physics, materials, rules, geometry, conservation and feel. Bring them as two or three concrete options with a recommendation.
- **Implement the design doc as written.** Add no constraint it doesn't state, and don't flag implementation details (how a stage does its job) as gaps: they are Claude's to fill.
- **Behavior is found, not prescribed.** Set up the causes (material properties, contracts, the physics distribution) and watch what emerges; intuitions about outcomes are often wrong, and the sim is the judge.
- Engineering calls (crate layout, encodings, test harness, tooling) are Claude's. A refinement within a settled design can be built first and raised in the wrap-up.

## Workflow

Each version targets one mechanic:

1. Read what the design doc says about the mechanic, and resolve open questions with Ethan. A new model is proved on tiny scenarios (the doc's toy cases) before it carries the game.
2. Write failing tests, as many per entity, capability or policy as it needs, named after what they pin (`propose_*`, `evaluate_*`, `resolve_*`, `conservation_*`). Invariant tests (conservation, order independence, the speed of information) are hard checks and always stay. Behavior tests are measuring tools: when a mechanic changes on purpose and an outcome moves, report it rather than treat it as a bug. When a version replaces a piece of the model, the tests that pinned the old one are rewritten or retired, not satisfied. Conservation is measured on published state; a cell's state mid-update can look duplicated while a self-write is in transit.
3. Implement until they pass.
4. Make it playable on desktop and phone, even before it's fun. Ethan plays on a phone.
5. Commit, with the version prefix.

Adversarial review comes in batches, not every version: a subagent tries to break everything changed since the `reviewed` git tag (conservation, ambiguous contracts, order independence, stuck states, encoding). Run it before each push or deploy, after at most three versions, and right away when a version changes the foundations. Then move the tag: `git tag -f reviewed`.

Keep it low-key:
- Tests must stay fast. Compile times must stay low: few dependencies, and the sim crate has none.
- The foundations (synchrony, contracts, rendering) get the most care; everything builds on them.
- Context is a budget. Read only what the step needs; re-read only the parts of the design doc that changed. Send wide reading, searching and surveys to subagents, which return only their conclusions.
- Write code with the file-editing tools, not shell heredocs: the Bash tool's heredocs break on apostrophes.
- Keep this file accurate when a rule or a command changes. Never put status here.

## Architecture rules

- **Rules live in plain Rust first.** `crates/sim` is the CPU reference simulation: cell state, rules, the counter-based hash. It has no GPU or platform dependencies and is what `cargo test` exercises.
- **GPU code mirrors the reference.** The WGSL compute shaders implement the same rules bit for bit. A few headless GPU tests, marked `#[ignore]` so normal runs skip them, check that the shaders match the reference.
- **Every write is a self-write**: a cell affects a neighbor only by a contract the neighbor answers, and reads only its neighbors' published state. Results never depend on evaluation order, and information travels at most one cell per phase. Randomness comes only from the hash of position, phase and salt; never from thread IDs or a sequential RNG inside the sim.
- **The shared engine** (window, frame loop, input, config panel) is the `engine` crate from contra-ethos-games, used as a git dependency pinned to a `rev`. To develop both repos together, create a gitignored `.cargo/config.toml`:

  ```toml
  [patch."https://github.com/Ethan23p/contra-ethos-games"]
  engine = { path = "../contra-ethos-games/crates/engine" }
  ```

  Commit engine changes in contra-ethos-games first, then bump the `rev` here.
- **The config is the one source of truth for tunables**, through the engine's `Tunable` and config panel. Its `Default` is the shipped values. Expose anything with a feel worth tweaking: Ethan tunes there, often from his phone, and sends values back with Copy values.

## Commands

From the repo root:
- Tests: `cargo test --workspace`. Lint: `cargo clippy --workspace --all-targets`. Format: `cargo fmt --all` (120 columns).
- GPU parity test (needs a GPU): `cargo test -p sand -- --ignored`.
- Native window: `cargo run -p sand [-- --scene NAME]`; on the web, `?scene=NAME`. Scenes are presets shared by the game (the Scene choice in the config panel) and the harness. Copy scene, in the config panel, copies the world as a painting's text; save one Ethan sends as `crates/scenes/painted/NAME.fse` and list it in `scenes::ALL` with `Build::Painted(include_str!(...))`.
- Strips: `cargo run -p sand -- strip SCENE... [--note "a couple words"] [--at 0,1,3,8] [--when settled,moving<10] [--set ID=VALUE]` renders a vertical strip of frames, with a header and per-frame metrics, to `target/strips/<scene>.png` and `.txt`. `strip all` does every scene; `strip` alone prints the options. Look at one after any sim or render change.
- Feel bench: `cargo run -p sand -- bench` measures feel (pile angle, settling) on the CPU reference and compares with the last run, saved in `target/bench.txt`. Run it before and after a change to a rule. It's an instrument, not a target: when a version changes the rules on purpose, report what moved and why, and let Ethan judge feel in play. Nothing is tuned back toward an earlier version's numbers.
- Web: `trunk serve` from `apps/sand/` (http://127.0.0.1:8770); the `sand` entry in `.claude/launch.json` does the same for the in-app browser. A hidden browser pane throttles to a few frames per second, so judge speed only with the pane shown.

Running them, with contingencies:
- Expect competent speeds: the sim tests take about a second and a warm build about 15. A test that waits on the world (settling, a scene reaching some state) caps its phases and fails with `world.ascii()` instead of hanging.
- Give every command a timeout of a few times its expected run (two minutes for the tests). A run that blows its timeout is a bug to find, not a timeout to raise.
- Long runs (cold builds, `strip all`, `bench`, the GPU parity test) go in the background and print progress as they go, a line per scene or case, so a stall shows. Add that output to any harness command that lacks it.

Layout: `crates/sim` (the CPU reference; its `debug` module draws a world as text, true to the hex offset, for failing assertions: `world.ascii()`, `debug::diff`), `crates/scenes` (preset scenes, scripted brush input and the text format for painted scenes; `run` steps a scene on the CPU reference and measures it), `apps/sand` (the game: `gpu.rs` buffers and passes, `sim.wgsl` mirroring the reference, `render.wgsl` the observer; natively, `harness.rs` holds strips and the bench).

## Environment

- Windows, with PowerShell 7 and Git Bash. Rust stable, plus the `wasm32-unknown-unknown` target and Trunk for web builds.
- Commit when the change is one Ethan asked for. Don't push or deploy without asking.
- wgpu 30 differs from most examples online; the contra-ethos-games `CLAUDE.md` lists the gotchas we've already hit.
