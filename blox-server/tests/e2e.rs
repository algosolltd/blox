use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    process::{Command, Stdio},
    time::Duration,
};

fn spawn_server() -> (std::process::Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_blox-server"))
        .arg("127.0.0.1:0")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    let addr = line.trim().strip_prefix("listening ").unwrap().to_owned();
    (child, addr)
}

fn round_trip(stream: &mut TcpStream, reader: &mut BufReader<TcpStream>, request: &str) -> String {
    writeln!(stream, "{request}").unwrap();
    stream.flush().unwrap();
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    response
}

#[test]
fn json_lines_server_matches_and_streams_a_trade() {
    let (mut child, addr) = spawn_server();
    let mut line = String::new();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    for request in [
        r#"{"schema_version":1,"request_id":"1","command":"create_book","book_id":"x"}"#,
        r#"{"schema_version":1,"request_id":"2","command":"subscribe","book_id":"x"}"#,
        r#"{"schema_version":1,"request_id":"3","command":"submit","book_id":"x","id":1,"side":"sell","qty":2,"kind":{"kind":"limit","price":100}}"#,
        r#"{"schema_version":1,"request_id":"4","command":"submit","book_id":"x","id":2,"side":"buy","qty":2,"kind":{"kind":"market"}}"#,
    ] {
        writeln!(stream, "{request}").unwrap();
    }
    stream.flush().unwrap();
    let mut saw_trade = false;
    for _ in 0..12 {
        line.clear();
        if reader.read_line(&mut line).is_err() {
            break;
        }
        if line.contains("\"kind\":\"trade\"") {
            saw_trade = true;
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(saw_trade, "no trade event received");
}

#[test]
fn duplicate_create_preserves_state_and_rejections_are_correlated() {
    let (mut child, addr) = spawn_server();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    assert!(round_trip(
        &mut stream,
        &mut reader,
        r#"{"schema_version":1,"request_id":"1","command":"create_book","book_id":"x"}"#
    )
    .contains(r#""type":"ok""#));
    assert!(round_trip(
        &mut stream,
        &mut reader,
        r#"{"schema_version":1,"request_id":"2","command":"submit","book_id":"x","id":1,"side":"buy","qty":2,"kind":{"kind":"limit","price":100}}"#
    )
    .contains(r#""type":"ok""#));
    let duplicate = round_trip(
        &mut stream,
        &mut reader,
        r#"{"schema_version":1,"request_id":"3","command":"create_book","book_id":"x"}"#,
    );
    assert!(duplicate.contains(r#""request_id":"3""#));
    assert!(duplicate.contains("book exists"));
    let snapshot = round_trip(
        &mut stream,
        &mut reader,
        r#"{"schema_version":1,"request_id":"4","command":"snapshot","book_id":"x","depth":10}"#,
    );
    assert!(snapshot.contains(r#""bids":[[100,2]]"#), "{snapshot}");
    let rejection = round_trip(
        &mut stream,
        &mut reader,
        r#"{"schema_version":1,"request_id":"5","command":"cancel","book_id":"x","id":99}"#,
    );
    assert!(rejection.contains(r#""request_id":"5""#));
    assert!(rejection.contains(r#""message":"unknown_order""#));

    let _ = child.kill();
    let _ = child.wait();
}
