//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    Component, Elements, Line, Span, TextBlock, VStack, element, impl_slot_children,
};
use ratatui_core::style::{Color, Modifier, Style};

use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::state::{AppMode, AppState, ConversationEvent};

#[derive(Debug)]
enum UiEvent {
    Text {
        content: String,
    },
    ToolCall {
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        status: ToolResultStatus,
    },
    SuggestedCommand {
        command: String,
        danger_level: String,
        danger_notes: String,
        confidence_level: String,
        confidence_notes: String,
    },
}

#[derive(Debug)]
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
            events.push(UiEvent::SuggestedCommand {
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
            });
        }
    }

    fn add_tool_call(&mut self, id: &str, name: &str, input: &serde_json::Value) {
        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            events.push(UiEvent::ToolCall {
                tool_use_id: id.to_string(),
                name: name.to_string(),
                input: input.clone(),
                status: ToolResultStatus::Pending,
            });
        }
    }

    fn add_tool_result(&mut self, tool_use_id: &str, _content: &str, is_error: bool) {
        self.start_agent_turn();
        if let UiTurn::AgentTurn { events } = self.turn_mut_unsafe() {
            let event = events.iter_mut().find(|e| match e {
                UiEvent::ToolCall {
                    tool_use_id: id, ..
                } => id == tool_use_id,
                _ => false,
            });
            if let Some(UiEvent::ToolCall { status, .. }) = event {
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

        // TODO: collapse multiple tool calls into a ToolSummary

        std::mem::take(&mut self.turns)
    }
}

struct LeftPadded;

impl Default for LeftPadded {
    fn default() -> Self {
        Self
    }
}

impl Component for LeftPadded {
    type State = ();

    fn content_inset(&self, _state: &Self::State) -> eye_declare::Insets {
        eye_declare::Insets::ZERO.left(2)
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

impl_slot_children!(LeftPadded);

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

    element! {
        #(for turn in turns {
            #(match turn {
                UiTurn::UserTurn { events } => {
                    user_turn_view(&events)
                }
                UiTurn::AgentTurn { events } => {
                    agent_turn_view(&events)
                }
            })
        })

        #(if !state.exiting {
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
                            LeftPadded {
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

fn agent_turn_view(events: &[UiEvent]) -> Elements {
    let label_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);

    element! {
        VStack {
            TextBlock {
                Line {
                    Span(text: "Atuin AI", style: label_style)
                }
            }
            #(for event in events {
                #(match event {
                    UiEvent::Text { content } => {
                        element! {
                            LeftPadded {
                                Markdown(source: content)
                            }
                        }
                    },
                    UiEvent::SuggestedCommand { command, danger_level, danger_notes, confidence_level, confidence_notes } => {
                        suggested_command_view(command, danger_level, danger_notes, confidence_level, confidence_notes)
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn suggested_command_view(
    command: &str,
    danger_level: &str,
    danger_notes: &str,
    confidence_level: &str,
    confidence_notes: &str,
) -> Elements {
    element! {
        VStack {
            TextBlock {
                Line {
                    Span(text: "Suggested command:", style: Style::default().fg(Color::Cyan))
                }
                Line {
                    #(if danger_level == "high" || danger_level == "medium" || danger_level == "med" {
                        Span(text: "! ", style: Style::default().fg(Color::Yellow))
                    } else {
                        Span(text: "$ ", style: Style::default().fg(Color::Blue))
                    })
                    Span(text: command, style: Style::default().fg(Color::Green))
                }
            }
            #(if !danger_notes.is_empty() {
                LeftPadded {
                    Markdown(source: danger_notes)
                }
            })
        }
    }
}

// ai_view_old removed — superseded by ai_view above
