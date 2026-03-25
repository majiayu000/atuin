//! View function that builds the eye-declare element tree from app state.

use eye_declare::{
    Column, Component, Elements, HStack, Line, Span, Spinner, TextBlock, VStack, WidthConstraint,
    element, impl_slot_children,
};
use ratatui_core::style::{Color, Modifier, Style};

use super::components::input_box::InputBox;
use super::components::markdown::Markdown;
use super::state::{AppMode, AppState, ConversationEvent};

mod turn;

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

    let mut turn_builder = turn::TurnBuilder::new();

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
                turn::UiTurn::UserTurn { events } => {
                    user_turn_view(&events, index == 0)
                }
                turn::UiTurn::AgentTurn { events } => {
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

fn user_turn_view(events: &[turn::UiEvent], first_turn: bool) -> Elements {
    let label_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    element! {
        VStack {
            TextBlock {
                #(if !first_turn {
                    Line { Span() }
                })
                Line {
                    Span(text: "You", style: label_style)
                }
            }
            #(for event in events {
                #(match event {
                    turn::UiEvent::Text { content } => {
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

fn agent_turn_view(events: &[turn::UiEvent], busy: bool) -> Elements {
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
                    turn::UiEvent::Text { content } => {
                        element! {
                            Padding(left: 2u16) {
                                Markdown(source: content)
                            }
                        }
                    },
                    turn::UiEvent::ToolSummary(summary) => {
                        tool_summary_view(summary)
                    },
                    turn::UiEvent::SuggestedCommand(details) => {
                        suggested_command_view(details)
                    },
                    _ => element!{}
                })
            })
        }
    }
}

fn tool_summary_view(summary: &turn::ToolSummary) -> Elements {
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

fn suggested_command_view(details: &turn::SuggestedCommandDetails) -> Elements {
    let is_dangerous = matches!(
        details.danger_level,
        turn::DangerLevel::High(_) | turn::DangerLevel::Medium(_)
    );
    let danger_notes = details.danger_level.notes();
    let danger_style = match details.danger_level {
        turn::DangerLevel::High(_) => Style::default().fg(Color::Red).bold(),
        turn::DangerLevel::Medium(_) => Style::default().fg(Color::Yellow),
        turn::DangerLevel::Low(_) => Style::default().fg(Color::Green),
    };
    let danger_text = match details.danger_level {
        turn::DangerLevel::High(_) => "High",
        turn::DangerLevel::Medium(_) => "Medium",
        turn::DangerLevel::Low(_) => "Low",
    };

    let low_confidence = matches!(
        details.confidence_level,
        turn::ConfidenceLevel::Low(_) | turn::ConfidenceLevel::Medium(_)
    );

    let confidence_level = match details.confidence_level {
        turn::ConfidenceLevel::Low(_) => "low",
        turn::ConfidenceLevel::Medium(_) => "medium",
        turn::ConfidenceLevel::High(_) => "high",
    };

    let confidence_notes = details.confidence_level.notes();

    element! {
        VStack {
            TextBlock {
                #(if !details.first_event_in_turn {
                    Line { Span() }
                })
                Line {
                    Span(text: "  Suggested command:", style: Style::default().fg(Color::Cyan))
                }
            }
            HStack {
                Column(width: WidthConstraint::Fixed(2)) {
                    TextBlock {
                        Line {
                            #(if is_dangerous {
                                Span(text: "! ", style: Style::default().fg(Color::Yellow))
                            } else {
                                Span(text: "$ ", style: Style::default().fg(Color::Blue))
                            })
                        }
                    }
                }
                Column {
                    TextBlock {
                        Line {
                            Span(text: &details.command, style: Style::default().fg(Color::Green))
                        }
                    }
                }
            }
            #(if is_dangerous {
                Padding(left: 2u16) {
                    TextBlock {
                        Line {
                            Span(text: "Danger: ", style: danger_style)
                            Span(text: danger_text, style: danger_style)
                        }
                    }
                }
            })
            #(if is_dangerous && danger_notes.is_some() {
                Padding(left: 2u16) {
                    HStack {
                        Column(width: WidthConstraint::Fixed(2)) {
                            TextBlock {
                                Line {
                                    Span(text: "└")
                                }
                            }
                        }
                        Column(width: WidthConstraint::Fill) {
                            Markdown(source: danger_notes.unwrap())
                        }
                    }
                }
            })
            #(if low_confidence && confidence_notes.is_some() {
                Padding(left: 2u16) {
                    Markdown(source: confidence_notes.unwrap())
                }
            })
            #(if low_confidence && !confidence_notes.is_some() {
                Padding(left: 3u16) {
                    Markdown(source: format!("The AI has {} confidence in this answer.", confidence_level))
                }
            })
        }
    }
}

// ai_view_old removed — superseded by ai_view above
