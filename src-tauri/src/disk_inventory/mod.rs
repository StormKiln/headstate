//! Bounded, read-only disk discovery. Visibility never grants deletion authority.
pub mod accounting;
pub mod commands;
pub mod discovery;
pub mod measure;
pub mod model;
pub mod platform;
mod process;

#[cfg(test)]
mod tests;
