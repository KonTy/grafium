//! All requests use synthetic content and in-process loopback servers.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use model_runtime::{
    error::{Result, RuntimeError},
    providers::{
        anthropic::AnthropicLlm,
        ollama::{OllamaEmbedder, OllamaLlm},
        openai_compatible::{OpenAiCompatibleEmbedder, OpenAiCompatibleLlm},
        NetworkConfig, ResponseLimits,
    },
    types::{ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole},
};
use serde_json::{json, Value};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

fn network() -> NetworkConfig {
    NetworkConfig::from_client(
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap(),
        Arc::new(|_: &url::Url| Ok(())),
    )
}

async fn request(socket: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let (headers, start) = loop {
        let mut buffer = [0; 2048];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < 64 * 1024);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break (String::from_utf8(bytes[..i].to_vec()).unwrap(), i + 4);
        }
    };
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < start + length {
        let mut buffer = [0; 2048];
        let count = socket.read(&mut buffer).await.unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[start..start + length]).unwrap()
    };
    (headers, body)
}

async fn serve_response(
    status: &str,
    content_type: &str,
    body: &str,
    fragment: usize,
) -> (String, oneshot::Receiver<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let captured = request(&mut socket).await;
        let _ = tx.send(captured);
        for chunk in response.as_bytes().chunks(fragment.max(1)) {
            if socket.write_all(chunk).await.is_err() {
                break;
            }
            tokio::task::yield_now().await;
        }
    });
    (base, rx)
}

fn messages() -> Vec<ChatMessage> {
    vec![
        ChatMessage {
            role: MessageRole::System,
            content: "source system".into(),
        },
        ChatMessage {
            role: MessageRole::User,
            content: "synthetic source [1]".into(),
        },
        ChatMessage {
            role: MessageRole::Assistant,
            content: "previous answer".into(),
        },
        ChatMessage {
            role: MessageRole::User,
            content: "question".into(),
        },
    ]
}

fn options() -> CompletionOptions {
    CompletionOptions {
        max_tokens: Some(19),
        temperature: Some(0.25),
        system_prompt: Some("host prompt".into()),
        stop: Some(vec!["STOP".into()]),
        cancel: None,
    }
}

#[tokio::test]
async fn compatible_preserves_payload_roles_auth_and_json_completion() {
    let (base, captured) = serve_response(
        "200 OK",
        "application/json",
        r#"{"choices":[{"message":{"content":"answer"}}]}"#,
        17,
    )
    .await;
    let provider = OpenAiCompatibleLlm::with_network(
        &format!("{base}/v1/"),
        "synthetic",
        Some("synthetic-key".into()),
        network(),
    )
    .unwrap();
    assert_eq!(
        provider.complete(&messages(), &options()).await.unwrap(),
        "answer"
    );
    let (headers, body) = captured.await.unwrap();
    assert!(headers.starts_with("POST /v1/chat/completions "));
    assert!(headers
        .to_lowercase()
        .contains("authorization: bearer synthetic-key"));
    assert_eq!(
        body,
        json!({
            "model":"synthetic",
            "messages":[
                {"role":"system","content":"host prompt"},
                {"role":"system","content":"source system"},
                {"role":"user","content":"synthetic source [1]"},
                {"role":"assistant","content":"previous answer"},
                {"role":"user","content":"question"}
            ],
            "max_tokens":19,"temperature":0.25,"stop":["STOP"]
        })
    );
}

#[tokio::test]
async fn compatible_streams_before_terminal_event_and_preserves_utf8() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let (emitted_tx, emitted_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (_, body) = request(&mut socket).await;
        assert_eq!(body["stream"], true);
        assert!(body.get("tools").is_none());
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let first = ": keepalive\r\nevent: message\r\ndata: {\"choices\":[\r\ndata: {\"index\":0,\"delta\":{\"content\":\"h\u{e9}\"}}]}\r\n\r\n";
        for byte in first.as_bytes() {
            socket.write_all(&[*byte]).await.unwrap();
        }
        // A buffered implementation deadlocks here: the server only finishes
        // after the caller has actually observed the first delta.
        tokio::time::timeout(Duration::from_secs(2), emitted_rx)
            .await
            .unwrap()
            .unwrap();
        socket.write_all(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"}}]}\n\ndata: [DONE]\n\n").await.unwrap();
    });
    let provider = OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
    let mut emitted = Some(emitted_tx);
    let mut deltas = Vec::new();
    let result = provider
        .complete_stream(&messages(), &options(), &mut |delta| {
            deltas.push(delta.to_string());
            if let Some(tx) = emitted.take() {
                let _ = tx.send(());
            }
        })
        .await
        .unwrap();
    assert_eq!(result, "héllo");
    assert_eq!(deltas, ["hé", "llo"]);
    server.await.unwrap();
}

#[tokio::test]
async fn compatible_stream_allows_servers_that_return_json_instead() {
    let (base, _) = serve_response(
        "200 OK",
        "application/json; charset=utf-8",
        r#"{"choices":[{"message":{"content":"answer"}}]}"#,
        31,
    )
    .await;
    let provider = OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
    let mut deltas = Vec::new();
    assert_eq!(
        provider
            .complete_stream(&[], &options(), &mut |s| deltas.push(s.to_string()))
            .await
            .unwrap(),
        "answer"
    );
    assert_eq!(deltas, ["answer"]);
}

#[tokio::test]
async fn sse_truncation_and_remote_error_do_not_return_partial_success() {
    for suffix in [
        "",
        "data: {\"error\":{\"message\":\"synthetic failure\"}}\n\n",
        "data: {\"choices\":[],\"error\":{\"message\":\"synthetic failure\"}}\n\ndata: [DONE]\n\n",
        "data: invalid json\n\n",
    ] {
        let body = format!("data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"partial\"}}}}]}}\n\n{suffix}");
        let (base, _) = serve_response("200 OK", "text/event-stream", &body, 19).await;
        let provider =
            OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
        let mut deltas = String::new();
        assert!(provider
            .complete_stream(&[], &options(), &mut |s| deltas.push_str(s))
            .await
            .is_err());
        assert_eq!(deltas, "partial");
    }
}

#[tokio::test]
async fn cancellation_before_send_and_after_delta_is_an_error() {
    let cancelled = Arc::new(AtomicBool::new(true));
    let provider =
        OpenAiCompatibleLlm::with_network("http://127.0.0.1:1", "synthetic", None, network())
            .unwrap();
    let mut options = CompletionOptions {
        cancel: Some(cancelled.clone()),
        ..Default::default()
    };
    assert!(matches!(
        provider.complete(&[], &options).await,
        Err(RuntimeError::Cancelled)
    ));
    cancelled.store(false, Ordering::Release);
    let (base, _) = serve_response("200 OK", "text/event-stream",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\ndata: [DONE]\n\n", 11).await;
    let provider = OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
    options.cancel = Some(cancelled.clone());
    assert!(matches!(
        provider
            .complete_stream(&[], &options, &mut |_| cancelled
                .store(true, Ordering::Release))
            .await,
        Err(RuntimeError::Cancelled)
    ));
}

#[tokio::test]
async fn cancellation_interrupts_waiting_for_headers_and_a_stalled_body() {
    for headers in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            if headers {
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n").await.unwrap();
            }
            ready_tx.send(()).unwrap();
            let mut byte = [0];
            let _ = socket.read(&mut byte).await;
        });
        let flag = Arc::new(AtomicBool::new(false));
        let cancel = flag.clone();
        let setter = tokio::spawn(async move {
            ready_rx.await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.store(true, Ordering::Release);
        });
        let provider =
            OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
        let options = CompletionOptions {
            cancel: Some(flag),
            ..Default::default()
        };
        let result = tokio::time::timeout(Duration::from_secs(1), provider.complete(&[], &options))
            .await
            .unwrap();
        assert!(matches!(result, Err(RuntimeError::Cancelled)));
        setter.await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn ollama_payload_and_ndjson_stream_are_preserved() {
    let body = "{\"message\":{\"content\":\"one\"},\"done\":false}\n{\"message\":{\"content\":\" two\"},\"done\":false}\n{\"done\":true}";
    let (base, captured) = serve_response("200 OK", "application/x-ndjson", body, 3).await;
    let provider = OllamaLlm::with_network(&base, "synthetic", network()).unwrap();
    let mut deltas = Vec::new();
    assert_eq!(
        provider
            .complete_stream(&messages(), &options(), &mut |s| deltas.push(s.to_string()))
            .await
            .unwrap(),
        "one two"
    );
    assert_eq!(deltas, ["one", " two"]);
    let (headers, body) = captured.await.unwrap();
    assert!(headers.starts_with("POST /api/chat "));
    assert_eq!(body["stream"], true);
    assert_eq!(
        body["options"],
        json!({"temperature":0.25,"num_predict":19,"stop":["STOP"]})
    );
    assert_eq!(
        body["messages"][0],
        json!({"role":"system","content":"host prompt"})
    );
    assert_eq!(
        body["messages"][1],
        json!({"role":"system","content":"source system"})
    );
    assert_eq!(
        body["messages"][3],
        json!({"role":"assistant","content":"previous answer"})
    );
}

#[tokio::test]
async fn ollama_nonstream_and_failure_completion_are_distinct() {
    let (base, captured) = serve_response(
        "200 OK",
        "application/json",
        r#"{"message":{"content":"answer"}}"#,
        37,
    )
    .await;
    let provider = OllamaLlm::with_network(&base, "synthetic", network()).unwrap();
    assert_eq!(provider.complete(&[], &options()).await.unwrap(), "answer");
    assert_eq!(captured.await.unwrap().1["stream"], false);
    for tail in ["", "{\"done\":true,\"error\":\"synthetic failure\"}\n"] {
        let body = format!("{{\"message\":{{\"content\":\"partial\"}},\"done\":false}}\n{tail}");
        let (base, _) = serve_response("200 OK", "application/x-ndjson", &body, 43).await;
        let provider = OllamaLlm::with_network(&base, "synthetic", network()).unwrap();
        assert!(provider
            .complete_stream(&[], &options(), &mut |_| {})
            .await
            .is_err());
    }
}

#[tokio::test]
async fn anthropic_preserves_system_precedence_and_streams_text() {
    let body = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n\
        data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
        data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"answer\"}}\n\n\
        data: {\"type\":\"message_stop\"}\n\n";
    let (base, captured) = serve_response("200 OK", "text/event-stream", body, 7).await;
    let provider = AnthropicLlm::with_network(
        &format!("{base}/v1"),
        "synthetic-key",
        "synthetic",
        network(),
    )
    .unwrap();
    let mut text = String::new();
    assert_eq!(
        provider
            .complete_stream(&messages(), &options(), &mut |s| text.push_str(s))
            .await
            .unwrap(),
        "answer"
    );
    assert_eq!(text, "answer");
    let (headers, body) = captured.await.unwrap();
    assert!(headers.starts_with("POST /v1/messages "));
    assert!(headers.contains("anthropic-version: 2023-06-01"));
    assert!(headers.contains("x-api-key: synthetic-key"));
    assert_eq!(
        body,
        json!({
            "model":"synthetic","system":"source system","max_tokens":19,"temperature":0.25,
            "stop_sequences":["STOP"],"stream":true,
            "messages":[{"role":"user","content":"synthetic source [1]"},{"role":"assistant","content":"previous answer"},{"role":"user","content":"question"}]
        })
    );
}

#[tokio::test]
async fn anthropic_nonstream_system_fallback_and_stream_failure() {
    let (base, captured) = serve_response(
        "200 OK",
        "application/json",
        r#"{"content":[{"text":"answer"}]}"#,
        100,
    )
    .await;
    let provider = AnthropicLlm::with_network(&base, "synthetic", "synthetic", network()).unwrap();
    assert_eq!(provider.complete(&[], &options()).await.unwrap(), "answer");
    assert_eq!(captured.await.unwrap().1["system"], "host prompt");
    for suffix in ["", "data: {\"type\":\"error\"}\n\n"] {
        let body = format!("data: {{\"type\":\"content_block_delta\",\"delta\":{{\"type\":\"text_delta\",\"text\":\"partial\"}}}}\n\n{suffix}");
        let (base, _) = serve_response("200 OK", "text/event-stream", &body, 31).await;
        let provider =
            AnthropicLlm::with_network(&base, "synthetic", "synthetic", network()).unwrap();
        assert!(provider
            .complete_stream(&[], &options(), &mut |_| {})
            .await
            .is_err());
    }
}

#[tokio::test]
async fn compatible_embeddings_require_cardinality_indices_and_dimensions() {
    let inputs = vec!["synthetic one".into(), "synthetic two".into()];
    let valid =
        json!({"data":[{"index":1,"embedding":[3.0,4.0]},{"index":0,"embedding":[1.0,2.0]}]});
    let invalid = [
        json!({"data":[]}),
        json!({"data":[{"index":0,"embedding":[1.0,2.0]}]}),
        json!({"data":[{"index":0,"embedding":[1.0,2.0]},{"index":0,"embedding":[3.0,4.0]}]}),
        json!({"data":[{"index":0,"embedding":[1.0,2.0]},{"index":2,"embedding":[3.0,4.0]}]}),
        json!({"data":[{"embedding":[1.0,2.0]},{"embedding":[3.0,4.0]}]}),
        json!({"data":[{"index":0,"embedding":[]},{"index":1,"embedding":[3.0,4.0]}]}),
        json!({"data":[{"index":0,"embedding":[1.0]},{"index":1,"embedding":[3.0,4.0]}]}),
    ];
    for (response, succeeds) in
        std::iter::once((valid, true)).chain(invalid.into_iter().map(|v| (v, false)))
    {
        let (base, captured) =
            serve_response("200 OK", "application/json", &response.to_string(), 23).await;
        let provider = OpenAiCompatibleEmbedder::with_network(
            &format!("{base}/v1"),
            "synthetic",
            2,
            None,
            network(),
        )
        .unwrap();
        let result = provider.embed_documents(&inputs).await;
        if succeeds {
            assert_eq!(result.unwrap(), vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        } else {
            assert!(result.is_err());
        }
        let (headers, body) = captured.await.unwrap();
        assert!(headers.starts_with("POST /v1/embeddings "));
        assert_eq!(body, json!({"model":"synthetic","input":inputs}));
    }
}

#[tokio::test]
async fn ollama_embeddings_validate_shape_and_empty_input_skips_network() {
    let empty =
        OllamaEmbedder::with_network("http://127.0.0.1:1", "synthetic", 2, network()).unwrap();
    assert!(empty.embed(&[]).await.unwrap().is_empty());
    let compatible = OpenAiCompatibleEmbedder::with_network(
        "http://127.0.0.1:1",
        "synthetic",
        2,
        None,
        network(),
    )
    .unwrap();
    assert!(compatible.embed(&[]).await.unwrap().is_empty());
    for (response, succeeds) in [
        (json!({"embeddings":[[1.0,2.0]]}), true),
        (json!({"embeddings":[]}), false),
        (json!({"embeddings":[[]]}), false),
        (json!({"embeddings":[[1.0]]}), false),
        (json!({"embeddings":[[1.0,2.0],[3.0,4.0]]}), false),
    ] {
        let (base, captured) =
            serve_response("200 OK", "application/json", &response.to_string(), 41).await;
        let provider = OllamaEmbedder::with_network(&base, "synthetic", 2, network()).unwrap();
        let result = provider.embed_query("synthetic source").await;
        if succeeds {
            assert_eq!(result.unwrap(), vec![1.0, 2.0]);
        } else {
            assert!(result.is_err());
        }
        let (headers, body) = captured.await.unwrap();
        assert!(headers.starts_with("POST /api/embed "));
        assert_eq!(body["input"], json!(["synthetic source"]));
    }
}

#[tokio::test]
async fn body_frame_event_and_output_limits_are_enforced() {
    let limits = ResponseLimits {
        body_bytes: 128,
        frame_bytes: 128,
        output_bytes: 128,
    };
    let (base, _) = serve_response(
        "200 OK",
        "application/json",
        &format!(
            r#"{{"choices":[{{"message":{{"content":"{}"}}}}]}}"#,
            "x".repeat(200)
        ),
        17,
    )
    .await;
    let provider = OpenAiCompatibleLlm::with_network(
        &base,
        "synthetic",
        None,
        network().with_limits(limits).unwrap(),
    )
    .unwrap();
    assert!(provider
        .complete(&[], &options())
        .await
        .unwrap_err()
        .to_string()
        .contains("body limit"));
    for (body, limits, expected) in [
        (format!("data: {}\n\n", "x".repeat(200)), ResponseLimits { frame_bytes: 32, ..Default::default() }, "frame limit"),
        (format!("{}\n", ": x\n".repeat(40)), ResponseLimits { frame_bytes: 32, ..Default::default() }, "event limit"),
        (format!("{}\n", ":\n\n".repeat(40)), ResponseLimits { body_bytes: 32, ..Default::default() }, "body limit"),
        ("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"too long\"}}]}\n\ndata: [DONE]\n\n".into(), ResponseLimits { output_bytes: 2, ..Default::default() }, "text limit"),
    ] {
        let (base, _) = serve_response("200 OK", "text/event-stream", &body, 8).await;
        let provider = OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network().with_limits(limits).unwrap()).unwrap();
        assert!(provider.complete_stream(&[], &options(), &mut |_| {}).await.unwrap_err().to_string().contains(expected));
    }
}

#[tokio::test]
async fn policy_can_deny_per_request_without_contacting_the_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let config = network().with_policy(Arc::new(move |url: &url::Url| -> Result<()> {
        counter.fetch_add(1, Ordering::Relaxed);
        if url.path() == "/" {
            Ok(())
        } else {
            Err(RuntimeError::Other("Host policy denied request".into()))
        }
    }));
    let provider = OllamaLlm::with_network(&base, "synthetic", config).unwrap();
    assert!(provider.complete(&[], &options()).await.is_err());
    assert!(provider.health_check().await.is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn default_client_does_not_follow_redirects_or_retry_http_errors() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let location = format!("http://{}/other", target.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        socket.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        drop(socket);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let provider = OllamaLlm::new(&base, "synthetic").unwrap();
    assert!(provider
        .complete(&[], &options())
        .await
        .unwrap_err()
        .to_string()
        .contains("307"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), target.accept())
            .await
            .is_err()
    );
    server.await.unwrap();
    let (base, _) = serve_response(
        "429 Too Many Requests",
        "application/json",
        r#"{"error":"synthetic"}"#,
        256,
    )
    .await;
    let provider = OllamaLlm::new(&base, "synthetic").unwrap();
    assert!(provider
        .complete(&[], &options())
        .await
        .unwrap_err()
        .to_string()
        .contains("429"));
}

#[tokio::test]
async fn transport_and_malformed_response_errors_surface_without_success() {
    for response in [
        "not-json",
        r#"{"choices":[]}"#,
        r#"{"choices":[{"message":{"content":""}}]}"#,
    ] {
        let (base, _) = serve_response("200 OK", "application/json", response, 50).await;
        let provider =
            OpenAiCompatibleLlm::with_network(&base, "synthetic", None, network()).unwrap();
        assert!(provider.complete(&[], &options()).await.is_err());
    }
    let provider = OllamaLlm::with_network("http://127.0.0.1:1", "synthetic", network()).unwrap();
    assert!(provider.complete(&[], &options()).await.is_err());
    for invalid in [
        "file:///etc/passwd",
        "invalid-url",
        "http://user:password@127.0.0.1",
        "http://127.0.0.1?api_key=synthetic",
    ] {
        assert!(OllamaLlm::with_network(invalid, "synthetic", network()).is_err());
    }
    assert!(network()
        .with_limits(ResponseLimits {
            body_bytes: 0,
            ..Default::default()
        })
        .is_err());
}
