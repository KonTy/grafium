# AI Setup And Privacy

AI is optional. Grafium's editor, journal, links, graph, tasks, flashcards,
search, imports, and sync work without an AI provider.

## Set up AI

1. Open **Settings → AI / Knowledge Engine**.
2. Choose how the model runs:
   - **Embedded local model:** select compatible GGUF model files. Inference
     stays on this computer after the model is available, but model downloads
     still use the network.
   - **Ollama or another local server:** start the server, select its endpoint,
     and choose an exposed model.
   - **OpenAI-compatible endpoint:** enter an endpoint and credentials for a
     service you control or trust.
   - **Cloud provider:** configure the provider and API key, then select a
     generation model.
3. Test the connection before asking Chat to use it.
4. Configure **generation** and **embedding** models separately when available.
   Chat can work with keyword retrieval before embeddings are ready.
5. Index only graph content you intend to make available to AI.

Model settings are validated by Grafium's shared model manager. The current
settings dialog stays the same; existing model paths and provider choices are
retained. A valid configuration can still be unavailable (for example, a model
file is missing or a server is offline); Grafium reports that rather than
silently selecting a different provider. If preparing or saving new settings
fails, your previous configuration stays active.

The shared library also supplies a settings schema for other frontends. Apps
choose their own storage, credentials and permitted providers; a common schema
does not mean their privacy policies are interchangeable. Managed model-file
removal is explicit and reversible; borrowed external model files are not deleted.

Local model setup may require substantial disk space, RAM, and GPU support.
Vulkan acceleration is optional but can make local inference much faster.
Grafium runs embedded models in a supervised worker. Closing the app stops
queued and active native work, requests shutdown, and terminates an unresponsive
worker after a grace period. Idle models and models replaced by another workload
are evicted rather than kept resident indefinitely.
Changing provider configuration releases an idle native model immediately.
An active transcription or other native job is left to finish; it is not
interrupted just because you changed the Chat provider.

Chat, embeddings, and transcription check memory before loading. Native device
queries run inside the supervised worker, not the editor process. Grafium selects
a specific dedicated GPU with a measured backend budget, including systems with
multiple GPUs, and binds the model to that device. A reported total heap is not
treated as free memory without a measured budget or matching vendor counters.
Integrated/shared-memory GPUs and unknown budgets conservatively use CPU.

For GGUF models, native fitting accounts for model tensors, KV cache, and compute
buffers. It may reduce GPU offload, but does not silently shorten the requested
context. Smaller decoding batches reduce peak memory. Host RAM estimates also use
available architecture metadata, with a conservative fallback for other models.
If GPU headroom is insufficient or cannot be measured, Grafium uses CPU and
displays a runtime warning, provided RAM is sufficient.

Active requests monitor RAM and the selected GPU's remaining headroom. On systems
with delegated Linux cgroup v2 memory controls, native workers receive their own
RAM/swap limits; supported Windows builds use Job Object memory limits. If hard
containment is unavailable, the app says so instead of claiming it is enforced.
Admission and pressure monitoring still apply. Hard RAM limits do not cap VRAM
or isolate the GPU driver from the OS.

## Recovery after an unconfirmed exit

Before a GPU attempt, Grafium durably records a recovery lease in its application
state, outside your graphs. It clears that lease only after observing the native
worker exit. A crash, reboot, timeout, or unconfirmed exit can therefore disable
automatic GPU attempts for that model on the next run. This is conservative:
an unconfirmed exit does not prove that the GPU caused it.

Open **Settings → AI / Knowledge Engine → Native model recovery** and select
**Allow one GPU attempt** for the affected model. The permission is consumed once;
another abnormal exit blocks GPU again. **Request GPU for Chat** requests GPU
offload for the selected chat model. Neither action bypasses memory checks.
Do not delete recovery files to bypass a damaged journal; use CPU or a network
server and inspect the reported error instead.

Process isolation contains native application crashes, **not GPU driver or
kernel failures**. Memory checks are conservative estimates, not a guarantee
against an OS freeze. On an unstable driver, use CPU or an inference server on
another machine. Grafium never shuts down an independently managed Ollama or
OpenAI-compatible server when it closes.

## Choose context deliberately

- **Local graph** searches your notes; it does not mean a cloud provider will
  not receive the selected prompt and retrieved excerpts.
- **Internet** enables web search and Research. It is separate from where the
  model runs.
- Page, block, journal, and graph scopes limit retrieval; they do not override
  the provider's handling after data is sent.

Never paste passwords, private keys, health records, or confidential material
into a cloud prompt unless you have decided that provider is appropriate.
Review citations and generated edits before saving them.

## Troubleshooting

- No answer: check the provider, model, endpoint, and API key in Settings.
- Local model is slow: check model size, memory, and GPU/Vulkan support.
- CPU fallback: read the runtime warning beside **Model & index status** in
  Chat; choose a smaller model or an inference server if necessary.
- Repeated native crashes: automatic retries stop. Choose a smaller model or
  CPU/network inference; use an explicit one-shot GPU retry only when ready.
- No semantic results: configure an embedding model and run the graph index.
- Web research unavailable: switch source scope to **Internet**.

See [[Chat And Research]] for examples and [[Your Files]] for what stays on disk.
