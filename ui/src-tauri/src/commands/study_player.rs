//! A separate, unprivileged HTTP origin gives embedded players a real Referer.
//! Only one immutable wrapper is served: never graph assets, files, or IPC.
use std::sync::OnceLock;
use tiny_http::{Header, Method, Response, Server, StatusCode};

const PLAYER: &str = include_str!("../../resources/study-player.html");
static PLAYER_URL: OnceLock<Result<String, String>> = OnceLock::new();

fn start_player_server() -> Result<String, String> {
    let server = Server::http("127.0.0.1:0")
        .map_err(|e| format!("Could not start the study player: {e}"))?;
    let address = server
        .server_addr()
        .to_ip()
        .ok_or("Study player did not bind a local address")?;
    let route = format!("/{}/player", uuid::Uuid::new_v4());
    let url = format!("http://{address}{route}");
    let host = address.to_string();
    let nonce = uuid::Uuid::new_v4().to_string();
    let html = PLAYER.replace("__SCRIPT_NONCE__", &nonce);
    let csp = format!("default-src 'none'; script-src 'nonce-{nonce}'; style-src 'nonce-{nonce}'; frame-src https://www.youtube-nocookie.com; base-uri 'none'; form-action 'none'; object-src 'none'");
    let headers = [
        ("Content-Type", "text/html; charset=utf-8".to_string()),
        ("Cache-Control", "no-store".to_string()),
        (
            "Referrer-Policy",
            "strict-origin-when-cross-origin".to_string(),
        ),
        ("X-Content-Type-Options", "nosniff".to_string()),
        ("Content-Security-Policy", csp),
    ]
    .into_iter()
    .map(|(name, value)| {
        Header::from_bytes(name, value)
            .map_err(|_| "Invalid study player response header".to_string())
    })
    .collect::<Result<Vec<_>, _>>()?;
    std::thread::Builder::new()
        .name("study-player".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                let valid_host = request
                    .headers()
                    .iter()
                    .any(|header| header.field.equiv("Host") && header.value.as_str() == host);
                let response =
                    if request.method() == &Method::Get && request.url() == route && valid_host {
                        let mut response = Response::from_string(html.clone());
                        for header in &headers {
                            response.add_header(header.clone());
                        }
                        response
                    } else {
                        Response::from_string("Not found").with_status_code(StatusCode(404))
                    };
                if let Err(error) = request.respond(response) {
                    tracing::warn!("Study player response failed: {error}");
                }
            }
        })
        .map_err(|e| format!("Could not start the study player worker: {e}"))?;
    Ok(url)
}

#[tauri::command(rename_all = "camelCase")]
pub fn study_youtube_embed(video_id: String, start: f64) -> Result<String, String> {
    if video_id.len() != 11
        || !video_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        || !start.is_finite()
        || !(0.0..=1e12).contains(&start)
    {
        return Err("Invalid YouTube video or resume timestamp".into());
    }
    let url = PLAYER_URL
        .get_or_init(start_player_server)
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(format!("{url}#video={video_id}&start={}", start.floor()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn request(url: &str, path: &str, host: Option<&str>) -> String {
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            host.unwrap_or(address)
        )
        .unwrap();
        let mut result = String::new();
        stream.read_to_string(&mut result).unwrap();
        result
    }

    #[test]
    fn player_serves_only_its_fixed_wrapper_without_graph_or_ipc_access() {
        let url = study_youtube_embed("aqz-KE-bpKQ".into(), 42.8).unwrap();
        assert!(url.ends_with("#video=aqz-KE-bpKQ&start=42"));
        let path = format!(
            "/{}",
            url.strip_prefix("http://")
                .unwrap()
                .split_once('/')
                .unwrap()
                .1
                .split('#')
                .next()
                .unwrap()
        );
        let response = request(&url, &path, None);
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.to_lowercase().contains("content-security-policy:"));
        assert!(response.contains("https://com.grafium.app/"));
        assert!(!response.contains("__SCRIPT_NONCE__"));
        assert!(!response.contains("__TAURI"));
        assert!(request(&url, "/", None).starts_with("HTTP/1.1 404"));
        assert!(request(&url, &path, Some("external.example")).starts_with("HTTP/1.1 404"));
        assert!(request(&url, "/../../pages/private.md", None).starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn player_rejects_non_video_inputs_and_invalid_timestamps() {
        for id in [
            "",
            "../anything",
            "aqz-KE-bpKQ<script>",
            "https://example.com",
        ] {
            assert!(study_youtube_embed(id.into(), 0.0).is_err());
        }
        for time in [-1.0, f64::NAN, f64::INFINITY, 1e13] {
            assert!(study_youtube_embed("aqz-KE-bpKQ".into(), time).is_err());
        }
    }
}
