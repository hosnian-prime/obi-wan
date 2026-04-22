#!/usr/bin/env bash
set -euo pipefail

# ─── obi-wan installer ──────────────────────────────────────────────
# AI-Native TUI IDE — VS Code UI + Obsidian graph + AI agent
# https://github.com/l00pss/obi-wan
# ─────────────────────────────────────────────────────────────────────

BOLD='\033[1m'
DIM='\033[2m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
CYAN='\033[0;36m'
PURPLE='\033[0;35m'
RESET='\033[0m'

OBI_CONFIG_DIR="$HOME/.obi"
OBI_CONFIG_FILE="$OBI_CONFIG_DIR/config.toml"
OLLAMA_MODEL="qwen3.5:9b"
EMBED_MODEL="nomic-embed-text"
MIN_RUST_VERSION="1.75.0"

# ─── Helpers ─────────────────────────────────────────────────────────

info()  { printf "${CYAN}  [*]${RESET} %s\n" "$1"; }
ok()    { printf "${GREEN}  [+]${RESET} %s\n" "$1"; }
warn()  { printf "${YELLOW}  [!]${RESET} %s\n" "$1"; }
fail()  { printf "${RED}  [-]${RESET} %s\n" "$1"; exit 1; }

header() {
    printf "\n${PURPLE}${BOLD}%s${RESET}\n" "$1"
    printf "${DIM}%s${RESET}\n\n" "$(printf '%.0s─' $(seq 1 ${#1}))"
}

version_ge() {
    # Returns 0 if $1 >= $2 (semver comparison)
    [ "$(printf '%s\n%s' "$1" "$2" | sort -V | head -n1)" = "$2" ]
}

# ─── Banner ──────────────────────────────────────────────────────────

printf "\n"
printf "${PURPLE}${BOLD}"
printf "   ____  __    _       _       __\n"
printf "  / __ \\/ /_  (_)     | |     / /___ _____\n"
printf " / / / / __ \\/ /______| | /| / / __ \`/ __ \\\\\n"
printf "/ /_/ / /_/ / //_____/| |/ |/ / /_/ / / / /\n"
printf "\\____/_.___/_/        |__/|__/\\__,_/_/ /_/\n"
printf "${RESET}\n"
printf "${DIM}  AI-Native TUI IDE${RESET}\n\n"

# ─── Step 1: Check system dependencies ──────────────────────────────

header "Checking prerequisites"

# Check OS
OS="$(uname -s)"
case "$OS" in
    Linux*)  PLATFORM="linux" ;;
    Darwin*) PLATFORM="macos" ;;
    *)       fail "Unsupported OS: $OS (only Linux and macOS are supported)" ;;
esac
ok "Platform: $PLATFORM ($(uname -m))"

# Check Rust
if ! command -v rustc &>/dev/null; then
    warn "Rust not found"
    info "Installing Rust via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    ok "Rust installed: $(rustc --version)"
else
    RUST_VER="$(rustc --version | awk '{print $2}')"
    if version_ge "$RUST_VER" "$MIN_RUST_VERSION"; then
        ok "Rust: $RUST_VER"
    else
        fail "Rust $RUST_VER is too old (need >= $MIN_RUST_VERSION). Run: rustup update"
    fi
fi

# Check cargo
command -v cargo &>/dev/null || fail "cargo not found (should come with Rust)"

# Check git
command -v git &>/dev/null || fail "git not found. Install it first."
ok "git: $(git --version | awk '{print $3}')"

# ─── Step 2: Build ──────────────────────────────────────────────────

header "Building obi-wan"

info "Compiling release binary (this may take a few minutes)..."
cargo build --release 2>&1 | tail -1

BINARY="target/release/obi"
if [ ! -f "$BINARY" ]; then
    fail "Build failed — binary not found at $BINARY"
fi

BIN_SIZE=$(du -h "$BINARY" | awk '{print $1}')
ok "Built: $BINARY ($BIN_SIZE)"

# ─── Step 3: Install binary ─────────────────────────────────────────

header "Installing"

INSTALL_DIR="$HOME/.cargo/bin"
mkdir -p "$INSTALL_DIR"
cp "$BINARY" "$INSTALL_DIR/obi"
chmod +x "$INSTALL_DIR/obi"
ok "Installed: $INSTALL_DIR/obi"

# Check PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
    warn "$INSTALL_DIR is not in your PATH"
    info "Add this to your shell profile:"
    printf "\n  ${DIM}export PATH=\"\$HOME/.cargo/bin:\$PATH\"${RESET}\n\n"
fi

# ─── Step 4: Default config ─────────────────────────────────────────

header "Configuration"

if [ -f "$OBI_CONFIG_FILE" ]; then
    ok "Config exists: $OBI_CONFIG_FILE"
else
    mkdir -p "$OBI_CONFIG_DIR"
    cat > "$OBI_CONFIG_FILE" << 'TOML'
# obi-wan configuration
# Global config: ~/.obi/config.toml
# Project override: <project>/.obi/config.toml

[llm]
primary = "ollama"
# fallback = "ollama"

[llm.ollama]
model = "qwen3.5:9b"
host = "http://localhost:11434"

[embedding]
provider = "ollama"
model = "nomic-embed-text"
dimensions = 768

[brain]
project_enabled = true
global_enabled = true

[sources]
code = ["src/", "lib/", "crates/"]
notes = [".obi/notes/"]
docs = ["docs/"]
TOML
    ok "Created: $OBI_CONFIG_FILE"
fi

# ─── Step 5: Ollama setup ───────────────────────────────────────────

header "Ollama (AI backend)"

if command -v ollama &>/dev/null; then
    ok "Ollama found: $(ollama --version 2>&1 | head -1)"

    # Check if Ollama is running
    if curl -sf http://localhost:11434/api/tags &>/dev/null; then
        ok "Ollama server is running"

        # Check/pull models
        INSTALLED_MODELS=$(curl -sf http://localhost:11434/api/tags | grep -o '"name":"[^"]*"' | sed 's/"name":"//;s/"//')

        if echo "$INSTALLED_MODELS" | grep -q "$OLLAMA_MODEL"; then
            ok "Model ready: $OLLAMA_MODEL"
        else
            info "Pulling $OLLAMA_MODEL..."
            ollama pull "$OLLAMA_MODEL"
            ok "Model pulled: $OLLAMA_MODEL"
        fi

        if echo "$INSTALLED_MODELS" | grep -q "$EMBED_MODEL"; then
            ok "Model ready: $EMBED_MODEL"
        else
            info "Pulling $EMBED_MODEL..."
            ollama pull "$EMBED_MODEL"
            ok "Model pulled: $EMBED_MODEL"
        fi
    else
        warn "Ollama is installed but not running"
        info "Start it with: ollama serve"
    fi
else
    warn "Ollama not found"
    info "Install from: https://ollama.ai"
    info "Then run:"
    printf "  ${DIM}ollama serve${RESET}\n"
    printf "  ${DIM}ollama pull %s${RESET}\n" "$OLLAMA_MODEL"
    printf "  ${DIM}ollama pull %s${RESET}\n\n" "$EMBED_MODEL"
fi

# ─── Done ────────────────────────────────────────────────────────────

header "Installation complete"

printf "  ${GREEN}${BOLD}obi-wan is ready!${RESET}\n\n"
printf "  ${BOLD}Quick start:${RESET}\n"
printf "    ${DIM}1.${RESET} cd your-project/\n"
printf "    ${DIM}2.${RESET} obi index          ${DIM}# index codebase${RESET}\n"
printf "    ${DIM}3.${RESET} obi                ${DIM}# launch TUI${RESET}\n"
printf "\n"
printf "  ${BOLD}Keybindings:${RESET}\n"
printf "    ${DIM}Tab${RESET}       Switch panels\n"
printf "    ${DIM}Ctrl+G${RESET}    Toggle graph\n"
printf "    ${DIM}Ctrl+P${RESET}    Command palette\n"
printf "    ${DIM}Ctrl+Q${RESET}    Quit\n"
printf "\n"
printf "  ${DIM}Config: %s${RESET}\n" "$OBI_CONFIG_FILE"
printf "  ${DIM}Docs:   make help${RESET}\n\n"
