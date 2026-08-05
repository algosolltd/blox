mod action;
mod agent;
mod dashboard;
mod features;
mod model;
mod server;
mod train;

use std::sync::{Arc, Mutex};
use std::env;
use std::thread;
use std::time::Duration;

fn main() {
    let port: u16 = env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(9090);
    let blox_server = env::var("BLOX_SERVER").unwrap_or_else(|_| "127.0.0.1:7070".into());
    let mut state = dashboard::AppState::new();
    state.blox_server_addr = blox_server;
    let state = Arc::new(Mutex::new(state));

    // Watchdog: keep blox-sim alive for continuous price data
    let addr = state.lock().unwrap().blox_server_addr.clone();
    thread::spawn(move || loop {
        let child = std::process::Command::new("C:\\dev\\blox\\target\\release\\blox-sim.exe")
            .arg("-addr")
            .arg(&addr)
            .spawn();
        if let Ok(mut c) = child {
            let _ = c.wait();
        }
        thread::sleep(Duration::from_secs(2));
    });

    server::run_server(state, port);
}
