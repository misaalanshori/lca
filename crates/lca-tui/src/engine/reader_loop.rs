//! The stdin reader thread and the negotiation replies it dispatches
//! (`pi-tui-re/src_re/tui-engine/terminal.md` §2/§3).
//!
//! Split from `terminal.rs` for gate 11's 1,200-line ceiling; behaviour unchanged.
//! Everything here runs on the reader thread: it waits on stdin, assembles
//! sequences, dispatches keys and negotiation replies, and re-queries the window
//! size after every arm - the path a resize takes when a terminal reports it
//! (gh #522).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::engine::program_status::{
    ProgramStatusMode, is_program_status_reply, is_program_status_traffic, multiplexer_strips_osc,
    program_status_mode,
};
use crate::engine::stdin_buffer::{StdinBuffer, StdinEvent};
use crate::engine::sys;
use crate::engine::terminal::env_var;
use crate::engine::terminal::{
    DEFAULT_SEQUENCE_TIMEOUT_MS, Negotiation, Shared, parse_negotiation, write_stdout,
};

pub(crate) fn reader_loop(shared: Arc<Shared>, escape_timeout_ms: u64) {
    let mut buffer = StdinBuffer::with_timeouts(DEFAULT_SEQUENCE_TIMEOUT_MS, escape_timeout_ms);
    let mut last_data = Instant::now();
    let mut last_size = sys::terminal_size().unwrap_or((80, 24));
    let mut buf = [0u8; 4096];

    while !shared.shutdown.load(Ordering::SeqCst) {
        // Gh #403 gap 4: a terminal already known to be gone is not waited on.
        // The loop's own copy is the one to check: the process-wide flag may be
        // set by a *different* terminal's hangup (a test owning two).
        if shared.hangup.load(Ordering::SeqCst) {
            break;
        }
        // Gh #323 defect 4: a resize record wakes this loop **and is acted on
        // now**, rather than being swallowed and noticed on the next tick. The
        // size check below runs on every arm, so the only difference is latency -
        // but latency is the whole defect: a dropped record means the app paints
        // at the old width until something else happens to wake it (#522's class).
        match sys::wait_stdin(50) {
            // Gh #403 gap 4: the terminal left. `poll` says so directly
            // (`POLLHUP`/`POLLERR`/`POLLNVAL`), which is the classification that
            // keeps this loop from reading "no bytes" as "nothing yet" - the shape
            // that spun at ~92% of a core forever on a closed pty.
            Ok(outcome) if outcome.is_hangup() => {
                shared.hangup.store(true, Ordering::SeqCst);
                break;
            }
            Ok(outcome) if outcome.is_key() => match sys::read_stdin(&mut buf) {
                // End-of-file on a terminal means the same thing one layer down:
                // the pty is closed. A terminal that reports `POLLIN` and then
                // returns 0 would otherwise be a permanent "key" and spin.
                Ok(0) => {
                    shared.hangup.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(n) => {
                    last_data = Instant::now();
                    for event in buffer.process_bytes(&buf[..n]) {
                        let _ = dispatch(&shared, event, &mut |seq| write_stdout(seq));
                    }
                }
                Err(_) => break,
            },
            // The console told us the buffer changed size (gh #323 defect 4). It
            // is not a key, so nothing is read; falling straight through to the
            // size check below is the whole fix. No second code path, because the
            // check already runs on every arm - what was broken was *when* the
            // loop got there: a dropped record meant the next iteration blocked in
            // `wait_stdin` for another full timeout first.
            Ok(_) => {}
            Err(_) => break,
        }

        if let Some(timeout) = buffer.pending_timeout_ms()
            && last_data.elapsed().as_millis() as u64 >= timeout
        {
            for event in buffer.flush() {
                let _ = dispatch(&shared, event, &mut |seq| write_stdout(seq));
            }
        }

        if let Ok(size) = sys::terminal_size()
            && size != last_size
        {
            last_size = size;
            shared.cols.store(size.0, Ordering::SeqCst);
            shared.rows.store(size.1, Ordering::SeqCst);
            shared.fire_resize();
        }
    }
    shared.reader_done.store(true, Ordering::SeqCst);
}

pub(crate) fn dispatch(shared: &Shared, event: StdinEvent, out: &mut dyn FnMut(&str)) -> bool {
    match event {
        StdinEvent::Data(seq) => {
            // Gh #213: OSC 7501 traffic is negotiation, never input -
            // the support reply arms auto-mode reporting, and unknown
            // future revisions are swallowed, not typed.
            if is_program_status_traffic(&seq) {
                if is_program_status_reply(&seq) {
                    shared.program_status.store(true, Ordering::SeqCst);
                }
                return true;
            }
            if apply_negotiation(shared, &seq, out) {
                return true;
            }
            shared.forward_input(seq);
            false
        }
        StdinEvent::Paste(content) => {
            shared.forward_input(format!("\x1b[200~{content}\x1b[201~"));
            false
        }
    }
}

/// Apply a negotiation reply, writing the fallback enable sequence when the
/// terminal is not Kitty-capable. Returns whether the sequence was consumed.
///
/// The invariant (io-reliability.md §4, R8(c)): **exactly one key
/// disambiguation layer is ever active** — a Kitty answer retires
/// `modifyOtherKeys`, and `modifyOtherKeys` is only enabled while Kitty
/// has not answered. `out` is the write sink so the transitions are
/// assertable in a test instead of only observable on a live terminal.
pub(crate) fn apply_negotiation(shared: &Shared, seq: &str, out: &mut dyn FnMut(&str)) -> bool {
    match parse_negotiation(seq) {
        Some(Negotiation::KittyFlags(flags)) => {
            if flags != 0 {
                if shared.modify_other_keys.swap(false, Ordering::SeqCst) {
                    out("\x1b[>4;0m");
                }
                // Gh #323 defect 2: the local cell is what the matcher reads, so
                // negotiation only has to reach it. No process-global write.
                shared.kitty.store(true, Ordering::SeqCst);
            } else if !shared.modify_other_keys.load(Ordering::SeqCst) {
                out("\x1b[>4;2m");
                shared.modify_other_keys.store(true, Ordering::SeqCst);
            }
            true
        }
        Some(Negotiation::DeviceAttributes) => {
            if !shared.kitty.load(Ordering::SeqCst)
                && !shared.modify_other_keys.load(Ordering::SeqCst)
            {
                out("\x1b[>4;2m");
                shared.modify_other_keys.store(true, Ordering::SeqCst);
            }
            // Gh #379 gap 1: the DA1 answer is **also** the sentinel that closes
            // the startup colour query, so it must reach the interface as well as
            // being consumed here. Swallowing it entirely meant the theme's query
            // window never closed: `close_theme_query` was unreachable on a real
            // terminal, which left every stray OSC 11 applying forever. Forwarded
            // as input *after* the negotiation work, so a consumer that ignores it
            // sees no behaviour change.
            shared.forward_input(seq.to_string());
            true
        }
        None => false,
    }
}

/// Whether an OSC 7501 report may flow (gh #213): forced off never,
/// forced on always, auto only after the support reply and never
/// under a stripping multiplexer.
///
/// Lives beside the reader (its only caller) since the gate-11 split; `terminal.rs`
/// re-exports it for `set_program_status`.
pub(crate) fn program_status_active(shared: &Shared) -> bool {
    match program_status_mode(env_var) {
        ProgramStatusMode::ForcedOff => false,
        ProgramStatusMode::ForcedOn => true,
        ProgramStatusMode::Auto => {
            !multiplexer_strips_osc(env_var) && shared.program_status.load(Ordering::SeqCst)
        }
    }
}
