//! Domain state types for the TUI application
//!
//! This module contains the core state types that represent the application's
//! domain model. Conversation events match the API protocol format.

use tokio::sync::watch;

/// Streaming status indicators from server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingStatus {
    Processing,
    Searching,
    Thinking,
    WaitingForTools,
}

impl StreamingStatus {
    pub fn from_status_str(s: &str) -> Self {
        match s {
            "processing" => Self::Processing,
            "searching" => Self::Searching,
            "waiting_for_tools" => Self::WaitingForTools,
            _ => Self::Thinking,
        }
    }

    pub fn display_text(&self) -> &'static str {
        match self {
            Self::Processing => "Processing...",
            Self::Searching => "Searching...",
            Self::Thinking => "Thinking...",
            Self::WaitingForTools => "Waiting for tools...",
        }
    }
}

/// Conversation event types matching the API protocol
#[derive(Debug, Clone)]
pub enum ConversationEvent {
    /// User message (what the user typed)
    UserMessage { content: String },
    /// Text content from assistant (streamed or complete)
    Text { content: String },
    /// Tool call from assistant
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (usually from server-side execution)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl ConversationEvent {
    /// Convert to JSON for API calls
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            ConversationEvent::UserMessage { content } => serde_json::json!({
                "type": "user_message",
                "content": content
            }),
            ConversationEvent::Text { content } => serde_json::json!({
                "type": "text",
                "content": content
            }),
            ConversationEvent::ToolCall { id, name, input } => serde_json::json!({
                "type": "tool_call",
                "id": id,
                "name": name,
                "input": input
            }),
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error
            }),
        }
    }

    /// Extract command from a suggest_command tool call
    pub fn as_command(&self) -> Option<&str> {
        if let ConversationEvent::ToolCall { name, input, .. } = self
            && name == "suggest_command"
        {
            return input.get("command").and_then(|v| v.as_str());
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    /// User is typing input
    Input,
    /// Waiting for generation (showing spinner)
    Generating,
    /// Streaming SSE response
    Streaming,
    /// Reviewing generated command
    Review,
    /// Error state, can retry
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitAction {
    /// Run the command
    Execute(String),
    /// Insert command without running
    Insert(String),
    /// User canceled
    Cancel,
}

/// Application state — the domain model
///
/// Conversation is stored as a sequence of events matching the API protocol.
/// The view function derives the UI from this state.
#[derive(Debug)]
pub struct AppState {
    /// Current application mode
    pub mode: AppMode,
    /// Conversation events (source of truth, matches API protocol)
    pub events: Vec<ConversationEvent>,
    /// Text being streamed (accumulated, flushed to Text event on completion)
    pub streaming_text: String,
    /// Receiver for current input text (sent by InputBox component)
    pub input_rx: watch::Receiver<String>,
    /// Sender for input text (cloned into InputBox component as a prop)
    pub input_tx: watch::Sender<String>,
    /// Current error message
    pub error: Option<String>,
    /// Exit action (set when exiting)
    pub exit_action: Option<ExitAction>,
    /// Session ID from server
    pub session_id: Option<String>,
    /// Current streaming status
    pub streaming_status: Option<StreamingStatus>,
    /// Whether current turn was interrupted by user
    pub was_interrupted: bool,
    /// True when user has pressed Enter once on a dangerous command
    pub confirmation_pending: bool,
    /// True when the app is about to exit (hides the input box on final render)
    pub exiting: bool,
}

impl AppState {
    pub fn new() -> Self {
        let (input_tx, input_rx) = watch::channel(String::new());
        Self {
            mode: AppMode::Input,
            events: Vec::new(),
            streaming_text: String::new(),
            input_rx,
            input_tx,
            error: None,
            exit_action: None,
            session_id: None,
            streaming_status: None,
            was_interrupted: false,
            confirmation_pending: false,
            exiting: false,
        }
    }

    /// Get the submitted input text (sent by InputBox on Enter)
    pub fn input(&self) -> String {
        self.input_rx.borrow().clone()
    }

    /// Check if the submitted input is empty
    pub fn input_is_empty(&self) -> bool {
        self.input_rx.borrow().trim().is_empty()
    }

    /// Reset the input channel (after consuming the submitted text)
    pub fn clear_input(&mut self) {
        let _ = self.input_tx.send(String::new());
    }

    /// Convert conversation events to Claude API message format
    pub fn events_to_messages(&self) -> Vec<serde_json::Value> {
        let mut messages = Vec::new();
        let mut i = 0;
        let events = &self.events;

        while i < events.len() {
            match &events[i] {
                ConversationEvent::UserMessage { content } => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": content
                    }));
                    i += 1;
                }
                ConversationEvent::Text { content } => {
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": content
                    }));
                    i += 1;
                }
                ConversationEvent::ToolCall { .. } => {
                    let mut tool_uses = Vec::new();
                    while i < events.len() {
                        if let ConversationEvent::ToolCall { id, name, input } = &events[i] {
                            tool_uses.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input
                            }));
                            i += 1;
                        } else {
                            break;
                        }
                    }
                    messages.push(serde_json::json!({
                        "role": "assistant",
                        "content": tool_uses
                    }));
                }
                ConversationEvent::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    messages.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error
                        }]
                    }));
                    i += 1;
                }
            }
        }

        messages
    }

    // ===== Generation lifecycle methods =====

    /// Start generating from current input
    pub fn start_generating(&mut self) {
        self.events.push(ConversationEvent::UserMessage {
            content: self.input(),
        });
        self.clear_input();
        self.mode = AppMode::Generating;
    }

    /// Generation error occurred
    pub fn generation_error(&mut self, error: String) {
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    /// Cancel during generation
    pub fn cancel_generation(&mut self) {
        if let Some(ConversationEvent::UserMessage { .. }) = self.events.last() {
            self.events.pop();
        }
        self.mode = AppMode::Input;
        self.clear_input();
    }

    // ===== Streaming lifecycle methods =====

    /// Start streaming response
    pub fn start_streaming(&mut self) {
        self.streaming_text.clear();
        self.streaming_status = None;
        self.was_interrupted = false;
        self.mode = AppMode::Streaming;
    }

    /// Store session ID from server response
    pub fn store_session_id(&mut self, session_id: String) {
        self.session_id = Some(session_id);
    }

    /// Update streaming status from SSE event
    pub fn update_streaming_status(&mut self, status: &str) {
        self.streaming_status = Some(StreamingStatus::from_status_str(status));
    }

    /// Cancel streaming with context preservation
    pub fn cancel_streaming(&mut self) {
        self.was_interrupted = true;

        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            let interrupted_text = format!("{trimmed}\n\n[User cancelled this generation]");
            self.events.push(ConversationEvent::Text {
                content: interrupted_text,
            });
        }

        self.streaming_status = None;
        self.confirmation_pending = false;
        self.mode = AppMode::Input;
    }

    /// Append text chunk during streaming
    pub fn append_streaming_text(&mut self, chunk: &str) {
        if self.streaming_text.is_empty() {
            let trimmed = chunk.trim_start();
            if !trimmed.is_empty() {
                self.streaming_text.push_str(trimmed);
            }
        } else {
            self.streaming_text.push_str(chunk);
        }
    }

    /// Add a tool call event during streaming
    pub fn add_tool_call(&mut self, id: String, name: String, input: serde_json::Value) {
        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            self.events.push(ConversationEvent::Text {
                content: trimmed.to_string(),
            });
        }

        let is_suggest_command = name == "suggest_command";
        self.events
            .push(ConversationEvent::ToolCall { id, name, input });

        if is_suggest_command {
            self.streaming_status = None;
            self.mode = AppMode::Review;
        }
    }

    /// Add a tool result event during streaming
    pub fn add_tool_result(&mut self, tool_use_id: String, content: String, is_error: bool) {
        self.events.push(ConversationEvent::ToolResult {
            tool_use_id,
            content,
            is_error,
        });
    }

    /// Finalize streaming — flush accumulated text to event
    pub fn finalize_streaming(&mut self) {
        let content = std::mem::take(&mut self.streaming_text);
        let trimmed = content.trim_start();
        if !trimmed.is_empty() {
            self.events.push(ConversationEvent::Text {
                content: trimmed.to_string(),
            });
        }
        self.streaming_status = None;
        self.mode = AppMode::Review;
    }

    /// Streaming error
    pub fn streaming_error(&mut self, error: String) {
        self.streaming_text.clear();
        self.error = Some(error);
        self.mode = AppMode::Error;
    }

    // ===== Edit mode and exit methods =====

    /// Start edit mode for refinement
    pub fn start_edit_mode(&mut self) {
        self.confirmation_pending = false;
        self.clear_input();
        self.mode = AppMode::Input;
    }

    /// Retry after error
    pub fn retry(&mut self) {
        self.error = None;
        self.mode = AppMode::Generating;
    }

    // ===== Query methods =====

    /// Get the most recent command from events
    pub fn current_command(&self) -> Option<&str> {
        self.events.iter().rev().find_map(|e| e.as_command())
    }

    /// Check if the most recent command is marked dangerous
    pub fn is_current_command_dangerous(&self) -> bool {
        self.events
            .iter()
            .rev()
            .find_map(|e| {
                if let ConversationEvent::ToolCall { name, input, .. } = e
                    && name == "suggest_command"
                {
                    let danger_level = input
                        .get("danger")
                        .and_then(|v| v.as_str())
                        .unwrap_or("low");
                    return Some(
                        danger_level == "high" || danger_level == "medium" || danger_level == "med",
                    );
                }
                None
            })
            .unwrap_or(false)
    }

    /// Count non-suggest_command tool calls since the last user message
    pub fn tool_count_since_last_user(&self) -> usize {
        let last_user_idx = self
            .events
            .iter()
            .rposition(|e| matches!(e, ConversationEvent::UserMessage { .. }))
            .unwrap_or(0);

        let mut completed = 0;
        let mut in_flight = false;

        for event in &self.events[last_user_idx..] {
            match event {
                ConversationEvent::ToolCall { name, .. } if name != "suggest_command" => {
                    if in_flight {
                        completed += 1;
                    }
                    in_flight = true;
                }
                ConversationEvent::ToolResult { .. } => {
                    if in_flight {
                        completed += 1;
                        in_flight = false;
                    }
                }
                _ => {}
            }
        }

        completed
    }

    /// Check if any turn in the conversation has a command
    pub fn has_any_command(&self) -> bool {
        self.events.iter().any(|e| {
            if let ConversationEvent::ToolCall { name, input, .. } = e {
                name == "suggest_command" && input.get("command").and_then(|v| v.as_str()).is_some()
            } else {
                false
            }
        })
    }

    /// Get the footer text for current mode
    pub fn footer_text(&self) -> &'static str {
        match self.mode {
            AppMode::Input => "[Enter] Send  [Esc] Exit",
            AppMode::Generating | AppMode::Streaming => "[Esc] Cancel",
            AppMode::Review => {
                if self.confirmation_pending {
                    "[Enter] Confirm dangerous command  [Esc] Cancel"
                } else if self.has_any_command() {
                    "[Enter] Execute  [Tab] Insert  [f] Follow-up  [Esc] Exit"
                } else {
                    "[f] Follow-up  [Esc] Exit"
                }
            }
            AppMode::Error => "[Enter]/[r] Retry  [Esc] Exit",
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
