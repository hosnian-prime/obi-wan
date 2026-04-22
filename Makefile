# Obi-Wan — AI-Native TUI IDE
# ============================

.PHONY: build run index status brain test check clean release install help

# Default: show help
help:
	@echo "Obi-Wan — AI-Native TUI IDE"
	@echo ""
	@echo "Usage: make <target>"
	@echo ""
	@echo "Development:"
	@echo "  make build      Build debug binary"
	@echo "  make run        Build and launch the TUI"
	@echo "  make check      Run cargo check (fast compilation check)"
	@echo "  make test       Run all tests"
	@echo "  make clippy     Run clippy lints"
	@echo "  make fmt        Format all code"
	@echo "  make fmt-check  Check formatting without changing files"
	@echo ""
	@echo "Commands:"
	@echo "  make index      Index the current project (obi index)"
	@echo "  make status     Show index stats (obi status)"
	@echo "  make brain      Show dual-brain status (obi brain)"
	@echo ""
	@echo "Release:"
	@echo "  make release    Build optimized release binary"
	@echo "  make install    Install to ~/.cargo/bin"
	@echo ""
	@echo "Cleanup:"
	@echo "  make clean      Remove build artifacts"
	@echo "  make clean-obi  Remove .obi/ index data"
	@echo "  make clean-all  Remove build artifacts + .obi/ data"
	@echo ""
	@echo "Prerequisites:"
	@echo "  make deps       Check and show required dependencies"
	@echo "  make ollama     Start Ollama server (required for AI features)"

# ── Development ──────────────────────────────────────────

build:
	cargo build

run: build
	cargo run --bin obi

check:
	cargo check

test:
	cargo test

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

# ── Commands ─────────────────────────────────────────────

index: build
	cargo run --bin obi -- index

status: build
	cargo run --bin obi -- status

brain: build
	cargo run --bin obi -- brain

# ── Release ──────────────────────────────────────────────

release:
	cargo build --release
	@echo ""
	@echo "Binary: target/release/obi"
	@ls -lh target/release/obi

install:
	cargo install --path crates/obi-tui
	@echo ""
	@echo "Installed 'obi' to ~/.cargo/bin/obi"

# ── Cleanup ──────────────────────────────────────────────

clean:
	cargo clean

clean-obi:
	rm -rf .obi/
	@echo "Removed .obi/ directory"

clean-all: clean clean-obi

# ── Prerequisites ────────────────────────────────────────

deps:
	@echo "Checking dependencies..."
	@echo ""
	@printf "  rustc:   " && (rustc --version 2>/dev/null || echo "NOT FOUND — install from https://rustup.rs")
	@printf "  cargo:   " && (cargo --version 2>/dev/null || echo "NOT FOUND — install from https://rustup.rs")
	@printf "  ollama:  " && (ollama --version 2>/dev/null || echo "NOT FOUND — install from https://ollama.ai")
	@echo ""
	@echo "Required Ollama models:"
	@echo "  ollama pull gemma4:e4b         # Completion (32k context)"
	@echo "  ollama pull nomic-embed-text    # Embeddings (768d)"
	@echo ""
	@echo "Pull models:"
	@echo "  make pull-models"

pull-models:
	ollama pull nomic-embed-text
	ollama pull gemma4:e4b

ollama:
	@echo "Starting Ollama server..."
	ollama serve

# ── Quick Start ──────────────────────────────────────────
# 1. make deps          — check prerequisites
# 2. make pull-models   — download required AI models
# 3. make ollama        — start Ollama in another terminal
# 4. make index         — index the current project
# 5. make run           — launch the TUI
