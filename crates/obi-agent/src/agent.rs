use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;

use obi_core::config::ObiConfig;
use obi_core::graph::KnowledgeGraph;
use obi_core::node::NodeId;
use obi_llm::factory::build_router;
use obi_llm::provider::{CompletionProvider, CompletionRequest, Message};
use obi_llm::router::LlmRouter;
use obi_llm::stream::StreamEvent;

use crate::brain::DualBrain;
use crate::context::{ContextBuilder, ContextWindow};
use crate::conversation::ConversationHistory;
use crate::executor::{ExecutionOutcome, Executor};
use crate::planner::Planner;
use crate::tools::ToolRegistry;

/// Events emitted by the agent back to the TUI.
#[derive(Debug)]
pub enum AgentEvent {
    /// Partial text from LLM streaming.
    StreamChunk(String),
    /// Partial thinking/reasoning text from LLM streaming (rendered differently in UI).
    ThinkingChunk(String),
    /// Agent has finished a complete response.
    ResponseComplete(String),
    /// Agent wants to execute a tool that needs user confirmation.
    ToolConfirmation {
        tool_name: String,
        description: String,
        args_display: String,
        call_id: String,
    },
    /// A plan was generated.
    PlanGenerated(String),
    /// Context building started.
    ContextBuilding,
    /// Context was built.
    ContextBuilt(ContextWindow),
    /// Model info (provider/model names) for display.
    ModelInfo {
        provider: String,
        chat_model: String,
        embed_model: String,
    },
    /// Error occurred.
    Error(String),
}

/// Commands sent from the TUI to the agent.
#[derive(Debug)]
pub enum AgentCommand {
    /// User submitted a query.
    Query(String),
    /// User approved a pending tool call.
    ApproveToolCall(String),
    /// User denied a pending tool call.
    DenyToolCall(String),
    /// Update pinned/excluded node sets from graph widget.
    UpdateContextSets {
        pinned: HashSet<NodeId>,
        excluded: HashSet<NodeId>,
    },
    /// Hot-reload: rebuild router and brain from updated config.
    ReloadConfig,
}

/// The AI Agent — main loop that orchestrates planner, tools, LLM, and context.
///
/// Runs as an async task, communicating with the TUI via channels.
/// Uses DualBrain (Phase 6) for dual-scope context building.
pub struct Agent {
    #[allow(dead_code)]
    project_root: PathBuf,
    dual_brain: DualBrain,
    router: LlmRouter,
    tool_registry: ToolRegistry,
    conversation: ConversationHistory,
    pinned: HashSet<NodeId>,
    excluded: HashSet<NodeId>,
    /// Max tokens per node — truncate large functions (from brain.max_node_tokens).
    max_node_tokens: usize,
    /// Tool calls awaiting user confirmation (queued sequentially).
    pending_confirmations: std::collections::VecDeque<obi_llm::provider::ToolCall>,
}

/// System prompt for the agent.
const AGENT_SYSTEM: &str = r#"You are Obi-Wan, an AI coding assistant embedded in a TUI IDE. You have access to the project's knowledge graph and can read, edit, search files, run commands, and query the code graph.

When helping the user:
1. Use the provided code context to understand the codebase
2. Use tools to gather more information or make changes
3. Be precise and concise in your responses
4. When editing code, make minimal, targeted changes
5. Always explain what you're doing and why

Available tools:
- read: Read file contents (path, optional start_line/end_line)
- edit: Edit a file (path, old_string, new_string — old_string must be unique)
- search: Search for patterns in files (pattern, optional file_extension, max_results)
- run: Execute shell commands (command — requires user approval)
- graph_query: Query the knowledge graph (operation: neighbors/info/stats)"#;

impl Agent {
    pub fn new(
        project_root: PathBuf,
        knowledge_graph: Option<Arc<KnowledgeGraph>>,
    ) -> Self {
        // Load config (merges global + project) and build provider from it
        let config = ObiConfig::load(&project_root);
        let router = build_router(&config);

        // Load dual brain (project + global)
        let dual_brain = DualBrain::load(&project_root, &config);

        // Use the merged graph from dual brain for tool registry, or fall back
        // to the provided knowledge_graph (backward compat with TUI startup)
        let graph_for_tools = if dual_brain.has_any_brain() {
            Some(dual_brain.merged_graph_arc())
        } else {
            knowledge_graph
        };

        let tool_registry = ToolRegistry::new(project_root.clone(), graph_for_tools);
        let max_node_tokens = config.brain.max_node_tokens;

        Self {
            project_root,
            dual_brain,
            router,
            tool_registry,
            conversation: ConversationHistory::new(6000),
            pinned: HashSet::new(),
            excluded: HashSet::new(),
            max_node_tokens,
            pending_confirmations: std::collections::VecDeque::new(),
        }
    }

    /// Update pinned/excluded sets (called from TUI when graph widget changes).
    pub fn update_context_sets(&mut self, pinned: HashSet<NodeId>, excluded: HashSet<NodeId>) {
        self.pinned = pinned;
        self.excluded = excluded;
    }

    /// Hot-reload: re-read config from disk and rebuild the LLM router.
    pub fn reload_config(&mut self) {
        let config = ObiConfig::load(&self.project_root);
        self.router = build_router(&config);
        self.dual_brain = DualBrain::load(&self.project_root, &config);
        self.max_node_tokens = config.brain.max_node_tokens;
    }

    /// Process a user query — builds context, plans, executes, and streams response.
    /// Returns events via the provided sender.
    pub async fn process_query(
        &mut self,
        query: String,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<()> {
        // Clear any stale pending confirmations from previous query
        self.pending_confirmations.clear();

        // Step 1: Build context (dual-brain)
        let _ = event_tx.send(AgentEvent::ContextBuilding);
        let t0 = std::time::Instant::now();
        let context_window = self.build_context(&query).await;
        let ctx_ms = t0.elapsed().as_millis();
        let context_text = match &context_window {
            Ok(cw) => {
                let _ = event_tx.send(AgentEvent::ContextBuilt(cw.clone()));
                cw.formatted.clone()
            }
            Err(e) => {
                let _ = event_tx.send(AgentEvent::Error(format!("Context ({ctx_ms}ms): {e}")));
                String::new()
            }
        };

        // Step 2: Plan (skip for short/simple queries to save an LLM round-trip)
        let word_count = query.split_whitespace().count();
        if word_count > 5 {
            let t1 = std::time::Instant::now();
            if let Ok(plan) = Planner::plan(&self.router, &query, &context_text).await {
                let plan_ms = t1.elapsed().as_millis();
                let _ = event_tx.send(AgentEvent::PlanGenerated(format!(
                    "{} (ctx: {}ms, plan: {}ms)",
                    plan.summary(), ctx_ms, plan_ms
                )));
            }
        }

        // Step 3: Build the completion request with context + conversation
        let system_prompt = if context_text.is_empty() {
            AGENT_SYSTEM.to_string()
        } else {
            format!(
                "{}\n\n--- Code Context ---\n{}",
                AGENT_SYSTEM, context_text
            )
        };

        self.conversation.push(Message::user(&query));

        let req = CompletionRequest {
            system: Some(system_prompt),
            messages: self.conversation.messages().to_vec(),
            max_tokens: 4096,
            tools: self.tool_registry.definitions(),
            temperature: 0.1,
        };

        // Step 4: Stream the response
        let mut stream = self.router.complete_stream(req).await?;
        let mut full_response = String::new();
        let mut pending_tool_calls = Vec::new();

        while let Some(event) = stream.receiver.recv().await {
            match event {
                StreamEvent::TextDelta(text) => {
                    full_response.push_str(&text);
                    let _ = event_tx.send(AgentEvent::StreamChunk(text));
                }
                StreamEvent::ThinkingDelta(text) => {
                    let _ = event_tx.send(AgentEvent::ThinkingChunk(text));
                }
                StreamEvent::ToolCallStart { name, id } => {
                    let _ = event_tx.send(AgentEvent::StreamChunk(format!(
                        "\n[Using tool: {}]\n",
                        name
                    )));
                    pending_tool_calls.push(obi_llm::provider::ToolCall {
                        id: id.clone(),
                        name,
                        arguments: String::new(),
                    });
                }
                StreamEvent::ToolCallDelta(args) => {
                    if let Some(last) = pending_tool_calls.last_mut() {
                        last.arguments.push_str(&args);
                    }
                }
                StreamEvent::Done(response) => {
                    if response.has_tool_use && !response.tool_calls.is_empty() {
                        pending_tool_calls = response.tool_calls;
                    }
                    break;
                }
                StreamEvent::Error(err) => {
                    let _ = event_tx.send(AgentEvent::Error(err));
                    return Ok(());
                }
            }
        }

        // Step 5: Execute tool calls if any
        if !pending_tool_calls.is_empty() {
            // Save assistant message with tool calls to conversation first
            let mut assistant_msg = Message::assistant(&full_response);
            assistant_msg.tool_calls = pending_tool_calls.clone();
            self.conversation.push(assistant_msg);

            // Execute ReadOnly tools immediately, queue Mutating/Dangerous for confirmation
            for call in &pending_tool_calls {
                let outcome =
                    Executor::execute(&self.tool_registry, call, false).await?;

                match outcome {
                    ExecutionOutcome::Completed(result) => {
                        let _ = event_tx.send(AgentEvent::StreamChunk(format!(
                            "\n[Tool '{}' result: {}]\n",
                            call.name,
                            if result.output.len() > 200 {
                                format!("{}...", &result.output[..200])
                            } else {
                                result.output.clone()
                            }
                        )));

                        self.conversation.push(Message::tool_result(
                            call.id.clone(),
                            &result.output,
                        ));
                    }
                    ExecutionOutcome::NeedsConfirmation { call: pending_call, .. } => {
                        self.pending_confirmations.push_back(pending_call);
                    }
                    ExecutionOutcome::NotFound(msg) => {
                        let _ = event_tx.send(AgentEvent::StreamChunk(format!(
                            "\n[Error: {}]\n",
                            msg
                        )));
                    }
                }
            }

            // Send the first pending confirmation to TUI (queue the rest)
            self.send_next_confirmation(event_tx);
        } else {
            self.conversation
                .push(Message::assistant(&full_response));
            let _ = event_tx.send(AgentEvent::ResponseComplete(full_response));
        }

        Ok(())
    }

    /// Send the next pending confirmation to TUI, if any.
    fn send_next_confirmation(&mut self, event_tx: &mpsc::UnboundedSender<AgentEvent>) {
        if let Some(call) = self.pending_confirmations.front() {
            let args: serde_json::Value = serde_json::from_str(&call.arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            let args_display = if call.name == "run" {
                args.get("command")
                    .and_then(|v| v.as_str())
                    .map(|cmd| format!("$ {cmd}"))
                    .unwrap_or_else(|| args.to_string())
            } else {
                args.get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| format!("{} {p}", call.name))
                    .unwrap_or_else(|| args.to_string())
            };

            let _ = event_tx.send(AgentEvent::ToolConfirmation {
                tool_name: call.name.clone(),
                description: format!("AI wants to execute: {args_display}"),
                args_display,
                call_id: call.id.clone(),
            });
        }
    }

    /// Execute a previously-confirmed tool call, then process remaining queue or stream LLM response.
    pub async fn execute_approved_tool(
        &mut self,
        call_id: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<()> {
        // Pop the confirmed call from the front of the queue
        let call = match self.pending_confirmations.pop_front() {
            Some(c) if c.id == call_id => c,
            Some(c) => {
                // Wrong order — put it back and search
                self.pending_confirmations.push_front(c);
                match self.pending_confirmations.iter().position(|tc| tc.id == call_id) {
                    Some(idx) => self.pending_confirmations.remove(idx).unwrap(),
                    None => {
                        let _ = event_tx.send(AgentEvent::Error(format!(
                            "Tool call '{call_id}' not found in pending queue"
                        )));
                        return Ok(());
                    }
                }
            }
            None => {
                // Fallback: search conversation history (backward compat)
                let found = self
                    .conversation
                    .messages()
                    .iter()
                    .rev()
                    .flat_map(|m| m.tool_calls.iter())
                    .find(|tc| tc.id == call_id)
                    .cloned();

                match found {
                    Some(c) => c,
                    None => {
                        let _ = event_tx.send(AgentEvent::Error(format!(
                            "Tool call '{call_id}' not found in history"
                        )));
                        return Ok(());
                    }
                }
            }
        };

        let outcome = Executor::execute(&self.tool_registry, &call, true).await?;

        match outcome {
            ExecutionOutcome::Completed(result) => {
                let _ = event_tx.send(AgentEvent::StreamChunk(format!(
                    "\n[Tool '{}' result: {}]\n",
                    call.name,
                    if result.output.len() > 200 {
                        format!("{}...", &result.output[..200])
                    } else {
                        result.output.clone()
                    }
                )));

                self.conversation
                    .push(Message::tool_result(call.id, &result.output));

                // If more confirmations are queued, send the next one
                if !self.pending_confirmations.is_empty() {
                    self.send_next_confirmation(event_tx);
                    return Ok(());
                }

                // All tool calls processed — stream follow-up LLM response
                let req = CompletionRequest {
                    system: Some(AGENT_SYSTEM.to_string()),
                    messages: self.conversation.messages().to_vec(),
                    max_tokens: 4096,
                    tools: self.tool_registry.definitions(),
                    temperature: 0.1,
                };

                let mut stream = self.router.complete_stream(req).await?;
                let mut full_response = String::new();

                while let Some(event) = stream.receiver.recv().await {
                    match event {
                        StreamEvent::TextDelta(text) => {
                            full_response.push_str(&text);
                            let _ = event_tx.send(AgentEvent::StreamChunk(text));
                        }
                        StreamEvent::ThinkingDelta(text) => {
                            let _ = event_tx.send(AgentEvent::ThinkingChunk(text));
                        }
                        StreamEvent::Done(_response) => {
                            break;
                        }
                        StreamEvent::Error(err) => {
                            let _ = event_tx.send(AgentEvent::Error(err));
                            return Ok(());
                        }
                        _ => {}
                    }
                }

                self.conversation
                    .push(Message::assistant(&full_response));
                let _ = event_tx.send(AgentEvent::ResponseComplete(full_response));
            }
            _ => {
                let _ = event_tx.send(AgentEvent::Error(
                    "Unexpected outcome for approved tool".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Build context using the dual-brain system.
    async fn build_context(&self, query: &str) -> Result<ContextWindow> {
        if !self.dual_brain.has_any_brain() {
            anyhow::bail!("No brain available. Run `obi index` first.");
        }

        let provider_max = self.router.max_context_tokens();
        let system_tokens = crate::token::estimate_tokens(AGENT_SYSTEM);
        let conv_tokens = self.conversation.token_count();
        let response_reserve = 4096;
        let context_budget = provider_max
            .saturating_sub(system_tokens)
            .saturating_sub(conv_tokens)
            .saturating_sub(response_reserve);

        let context_budget = context_budget.max(1000);

        ContextBuilder::build_dual(
            query,
            context_budget,
            &self.dual_brain,
            &self.router,
            &self.pinned,
            &self.excluded,
            self.max_node_tokens,
        )
        .await
    }
}

/// Spawn the agent as an async task. Returns channels for communication.
pub fn spawn_agent(
    project_root: PathBuf,
    knowledge_graph: Option<Arc<KnowledgeGraph>>,
) -> (
    mpsc::UnboundedSender<AgentCommand>,
    mpsc::UnboundedReceiver<AgentEvent>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<AgentCommand>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();

    tokio::spawn(async move {
        let mut agent = Agent::new(project_root, knowledge_graph);

        // Send model info to TUI on startup
        let _ = event_tx.send(AgentEvent::ModelInfo {
            provider: agent.router.completion_name().to_string(),
            chat_model: agent.router.completion_model().to_string(),
            embed_model: agent.router.embedding_model().to_string(),
        });

        while let Some(cmd) = cmd_rx.recv().await {
            match cmd {
                AgentCommand::Query(query) => {
                    if let Err(e) = agent.process_query(query, &event_tx).await {
                        let _ = event_tx.send(AgentEvent::Error(e.to_string()));
                    }
                }
                AgentCommand::ApproveToolCall(call_id) => {
                    if let Err(e) = agent.execute_approved_tool(&call_id, &event_tx).await {
                        let err_msg = format!("Tool execution error: {e:#}");
                        let _ = event_tx.send(AgentEvent::Error(err_msg));
                    }
                }
                AgentCommand::DenyToolCall(call_id) => {
                    // Remove denied call from queue
                    agent.pending_confirmations.retain(|tc| tc.id != call_id);
                    let _ = event_tx.send(AgentEvent::StreamChunk(format!(
                        "\n[Tool call '{}' denied by user]\n",
                        call_id
                    )));
                    // Send next confirmation if any remain, otherwise finish
                    if !agent.pending_confirmations.is_empty() {
                        agent.send_next_confirmation(&event_tx);
                    }
                }
                AgentCommand::UpdateContextSets { pinned, excluded } => {
                    agent.update_context_sets(pinned, excluded);
                }
                AgentCommand::ReloadConfig => {
                    agent.reload_config();
                    let _ = event_tx.send(AgentEvent::ModelInfo {
                        provider: agent.router.completion_name().to_string(),
                        chat_model: agent.router.completion_model().to_string(),
                        embed_model: agent.router.embedding_model().to_string(),
                    });
                }
            }
        }
    });

    (cmd_tx, event_rx)
}
