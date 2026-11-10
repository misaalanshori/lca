//! The harness's load allowance, in one place (gh #589).
//!
//! The wait budget every real-terminal row rides is scaled by a load factor, and
//! that factor used to be Linux-only: `load_factor()` read `/proc/loadavg` and
//! defaulted the absent file to zero, i.e. **1x** on macOS and Windows — the two
//! platforms whose hosted runners are slowest, while `docs/testing-plan.md` §14
//! promised the allowance on all three. This module is the whole rule: the two
//! pure classifications, the portable probe that feeds the one a platform with no
//! load average needs, and the per-process cache.
//!
//! The split is not cosmetic. `crates/lca-cli/tests/common/mod.rs` was 1,152 lines
//! and this change took it past gate 11's ceiling; more to the point, the *rule*
//! is what a row can test on a platform it is not running on, so it lives where a
//! reader looks for it and where its comment has room for the measurements.

#[cfg(unix)]
use std::sync::atomic::AtomicU64;
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// The portable scheduling signal: how far a fixed sleep overran (gh #589).
///
/// Three samples of 20 ms, taking the **smallest** overshoot — `min` on purpose,
/// and the reasoning is worth stating because it is the opposite of the intuitive
/// choice.
///
/// A signal that took the *mean* would answer "how busy is this box on average",
/// which the load average already answers and which a busy-but-responsive box
/// deserves a 1x for. What the harness needs to know is whether the scheduler
/// **reliably** hands a sleeping thread its slice: a pane's first paint is one
/// long wait, and what hurts it is systematic delay, not an occasional hiccup.
/// Taking the minimum asks exactly that - "even the best of three was late" - so
/// the factor only rises when lateness is the norm.
///
/// # Evidence, measured and cited rather than assumed
///
/// On this Linux box the minimum stays **1x even at load 37**: CFS gives a
/// sleeping task its slice as soon as one is free, so 20 ms sleeps land within a
/// fraction of a millisecond (min-of-3: 82-132 us over 20-odd trials at idle, and
/// still 1x with 16 and then 32 spinners running). That is a *weakness of the
/// signal here* and it is deliberately harmless: Linux never takes this branch,
/// because `/proc/loadavg` answers first and costs nothing.
///
/// The branch exists for platforms where the delay is **systematic**, and the
/// evidence for that is recorded rather than invented: gh #588 measured a macOS
/// hosted runner where a thread that slept 5 ms and then sent a wake returned to
/// its reader at **55.227 ms** — one 60-80 ms quantum of scheduling delay
/// (`docs/platform-notes.md`'s recording). A 20 ms sleep under that regime
/// overshoots by most of a quantum, which classifies as 2-4x here. On an idle
/// Mac it would land on time and stay 1x, which is the correct answer there too.
///
/// This is not a perfect instrument and it is not meant to be: it only has to tell
/// "nothing is waiting" from "a thread is waiting for the CPU", and it needs no
/// platform API to do it.
#[cfg(unix)]
pub(super) fn portable_overshoot() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..PROBE_SAMPLES {
        let started = std::time::Instant::now();
        std::thread::sleep(PROBE_SLEEP);
        let elapsed = started.elapsed();
        best = best.min(elapsed.saturating_sub(PROBE_SLEEP));
    }
    best
}

/// The rule that picks between the signals, as a pure function (gh #589).
///
/// `None` means "this platform publishes no load average" - which is macOS and
/// Windows - and that must never collapse to 1x, because 1x is exactly the
/// defect: the allowance described in `docs/testing-plan.md` §14 existed only on
/// the Linux dev box and not on the runners. Where there is a load average it is
/// used (Linux keeps the factor it always had, and pays nothing for the probe);
/// where there is not, the measured overshoot answers instead.
///
/// Pure on purpose: this is the branch a Linux box cannot exercise by running,
/// because `/proc/loadavg` is always there. A function over `(Option<f64>,
/// Duration)` is testable from a table on every platform.
pub(crate) fn factor_from(load: Option<f64>, overshoot: Duration) -> u32 {
    match load {
        Some(load) => factor_for_load(load),
        None => factor_for_overshoot(overshoot),
    }
}

/// The load-average → factor rule: 1.0x below light load, then +1x per four
/// points of load, capped so a pathological runner cannot extend a suite for
/// hours. Pure, so the rule is testable without depending on this box.
pub(crate) fn factor_for_load(load: f64) -> u32 {
    ((load / 4.0).floor() as u32 + 1).clamp(1, 4)
}

/// The overshoot → factor rule: 1x when a 20 ms sleep lands inside its own
/// quantum, +1x per further `PROBE_SLEEP` of overrun, capped at 4x.
///
/// The cap is the same 4x the load rule had, and its reason is the same: a
/// pathological runner must not extend a suite for hours. The unit is the
/// quantum the probe measured rather than "one core-ish of load", because on a
/// platform with no load average a quantum is the only thing left to count.
pub(crate) fn factor_for_overshoot(overshoot: Duration) -> u32 {
    let quantum = PROBE_SLEEP.as_millis().max(1);
    let quanta = overshoot.as_millis() / quantum;
    (1 + u32::try_from(quanta.min(3)).unwrap_or(3)).clamp(1, 4)
}

/// How many sleeps [`load_factor`](Tmux::load_factor)'s probe takes (`PROBE_SAMPLES`
/// straight-line sleeps of [`PROBE_SLEEP`], 60 ms in total).
#[cfg(unix)]
const PROBE_SAMPLES: usize = 3;

/// The sleep each probe sample asks for — and the unit the overshoot rule counts
/// in, which is why it is not behind `cfg(unix)`.
const PROBE_SLEEP: Duration = Duration::from_millis(20);

/// How many times `load_factor` was asked in this process. The probe itself runs
/// once (it is cached); this counts the callers, which is the receipt that the
/// cache is what keeps a probe per wait from happening.
#[cfg(unix)]
static LOAD_FACTOR_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The factor this process will use, sampled once (gh #589).
///
/// The probe is three sleeps of 20 ms (60 ms) and it runs **once per test
/// process**, cached in a `OnceLock`; `LCA_LOAD_STATS=1` prints the factor and how
/// many calls the cache collapsed. Every wait primitive in a binary calls this
/// function (`wait_for`, the settle loop, the layout wait, the input-ready probe,
/// and the probe's erase check), so a row with six waits would pay six probes
/// without the cache; with it, one. A test whose first wait is the probe's first
/// caller pays the 60 ms, which is deliberate: the expensive thing the allowance
/// exists for (a WASM component's first paint) happens early in a row, and a
/// factor measured later would be measuring the wrong moment. The cost of the
/// cache in the other direction is stated plainly: a suite that starts on an idle
/// box keeps factor 1 for its whole life even if the box loads up later.
#[cfg(unix)]
pub(super) fn cached_factor() -> u32 {
    static FACTOR: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *FACTOR.get_or_init(|| {
        LOAD_FACTOR_CALLS.fetch_add(1, Ordering::Relaxed);
        let load = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|text| text.split_whitespace().next()?.parse::<f64>().ok());
        // The probe runs only where it is needed: a platform with a load average
        // does not pay 60 ms for a signal it already has.
        let overshoot = match load {
            Some(_) => Duration::ZERO,
            None => portable_overshoot(),
        };
        let factor = factor_from(load, overshoot);
        // `LCA_LOAD_STATS=1` makes the allowance visible, the same way
        // `LCA_SETTLE_STATS` makes a settle's cost visible: a receipt that says
        // "the factor was 4 on that runner" has to come from the run itself, not
        // from the report.
        if std::env::var_os("LCA_LOAD_STATS").is_some() {
            let evidence = match load {
                Some(load) => format!("load average {load}"),
                None => format!("{overshoot:?} overshoot over {PROBE_SAMPLES} x {PROBE_SLEEP:?}"),
            };
            eprintln!(
                "load factor: {factor}x from {evidence} ({} call(s) served by this one probe)",
                LOAD_FACTOR_CALLS.load(Ordering::Relaxed)
            );
        }
        factor
    })
}

/// How many times the factor was asked for in this process (see
/// [`cached_factor`]). The cache's effect, made observable so a report can quote
/// a number instead of asserting "it is cheap".
#[cfg(unix)]
pub(super) fn calls() -> u64 {
    LOAD_FACTOR_CALLS.load(Ordering::Relaxed)
}
