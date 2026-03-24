use crate::commands::detect_shell;
use crate::tui::state::{AppMode, AppState, ExitAction};
use crate::tui::view::ai_view;
use atuin_client::distro::detect_linux_distribution;
use atuin_common::tls::ensure_crypto_provider;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use eventsource_stream::Eventsource;
use eye_declare::{Application, ControlFlow, Handle};
use eyre::{Context as _, Result, bail};
use futures::StreamExt;
use reqwest::Url;
use tracing::{debug, error, info, trace};

pub async fn run(
    initial_command: Option<String>,
    api_endpoint: Option<String>,
    api_token: Option<String>,
    _keep_output: bool,
    _debug_state_file: Option<String>,
    settings: &atuin_client::settings::Settings,
    output_for_hook: bool,
) -> Result<()> {
    if !settings.ai.enabled.unwrap_or(false) {
        emit_shell_result(
            Action::Print(
                "Atuin AI is not enabled. Please enable it in your settings or run `atuin setup`."
                    .to_string(),
            ),
            output_for_hook,
        );
        return Ok(());
    }

    let endpoint = api_endpoint.as_deref().unwrap_or(
        settings
            .ai
            .endpoint
            .as_deref()
            .unwrap_or("https://hub.atuin.sh"),
    );
    let api_token = api_token.as_deref().or(settings.ai.api_token.as_deref());

    let token = if let Some(token) = &api_token {
        token.to_string()
    } else {
        ensure_hub_session(settings).await?
    };

    let action = run_inline_tui(endpoint.to_string(), token, initial_command, settings).await?;
    emit_shell_result(action, output_for_hook);

    Ok(())
}

async fn ensure_hub_session(settings: &atuin_client::settings::Settings) -> Result<String> {
    if let Some(token) = atuin_client::hub::get_session_token().await? {
        debug!("Found Hub session, using existing token");
        return Ok(token);
    }

    let hub_address = settings.active_hub_endpoint().unwrap_or_default();
    let will_sync = settings.is_hub_sync();

    info!("No Hub session found, prompting for authentication");

    println!("Atuin AI requires authenticating with Atuin Hub.");
    if will_sync {
        println!(
            "Once logged in, your shell history will be synchronized via Atuin Hub if auto_sync is enabled or when manually syncing."
        )
    }
    println!(
        "If you have an existing Atuin sync account, you can log in with your existing credentials."
    );
    println!("Press enter to begin (or esc to cancel).");
    if !wait_for_login_confirmation()? {
        bail!("authentication canceled");
    }

    debug!("Starting Atuin Hub authentication...");
    println!("Authenticating with Atuin Hub...");

    let session = atuin_client::hub::HubAuthSession::start(hub_address.as_ref()).await?;
    println!("Open this URL to continue:");
    println!("{}", session.auth_url);

    let token = session
        .wait_for_completion(
            atuin_client::hub::DEFAULT_AUTH_TIMEOUT,
            atuin_client::hub::DEFAULT_POLL_INTERVAL,
        )
        .await?;

    info!("Authentication complete, saving session token");
    atuin_client::hub::save_session(&token).await?;

    if let Ok(meta) = atuin_client::settings::Settings::meta_store().await
        && let Ok(Some(cli_token)) = meta.session_token().await
    {
        debug!("CLI session found, attempting to link accounts");
        if let Err(e) = atuin_client::hub::link_account(hub_address.as_ref(), &cli_token).await {
            debug!("Could not link CLI account to Hub: {}", e);
        } else {
            info!("Successfully linked CLI account to Hub");
        }
    }

    Ok(token)
}

// ───────────────────────────────────────────────────────────────────
// SSE streaming
// ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum ChatStreamEvent {
    TextChunk(String),
    ToolCall {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    Status(String),
    Done {
        session_id: String,
    },
    Error(String),
}

fn create_chat_stream(
    hub_address: String,
    token: String,
    session_id: Option<String>,
    messages: Vec<serde_json::Value>,
    send_cwd: bool,
) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<ChatStreamEvent>> + Send>> {
    Box::pin(async_stream::stream! {
        ensure_crypto_provider();
        let endpoint = match hub_url(&hub_address, "/api/cli/chat") {
            Ok(url) => url,
            Err(e) => {
                yield Err(e);
                return;
            }
        };

        debug!("Sending SSE request to {endpoint}");

        let os = detect_os();
        let shell = detect_shell();

        let mut context = serde_json::json!({
            "os": os,
            "shell": shell,
            "pwd": if send_cwd { std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()) } else { None },
        });

        if os == "linux" {
            context["distro"] = serde_json::json!(detect_linux_distribution());
        }

        let mut request_body = serde_json::json!({
            "messages": messages,
            "context": context,
        });

        if let Some(ref sid) = session_id {
            trace!("Including session_id in request: {sid}");
            request_body["session_id"] = serde_json::json!(sid);
        }

        let client = reqwest::Client::new();
        let response = match client
            .post(endpoint.clone())
            .header("Accept", "text/event-stream")
            .bearer_auth(&token)
            .json(&request_body)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                yield Err(eyre::eyre!("Failed to send SSE request: {}", e));
                return;
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            error!("SSE request failed with status: {status}, clearing session");
            let _ = atuin_client::hub::delete_session().await;
            yield Err(eyre::eyre!("Hub session expired. Re-run to authenticate again."));
            return;
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!("SSE request failed ({}): {}", status, body);
            yield Err(eyre::eyre!("SSE request failed ({}): {}", status, body));
            return;
        }

        let byte_stream = response.bytes_stream();
        let mut stream = byte_stream.eventsource();

        while let Some(event) = stream.next().await {
            match event {
                Ok(sse_event) => {
                    let event_type = sse_event.event.as_str();
                    let data = sse_event.data.clone();

                    debug!(event_type = %event_type, "SSE event received");

                    match event_type {
                        "text" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(content) = json.get("content").and_then(|v| v.as_str())
                            {
                                yield Ok(ChatStreamEvent::TextChunk(content.to_string()));
                            }
                        }
                        "tool_call" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let id = json.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let name = json.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let input = json.get("input").cloned().unwrap_or(serde_json::json!({}));
                                yield Ok(ChatStreamEvent::ToolCall { id, name, input });
                            }
                        }
                        "tool_result" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let tool_use_id = json.get("tool_use_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let content = json.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let is_error = json.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                                yield Ok(ChatStreamEvent::ToolResult { tool_use_id, content, is_error });
                            }
                        }
                        "status" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data)
                                && let Some(state) = json.get("state").and_then(|v| v.as_str())
                            {
                                yield Ok(ChatStreamEvent::Status(state.to_string()));
                            }
                        }
                        "done" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let session_id = json.get("session_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                yield Ok(ChatStreamEvent::Done { session_id });
                            } else {
                                yield Ok(ChatStreamEvent::Done { session_id: String::new() });
                            }
                            break;
                        }
                        "error" => {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error").to_string();
                                error!("SSE error: {}", message);
                                yield Ok(ChatStreamEvent::Error(message));
                            } else {
                                error!("SSE error: {}", data);
                                yield Ok(ChatStreamEvent::Error(data));
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    yield Err(eyre::eyre!("SSE error: {}", e));
                    break;
                }
            }
        }
    })
}

// ───────────────────────────────────────────────────────────────────
// Async streaming task — pushes updates to app state via Handle
// ───────────────────────────────────────────────────────────────────

async fn run_chat_stream(
    handle: Handle<AppState>,
    endpoint: String,
    token: String,
    session_id: Option<String>,
    messages: Vec<serde_json::Value>,
    send_cwd: bool,
) {
    let stream = create_chat_stream(endpoint, token, session_id, messages, send_cwd);
    futures::pin_mut!(stream);

    while let Some(event) = stream.next().await {
        match event {
            Ok(ChatStreamEvent::TextChunk(text)) => {
                trace!(text = %text, "Processing TextChunk");
                handle.update(move |state| {
                    state.append_streaming_text(&text);
                });
            }
            Ok(ChatStreamEvent::ToolCall { id, name, input }) => {
                trace!(id = %id, name = %name, "Processing ToolCall");
                handle.update(move |state| {
                    state.add_tool_call(id, name, input);
                });
            }
            Ok(ChatStreamEvent::ToolResult {
                tool_use_id,
                content,
                is_error,
            }) => {
                trace!(tool_use_id = %tool_use_id, "Processing ToolResult");
                handle.update(move |state| {
                    state.add_tool_result(tool_use_id, content, is_error);
                });
            }
            Ok(ChatStreamEvent::Status(status)) => {
                trace!(status = %status, "Processing Status");
                handle.update(move |state| {
                    state.update_streaming_status(&status);
                });
            }
            Ok(ChatStreamEvent::Done { session_id }) => {
                trace!(session_id = %session_id, "Processing Done");
                handle.update(move |state| {
                    if !session_id.is_empty() {
                        state.store_session_id(session_id);
                    }
                    state.finalize_streaming();
                });
                break;
            }
            Ok(ChatStreamEvent::Error(msg)) => {
                trace!(error = %msg, "Processing Error");
                handle.update(move |state| {
                    state.streaming_error(msg);
                });
                break;
            }
            Err(e) => {
                let msg = e.to_string();
                handle.update(move |state| {
                    state.streaming_error(msg);
                });
                break;
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// Main TUI entry point
// ───────────────────────────────────────────────────────────────────

async fn run_inline_tui(
    endpoint: String,
    token: String,
    initial_prompt: Option<String>,
    settings: &atuin_client::settings::Settings,
) -> Result<Action> {
    let initial_state = AppState::new();
    if let Some(prompt) = initial_prompt {
        let _ = initial_state.input_tx.send(prompt);
    }

    let (mut app, handle) = Application::builder()
        .state(initial_state)
        .view(ai_view)
        .on_commit(|_, state: &mut AppState| {
            // Evict the oldest message when content scrolls into terminal scrollback
            if !state.events.is_empty() {
                state.events.remove(0);
            }
        })
        .build()?;

    let h = handle;
    let send_cwd = settings.ai.send_cwd;
    let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;

    app.run_interactive(move |event, state| {
        let Event::Key(KeyEvent {
            code,
            kind: KeyEventKind::Press,
            modifiers,
            ..
        }) = event
        else {
            return ControlFlow::Continue;
        };

        // Helper: set exit action and mark as exiting so the final render
        // hides the input box, leaving only conversation content visible.
        let do_exit = |state: &mut AppState, action: ExitAction| -> ControlFlow {
            state.exit_action = Some(action);
            state.exiting = true;
            ControlFlow::Exit
        };

        // Ctrl+C always exits
        if modifiers.contains(KeyModifiers::CONTROL) && *code == KeyCode::Char('c') {
            return do_exit(state, ExitAction::Cancel);
        }

        match state.mode {
            AppMode::Input => {
                match code {
                    KeyCode::Esc => {
                        return do_exit(state, ExitAction::Cancel);
                    }
                    KeyCode::Enter => {
                        if state.input_is_empty() {
                            return do_exit(state, ExitAction::Cancel);
                        }

                        // Start generation
                        let messages = {
                            state.start_generating();
                            state.events_to_messages()
                        };
                        state.start_streaming();

                        // Spawn SSE streaming task
                        let h2 = h.clone();
                        let ep = endpoint.clone();
                        let tk = token.clone();
                        let sid = state.session_id.clone();
                        stream_task = Some(tokio::spawn(async move {
                            run_chat_stream(h2, ep, tk, sid, messages, send_cwd).await;
                        }));
                    }
                    _ => {
                        // Other keys handled by InputBox component via handle_event
                    }
                }
            }
            AppMode::Generating => {
                if *code == KeyCode::Esc {
                    state.cancel_generation();
                    if let Some(task) = stream_task.take() {
                        task.abort();
                    }
                }
            }
            AppMode::Streaming => {
                if *code == KeyCode::Esc {
                    state.cancel_streaming();
                    if let Some(task) = stream_task.take() {
                        task.abort();
                    }
                }
            }
            AppMode::Review => match code {
                KeyCode::Esc => {
                    state.confirmation_pending = false;
                    return do_exit(state, ExitAction::Cancel);
                }
                KeyCode::Enter => {
                    let cmd = state.current_command().map(|c| c.to_string());
                    if let Some(cmd) = cmd {
                        if state.is_current_command_dangerous() && !state.confirmation_pending {
                            state.confirmation_pending = true;
                        } else {
                            state.confirmation_pending = false;
                            return do_exit(state, ExitAction::Execute(cmd));
                        }
                    }
                }
                KeyCode::Tab => {
                    let cmd = state.current_command().map(|c| c.to_string());
                    if let Some(cmd) = cmd {
                        state.confirmation_pending = false;
                        return do_exit(state, ExitAction::Insert(cmd));
                    }
                }
                KeyCode::Char('f') => {
                    state.confirmation_pending = false;
                    state.start_edit_mode();
                }
                _ => {}
            },
            AppMode::Error => {
                match code {
                    KeyCode::Esc => {
                        return do_exit(state, ExitAction::Cancel);
                    }
                    KeyCode::Enter | KeyCode::Char('r') => {
                        // Retry: re-enter generating and spawn new stream
                        state.retry();
                        let messages = state.events_to_messages();
                        state.start_streaming();

                        let h2 = h.clone();
                        let ep = endpoint.clone();
                        let tk = token.clone();
                        let sid = state.session_id.clone();
                        stream_task = Some(tokio::spawn(async move {
                            run_chat_stream(h2, ep, tk, sid, messages, send_cwd).await;
                        }));
                    }
                    _ => {}
                }
            }
        }

        ControlFlow::Continue
    })
    .await?;

    // Map exit action to return value
    let result = match app.state().exit_action {
        Some(ExitAction::Execute(ref cmd)) => Action::Execute(cmd.clone()),
        Some(ExitAction::Insert(ref cmd)) => Action::Insert(cmd.clone()),
        _ => Action::Cancel,
    };

    Ok(result)
}

// ───────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────

fn hub_url(base: &str, path: &str) -> Result<Url> {
    let base_with_slash = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    let stripped = path.strip_prefix('/').unwrap_or(path);
    Url::parse(&base_with_slash)?
        .join(stripped)
        .context("failed to build hub URL")
}

fn detect_os() -> String {
    match std::env::consts::OS {
        "macos" => "macos".to_string(),
        "linux" => "linux".to_string(),
        "windows" => "windows".to_string(),
        other => format!("Other: {other}"),
    }
}

#[derive(Clone)]
enum Action {
    Execute(String),
    Insert(String),
    Print(String),
    Cancel,
}

fn emit_shell_result(action: Action, output_for_hook: bool) {
    if output_for_hook {
        match action {
            Action::Execute(output) => eprintln!("__atuin_ai_execute__:{output}"),
            Action::Insert(output) => eprintln!("__atuin_ai_insert__:{output}"),
            Action::Print(output) => eprintln!("__atuin_ai_print__:{output}"),
            Action::Cancel => eprintln!("__atuin_ai_cancel__"),
        }
    } else {
        match action {
            Action::Execute(output) => eprintln!("{output}"),
            Action::Insert(output) => eprintln!("{output}"),
            Action::Print(output) => eprintln!("{output}"),
            Action::Cancel => eprintln!(),
        }
    }
}

fn wait_for_login_confirmation() -> Result<bool> {
    use crossterm::{
        event::{self, Event, KeyCode},
        terminal::{disable_raw_mode, enable_raw_mode},
    };

    enable_raw_mode().context("failed enabling raw mode for login prompt")?;
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
    let _guard = Guard;

    loop {
        let ev = event::read().context("failed to read login confirmation key")?;
        if let Event::Key(key) = ev {
            match key.code {
                KeyCode::Enter => return Ok(true),
                KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}
