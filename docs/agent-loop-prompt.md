# A prompt for an agent that loops until the city runs at 30 fps

Copy everything under the line into the other agent. It is written for an agent that runs **on the
player's own Windows PC**, in the checkout of this repository, and that may build, launch, capture and
measure. If the agent cannot build (no MSVC), it is the wrong agent: it will only guess.

---

You are working on `VIctory-NVIDIA`, an emulator/layer that runs GTA VI's Xbox build on Windows through
D3D12. Your one objective: **the dense city at 30 real frames a second on this PC, with the picture it
has now.** You keep working, measuring, patching and re-measuring until that is true or 3 hours are
gone. Do not stop at the first idea, do not ask "shall I continue", do not summarise and quit while the
number is under 30.

## Definition of done (all three, measured, not claimed)

1. **A real run**: ≥ 30 fps held for ≥ 60 consecutive seconds in the dense city, frames counted, not
   generated.
2. **The same picture**: `-Low` as it is today — scene 1280×720 through the title's FidelityFX path
   (performance), DLSS on, same draw distance, HUD shown. Lowering any of that is not a win.
3. **Real frames only**: frame generation off (`VICTORY_FRAMEGEN` absent). A generated frame is not a
   frame. The F1 menu's counter and the log's `frame=` count are the truth.

Report the three numbers (city fps, `preparing per frame` ms, replay frame ms) every time you claim
progress. If you reach the time limit without 30 fps, say the best number you measured, what you tried
in a table, and what you would try next — but only after the full 3 hours of real attempts.

## Environment — verify before anything else, then never re-verify

- Repository: `D:\Gamma\VIctory-NVIDIA` (Windows, MSVC toolchain, `cargo build --release`; the first
  build takes 10-20 minutes, later ones seconds to minutes). `rust-toolchain`/`rustfmt.toml` in the root:
  max line width 100, `cargo fmt` before every commit, `cargo test` for the crates you touch.
- Title: `D:\GTA\GTA6.Xbox.v1.0.2.1-CYBERLEEK\game_scarlett_master.exe`.
- Launcher: `scripts\play-foxhound.ps1 -Exe '<that path>' -Low` (and `-SeriesS`, `-More @{K='V'}`).
- Logs: `%LOCALAPPDATA%\victory\logs\victory-nvidia-<yyyymmdd>-<hhmmss>.log`, one per run, ANSI
  colouring to strip. Cache and pipelines: `%LOCALAPPDATA%\victory\cache`. Saves (never touch):
  `%LOCALAPPDATA%\victory\data\7952987A\storage`.
- Pipelines must be warm before measuring: `victory preload 'D:\GTA\GTA6.Xbox.v1.0.2.1-CYBERLEEK'`
  (with `$env:VICTORY_HOST_MAX_WAVE='32'`). Without it the first minutes are compilation, not the city.
- Diagnostics build for captures: `cargo build --locked --profile diag -p victory --features diagnostics`
  (see `docs/replay.md`).
- Only one loader at a time. `setx` only affects new windows. The game is the user's; do not modify
  anything inside the title's folder.

If `cargo`, the title or `%LOCALAPPDATA%\victory` is missing, say exactly what is missing and stop.

## The baseline — measured on this PC on 2026-11-27, do not spend an hour rediscovering it

Full numbers: `docs/findings.md`, section "A 16 GB laptop (RTX 4050, 2026-11-27): where it stops".
RTX 4050 Laptop, budget 5,152 MB of card memory, 16 GB of RAM, 16 logical processors, 2560×1440.

| where | number |
|---|---|
| opening, `-Low`, pipelines warm | 30 fps to frame ~2,000 |
| **dense city** | **5-11 fps**, frame 135-190 ms |
| the submitting thread, per frame | 98-124 ms (50-57 of them in the lists' steps) — **one thread** |
| the card, per frame | 63-78 % busy, 105-120 ms |
| list calls translated, per frame | 25,000-30,500, 570 ms of processor time on 8-12 threads |
| cost of one call | ~19 µs here (3.5 µs on the 24-thread reference PC) |
| free card memory at the line | 0-800 MB of 5,152, pressure lines fire at ~1.5 GB of free RAM |

Already measured and **dead** — do not retry these:

- **Resolution**: `-SeriesS` (640×360, sixteen times fewer pixels) left the card's time (105→120 ms) and
  the translation (573→575 ms) unchanged. The wall is per command, not per pixel.
- **Deduplicating list calls**: *one* call a frame repeats the one before it, of 30,000.
- **Finding one hot method**: the most frequent is 7 % of the calls (`graphics_table` 2,137-3,668,
  `compute_table` 2,029-2,637, `execute_indirect` 1,717-2,241, `set_pipeline` 1,406-2,076, then the two
  view and two constants families near 1,000).
- **More translating threads**: 8 against 12 of the 16 left the work and the city the same (7.1 against
  7.4 fps); the tight tier already asks for `processors - 4`.

So 30 fps means ~4-5× off *both* walls: the serial 98-124 ms of the submitting thread and the
105-120 ms of card time. Getting one of them to 33 ms is not enough. Plan for both.

## The loop — one experiment every 20-25 minutes, measured, kept or reverted

**Card side: no human needed.** Record one dense-city frame once, then replay it in seconds, forever:

```powershell
# once, with the diagnostics build; the user drives to the city, then either
#   - set VICTORY_RECORD_AT=f2400 (a frame in the city, from a previous run's log), or
#   - create the trigger file: <folder>\go
$env:VICTORY_RECORD='D:\captures\city'
# then, as often as you like, in seconds:
.\target\diag\victory.exe replay 'D:\captures\city' --loops 7 --timing detailed --top 20
```

The median of the loops is the frame's card time; `--timing detailed` gives sums by kind and the
costliest pipelines. `--compare` runs an `--skip-*` / `--merge-barriers` / `--batch-clamps` experiment
against the unchanged frame and prints the difference — use it to attribute card time before touching
code. Read `notes.txt`: anything that could not be recorded is listed there.

**Processor side: needs real runs.** Each run is ~4 minutes and needs the user at the keyboard to reach
the city. Ask for **one** run at a time, with the exact command, then parse the log yourself:

```python
# python3 measure.py <log> — the numbers below is what a run must report
import re, statistics, datetime
t = re.sub(r'\x1b\[[0-9;]*m', '', open(log, encoding='utf-8', errors='replace').read())
lines = t.split('\n')
def stamp(l):
    m = re.match(r'\s*(2026-\d\d-\d\dT\d\d:\d\d:\d\d\.\d+)Z', l)
    return datetime.datetime.strptime(m.group(1), '%Y-%m-%dT%H:%M:%S.%f') if m else None
frame = {int(m.group(1)): stamp(l) for l in lines if (m := re.search(r'\bframe=(\d+)', l)) and stamp(l)}
order = sorted(frame)
fps = [(b, (b - a) / (frame[b] - frame[a]).total_seconds())
       for a, b in zip(order, order[1:]) if (frame[b] - frame[a]).total_seconds() > 0]
print('city fps (last 200 frames):', statistics.mean([v for f, v in fps if f > order[-1] - 200]))
def mean(pat, n=20):
    got = [(float(m.group(1)), float(m.group(2))) for l in lines
           if (m := re.search(pat, l))]
    return [statistics.mean(c[i] for c in got[-n:]) for i in (0, 1)] if got else None
print('calls and ms   ', mean(r'list calls translated by threads of ours per frame \(VICTORY_THIN\): (\d+) in ([\d.]+) ms'))
print('preparing ms   ', mean(r"preparing per frame, the submitting thread: ([\d.]+) ms in its submissions.*?([\d.]+) in the lists' steps"))
print('card busy/queued', mean(r'nothing of ours queued ([\d.]+) ms \(the card about (\d+)% busy'))
```

Compare like with like: the same stretch of city, the same switches. `VICTORY_THIN_KINDS=1` adds an
atomic per call — diagnostic runs only, never in the run you time. One run per change, one change per
run, and write down the numbers even when they are worse (especially then).

**The record of the loop** goes in `docs/findings.md`, one section per attempt, in this repository's
voice: the hypothesis, the change, the number before and after, and the verdict ("measured and kept" /
"measured and not worth it"). No paragraph without a number.

## Where the time is, and the ranked ideas

Read `docs/findings.md`'s "what is next" and `docs/arch.md` first; both list what the authors already
think is worth doing. In order of what the numbers above suggest:

1. **The submitting thread, 98-124 ms alone.** What one submission does is serial: the layer's own
   bookkeeping (descriptor tables, barriers, clamps, copies) is done by the thread that submits. Split
   it: everything that does not depend on the previous call can be prepared ahead by another thread, as
   the list translation already is (`crates/victory-gpu/src/list/thin.rs`). This is the authors' own
   number one. It is the biggest single win and the hardest: expect the ordering rules (barriers,
   argument clamps, what a dispatch writes) to be the whole problem. `crates/victory-gpu/src/draw.rs`'s
   comments on `VICTORY_INDIRECT_BATCH` explain what may be reordered and what may not.
2. **19 µs a call, against 3.5 µs on a 24-thread PC.** Find where it goes: a cache miss per call? an
   atomic? a lock? a `Vec` growth? a `HashMap` lookup keyed by a string? Measure before changing
   (`perf` is not available on Windows MSVC by default; use counters printed every second, as the
   layer's own diagnostics do, and `--timing detailed` for the card side).
3. **Descriptor tables: 2,137-3,668 `graphics_table` and 2,029-2,637 `compute_table` a frame.** If the
   same table is set again with the same contents, the layer may be able to elide it — the
   de-duplication of *calls* is dead (one repeat a frame), but the de-duplication of the *contents* of a
   table is not the same thing, and the host's root-signature write is where the microseconds are.
4. **`set_pipeline`: 1,406-2,076 a frame.** The title switches pipeline state constantly; the host's
   PSO switch has a cost. Is the layer asking the card for a PSO the card already has? Are identical
   PSOs deduplicated? Does the PSO cache key hash a long string?
5. **The card's 105-120 ms.** Use `replay --timing detailed` on the city capture to see whether it is
   the number of draws and indirects (4,300 a frame) or a few expensive passes, and try
   `--skip-hash-passes`, `--merge-barriers`, `--skip-clamps`, `--batch-clamps` to attribute it before
   writing code. If a pass of ours dominates, look at `VICTORY_LEAVE_OUT`'s existing switches rather
   than inventing one.
6. **Fetch upstream first.** `git remote -v`; if `trustno1-coder/VIctory-NVIDIA` is there, `git fetch
   upstream` and read what landed since this baseline (`git log HEAD..upstream/main --oneline`). The
   authors are optimising this same wall; their patch is worth more than an hour of yours. Same for any
   public fork that reports a fix in this area. Merge, do not reinvent.

## Rules of engagement

- **No cheating, ever.** Not by lowering the picture or the resolution, not by frame generation, not by
  `VICTORY_LEAVE_OUT`-ing visible passes, not by hiding the HUD, not by capping fps, not by measuring a
  cutscene or the loading screen, not by editing logs or the measurement script, not by removing the
  diagnostics that report the cost. If you cannot tell whether a change cheats, it cheats.
- **One change per experiment**, committed on its own, on a branch named `perf/30fps-loop` off the
  current HEAD. Never push to `main`. `cargo fmt`, `cargo test` for touched crates, comment in the
  repository's voice (why, with the measurement that justifies it).
- **A change that does not beat the baseline by at least 5 % on the number it targets is reverted**, and
  `docs/findings.md` says so with its number. A smaller diffs that is correct beats a bigger one.
- **Correctness first**: the replay compares its picture with the presented one; `--compare`'s picture
  difference and D3D12's debug layer (`--debug-layer`) are how you know a speed-up did not change what
  is drawn. A frame that is 2 ms faster and one pixel wrong is wrong.
- **The user is not your build server.** Batch your questions. Ask at most one thing at a time, with the
  exact command to paste, and never ask the user to interpret a log — parse it yourself.
- **Report every 30 minutes** in two lines: current city fps, what you are trying, time left.

## Final report

A table of every attempt (change, city fps, preparing ms, replay frame ms, kept or reverted), the three
definition-of-done numbers, the branch and PR, and one paragraph on what a next 3-hour session should
attack first. Honest beats optimistic: the number is the deliverable.
