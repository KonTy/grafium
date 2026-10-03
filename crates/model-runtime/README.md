# Model runtime

Internal Rust workspace crate for reusable model settings and lifecycle management.
Grafium is the first consumer. This is not an independently released package or an action agent.
It follows the workspace version and **AGPL-3.0-only** license; extracting it does
not relicense existing code. Decide licensing before incorporating it into a
differently licensed application.

## Boundaries

- `settings`: versioned configuration, generated JSON Schema, structured
  validation, capability filtering, and a controller with host-injected storage.
- `manager`: shared provider preparation, reconfiguration, recovery, native
  eviction and managed-file operations. Prepared does not mean resident or healthy.
- `models`: discovery, import, integrity-checked downloads and reversible removal
  of library-owned files. User originals and unmanaged models are not deleted.
- `types` and `providers`: model messages, generation/embedding interfaces, and
  HTTP transports for local servers and hosted providers.
- `protocol` and `supervisor`: bounded subprocess IPC, request ordering,
  cancellation, shutdown, idle/model-switch eviction, and crash recovery.
- `resources`: conservative RAM/GPU admission and thread/context/input limits.
- `gguf`: bounded pure-Rust model metadata inspection, without initializing
  native libraries in the host process.
- `gpu`: device-bound budget checks and matching PCI memory counters.
- `recovery`: durable worker-lifetime leases and one bounded automatic GPU
  recovery attempt per model identity in a host-provided private state directory.

The host owns secret storage, settings persistence, endpoint/data-access policy,
conversation persistence, tool authorization, and domain transactions. There is
no SQLite, graph, Tauri, or UI dependency. Optional native features contain the
llama.cpp/Whisper implementations and their worker dispatch; Grafium only adapts
its legacy settings/errors and bootstraps the worker before starting its UI.

No shared settings dialog is required. A frontend can render `SettingsPolicy::schema()`
and submit values validated by the same backend policy. Credential fields contain
references, not secret values; a host `CredentialProvider` resolves them when
providers are prepared. A `SettingsStore` can use an encrypted vault, a host
database, or another atomic store without adding that dependency here.

`ModelManager::configure_persisted` prepares providers and saves through the
settings controller before replacing its live configuration. Validation/storage
failures retain the previous settings. Missing model files or credentials appear
as explicit per-role issues; they never cause a silent switch to another provider.

## Schema and host integration

Export the schema for a network-only build:

```bash
cargo run -p model-runtime --example settings_schema
```

Enable `llm-local` / `media` (or their `-vulkan` variants) when the host includes
native inference. Offered schema variants reflect compiled and host-allowed
capabilities. Applications still need a small early-start worker bootstrap and
their own UI; they do not need separate model loading/eviction implementations.

The native supervisor is shared by all managers within one process. Configure it
once with `native::NativeHostConfig` and `native::configure_with_options`; dispatch
the private worker invocation to `native::worker::run_from_stdio` before UI
startup. Supply the executable, worker argument, private recovery directory and
diagnostic callback explicitly. Providers prepare lazily; actual model loading
and model-switch eviction happen inside the supervised worker.
`ModelManager::status()` distinguishes prepared bindings from confirmed resident
weights. `unload_idle()` is reversible and reports busy native work.
`shutdown_native()` permanently closes this process's native pool, cancels queued
and active native requests, and returns incomplete-cleanup errors. Call it during
application exit, not when closing a settings dialog or dropping one manager.
Network services and bindings are not stopped by native shutdown.

Grafium exposes the shared JSON Schema through `ai_model_settings_schema` and
secret-reference-only settings through `ai_runtime_settings`. Its current Svelte
dialog remains app-specific, and its existing preferences are translated rather
than discarded. Knowledge indexing and reference-generation settings stay in
Grafium, not the generic model schema.

Managed removal requires explicit intent, refuses configured/in-use or unmanaged
models, and quarantines bytes instead of permanently deleting them. Downloads
are explicit, bounded and checksum-verified before publication. These operations
never stop or delete models owned by an independent network inference server.
Loaded local models retain shared locks on the actual file; cooperating apps
must acquire an exclusive lock before mutation. This prevents managed removal
while another runtime worker is using the file, including CPU-only inference.

The runtime supervises only processes it starts. It must not unload or terminate
independently managed Ollama, vLLM, or other network inference services. A shared
library does not share model memory between applications; use a shared inference
server when that is needed.

## Safety boundaries

Subprocesses isolate native application faults, not kernel or GPU-driver faults.
Resource estimates and preflight checks cannot guarantee against an OS freeze or
an allocation racing another application's load.

GPU admission includes weights, context/scratch estimates, and desktop headroom.
Unknown or insufficient headroom selects CPU, and insufficient RAM rejects the
request. The shared native runtime binds to a specific backend device, uses native
model fitting with an unchanged context, and monitors pressure during execution.
An uncorroborated total-as-free heap report is not accepted as a measured budget.
The caller must surface fallback reasons rather than claiming GPU acceleration.
Grafium preserves its `GRAFIUM_DISABLE_GPU_OFFLOAD` and `GRAFIUM_LLM_THREADS`
environment options in the host adapter, not in this library.

Supervision can attach per-spawn OS RAM limits and a recovery lease. Linux uses
already-delegated cgroup v2 memory controls without changing ancestor limits;
Windows uses Job Object limits. Unsupported enforcement is reported explicitly.
The host's recovery lease is confirmed only after process exit is observed, not
after a single successful completion while the model remains resident.

After an abnormal or unconfirmed GPU worker exit, the next actual GPU model use
gets one automatic recovery attempt, still subject to the same RAM, VRAM,
pressure, explicit CPU selection, and active-worker locks. Status/health reads
never start inference or consume this credit. Local chat health checks inspect
locked GGUF metadata only; they do not claim tensor validity or residency.
Native validation and allocation checks still run on actual use.
The credit is atomically persisted
before launch and remains spent across concurrent hosts, restarts, and clean
exits. Failed recovery remembers CPU-only fallback; `allow_once` / `retry_gpu`
explicitly permit one additional attempt without restoring automatic credit.
`RecoveryStore::use_cpu(key)` can decline an unused attempt and durably remember
CPU mode for a known recovery record without loading or stopping any worker.
It is idempotent, revokes unused manual permission, and refuses unknown, healthy,
active-worker, or damaged-state requests. Only an explicit one-shot retry leaves
the selected CPU state. Grafium exposes this as `ai_use_cpu_for_model({ key })`,
returning success without a payload; clients then refresh recovery status.
Changing model identity gives that identity its own budget; returning to a known
identity does not reset its budget.

`BlockedModel.state` distinguishes `retry_pending`, `retrying`, and `cpu_only`.
The store/native recovery list is saved history across model identities.
`ModelManager::filter_current_recovery` narrows it to the currently selected
native role/file identities, without re-running automatic model selection.
Grafium's Chat/index health uses this filtered list; switching models, switching
to a network provider, disabling AI, or having no engine does not present old
model recovery as current. The saved history itself is retained.
Fresh active workers and successfully recovered, cleanly exited workers are not
reported as blocked. An active recovery is not an approval request. Legacy v1
journals are read without deletion and converted to v2 on the next durable write:
unauthorized failures get the new single automatic credit, while authorized or
in-flight legacy retries do not acquire another credit. Damaged, incomplete, or
unsupported journals still fail closed and are never repaired by retrying.

Cached CPU workers denied by recovery are re-admitted on a subsequent request
when an attempt becomes eligible; provider preparation does not pin a transient
recovery decision into its GPU settings. Resource-driven CPU fallback within an
admitted worker still remains subject to native safety checks. A failed request
is returned to the host without replay, including after partial progress/output.
The host must retain the draft and surface the failure; a later request attempts
bounded recovery or CPU fallback as appropriate, rather than duplicating output.

Peculium's local-only inference and encrypted-storage requirements are host
policies, not implications of an OpenAI-compatible transport name. Importing
this crate must not implicitly enable cloud access or persist prompts.

## Development

Run `cargo test -p model-runtime` from the workspace root. Lifecycle tests use
synthetic subprocesses, not real models or GPU exhaustion. The default,
network-only build needs no C++ model engine. Native feature builds compile
llama.cpp/Whisper and exercise their adapters. Do not test OOM recovery by
exhausting the user's RAM or VRAM.

To check a real GPU deliberately, `cargo run -p model-runtime --release
--features llm-local-vulkan,media-vulkan --example gpu_smoke -- /path/to/model.gguf`
uses synthetic text, a temporary recovery journal, the normal worker admission
checks, and two streamed 64-token requests with a 60-second cancellation deadline each.
It does not import the model or open graphs. It reports cold/warm time to first
streamed text and total time, requires the streamed pieces to reassemble the final
answer, and requires confirmed GPU residency rather than treating requested offload as
proof. Ordinary tests cover the fitter's `-1` all-layers sentinel, partial
offload, and CPU-only results without loading models.

Native generation streams text through ordered worker `Text` events when the
caller uses `complete_stream`. Stop sequences are withheld until they can no
longer match, so streamed text never contains one; streamed text is provisional
until the request succeeds.
