//! The plan for PCs below this fork's usual size: 16 GB of RAM and a card of
//! 8 GB or less (`VICTORY_LOW_VRAM`).
//!
//! Everything in this layer was built and measured on 32 to 64 GB machines
//! with 12 to 24 GB cards. What it keeps "because it will be used again
//! soon" is a bet paid from the PC's memory: blocks of upload memory made
//! ahead, spare scratch chunks, snapshot buffers for the compares, host
//! copies of textures the title fills. On 64 GB those bets are free; on 16 GB
//! they decide between a frame that runs and one that waits for the disk
//! while Windows pages. The numbers below are the same rules the big
//! machines use with the bet smaller: nothing here changes what is
//! *correct*, only how much speculative memory is held, and how soon what
//! is idle goes.
//!
//! Three kinds of PC memory:
//!
//! | [`Ram`] | When | What changes |
//! |---|---|---|
//! | `Roomy` | over 32 GiB | as measured |
//! | `Limited` | 32 GiB or less | as the switches of 2026-11-27 have it |
//! | `Tight` | 18 GiB or less, or `VICTORY_LOW_VRAM=1` | the numbers below |
//!
//! `VICTORY_LOW_VRAM=1` puts any PC in the tight tier (a 32 GB PC with a
//! 6 GB card may want it); `VICTORY_LOW_VRAM=0` keeps the limited tier's
//! numbers on a 16 GB PC, for comparing one run against another.
//!
//! A card of 9 GiB or less has one rule of its own that does not wait for
//! the switch: [`card_headroom`], the bytes the texture queue keeps free on
//! it (the host's own allocations that are made with no room test do not
//! scale with the card).

use std::sync::OnceLock;

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// How much of the PC's memory a machine has, as the rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ram {
    /// Over 32 GiB: the machines this fork was measured on.
    Roomy,
    /// 32 GiB or less: what the switches call "limited RAM"
    /// (`residency::limited_host_ram`).
    Limited,
    /// 18 GiB or less (a 16 GB PC), or `VICTORY_LOW_VRAM=1`.
    Tight,
}

impl Ram {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Ram::Roomy => "roomy",
            Ram::Limited => "limited",
            Ram::Tight => "tight (16 GB PC)",
        }
    }
}

/// The tier `total` bytes of RAM and the switch make; `forced` is
/// `VICTORY_LOW_VRAM` (`Some(true)`: on, `Some(false)`: off).
fn tier_from(total: Option<u64>, forced: Option<bool>) -> Ram {
    if let Some(on) = forced {
        return if on { Ram::Tight } else { Ram::Limited };
    }
    match total {
        Some(total) if total <= 18 * GIB => Ram::Tight,
        Some(total) if total <= 32 * GIB => Ram::Limited,
        _ => Ram::Roomy,
    }
}

/// `VICTORY_LOW_VRAM=1` / `=0`; anything else: the PC's memory decides.
fn forced() -> Option<bool> {
    match std::env::var("VICTORY_LOW_VRAM").ok()?.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// The PC's physical memory in bytes, from `GlobalMemoryStatusEx`.
#[cfg(windows)]
fn total_physical() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
    ok.then_some(m.ullTotalPhys)
}

/// Not Windows: the tier then comes from the switch alone.
#[cfg(not(windows))]
fn total_physical() -> Option<u64> {
    None
}

/// The tier, read once and said in the log with the figure it was read from.
pub(crate) fn ram() -> Ram {
    static RAM: OnceLock<Ram> = OnceLock::new();
    *RAM.get_or_init(|| {
        let (total, asked) = (total_physical(), forced());
        let tier = tier_from(total, asked);
        let switch = match asked {
            Some(true) => "VICTORY_LOW_VRAM=1: on (VICTORY_LOW_VRAM=0: the PC's memory decides)",
            Some(false) => "VICTORY_LOW_VRAM=0: off, however much the PC has",
            None => "VICTORY_LOW_VRAM unset: the PC's memory decides",
        };
        tracing::info!(
            ram_gb = total.map(|b| b as f64 / GIB as f64),
            tier = tier.name(),
            switch,
            "how much of the PC's memory the layer plans for"
        );
        tier
    })
}

/// The same question for a start of the title (`victory play`), read from the
/// environment and the PC **without** the one-time cache [`ram`] keeps: the
/// front end asks before it clears and rewrites the environment, and its own
/// answer must not freeze what the layer reads afterwards.
pub fn small_pc() -> bool {
    tier_from(total_physical(), forced()) == Ram::Tight
}

/// Free commit a texture upload wants before it starts (and a one-shot
/// material bake, which cannot be retried later). As the switches of
/// 2026-11-27 have it: 512 MiB, 256 for a bake. On a 16 GB PC the free
/// commit hovers under a gigabyte while the world streams, so the floor is
/// half: the uploads that matter still go, and the ones that wait are the
/// speculative ones.
pub(crate) fn upload_reserve(ram: Ram, bake: bool) -> u64 {
    match (ram, bake) {
        (Ram::Tight, true) => 128 * MIB,
        (Ram::Tight, false) => 256 * MIB,
        (_, true) => 256 * MIB,
        (_, false) => 512 * MIB,
    }
}

/// The same for growing a brand-new 64 MiB block rather than reusing what a
/// block already holds (speculative bindless prefetch). 1 GiB and 256 MiB as
/// before; half on 16 GB.
pub(crate) fn growth_reserve(ram: Ram, bake: bool) -> u64 {
    match (ram, bake) {
        (Ram::Tight, true) => 128 * MIB,
        (Ram::Tight, false) => 512 * MIB,
        (_, true) => 256 * MIB,
        (_, false) => 1024 * MIB,
    }
}

/// Free physical memory below which nothing new is staged and the idle
/// storage is given back at the next sweep: 768 MiB. Past it Windows pages
/// out what is in use, and a frame waits on the disk instead of on the card.
pub(crate) const PHYSICAL_FLOOR: u64 = 768 * MIB;

/// Snapshot buffers kept for the compares (`shadow::snapshots_kept`): how
/// many, and how many bytes in all. 1,024 and 512 MiB as before; half of
/// each on 16 GB.
pub(crate) fn snapshots_kept(ram: Ram) -> (usize, usize) {
    match ram {
        Ram::Tight => (512, 256 << 20),
        _ => (1024, 512 << 20),
    }
}

/// Spare list-scratch bytes kept idle between frames (the launcher's spare
/// chunks, `host::give_chunks`): 128 MiB as before, 64 on 16 GB.
pub(crate) fn scratch_spare(ram: Ram) -> u64 {
    if ram == Ram::Tight { 64 * MIB } else { 128 * MIB }
}

/// The threads that translate lists (`VICTORY_THIN_THREADS`): eight, as
/// measured, while the machine has processors to spare; on 16 GB four less
/// than the PC has, at most twelve, because the title's own workers want the
/// same processors.
///
/// Measured on the 16 GB laptop of 2026-11-27 (16 logical processors): eight
/// against twelve threads left the work the same — 570 against 573 ms of
/// processor time a frame, the city 7.1 against 7.4 fps — but twelve held
/// 30 fps from frame 1,500 to 2,000, and the frames after that averaged 24.6
/// fps where eight were already at 4. Half the processors (eight there) was
/// the guess; four less does better. Four of the sixteen are left for the
/// title's five or six recording threads.
pub(crate) fn thin_threads(ram: Ram, processors: usize) -> usize {
    match ram {
        Ram::Tight => processors.saturating_sub(4).clamp(2, 12),
        _ => 8,
    }
}

/// The threads that look over the title's bytes under flights' writes
/// (`VICTORY_UNDER_THREADS`): six as before, four on 16 GB.
pub(crate) fn under_threads(ram: Ram) -> usize {
    if ram == Ram::Tight { 4 } else { 6 }
}

/// The bytes of the card's memory the texture queue keeps free when
/// `VICTORY_TEXTURE_HEADROOM_MB` is not set: 1 GiB on a card whose budget is
/// 9 GiB or less, none as before on a larger one.
///
/// An 8 GB card's WDDM budget is about 7.6 to 8.0 GiB, and the 90 % floor
/// leaves it ~780 MiB free — less than the host's own allocations that are
/// made with no room test (render targets, card copies of title buffers),
/// which do not scale down with the card. Past the budget Windows moves
/// card memory into the PC's, which is the paging this whole plan is
/// against. One GiB free costs the texture queue that much of a small card
/// and nothing at all of a large one.
pub(crate) fn card_headroom(card_budget: u64) -> u64 {
    if card_budget <= 9 * GIB { GIB } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_switch_wins_and_the_pc_decides_otherwise() {
        assert_eq!(tier_from(Some(16 * GIB), None), Ram::Tight);
        assert_eq!(tier_from(Some(18 * GIB), None), Ram::Tight);
        assert_eq!(tier_from(Some(24 * GIB), None), Ram::Limited);
        assert_eq!(tier_from(Some(32 * GIB), None), Ram::Limited);
        assert_eq!(tier_from(Some(64 * GIB), None), Ram::Roomy);
        assert_eq!(tier_from(None, None), Ram::Roomy);
        // The switch puts any PC in the tight tier, and `=0` keeps the
        // limited numbers on a 16 GB one.
        assert_eq!(tier_from(Some(64 * GIB), Some(true)), Ram::Tight);
        assert_eq!(tier_from(Some(16 * GIB), Some(false)), Ram::Limited);
        assert_eq!(tier_from(None, Some(false)), Ram::Limited);
    }

    #[test]
    fn the_reserves_are_half_on_sixteen_gigabytes() {
        assert_eq!(upload_reserve(Ram::Roomy, false), 512 * MIB);
        assert_eq!(upload_reserve(Ram::Limited, false), 512 * MIB);
        assert_eq!(upload_reserve(Ram::Limited, true), 256 * MIB);
        assert_eq!(upload_reserve(Ram::Tight, false), 256 * MIB);
        assert_eq!(upload_reserve(Ram::Tight, true), 128 * MIB);
        assert_eq!(growth_reserve(Ram::Limited, false), 1024 * MIB);
        assert_eq!(growth_reserve(Ram::Limited, true), 256 * MIB);
        assert_eq!(growth_reserve(Ram::Tight, false), 512 * MIB);
        assert_eq!(growth_reserve(Ram::Tight, true), 128 * MIB);
        // A bake always asks for the least: it cannot be retried later.
        assert!(upload_reserve(Ram::Tight, true) <= upload_reserve(Ram::Tight, false));
        assert!(growth_reserve(Ram::Tight, true) <= growth_reserve(Ram::Tight, false));
    }

    #[test]
    fn the_storage_kept_for_later_is_smaller_and_nothing_else_changes() {
        assert_eq!(snapshots_kept(Ram::Roomy), (1024, 512 << 20));
        assert_eq!(snapshots_kept(Ram::Tight), (512, 256 << 20));
        assert_eq!(scratch_spare(Ram::Limited), 128 * MIB);
        assert_eq!(scratch_spare(Ram::Tight), 64 * MIB);
    }

    #[test]
    fn the_threads_follow_the_processors_on_sixteen_gigabytes() {
        assert_eq!(thin_threads(Ram::Limited, 32), 8);
        assert_eq!(thin_threads(Ram::Tight, 4), 2);
        assert_eq!(thin_threads(Ram::Tight, 6), 2);
        assert_eq!(thin_threads(Ram::Tight, 8), 4);
        assert_eq!(thin_threads(Ram::Tight, 12), 8);
        assert_eq!(thin_threads(Ram::Tight, 16), 12);
        assert_eq!(thin_threads(Ram::Tight, 64), 12);
        assert!(thin_threads(Ram::Tight, 1) >= 1, "never no thread at all");
        assert_eq!(under_threads(Ram::Limited), 6);
        assert_eq!(under_threads(Ram::Tight), 4);
    }

    #[test]
    fn a_small_card_keeps_a_gigabyte_free_and_a_large_one_none() {
        // An 8 GB card's budget (7.6 to 8.0 GiB), a 6 GB one's, a 4 GB one's.
        for budget in [7_692 << 20, 8 * GIB, 6 * GIB, 4 * GIB, 9 * GIB] {
            assert_eq!(card_headroom(budget), GIB, "{budget:#x}");
        }
        // A 12 GB card (budget ~11.2 GiB) and larger: as before.
        for budget in [11_229 << 20, 12 * GIB, 16 * GIB, 24 * GIB] {
            assert_eq!(card_headroom(budget), 0, "{budget:#x}");
        }
        assert_eq!(card_headroom(0), GIB);
        // Never more than the card itself.
        assert!(card_headroom(GIB) <= GIB);
    }

    #[test]
    fn the_tier_is_one_of_the_three_and_its_name_says_which() {
        assert_eq!(Ram::Tight.name(), "tight (16 GB PC)");
        assert_ne!(Ram::Roomy, Ram::Limited);
        // The plan reads the PC once; this only checks it can be read at all.
        let _ = ram();
    }
}
