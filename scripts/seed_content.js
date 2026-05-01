// This script generates 100 days of rich journal content.
// It's designed to be run inside the Tauri app via the invoke API.
// Usage: Run the app, open devtools console, paste this script.

const CONTENT_BLOCKS = [
  // Mermaid
  '```mermaid\ngraph TD\n    A[Start] --> B{Decision}\n    B -->|Yes| C[Action]\n    B -->|No| D[Skip]\n    C --> E[End]\n    D --> E\n```',
  '```mermaid\nsequenceDiagram\n    Client->>Server: Request\n    Server->>DB: Query\n    DB-->>Server: Result\n    Server-->>Client: Response\n```',
  '```mermaid\npie title Weekly Time\n    "Code" : 40\n    "Review" : 20\n    "Meetings" : 15\n    "Learning" : 25\n```',
  // Math
  'Quadratic formula: $x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$',
  'Euler identity: $e^{i\\pi} + 1 = 0$',
  'Gaussian integral: $\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}$',
  'Fourier transform: $\\hat{f}(\\xi) = \\int_{-\\infty}^{\\infty} f(x) e^{-2\\pi i x \\xi} dx$',
  'Bayes theorem: $P(A|B) = \\frac{P(B|A) P(A)}{P(B)}$',
  // Code
  '```rust\nfn main() {\n    let fib: Vec<u64> = (0..20).scan((0u64, 1u64), |state, _| {\n        let next = state.0 + state.1;\n        *state = (state.1, next);\n        Some(state.0)\n    }).collect();\n    println!("{:?}", fib);\n}\n```',
  '```python\nimport asyncio\n\nasync def fetch_all(urls):\n    async with aiohttp.ClientSession() as session:\n        tasks = [session.get(url) for url in urls]\n        return await asyncio.gather(*tasks)\n```',
  '```typescript\ntype DeepPartial<T> = {\n  [P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];\n};\n\nfunction merge<T>(target: T, source: DeepPartial<T>): T {\n  return { ...target, ...source };\n}\n```',
  '```haskell\nfmap :: Functor f => (a -> b) -> f a -> f b\nfmap f (Just x) = Just (f x)\nfmap _ Nothing  = Nothing\n```',
  // Notes
  'TODO Research [[distributed consensus]] - Raft vs Paxos comparison',
  'DONE Setup monitoring with [[Prometheus]] and [[Grafana]]',
  'Key insight: [[category theory]] morphisms map directly to type transformations',
  'Meeting: discussed [[microservices]] migration strategy. Target Q3.',
  'Bug: race condition in connection pool. Fix: add mutex around checkout.',
  'Performance: p99 latency down from 200ms to 8ms after adding [[Redis]] cache',
  'Read "Designing Data-Intensive Applications" Ch.7 on [[transactions]]',
  'Idea: use [[WebAssembly]] for compute-heavy client-side operations',
  '| Metric | Before | After | Improvement |\n|--------|--------|-------|-------------|\n| Latency p50 | 45ms | 12ms | 73% |\n| Latency p99 | 200ms | 35ms | 82% |\n| Throughput | 1.2k/s | 8.5k/s | 608% |',
  '> "Simplicity is prerequisite for reliability." — Edsger Dijkstra',
  '#project/atlas Sprint goals:\n- [ ] Implement SSO with [[SAML]]\n- [x] Database sharding migration\n- [ ] Load testing at 10x traffic\n- [ ] Write runbook for failover',
];

async function generateContent() {
  const { invoke } = window.__TAURI_INTERNALS__;
  let created = 0;
  let errors = 0;
  
  for (let day = 0; day < 100; day++) {
    const date = new Date();
    date.setDate(date.getDate() - day);
    const title = date.toISOString().split('T')[0];
    
    try {
      const page = await invoke("create_page", { title, isJournal: true });
      const numBlocks = 3 + Math.floor(Math.random() * 5);
      
      for (let i = 0; i < numBlocks; i++) {
        const content = CONTENT_BLOCKS[Math.floor(Math.random() * CONTENT_BLOCKS.length)];
        await invoke("create_block", { 
          pageId: page.id, 
          parentId: null, 
          orderIndex: i, 
          content 
        });
      }
      created++;
    } catch(e) {
      errors++;
    }
  }
  
  console.log(`Generated ${created} days, ${errors} errors (likely existing pages)`);
  return { created, errors };
}

generateContent();
