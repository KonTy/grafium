use super::worker::*;

#[test]
fn host_configuration_does_not_infer_paths_or_claim_a_loaded_model() {
    let config = NativeHostConfig::new("synthetic-host".into(), "--synthetic-worker");
    assert_eq!(
        config.executable,
        std::path::PathBuf::from("synthetic-host")
    );
    assert_eq!(config.worker_argument, "--synthetic-worker");
    assert!(config.recovery_state_dir.is_none());
    assert!(config.inference_threads.is_none());
    assert!(config.on_diagnostic.is_none());
    assert!(!config.force_cpu);
    assert!(resident_status().is_none());
}

#[test]
fn reported_residency_is_serializable_without_native_handles() {
    let resident = ResidentStatus {
        worker_pid: 42,
        model: ResidentModel {
            kind: NativeModelKind::Embeddings,
            model_path: "synthetic.gguf".into(),
            context_size: Some(512),
            on_gpu: false,
            gpu_layers: Some(0),
        },
    };
    let decoded: ResidentStatus =
        serde_json::from_value(serde_json::to_value(&resident).unwrap()).unwrap();
    assert_eq!(decoded.worker_pid, 42);
    assert_eq!(decoded.model.kind, NativeModelKind::Embeddings);
    assert!(!decoded.model.on_gpu);
}

#[cfg(feature = "llm-local")]
#[test]
fn configured_embedding_request_preserves_explicit_cpu_and_context_choices() {
    let request = WorkerRequest::EmbedConfigured {
        model_path: "synthetic.gguf".into(),
        context_size: 1024,
        gpu_layers: 0,
        texts: vec!["synthetic input".into()],
    };
    let decoded: WorkerRequest =
        serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
    assert!(matches!(
        decoded,
        WorkerRequest::EmbedConfigured {
            context_size: 1024,
            gpu_layers: 0,
            ..
        }
    ));
}
