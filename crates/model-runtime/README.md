# Model runtime

Internal Rust workspace crate for reusable model infrastructure. Grafium is the
first consumer. This is not an independently released package or an action agent.
It follows the workspace version and **AGPL-3.0-only** license; extracting it does
not relicense existing code. Decide licensing before incorporating it into a
differently licensed application.

## Boundaries

- `types` and `providers`: model messages, generation/embedding interfaces, and
  HTTP transports for local servers and hosted providers.
- `protocol` and `supervisor`: bounded subprocess IPC, request ordering,
  cancellation, shutdown, idle/model-switch eviction, and crash recovery.
- `resources`: conservative RAM/GPU admission and thread/context/input limits.
- `gguf`: bounded pure-Rust embedding metadata inspection, without initializing
  native libraries in the host process.
- `gpu`: device-bound budget checks and matching PCI memory counters.
- `recovery`: durable worker-lifetime leases, stale-exit quarantine, and explicit
  one-shot retry authorization in a host-provided private state directory.

The host owns credentials, provider selection, endpoint/data-access policy,
conversation persistence, tool authorization, and domain transactions. There is
no SQLite, graph, Tauri, or UI dependency. Native llama.cpp/Whisper handlers and
model discovery remain Grafium adapters; the supervisor can run another host's
native implementation without linking those engines into this crate.

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
request. Grafium's native adapter binds to a specific backend device, uses native
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

Peculium's local-only inference and encrypted-storage requirements are host
policies, not implications of an OpenAI-compatible transport name. Importing
this crate must not implicitly enable cloud access or persist prompts.

## Development

Run `cargo test -p model-runtime` from the workspace root. Lifecycle tests use
synthetic subprocesses, not real models or GPU exhaustion. Grafium's native
feature builds also exercise the adapters; the library itself needs no C++
model engine. Do not test OOM recovery by exhausting the user's RAM or VRAM.
