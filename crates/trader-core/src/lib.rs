//! Indicators, strategies, sizing and risk limits for the monkeytrading bot.
//!
//! This crate does no IO: no clocks, randomness, files, environment, or network. Time and prices
//! arrive as arguments, so strategies are tested and benchmarked without a network, and a recorded
//! log replays through it to the same decisions.

pub mod bars;
pub mod marks;
pub mod sma;
pub mod strategy;
