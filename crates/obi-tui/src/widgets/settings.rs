use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use obi_core::config::{
    AnthropicConfig, CompletionModelConfig, EmbeddingModelConfig, ObiConfig, OllamaConfig, ZaiConfig,
};

// ─── Theme ──────────────────────────────────────────────────────────
const CLR_LABEL: Color = Color::Rgb(140, 140, 160);
const CLR_VALUE: Color = Color::Rgb(140, 200, 255);
const CLR_SELECTED_BG: Color = Color::Rgb(40, 40, 60);
const CLR_EDITING: Color = Color::Yellow;
const CLR_SECTION: Color = Color::Rgb(180, 140, 255);
const CLR_HINT: Color = Color::Rgb(80, 80, 100);
const CLR_SUCCESS: Color = Color::Green;
const CLR_DROPDOWN_BG: Color = Color::Rgb(35, 35, 55);
const CLR_DROPDOWN_HOVER: Color = Color::Rgb(60, 60, 100);
const CLR_DROPDOWN_TEXT: Color = Color::Rgb(200, 200, 220);
const CLR_DROPDOWN_ACTIVE: Color = Color::Rgb(120, 220, 160);
const CLR_DROPDOWN_BORDER: Color = Color::Rgb(80, 80, 120);

/// Action returned from settings key handler.
pub enum SettingsAction {
    /// Nothing happened.
    None,
    /// Config was saved — caller should hot-reload the agent.
    Saved,
    /// Need to fetch model list from Anthropic API (pass the api_key).
    FetchAnthropicModels(String),
    /// Need to fetch model list from Z.ai API (pass the api_key).
    FetchZaiModels(String),
}

// ─── Available providers ────────────────────────────────────────────

const LLM_PROVIDERS: &[&str] = &["ollama", "anthropic", "zai"];
const EMBED_PROVIDERS: &[&str] = &["ollama"];

// ─── Settings fields ────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Field {
    LlmProvider,
    OllamaModel,
    OllamaHost,
    OllamaMaxContext,
    AnthropicApiKey,
    AnthropicModel,
    AnthropicMaxContext,
    ZaiApiKey,
    ZaiModel,
    ZaiMaxContext,
    ZaiBaseUrl,
    EmbeddingEnabled,
    EmbedProvider,
    EmbedModel,
    EmbedDimensions,
}

impl Field {
    fn label(self) -> &'static str {
        match self {
            Field::LlmProvider => "Provider",
            Field::OllamaModel => "Model",
            Field::OllamaHost => "Host",
            Field::OllamaMaxContext => "Max Context",
            Field::AnthropicApiKey => "API Key",
            Field::AnthropicModel => "Model",
            Field::AnthropicMaxContext => "Max Context",
            Field::ZaiApiKey => "API Key",
            Field::ZaiModel => "Model",
            Field::ZaiMaxContext => "Max Context",
            Field::ZaiBaseUrl => "Base URL",
            Field::EmbeddingEnabled => "Enabled",
            Field::EmbedProvider => "Provider",
            Field::EmbedModel => "Model",
            Field::EmbedDimensions => "Dimensions",
        }
    }

    fn section(self) -> &'static str {
        match self {
            Field::LlmProvider => "Completion",
            Field::OllamaModel | Field::OllamaHost | Field::OllamaMaxContext => "  Ollama",
            Field::AnthropicApiKey | Field::AnthropicModel | Field::AnthropicMaxContext => "  Anthropic",
            Field::ZaiApiKey | Field::ZaiModel | Field::ZaiMaxContext | Field::ZaiBaseUrl => "  Z.ai",
            Field::EmbeddingEnabled | Field::EmbedProvider | Field::EmbedModel | Field::EmbedDimensions => "Embedding",
        }
    }

    fn dropdown_options(self) -> Option<&'static [&'static str]> {
        match self {
            Field::LlmProvider => Some(LLM_PROVIDERS),
            Field::EmbedProvider => Some(EMBED_PROVIDERS),
            Field::EmbeddingEnabled => Some(&["true", "false"]),
            _ => None,
        }
    }
}

// ─── Dropdown state ─────────────────────────────────────────────────

struct Dropdown {
    cursor: usize,
    /// Owned list of options (either from static list or fetched from API).
    options: Vec<String>,
    /// Row offset of the field within the settings inner area (for positioning).
    field_row: u16,
    /// Whether the dropdown is still loading data from an API.
    loading: bool,
}

// ─── Widget ─────────────────────────────────────────────────────────

pub struct SettingsWidget {
    pub visible: bool,
    selected: usize,
    editing: bool,
    edit_buffer: String,
    status_msg: Option<&'static str>,
    dropdown: Option<Dropdown>,
    /// Cached Anthropic model list (fetched once per session).
    anthropic_models: Option<Vec<String>>,
    /// Cached Z.ai model list (fetched once per session).
    zai_models: Option<Vec<String>>,
    // Completion
    llm_provider: String,
    // Ollama
    ollama_model: String,
    ollama_host: String,
    ollama_max_context: String,
    // Anthropic
    anthropic_api_key: String,
    anthropic_model: String,
    anthropic_max_context: String,
    // Z.ai
    zai_api_key: String,
    zai_model: String,
    zai_max_context: String,
    zai_base_url: String,
    // Embedding
    embedding_enabled: String,
    embed_provider: String,
    embed_model: String,
    embed_dimensions: String,
}

impl SettingsWidget {
    pub fn new() -> Self {
        Self {
            visible: false,
            selected: 0,
            editing: false,
            edit_buffer: String::new(),
            status_msg: None,
            dropdown: None,
            anthropic_models: None,
            zai_models: None,
            llm_provider: String::new(),
            ollama_model: String::new(),
            ollama_host: String::new(),
            ollama_max_context: String::new(),
            anthropic_api_key: String::new(),
            anthropic_model: String::new(),
            anthropic_max_context: String::new(),
            zai_api_key: String::new(),
            zai_model: String::new(),
            zai_max_context: String::new(),
            zai_base_url: String::new(),
            embedding_enabled: String::new(),
            embed_provider: String::new(),
            embed_model: String::new(),
            embed_dimensions: String::new(),
        }
    }

    /// Open the settings panel, loading current config values.
    pub fn open(&mut self, config: &ObiConfig) {
        self.visible = true;
        self.selected = 0;
        self.editing = false;
        self.dropdown = None;
        self.status_msg = None;

        let p = &config.providers;
        self.llm_provider = p.completion.clone();
        self.ollama_model = p.ollama.completion.model.clone();
        self.ollama_host = p.ollama.host.clone();
        self.ollama_max_context = p.ollama.completion.max_context.to_string();
        self.anthropic_api_key = p.anthropic.api_key.clone();
        self.anthropic_model = p.anthropic.completion.model.clone();
        self.anthropic_max_context = p.anthropic.completion.max_context.to_string();
        self.zai_api_key = p.zai.api_key.clone();
        self.zai_model = p.zai.completion.model.clone();
        self.zai_max_context = p.zai.completion.max_context.to_string();
        self.zai_base_url = p.zai.base_url.clone();
        self.embedding_enabled = config.brain.embedding_enabled.to_string();
        self.embed_provider = p.embedding.clone();
        self.embed_model = p.ollama.embedding.model.clone();
        self.embed_dimensions = p.ollama.embedding.dimensions.to_string();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.editing = false;
        self.dropdown = None;
    }

    /// Fields visible for the currently selected LLM provider.
    fn visible_fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::LlmProvider];
        match self.llm_provider.as_str() {
            "anthropic" => {
                fields.extend_from_slice(&[
                    Field::AnthropicApiKey,
                    Field::AnthropicModel,
                    Field::AnthropicMaxContext,
                ]);
            }
            "zai" => {
                fields.extend_from_slice(&[
                    Field::ZaiApiKey,
                    Field::ZaiModel,
                    Field::ZaiMaxContext,
                    Field::ZaiBaseUrl,
                ]);
            }
            _ => {
                fields.extend_from_slice(&[
                    Field::OllamaModel,
                    Field::OllamaHost,
                    Field::OllamaMaxContext,
                ]);
            }
        }
        fields.push(Field::EmbeddingEnabled);
        if self.embedding_enabled == "true" {
            fields.extend_from_slice(&[
                Field::EmbedProvider,
                Field::EmbedModel,
                Field::EmbedDimensions,
            ]);
        }
        fields
    }

    /// Handle key input. Returns a `SettingsAction` indicating what the caller should do.
    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsAction {
        // Dropdown intercepts all keys while open
        if self.dropdown.is_some() {
            return self.handle_dropdown_key(key);
        }

        if self.editing {
            return self.handle_edit_key(key);
        }

        let fields = self.visible_fields();

        match key.code {
            KeyCode::Esc => {
                self.close();
                SettingsAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < fields.len() {
                    self.selected += 1;
                }
                SettingsAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                SettingsAction::None
            }
            KeyCode::Enter => {
                let field = fields[self.selected];
                if field == Field::AnthropicModel {
                    return self.open_model_dropdown(field);
                }
                if field == Field::ZaiModel {
                    return self.open_zai_model_dropdown(field);
                }
                if let Some(options) = field.dropdown_options() {
                    self.open_static_dropdown(field, options);
                } else {
                    self.start_editing(&fields);
                }
                SettingsAction::None
            }
            KeyCode::Char('s') => {
                self.save_config();
                SettingsAction::Saved
            }
            _ => SettingsAction::None,
        }
    }

    fn open_static_dropdown(&mut self, field: Field, options: &[&str]) {
        let current = self.field_value(field);
        let owned: Vec<String> = options.iter().map(|s| s.to_string()).collect();
        let cursor = owned.iter().position(|o| o == current).unwrap_or(0);
        let field_row = self.field_render_row(field);
        self.dropdown = Some(Dropdown { cursor, options: owned, field_row, loading: false });
    }

    /// Open model dropdown for Anthropic — uses cached list or triggers async fetch.
    fn open_model_dropdown(&mut self, field: Field) -> SettingsAction {
        let field_row = self.field_render_row(field);

        if let Some(ref models) = self.anthropic_models {
            let cursor = models.iter().position(|m| m == &self.anthropic_model).unwrap_or(0);
            self.dropdown = Some(Dropdown {
                cursor,
                options: models.clone(),
                field_row,
                loading: false,
            });
            return SettingsAction::None;
        }

        // No cached list — show loading dropdown and request fetch
        if self.anthropic_api_key.is_empty() {
            self.status_msg = Some("Set API Key first");
            return SettingsAction::None;
        }

        self.dropdown = Some(Dropdown {
            cursor: 0,
            options: vec![],
            field_row,
            loading: true,
        });
        SettingsAction::FetchAnthropicModels(self.anthropic_api_key.clone())
    }

    /// Open model dropdown for Z.ai — uses cached list or triggers async fetch.
    fn open_zai_model_dropdown(&mut self, field: Field) -> SettingsAction {
        let field_row = self.field_render_row(field);

        if let Some(ref models) = self.zai_models {
            let cursor = models.iter().position(|m| m == &self.zai_model).unwrap_or(0);
            self.dropdown = Some(Dropdown {
                cursor,
                options: models.clone(),
                field_row,
                loading: false,
            });
            return SettingsAction::None;
        }

        if self.zai_api_key.is_empty() {
            self.status_msg = Some("Set API Key first");
            return SettingsAction::None;
        }

        self.dropdown = Some(Dropdown {
            cursor: 0,
            options: vec![],
            field_row,
            loading: true,
        });
        SettingsAction::FetchZaiModels(self.zai_api_key.clone())
    }

    /// Called by the app when model list arrives from the async fetch.
    pub fn receive_model_list(&mut self, models: Vec<String>) {
        self.anthropic_models = Some(models.clone());
        // If dropdown is still open and loading, populate it
        if let Some(ref mut dd) = self.dropdown {
            if dd.loading {
                dd.options = models;
                dd.cursor = dd.options.iter().position(|m| m == &self.anthropic_model).unwrap_or(0);
                dd.loading = false;
            }
        }
    }

    /// Called by the app when Z.ai model list arrives from the async fetch.
    pub fn receive_zai_model_list(&mut self, models: Vec<String>) {
        self.zai_models = Some(models.clone());
        if let Some(ref mut dd) = self.dropdown {
            if dd.loading {
                dd.options = models;
                dd.cursor = dd.options.iter().position(|m| m == &self.zai_model).unwrap_or(0);
                dd.loading = false;
            }
        }
    }

    /// Compute the row index of a field within the inner area (for dropdown positioning).
    fn field_render_row(&self, target: Field) -> u16 {
        let fields = self.visible_fields();
        let mut row = 0u16;
        let mut prev_section = "";
        for &field in &fields {
            let section = field.section();
            if section != prev_section {
                if !prev_section.is_empty() {
                    row += 1; // blank line
                }
                row += 1; // section header
                prev_section = section;
            }
            if field == target {
                return row;
            }
            row += 1;
        }
        row
    }

    fn handle_dropdown_key(&mut self, key: KeyEvent) -> SettingsAction {
        let dd = self.dropdown.as_mut().unwrap();
        if dd.loading {
            // While loading, only Esc cancels
            if key.code == KeyCode::Esc {
                self.dropdown = None;
            }
            return SettingsAction::None;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if dd.cursor + 1 < dd.options.len() {
                    dd.cursor += 1;
                }
                SettingsAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                dd.cursor = dd.cursor.saturating_sub(1);
                SettingsAction::None
            }
            KeyCode::Enter => {
                if dd.options.is_empty() {
                    self.dropdown = None;
                    return SettingsAction::None;
                }
                let selected_value = dd.options[dd.cursor].clone();
                let fields = self.visible_fields();
                let field = fields[self.selected];
                self.apply_dropdown(field, selected_value);
                self.dropdown = None;
                SettingsAction::None
            }
            KeyCode::Esc => {
                self.dropdown = None;
                SettingsAction::None
            }
            _ => SettingsAction::None,
        }
    }

    fn apply_dropdown(&mut self, field: Field, value: String) {
        self.status_msg = None;
        match field {
            Field::LlmProvider => {
                self.llm_provider = value;
                let max = self.visible_fields().len().saturating_sub(1);
                if self.selected > max {
                    self.selected = max;
                }
            }
            Field::EmbeddingEnabled => {
                self.embedding_enabled = value;
            }
            Field::EmbedProvider => {
                self.embed_provider = value;
            }
            Field::AnthropicModel => {
                self.anthropic_model = value;
            }
            Field::ZaiModel => {
                self.zai_model = value;
            }
            _ => {}
        }
    }

    fn start_editing(&mut self, fields: &[Field]) {
        self.editing = true;
        self.status_msg = None;
        self.edit_buffer = self.field_value(fields[self.selected]).to_string();
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> SettingsAction {
        match key.code {
            KeyCode::Esc => {
                self.editing = false;
                SettingsAction::None
            }
            KeyCode::Enter => {
                self.apply_edit();
                self.editing = false;
                SettingsAction::None
            }
            KeyCode::Char(c) => {
                self.edit_buffer.push(c);
                SettingsAction::None
            }
            KeyCode::Backspace => {
                self.edit_buffer.pop();
                SettingsAction::None
            }
            _ => SettingsAction::None,
        }
    }

    fn apply_edit(&mut self) {
        let value = self.edit_buffer.clone();
        let fields = self.visible_fields();
        match fields[self.selected] {
            Field::LlmProvider => self.llm_provider = value,
            Field::OllamaModel => self.ollama_model = value,
            Field::OllamaHost => self.ollama_host = value,
            Field::OllamaMaxContext => self.ollama_max_context = value,
            Field::AnthropicApiKey => self.anthropic_api_key = value,
            Field::AnthropicModel => self.anthropic_model = value,
            Field::AnthropicMaxContext => self.anthropic_max_context = value,
            Field::ZaiApiKey => self.zai_api_key = value,
            Field::ZaiModel => self.zai_model = value,
            Field::ZaiMaxContext => self.zai_max_context = value,
            Field::ZaiBaseUrl => self.zai_base_url = value,
            Field::EmbeddingEnabled => self.embedding_enabled = value,
            Field::EmbedProvider => self.embed_provider = value,
            Field::EmbedModel => self.embed_model = value,
            Field::EmbedDimensions => self.embed_dimensions = value,
        }
    }

    fn field_value(&self, field: Field) -> &str {
        match field {
            Field::LlmProvider => &self.llm_provider,
            Field::OllamaModel => &self.ollama_model,
            Field::OllamaHost => &self.ollama_host,
            Field::OllamaMaxContext => &self.ollama_max_context,
            Field::AnthropicApiKey => &self.anthropic_api_key,
            Field::AnthropicModel => &self.anthropic_model,
            Field::AnthropicMaxContext => &self.anthropic_max_context,
            Field::ZaiApiKey => &self.zai_api_key,
            Field::ZaiModel => &self.zai_model,
            Field::ZaiMaxContext => &self.zai_max_context,
            Field::ZaiBaseUrl => &self.zai_base_url,
            Field::EmbeddingEnabled => &self.embedding_enabled,
            Field::EmbedProvider => &self.embed_provider,
            Field::EmbedModel => &self.embed_model,
            Field::EmbedDimensions => &self.embed_dimensions,
        }
    }

    fn display_value(&self, field: Field) -> String {
        let raw = self.field_value(field);
        match field {
            Field::AnthropicApiKey | Field::ZaiApiKey if raw.len() > 8 => {
                format!("{}...{}", &raw[..4], &raw[raw.len() - 4..])
            }
            Field::AnthropicApiKey | Field::ZaiApiKey if raw.is_empty() => "(not set)".to_string(),
            _ => raw.to_string(),
        }
    }

    /// Build an ObiConfig from current field values.
    pub fn to_config(&self) -> ObiConfig {
        let mut config = ObiConfig::default();
        config.brain.embedding_enabled = self.embedding_enabled == "true";
        config.providers.completion = self.llm_provider.clone();
        config.providers.embedding = self.embed_provider.clone();
        config.providers.ollama = OllamaConfig {
            host: self.ollama_host.clone(),
            completion: CompletionModelConfig {
                model: self.ollama_model.clone(),
                max_context: self.ollama_max_context.parse().unwrap_or(32768),
            },
            embedding: EmbeddingModelConfig {
                model: self.embed_model.clone(),
                dimensions: self.embed_dimensions.parse().unwrap_or(768),
            },
        };
        config.providers.anthropic = AnthropicConfig {
            api_key: self.anthropic_api_key.clone(),
            completion: CompletionModelConfig {
                model: self.anthropic_model.clone(),
                max_context: self.anthropic_max_context.parse().unwrap_or(200000),
            },
        };
        config.providers.zai = ZaiConfig {
            api_key: self.zai_api_key.clone(),
            base_url: self.zai_base_url.clone(),
            completion: CompletionModelConfig {
                model: self.zai_model.clone(),
                max_context: self.zai_max_context.parse().unwrap_or(200000),
            },
        };
        config
    }

    fn save_config(&mut self) {
        let config = self.to_config();
        let config_dir = obi_core::config::global_obi_dir();
        let config_path = config_dir.join("config.toml");

        if std::fs::create_dir_all(&config_dir).is_err() {
            self.status_msg = Some("Failed to create ~/.obi/");
            return;
        }

        match toml::to_string_pretty(&config) {
            Ok(content) => {
                if std::fs::write(&config_path, content).is_ok() {
                    self.status_msg = Some("Saved! Config applied.");
                } else {
                    self.status_msg = Some("Failed to write config");
                }
            }
            Err(_) => {
                self.status_msg = Some("Failed to serialize config");
            }
        }
    }

    // ─── Rendering ──────────────────────────────────────────────

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        if !self.visible {
            return;
        }

        let width = 58.min(area.width.saturating_sub(4));
        let height = 22.min(area.height.saturating_sub(2));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let popup = Rect::new(x, y, width, height);

        frame.render_widget(Clear, popup);

        let block = Block::default()
            .title(" Settings ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(CLR_SECTION));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let fields = self.visible_fields();
        let mut lines: Vec<Line> = Vec::new();
        let mut prev_section = "";

        for (i, &field) in fields.iter().enumerate() {
            let section = field.section();
            if section != prev_section {
                if !prev_section.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    format!(" {section}"),
                    Style::default().fg(CLR_SECTION).add_modifier(Modifier::BOLD),
                )));
                prev_section = section;
            }

            let is_selected = i == self.selected;
            let is_editing = is_selected && self.editing;
            let has_dropdown = field.dropdown_options().is_some() || field == Field::AnthropicModel || field == Field::ZaiModel;

            let label = Span::styled(
                format!("   {:<14}", field.label()),
                Style::default().fg(CLR_LABEL),
            );

            let (value_text, value_color) = if is_editing {
                (format!("{}\u{2588}", self.edit_buffer), CLR_EDITING)
            } else if has_dropdown {
                let display = self.display_value(field);
                (format!("{display}  \u{25BE}"), CLR_VALUE)
            } else {
                (self.display_value(field), CLR_VALUE)
            };

            let value = Span::styled(value_text, Style::default().fg(value_color));

            let mut line = Line::from(vec![label, value]);
            if is_selected {
                line = line.style(Style::default().bg(CLR_SELECTED_BG));
            }
            lines.push(line);
        }

        // Status message
        if let Some(msg) = self.status_msg {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("  {msg}"),
                Style::default().fg(CLR_SUCCESS).add_modifier(Modifier::BOLD),
            )));
        }

        // Pad to push hints to bottom
        while lines.len() < inner.height.saturating_sub(1) as usize {
            lines.push(Line::from(""));
        }

        let hint = if self.dropdown.is_some() {
            " j/k: select  Enter: confirm  Esc: cancel"
        } else if self.editing {
            " Enter: confirm  Esc: cancel"
        } else {
            " j/k: navigate  Enter: edit  s: save  Esc: close"
        };
        lines.push(Line::from(Span::styled(hint, Style::default().fg(CLR_HINT))));

        frame.render_widget(Paragraph::new(lines), inner);

        // ─── Dropdown overlay (rendered on top) ─────────────────
        if let Some(ref dd) = self.dropdown {
            if dd.loading {
                // Loading indicator
                let dd_width = 22u16;
                let dd_x = inner.x + 17;
                let dd_y = inner.y + dd.field_row + 1;
                let dd_rect = Rect::new(
                    dd_x.min(inner.x + inner.width - dd_width),
                    dd_y,
                    dd_width.min(inner.width),
                    3,
                );
                frame.render_widget(Clear, dd_rect);
                let dd_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CLR_DROPDOWN_BORDER))
                    .style(Style::default().bg(CLR_DROPDOWN_BG));
                let dd_inner = dd_block.inner(dd_rect);
                frame.render_widget(dd_block, dd_rect);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "  Loading...",
                        Style::default().fg(CLR_HINT),
                    ))),
                    dd_inner,
                );
            } else if !dd.options.is_empty() {
                let max_opt_len = dd.options.iter().map(|o| o.len()).max().unwrap_or(10);
                let dd_width = (max_opt_len as u16 + 6).max(22).min(inner.width);
                let dd_x = inner.x + 17;
                let dd_y = inner.y + dd.field_row + 1;
                // Cap visible rows to avoid overflowing the popup
                let max_visible = (inner.height.saturating_sub(dd.field_row + 2)).max(3) as usize;
                let visible_count = dd.options.len().min(max_visible);
                let dd_height = visible_count as u16 + 2;

                let dd_rect = Rect::new(
                    dd_x.min(inner.x + inner.width - dd_width),
                    dd_y.min(inner.y + inner.height - dd_height),
                    dd_width.min(inner.width),
                    dd_height.min(inner.height),
                );

                frame.render_widget(Clear, dd_rect);

                let dd_block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(CLR_DROPDOWN_BORDER))
                    .style(Style::default().bg(CLR_DROPDOWN_BG));
                let dd_inner = dd_block.inner(dd_rect);
                frame.render_widget(dd_block, dd_rect);

                // Scroll the list so cursor is always visible
                let scroll_offset = if dd.cursor >= visible_count {
                    dd.cursor - visible_count + 1
                } else {
                    0
                };

                let mut dd_lines: Vec<Line> = Vec::new();
                for i in scroll_offset..scroll_offset + visible_count {
                    if i >= dd.options.len() { break; }
                    let option = &dd.options[i];
                    let is_hover = i == dd.cursor;
                    let marker = if is_hover { " \u{25B8} " } else { "   " };
                    let text_color = if is_hover { CLR_DROPDOWN_ACTIVE } else { CLR_DROPDOWN_TEXT };
                    let bg = if is_hover { CLR_DROPDOWN_HOVER } else { CLR_DROPDOWN_BG };

                    dd_lines.push(Line::from(Span::styled(
                        format!("{marker}{option}"),
                        Style::default().fg(text_color).bg(bg),
                    )));
                }

                frame.render_widget(Paragraph::new(dd_lines), dd_inner);
            }
        }
    }
}
