//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    Component, Elements, Line, Span, Spinner, TextBlock, VStack, element, impl_slot_children,
};
use ratatui_core::style::{Color, Modifier, Style};

use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::state::{AppMode, AppState, ConversationEvent};

#[derive(Debug)]
enum UiEvent {
    Text { content: String },
    ToolCall(ToolCallDetails),
    ToolSummary(ToolSummary),
    SuggestedCommand(SuggestedCommandDetails),
}

#[derive(Debug)]
struct ToolCallDetails {
    tool_use_id: String,
    name: String,
    input: serde_json::Value,
    status: ToolResultStatus,
}

#[derive(Debug)]
struct SuggestedCommandDetails {
    command: String,
    danger_level: String,
    danger_notes: String,
    confidence_level: String,
    confidence_notes: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ToolResultStatus {
    Pending,
    Success,
    Error,
}

#[derive(Debug)]
enum UiTurn {
    UserTurn { events: Vec<UiEvent> },
    AgentTurn { events: Vec<UiEvent> },
}

struct TurnBuilder {
    turns: Vec<UiTurn>,
    current_turn: Option<UiTurn>,
}

impl TurnBuilder {
    fn new() -> Self {
        Self {
            turns: Vec::new(),
            current_turn: None,
        }
    }

    fn commit_turn(&mut self) {
        if let Some(turn) = self.current_turn.take() {
            self.turns.push(turn);
        }
    }

    fn start_user_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::UserTurn { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::UserTurn { events: vec![] });
        }
    }

    fn start_agent_turn(&mut self) {
        if !matches!(self.current_turn, Some(UiTurn::AgentTurn { .. })) {
            self.commit_turn();
            self.current_turn = Some(UiTurn::AgentTurn { events: vec![] });
        }
    }

    fn turn_mut_unsafe(&mut self) -> &mut UiTurn {
        self.current_turn.as_mut().unwrap()
    }

    fn add_event(&mut self, event: &ConversationEvent) {
        match event {
            ConversationEvent::UserMessage { content } => {
                self.add_user_message(content);
            }
            ConversationEvent::Text { content } => {
                self.add_agent_text(content);
            }
            ConversationEvent::ToolCall { id, name, input } => {
                if name == "suggest_command" {
                    self.add_suggested_command(input);
                } else {
                    self.add_tool_call(id, name, input);
                }
            }
            ConversationEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                self.add_tool_result(tool_use_id, content, *is_error);
            }
        }
    }

    fn add_user_message(&mut self, content: &str) {
        self.start_user_turn();
        if let UiTurn::UserTurn { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_agent_text(&mut self, content: &str) {
        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::Text {
                content: content.to_string(),
            });
        }
    }

    fn add_suggested_command(&mut self, input: &serde_json::Value) {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        if command.is_empty() {
            return;
        }

        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::SuggestedCommand(SuggestedCommandDetails {
                command: input
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                danger_level: input
                    .get("danger")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                danger_notes: input
                    .get("danger_notes")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                confidence_level: input
                    .get("confidence")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                confidence_notes: input
                    .get("confidence_notes")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            }));
        }
    }

    fn add_tool_call(&mut self, id: &str, name: &str, input: &serde_json::Value) {
        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::ToolCall(ToolCallDetails {
                tool_use_id: id.to_string(),
                name: name.to_string(),
                input: input.clone(),
                status: ToolResultStatus::Pending,
            }));
        }
    }

    fn add_tool_result(&mut self, tool_use_id: &str, _content: &str, is_error: bool) {
        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            let event = events.iter_mut().find(|e| match e {
                UiEvent::ToolCall(ToolCallDetails {
                    tool_use_id: id, ..
                }) => id == tool_use_id,
                _ => false,
            });
            if let Some(UiEvent::ToolCall(ToolCallDetails { status, .. })) = event {
                *status = if is_error {
                    ToolResultStatus::Error
                } else {
                    ToolResultStatus::Success
                };
            }
        }
    }

    fn build(&mut self) -> Vec<UiTurn> {
        self.commit_turn();

        // Collapse consecutive tool calls within each agent turn into ToolSummary
        for turn in &mut self.turns {
            if let UiTurn::AgentTurn { events } = turn {
                let mut new_events: Vec<UiEvent> = Vec::new();
                let mut pending_tools: Vec<ToolCallDetails> = Vec::new();

                for event in events.drain(..) {
                    match event {
                        UiEvent::ToolCall(details) => {
                            pending_tools.push(details);
                        }
                        other => {
                            if !pending_tools.is_empty() {
                                new_events.push(UiEvent::ToolSummary(ToolSummary {
                                    tool_calls: std::mem::take(&mut pending_tools),
                                }));
                            }
                            new_events.push(other);
                        }
                    }
                }

                if !pending_tools.is_empty() {
                    new_events.push(UiEvent::ToolSummary(ToolSummary {
                        tool_calls: pending_tools,
                    }));
                }

                *events = new_events;
            }
        }

        std::mem::take(&mut self.turns)
    }
}

#[derive(Debug)]
struct ToolSummary {
    tool_calls: Vec<ToolCallDetails>,
}

impl ToolSummary {
    /// Determines the summary line:
    /// - If any call is pending, use present tense verb with `-ing`
    /// - If multiple calls are complete, say "Used n tools"
    /// - If a single call is complete, use past tense verb
    fn summary(&self) -> String {
        if self.any_pending() {
            // Find the last pending tool for the active verb
            if let Some(pending) = self
                .tool_calls
                .iter()
                .rev()
                .find(|t| t.status == ToolResultStatus::Pending)
            {
                return Self::progressive_verb(&pending.name);
            }
        }

        if self.tool_calls.len() == 1 {
            return Self::past_verb(&self.tool_calls[0].name);
        }

        format!("Used {} tools", self.tool_calls.len())
    }

    /// Determines if the spinner should be spinning
    fn any_pending(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|tool_call| tool_call.status == ToolResultStatus::Pending)
    }

    /// Present-tense progressive verb for a tool name (e.g. "Searching...")
    fn progressive_verb(name: &str) -> String {
        match name {
            "search" => "Searching...".into(),
            "read" | "read_file" => "Reading file...".into(),
            "write" | "write_file" => "Writing file...".into(),
            "execute" | "run" | "bash" => "Running command...".into(),
            "list" | "list_files" => "Listing files...".into(),
            _ => format!("Running {}...", name.replace('_', " ")),
        }
    }

    /// Past-tense verb for a tool name (e.g. "Searched")
    fn past_verb(name: &str) -> String {
        match name {
            "search" => "Searched".into(),
            "read" | "read_file" => "Read file".into(),
            "write" | "write_file" => "Wrote file".into(),
            "execute" | "run" | "bash" => "Ran command".into(),
            "list" | "list_files" => "Listed files".into(),
            _ => format!("Ran {}", name.replace('_', " ")),
        }
    }
}

struct Padding {
    top: u16,
    left: u16,
    right: u16,
    bottom: u16,
}

impl Default for Padding {
    fn default() -> Self {
        Self {
            top: 0,
            left: 0,
            right: 0,
            bottom: 0,
        }
    }
}

impl Component for Padding {
    type State = ();

    fn content_inset(&self, _state: &Self::State) -> eye_declare::Insets {
        eye_declare::Insets::ZERO
            .left(self.left)
            .right(self.right)
            .top(self.top)
            .bottom(self.bottom)
    }

    fn desired_height(&self, _width: u16, _state: &Self::State) -> u16 {
        0
    }

    fn render(
        &self,
        _area: ratatui::layout::Rect,
        _buf: &mut ratatui::buffer::Buffer,
        _state: &(),
    ) {
    }
}

impl_slot_children!(Padding);

/// Build the element tree from current state.
///
/// Layout (top to bottom):
/// - Conversation messages (user messages, agent responses, tool status)
/// - Streaming content (if actively streaming)
/// - Error display (if in error state)
/// - Spacer
/// - Input box (bordered, with contextual keybindings)
pub fn ai_view(state: &AppState) -> Elements {
    // println!("{:?}", state.events);

    let mut turn_builder = TurnBuilder::new();

    for event in &state.events {
        turn_builder.add_event(event);
    }
    let turns = turn_builder.build();

    // println!("{:?}", turns);

    let busy = state.mode == AppMode::Streaming || state.mode == AppMode::Generating;
    let last_index = turns.len().saturating_sub(1);

    element! {
        #(for (index, turn) in turns.iter().enumerate() {
            #(match turn {
                UiTurn::UserTurn { events } => {
                    user_turn_view(&events)
                }
                UiTurn::AgentTurn { events } => {
                    agent_turn_view(&events, busy && index == last_index)
                }
            })
        })

        #(if !state.exiting {
            TextBlock { Line { Span(text: "") } }
            InputBox(
                key: "input",
                title: "Generate a command or ask a question",
                title_right: "Atuin AI",
                footer: state.footer_text(),
                active: state.mode == AppMode::Input,
                tx: state.input_tx.clone(),
            )
        })
    }
}

fn user_turn_view(events: &[UiEvent]) -> Elements {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    element! {
        VStack {
            TextBlock {
                Line {
                    Span(text: "You", style: label_style)
                }
            }
            #(for event in events {
                #(match event {
                    UiEvent::Text { content } => {
                        element! {
                            Padding(left: 2u16) {
                                TextBlock {
                                    Line {
                                        Span(text: content, style: Style::default())
                                    }
                                }
                            }
                        }
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn agent_turn_view(events: &[UiEvent], busy: bool) -> Elements {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    element! {
        VStack {
            Spinner(
                label: "Atuin AI",
                done: false,
                label_style: label_style,
                done_label_style: label_style,
                hide_checkmark: true,
                label_first: true,
                done: !busy,
            )
            #(for event in events {
                #(match event {
                    UiEvent::Text { content } => {
                        element! {
                            Padding(left: 2u16) {
                                Markdown(source: content)
                            }
                        }
                    },
                    UiEvent::ToolSummary(summary) => {
                        tool_summary_view(summary)
                    },
                    UiEvent::SuggestedCommand(details) => {
                        suggested_command_view(details)
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn tool_summary_view(summary: &ToolSummary) -> Elements {
    element! {
        Spinner(label: summary.summary(), done: !summary.any_pending())
    }
    // LeftPadded {
    //     TextBlock {
    //         Line {
    //             Span(text: icon, style: icon_style)
    //             Span(text: summary.summary(), style: style)
    //         }
    //     }
    // }
}

fn suggested_command_view(details: &SuggestedCommandDetails) -> Elements {
    element! {
        VStack {
            TextBlock {
                Line {
                    Span(text: "Suggested command:", style: Style::default().fg(Color::Cyan))
                }
                Line {
                    #(if details.danger_level == "high" || details.danger_level == "medium" || details.danger_level == "med" {
                        Span(text: "! ", style: Style::default().fg(Color::Yellow))
                    } else {
                        Span(text: "$ ", style: Style::default().fg(Color::Blue))
                    })
                    Span(text: &details.command, style: Style::default().fg(Color::Green))
                }
            }
            #(if !details.danger_notes.is_empty() {
                Padding(left: 2u16) {
                    Markdown(source: &details.danger_notes)
                }
            })
        }
    }
}

// ai_view_old removed — superseded by ai_view above
