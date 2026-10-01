//! Bounded, read-only metadata lookups. Unlike the research scraper, automatic
//! title lookup must never reach private networks or fetch HTML subresources.
use crate::async_util::BoxFuture;
use crate::error::{CoreError, Result};
use reqwest::{header, redirect::Policy, Client, StatusCode};
use scraper::{Html, Selector};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use url::{Host, Url};

const TIMEOUT: Duration = Duration::from_secs(8);
const MAX_BYTES: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const MAX_TITLE_BYTES: usize = 512;

fn unavailable(reason: &str) -> CoreError {
    CoreError::Other(format!("Study title unavailable: {reason}"))
}

fn validate_url(raw: &str) -> Result<Url> {
    if raw.len() > 8192 || raw.chars().any(|c| c.is_control() || c == '\\') {
        return Err(unavailable("invalid URL"));
    }
    let mut url =
        Url::parse(raw.trim()).map_err(|_| unavailable("enter a complete HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() == Some(0)
    {
        return Err(unavailable(
            "only HTTP(S) URLs without credentials are supported",
        ));
    }
    if let Some(host) = url.host_str() {
        let host = host.trim_end_matches('.');
        if host.eq_ignore_ascii_case("localhost")
            || [".localhost", ".local", ".internal", ".onion"]
                .iter()
                .any(|suffix| host.ends_with(suffix))
        {
            return Err(unavailable(
                "local and private network addresses are blocked",
            ));
        }
    }
    url.set_fragment(None);
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99)))
                || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let parts = ip.segments();
            // Accept only global unicast, excluding documentation, transition,
            // protocol-assignment and tunneling ranges (including mapped IPv4).
            parts[0] & 0xe000 == 0x2000
                && parts[0] != 0x2002
                && !(parts[0] == 0x2001 && (parts[1] < 0x0200 || parts[1] == 0x0db8))
                && !(parts[0] == 0x3fff && parts[1] < 0x1000)
        }
    }
}

trait Resolver: Send + Sync {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Result<Vec<SocketAddr>>>;

    fn permits(&self, address: SocketAddr) -> bool {
        public_ip(address.ip())
    }
}

struct SystemResolver;

impl Resolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Result<Vec<SocketAddr>>> {
        Box::pin(async move {
            tokio::net::lookup_host((host, port))
                .await
                .map(|addresses| addresses.collect())
                .map_err(|_| unavailable("host could not be resolved"))
        })
    }
}

#[derive(Clone, Copy)]
enum Format {
    Html,
    Oembed,
}

fn request_target(source: &str) -> Result<(Url, Format)> {
    let url = validate_url(source)?;
    let host = url.host_str().unwrap_or_default();
    if ![
        "youtu.be",
        "youtube.com",
        "www.youtube.com",
        "m.youtube.com",
        "youtube-nocookie.com",
        "www.youtube-nocookie.com",
    ]
    .contains(&host)
    {
        return Ok((url, Format::Html));
    }
    if url.port().is_some() {
        return Err(unavailable("invalid YouTube video URL"));
    }
    let path = url.path().strip_suffix('/').unwrap_or(url.path());
    let parts: Vec<_> = path.strip_prefix('/').unwrap_or(path).split('/').collect();
    let id = if host == "youtu.be" {
        match parts.as_slice() {
            [id] => Some((*id).to_string()),
            _ => None,
        }
    } else if url.path() == "/watch" && !host.contains("nocookie") {
        let ids: Vec<_> = url.query_pairs().filter(|(key, _)| key == "v").collect();
        if ids.len() == 1 {
            Some(ids[0].1.to_string())
        } else {
            None
        }
    } else {
        match parts.as_slice() {
            ["embed" | "shorts", id] => Some((*id).to_string()),
            _ => None,
        }
    }
    .filter(|id| {
        id.len() == 11
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    })
    .ok_or_else(|| unavailable("enter a supported YouTube video URL"))?;
    // Send only the public video identity to YouTube, not source query tokens.
    let canonical = format!("https://www.youtube.com/watch?v={id}");
    let mut endpoint = Url::parse("https://www.youtube.com/oembed").expect("fixed valid URL");
    endpoint
        .query_pairs_mut()
        .append_pair("url", &canonical)
        .append_pair("format", "json");
    Ok((endpoint, Format::Oembed))
}

/// Fetches a plain title without touching a graph, executing scripts, using a
/// model, or downloading media. Errors intentionally omit source URLs/tokens.
pub async fn fetch_study_link_title(source: &str) -> Result<String> {
    let (url, format) = request_target(source)?;
    lookup_with_timeout(url, format, &SystemResolver, TIMEOUT).await
}

async fn lookup_with_timeout(
    url: Url,
    format: Format,
    resolver: &impl Resolver,
    timeout: Duration,
) -> Result<String> {
    tokio::time::timeout(timeout, fetch_title(url, format, resolver))
        .await
        .map_err(|_| unavailable("lookup timed out"))?
}

async fn pinned_client(url: &Url, resolver: &impl Resolver) -> Result<Client> {
    let host = url.host_str().ok_or_else(|| unavailable("missing host"))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| unavailable("missing port"))?;
    let addresses = match url.host() {
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(IpAddr::V4(ip), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(IpAddr::V6(ip), port)],
        _ => resolver.resolve(host, port).await?,
    };
    if addresses.is_empty() || addresses.len() > 64 {
        return Err(unavailable("host returned no usable addresses"));
    }
    if addresses.iter().any(|&address| !resolver.permits(address)) {
        return Err(unavailable(
            "local and private network addresses are blocked",
        ));
    }
    Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .referer(false)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(3))
        .user_agent("Grafium Studies")
        // A fresh client per hop prevents connection reuse from bypassing the
        // new DNS validation. TLS still validates the requested hostname.
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| unavailable("could not initialize the HTTP client"))
}

fn redirect_target(url: &Url, location: &str) -> Result<Url> {
    if location.chars().any(|c| c.is_control() || c == '\\') {
        return Err(unavailable("invalid redirect location"));
    }
    let next = url
        .join(location)
        .map_err(|_| unavailable("invalid redirect location"))?;
    let next = validate_url(next.as_str())?;
    if url.scheme() == "https" && next.scheme() != "https" {
        return Err(unavailable("insecure redirect was blocked"));
    }
    Ok(next)
}

async fn fetch_title(mut url: Url, format: Format, resolver: &impl Resolver) -> Result<String> {
    for hop in 0..=MAX_REDIRECTS {
        let client = pinned_client(&url, resolver).await?;
        let mut response = client
            .get(url.clone())
            .header(
                header::ACCEPT,
                match format {
                    Format::Html => "text/html,application/xhtml+xml",
                    Format::Oembed => "application/json",
                },
            )
            .header(header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|_| unavailable("HTTP request failed"))?;
        if matches!(
            response.status(),
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            if hop == MAX_REDIRECTS {
                return Err(unavailable("too many redirects"));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| unavailable("redirect has no valid location"))?;
            url = redirect_target(&url, location)?;
            continue;
        }
        if !response.status().is_success() {
            return Err(unavailable(&format!(
                "server returned HTTP {}",
                response.status().as_u16()
            )));
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let supported = match format {
            Format::Html => matches!(content_type.as_str(), "text/html" | "application/xhtml+xml"),
            Format::Oembed => content_type == "application/json",
        };
        if !supported {
            return Err(unavailable(
                "the response is not supported title metadata (media is not downloaded)",
            ));
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|value| value != "identity")
        {
            return Err(unavailable("compressed metadata is not supported"));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(unavailable("metadata exceeds the 1 MiB limit"));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| unavailable("could not read metadata"))?
        {
            if chunk.len() > MAX_BYTES - body.len() {
                return Err(unavailable("metadata exceeds the 1 MiB limit"));
            }
            body.extend_from_slice(&chunk);
        }
        return match format {
            Format::Html => html_title(&body),
            Format::Oembed => oembed_title(&body),
        };
    }
    Err(unavailable("too many redirects"))
}

fn normalized_title(raw: &str) -> Result<String> {
    let text: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let end = crate::ai::text::char_boundary_prefix_end(&text, MAX_TITLE_BYTES);
    let title = text[..end].trim().to_string();
    if title.is_empty() {
        Err(unavailable("no title was provided"))
    } else {
        Ok(title)
    }
}

fn html_title(body: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(body).map_err(|_| unavailable("page is not valid UTF-8"))?;
    let document = Html::parse_document(text);
    let metas = Selector::parse("meta[property], meta[name]").expect("fixed valid selector");
    for node in document.select(&metas) {
        let property = node
            .value()
            .attr("property")
            .or_else(|| node.value().attr("name"));
        if property.is_some_and(|property| property.eq_ignore_ascii_case("og:title")) {
            if let Some(title) = node.value().attr("content") {
                if let Ok(title) = normalized_title(title) {
                    return Ok(title);
                }
            }
        }
    }
    let titles = Selector::parse("title").expect("fixed valid selector");
    for node in document.select(&titles) {
        if let Ok(title) = normalized_title(&node.text().collect::<String>()) {
            return Ok(title);
        }
    }
    Err(unavailable("page has no title"))
}

fn oembed_title(body: &[u8]) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Video {
        title: String,
        #[serde(rename = "type")]
        kind: String,
    }
    let video: Video =
        serde_json::from_slice(body).map_err(|_| unavailable("invalid YouTube metadata"))?;
    if video.kind != "video" {
        return Err(unavailable("YouTube metadata is not a video"));
    }
    normalized_title(&video.title)
}

#[cfg(test)]
mod tests;
