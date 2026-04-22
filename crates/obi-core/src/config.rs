use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Top-level Obi configuration.
///
/// Loaded from `~/.obi/config.toml` (global defaults) and `.obi/config.toml`
/// (project overrides). Project fields override global field-by-field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObiConfig {
    #[serde(default)]
    pub sources: SourceConfig,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub brain: BrainConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub code: Vec<PathBuf>,
    pub notes: Vec<PathBuf>,
    pub docs: Vec<PathBuf>,
    pub exclude: Vec<String>,
}

// ─── Providers ─────────────────────────────────────────────────────

/// All provider configurations live under one roof.
/// `completion` and `embedding` select the active provider by name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    /// Active completion provider: "ollama" | "anthropic" | "zai".
    pub completion: String,
    /// Active embedding provider: "ollama".
    pub embedding: String,
    /// Optional fallback completion provider.
    pub fallback: Option<String>,
    #[serde(default)]
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
    #[serde(default)]
    pub zai: ZaiConfig,
}

// ─── Shared model configs ──────────────────────────────────────────

/// Model settings shared by every completion provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionModelConfig {
    pub model: String,
    pub max_context: usize,
}

/// Model settings shared by every embedding provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingModelConfig {
    pub model: String,
    pub dimensions: usize,
}

// ─── Per-provider configs ──────────────────────────────────────────

/// Ollama — local inference server. Supports both completion and embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    pub host: String,
    #[serde(default = "OllamaConfig::default_completion")]
    pub completion: CompletionModelConfig,
    #[serde(default = "OllamaConfig::default_embedding")]
    pub embedding: EmbeddingModelConfig,
}

/// Anthropic Claude — remote API, completion only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    pub api_key: String,
    #[serde(default = "AnthropicConfig::default_completion")]
    pub completion: CompletionModelConfig,
}

/// Z.ai (Zhipu AI) Coding Plan — remote API, completion only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaiConfig {
    pub api_key: String,
    pub base_url: String,
    #[serde(default = "ZaiConfig::default_completion")]
    pub completion: CompletionModelConfig,
}

// ─── Brain ─────────────────────────────────────────────────────────

/// Dual-brain configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    pub project_enabled: bool,
    pub global_enabled: bool,
    pub project_boost: f64,
    pub code_sources: Vec<PathBuf>,
    pub note_sources: Vec<PathBuf>,
    pub exclude: Vec<String>,
    pub max_node_tokens: usize,
}

// ─── Defaults ──────────────────────────────────────────────────────

impl Default for ObiConfig {
    fn default() -> Self {
        Self {
            sources: SourceConfig::default(),
            providers: ProvidersConfig::default(),
            brain: BrainConfig::default(),
        }
    }
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            code: vec![PathBuf::from("src/"), PathBuf::from("lib/")],
            notes: vec![PathBuf::from(".obi/notes/")],
            docs: vec![PathBuf::from("docs/")],
            exclude: vec![
                "target/".into(),
                "node_modules/".into(),
                ".git/".into(),
            ],
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            completion: "ollama".into(),
            embedding: "ollama".into(),
            fallback: None,
            ollama: OllamaConfig::default(),
            anthropic: AnthropicConfig::default(),
            zai: ZaiConfig::default(),
        }
    }
}

impl OllamaConfig {
    fn default_completion() -> CompletionModelConfig {
        CompletionModelConfig {
            model: "gemma4:e4b".into(),
            max_context: 32768,
        }
    }

    fn default_embedding() -> EmbeddingModelConfig {
        EmbeddingModelConfig {
            model: "nomic-embed-text".into(),
            dimensions: 768,
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            host: "http://localhost:11434".into(),
            completion: Self::default_completion(),
            embedding: Self::default_embedding(),
        }
    }
}

impl AnthropicConfig {
    fn default_completion() -> CompletionModelConfig {
        CompletionModelConfig {
            model: "claude-sonnet-4-6".into(),
            max_context: 200000,
        }
    }
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            completion: Self::default_completion(),
        }
    }
}

impl ZaiConfig {
    fn default_completion() -> CompletionModelConfig {
        CompletionModelConfig {
            model: "glm-4.6".into(),
            max_context: 200000,
        }
    }
}

impl Default for ZaiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: "https://api.z.ai/api/coding/paas/v4".into(),
            completion: Self::default_completion(),
        }
    }
}

impl Default for BrainConfig {
    fn default() -> Self {
        Self {
            project_enabled: true,
            global_enabled: true,
            project_boost: 1.5,
            code_sources: vec![PathBuf::from("src/"), PathBuf::from("lib/")],
            note_sources: vec![PathBuf::from(".obi/notes/")],
            exclude: vec![
                "target/".into(),
                "node_modules/".into(),
                ".git/".into(),
            ],
            max_node_tokens: 500,
        }
    }
}

// ─── Config loading & merging ──────────────────────────────────────

impl ObiConfig {
    /// Load config by merging global (~/.obi/config.toml) and project (.obi/config.toml).
    /// Project values override global values field-by-field.
    pub fn load(project_root: &Path) -> Self {
        let global = Self::load_file(&global_config_path());
        let project = Self::load_file(&project_root.join(".obi").join("config.toml"));

        match (global, project) {
            (Some(g), Some(p)) => g.merge_with(p),
            (Some(g), None) => g,
            (None, Some(p)) => ObiConfig::default().merge_with(p),
            (None, None) => ObiConfig::default(),
        }
    }

    fn load_file(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        toml::from_str(&content).ok()
    }

    /// Merge self with an override config. Override values take precedence
    /// for non-default fields (field-by-field override).
    fn merge_with(mut self, over: ObiConfig) -> Self {
        // Brain
        let db = BrainConfig::default();
        if over.brain.project_enabled != db.project_enabled {
            self.brain.project_enabled = over.brain.project_enabled;
        }
        if over.brain.global_enabled != db.global_enabled {
            self.brain.global_enabled = over.brain.global_enabled;
        }
        if (over.brain.project_boost - db.project_boost).abs() > f64::EPSILON {
            self.brain.project_boost = over.brain.project_boost;
        }
        if over.brain.code_sources != db.code_sources {
            self.brain.code_sources = over.brain.code_sources;
        }
        if over.brain.note_sources != db.note_sources {
            self.brain.note_sources = over.brain.note_sources;
        }
        if over.brain.exclude != db.exclude {
            self.brain.exclude = over.brain.exclude;
        }
        if over.brain.max_node_tokens != db.max_node_tokens {
            self.brain.max_node_tokens = over.brain.max_node_tokens;
        }

        // Providers — routing
        let dp = ProvidersConfig::default();
        if over.providers.completion != dp.completion {
            self.providers.completion = over.providers.completion;
        }
        if over.providers.embedding != dp.embedding {
            self.providers.embedding = over.providers.embedding;
        }
        if over.providers.fallback != dp.fallback {
            self.providers.fallback = over.providers.fallback;
        }

        // Ollama
        let do_ = OllamaConfig::default();
        if over.providers.ollama.host != do_.host {
            self.providers.ollama.host = over.providers.ollama.host;
        }
        if over.providers.ollama.completion.model != do_.completion.model {
            self.providers.ollama.completion.model = over.providers.ollama.completion.model;
        }
        if over.providers.ollama.completion.max_context != do_.completion.max_context {
            self.providers.ollama.completion.max_context = over.providers.ollama.completion.max_context;
        }
        if over.providers.ollama.embedding.model != do_.embedding.model {
            self.providers.ollama.embedding.model = over.providers.ollama.embedding.model;
        }
        if over.providers.ollama.embedding.dimensions != do_.embedding.dimensions {
            self.providers.ollama.embedding.dimensions = over.providers.ollama.embedding.dimensions;
        }

        // Anthropic
        let da = AnthropicConfig::default();
        if over.providers.anthropic.api_key != da.api_key {
            self.providers.anthropic.api_key = over.providers.anthropic.api_key;
        }
        if over.providers.anthropic.completion.model != da.completion.model {
            self.providers.anthropic.completion.model = over.providers.anthropic.completion.model;
        }
        if over.providers.anthropic.completion.max_context != da.completion.max_context {
            self.providers.anthropic.completion.max_context = over.providers.anthropic.completion.max_context;
        }

        // Z.ai
        let dz = ZaiConfig::default();
        if over.providers.zai.api_key != dz.api_key {
            self.providers.zai.api_key = over.providers.zai.api_key;
        }
        if over.providers.zai.base_url != dz.base_url {
            self.providers.zai.base_url = over.providers.zai.base_url;
        }
        if over.providers.zai.completion.model != dz.completion.model {
            self.providers.zai.completion.model = over.providers.zai.completion.model;
        }
        if over.providers.zai.completion.max_context != dz.completion.max_context {
            self.providers.zai.completion.max_context = over.providers.zai.completion.max_context;
        }

        self
    }
}

/// Path to the global config: ~/.obi/config.toml
pub fn global_obi_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".obi")
}

fn global_config_path() -> PathBuf {
    global_obi_dir().join("config.toml")
}

/// Path to the project's .obi directory.
pub fn project_obi_dir(project_root: &Path) -> PathBuf {
    project_root.join(".obi")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ObiConfig::default();
        assert!(config.brain.project_enabled);
        assert!(config.brain.global_enabled);
        assert!((config.brain.project_boost - 1.5).abs() < f64::EPSILON);
        assert_eq!(config.brain.max_node_tokens, 500);
    }

    #[test]
    fn test_brain_config_defaults() {
        let brain = BrainConfig::default();
        assert_eq!(brain.code_sources, vec![PathBuf::from("src/"), PathBuf::from("lib/")]);
        assert_eq!(brain.note_sources, vec![PathBuf::from(".obi/notes/")]);
        assert_eq!(brain.exclude.len(), 3);
    }

    #[test]
    fn test_merge_override_project_boost() {
        let base = ObiConfig::default();
        let mut over = ObiConfig::default();
        over.brain.project_boost = 2.0;

        let merged = base.merge_with(over);
        assert!((merged.brain.project_boost - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_merge_preserves_base_when_override_is_default() {
        let mut base = ObiConfig::default();
        base.brain.project_boost = 2.5;
        base.brain.max_node_tokens = 1000;

        let over = ObiConfig::default();

        let merged = base.merge_with(over);
        assert!((merged.brain.project_boost - 2.5).abs() < f64::EPSILON);
        assert_eq!(merged.brain.max_node_tokens, 1000);
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let config = ObiConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: ObiConfig = toml::from_str(&toml_str).unwrap();
        assert!((parsed.brain.project_boost - 1.5).abs() < f64::EPSILON);
        assert!(parsed.brain.project_enabled);
    }

    #[test]
    fn test_global_obi_dir() {
        let dir = global_obi_dir();
        assert!(dir.to_string_lossy().contains(".obi"));
    }

    #[test]
    fn test_providers_structure() {
        let config = ObiConfig::default();
        assert_eq!(config.providers.completion, "ollama");
        assert_eq!(config.providers.embedding, "ollama");
        assert_eq!(config.providers.ollama.completion.model, "gemma4:e4b");
        assert_eq!(config.providers.ollama.embedding.model, "nomic-embed-text");
        assert_eq!(config.providers.anthropic.completion.model, "claude-sonnet-4-6");
        assert_eq!(config.providers.zai.completion.model, "glm-4.6");
    }

    #[test]
    fn test_merge_provider_override() {
        let base = ObiConfig::default();
        let mut over = ObiConfig::default();
        over.providers.completion = "anthropic".into();
        over.providers.anthropic.api_key = "sk-test".into();

        let merged = base.merge_with(over);
        assert_eq!(merged.providers.completion, "anthropic");
        assert_eq!(merged.providers.anthropic.api_key, "sk-test");
        // Ollama defaults preserved
        assert_eq!(merged.providers.ollama.host, "http://localhost:11434");
    }
}
