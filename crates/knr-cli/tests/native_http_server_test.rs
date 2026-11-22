use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn knr_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_knr"))
}

fn http_request(port: u16, method: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to native knr server");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let raw = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).expect("write HTTP request");
    stream.flush().expect("flush HTTP request");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read HTTP response");
    resp
}

#[test]
fn test_milestone_c_native_compiled_http_server_e2e() {
    let root = repo_root();
    let server_ts = root.join("examples/http_server.tsk");
    let out_bin = std::env::temp_dir().join(format!(
        "knr_native_http_server_test_{}",
        std::process::id()
    ));

    // 1. Verify multi-file Strict TS project cleanly (imports ./http_types.tsk)
    let check_out = Command::new(knr_bin())
        .args(["check", server_ts.to_str().unwrap()])
        .output()
        .expect("knr check http_server.tsk");
    assert!(
        check_out.status.success(),
        "knr check examples/http_server.tsk failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check_out.stdout),
        String::from_utf8_lossy(&check_out.stderr)
    );

    // 1b. Run built-in deterministic unit tests (`knr test examples/http_server.tsk`)
    let test_out = Command::new(knr_bin())
        .args(["test", server_ts.to_str().unwrap(), "--json"])
        .output()
        .expect("knr test http_server.tsk");
    assert!(
        test_out.status.success(),
        "knr test examples/http_server.tsk failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&test_out.stdout),
        String::from_utf8_lossy(&test_out.stderr)
    );

    // 2. Compile to standalone native ELF binary linked against Rust libknr_runtime.a
    let build_out = Command::new(knr_bin())
        .args([
            "build",
            server_ts.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            out_bin.to_str().unwrap(),
        ])
        .output()
        .expect("knr build native http_server.tsk");
    assert!(
        build_out.status.success(),
        "knr build native failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr)
    );
    assert!(out_bin.exists(), "Native binary must exist on disk");

    // 3. Launch the standalone compiled native binary on ephemeral KNR_PORT=0 (NO knr VM!)
    let mut child = Command::new(&out_bin)
        .env("KNR_PORT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn compiled native http_server binary");

    let stderr = child.stderr.take().expect("capture stderr");
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    let start = Instant::now();
    let port: u16 = loop {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "Timed out waiting for native server Listening line"
        );
        line.clear();
        let n = reader.read_line(&mut line).expect("read stderr line");
        if n == 0 {
            panic!("Native server exited prematurely before printing Listening URL");
        }
        if let Some(pos) = line.find("127.0.0.1:") {
            let port_str = line[pos + "127.0.0.1:".len()..].trim();
            if let Ok(p) = port_str.parse::<u16>() {
                break p;
            }
        }
    };

    // 4. Verify structured JSON status on GET / and GET /health
    let home = http_request(port, "GET", "/");
    assert!(home.starts_with("HTTP/1.1 200 OK"), "GET / failed: {home}");
    assert!(
        home.contains(r#"{"service":"knr-http","count":0,"requests":1,"route":"/"}"#),
        "GET / body unexpected: {home}"
    );

    let health = http_request(port, "GET", "/health");
    assert!(health.starts_with("HTTP/1.1 200 OK"), "GET /health failed: {health}");
    assert!(health.contains(r#"{"status":"ok"}"#), "GET /health body unexpected: {health}");

    // 5. Verify state transitions via POST /api/inc, /api/dec, /api/reset
    let inc1 = http_request(port, "POST", "/api/inc");
    assert!(inc1.starts_with("HTTP/1.1 200 OK"), "POST /api/inc #1 failed: {inc1}");
    assert!(inc1.contains(r#"{"count":1}"#), "inc1 body unexpected: {inc1}");

    let inc2 = http_request(port, "POST", "/api/inc");
    assert!(inc2.starts_with("HTTP/1.1 200 OK"), "POST /api/inc #2 failed: {inc2}");
    assert!(inc2.contains(r#"{"count":2}"#), "inc2 body unexpected: {inc2}");

    let inc3 = http_request(port, "POST", "/api/inc");
    assert!(inc3.starts_with("HTTP/1.1 200 OK"), "POST /api/inc #3 failed: {inc3}");
    assert!(inc3.contains(r#"{"count":3}"#), "inc3 body unexpected: {inc3}");

    let dec1 = http_request(port, "POST", "/api/dec");
    assert!(dec1.starts_with("HTTP/1.1 200 OK"), "POST /api/dec failed: {dec1}");
    assert!(dec1.contains(r#"{"count":2}"#), "dec1 body unexpected: {dec1}");

    let count_resp = http_request(port, "GET", "/count");
    assert!(count_resp.starts_with("HTTP/1.1 200 OK"), "GET /count failed: {count_resp}");
    assert!(count_resp.ends_with("\r\n\r\n2"), "GET /count body unexpected: {count_resp}");

    let reset_resp = http_request(port, "POST", "/api/reset");
    assert!(reset_resp.starts_with("HTTP/1.1 200 OK"), "POST /api/reset failed: {reset_resp}");
    assert!(reset_resp.contains(r#"{"count":0}"#), "reset body unexpected: {reset_resp}");

    let inc_final_1 = http_request(port, "POST", "/api/inc");
    assert!(inc_final_1.contains(r#"{"count":1}"#), "inc_final_1 unexpected: {inc_final_1}");
    let inc_final_2 = http_request(port, "POST", "/api/inc");
    assert!(inc_final_2.contains(r#"{"count":2}"#), "inc_final_2 unexpected: {inc_final_2}");

    let missing = http_request(port, "GET", "/nope");
    assert!(missing.starts_with("HTTP/1.1 404 Not Found"), "GET /nope expected 404: {missing}");

    let bye = http_request(port, "GET", "/shutdown");
    assert!(bye.starts_with("HTTP/1.1 200 OK"), "GET /shutdown failed: {bye}");
    assert!(bye.contains("bye"), "GET /shutdown body unexpected: {bye}");

    // 6. Verify clean shutdown with exit code == final counter (2)
    let status = child.wait().expect("wait for native server exit");
    let _ = std::fs::remove_file(&out_bin);
    assert_eq!(
        status.code(),
        Some(2),
        "Native server must exit cleanly with count == 2"
    );
}
