# Obi-Wan

Your code has a story. Obi-Wan remembers it.

Obi-Wan is a terminal-based IDE that builds a persistent knowledge graph of your codebase. When you ask the AI a question, it doesn't dump entire files into the context window — it finds exactly the functions, structs, and notes that matter, and sends only those. The result is faster answers, lower cost, and an AI that actually understands how your code fits together.

## The Problem

Every time you start a conversation with an AI coding tool, it starts from zero. It reads files, guesses what's relevant, and burns through tokens on code you didn't ask about. You pay for context the AI doesn't need.

## What Obi-Wan Does Differently

Obi-Wan indexes your codebase at the function level using tree-sitter. Every function, struct, and type becomes a node in a knowledge graph. Relationships — who calls what, who imports what, what's semantically similar — become edges. When you ask a question, vector search finds the relevant nodes, graph traversal pulls in their connections, and only that subgraph goes to the LLM.

**~95% fewer tokens per query.** Not because we cut corners, but because we send the right context.

On top of that, you get an Obsidian-style graph view right in your terminal. You can see what the AI sees, pin nodes to force-include them, and write markdown notes that link directly to code via `[[function_name]]`.

## Quick Start

```bash
cargo build --release
./target/release/obi
```

Requires Rust 1.75+. Optionally install [Ollama](https://ollama.ai) for free local LLM and embedding support.

## Works With Any LLM

Use Claude, OpenAI, or Ollama — or mix them. Run embeddings locally with Ollama while using Claude for completions. Switch providers without changing anything else. API keys are stored in your OS keyring, never in plaintext.

## Built With

Rust, Ratatui, Tree-sitter, LanceDB. No Electron. No browser. Just your terminal.

## License

MIT
