#!/usr/bin/env -S npx tsx
/**
 * Generate 100 days of rich journal content for testing.
 * Run from the project root: npx tsx scripts/generate_test_content.ts
 * 
 * Requires the app's graph DB at /home/blin/Pictures/tes/tes
 */

import { invoke } from "@tauri-apps/api/core";

// We can't use invoke outside the app, so this generates markdown files
// that can be imported, OR we can run this inside the app via eval.

// Instead, let's generate a JS script that can be eval'd inside the running app.
const CONTENT_TEMPLATES = [
  // Mermaid diagrams
  `\`\`\`mermaid
graph TD
    A[Start] --> B{Decision}
    B -->|Yes| C[Do Something]
    B -->|No| D[Do Something Else]
    C --> E[End]
    D --> E
\`\`\``,
  
  `\`\`\`mermaid
sequenceDiagram
    participant A as Client
    participant B as Server
    participant C as Database
    A->>B: HTTP Request
    B->>C: SQL Query
    C-->>B: Results
    B-->>A: JSON Response
\`\`\``,

  `\`\`\`mermaid
pie title Time Allocation
    "Coding" : 45
    "Meetings" : 20
    "Review" : 15
    "Learning" : 20
\`\`\``,

  // Math formulas
  `The quadratic formula: $x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$`,
  
  `Euler's identity: $e^{i\\pi} + 1 = 0$`,
  
  `Integration by parts: $\\int u \\, dv = uv - \\int v \\, du$`,
  
  `Maxwell's equations in differential form:
$$\\nabla \\cdot \\mathbf{E} = \\frac{\\rho}{\\epsilon_0}$$
$$\\nabla \\cdot \\mathbf{B} = 0$$
$$\\nabla \\times \\mathbf{E} = -\\frac{\\partial \\mathbf{B}}{\\partial t}$$`,

  `The Schrödinger equation: $i\\hbar \\frac{\\partial}{\\partial t}|\\psi\\rangle = \\hat{H}|\\psi\\rangle$`,

  // Code blocks
  `\`\`\`rust
fn fibonacci(n: u64) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}
\`\`\``,

  `\`\`\`python
import numpy as np
from scipy.optimize import minimize

def rosenbrock(x):
    return sum(100.0*(x[1:]-x[:-1]**2.0)**2.0 + (1-x[:-1])**2.0)

x0 = np.array([1.3, 0.7, 0.8, 1.9, 1.2])
result = minimize(rosenbrock, x0, method='nelder-mead')
print(f"Minimum at: {result.x}")
\`\`\``,

  `\`\`\`typescript
interface Observer<T> {
  next(value: T): void;
  error(err: Error): void;
  complete(): void;
}

class Observable<T> {
  constructor(private subscribe: (observer: Observer<T>) => () => void) {}
  
  pipe<R>(...operators: Array<(source: Observable<any>) => Observable<any>>): Observable<R> {
    return operators.reduce((prev, op) => op(prev), this as any);
  }
}
\`\`\``,

  `\`\`\`haskell
quicksort :: (Ord a) => [a] -> [a]
quicksort [] = []
quicksort (x:xs) =
  let smaller = quicksort [a | a <- xs, a <= x]
      bigger  = quicksort [a | a <- xs, a > x]
  in smaller ++ [x] ++ bigger
\`\`\``,

  // Regular notes
  `TODO Research distributed consensus algorithms - [[Raft]] vs [[Paxos]] vs [[PBFT]]`,
  `DONE Set up CI/CD pipeline with GitHub Actions`,
  `Reading notes on "Designing Data-Intensive Applications" by Martin Kleppmann`,
  `Met with team to discuss [[Architecture Decision Records]] for the new microservice`,
  `Interesting talk on [[WebAssembly]] at the meetup - WASI is maturing fast`,
  `Bug fix: race condition in the event loop causing dropped messages under load`,
  `Performance optimization: reduced p99 latency from 450ms to 12ms by adding connection pooling`,
  `Idea: Build a [[knowledge graph]] visualization using D3.js force-directed layout`,
  `TIL: PostgreSQL's EXPLAIN ANALYZE shows actual vs estimated row counts`,
  `Weekly retro: team velocity up 20%, but technical debt backlog growing`,

  // Tables
  `| Algorithm | Time Complexity | Space | Stable |
|-----------|----------------|-------|--------|
| QuickSort | O(n log n) avg | O(log n) | No |
| MergeSort | O(n log n) | O(n) | Yes |
| HeapSort | O(n log n) | O(1) | No |
| TimSort | O(n log n) | O(n) | Yes |`,

  // Links and tags
  `#project/alpha Sprint planning: 
- [ ] Implement user authentication with [[OAuth2]]
- [ ] Set up [[Redis]] caching layer  
- [x] Database schema migration
- [ ] Write integration tests for the API gateway`,

  `Reflections on [[functional programming]]:
> "OOP makes code understandable by encapsulating moving parts. FP makes code understandable by minimizing moving parts." — Michael Feathers`,
];

const TOPICS = [
  "machine learning", "distributed systems", "compiler design", "quantum computing",
  "category theory", "graph algorithms", "cryptography", "operating systems",
  "database internals", "network protocols", "type theory", "formal verification",
  "signal processing", "control theory", "information theory", "game theory",
  "linear algebra", "differential equations", "topology", "abstract algebra",
];

function randomChoice<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

function generateDayContent(dayOffset: number): string[] {
  const blocks: string[] = [];
  const numBlocks = 3 + Math.floor(Math.random() * 5); // 3-7 blocks per day
  
  for (let i = 0; i < numBlocks; i++) {
    if (Math.random() < 0.3) {
      // Use a template
      blocks.push(randomChoice(CONTENT_TEMPLATES));
    } else if (Math.random() < 0.5) {
      // Generate a note about a topic
      const topic = randomChoice(TOPICS);
      const notes = [
        `Deep dive into [[${topic}]] today. Key insight: the relationship between ${randomChoice(TOPICS)} and ${topic} is more fundamental than I realized.`,
        `Reading paper on ${topic}. The authors propose a novel approach using ${randomChoice(TOPICS)} principles.`,
        `TODO Implement a proof-of-concept for ${topic} using the framework from last week's [[${randomChoice(TOPICS)}]] exploration.`,
        `Meeting notes: discussed ${topic} with the team. Action items: research ${randomChoice(TOPICS)} integration.`,
        `Breakthrough! The ${topic} problem reduces to ${randomChoice(TOPICS)} when you apply the right transformation.`,
      ];
      blocks.push(randomChoice(notes));
    } else {
      // Use a random template
      blocks.push(randomChoice(CONTENT_TEMPLATES));
    }
  }
  
  return blocks;
}

// Generate the eval script
function generateEvalScript(): string {
  const lines: string[] = [];
  lines.push(`(async function() {`);
  lines.push(`  const { invoke } = window.__TAURI_INTERNALS__;`);
  lines.push(`  let created = 0;`);
  
  for (let day = 0; day < 100; day++) {
    const date = new Date();
    date.setDate(date.getDate() - day);
    const title = date.toISOString().split('T')[0]; // YYYY-MM-DD
    const blocks = generateDayContent(day);
    
    lines.push(`  try {`);
    lines.push(`    const page = await invoke("create_page", { title: "${title}", isJournal: true });`);
    
    for (let i = 0; i < blocks.length; i++) {
      const content = JSON.stringify(blocks[i]);
      lines.push(`    await invoke("create_block", { pageId: page.id, parentId: null, orderIndex: ${i}, content: ${content} });`);
    }
    
    lines.push(`    created++;`);
    lines.push(`  } catch(e) { /* page may exist */ }`);
  }
  
  lines.push(`  document.title = "Created " + created + " days of content";`);
  lines.push(`  return created;`);
  lines.push(`})();`);
  
  return lines.join('\n');
}

// Output the script to stdout
console.log(generateEvalScript());
