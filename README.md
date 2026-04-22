# Obi-Wan

**Your code has a story. Obi-Wan remembers it.**

Obi-Wan is an AI-native terminal IDE that builds a persistent knowledge graph of your codebase. When you ask the AI a question, it doesn't dump entire files into the context window — it finds exactly the functions, structs, and relationships that matter, and sends only those.

**~95% fewer tokens per query. Same quality answers. Fraction of the cost.**

```
Traditional approach:  "How does the parser work?"  →  5,000+ tokens (entire files)
Obi-Wan approach:      "How does the parser work?"  →  ~450 tokens (exact subgraph)
```

---

## Why

Every AI coding tool starts from zero. It reads files, guesses what's relevant, and burns through tokens on code you didn't ask about. You pay for context the AI doesn't need. At scale, this means thousands of dollars in wasted API costs per month.

Obi-Wan solves this by understanding your code *structurally* — not just textually.

---

## How It Works

### 1. AST-Level Indexing

Obi-Wan parses your codebase using [Tree-sitter](https://tree-sitter.github.io/) at the function/struct level. Every function, method, struct, trait, and constant becomes a **node**. No line-level guessing — real AST extraction.

**Supported languages:** Rust, Python, TypeScript, JavaScript, Go

### 2. Knowledge Graph Construction

Nodes are connected by weighted edges that encode real relationships:

| Relationship | How It's Detected | Weight |
|---|---|---|
| **Function calls** | Tree-sitter call resolution | 0.8 |
| **Imports** | `use` / `import` statement parsing | 0.7 |
| **Type references** | Signature analysis (params, returns) | 0.7 |
| **Semantic similarity** | Cosine similarity between embeddings (>0.85 threshold) | 0.77-0.9 |
| **User-defined links** | `[[function_name]]` in markdown notes | 1.0 |

### 3. Smart Context Retrieval

When you ask a question, a 4-step pipeline builds the optimal context:

```
Query → Embed → Vector Search (anchors) → Graph Expansion (BFS) → Token Packing → LLM
```

1. **Anchor Selection** — Vector search finds the top-k most relevant nodes
2. **Graph Expansion** — BFS traversal pulls in callers, callees, and related types (depth 2, with decay weighting)
3. **Greedy Token Packing** — Nodes are ranked by score and packed into the token budget
4. **Context Assembly** — Nodes are formatted with relationship hints (`calls:`, `called_by:`, `imports:`)

The LLM receives a precise subgraph instead of raw files.

### 4. Dual-Brain Architecture

Two independent knowledge graphs work together:

- **Project Brain** (`.obi/`) — Your current project, boosted 1.5x in search results
- **Global Brain** (`~/.obi/`) — Shared patterns across all your projects

Project-specific code always takes priority. Global patterns fill in the gaps.

---

## Features

### Token Cost Optimization
The core value proposition. Every query goes through the knowledge graph instead of naive file inclusion. Typical savings: **90-95%** of tokens per interaction. This directly translates to lower API bills and faster responses.

### Incremental Indexing
Files are tracked by content hash (BLAKE3). On re-index, only modified nodes are re-parsed and re-embedded. No wasted compute on unchanged code.

### Pluggable LLM Providers
Mix and match providers for different tasks:

| Provider | Completion | Embedding | Notes |
|---|---|---|---|
| **Anthropic Claude** | claude-sonnet-4-6 (200k context) | - | Tool use support, streaming |
| **Ollama** | gemma3, llama3, etc. | nomic-embed-text (768d) | Free, local, no API key |
| **Z.ai (Zhipu)** | GLM-4 | - | Tool use support |

Use Ollama for free local embeddings + Claude for completions. The **LLM Router** handles automatic fallback on rate limits (429) and server errors (5xx).

### Interactive Knowledge Graph
An Obsidian-style graph visualization right in your terminal. See what the AI sees. Understand how your code connects.

- Pin nodes to force-include them in context
- Exclude nodes to remove noise
- Write markdown notes with `[[function_name]]` wiki-links that connect directly to code

### Graph-Only Mode
No embedding provider? No problem. Obi-Wan falls back to keyword matching on node names with shallow graph expansion. You still get structured context — just without vector search.

### Tool Use & Agent Loop
The built-in agent supports structured tool calls (edit, search, run). Tool executions require user confirmation before running. Responses are streamed in real-time.

### Terminal-Native
Built with [Ratatui](https://ratatui.rs/). No Electron. No browser. No VS Code dependency. Just your terminal.

---

## Architecture

```
obi-wan/
├── obi-core       Data structures: SemanticNode, Edge, KnowledgeGraph, Config
├── obi-indexer     Tree-sitter parsing, LanceDB vector storage, embedding pipeline
├── obi-llm         Provider abstraction: Anthropic, Ollama, Z.ai + Router with fallback
├── obi-agent       Dual-brain, context builder (4-step pipeline), agent orchestration
└── obi-tui         Terminal UI, graph visualization, chat interface
```

### Tech Stack

| Component | Technology |
|---|---|
| Language | Rust |
| Parsing | Tree-sitter |
| Vector Database | LanceDB (Apache Arrow) |
| Graph Storage | Bincode serialization |
| Content Hashing | BLAKE3 |
| TUI | Ratatui + Crossterm |
| Async Runtime | Tokio |
| HTTP | Reqwest |

---

## Quick Start

### Prerequisites

- Rust 1.75+
- [Ollama](https://ollama.ai) (optional, for free local LLM/embeddings)

### Build & Run

```bash
# Build
cargo build --release

# (Optional) Start Ollama and pull models
ollama serve
ollama pull nomic-embed-text    # embedding model (768d)
ollama pull gemma3               # completion model

# Index your project
./target/release/obi index

# Launch the TUI
./target/release/obi
```

### Commands

```bash
obi              # Launch terminal IDE
obi index        # Index/re-index the project
obi status       # Show index stats (nodes, edges, files)
obi brain        # Show dual-brain status
```

### Configuration

Global config at `~/.obi/config.toml`, project override at `.obi/config.toml`:

```toml
[sources]
code = ["src/", "lib/"]
notes = [".obi/notes/"]
exclude = ["target/", "node_modules/", ".git/"]

[providers]
completion = "anthropic"     # or "ollama", "zai"
embedding = "ollama"         # decoupled — mix providers freely

[providers.anthropic]
api_key = "sk-ant-..."

[providers.ollama]
host = "http://localhost:11434"
embedding.model = "nomic-embed-text"
embedding.dimensions = 768

[brain]
project_boost = 1.5          # favor project-specific results
max_node_tokens = 500        # truncate large functions
embedding_enabled = true     # false = graph-only mode
```

---

## How Token Savings Work — Concrete Example

**Query:** *"How does tokenization work?"*

```
Step 1: Vector Search
  → Finds: tokenize() [score: 0.92], Token struct [0.88], Lexer [0.85]

Step 2: Graph Expansion (BFS, depth 2)
  → Depth 0: tokenize()                    score: 0.92
  → Depth 1: parse() calls tokenize()      score: 0.92 × 0.8 × 0.6 = 0.44
  → Depth 1: Token struct (type ref)       score: 0.88 × 0.7 × 0.6 = 0.37
  → Depth 2: main() calls parse()          score: 0.44 × 0.8 × 0.3 = 0.11

Step 3: Token Packing (budget: 4096)
  → tokenize: 200 tokens  ✓
  → Token:    100 tokens  ✓
  → parse:    150 tokens  ✓
  → Lexer:     80 tokens  ✓
  → main:      30 tokens  ✓
  → Total:    560 tokens (vs 5,000+ naive)

Savings: ~89%
```

The LLM receives structured context with relationship hints:

```rust
// src/lexer.rs:10-45 | struct Token | called by: tokenize
pub struct Token { kind: TokenKind, span: Span }

// src/lexer.rs:50-120 | fn tokenize | calls: Token, Lexer | called by: parse
pub fn tokenize(input: &str) -> Vec<Token> { ... }

// src/parser.rs:1-50 | fn parse | calls: tokenize | called by: main
pub fn parse(tokens: Vec<Token>) -> Ast { ... }
```

---

## License

MIT
