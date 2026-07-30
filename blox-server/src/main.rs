//! blox-server — the I/O shell around `blox-core`.
//!
//! The core has no sockets and no clock, so something has to own them. This is
//! that something, in its smallest honest form: a TCP listener, one reader
//! thread per connection, and **one thread that owns the engine**.
//!
//! ```text
//!   conn 1 reader ─┐
//!   conn 2 reader ─┼─▶ mpsc (the sequencer) ─▶ engine thread ─┬─▶ conn 1 writer
//!   conn 3 reader ─┘                                          ├─▶ conn 2 writer
//!                                                             └─▶ conn 3 writer
//! ```
//!
//! The channel **is** the sequencer from `DESIGN.md` D4. Events race on the
//! wire; the order they come out of this channel is arbitrary but recorded,
//! and everything downstream is a pure fold over that order. The engine itself
//! is single-writer with no locks (D12).
//!
//! Deliberately not here: TLS, auth, WebSocket, framing beyond newlines,
//! persistence. This is a driver for tests and benchmarks, not a production
//! surface — that would add the intent log, risk gate, and conflation from
//! `DESIGN.md` D23–D26.

mod protocol;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread;

use blox_core::*;
use protocol::{change_instrument, format_book, format_change, format_top, parse, Cmd};

/// Bounded so a slow client cannot make the engine allocate without limit.
/// Overflow disconnects that client (`DESIGN.md` D25): disconnect is
/// recoverable, OOM is not.
const CLIENT_QUEUE: usize = 8192;

type ConnId = u64;

enum Msg {
    Connect(ConnId, SyncSender<String>),
    Line(ConnId, String),
    Disconnect(ConnId),
}

struct Conn {
    tx: SyncSender<String>,
    subs: HashSet<InstrumentId>,
    /// Suppress `OK` acks.
    quiet: bool,
    /// Deliver order-lifecycle changes back to the submitter.
    echo: bool,
    /// Orders submitted here, so lifecycle changes route back to their owner.
    orders: HashSet<OrderId>,
}

fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7070".to_string());

    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    // Report the resolved address so a harness can bind port 0 and read it.
    let bound = listener.local_addr().expect("local_addr");
    println!("listening {bound}");
    let _ = std::io::stdout().flush();

    let (tx, rx) = mpsc::channel::<Msg>();
    thread::spawn(move || engine_loop(rx));

    let next_id = Arc::new(AtomicU64::new(1));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let _ = stream.set_nodelay(true);
        let id = next_id.fetch_add(1, Ordering::Relaxed);
        let tx = tx.clone();
        thread::spawn(move || serve(id, stream, tx));
    }
}

/// One connection: a writer thread plus a read loop feeding the sequencer.
fn serve(id: ConnId, stream: TcpStream, tx: Sender<Msg>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let (out_tx, out_rx) = mpsc::sync_channel::<String>(CLIENT_QUEUE);

    let writer = thread::spawn(move || {
        let mut w = BufWriter::new(write_half);
        // Coalesce whatever is already queued into one flush. Under load this
        // turns thousands of tiny writes into a handful of syscalls.
        while let Ok(first) = out_rx.recv() {
            if w.write_all(first.as_bytes()).is_err() || w.write_all(b"\n").is_err() {
                return;
            }
            for more in out_rx.try_iter() {
                if w.write_all(more.as_bytes()).is_err() || w.write_all(b"\n").is_err() {
                    return;
                }
            }
            if w.flush().is_err() {
                return;
            }
        }
    });

    if tx.send(Msg::Connect(id, out_tx)).is_err() {
        return;
    }

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if tx.send(Msg::Line(id, line)).is_err() {
            break;
        }
    }

    let _ = tx.send(Msg::Disconnect(id));
    let _ = writer.join();
}

/// The single writer. Owns the engine; nothing else touches it.
fn engine_loop(rx: Receiver<Msg>) {
    let mut engine = Engine::new();
    let mut conns: HashMap<ConnId, Conn> = HashMap::new();
    let mut changes: Vec<Change> = Vec::with_capacity(1024);
    let mut applied: u64 = 0;
    let mut errors: u64 = 0;
    // Dead clients, collected after the borrow of `conns` ends.
    let mut drop_list: Vec<ConnId> = Vec::new();

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Connect(id, tx) => {
                conns.insert(
                    id,
                    Conn {
                        tx,
                        subs: HashSet::new(),
                        quiet: false,
                        echo: true,
                        orders: HashSet::new(),
                    },
                );
            }
            Msg::Disconnect(id) => {
                conns.remove(&id);
            }
            Msg::Line(id, line) => {
                let cmd = match parse(&line) {
                    Ok(c) => c,
                    Err(e) => {
                        errors += 1;
                        send(&mut conns, &mut drop_list, id, format!("ERR {e}"));
                        reap(&mut conns, &mut drop_list);
                        continue;
                    }
                };

                match cmd {
                    Cmd::Event(ev) => {
                        // Remember which connection owns which order id, so
                        // fills and rejects route back to the submitter.
                        if let Some(c) = conns.get_mut(&id) {
                            match &ev {
                                Event::New { id: oid, .. } => {
                                    c.orders.insert(*oid);
                                }
                                Event::PlaceTrigger { trigger } => {
                                    c.orders.insert(trigger.id);
                                }
                                _ => {}
                            }
                        }

                        changes.clear();
                        engine.apply_into(&ev, &mut changes);
                        applied += 1;

                        fan_out(&mut conns, &mut drop_list, id, &changes);

                        let quiet = conns.get(&id).map(|c| c.quiet).unwrap_or(true);
                        if !quiet {
                            send(&mut conns, &mut drop_list, id, "OK".to_string());
                        }
                    }
                    Cmd::Top(i) => {
                        let msg = format_top(i, &engine.top_of(i));
                        send(&mut conns, &mut drop_list, id, msg);
                    }
                    Cmd::Book(i, depth) => {
                        let msg = format_book(&engine.aggregate(i, depth.min(100)));
                        send(&mut conns, &mut drop_list, id, msg);
                    }
                    Cmd::Sub(i) => {
                        if let Some(c) = conns.get_mut(&id) {
                            c.subs.insert(i);
                        }
                        send(&mut conns, &mut drop_list, id, "OK".to_string());
                    }
                    Cmd::Unsub(i) => {
                        if let Some(c) = conns.get_mut(&id) {
                            c.subs.remove(&i);
                        }
                        send(&mut conns, &mut drop_list, id, "OK".to_string());
                    }
                    Cmd::Quiet(on) => {
                        if let Some(c) = conns.get_mut(&id) {
                            c.quiet = on;
                        }
                        // Always acked, so a client can await the switch.
                        send(&mut conns, &mut drop_list, id, "OK".to_string());
                    }
                    Cmd::Echo(on) => {
                        if let Some(c) = conns.get_mut(&id) {
                            c.echo = on;
                        }
                        send(&mut conns, &mut drop_list, id, "OK".to_string());
                    }
                    Cmd::Ping(tok) => {
                        send(&mut conns, &mut drop_list, id, format!("PONG {tok}"));
                    }
                    Cmd::Stats => {
                        let msg = format!(
                            "STATS applied={applied} errors={errors} books={} conns={} dropped_deltas={}",
                            engine.book_count(),
                            conns.len(),
                            engine.dropped_deltas()
                        );
                        send(&mut conns, &mut drop_list, id, msg);
                    }
                    Cmd::Check => {
                        let msg = match engine.check() {
                            Ok(()) => "OK invariants hold".to_string(),
                            Err(e) => format!("ERR INVARIANT {e}"),
                        };
                        send(&mut conns, &mut drop_list, id, msg);
                    }
                    Cmd::Close => {
                        conns.remove(&id);
                    }
                }
                reap(&mut conns, &mut drop_list);
            }
        }
    }
}

/// Route changes: instrument-scoped ones to subscribers, order lifecycle back
/// to whoever submitted the order.
fn fan_out(
    conns: &mut HashMap<ConnId, Conn>,
    drop_list: &mut Vec<ConnId>,
    origin: ConnId,
    changes: &[Change],
) {
    for c in changes {
        let text = format_change(c);
        match change_instrument(c) {
            Some(inst) => {
                for (id, conn) in conns.iter() {
                    if conn.subs.contains(&inst) {
                        try_push(drop_list, *id, conn, &text);
                    }
                }
            }
            None => {
                let owner = order_id_of(c)
                    .and_then(|oid| {
                        conns
                            .iter()
                            .find(|(_, c)| c.orders.contains(&oid))
                            .map(|(id, _)| *id)
                    })
                    .unwrap_or(origin);
                if let Some(conn) = conns.get(&owner) {
                    // A client that pipelines without reading would otherwise
                    // fill its own queue and be disconnected as a slow
                    // consumer — correct behaviour (D25), but it makes
                    // "submit as fast as possible" impossible to express.
                    if conn.echo {
                        try_push(drop_list, owner, conn, &text);
                    }
                }
            }
        }
    }
}

fn order_id_of(c: &Change) -> Option<OrderId> {
    match c {
        Change::Accepted { id }
        | Change::Rejected { id, .. }
        | Change::Filled { id, .. }
        | Change::Cancelled { id, .. }
        | Change::Amended { id, .. }
        | Change::TriggerPlaced { id }
        | Change::TriggerFired { id, .. }
        | Change::TriggerCancelled { id, .. } => Some(*id),
        _ => None,
    }
}

fn send(conns: &mut HashMap<ConnId, Conn>, drop_list: &mut Vec<ConnId>, id: ConnId, text: String) {
    if let Some(conn) = conns.get(&id) {
        try_push(drop_list, id, conn, &text);
    }
}

/// Never blocks the engine on a client. A full queue means the client cannot
/// keep up; mark it for disconnect rather than buffering without limit.
fn try_push(drop_list: &mut Vec<ConnId>, id: ConnId, conn: &Conn, text: &str) {
    match conn.tx.try_send(text.to_string()) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            eprintln!("conn {id}: too slow, disconnecting");
            drop_list.push(id);
        }
        Err(TrySendError::Disconnected(_)) => drop_list.push(id),
    }
}

fn reap(conns: &mut HashMap<ConnId, Conn>, drop_list: &mut Vec<ConnId>) {
    for id in drop_list.drain(..) {
        conns.remove(&id);
    }
}
