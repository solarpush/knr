use knr_core::Effect;
use knr_runtime::{HostPlatform, NetPlatform, RuntimeError, Value};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test(flavor = "multi_thread")]
async fn test_net_listen_and_respond_http_flow() {
    let mut net = NetPlatform::new().expect("NetPlatform should initialize");

    // Start listening on ephemeral port 0
    let listen_val = net
        .host_call(Effect::Net, "net.listen", Value::I64(0))
        .expect("net.listen should succeed");

    let map = listen_val.as_map().expect("should return map");
    let port = map.get("port").and_then(Value::as_i64).expect("port") as u16;
    let chan_id = map.get("chan_id").and_then(Value::as_i64).expect("chan_id") as usize;
    assert!(port > 0);

    // Client connects and sends HTTP request
    let mut client = TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("Client should connect");

    let req_data = b"GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n";
    client.write_all(req_data).await.expect("Client send");

    // Server receives request from channel (retry with small sleep if async queue pending)
    let mut req_val = None;
    for _ in 0..50 {
        if let Some(r) = net.recv_request(chan_id).unwrap() {
            req_val = Some(r);
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }

    let req = req_val.expect("Server should receive request");
    let req_map = req.as_map().expect("Request should be a Map");
    assert_eq!(req_map.get("method"), Some(&Value::Str("GET".into())));
    assert_eq!(req_map.get("path"), Some(&Value::Str("/status".into())));
    let req_id = req_map.get("req_id").and_then(Value::as_i64).expect("req_id");

    // Server responds with 200 OK
    let mut resp_payload = BTreeMap::new();
    resp_payload.insert("req_id".to_string(), Value::I64(req_id));
    resp_payload.insert("status".to_string(), Value::I64(200));
    resp_payload.insert(
        "body".to_string(),
        Value::Buf(b"{\"status\":\"ok\"}".to_vec()),
    );

    let resp_res = net
        .host_call(Effect::Net, "net.respond", Value::Map(resp_payload))
        .expect("net.respond should succeed");
    assert_eq!(resp_res, Value::Bool(true));

    // Client receives response
    let mut resp_buf = vec![0u8; 1024];
    let n = client.read(&mut resp_buf).await.expect("Client read");
    let resp_str = String::from_utf8_lossy(&resp_buf[..n]);
    assert!(resp_str.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp_str.contains("Content-Length: 15\r\n"));
    assert!(resp_str.ends_with("{\"status\":\"ok\"}"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_net_respond_empty_and_large_body() {
    let mut net = NetPlatform::new().unwrap();

    let listen_val = net.host_call(Effect::Net, "net.listen", Value::I64(0)).unwrap();
    let port = listen_val.as_map().unwrap().get("port").unwrap().as_i64().unwrap() as u16;
    let chan_id = listen_val.as_map().unwrap().get("chan_id").unwrap().as_i64().unwrap() as usize;

    // Test 1: Empty body
    let mut client = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    client.write_all(b"GET /empty HTTP/1.1\r\n\r\n").await.unwrap();

    let mut req_val = None;
    for _ in 0..50 {
        if let Some(r) = net.recv_request(chan_id).unwrap() {
            req_val = Some(r);
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    let req_id = req_val.unwrap().as_map().unwrap().get("req_id").unwrap().as_i64().unwrap();

    let mut resp_payload = BTreeMap::new();
    resp_payload.insert("req_id".to_string(), Value::I64(req_id));
    resp_payload.insert("status".to_string(), Value::I64(204));
    resp_payload.insert("body".to_string(), Value::Buf(vec![]));

    net.host_call(Effect::Net, "net.respond", Value::Map(resp_payload)).unwrap();

    let mut resp_buf = vec![0u8; 512];
    let n = client.read(&mut resp_buf).await.unwrap();
    let resp_str = String::from_utf8_lossy(&resp_buf[..n]);
    assert!(resp_str.starts_with("HTTP/1.1 204 No Content\r\n"));
    assert!(resp_str.contains("Content-Length: 0\r\n"));

    // Test 2: Large body (64 KiB)
    let mut client2 = TcpStream::connect(format!("127.0.0.1:{port}")).await.unwrap();
    client2.write_all(b"GET /large HTTP/1.1\r\n\r\n").await.unwrap();

    let mut req_val2 = None;
    for _ in 0..50 {
        if let Some(r) = net.recv_request(chan_id).unwrap() {
            req_val2 = Some(r);
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    let req_id2 = req_val2.unwrap().as_map().unwrap().get("req_id").unwrap().as_i64().unwrap();

    let large_body = vec![b'X'; 65536];
    let mut resp_payload2 = BTreeMap::new();
    resp_payload2.insert("req_id".to_string(), Value::I64(req_id2));
    resp_payload2.insert("status".to_string(), Value::I64(200));
    resp_payload2.insert("body".to_string(), Value::Buf(large_body.clone()));

    net.host_call(Effect::Net, "net.respond", Value::Map(resp_payload2)).unwrap();

    let mut read_data = Vec::new();
    let mut chunk = vec![0u8; 8192];
    loop {
        let n = client2.read(&mut chunk).await.unwrap();
        if n == 0 {
            break;
        }
        read_data.extend_from_slice(&chunk[..n]);
    }
    let resp_str2 = String::from_utf8_lossy(&read_data);
    assert!(resp_str2.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(resp_str2.contains("Content-Length: 65536\r\n"));
    assert!(read_data.ends_with(&large_body));
}

#[test]
fn test_net_respond_invalid_id() {
    let mut net = NetPlatform::new().unwrap();
    let mut resp_payload = BTreeMap::new();
    resp_payload.insert("req_id".to_string(), Value::I64(999999));
    resp_payload.insert("status".to_string(), Value::I64(200));

    let err = net.host_call(Effect::Net, "net.respond", Value::Map(resp_payload));
    assert!(matches!(err, Err(RuntimeError::InvalidArgument(_))));
}

#[test]
fn test_net_http_response_formatting_helper() {
    let mut net = NetPlatform::new().unwrap();
    let mut payload = BTreeMap::new();
    payload.insert("status".to_string(), Value::I64(404));
    payload.insert("body".to_string(), Value::Str("Not Found Here".into()));

    let val = net
        .host_call(Effect::Net, "net.http_response", Value::Map(payload))
        .expect("http_response should succeed");

    let buf = val.as_buf().expect("should return Buf");
    let resp = String::from_utf8_lossy(buf);
    assert!(resp.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(resp.contains("Content-Length: 14\r\n"));
    assert!(resp.ends_with("Not Found Here"));
}
