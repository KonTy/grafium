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

Local model setup may require substantial disk space, RAM, and GPU support.
Vulkan acceleration is optional but can make local inference much faster.

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
- No semantic results: configure an embedding model and run the graph index.
- Web research unavailable: switch source scope to **Internet**.

See [[Chat And Research]] for examples and [[Your Files]] for what stays on disk.
