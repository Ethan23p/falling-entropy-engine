# AGENTS.md

Falling Entropy Engine: Ethan's falling-sand game, v2, in Rust + WebGPU. It grows one mechanic at a time on cellular-automaton primitives. v1 is [Phosphor-Sand](https://github.com/Ethan23p/Phosphor-Sand); borrow its dynamics freely. This file holds standing rules only, no status.

Treat this engine moreso as a tiny universe with real physics than a toy. Every behavior has to emerge from strict locality, self-writing cells, order-independent updates, and conserved quantities. If a given solution seems good but violates any of those, find a physically honest one, even if it's more difficult.

## Sources of truth

- **The design doc is the authority:** Ethan's Logseq page [[Falling Entropy Engine]] (graph Logseq-DB-Aurelius), read on his PC (`C:\Caesar-General`) with the logseq-interface skill at `--level 50`. Use its vocabulary in code and tests. If the code disagrees, the doc wins; if the doc is silent, ask Ethan.
- **The git log is the progress tracker.** Prefix commits with the version, e.g. `v01.00: sand overwrites air`.

## Working with Ethan

Ethan designs; Claude engineers (code, tooling, verification).
- Implement the doc as written. Add no constraint it doesn't state; how a stage does its job is Claude's to fill, not a gap to flag.
- Design decisions (physics, materials, rules, feel) are settled with Ethan: bring two or three options and a recommendation.
- Behavior is found, not prescribed: set up the causes and let the sim judge.
- Ask before pushing or merging (merging to `main` deploys). A push goes on its own branch, through a PR Claude opens and, once Ethan agrees, merges.

## Workflow

1. Read what the doc says about the mechanic; settle open questions.
2. Write failing tests named after what they pin (`propose_*`, `evaluate_*`, `resolve_*`, `conservation_*`). Invariants (conservation on published state, order independence, one cell per phase) always stay; behavior tests are instruments, so report when an outcome moves.
3. Implement until they pass, then look at a strip.
4. Make it playable on desktop and phone (Ethan plays on a phone). Commit.

## Architecture

- `crates/sim` is the CPU reference (no dependencies) and holds every rule first. `apps/sand/src/sim.wgsl` mirrors it bit for bit; the GPU parity test checks that.
- Every write is a self-write: a cell affects a neighbor only through a contract the neighbor answers, and reads only published state. Randomness comes only from the hash of position, phase and salt.
- Tunables live in the config (`Config` in `apps/sand/src/main.rs`, shown in the engine's config panel); its `Default` is the shipped values.
- The engine crate comes from contra-ethos-games, pinned by `rev`. The app builds only on Ethan's PC, whose gitignored `.cargo/config.toml` patches in his local engine branch; the cloud can test `sim` and `scenes`.

## Commands

- `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --all` (120 columns).
- GPU parity test: `cargo test -p sand -- --ignored`.
- Strips: `cargo run -p sand -- strip SCENE... [--at 0,1,3,8] [--set ID=VALUE] [--tag NAME] [--note "..."]` writes `target/strips/`. `--set` takes numbers (toggles are 0 or 1).
- Feel bench: `cargo run -p sand -- bench`.
- Game: `cargo run -p sand [-- --scene NAME]`; web: `trunk serve` in `apps/sand/`.
- Deploy: every push to `main` builds the web app (`.github/workflows/pages.yml`) and publishes it to sand.contra-ethos.com, with Phosphor Sand at `/phosphor/`. Merging to `main` is deploying.
- Keep runs fast and give each a timeout; a run that hangs is a bug.
- WGSL rejects reserved words (`target`) and names that shadow built-ins (`read`).
