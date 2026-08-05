use crate::dashboard::{AppState, connect_to_server, start_training};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

pub fn run_server(state: Arc<Mutex<AppState>>, port: u16) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).expect("Failed to bind");
    println!("blox-agent dashboard → http://{}", addr);

    connect_to_server(&state);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || handle_client(stream, state));
            }
            Err(e) => eprintln!("Connection error: {}", e),
        }
    }
}

fn handle_client(mut stream: TcpStream, state: Arc<Mutex<AppState>>) {
    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buf[..n]);
    let (method, path, _) = parse_request(&request);

    let response = match (method, path) {
        ("GET", "/") => serve_html(),
        ("GET", "/api/agents") => serve_json(&json_agents(&state)),
        ("GET", "/api/extractors") => serve_json(&json_extractors(&state)),
        ("GET", "/api/models") => serve_json(&json_models(&state)),
        ("GET", "/api/status") => serve_json(&json_status(&state)),
        ("GET", "/api/runs") => serve_json(&json_runs(&state)),
        ("GET", "/api/results") => serve_json(&json_results(&state)),
        ("GET", "/api/prices") => serve_json(&json_prices(&state)),
        ("GET", path) if path.starts_with("/api/run/") => serve_json(&json_run(&state, path)),
        ("POST", "/api/start") => {
            let s = state.lock().unwrap();
            if !s.training {
                drop(s);
                start_training(&state);
                serve_json("{\"status\":\"started\"}")
            } else {
                serve_json("{\"status\":\"already running\"}")
            }
        }
        ("POST", "/api/stop") => {
            let mut s = state.lock().unwrap();
            s.training = false;
            serve_json("{\"status\":\"stopped\"}")
        }
        _ => ("HTTP/1.1 404 NOT FOUND\r\nContent-Length: 0\r\n\r\n".to_string(), Vec::new()),
    };

    let mut resp = Vec::new();
    resp.extend_from_slice(response.0.as_bytes());
    resp.extend_from_slice(&response.1);
    let _ = stream.write_all(&resp);
}

fn parse_request(req: &str) -> (&str, &str, &str) {
    let lines: Vec<&str> = req.lines().collect();
    if lines.is_empty() {
        return ("", "", "");
    }
    let parts: Vec<&str> = lines[0].split_whitespace().collect();
    if parts.len() >= 3 {
        (parts[0], parts[1], parts[2])
    } else {
        ("", "", "")
    }
}

fn serve_html() -> (String, Vec<u8>) {
    let html = DASHBOARD_HTML.as_bytes();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        html.len()
    );
    (headers, html.to_vec())
}

fn serve_json(body: &str) -> (String, Vec<u8>) {
    let bytes = body.as_bytes();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        bytes.len()
    );
    (headers, bytes.to_vec())
}

fn json_agents(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    let agents: Vec<serde_json::Value> = s.agents.iter().map(|a| {
        serde_json::json!({ "name": a, "status": if s.training { "training" } else { "idle" } })
    }).collect();
    serde_json::to_string(&agents).unwrap_or("[]".into())
}

fn json_extractors(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    serde_json::to_string(&s.extractors).unwrap_or("[]".into())
}

fn json_models(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    serde_json::to_string(&s.models).unwrap_or("[]".into())
}

fn json_status(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    let active = s.runs.iter().filter(|r| !r.completed).count();
    let done = s.runs.iter().filter(|r| r.completed).count();
    let total = s.agents.len() * s.extractors.len() * s.models.len();
    serde_json::json!({
        "training": s.training,
        "active": active,
        "completed": done,
        "total": total,
        "price": s.last_trade_price,
        "price_count": s.prices.len(),
        "connected": s.connected,
        "server": s.blox_server_addr,
        "debug_buys": s.debug_buys,
        "debug_sells": s.debug_sells,
        "debug_holds": s.debug_holds,
    }).to_string()
}

fn json_runs(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    let runs: Vec<serde_json::Value> = s.runs.iter().map(|r| {
        serde_json::json!({
            "agent": r.agent_name,
            "features": r.feature_name,
            "model": r.model_name,
            "episode": r.episode,
            "reward": format!("{:.2}", r.reward),
            "pnl": format!("{:.2}", r.pnl),
            "sharpe": format!("{:.2}", r.sharpe),
            "completed": r.completed,
        })
    }).collect();
    serde_json::to_string(&runs).unwrap_or("[]".into())
}

fn json_results(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    let mut results: Vec<serde_json::Value> = s.results.iter().map(|(a, f, m, met)| {
        serde_json::json!({
            "agent": a, "features": f, "model": m,
            "pnl": format!("{:.0}", met.total_pnl),
            "sharpe": format!("{:.2}", met.sharpe),
            "win_rate": format!("{:.1}%", met.win_rate * 100.0),
            "trades": met.trade_count,
        })
    }).collect();
    results.sort_by(|a, b| {
        let ap: f64 = a["pnl"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        let bp: f64 = b["pnl"].as_str().unwrap_or("0").parse().unwrap_or(0.0);
        bp.partial_cmp(&ap).unwrap()
    });
    serde_json::to_string(&results).unwrap_or("[]".into())
}

fn json_run(state: &Arc<Mutex<AppState>>, path: &str) -> String {
    let idx: usize = path.trim_start_matches("/api/run/").parse().unwrap_or(9999);
    let s = state.lock().unwrap();
    if let Some(r) = s.runs.get(idx) {
        serde_json::json!({
            "agent": r.agent_name,
            "features": r.feature_name,
            "model": r.model_name,
            "episode": r.episode,
            "reward": format!("{:.2}", r.reward),
            "pnl": format!("{:.2}", r.pnl),
            "sharpe": format!("{:.2}", r.sharpe),
            "completed": r.completed,
            "equity_curve": r.equity_curve,
            "ep_rewards": r.ep_rewards,
        }).to_string()
    } else {
        "{}".to_string()
    }
}

fn json_prices(state: &Arc<Mutex<AppState>>) -> String {
    let s = state.lock().unwrap();
    let prices: Vec<f64> = s.prices.iter().rev().take(200).copied().rev().collect();
    serde_json::to_string(&prices).unwrap_or("[]".into())
}

const DASHBOARD_HTML: &str = include_str!("dashboard.html");
