//! Shared Codex Meter core.
//!
//! The CLI and desktop application both use this crate. Keeping collection,
//! normalization, pricing, privacy filtering, and SQLite queries here prevents
//! the desktop UI from becoming a second implementation of the product.

#![allow(dead_code)]

pub mod analytics;
pub mod app_server;
pub mod cli;
pub mod collector;
pub mod config;
pub mod desktop;
pub mod doctor;
pub mod interactive;
pub mod models;
pub mod network;
pub mod otlp;
pub mod pricing;
pub mod process_command;
pub mod proxy;
pub mod quota;
pub mod remote;
pub mod storage;
pub mod tui;
