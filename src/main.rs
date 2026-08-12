// This binary exposes reusable parity adapters whose public APIs are exercised
// independently and intentionally not all called by the default dashboard.
#![allow(dead_code)]

mod analytics;
mod app_server;
mod cli;
mod collector;
mod config;
mod doctor;
mod interactive;
mod models;
mod network;
mod otlp;
mod pricing;
mod proxy;
mod quota;
mod remote;
mod storage;
mod tui;

fn main() {
    if let Err(error) = cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(cli::error_exit_code(&error));
    }
}
