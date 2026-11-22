use crate::platform::{RuntimeError, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

fn extract_two_strs(
    payload: Value,
    op: &str,
    key0: &str,
    key1: &str,
) -> Result<(String, String), RuntimeError> {
    match payload {
        Value::Struct { fields, .. } if fields.len() >= 2 => {
            let mut it = fields.into_iter();
            let first = match it.next() {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects field 0 ({key0}) to be str"
                    )));
                }
            };
            let second = match it.next() {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects field 1 ({key1}) to be str"
                    )));
                }
            };
            Ok((first, second))
        }
        Value::Vec(v) if v.len() >= 2 => {
            let mut it = v.into_iter();
            let first = match it.next() {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects element 0 ({key0}) to be str"
                    )));
                }
            };
            let second = match it.next() {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects element 1 ({key1}) to be str"
                    )));
                }
            };
            Ok((first, second))
        }
        Value::Map(mut m) => {
            let first = match m.remove(key0) {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects map key '{key0}' to be str"
                    )));
                }
            };
            let second = match m.remove(key1) {
                Some(Value::Str(s)) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(format!(
                        "{op} expects map key '{key1}' to be str"
                    )));
                }
            };
            Ok((first, second))
        }
        _ => Err(RuntimeError::InvalidArgument(format!(
            "{op} expects struct, vec, or map with ({key0}, {key1}) strings"
        ))),
    }
}

/// Handles filesystem operations (`fs.read_file`, `fs.write_file`, `fs.exists`,
/// `fs.remove_file`, `fs.list_dir`) under the `Hw` capability.
///
/// Returns `Ok(None)` if `op` is not a recognized filesystem operation.
pub fn handle_fs_op(op: &str, payload: Value) -> Result<Option<Value>, RuntimeError> {
    match op {
        "fs.read_file" => {
            let path = match payload {
                Value::Str(p) => p,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "fs.read_file expects str path".into(),
                    ));
                }
            };
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            Ok(Some(Value::Str(content)))
        }
        "fs.write_file" => {
            let (path, content) = extract_two_strs(payload, op, "path", "content")?;
            let p = Path::new(&path);
            if let Some(parent) = p
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty() && !dir.exists())
            {
                let _ = std::fs::create_dir_all(parent);
            }
            let ok = std::fs::write(p, content.as_bytes()).is_ok();
            Ok(Some(Value::Bool(ok)))
        }
        "fs.exists" => {
            let path = match payload {
                Value::Str(p) => p,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "fs.exists expects str path".into(),
                    ));
                }
            };
            Ok(Some(Value::Bool(Path::new(&path).exists())))
        }
        "fs.remove_file" => {
            let path = match payload {
                Value::Str(p) => p,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "fs.remove_file expects str path".into(),
                    ));
                }
            };
            Ok(Some(Value::Bool(std::fs::remove_file(&path).is_ok())))
        }
        "fs.list_dir" => {
            let dir = match payload {
                Value::Str(d) => d,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "fs.list_dir expects str directory path".into(),
                    ));
                }
            };
            let mut names: Vec<String> = match std::fs::read_dir(&dir) {
                Ok(entries) => entries
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect(),
                Err(_) => Vec::new(),
            };
            names.sort();
            Ok(Some(Value::Str(names.join("\n"))))
        }
        _ => Ok(None),
    }
}

fn json_get_path(json_str: &str, key_path: &str) -> String {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return String::new();
    };
    let mut current = &root;
    if !key_path.is_empty() {
        for segment in key_path.split('.') {
            match current {
                serde_json::Value::Object(map) => match map.get(segment) {
                    Some(next) => current = next,
                    None => return String::new(),
                },
                serde_json::Value::Array(arr) => {
                    match segment.parse::<usize>().ok().and_then(|idx| arr.get(idx)) {
                        Some(next) => current = next,
                        None => return String::new(),
                    }
                }
                _ => return String::new(),
            }
        }
    }
    match current {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Handles JSON operations (`json.is_valid`, `json.pretty`, `json.get`) under the `Hw` capability.
///
/// Returns `Ok(None)` if `op` is not a recognized JSON operation.
pub fn handle_json_op(op: &str, payload: Value) -> Result<Option<Value>, RuntimeError> {
    match op {
        "json.is_valid" => {
            let raw = match payload {
                Value::Str(s) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "json.is_valid expects str JSON input".into(),
                    ));
                }
            };
            let valid = serde_json::from_str::<serde_json::Value>(&raw).is_ok();
            Ok(Some(Value::Bool(valid)))
        }
        "json.pretty" => {
            let raw = match payload {
                Value::Str(s) => s,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "json.pretty expects str JSON input".into(),
                    ));
                }
            };
            let pretty = serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or(raw);
            Ok(Some(Value::Str(pretty)))
        }
        "json.get" => {
            let (json_str, key_path) = extract_two_strs(payload, op, "json", "key")?;
            Ok(Some(Value::Str(json_get_path(&json_str, &key_path))))
        }
        _ => Ok(None),
    }
}

fn parse_http_url(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return None;
    }
    let connect_addr = if authority.starts_with('[') {
        if authority.contains("]:") {
            authority.to_string()
        } else {
            format!("{authority}:80")
        }
    } else if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Some((connect_addr, authority.to_string(), path.to_string()))
}

fn decode_chunked(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(pos) = data.windows(2).position(|w| w == b"\r\n") {
        let size_line = std::str::from_utf8(&data[..pos]).unwrap_or("");
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        data = &data[pos + 2..];
        if size == 0 || size > data.len() {
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}

fn http_request(method: &str, url: &str, body: Option<&str>) -> Option<String> {
    let (connect_addr, host_header, path) = parse_http_url(url)?;
    let sock_addr = connect_addr.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&sock_addr, HTTP_TIMEOUT).ok()?;
    let _ = stream.set_read_timeout(Some(HTTP_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_TIMEOUT));

    let request = match body {
        Some(payload) => format!(
            "{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ),
        None => format!(
            "{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\n\r\n"
        ),
    };

    stream.write_all(request.as_bytes()).ok()?;
    stream.flush().ok()?;

    let mut raw = Vec::new();
    if stream.read_to_end(&mut raw).is_err() && raw.is_empty() {
        return None;
    }

    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        let headers = String::from_utf8_lossy(&raw[..pos]).to_ascii_lowercase();
        let body_bytes = &raw[pos + 4..];
        let is_chunked = headers
            .lines()
            .any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked"));
        if is_chunked {
            let decoded = decode_chunked(body_bytes);
            Some(String::from_utf8_lossy(&decoded).into_owned())
        } else {
            Some(String::from_utf8_lossy(body_bytes).into_owned())
        }
    } else {
        Some(String::from_utf8_lossy(&raw).into_owned())
    }
}

/// Pure-Rust HTTP/1.1 GET helper returning the response body (or empty string on failure).
pub fn http_get(url: &str) -> String {
    http_request("GET", url, None).unwrap_or_default()
}

/// Pure-Rust HTTP/1.1 POST helper returning the response body (or empty string on failure).
pub fn http_post(url: &str, body: &str) -> String {
    http_request("POST", url, Some(body)).unwrap_or_default()
}

/// Handles pure-Rust HTTP/1.1 client operations (`http.get`, `http.post`) under the `Net` capability.
///
/// Returns `Ok(None)` if `op` is not a recognized HTTP client operation.
pub fn handle_http_client_op(op: &str, payload: Value) -> Result<Option<Value>, RuntimeError> {
    match op {
        "http.get" => {
            let url = match payload {
                Value::Str(u) => u,
                _ => {
                    return Err(RuntimeError::InvalidArgument(
                        "http.get expects str URL".into(),
                    ));
                }
            };
            let body = http_get(&url);
            Ok(Some(Value::Str(body)))
        }
        "http.post" => {
            let (url, body) = extract_two_strs(payload, op, "url", "body")?;
            let resp = http_post(&url, &body);
            Ok(Some(Value::Str(resp)))
        }
        _ => Ok(None),
    }
}

/// Native multi-threaded `spawn` C-ABI entrypoint exported by `knr-runtime`.
///
/// Casts `fn_ptr` to `extern "C" fn(i64) -> i64` and executes it on a newly spawned OS thread.
/// Returns `1` on success, or `0` if `fn_ptr == 0` or thread creation fails.
#[no_mangle]
pub extern "C" fn rt_spawn(fn_ptr: usize, arg: i64) -> i64 {
    if fn_ptr == 0 {
        return 0;
    }
    let f: extern "C" fn(i64) -> i64 =
        unsafe { std::mem::transmute::<usize, extern "C" fn(i64) -> i64>(fn_ptr) };
    match std::thread::Builder::new().spawn(move || {
        let _ = f(arg);
    }) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hw::HwPlatform;
    use crate::net::NetPlatform;
    use crate::platform::HostPlatform;
    use knr_core::Effect;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_fs_write_read_exists_list_and_remove() {
        let base_dir = std::env::temp_dir().join(format!(
            "knr_sys_fs_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let nested_file = base_dir.join("sub").join("alpha.txt");
        let second_file = base_dir.join("sub").join("beta.txt");
        let nested_str = nested_file.to_string_lossy().into_owned();
        let second_str = second_file.to_string_lossy().into_owned();
        let sub_dir_str = base_dir.join("sub").to_string_lossy().into_owned();

        // Initially does not exist
        assert_eq!(
            handle_fs_op("fs.exists", Value::Str(nested_str.clone())).unwrap(),
            Some(Value::Bool(false))
        );

        // Write creates parent directories automatically
        let write_payload = Value::Struct {
            schema: "FileWrite".into(),
            fields: vec![
                Value::Str(nested_str.clone()),
                Value::Str("hello knr fs".into()),
            ],
        };
        assert_eq!(
            handle_fs_op("fs.write_file", write_payload).unwrap(),
            Some(Value::Bool(true))
        );

        let write_second = Value::Struct {
            schema: "FileWrite".into(),
            fields: vec![Value::Str(second_str.clone()), Value::Str("second".into())],
        };
        assert_eq!(
            handle_fs_op("fs.write_file", write_second).unwrap(),
            Some(Value::Bool(true))
        );

        // Exists and reads back identically
        assert_eq!(
            handle_fs_op("fs.exists", Value::Str(nested_str.clone())).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            handle_fs_op("fs.read_file", Value::Str(nested_str.clone())).unwrap(),
            Some(Value::Str("hello knr fs".into()))
        );

        // Directory listing is sorted and newline-joined
        assert_eq!(
            handle_fs_op("fs.list_dir", Value::Str(sub_dir_str)).unwrap(),
            Some(Value::Str("alpha.txt\nbeta.txt".into()))
        );

        // Remove file and verify gone
        assert_eq!(
            handle_fs_op("fs.remove_file", Value::Str(nested_str.clone())).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            handle_fs_op("fs.exists", Value::Str(nested_str.clone())).unwrap(),
            Some(Value::Bool(false))
        );
        assert_eq!(
            handle_fs_op("fs.read_file", Value::Str(nested_str)).unwrap(),
            Some(Value::Str(String::new()))
        );

        let _ = std::fs::remove_dir_all(&base_dir);
    }

    #[test]
    fn test_json_is_valid_pretty_and_nested_get() {
        let sample =
            r#"{"user":{"name":"Ada","age":36},"items":["first","second",{"tag":"nested"}],"ok":true}"#;

        assert_eq!(
            handle_json_op("json.is_valid", Value::Str(sample.into())).unwrap(),
            Some(Value::Bool(true))
        );
        assert_eq!(
            handle_json_op("json.is_valid", Value::Str("{not json".into())).unwrap(),
            Some(Value::Bool(false))
        );

        let pretty = handle_json_op("json.pretty", Value::Str(sample.into())).unwrap();
        match pretty {
            Some(Value::Str(s)) => {
                assert!(s.contains('\n'));
                assert!(s.contains("\"name\": \"Ada\""));
            }
            other => panic!("expected pretty JSON string, got {other:?}"),
        }

        // Invalid JSON passed to json.pretty returns original string unchanged
        assert_eq!(
            handle_json_op("json.pretty", Value::Str("raw-invalid".into())).unwrap(),
            Some(Value::Str("raw-invalid".into()))
        );

        let query = |path: &str| {
            handle_json_op(
                "json.get",
                Value::Struct {
                    schema: "JsonQuery".into(),
                    fields: vec![Value::Str(sample.into()), Value::Str(path.into())],
                },
            )
            .unwrap()
        };

        assert_eq!(query("user.name"), Some(Value::Str("Ada".into())));
        assert_eq!(query("user.age"), Some(Value::Str("36".into())));
        assert_eq!(query("items.0"), Some(Value::Str("first".into())));
        assert_eq!(query("items.1"), Some(Value::Str("second".into())));
        assert_eq!(query("items.2.tag"), Some(Value::Str("nested".into())));
        assert_eq!(query("ok"), Some(Value::Str("true".into())));
        assert_eq!(query("user.missing"), Some(Value::Str(String::new())));
        assert_eq!(query("items.99"), Some(Value::Str(String::new())));
    }

    fn read_http_request(stream: &mut TcpStream) -> (String, String) {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let header_end = loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break buf.len(),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos;
                    }
                }
            }
        };

        let header_str = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let content_len: usize = header_str
            .lines()
            .find_map(|line| {
                let lower = line.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);

        let body_start = (header_end + 4).min(buf.len());
        while buf.len().saturating_sub(body_start) < content_len {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }

        let body = String::from_utf8_lossy(&buf[body_start..]).into_owned();
        (header_str, body)
    }

    #[test]
    fn test_http_get_and_post_against_ephemeral_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral listener");
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let (headers, body) = read_http_request(&mut stream);
                    let first_line = headers.lines().next().unwrap_or("");
                    let resp_body = if first_line.starts_with("GET /hello") {
                        "hello-http-get".to_string()
                    } else if first_line.starts_with("POST /echo") {
                        format!("echo:{body}")
                    } else {
                        "not-found".to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
            }
        });

        let get_url = format!("http://127.0.0.1:{port}/hello");
        let get_res = handle_http_client_op("http.get", Value::Str(get_url)).unwrap();
        assert_eq!(get_res, Some(Value::Str("hello-http-get".into())));

        let post_payload = Value::Struct {
            schema: "HttpPost".into(),
            fields: vec![
                Value::Str(format!("http://127.0.0.1:{port}/echo")),
                Value::Str(r#"{"msg":"hi"}"#.into()),
            ],
        };
        let post_res = handle_http_client_op("http.post", post_payload).unwrap();
        assert_eq!(
            post_res,
            Some(Value::Str(r#"echo:{"msg":"hi"}"#.into()))
        );

        // Invalid URL returns empty string gracefully
        assert_eq!(
            handle_http_client_op("http.get", Value::Str("invalid://url".into())).unwrap(),
            Some(Value::Str(String::new()))
        );

        server.join().expect("server thread finished");
    }

    #[test]
    fn test_hw_and_net_platform_integration() {
        let mut hw = HwPlatform::new();
        let valid = hw
            .host_call(Effect::Hw, "json.is_valid", Value::Str(r#"{"a":1}"#.into()))
            .unwrap();
        assert_eq!(valid, Value::Bool(true));

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_http_request(&mut stream);
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });

        let mut net = NetPlatform::new().expect("net platform");
        let resp = net
            .host_call(
                Effect::Net,
                "http.get",
                Value::Str(format!("http://127.0.0.1:{port}/status")),
            )
            .unwrap();
        assert_eq!(resp, Value::Str("ok".into()));
        server.join().expect("server thread joined");
    }

    extern "C" fn test_spawn_worker(arg: i64) -> i64 {
        let ptr = arg as *const AtomicI64;
        if !ptr.is_null() {
            unsafe {
                (*ptr).store(12345, Ordering::SeqCst);
            }
        }
        12345
    }

    #[test]
    fn test_rt_spawn_null_and_real_os_thread() {
        assert_eq!(rt_spawn(0, 42), 0);

        let slot = Arc::new(AtomicI64::new(0));
        let ptr = Arc::as_ptr(&slot) as i64;
        let spawned = rt_spawn(test_spawn_worker as *const () as usize, ptr);
        assert_eq!(spawned, 1);

        let mut observed = 0;
        for _ in 0..200 {
            observed = slot.load(Ordering::SeqCst);
            if observed == 12345 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(observed, 12345);
    }
}
