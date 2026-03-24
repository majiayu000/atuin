//! Bordered input box component for the AI TUI.
//!
//! Wraps tui-textarea's TextArea, which handles rendering, wrapping, cursor
//! positioning, and height measurement natively. The component configures the
//! TextArea's block (border + titles) and forwards events to it.
//!
//! Text changes are communicated back to the app via a `tokio::sync::watch` channel.

use crossterm::event::KeyModifiers;
use eye_declare::{Component, EventResult, Hooks};
use ratatui::widgets::{Block, Borders, Padding};
use ratatui_core::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::Widget,
};
use tokio::sync::watch;
use tui_textarea::TextArea;

/// A bordered text input box backed by tui-textarea.
///
/// Props configure the chrome (title, footer). The TextArea itself lives
/// in the component's State so it owns cursor, wrapping, and rendering.
pub struct InputBox {
    /// Title shown in top-left border
    pub title: String,
    /// Right-side label in top border
    pub title_right: String,
    /// Footer text shown in bottom border (keybinding hints)
    pub footer: String,
    /// Whether the input is currently active (shows cursor, accepts input)
    pub active: bool,
    /// Channel sender for reporting text changes back to the app
    pub tx: watch::Sender<String>,
}

impl Default for InputBox {
    fn default() -> Self {
        let (tx, _) = watch::channel(String::new());
        Self {
            title: String::new(),
            title_right: String::new(),
            footer: String::new(),
            active: false,
            tx,
        }
    }
}

pub struct InputBoxState {
    textarea: TextArea<'static>,
}

impl Default for InputBoxState {
    fn default() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        textarea.set_wrap_mode(tui_textarea::WrapMode::Word);
        textarea.set_placeholder_text("Type a message...");
        textarea.set_placeholder_style(
            ratatui::style::Style::default()
                .fg(ratatui::style::Color::DarkGray)
                .add_modifier(ratatui::style::Modifier::ITALIC),
        );
        Self { textarea }
    }
}

impl InputBox {
    /// Build the ratatui Block with current titles/footer.
    fn make_block(&self) -> Block<'_> {
        let border_style = Style::default().fg(Color::DarkGray);
        let title_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .padding(Padding::horizontal(1));

        if !self.title.is_empty() {
            block = block
                .title_top(Line::styled(format!(" {} ", self.title), title_style).left_aligned());
        }
        if !self.title_right.is_empty() {
            block = block.title_top(
                Line::styled(format!(" {} ", self.title_right), border_style).right_aligned(),
            );
        }
        if !self.footer.is_empty() {
            block = block.title_bottom(
                Line::styled(format!(" {} ", self.footer), border_style).right_aligned(),
            );
        }

        block
    }
}

impl Component for InputBox {
    type State = InputBoxState;

    fn initial_state(&self) -> Option<InputBoxState> {
        Some(InputBoxState::default())
    }

    fn lifecycle(&self, hooks: &mut Hooks<Self::State>, _state: &Self::State) {
        if self.active {
            hooks.use_autofocus();
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, state: &Self::State) {
        if area.height < 3 || area.width < 4 {
            return;
        }
        // Configure the block on each render so titles/footer stay current.
        // Note: set_block takes ownership, but the block is cheap to rebuild.
        // We can't call set_block here since we only have &self/&state,
        // so we render block + textarea separately.
        let block = self.make_block();
        let inner = block.inner(area);
        block.render(area, buf);

        // Render textarea into the inner area
        (&state.textarea).render(inner, buf);
    }

    fn desired_height(&self, width: u16, state: &Self::State) -> u16 {
        if width < 4 {
            return 3;
        }
        // Use logical line count + chrome as the height.
        // TextArea handles scrolling internally if content overflows.
        let block = self.make_block();
        let inner = block.inner(Rect::new(0, 0, width, u16::MAX));
        let chrome = (u16::MAX).saturating_sub(inner.height);
        let content = state.textarea.clone().measure(width - 4);
        chrome + content.preferred_rows
    }

    fn is_focusable(&self, _state: &Self::State) -> bool {
        self.active
    }

    fn handle_event(
        &self,
        event: &crossterm::event::Event,
        state: &mut Self::State,
    ) -> EventResult {
        if !self.active {
            return EventResult::Ignored;
        }

        if let crossterm::event::Event::Key(key) = event {
            if key.kind != crossterm::event::KeyEventKind::Press {
                return EventResult::Ignored;
            }

            match key.code {
                crossterm::event::KeyCode::Char('j')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    state.textarea.insert_newline();
                    return EventResult::Consumed;
                }
                crossterm::event::KeyCode::Enter => {
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        state.textarea.insert_char('!');
                        return EventResult::Consumed;
                    } else {
                        // Send current text to app, then bubble up for app to act on
                        let _ = self.tx.send(state.textarea.lines().join("\n"));
                        state.textarea.clear();
                        return EventResult::Ignored;
                    }
                }
                // Esc: bubble up to app
                crossterm::event::KeyCode::Esc => {
                    return EventResult::Ignored;
                }
                _ => {}
            }

            // All other keys: forward to textarea
            if let Some(input) = tui_textarea_input_from_key(key) {
                state.textarea.input(input);
                return EventResult::Consumed;
            }
        }

        EventResult::Ignored
    }
}

/// Convert a crossterm KeyEvent to a tui-textarea Input.
fn tui_textarea_input_from_key(key: &crossterm::event::KeyEvent) -> Option<tui_textarea::Input> {
    use crossterm::event::{KeyCode, KeyModifiers};
    use tui_textarea::{Input, Key};

    let tui_key = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Delete => Key::Delete,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab => Key::Tab,
        KeyCode::Enter => Key::Enter,
        _ => return None,
    };

    Some(Input {
        key: tui_key,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    })
}
