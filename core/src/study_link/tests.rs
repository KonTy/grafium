use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    url: Url,
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(responses: Vec<String>) -> Self {
        Self::with_response_delay(responses, Duration::ZERO).await
    }

    async fn with_response_delay(responses: Vec<String>, delay: Duration) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let response = response.replace("__FIXTURE_PORT__", &address.port().to_string());
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let count = stream.read(&mut chunk).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..count]);
                    assert!(request.len() < 16 * 1024);
                }
                received
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                // Oversize/media rejection may close the stream before the
                // fixture finishes sending its deliberately unwanted body.
                let _ = stream.write_all(response.as_bytes()).await;
                tokio::time::sleep(delay).await;
            }
        });
        Self {
            url: Url::parse(&format!(
                "http://metadata.example:{}/page?token=private",
                address.port()
            ))
            .unwrap(),
            address,
            requests,
            task,
        }
    }

    fn resolver(&self) -> FixtureResolver {
        FixtureResolver {
            address: self.address,
            calls: AtomicUsize::new(0),
            rebind: false,
            cross_host: None,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct FixtureResolver {
    address: SocketAddr,
    calls: AtomicUsize,
    rebind: bool,
    cross_host: Option<(&'static str, SocketAddr)>,
}

impl Resolver for FixtureResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> BoxFuture<'a, Result<Vec<SocketAddr>>> {
        Box::pin(async move {
            let address = match self.cross_host {
                Some((cross_host, address)) if host == cross_host => address,
                _ => {
                    assert_eq!(host, "metadata.example");
                    self.address
                }
            };
            assert_eq!(port, address.port());
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![if self.rebind && call > 0 {
                "10.0.0.1:80".parse().unwrap()
            } else {
                address
            }])
        })
    }

    fn permits(&self, address: SocketAddr) -> bool {
        // The sole private-address exception exists only in this test module,
        // scoped to the exact ephemeral listener created by the fixture.
        address == self.address || public_ip(address.ip())
    }
}

fn response(content_type: &str, body: &str) -> String {
    format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

fn redirect(location: &str) -> String {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nSet-Cookie: secret=forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

#[test]
fn title_parser_prefers_open_graph_and_decodes_entities_as_plain_text() {
    let html = br#"<title>Fallback</title><meta property="og:title" content=" A &amp; B &#x2014; &lt;script&gt;  &#10; C ">
        <script>throw new Error("never executed")</script>"#;
    assert_eq!(html_title(html).unwrap(), "A & B — <script> C");
    assert_eq!(
        html_title(b"<meta property='og:title' content=' '><title>A &amp; B\n\t C</title>")
            .unwrap(),
        "A & B C"
    );
    assert_eq!(
        html_title(b"<meta name='OG:TITLE' content='Title'>").unwrap(),
        "Title"
    );
    assert!(html_title(b"<html>no title</html>").is_err());
    assert!(html_title(b"<title>\xff</title>").is_err());
}

#[test]
fn titles_are_capped_on_utf8_boundaries_and_never_empty() {
    let title = normalized_title(&"界".repeat(200)).unwrap();
    assert_eq!(title.len(), 510);
    assert_eq!(title.chars().count(), 170);
    assert_eq!(normalized_title(&"x".repeat(600)).unwrap().len(), 512);
    assert_eq!(
        normalized_title(&("x".repeat(511) + "😀")).unwrap().len(),
        511
    );
    assert_eq!(normalized_title("a\u{0000}b\u{00a0}c").unwrap(), "a b c");
    assert!(normalized_title("\n\t\u{0000} ").is_err());
}

#[test]
fn oembed_requires_a_video_and_a_string_title() {
    assert_eq!(
        oembed_title(
            br#"{"type":"video","title":"  Example \n title ","html":"<script>ignored</script>"}"#
        )
        .unwrap(),
        "Example title"
    );
    for json in [
        r#"{"type":"photo","title":"x"}"#,
        r#"{"type":"video","title":12}"#,
        r#"{"type":"video","title":" "}"#,
        r#"{"title":"x"}"#,
        "null",
        "not-json",
    ] {
        assert!(oembed_title(json.as_bytes()).is_err(), "{json}");
    }
}

#[test]
fn youtube_targets_official_oembed_with_only_canonical_video_identity() {
    for input in [
        "https://youtu.be/dQw4w9WgXcQ?token=private",
        "http://www.youtube.com/watch?v=dQw4w9WgXcQ&secret=private",
        "https://m.youtube.com/shorts/dQw4w9WgXcQ",
        "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ/",
    ] {
        let (url, format) = request_target(input).unwrap();
        assert!(matches!(format, Format::Oembed));
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("www.youtube.com"));
        assert_eq!(url.path(), "/oembed");
        let params: Vec<_> = url.query_pairs().collect();
        assert_eq!(
            params,
            vec![
                (
                    "url".into(),
                    "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()
                ),
                ("format".into(), "json".into()),
            ]
        );
        assert!(!url.as_str().contains("private"));
    }
    for input in [
        "https://youtu.be/short",
        "https://youtu.be//dQw4w9WgXcQ",
        "https://youtube.com/watch?v=dQw4w9WgXcQ&v=dQw4w9WgXcQ",
        "https://youtube.com:444/watch?v=dQw4w9WgXcQ",
        "https://youtube.com/playlist?list=dQw4w9WgXcQ",
        "https://youtube-nocookie.com/watch?v=dQw4w9WgXcQ",
        "https://youtube.com/embed/dQw4w9WgXcQ/extra",
    ] {
        assert!(request_target(input).is_err(), "{input}");
    }
    let (url, format) = request_target("http://youtube.com.evil.example/page").unwrap();
    assert!(matches!(format, Format::Html));
    assert_eq!(url.host_str(), Some("youtube.com.evil.example"));
}

#[test]
fn unsafe_urls_are_rejected_without_echoing_secrets() {
    for input in [
        "file:///private",
        "javascript:alert(1)",
        "ftp://example.com/private",
        "https://user:private@example.com",
        "http://localhost/private",
        "http://service.local/private",
        "http://example.com\\@localhost/private",
        "https://example.com/\nprivate",
        "https://example.com/\u{007f}private",
        "http://example.com:0/private",
    ] {
        let message = validate_url(input).unwrap_err().to_string();
        assert!(!message.contains(input), "{message}");
    }
    assert!(validate_url(&format!("https://example.com/{}", "x".repeat(8192))).is_err());
    assert_eq!(
        validate_url("http://example.com/path#fragment")
            .unwrap()
            .as_str(),
        "http://example.com/path"
    );
}

#[test]
fn private_reserved_and_transition_addresses_are_blocked() {
    for ip in [
        "0.0.0.0",
        "10.1.2.3",
        "100.64.0.1",
        "100.127.255.255",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "192.0.0.8",
        "192.0.2.1",
        "192.88.99.1",
        "198.18.0.1",
        "198.19.0.1",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::a00:1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "2001::1",
        "2001:db8::1",
        "2002:7f00:1::",
        "3fff::1",
    ] {
        assert!(!public_ip(ip.parse().unwrap()), "{ip}");
    }
    for ip in [
        "8.8.8.8",
        "1.1.1.1",
        "100.128.0.1",
        "172.32.0.1",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
    ] {
        assert!(public_ip(ip.parse().unwrap()), "{ip}");
    }
}

struct FixedResolver(Vec<SocketAddr>);
impl Resolver for FixedResolver {
    fn resolve<'a>(&'a self, _: &'a str, _: u16) -> BoxFuture<'a, Result<Vec<SocketAddr>>> {
        Box::pin(async { Ok(self.0.clone()) })
    }
}

#[tokio::test]
async fn private_dns_and_obfuscated_ip_literals_are_rejected_before_connection() {
    let resolver = FixedResolver(vec![
        "8.8.8.8:80".parse().unwrap(),
        "10.1.2.3:80".parse().unwrap(),
    ]);
    assert!(
        pinned_client(&validate_url("http://example.com").unwrap(), &resolver)
            .await
            .is_err()
    );
    for url in [
        "http://2130706433",
        "http://0x7f000001",
        "http://127.1",
        "http://[::1]",
        "http://[::ffff:127.0.0.1]",
    ] {
        let result = pinned_client(&validate_url(url).unwrap(), &SystemResolver).await;
        assert!(result.is_err(), "{url}");
    }
}

#[tokio::test]
async fn pinned_transport_fetches_html_without_loading_subresources_or_sending_auth() {
    let fixture = Fixture::new(vec![response("text/html; charset=utf-8",
        "<title>Fetched &amp; decoded</title><img src='http://127.0.0.1/private'><script src='/code.js'></script>")]).await;
    let title = fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
        .await
        .unwrap();
    assert_eq!(title, "Fetched & decoded");
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let headers = requests[0].to_ascii_lowercase();
    assert!(headers.contains("host: metadata.example:"));
    assert!(!headers.contains("authorization:"));
    assert!(!headers.contains("cookie:"));
    assert!(!headers.contains("referer:"));
}

#[tokio::test]
async fn redirects_revalidate_dns_and_never_send_cookies_or_referers() {
    let fixture = Fixture::new(vec![
        redirect("/next"),
        response("text/html", "<title>Next</title>"),
    ])
    .await;
    let resolver = fixture.resolver();
    assert_eq!(
        fetch_title(fixture.url.clone(), Format::Html, &resolver)
            .await
            .unwrap(),
        "Next"
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let second = requests[1].to_ascii_lowercase();
    assert!(!second.contains("cookie:"));
    assert!(!second.contains("referer:"));
    assert!(!second.contains("private"));
}

#[tokio::test]
async fn dns_rebinding_on_a_redirect_cannot_reach_private_addresses() {
    let fixture = Fixture::new(vec![redirect("/next")]).await;
    let mut resolver = fixture.resolver();
    resolver.rebind = true;
    let error = fetch_title(fixture.url.clone(), Format::Html, &resolver)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("private network"));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unsafe_redirects_are_rejected() {
    for location in [
        "http://127.0.0.1/private",
        "http://user:private@metadata.example/private",
        "file:///private",
    ] {
        let fixture = Fixture::new(vec![redirect(location)]).await;
        let error = fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains(location));
        assert!(!error.contains("user:private"));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn public_cross_host_redirects_are_pinned_without_leaking_original_headers_or_query() {
    let fixture = Fixture::new(vec![
        redirect("http://www.metadata.example:__FIXTURE_PORT__/next?from=location"),
        response("text/html", "<title>Public destination</title>"),
    ])
    .await;
    // Both public-host fixtures use the same exact test-only listener exception.
    let mut resolver = fixture.resolver();
    resolver.cross_host = Some(("www.metadata.example", fixture.address));
    assert_eq!(
        fetch_title(fixture.url.clone(), Format::Html, &resolver)
            .await
            .unwrap(),
        "Public destination"
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let second = requests[1].to_ascii_lowercase();
    assert!(second.starts_with("get /next?from=location http/1.1"));
    assert!(second.contains("host: www.metadata.example:"));
    for forbidden in ["authorization:", "cookie:", "referer:", "token=", "private"] {
        assert!(!second.contains(forbidden), "{forbidden}");
    }
}

#[tokio::test]
async fn cross_host_redirects_to_private_dns_answers_are_rejected_before_connection() {
    let fixture = Fixture::new(vec![redirect("http://private.example/next")]).await;
    let mut resolver = fixture.resolver();
    resolver.cross_host = Some(("private.example", "10.0.0.1:80".parse().unwrap()));
    let error = fetch_title(fixture.url.clone(), Format::Html, &resolver)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("private network"));
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[test]
fn redirect_targets_preserve_downgrade_and_credential_protection() {
    let source = validate_url("https://example.com/page?token=private").unwrap();
    assert!(redirect_target(&source, "http://www.example.com/next").is_err());
    assert!(redirect_target(&source, "https://user:private@www.example.com/next").is_err());
    assert_eq!(
        redirect_target(&source, "https://www.example.com/next?from=location")
            .unwrap()
            .as_str(),
        "https://www.example.com/next?from=location"
    );
}
#[tokio::test]
async fn redirects_require_a_location() {
    let fixture = Fixture::new(vec![
        "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n".into()
    ])
    .await;
    assert!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap_err()
            .to_string()
            .contains("location")
    );
}

#[tokio::test]
async fn redirects_allow_three_hops_but_not_four() {
    let fixture = Fixture::new(vec![
        redirect("/1"),
        redirect("/2"),
        redirect("/3"),
        response("text/html", "<title>Third</title>"),
    ])
    .await;
    assert_eq!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap(),
        "Third"
    );
    let fixture = Fixture::new(vec![
        redirect("/1"),
        redirect("/2"),
        redirect("/3"),
        redirect("/4"),
    ])
    .await;
    assert!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap_err()
            .to_string()
            .contains("too many redirects")
    );
    assert_eq!(fixture.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn binary_content_is_rejected_without_waiting_for_the_media_body() {
    let fixture = Fixture::new(vec![
        "HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 999999999\r\n\r\n".into(),
    ])
    .await;
    let error = fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("media is not downloaded"));
}

#[tokio::test]
async fn declared_and_streamed_oversize_payloads_are_rejected() {
    let fixture = Fixture::new(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
        MAX_BYTES + 1
    )])
    .await;
    assert!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap_err()
            .to_string()
            .contains("1 MiB")
    );
    let body = "x".repeat(MAX_BYTES + 1);
    let fixture = Fixture::new(vec![format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len())]).await;
    assert!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap_err()
            .to_string()
            .contains("1 MiB")
    );
}

#[tokio::test]
async fn payload_at_exactly_one_mib_is_accepted() {
    let prefix = "<title>Bounded title</title><!--";
    let body = format!("{prefix}{}-->", "x".repeat(MAX_BYTES - prefix.len() - 3));
    assert_eq!(body.len(), MAX_BYTES);
    let fixture = Fixture::new(vec![response("text/html", &body)]).await;
    assert_eq!(
        fetch_title(fixture.url.clone(), Format::Html, &fixture.resolver())
            .await
            .unwrap(),
        "Bounded title"
    );
}

#[tokio::test]
async fn oembed_transport_requires_json_and_propagates_explicit_errors() {
    let fixture = Fixture::new(vec![response(
        "application/json",
        r#"{"type":"video","title":"Video title"}"#,
    )])
    .await;
    assert_eq!(
        fetch_title(fixture.url.clone(), Format::Oembed, &fixture.resolver())
            .await
            .unwrap(),
        "Video title"
    );
    for raw in [
        response("text/html", "<title>Not JSON</title>"),
        response("application/json", r#"{"error":"not found"}"#),
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".into(),
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n".into(),
    ] {
        let fixture = Fixture::new(vec![raw]).await;
        assert!(fetch_title(fixture.url.clone(), Format::Oembed, &fixture.resolver()).await.is_err());
    }
}

struct SlowResolver;
impl Resolver for SlowResolver {
    fn resolve<'a>(&'a self, _: &'a str, _: u16) -> BoxFuture<'a, Result<Vec<SocketAddr>>> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            unreachable!()
        })
    }
}

#[tokio::test]
async fn overall_deadline_includes_dns_resolution() {
    let url = validate_url("https://example.com/?secret=private").unwrap();
    let error = lookup_with_timeout(url, Format::Html, &SlowResolver, Duration::from_millis(20))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(!error.to_string().contains("secret"));
}

#[tokio::test]
async fn overall_deadline_includes_stalled_body_reads() {
    let fixture = Fixture::with_response_delay(
        vec![
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 100\r\n\r\n<title>"
                .into(),
        ],
        Duration::from_secs(30),
    )
    .await;
    let error = lookup_with_timeout(
        fixture.url.clone(),
        Format::Html,
        &fixture.resolver(),
        Duration::from_millis(30),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}
