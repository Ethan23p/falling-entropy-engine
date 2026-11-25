//! The CPU reference simulation. Every rule lives here first, in plain Rust, and is tested
//! natively; the WGSL compute shaders mirror it bit for bit.
//!
//! The modules follow the design doc ([[Falling Entropy Engine]]), concept by concept:
//!
//! | Doc | Module |
//! |---|---|
//! | Lattice | `lattice`: the hexagonal geometry, directions and neighbors |
//! | Cell (position, phase, live particle, live contracts) | `cell`: what a cell publishes; `world`: the cells as stored |
//! | Particle, material properties | `particle`: materials (vacuum is nothing), velocity |
//! | Contract (terms: conditions paired with effects) | `contract` |
//! | Update: read, resolve, evaluate, evolve, publish, propose | `update` |
//! | Synchronizer (phase) | `world`: `World::update`, `World::step` |
//! | Selection | `selection` |
//! | Physics distribution, Movement, Gravity, material properties | `physics`: data (per-material properties, gravity) and pure functions only |
//! | Brush | `brush` |
//!
//! Randomness (`random`) comes only from the hash of a cell's position, its phase and a salt.

mod brush;
mod cell;
pub mod contract;
pub mod debug;
mod lattice;
mod particle;
pub mod physics;
mod random;
pub mod selection;
mod update;
mod world;

pub use cell::State;
pub use lattice::{Dir, neighbor};
pub use particle::{Material, Velocity};
pub use physics::Physics;
pub use random::hash;
pub use update::next;
pub use world::{PHASE_MASK, WORDS, World};

#[cfg(test)]
mod tests;
