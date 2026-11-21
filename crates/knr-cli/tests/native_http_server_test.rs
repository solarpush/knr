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
    let server_knr = root.join("examples/http_server.knr");
    let out_bin = std::env::temp_dir().join(format!(
        "knr_native_http_server_test_{}",
        std::process::id()
    ));

    // 1. Verify multi-file .knr project cleanly (imports http_types.knr + ui.knr)
    let check_out = Command::new(knr_bin())
        .args(["check", server_knr.to_str().unwrap()])
        .output()
        .expect("knr check http_server.knr");
    assert!(
        check_out.status.success(),
        "knr check examples/http_server.knr failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check_out.stdout),
        String::from_utf8_lossy(&check_out.stderr)
    );

    // 2. Compile to standalone native ELF binary linked against Rust libknr_runtime.a
    let build_out = Command::new(knr_bin())
        .args([
            "build",
            server_knr.to_str().unwrap(),
            "--target",
            "native",
            "-o",
            out_bin.to_str().unwrap(),
        ])
        .output()
        .expect("knr build native http_server.knr");
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

    // 4. Verify dark-mode typed UiNode SSR dashboard on GET /
    let home = http_request(port, "GET", "/");
    assert!(home.starts_with("HTTP/1.1 200 OK"), "GET / failed: {home}");
    assert!(home.contains("<!DOCTYPE html>"), "GET / must return full styled HTML5 doc");
    assert!(home.contains("Count: 0"), "GET / body unexpected: {home}");
    assert!(home.contains("data-knr-action=\"/ui/inc\""), "GET / must expose interactive SSR buttons");

    // 5. Verify live SSR interactive button routes (POST /ui/step5, POST /ui/inc -> Count: 5)
    let step5 = http_request(port, "POST", "/ui/step5");
    assert!(step5.starts_with("HTTP/1.1 200 OK"), "POST /ui/step5 failed: {step5}");
    assert!(step5.contains("Active step: x5"), "step5 UI did not update: {step5}");

    let ui_inc = http_request(port, "POST", "/ui/inc");
    assert!(ui_inc.starts_with("HTTP/1.1 200 OK"), "POST /ui/inc failed: {ui_inc}");
    assert!(ui_inc.contains("Count: 5"), "ui_inc must render Count: 5: {ui_inc}");

    // 6. Verify JSON & plain API routes still work cleanly (5 + 5 = 10)
    let api_inc = http_request(port, "POST", "/api/inc");
    assert!(api_inc.starts_with("HTTP/1.1 200 OK"), "POST /api/inc failed: {api_inc}");
    assert!(api_inc.contains("{\"count\":10}"), "api_inc body unexpected: {api_inc}");

    let count_resp = http_request(port, "GET", "/count");
    assert!(count_resp.starts_with("HTTP/1.1 200 OK"), "GET /count failed: {count_resp}");
    assert!(count_resp.ends_with("\r\n\r\n10"), "GET /count body unexpected: {count_resp}");

    let missing = http_request(port, "GET", "/nope");
    assert!(missing.starts_with("HTTP/1.1 404 Not Found"), "GET /nope expected 404: {missing}");

    let bye = http_request(port, "GET", "/shutdown");
    assert!(bye.starts_with("HTTP/1.1 200 OK"), "GET /shutdown failed: {bye}");
    assert!(bye.contains("bye"), "GET /shutdown body unexpected: {bye}");

    // 7. Verify clean shutdown with exit code == final counter (10)
    let status = child.wait().expect("wait for native server exit");
    let _ = std::fs::remove_file(&out_bin);
    assert_eq!(
        status.code(),
        Some(10),
        "Native server must exit cleanly with count == 10"
    );
}
