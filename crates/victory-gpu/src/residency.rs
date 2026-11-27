//! Host-memory admission for speculative texture work and reusable upload storage.
//!
//! From codec-d/cool_fox (641835a4, fabb872a, c64fc9b4, 08e4574d, e94d277a; the
//! values of the last). Enabled automatically on hosts with at most 16 GiB
//! of physical RAM, or explicitly with `VICTORY_RESIDENCY=1` ([`on`]); the memory
//! of the system is then asked once a second; below 3 GiB of free RAM or 4 GiB
//! of commit (out again above 5 / 6) texture prefetch is limited to 64 MiB a
//! frame and idle scratch and staging blocks are let go; below 1.5 GiB of
//! commit to 16 MiB; the texture pass stops below 512 MiB of commit, new
//! staging blocks for textures picked by index below 1 GiB. With 32 GiB of RAM
//! or less (or `VICTORY_RESIDENCY_LIMITED=1`) the budgets of a smaller PC.
//! Off, nothing here changes anything, except that for 500 ms after the card
//! refused memory (E_OUTOFMEMORY) the texture pass stops ([`upload_refused`]).
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const MIB: u64 = 1 << 20;

/// Automatically use memory admission on hosts with at most 16 GiB of
/// physical RAM. Explicit settings retain their previous meaning.
fn residency_default(setting: Option<&str>, total_phys: Option<u64>) -> bool {
    match setting {
        Some(value) => value == "1",
        None => total_phys.is_some_and(|bytes| bytes != 0 && bytes <= 16 * GIB),
    }
}

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let setting = std::env::var_os("VICTORY_RESIDENCY");
        let enabled = match setting.as_deref() {
            Some(value) => value.to_str().is_some_and(|v| residency_default(Some(v), None)),
            None => residency_default(None, memory_status().map(|s| s.ullTotalPhys)),
        };
        tracing::info!(enabled, automatic = setting.is_none(), "host memory admission policy");
        enabled
    })
}
const GIB: u64 = 1 << 30;
static PRESSURED: AtomicBool = AtomicBool::new(false);
static HEADROOM: AtomicU64 = AtomicU64::new(u64::MAX);

fn memory_status() -> Option<MEMORYSTATUSEX> {
    // SAFETY: the API initializes the fields of a correctly sized structure.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status)
}

// The texture sweep may run for seconds inside a single submitted frame.
static NEXT_COMMIT_SAMPLE: AtomicU64 = AtomicU64::new(0);
static AVAILABLE_COMMIT: AtomicU64 = AtomicU64::new(u64::MAX);
static RETRY_UPLOAD_AT: AtomicU64 = AtomicU64::new(0);

/// Shared and upload heaps both consume host commit. Keep per-class totals
/// while emitting at most one OOM warning per second for each class.
pub(crate) struct BufferFailures {
    next_log: AtomicU64,
    oom: AtomicU64,
}

impl BufferFailures {
    pub(crate) const fn new() -> Self {
        Self { next_log: AtomicU64::new(0), oom: AtomicU64::new(0) }
    }

    fn record_oom(&self, now: u64) -> Option<u64> {
        let total = self.oom.fetch_add(1, Ordering::Relaxed) + 1;
        let next = self.next_log.load(Ordering::Relaxed);
        (now >= next
            && self.next_log.compare_exchange(next, now.saturating_add(1000),
                Ordering::Relaxed, Ordering::Relaxed).is_ok()).then_some(total)
    }

    pub(crate) fn report(&self, kind: &'static str, error: &windows::core::Error, size: u64) {
        if error.code() != windows::Win32::Foundation::E_OUTOFMEMORY {
            tracing::warn!(kind, %error, size, "host buffer allocation failed");
            return;
        }
        let now = crate::host::coarse_ms();
        allocation_error(error);
        OOM_BY_CLASS[usize::from(kind == "shared")].fetch_add(1, Ordering::Relaxed);
        SIZE_COOLDOWN.note(size, now);
        if let Some(oom_total) = self.record_oom(now) {
            tracing::warn!(kind, %error, size, oom_total,
                fast_fails = fast_fails(),
                commit_available_mib = sample_commit(now) >> 20, "host buffer allocation failed");
        }
    }
}

/// How long a size whose allocation was refused is not asked for again.
/// Long enough to stop a burst of thousands of failed driver calls, short
/// enough that required work resumes on its own next frame.
const ALLOCATION_COOLDOWN_MS: u64 = 500;

/// A refused allocation is retried by size, not by caller. Sizes are bucketed
/// by powers of two: the critical log repeated 4 MiB shared buffers and
/// 256 KiB upload buffers, while unrelated small allocations still had to be
/// allowed through.
struct SizeCooldown {
    until: Mutex<BTreeMap<u64, u64>>,
    fast_fails: AtomicU64,
}

impl SizeCooldown {
    const fn new() -> Self {
        Self { until: Mutex::new(BTreeMap::new()), fast_fails: AtomicU64::new(0) }
    }

    fn bucket(size: u64) -> u64 {
        size.max(1).checked_next_power_of_two().unwrap_or(u64::MAX)
    }

    fn note(&self, size: u64, now: u64) {
        let mut until = self.until.lock().unwrap_or_else(|e| e.into_inner());
        // A run with many different refused sizes must not grow this map
        // without bound. Expired entries are of no use to anyone.
        until.retain(|_, deadline| *deadline > now);
        until.insert(Self::bucket(size), now.saturating_add(ALLOCATION_COOLDOWN_MS));
    }

    fn is_throttled(&self, size: u64, now: u64) -> bool {
        self.until
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&Self::bucket(size))
            .is_some_and(|&deadline| now < deadline)
    }
}

static SIZE_COOLDOWN: SizeCooldown = SizeCooldown::new();

/// Refusals by heap, for the diagnostics memory line: upload then shared.
static OOM_BY_CLASS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

#[cfg(feature = "diagnostics")]
pub(crate) fn oom_counts() -> [u64; 2] {
    [OOM_BY_CLASS[0].load(Ordering::Relaxed), OOM_BY_CLASS[1].load(Ordering::Relaxed)]
}

/// True when an allocation of this size failed recently and the caller
/// should defer it instead of asking the driver again. Only the refused size
/// class is held back; a different size still reaches D3D12.
pub(crate) fn throttled(size: u64) -> bool {
    let hit = SIZE_COOLDOWN.is_throttled(size, crate::host::coarse_ms());
    if hit {
        let n = SIZE_COOLDOWN.fast_fails.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            tracing::warn!(size, skipped = n, "skipping an allocation refused moments ago");
        }
    }
    hit
}

fn fast_fails() -> u64 {
    SIZE_COOLDOWN.fast_fails.load(Ordering::Relaxed)
}

/// Allocations deferred without another driver call because their size
/// class was refused in the last [`ALLOCATION_COOLDOWN_MS`].
#[cfg(feature = "diagnostics")]
pub(crate) fn skipped_while_cooling() -> u64 {
    fast_fails()
}

pub(crate) fn allocation_error(error: &windows::core::Error) {
    if error.code() == windows::Win32::Foundation::E_OUTOFMEMORY {
        RETRY_UPLOAD_AT.fetch_max(crate::host::coarse_ms().saturating_add(500), Ordering::Relaxed);
    }
}

thread_local! {
    static STAGING_DEFERRED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn note_staging_deferred() {
    STAGING_DEFERRED.set(true);
}

pub(crate) fn take_staging_deferred() -> bool {
    STAGING_DEFERRED.replace(false)
}

pub(crate) fn upload_refused() -> bool {
    crate::host::coarse_ms() < RETRY_UPLOAD_AT.load(Ordering::Relaxed)
}

fn sample_commit(now: u64) -> u64 {
    let next = NEXT_COMMIT_SAMPLE.load(Ordering::Relaxed);
    if now >= next
        && NEXT_COMMIT_SAMPLE
            .compare_exchange(next, now.saturating_add(32), Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        && let Some(status) = memory_status()
    {
        AVAILABLE_COMMIT.store(status.ullAvailPageFile, Ordering::Relaxed);
    }
    AVAILABLE_COMMIT.load(Ordering::Relaxed)
}

fn has_upload_reserve(commit: u64, bake: bool) -> bool {
    // Keep a protective floor against Rust/game OOM aborts without starving
    // bindless world textures or one-shot ped decoration bakes when steady-state
    // commit headroom sits in the 1-4 GiB range on 32 GiB hosts.
    commit >= if bake { 256 * MIB } else { 512 * MIB }
}

fn has_growth_reserve(commit: u64, bake: bool) -> bool {
    // Growing a fresh 64 MiB upload block for speculative bindless prefetch
    // requires more headroom than reusing slices already committed inside the
    // staging pool.
    commit >= if bake { 256 * MIB } else { 1024 * MIB }
}

/// Leave commit for game allocations and submissions already in flight while
/// still letting bindless textures and one-shot material bakes upload.
pub(crate) fn admit_texture_pass(bake: bool) -> bool {
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    if !on() {
        return true;
    }
    has_upload_reserve(sample_commit(now), bake)
}

/// Whether the staging allocator may commit a brand-new OS upload buffer
/// (reusing existing free ranges in already-committed pool blocks stays open
/// whenever `admit_texture_pass` is true).
pub(crate) fn allow_staging_growth(bake: bool) -> bool {
    if !on() {
        return true;
    }
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    has_growth_reserve(sample_commit(now), bake)
}

pub(crate) fn limited_host_ram() -> bool {
    static LIMITED: OnceLock<bool> = OnceLock::new();
    *LIMITED.get_or_init(|| {
        on() && (std::env::var_os("VICTORY_RESIDENCY_LIMITED").is_some_and(|v| v == "1")
            || memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB))
    })
}

fn next_pressure(was: bool, physical: u64, commit: u64) -> bool {
    if was {
        physical < 5 * GIB || commit < 6 * GIB
    } else {
        physical <= 3 * GIB || commit <= 4 * GIB
    }
}

/// Called by the once-per-second submission sweep. A failed sample keeps the last good state.
pub(crate) fn refresh() {
    if !on() {
        return;
    }
    let Some(status) = memory_status() else { return };
    let (physical, commit) = (status.ullAvailPhys, status.ullAvailPageFile);
    HEADROOM.store(physical.min(commit), Ordering::Relaxed);
    let was = pressured();
    let pressure = next_pressure(was, physical, commit);
    if pressure != was {
        PRESSURED.store(pressure, Ordering::Relaxed);
        if pressure {
            tracing::warn!(
                pid = std::process::id(),
                frame = crate::frame::current(),
                physical_free_mib = physical >> 20,
                available_commit_mib = commit >> 20,
                buffer_mirror_span_mib = crate::shadow::live_span_bytes() >> 20,
                texture_staging_mib = crate::texture::live_staging_bytes() >> 20,
                spare_scratch_mib = crate::host::ALIVE[4].load(Ordering::Relaxed) >> 20,
                game_filled_gpu_texture_mib = crate::resource::KINDS[1].load(Ordering::Relaxed) >> 20,
                "host memory pressure: limiting texture prefetch and idle upload storage"
            );
        } else {
            tracing::info!(physical_free_mib = physical >> 20, available_commit_mib = commit >> 20, "host memory pressure cleared");
        }
    }
}

pub(crate) fn headroom() -> Option<u64> {
    let bytes = HEADROOM.load(Ordering::Relaxed);
    (bytes != u64::MAX).then_some(bytes)
}

pub(crate) fn pressured() -> bool {
    PRESSURED.load(Ordering::Relaxed)
}

/// True when OS commit headroom is critically low (< 1.5 GiB) or an allocation
/// just failed with `E_OUTOFMEMORY`, requiring emergency throttling.
pub(crate) fn critical_commit() -> bool {
    if !on() {
        return false;
    }
    let now = crate::host::coarse_ms();
    now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) || sample_commit(now) < 1536 * MIB
}

/// A soft per-frame admission limit. A single texture can cross it. The
/// queue retains unserved slots so they can be retried on later frames.
pub(crate) fn prefetch_limit(bytes: u64, pressure: bool, critical: bool, limited: bool, staging: u64) -> u64 {
    if critical {
        bytes.min(16 << 20)
    } else if pressure || (limited && staging >= GIB) {
        bytes.min(64 << 20)
    } else {
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_size_class_is_held_back_without_blocking_other_sizes() {
        let cooldown = SizeCooldown::new();
        cooldown.note(4 << 20, 10_000);
        assert!(cooldown.is_throttled(4 << 20, 10_499));
        assert!(cooldown.is_throttled((4 << 20) - 1, 10_499));
        assert!(!cooldown.is_throttled(256 << 10, 10_499));
        assert!(!cooldown.is_throttled(4 << 20, 10_500));
        let count = SIZE_COOLDOWN.fast_fails.load(Ordering::Relaxed);
        assert_eq!(fast_fails(), count);
    }

    #[test]
    fn buffer_oom_burst_counts_every_failure_and_limits_diagnostics() {
        let failures = BufferFailures::new();
        assert_eq!(failures.record_oom(0), Some(1));
        for _ in 0..10_000 {
            assert_eq!(failures.record_oom(500), None);
        }
        assert_eq!(failures.record_oom(999), None);
        assert_eq!(failures.record_oom(1000), Some(10_003));
    }

    #[test]
    fn automatic_admission_targets_small_hosts_and_respects_explicit_switches() {
        assert!(residency_default(None, Some(16 * GIB)));
        assert!(residency_default(None, Some(8 * GIB)));
        assert!(!residency_default(None, Some(16 * GIB + 1)));
        assert!(!residency_default(None, Some(32 * GIB)));
        assert!(!residency_default(None, None));
        assert!(!residency_default(None, Some(0)));
        assert!(!residency_default(Some("0"), Some(16 * GIB)));
        assert!(residency_default(Some("1"), Some(64 * GIB)));
    }

    #[test]
    fn bake_and_lookahead_leave_room_for_live_game_allocations() {
        for (commit, bake, lookahead, growth) in [
            (128 * MIB, false, false, false),
            (256 * MIB, true, false, false),
            (512 * MIB, true, true, false),
            (768 * MIB, true, true, false),
            (1024 * MIB, true, true, true),
        ] {
            assert_eq!(has_upload_reserve(commit, true), bake);
            assert_eq!(has_upload_reserve(commit, false), lookahead);
            assert_eq!(has_growth_reserve(commit, false), growth);
        }
    }

    #[test]
    fn either_constraint_enters_pressure_and_both_must_recover() {
        assert!(next_pressure(false, 12 * GIB, 4 * GIB));
        assert!(next_pressure(false, 3 * GIB, 16 * GIB));
        assert!(next_pressure(true, 4 * GIB, 7 * GIB));
        assert!(next_pressure(true, 6 * GIB, 5 * GIB));
        assert!(!next_pressure(true, 5 * GIB, 6 * GIB));
        assert!(!next_pressure(false, 4 * GIB, 5 * GIB));
    }

    #[test]
    fn prefetch_respects_existing_budgets_and_resumes_after_recovery() {
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, true, GIB - 1), 512 << 20);
        assert_eq!(prefetch_limit(512 << 20, false, false, false, 2 * GIB), 512 << 20);
        assert_eq!(prefetch_limit(0, true, true, false, 0), 0);
        assert_eq!(prefetch_limit(8 << 20, true, true, false, 0), 8 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, false, false, 0), 64 << 20);
        assert_eq!(prefetch_limit(512 << 20, true, true, false, 0), 16 << 20);
        assert_eq!(prefetch_limit(192 << 20, false, false, false, 0), 192 << 20);
    }
}
