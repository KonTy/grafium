use super::{store::ReaderStore, types::*};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

type Routes = Arc<Mutex<VecDeque<(String, String, String)>>>;
const WORKERS: usize = 4;
const MAX_HEADERS: usize = 16 * 1024;

pub struct MediaServer {
    address: String,
    routes: Routes,
    stopping: Arc<AtomicBool>,
    #[cfg(test)]
    active: Arc<AtomicUsize>,
}

impl Drop for MediaServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
    }
}

impl MediaServer {
    pub fn start(store: Arc<Mutex<ReaderStore>>) -> ReaderResult<Self> {
        Self::start_with_timeouts(store, Duration::from_secs(2), Duration::from_secs(5))
    }

    pub(super) fn start_with_timeouts(
        store: Arc<Mutex<ReaderStore>>,
        read_timeout: Duration,
        write_timeout: Duration,
    ) -> ReaderResult<Self> {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?);
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let address = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let routes: Routes = Arc::new(Mutex::new(VecDeque::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        // Accept happens inside fixed workers: at most four accepted sockets
        // exist, even before any HTTP bytes arrive. No parser thread per client.
        for index in 0..WORKERS {
            let (listener, store, routes, stopping, address, active) = (
                listener.clone(),
                store.clone(),
                routes.clone(),
                stopping.clone(),
                address.clone(),
                active.clone(),
            );
            let worker_stopping = stopping.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("reader-media-{index}"))
                .spawn(move || {
                    while !worker_stopping.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut socket, _)) => {
                                active.fetch_add(1, Ordering::Relaxed);
                                let _ = socket.set_nonblocking(false);
                                let _ = serve(
                                    &mut socket,
                                    &address,
                                    &routes,
                                    &store,
                                    read_timeout,
                                    write_timeout,
                                );
                                active.fetch_sub(1, Ordering::Relaxed);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => break,
                        }
                    }
                })
            {
                stopping.store(true, Ordering::Relaxed);
                return Err(error.to_string());
            }
        }
        Ok(Self {
            address,
            routes,
            stopping,
            #[cfg(test)]
            active,
        })
    }

    #[cfg(test)]
    pub(super) fn active_connections(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn url(&self, book_id: &str, track_id: &str) -> ReaderResult<String> {
        let mut routes = self.routes.lock().map_err(|_| "Reader route lock failed")?;
        if let Some((route, _, _)) = routes
            .iter()
            .find(|(_, book, track)| book == book_id && track == track_id)
        {
            return Ok(format!("http://{}{route}", self.address));
        }
        let route = format!("/{}/audio", id());
        routes.push_back((route.clone(), book_id.into(), track_id.into()));
        if routes.len() > 256 {
            routes.pop_front();
        }
        Ok(format!("http://{}{route}", self.address))
    }

    pub fn revoke(&self) {
        if let Ok(mut routes) = self.routes.lock() {
            routes.clear();
        }
    }
}

/// Return inclusive bounds. Multipart ranges are deliberately unsupported.
pub fn byte_range(value: Option<&str>, size: u64) -> ReaderResult<Option<(u64, u64)>> {
    let Some(value) = value else { return Ok(None) };
    let value = value
        .strip_prefix("bytes=")
        .ok_or("Unsupported range unit")?;
    let (start, end) = value.split_once('-').ok_or("Invalid range")?;
    let number = |s: &str| -> ReaderResult<u64> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Invalid byte range".into());
        }
        s.parse().map_err(|_| "Invalid byte range".into())
    };
    if size == 0 {
        return Err("Range not satisfiable".into());
    }
    if start.is_empty() {
        let suffix = number(end)?;
        if suffix == 0 {
            return Err("Range not satisfiable".into());
        }
        return Ok(Some((size.saturating_sub(suffix), size - 1)));
    }
    let start = number(start)?;
    let end = if end.is_empty() {
        size - 1
    } else {
        number(end)?.min(size - 1)
    };
    if start >= size || start > end {
        return Err("Range not satisfiable".into());
    }
    Ok(Some((start, end)))
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

fn read_request(socket: &mut TcpStream, timeout: Duration) -> ReaderResult<Request> {
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Header timeout")?;
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let size = socket.read(&mut chunk).map_err(|e| e.to_string())?;
        if size == 0 {
            return Err("Incomplete headers".into());
        }
        bytes.extend_from_slice(&chunk[..size]);
        if bytes.len() > MAX_HEADERS {
            return Err("Headers too large".into());
        }
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
    };
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| "Invalid HTTP headers")?;
    let mut lines = text.split("\r\n");
    let parts: Vec<_> = lines.next().ok_or("Missing request")?.split(' ').collect();
    if parts.len() != 3
        || parts[2] != "HTTP/1.1"
        || parts[1].len() > 256
        || !parts[1].starts_with('/')
        || parts.iter().any(|p| p.chars().any(char::is_control))
    {
        return Err("Invalid HTTP request".into());
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or("Invalid HTTP header")?;
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || value.bytes().any(|b| b < 0x20 && b != b'\t' || b == 0x7f)
            || headers.len() >= 64
        {
            return Err("Invalid HTTP header".into());
        }
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err("Duplicate HTTP header".into());
        }
    }
    if headers.contains_key("transfer-encoding")
        || headers
            .get("content-length")
            .is_some_and(|length| length != "0")
    {
        return Err("Request bodies are unsupported".into());
    }
    Ok(Request {
        method: parts[0].into(),
        path: parts[1].into(),
        headers,
    })
}

fn write_deadline(socket: &mut TcpStream, mut bytes: &[u8], timeout: Duration) -> ReaderResult<()> {
    // The deadline covers the entire chunk, not just each individual short
    // write; a client cannot retain a worker forever by trickling one byte.
    let deadline = Instant::now() + timeout;
    while !bytes.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("Response timeout")?;
        socket
            .set_write_timeout(Some(remaining))
            .map_err(|e| e.to_string())?;
        let written = socket.write(bytes).map_err(|e| e.to_string())?;
        if written == 0 {
            return Err("Client closed".into());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn response(
    socket: &mut TcpStream,
    status: u16,
    length: u64,
    extra: &str,
    timeout: Duration,
) -> ReaderResult<()> {
    let reason = match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        410 => "Gone",
        416 => "Range Not Satisfiable",
        _ => "Bad Request",
    };
    write_deadline(socket, format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n{extra}\r\n").as_bytes(), timeout)
}

fn serve(
    socket: &mut TcpStream,
    address: &str,
    routes: &Routes,
    store: &Arc<Mutex<ReaderStore>>,
    read_timeout: Duration,
    write_timeout: Duration,
) -> ReaderResult<()> {
    let request = match read_request(socket, read_timeout) {
        Ok(request) => request,
        Err(_) => {
            let _ = response(socket, 400, 0, "", write_timeout);
            return Ok(());
        }
    };
    let get = |name: &str| request.headers.get(name).map(String::as_str);
    let origin = get("origin");
    let allowed_origin = origin.is_none_or(|o| {
        matches!(
            o,
            "tauri://localhost"
                | "http://tauri.localhost"
                | "https://tauri.localhost"
                | "http://localhost:5173"
                | "http://127.0.0.1:5173"
        )
    });
    let authorized = get("host") == Some(address)
        && allowed_origin
        && matches!(request.method.as_str(), "GET" | "HEAD");
    let target = if authorized {
        routes.lock().ok().and_then(|routes| {
            routes
                .iter()
                .find(|(route, _, _)| route == &request.path)
                .map(|(_, book, track)| (book.clone(), track.clone()))
        })
    } else {
        None
    };
    let Some((book, track)) = target else {
        return response(socket, 404, 0, "", write_timeout);
    };
    // The file is opened after the store lock is released, so a slow or
    // stalled Library location cannot hold up every other Library request.
    let opened = store
        .lock()
        .map_err(|_| "Reader lock failed".to_string())
        .and_then(|store| {
            Ok((
                store.media_source(&book, Some(&track))?,
                store.media_mime(&book, &track)?,
            ))
        })
        .and_then(|(source, mime)| Ok((source.open()?, mime)));
    let Ok((mut file, mime)) = opened else {
        return response(socket, 410, 0, "", write_timeout);
    };
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let range = match byte_range(get("range"), size) {
        Ok(range) => range,
        Err(_) => {
            return response(
                socket,
                416,
                0,
                &format!("Content-Range: bytes */{size}\r\n"),
                write_timeout,
            )
        }
    };
    let (start, length, status) = match range {
        Some((start, end)) => (start, end - start + 1, 206),
        None => (0, size, 200),
    };
    file.seek(SeekFrom::Start(start))
        .map_err(|e| e.to_string())?;
    let mut headers =
        format!("Content-Type: {mime}\r\nAccept-Ranges: bytes\r\nReferrer-Policy: no-referrer\r\n");
    if let Some(origin) = origin {
        headers.push_str(&format!(
            "Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\n"
        ));
    }
    if let Some((start, end)) = range {
        headers.push_str(&format!("Content-Range: bytes {start}-{end}/{size}\r\n"));
    }
    response(socket, status, length, &headers, write_timeout)?;
    if request.method == "HEAD" {
        return Ok(());
    }
    let mut remaining = length;
    let mut buffer = [0u8; 64 * 1024];
    while remaining > 0 {
        let length = (remaining.min(buffer.len() as u64)) as usize;
        let read = file
            .read(&mut buffer[..length])
            .map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("Source truncated during playback".into());
        }
        write_deadline(socket, &buffer[..read], write_timeout)?;
        remaining -= read as u64;
    }
    Ok(())
}
