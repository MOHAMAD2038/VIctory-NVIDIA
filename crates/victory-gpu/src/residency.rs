//! Host-memory admission for speculative texture work and reusable upload storage.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

use crate::low_vram::Ram;

const MIB: u64 = 1 << 20;
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
/// Free physical memory at the last sample (`u64::MAX` until the first: a
/// machine with none free has never been seen to report that).
static AVAILABLE_PHYSICAL: AtomicU64 = AtomicU64::new(u64::MAX);
static RETRY_UPLOAD_AT: AtomicU64 = AtomicU64::new(0);

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
        AVAILABLE_PHYSICAL.store(status.ullAvailPhys, Ordering::Relaxed);
    }
    AVAILABLE_COMMIT.load(Ordering::Relaxed)
}

fn has_upload_reserve(commit: u64, bake: bool, ram: Ram) -> bool {
    // Keep a protective floor against Rust/game OOM aborts without starving
    // bindless world textures or one-shot ped decoration bakes when steady-state
    // commit headroom sits in the 1-4 GiB range on 32 GiB hosts. A 16 GB PC's
    // free commit sits lower through the streaming, so there the floor is half
    // (`crate::low_vram::upload_reserve`): the uploads that matter still go.
    commit >= crate::low_vram::upload_reserve(ram, bake)
}

fn has_growth_reserve(commit: u64, bake: bool, ram: Ram) -> bool {
    // Growing a fresh 64 MiB upload block for speculative bindless prefetch
    // requires more headroom than reusing slices already committed inside the
    // staging pool.
    commit >= crate::low_vram::growth_reserve(ram, bake)
}

/// Leave commit for game allocations and submissions already in flight while
/// still letting bindless textures and one-shot material bakes upload.
pub(crate) fn admit_texture_pass(bake: bool) -> bool {
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    has_upload_reserve(sample_commit(now), bake, crate::low_vram::ram())
}

/// Whether the staging allocator may commit a brand-new OS upload buffer
/// (reusing existing free ranges in already-committed pool blocks stays open
/// whenever `admit_texture_pass` is true).
pub(crate) fn allow_staging_growth(bake: bool) -> bool {
    let now = crate::host::coarse_ms();
    if now < RETRY_UPLOAD_AT.load(Ordering::Relaxed) {
        return false;
    }
    has_growth_reserve(sample_commit(now), bake, crate::low_vram::ram())
}

pub(crate) fn limited_host_ram() -> bool {
    static LIMITED: OnceLock<bool> = OnceLock::new();
    *LIMITED.get_or_init(|| memory_status().is_some_and(|s| s.ullTotalPhys <= 32 * GIB))
}

fn next_pressure(was: bool, physical: u64, commit: u64) -> bool {
    next_pressure_at(was, physical, commit, pressure_lines(crate::low_vram::ram()))
}

/// Where pressure starts and ends: the free physical memory and the commit left
/// to the machine when it is entered, and the two it recovers above.
fn pressure_lines(ram: Ram) -> (u64, u64, u64, u64) {
    match ram {
        // A 16 GB PC keeps two or three gigabytes of physical memory free for
        // the whole of a session: the title fills the rest and Windows trims
        // working sets by itself. The 32 GB machines' 3 GiB line therefore sat
        // under the free memory of a whole run there (2026-11-27 21:31, frame
        // 1842: 2,719 MiB free against 10,277 MiB of commit, nothing paging),
        // so the layer was in pressure from the first minute on and its
        // prefetch and staging stayed at the slowest — which is one of the
        // ways a world that is streaming in waits for room.
        Ram::Tight => (2 * GIB, 3584 * MIB, 3 * GIB, 5 * GIB),
        Ram::Roomy | Ram::Limited => (3 * GIB, 5 * GIB, 4 * GIB, 6 * GIB),
    }
}

fn next_pressure_at(was: bool, physical: u64, commit: u64, lines: (u64, u64, u64, u64)) -> bool {
    let (enter_physical, keep_physical, enter_commit, keep_commit) = lines;
    if was {
        physical < keep_physical || commit < keep_commit
    } else {
        physical <= enter_physical || commit <= enter_commit
    }
}

/// Called by the once-per-second submission sweep. A failed sample keeps the last good state.
pub(crate) fn refresh() {
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

/// True when OS commit headroom is critically low (< 1.5 GiB), free physical
/// memory has fallen under [`crate::low_vram::PHYSICAL_FLOOR`] (768 MiB — past
/// it Windows pages out what is in use, so nothing speculative may take more),
/// or an allocation just failed with `E_OUTOFMEMORY`: emergency throttling.
pub(crate) fn critical_commit() -> bool {
    let now = crate::host::coarse_ms();
    now < RETRY_UPLOAD_AT.load(Ordering::Relaxed)
        || sample_commit(now) < 1536 * MIB
        || AVAILABLE_PHYSICAL.load(Ordering::Relaxed) < crate::low_vram::PHYSICAL_FLOOR
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

    /// The reserves of the machines this fork was measured on (32 GB and up:
    /// `Ram::Limited`'s numbers, which `Ram::Roomy` shares), and the halves a
    /// 16 GB PC keeps instead — its free commit sits lower through the
    /// streaming, and waiting for the big machines' floor would starve the
    /// uploads the world needs.
    #[test]
    fn bake_and_lookahead_leave_room_for_live_game_allocations() {
        for (commit, bake, lookahead, growth) in [
            (128 * MIB, false, false, false),
            (256 * MIB, true, false, false),
            (512 * MIB, true, true, false),
            (768 * MIB, true, true, false),
            (1024 * MIB, true, true, true),
        ] {
            assert_eq!(has_upload_reserve(commit, true, Ram::Limited), bake);
            assert_eq!(has_upload_reserve(commit, false, Ram::Limited), lookahead);
            assert_eq!(has_growth_reserve(commit, false, Ram::Limited), growth);
            // Roomy shares the limited numbers: only the tight tier halves them.
            assert_eq!(has_upload_reserve(commit, true, Ram::Roomy), bake);
            assert_eq!(has_growth_reserve(commit, false, Ram::Roomy), growth);
        }
        for (commit, bake, lookahead, growth) in [
            (128 * MIB, true, false, false),
            (256 * MIB, true, true, false),
            (512 * MIB, true, true, true),
            (768 * MIB, true, true, true),
        ] {
            assert_eq!(has_upload_reserve(commit, true, Ram::Tight), bake, "{commit}");
            assert_eq!(has_upload_reserve(commit, false, Ram::Tight), lookahead, "{commit}");
            assert_eq!(has_growth_reserve(commit, false, Ram::Tight), growth, "{commit}");
        }
    }

    #[test]
    fn either_constraint_enters_pressure_and_both_must_recover() {
        let big = pressure_lines(Ram::Limited);
        assert!(next_pressure_at(false, 12 * GIB, 4 * GIB, big));
        assert!(next_pressure_at(false, 3 * GIB, 16 * GIB, big));
        assert!(next_pressure_at(true, 4 * GIB, 7 * GIB, big));
        assert!(next_pressure_at(true, 6 * GIB, 5 * GIB, big));
        assert!(!next_pressure_at(true, 5 * GIB, 6 * GIB, big));
        assert!(!next_pressure_at(false, 4 * GIB, 5 * GIB, big));
    }

    #[test]
    fn a_16_gb_pc_is_not_pressured_at_the_memory_it_lives_in() {
        let tight = pressure_lines(Ram::Tight);
        // The run of 2026-11-27 21:31, frame 1842, to the megabyte.
        assert!(!next_pressure_at(false, 2719 * MIB, 10277 * MIB, tight));
        assert!(next_pressure_at(false, 2048 * MIB, 10277 * MIB, tight));
        assert!(next_pressure_at(false, 8 * GIB, 3 * GIB, tight));
        // It does stay in pressure while physical memory recovers to 3.5 GiB.
        assert!(next_pressure_at(true, 3400 * MIB, 8 * GIB, tight));
        assert!(!next_pressure_at(true, 3584 * MIB, 8 * GIB, tight));
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
