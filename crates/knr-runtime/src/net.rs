use crate::platform::{HostPlatform, RuntimeError, Value};
use knr_core::Effect;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// Helper to get standard HTTP reason phrase for a status code.
pub fn http_status_text(status: i64) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

/// Formats a full HTTP/1.1 wire response given a status code, body, and optional content type.
pub fn format_http_response(status: i64, body: &[u8], content_type: Option<&str>) -> Vec<u8> {
    let reason = http_status_text(status);
    let ctype = content_type.unwrap_or("text/html; charset=utf-8");
    let mut resp = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
        status,
        reason,
        body.len(),
        ctype
    )
    .into_bytes();
    resp.extend_from_slice(body);
    resp
}

type SharedReceiver = Arc<tokio::sync::Mutex<mpsc::Receiver<Value>>>;
type SharedTcpStream = Arc<tokio::sync::Mutex<TcpStream>>;
type SharedTcpListener = Arc<tokio::sync::Mutex<TcpListener>>;

/// Network Platform Adapter backed by Tokio async runtime for TCP sockets and HTTP bridging.
pub struct NetPlatform {
    runtime: Option<Arc<tokio::runtime::Runtime>>,
    listeners: Arc<Mutex<HashMap<usize, SharedTcpListener>>>,
    connections: Arc<Mutex<HashMap<usize, SharedTcpStream>>>,
    request_receivers: Arc<Mutex<HashMap<usize, SharedReceiver>>>,
    request_senders: Arc<Mutex<HashMap<usize, mpsc::Sender<Value>>>>,
    next_id: Arc<Mutex<usize>>,
}

impl Drop for NetPlatform {
    fn drop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                // If dropped inside an asynchronous context, shutdown the runtime on a separate thread
                let _ = std::thread::spawn(move || drop(rt)).join();
            } else {
                drop(rt);
            }
        }
    }
}

impl NetPlatform {
    /// Creates a new `NetPlatform`, initializing a dedicated Tokio multi-threaded runtime.
    pub fn new() -> Result<Self, RuntimeError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| RuntimeError::Io(format!("Failed to initialize Tokio runtime: {e}")))?;

        Ok(Self {
            runtime: Some(Arc::new(runtime)),
            listeners: Arc::new(Mutex::new(HashMap::new())),
            connections: Arc::new(Mutex::new(HashMap::new())),
            request_receivers: Arc::new(Mutex::new(HashMap::new())),
            request_senders: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(1)),
        })
    }

    fn alloc_id(&self) -> usize {
        let mut id = self.next_id.lock().unwrap();
        let curr = *id;
        *id += 1;
        curr
    }

    /// Executes an async future synchronously on the platform's Tokio runtime.
    pub fn block_on<F>(&self, f: F) -> F::Output
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let rt = self.runtime.as_ref().expect("NetPlatform runtime not initialized");
        if tokio::runtime::Handle::try_current().is_ok() {
            let rt = Arc::clone(rt);
            std::thread::spawn(move || rt.block_on(f)).join().expect("Async task thread panicked")
        } else {
            rt.block_on(f)
        }
    }

    /// Spawns a background task on the Tokio runtime.
    pub fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let rt = self.runtime.as_ref().expect("NetPlatform runtime not initialized");
        rt.spawn(f);
    }

    /// Starts listening on a port and pushes incoming HTTP requests to a newly allocated channel.
    /// Returns (channel_id, bound_port).
    pub fn listen(&mut self, port: u16) -> Result<(usize, u16), RuntimeError> {
        let bind_addr = format!("127.0.0.1:{port}");
        let listener = self.block_on(async move {
            TcpListener::bind(&bind_addr)
                .await
                .map_err(|e| RuntimeError::Io(format!("Failed to bind to {bind_addr}: {e}")))
        })?;

        let local_addr = listener
            .local_addr()
            .map_err(|e| RuntimeError::Io(format!("Failed to get local address: {e}")))?;
        let bound_port = local_addr.port();

        let chan_id = self.alloc_id();
        let (tx, rx) = mpsc::channel(256);
        self.request_receivers
            .lock()
            .unwrap()
            .insert(chan_id, Arc::new(tokio::sync::Mutex::new(rx)));
        self.request_senders.lock()
            .unwrap()
            .insert(chan_id, tx.clone());

        let connections = Arc::clone(&self.connections);
        let next_id = Arc::clone(&self.next_id);

        self.spawn(async move {
            while let Ok((mut stream, _addr)) = listener.accept().await {
                let conn_id = {
                    let mut id = next_id.lock().unwrap();
                    let cid = *id;
                    *id += 1;
                    cid
                };

                // Parse simple HTTP request from the incoming stream
                let mut buf = vec![0u8; 4096];
                let mut read_bytes = 0;
                while read_bytes < buf.len() {
                    match stream.read(&mut buf[read_bytes..]).await {
                        Ok(0) => break,
                        Ok(n) => {
                            read_bytes += n;
                            if buf[..read_bytes].windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }

                let data = &buf[..read_bytes];
                let header_end = data
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .unwrap_or(read_bytes);

                let header_str = String::from_utf8_lossy(&data[..header_end]);
                let first_line = header_str.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let path = parts.next().unwrap_or("/");

                let body = if header_end + 4 <= read_bytes {
                    data[header_end + 4..read_bytes].to_vec()
                } else {
                    Vec::new()
                };

                connections.lock().unwrap().insert(
                    conn_id,
                    Arc::new(tokio::sync::Mutex::new(stream)),
                );

                let mut req_map = BTreeMap::new();
                req_map.insert("method".to_string(), Value::Str(method.to_string()));
                req_map.insert("path".to_string(), Value::Str(path.to_string()));
                req_map.insert("body".to_string(), Value::Buf(body));
                req_map.insert("req_id".to_string(), Value::I64(conn_id as i64));
                req_map.insert("conn_id".to_string(), Value::I64(conn_id as i64));

                if tx.send(Value::Map(req_map)).await.is_err() {
                    break;
                }
            }
        });

        Ok((chan_id, bound_port))
    }

    /// Responds to a request with status code and body, consuming the connection.
    pub fn respond(&mut self, req_id: usize, status: i64, body: &[u8]) -> Result<bool, RuntimeError> {
        let stream_arc = self
            .connections
            .lock()
            .unwrap()
            .remove(&req_id)
            .ok_or_else(|| RuntimeError::InvalidArgument(format!("Connection ID {req_id} not found")))?;

        let resp_bytes = format_http_response(status, body, None);

        self.block_on(async move {
            let mut stream = stream_arc.lock().await;
            stream
                .write_all(&resp_bytes)
                .await
                .map_err(|e| RuntimeError::Io(format!("Failed to write HTTP response: {e}")))?;
            stream
                .flush()
                .await
                .map_err(|e| RuntimeError::Io(format!("Failed to flush HTTP response: {e}")))?;
            let _ = stream.shutdown().await;
            Ok(true)
        })
    }

    /// Receives a pending request from a channel.
    pub fn recv_request(&self, chan_id: usize) -> Result<Option<Value>, RuntimeError> {
        let rx_arc = self
            .request_receivers
            .lock()
            .unwrap()
            .get(&chan_id)
            .cloned()
            .ok_or_else(|| RuntimeError::Channel(format!("Channel {chan_id} not found")))?;

        self.block_on(async move {
            let mut rx = rx_arc.lock().await;
            Ok(rx.try_recv().ok())
        })
    }
}

impl Default for NetPlatform {
    fn default() -> Self {
        Self::new().expect("Failed to initialize default NetPlatform")
    }
}

impl HostPlatform for NetPlatform {
    fn host_call(&mut self, effect: Effect, op: &str, payload: Value) -> Result<Value, RuntimeError> {
        if effect != Effect::Net && effect != Effect::Async {
            return Err(RuntimeError::UnsupportedEffect(effect));
        }

        match op {
            "net.listen" => {
                let port = match payload {
                    Value::I64(p) if (0..=65535).contains(&p) => p as u16,
                    Value::Map(m) => m
                        .get("port")
                        .and_then(Value::as_i64)
                        .filter(|&p| (0..=65535).contains(&p))
                        .map(|p| p as u16)
                        .unwrap_or(0),
                    _ => 0,
                };
                let (chan_id, bound_port) = self.listen(port)?;
                let mut res = BTreeMap::new();
                res.insert("chan".to_string(), Value::Chan(chan_id));
                res.insert("port".to_string(), Value::I64(bound_port as i64));
                res.insert("chan_id".to_string(), Value::I64(chan_id as i64));
                Ok(Value::Map(res))
            }

            "net.respond" => {
                let (req_id, status, body) = match payload {
                    Value::Map(m) => {
                        let req_id = m
                            .get("req_id")
                            .or_else(|| m.get("conn_id"))
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("Missing 'req_id' in net.respond".into())
                            })? as usize;
                        let status = m.get("status").and_then(Value::as_i64).unwrap_or(200);
                        let body = match m.get("body") {
                            Some(Value::Buf(b)) => b.clone(),
                            Some(Value::Str(s)) => s.as_bytes().to_vec(),
                            _ => Vec::new(),
                        };
                        (req_id, status, body)
                    }
                    Value::Vec(v) if v.len() >= 3 => {
                        let req_id = v[0].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("req_id must be i64".into())
                        })? as usize;
                        let status = v[1].as_i64().unwrap_or(200);
                        let body = match &v[2] {
                            Value::Buf(b) => b.clone(),
                            Value::Str(s) => s.as_bytes().to_vec(),
                            _ => Vec::new(),
                        };
                        (req_id, status, body)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.respond expects {req_id, status, body} or [req_id, status, body]".into(),
                        ))
                    }
                };
                let success = self.respond(req_id, status, &body)?;
                Ok(Value::Bool(success))
            }

            "net.http_response" => {
                let (status, body, ctype) = match payload {
                    Value::Map(m) => {
                        let status = m.get("status").and_then(Value::as_i64).unwrap_or(200);
                        let body = match m.get("body") {
                            Some(Value::Buf(b)) => b.clone(),
                            Some(Value::Str(s)) => s.as_bytes().to_vec(),
                            _ => Vec::new(),
                        };
                        let ctype = m.get("content_type").and_then(Value::as_str).map(|s| s.to_string());
                        (status, body, ctype)
                    }
                    Value::I64(status) => (status, Vec::new(), None),
                    _ => (200, Vec::new(), None),
                };
                let resp = format_http_response(status, &body, ctype.as_deref());
                Ok(Value::Buf(resp))
            }

            "net.accept" => {
                let listener_id = match payload {
                    Value::I64(id) => id as usize,
                    Value::Chan(id) => id,
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.accept expects listener ID".into(),
                        ))
                    }
                };
                let listener_arc = self
                    .listeners
                    .lock()
                    .unwrap()
                    .get(&listener_id)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::InvalidArgument(format!("Listener ID {listener_id} not found"))
                    })?;

                let conn_id = self.alloc_id();
                let connections = Arc::clone(&self.connections);

                let stream = self.block_on(async move {
                    let listener = listener_arc.lock().await;
                    listener
                        .accept()
                        .await
                        .map(|(s, _)| s)
                        .map_err(|e| RuntimeError::Io(format!("Accept failed: {e}")))
                })?;

                connections
                    .lock()
                    .unwrap()
                    .insert(conn_id, Arc::new(tokio::sync::Mutex::new(stream)));
                Ok(Value::I64(conn_id as i64))
            }

            "net.send" => {
                let (conn_id, data) = match payload {
                    Value::Map(m) => {
                        let cid = m
                            .get("conn_id")
                            .or_else(|| m.get("id"))
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("Missing 'conn_id' in net.send".into())
                            })? as usize;
                        let d = match m.get("data") {
                            Some(Value::Buf(b)) => b.clone(),
                            Some(Value::Str(s)) => s.as_bytes().to_vec(),
                            _ => {
                                return Err(RuntimeError::InvalidArgument(
                                    "net.send requires 'data' as Buf or Str".into(),
                                ))
                            }
                        };
                        (cid, d)
                    }
                    Value::Vec(v) if v.len() >= 2 => {
                        let cid = v[0].as_i64().ok_or_else(|| {
                            RuntimeError::InvalidArgument("conn_id must be i64".into())
                        })? as usize;
                        let d = match &v[1] {
                            Value::Buf(b) => b.clone(),
                            Value::Str(s) => s.as_bytes().to_vec(),
                            _ => {
                                return Err(RuntimeError::InvalidArgument(
                                    "data must be Buf or Str".into(),
                                ))
                            }
                        };
                        (cid, d)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.send expects {conn_id, data} or [conn_id, data]".into(),
                        ))
                    }
                };

                let stream_arc = self
                    .connections
                    .lock()
                    .unwrap()
                    .get(&conn_id)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::InvalidArgument(format!("Connection ID {conn_id} not found"))
                    })?;

                let bytes_written = self.block_on(async move {
                    let mut stream = stream_arc.lock().await;
                    stream
                        .write_all(&data)
                        .await
                        .map_err(|e| RuntimeError::Io(format!("Send failed: {e}")))?;
                    stream
                        .flush()
                        .await
                        .map_err(|e| RuntimeError::Io(format!("Flush failed: {e}")))?;
                    Ok(data.len() as i64)
                })?;

                Ok(Value::I64(bytes_written))
            }

            "net.recv" => {
                let (conn_id, max_bytes) = match payload {
                    Value::I64(cid) => (cid as usize, 4096),
                    Value::Map(m) => {
                        let cid = m
                            .get("conn_id")
                            .or_else(|| m.get("id"))
                            .and_then(Value::as_i64)
                            .ok_or_else(|| {
                                RuntimeError::InvalidArgument("Missing 'conn_id'".into())
                            })? as usize;
                        let max = m.get("max_bytes").and_then(Value::as_i64).unwrap_or(4096) as usize;
                        (cid, max)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.recv expects conn_id or {conn_id, max_bytes}".into(),
                        ))
                    }
                };

                let stream_arc = self
                    .connections
                    .lock()
                    .unwrap()
                    .get(&conn_id)
                    .cloned()
                    .ok_or_else(|| {
                        RuntimeError::InvalidArgument(format!("Connection ID {conn_id} not found"))
                    })?;

                let buf = self.block_on(async move {
                    let mut stream = stream_arc.lock().await;
                    let mut b = vec![0u8; max_bytes];
                    let n = stream
                        .read(&mut b)
                        .await
                        .map_err(|e| RuntimeError::Io(format!("Recv failed: {e}")))?;
                    b.truncate(n);
                    Ok(b)
                })?;

                Ok(Value::Buf(buf))
            }

            "net.recv_req" => {
                let chan_id = match payload {
                    Value::Chan(c) => c,
                    Value::I64(c) => c as usize,
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.recv_req expects chan ID".into(),
                        ))
                    }
                };
                match self.recv_request(chan_id)? {
                    Some(req) => Ok(req),
                    None => Ok(Value::Void),
                }
            }

            "net.close" => {
                let id = match payload {
                    Value::I64(i) => i as usize,
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.close expects i64 ID".into(),
                        ))
                    }
                };
                let closed_conn = self.connections.lock().unwrap().remove(&id).is_some();
                let closed_listener = self.listeners.lock().unwrap().remove(&id).is_some();
                Ok(Value::Bool(closed_conn || closed_listener))
            }

            "net.serve_html" => {
                let default_port = std::env::var("KNR_PORT")
                    .ok()
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(8080);

                let (port, html) = match payload {
                    Value::Str(s) => (default_port, s),
                    Value::Map(m) => {
                        let p = m
                            .get("port")
                            .and_then(Value::as_i64)
                            .filter(|&p| (0..=65535).contains(&p))
                            .map(|p| p as u16)
                            .unwrap_or(default_port);
                        let h = m
                            .get("html")
                            .or_else(|| m.get("body"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        (p, h)
                    }
                    Value::Struct { fields, .. } => {
                        let p = fields
                            .first()
                            .and_then(Value::as_i64)
                            .filter(|&p| (0..=65535).contains(&p))
                            .map(|p| p as u16)
                            .unwrap_or(default_port);
                        let h = fields
                            .get(1)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        (p, h)
                    }
                    _ => {
                        return Err(RuntimeError::InvalidArgument(
                            "net.serve_html expects Str or {port, html}".into(),
                        ))
                    }
                };

                let max_reqs = std::env::var("KNR_MAX_REQUESTS")
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok());

                let served = self.block_on(async move {
                    let bind_addr = format!("127.0.0.1:{port}");
                    let listener = match TcpListener::bind(&bind_addr).await {
                        Ok(l) => l,
                        Err(_) => TcpListener::bind("127.0.0.1:0")
                            .await
                            .map_err(|e| RuntimeError::Io(format!("Failed to bind SSR server: {e}")))?,
                    };
                    let local_addr = listener
                        .local_addr()
                        .map_err(|e| RuntimeError::Io(format!("Failed to get local address: {e}")))?;
                    eprintln!("🚀 Serveur SSR knr actif sur http://{local_addr}");

                    let mut count = 0usize;
                    let mut counter_val: i64 = 0;
                    while let Ok((mut stream, _)) = listener.accept().await {
                        let mut req_buf = [0u8; 2048];
                        let n = stream.read(&mut req_buf).await.unwrap_or(0);
                        let req_str = String::from_utf8_lossy(&req_buf[..n]);
                        let first_line = req_str.lines().next().unwrap_or("");

                        let resp_bytes = if first_line.contains("/api/inc") {
                            counter_val += 1;
                            let json = format!(
                                "{{\"count\":{counter_val},\"status\":\"ok\",\"engine\":\"knr-runtime (Tokio SSR)\"}}"
                            );
                            format_http_response(
                                200,
                                json.as_bytes(),
                                Some("application/json; charset=utf-8"),
                            )
                        } else if first_line.contains("/api/dec") {
                            counter_val -= 1;
                            let json = format!(
                                "{{\"count\":{counter_val},\"status\":\"ok\",\"engine\":\"knr-runtime (Tokio SSR)\"}}"
                            );
                            format_http_response(
                                200,
                                json.as_bytes(),
                                Some("application/json; charset=utf-8"),
                            )
                        } else if first_line.contains("/api/reset") {
                            counter_val = 0;
                            let json = format!(
                                "{{\"count\":{counter_val},\"status\":\"ok\",\"engine\":\"knr-runtime (Tokio SSR)\"}}"
                            );
                            format_http_response(
                                200,
                                json.as_bytes(),
                                Some("application/json; charset=utf-8"),
                            )
                        } else {
                            let rendered = html.replace("{{KNR_COUNTER}}", &counter_val.to_string());
                            format_http_response(
                                200,
                                rendered.as_bytes(),
                                Some("text/html; charset=utf-8"),
                            )
                        };

                        let _ = stream.write_all(&resp_bytes).await;
                        let _ = stream.flush().await;
                        let _ = stream.shutdown().await;
                        count += 1;
                        if let Some(limit) = max_reqs {
                            if count >= limit {
                                break;
                            }
                        }
                    }
                    Ok::<i64, RuntimeError>(count as i64)
                })?;

                Ok(Value::I64(served))
            }

            _ => Err(RuntimeError::InvalidOperation(format!(
                "Unknown net operation: {op}"
            ))),
        }
    }
}
