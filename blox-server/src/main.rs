mod engine;
mod protocol;
use engine::{Engine, MAX_REQUEST_ID_LEN};
use protocol::{encode, Event, Reply, Request, WireChange, SCHEMA_VERSION};
use std::{
    collections::{HashMap, HashSet},
    io::{self, BufRead, BufReader, BufWriter, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
};
const CLIENT_QUEUE: usize = 8192;
const ENGINE_QUEUE: usize = 65_536;
const MAX_FRAME_BYTES: usize = 64 * 1024;
type ConnId = u64;
enum Msg {
    Connect(ConnId, SyncSender<String>, TcpStream),
    Line(ConnId, String),
    Disconnect(ConnId),
}
struct Client {
    tx: SyncSender<String>,
    subs: HashSet<String>,
    socket: TcpStream,
}
fn main() {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:7070".into());
    let listener = TcpListener::bind(&addr).unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    println!("listening {}", listener.local_addr().unwrap());
    let _ = std::io::stdout().flush();
    let (tx, rx) = mpsc::sync_channel(ENGINE_QUEUE);
    thread::spawn(move || engine_loop(rx));
    for (id, stream) in listener.incoming().flatten().enumerate() {
        let tx = tx.clone();
        thread::spawn(move || serve(id as u64 + 1, stream, tx));
    }
}
fn serve(id: ConnId, stream: TcpStream, tx: SyncSender<Msg>) {
    let write = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let (out_tx, out_rx) = mpsc::sync_channel(CLIENT_QUEUE);
    let control = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    thread::spawn(move || {
        let mut w = BufWriter::new(write);
        while let Ok(line) = out_rx.recv() {
            if writeln!(w, "{line}").and_then(|_| w.flush()).is_err() {
                break;
            }
        }
    });
    if tx.send(Msg::Connect(id, out_tx, control)).is_err() {
        return;
    }
    let mut reader = BufReader::new(stream);
    while let Ok(Some(line)) = read_frame(&mut reader) {
        if tx.send(Msg::Line(id, line)).is_err() {
            break;
        }
    }
    let _ = tx.send(Msg::Disconnect(id));
}
fn engine_loop(rx: Receiver<Msg>) {
    let mut engine = Engine::default();
    let mut clients: HashMap<ConnId, Client> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Connect(id, tx, socket) => {
                clients.insert(
                    id,
                    Client {
                        tx,
                        subs: HashSet::new(),
                        socket,
                    },
                );
            }
            Msg::Disconnect(id) => {
                clients.remove(&id);
            }
            Msg::Line(id, line) => {
                let req: Request = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(e) => {
                        send(
                            &mut clients,
                            id,
                            encode(
                                "",
                                Reply::Error {
                                    message: e.to_string(),
                                },
                            ),
                        );
                        continue;
                    }
                };
                if req.schema_version != SCHEMA_VERSION {
                    send(
                        &mut clients,
                        id,
                        encode(
                            &req.request_id,
                            Reply::Error {
                                message: "unsupported schema_version".into(),
                            },
                        ),
                    );
                    continue;
                }
                if req.request_id.len() > MAX_REQUEST_ID_LEN {
                    send(
                        &mut clients,
                        id,
                        encode(
                            "",
                            Reply::Error {
                                message: format!(
                                    "request_id exceeds maximum {MAX_REQUEST_ID_LEN} bytes"
                                ),
                            },
                        ),
                    );
                    continue;
                }
                let rid = req.request_id.clone();
                let result = engine.execute(req.command);
                if let Some(book_id) = result.subscribe {
                    if let Some(client) = clients.get_mut(&id) {
                        client.subs.insert(book_id);
                    }
                }
                send(&mut clients, id, encode(&rid, result.reply));
                if let Some((book_id, changes)) = result.events {
                    broadcast(&mut clients, &book_id, &changes);
                }
            }
        }
    }
}
fn send(clients: &mut HashMap<ConnId, Client>, id: ConnId, line: String) {
    if clients
        .get(&id)
        .is_some_and(|c| c.tx.try_send(line).is_err())
    {
        drop_client(clients, id);
    }
}
fn broadcast(clients: &mut HashMap<ConnId, Client>, book_id: &str, changes: &[blox_core::Change]) {
    let lines: Vec<_> = changes
        .iter()
        .map(|c| {
            serde_json::to_string(&Event {
                r#type: "event",
                book_id,
                change: WireChange::from(c),
            })
            .unwrap()
        })
        .collect();
    clients.retain(|_, c| {
        if !c.subs.contains(book_id) {
            return true;
        }
        let keep = lines.iter().all(|l| c.tx.try_send(l.clone()).is_ok());
        if !keep {
            let _ = c.socket.shutdown(Shutdown::Both);
        }
        keep
    });
}

fn drop_client(clients: &mut HashMap<ConnId, Client>, id: ConnId) {
    if let Some(client) = clients.remove(&id) {
        let _ = client.socket.shutdown(Shutdown::Both);
    }
}

fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if frame.is_empty() {
                return Ok(None);
            }
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if frame.len() + take > MAX_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "frame too large",
            ));
        }
        frame.extend_from_slice(&available[..take]);
        reader.consume(take);
        if frame.last() == Some(&b'\n') {
            frame.pop();
            break;
        }
    }
    if frame.last() == Some(&b'\r') {
        frame.pop();
    }
    String::from_utf8(frame)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_reader_accepts_json_line_and_rejects_oversize() {
        let mut valid = BufReader::new(&b"{}\r\n"[..]);
        assert_eq!(read_frame(&mut valid).unwrap().as_deref(), Some("{}"));
        let oversized = vec![b'x'; MAX_FRAME_BYTES + 1];
        let mut invalid = BufReader::new(oversized.as_slice());
        assert_eq!(
            read_frame(&mut invalid).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
