//! `--mode rpc`: the stdin/stdout JSONL command loop (gh #56).
//!
//! One session across commands (multi-turn state): `prompt`, `steer`,
//! `follow_up`, `cancel`, and `shutdown` drive turns on the session the
//! startup flags selected, stdout carries the extended event stream plus
//! one `response` per command, and `shutdown` (or stdin EOF) persists
//! the session and exits cleanly. Diagnostics go to stderr; stdout is
//! protocol records only (pi's framing rules).
//!
//! The RPC Extension UI sub-protocol is out: extension-UI-over-RPC needs
//! the ui@0.6.0 world work (#172's train).
//!
//! Concurrency shape: at most one run future exists, polled beside stdin
//! (never awaited inline), so `steer` and `cancel` land mid-turn. Turn
//! events and command responses ride channels into one stdout writer, so
//! neither producer ever blocks the other and stdout keeps one writer.

use super::headless::{HeadlessSink, Setup, setup};
use super::*;
use lca_core::{Agent, AgentConfig, TurnOutcome, TurnSink};
use lca_session::SessionStore;
use lca_tools::{CancelFlag, ToolExecutor};
use std::io::Write;
use std::path::Path;

/// A `Write` sink that ships complete lines over a channel (one stdout
/// writer downstream; a client that stops reading stalls nothing here,
/// and the process stays responsive to `cancel`). Line-buffered because
/// `write_fmt` may split one record across several `write` calls.
struct ChanWriter {
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    buf: String,
}

impl ChanWriter {
    fn new(tx: tokio::sync::mpsc::UnboundedSender<String>) -> ChanWriter {
        ChanWriter {
            tx,
            buf: String::new(),
        }
    }
}

impl std::io::Write for ChanWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buf.push_str(&String::from_utf8_lossy(bytes));
        while let Some(end) = self.buf.find('\n') {
            let line: String = self.buf.drain(..end).collect();
            self.buf.drain(..1);
            let _ = self.tx.send(line);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            let _ = self.tx.send(line);
        }
        Ok(())
    }
}

/// The per-turn mutable state: tools and the permission prompt. Owned
/// by the frame while idle, moved into the run future while a turn runs
/// (so the stored future borrows nothing mutable across commands), and
/// handed back when the run ends.
struct TurnState {
    tools: ToolExecutor,
    prompt_impl: HeadlessPrompt,
}

/// Run the RPC loop on the session the flags selected (gh #56).
#[allow(clippy::too_many_arguments)]
pub async fn rpc(
    model_override: Option<&str>,
    session: &crate::SessionSelector,
    cwd: &Path,
    yolo: bool,
    allow_host: &[String],
    flags: &crate::CliFlags,
) -> i32 {
    let setup = match setup(cwd, model_override, session, yolo, allow_host, flags, "rpc").await {
        Ok(setup) => setup,
        Err(code) => return code,
    };
    let Setup {
        store,
        session,
        grants,
        provider,
        tools,
        agent_config,
        proposals,
        prompt_impl,
        mcp_manager,
        follow_up_mode,
        _temp_guard: _,
        _volatile: _,
    } = setup;

    // One stdout writer for events and responses alike.
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (response_tx, mut response_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut out = std::io::stdout();
        loop {
            tokio::select! {
                biased;
                line = event_rx.recv() => {
                    let Some(line) = line else { break };
                    let _ = writeln!(out, "{line}");
                }
                line = response_rx.recv() => {
                    let Some(line) = line else { break };
                    let _ = writeln!(out, "{line}");
                }
            }
        }
        let _ = out.flush();
    });
    // The turn's event stream reuses the headless taxonomy verbatim.
    let event_sink = || {
        HeadlessSink::with_writer(
            true,
            false,
            Box::new(ChanWriter::new(event_tx.clone())) as Box<dyn std::io::Write + Send>,
        )
    };
    let mut session_sink = event_sink();
    session_sink.emit_session_start(session.id());
    // Gh #426: the accept is emitted when the queue takes a message, which
    // is between turns and not inside any turn's sink - so it needs its own
    // handle on the same writer. Same channel, same taxonomy, so the accept
    // and the `queue-flushed` drain stay in arrival order on one stream.
    let mut queue_sink = event_sink();
    let mut turn_state = Some(TurnState { tools, prompt_impl });

    // At most one run at a time; a second prompt while running needs an
    // explicit disposition, pi's rule.
    // Not `Send`: the agent holds non-`Send` prompt callbacks, and
    // `select!` polls inline without spawning, so it never needs to be.
    //
    // gh #489: the run future is a **pinned local the loop owns**, and the
    // `select!` below borrows it with `as_mut()`. It must never be moved into a
    // branch future: `select!` rebuilds every branch future on each iteration,
    // so anything a branch captured and owned is dropped the moment another
    // branch wins. That is exactly what used to happen - a `run.take()` inside
    // the branch future meant a stdin line arriving mid-turn killed the run, and
    // `turn_state`, which had moved into the same future, died with it. The
    // session then answered `"started"` to prompts it could never run.
    //
    // The invariant that keeps it safe: `run` is cleared only in the arm that
    // observed completion, and `turn_state` only leaves `Some` when a future
    // that actually finished hands it back.
    type RunFuture<'a> =
        std::pin::Pin<Box<dyn std::future::Future<Output = (TurnOutcome, TurnState)> + 'a>>;
    let mut run: Option<RunFuture<'_>> = None;
    let mut cancel_flag: Option<CancelFlag> = None;
    let mut follow_ups: Vec<String> = Vec::new();
    let mut shutdown_after_run = false;
    // Stdin rides a detached thread into a channel, never tokio's
    // stdin reader: its blocking read would otherwise hold runtime
    // shutdown until EOF, and an RPC session must exit on `shutdown`
    // with stdin still open. A thread blocked in `read` dies with the
    // process; the runtime never waits for it.
    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead as _;
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if stdin_tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stdin_open = true;

    // Start one turn on the shared state (mirrors the one-shot path: a
    // fresh agent per turn, borrowing tools for exactly one run). The
    // text moves into the future, so nothing dangles across commands.
    //
    // gh #489: the future still owns `turn_state` (it moves in here and is
    // handed back by the outcome arm), which is only safe because the `select!`
    // below polls the future **in place** - nothing but the completion arm can
    // clear `run`, so the future is never dropped mid-turn. Design point 2's
    // second clause: "the future owns it *and* is guaranteed not to be
    // dropped". Previously the `take()` lived inside a select branch future,
    // which `select!` rebuilds each iteration, so the drop was routine.
    macro_rules! start_run {
        ($text:expr, $queue:expr) => {{
            if let Some(mut state) = turn_state.take() {
                let flag = CancelFlag::new();
                cancel_flag = Some(flag.clone());
                let text_owned: String = $text.to_string();
                let grants = grants.clone();
                let proposals = proposals.as_ref();
                let agent_config = agent_config.clone();
                let provider = provider.clone();
                // Owned clones, not frame borrows: the future outlives
                // the iteration that built it, and `finish` still needs
                // the originals afterwards.
                let (store_ref, session_ref) = (store.clone(), session.clone());
                let mut turn_sink = event_sink();
                // gh #418 Phase E: the first turn waits (bounded) for servers
                // whose tools are declared to the model, so they appear in the
                // first request; a server still connecting costs the budget at
                // most and is then reported, never silently missing. RPC's
                // events are the machine-readable path, so the note rides the
                // `extension-event` envelope the interface and headless both
                // already understand.
                let still_connecting = mcp_manager.wait_for_startup();
                if !still_connecting.is_empty() {
                    turn_sink.on_event(lca_protocol::TurnEvent::ExtensionEvent {
                        extension: "mcp".to_string(),
                        event: "connecting".to_string(),
                        detail: format!(
                            "{} still connecting; their tools appear once they land",
                            still_connecting.join(", ")
                        ),
                    });
                }
                run = Some(Box::pin(async move {
                    let mut agent = Agent::new(
                        &store_ref,
                        &session_ref,
                        provider.as_ref(),
                        &mut state.tools,
                        grants,
                        &mut state.prompt_impl,
                        proposals,
                        agent_config.clone(),
                    );
                    // gh #269: the same nested-completion backend `headless`
                    // and the TUI runner install. Without it an RPC session's
                    // compaction had no backend and silently degraded to the
                    // mechanical excerpt summary, and an extension asking for
                    // the `completion` capability had nothing to call - in the
                    // very entry point a daemon client uses.
                    let nested = std::sync::Arc::new(lca_core::ext_provider::ProviderBackend::new(
                        provider.clone(),
                        agent_config.model.clone(),
                        session_ref.id().to_string(),
                    ));
                    agent.set_nested_backend(nested);
                    let outcome = agent
                        .run_turn_queued(&text_owned, $queue, &[], &mut turn_sink, &flag)
                        .await;
                    (outcome, state)
                }));
            }
        }};
    }

    // One response per command, correlated by the client's id.
    macro_rules! respond {
        ($id:expr, $command:expr, $success:expr, $data:expr) => {{
            let mut record = serde_json::json!({
                "type": "response",
                "command": $command,
                "success": $success,
            });
            if $success {
                record["data"] = $data;
            } else {
                record["error"] = $data;
            }
            if let Some(id) = $id {
                record["id"] = serde_json::Value::String(id);
            }
            let _ = response_tx.send(record.to_string());
        }};
    }

    // A malformed line answers with the parse response, whatever the
    // loop is doing.
    macro_rules! respond_parse_error {
        ($err:expr) => {{
            let _ = response_tx.send(
                serde_json::json!({
                    "type": "response",
                    "command": "parse",
                    "success": false,
                    "error": format!("failed to parse command: {}", $err),
                })
                .to_string(),
            );
        }};
    }

    // Queue one message by mode (steer drains at turn boundaries through
    // the shared queue; follow-ups submit after the run).
    macro_rules! enqueue {
        ($command:expr, $message:expr) => {{
            // Gh #426: the accept, emitted the moment the queue takes the
            // message and before anything can drain it - so a client sees
            // `queue-queued` (accepted) ahead of `queue-flushed` (drained),
            // and "accepted but never injected" is distinguishable from
            // "never arrived". The mode is the one actually used.
            if $command == "steer" {
                super::lock(&agent_config.steer).push(lca_protocol::QueuedMessage {
                    text: $message,
                    mode: lca_protocol::SubmitMode::Steer,
                });
                queue_sink.emit_queue_queued(lca_protocol::SubmitMode::Steer);
            } else {
                follow_ups.push($message);
                queue_sink.emit_queue_queued(lca_protocol::SubmitMode::FollowUp);
            }
        }};
    }

    /// Poll an active run future in place (gh #489).
    ///
    /// Takes `&mut Option<..>` rather than the `Option` itself so the future is
    /// never moved out: `select!` rebuilds branch futures every iteration, and a
    /// moved run would be dropped whenever the stdin branch won. The `None` arm
    /// yields a future that never resolves, which is unreachable because the
    /// caller's `if run.is_some()` guard disables the branch first.
    async fn poll_run(run: &mut Option<RunFuture<'_>>) -> (TurnOutcome, TurnState) {
        match run.as_mut() {
            Some(future) => future.await,
            None => std::future::pending().await,
        }
    }

    loop {
        if run.is_none() {
            if !stdin_open {
                break;
            }
            match stdin_rx.recv().await {
                None => break,
                Some(line) => {
                    let value: serde_json::Value = match serde_json::from_str(&line) {
                        Ok(value) => value,
                        Err(err) => {
                            respond_parse_error!(err);
                            continue;
                        }
                    };
                    let id = value
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(str::to_string);
                    let command = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    match command {
                        "prompt" => {
                            if shutdown_after_run {
                                respond!(
                                    id,
                                    command,
                                    false,
                                    serde_json::Value::String(
                                        "the session is shutting down".to_string()
                                    )
                                );
                                continue;
                            }
                            let Some(message) = value.get("message").and_then(|m| m.as_str())
                            else {
                                respond!(
                                    id,
                                    command,
                                    false,
                                    serde_json::Value::String("prompt needs a message".to_string())
                                );
                                continue;
                            };
                            if value.get("images").is_some() {
                                respond!(
                                    id,
                                    command,
                                    false,
                                    serde_json::Value::String(
                                        "images are not supported in rpc v1".to_string()
                                    )
                                );
                                continue;
                            }
                            start_run!(message, None);
                            respond!(
                                id,
                                command,
                                true,
                                serde_json::json!({"disposition": "started"})
                            );
                        }
                        "steer" | "follow_up" => {
                            let Some(message) = value.get("message").and_then(|m| m.as_str())
                            else {
                                respond!(
                                    id,
                                    command,
                                    false,
                                    serde_json::Value::String(format!("{command} needs a message"))
                                );
                                continue;
                            };
                            enqueue!(command, message.to_string());
                            respond!(
                                id,
                                command,
                                true,
                                serde_json::json!({"disposition": "queued"})
                            );
                        }
                        "cancel" => {
                            respond!(id, command, true, serde_json::json!({}));
                        }
                        "shutdown" => {
                            respond!(id, command, true, serde_json::json!({}));
                            break;
                        }
                        _ => {
                            respond!(
                                id,
                                command,
                                false,
                                serde_json::Value::String(format!("unknown command: {command}"))
                            );
                        }
                    }
                }
            }
            continue;
        }
        tokio::select! {
            biased;
            line = stdin_rx.recv(), if stdin_open => {
                match line {
                    None => {
                        stdin_open = false;
                        if run.is_none() {
                            break;
                        }
                        shutdown_after_run = true;
                    }
                    Some(line) => {
                        let value: serde_json::Value = match serde_json::from_str(&line) {
                            Ok(value) => value,
                            Err(err) => {
                                respond_parse_error!(err);
                                continue;
                            }
                        };
                        let id = value
                            .get("id")
                            .and_then(|id| id.as_str())
                            .map(str::to_string);
                        let command = value.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match command {
                            "prompt" => match value.get("streamingBehavior").and_then(|b| b.as_str()) {
                                Some("steer") => {
                                    let Some(message) = value.get("message").and_then(|m| m.as_str()) else {
                                        respond!(id, command, false, serde_json::Value::String("prompt needs a message".to_string()));
                                        continue;
                                    };
                                    enqueue!("steer", message.to_string());
                                    respond!(id, command, true, serde_json::json!({"disposition": "queued"}));
                                }
                                Some("followUp") => {
                                    let Some(message) = value.get("message").and_then(|m| m.as_str()) else {
                                        respond!(id, command, false, serde_json::Value::String("prompt needs a message".to_string()));
                                        continue;
                                    };
                                    enqueue!("follow_up", message.to_string());
                                    respond!(id, command, true, serde_json::json!({"disposition": "queued"}));
                                }
                                _ => {
                                    respond!(id, command, false, serde_json::Value::String("the agent is streaming; pass streamingBehavior \"steer\" or \"followUp\"".to_string()));
                                }
                            },
                            "steer" | "follow_up" => {
                                let Some(message) = value.get("message").and_then(|m| m.as_str()) else {
                                    respond!(id, command, false, serde_json::Value::String(format!("{command} needs a message")));
                                    continue;
                                };
                                enqueue!(command, message.to_string());
                                respond!(id, command, true, serde_json::json!({"disposition": "queued"}));
                            }
                            "cancel" => {
                                if let Some(flag) = cancel_flag.take() {
                                    flag.cancel();
                                }
                                respond!(id, command, true, serde_json::json!({}));
                            }
                            "shutdown" => {
                                respond!(id, command, true, serde_json::json!({}));
                                shutdown_after_run = true;
                            }
                            _ => {
                                respond!(id, command, false, serde_json::Value::String(format!("unknown command: {command}")));
                            }
                        }
                    }
                }
            }
            // gh #489: a borrow of the loop's own `run`, never `run.take()`
            // inside the branch. `select!` rebuilds branch futures every
            // iteration, so a future that owned the taken run was dropped
            // whenever the biased stdin branch won - destroying the turn, and
            // `turn_state` (which that future owned) with it. Polling the pinned
            // future where it lives means a stdin line mid-turn costs a poll,
            // not a turn.
            //
            // The guard is what makes the `None` arm in `poll_run` unreachable:
            // `select!` checks `if run.is_some()` before constructing the future.
            outcome = poll_run(&mut run), if run.is_some() => {
                cancel_flag = None;
                // Cleared only here, after the await finished: that is the
                // guarantee that keeps `turn_state` (which the future owns)
                // from ever being dropped.
                run = None;
                let (outcome, state) = outcome;
                turn_state = Some(state);
                let _ = outcome;
                // Follow-ups run after the run ends, whatever ended it:
                // cancelling a turn drops the turn, never the queue - and
                // a shutdown drains them before exiting, so nothing
                // accepted is silently dropped.
                if !follow_ups.is_empty() {
                    // gh #268: drain per `queue.follow_up_mode`, the same rule TUI
                    // and `lca-ui`'s queue follow. `all` hands every queued message
                    // over as one joined submission (TUI's `texts.join("\n\n")`),
                    // so one turn answers them together; `one-at-a-time` yields them
                    // serially, one per run end.
                    let next = match follow_up_mode {
                        lca_protocol::QueueMode::All => {
                            std::mem::take(&mut follow_ups).join("\n\n")
                        }
                        lca_protocol::QueueMode::OneAtATime => follow_ups.remove(0),
                    };
                    start_run!(next, Some(lca_protocol::SubmitMode::FollowUp));
                } else if shutdown_after_run {
                    break;
                }
            }
        }
    }
    drop(event_tx);
    drop(response_tx);
    let _ = writer.await;
    finish(store, &session, agent_config).await;
    exit::OK
}

/// Persist the session and run the close hooks (the one-shot path's
/// ending, shared so shutdown cannot skip it).
///
/// **`async` since gh #268.** It used to be a sync function that called
/// `lca_core::drive_blocking(async { registry.on_session_close().await })` while
/// being driven from `async fn rpc` - a thread plus a nested single-thread Tokio
/// runtime inside a running async context, for a hook that is already `async fn`.
/// Awaiting runs the same hook to the same completion without either, and the
/// `store.close` after it still happens only once the teardown has finished,
/// because an `.await` is what drives a future to the end.
async fn finish(store: SessionStore, session: &lca_session::Session, agent_config: AgentConfig) {
    let close_registry = agent_config.extensions.clone();
    close_registry.on_session_close().await;
    let _ = store.close(session);
}
