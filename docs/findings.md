# What was learned on the RX 7900 XTX (2026-11-23 to 2026-11-25)

One PC: i9-12900K (8 fast cores, 8 slow ones, 24 threads), 64 GB, Radeon RX 7900 XTX
24 GB, Windows 11, 1440p, Series X profile. Figures are of diagnostics builds with a
screenshot a second. Scenes are not the same from run to run, so read differences
under about 5 % as nothing. What a switch does is in the comment where it is read.

## Where a frame goes

Median of whole runs, same part of the prologue: 280 ms, then 124, 103, 75, now 46-65.
On the water (first boat mission) 46-53 ms.

The title needs 33 ms. Of a 55 ms frame on the water:

- **The thread that prepares submissions: 48 ms.** It is the wall. Steps of the lists
  18 (refreshes of host copies 10), steps done on the card 7, waiting for descriptor
  records 6, asking the write watch 5, copies in the card's memory 4, the title's bytes
  under shader writes 4.
- **The card: about 41 ms busy**, the next wall.
- **List calls: 36,000 a frame, 230 ms of work** spread over eight threads. In a street
  chase 67,000 calls and 430 ms.
- Descriptor records copied as plain bytes: 400,000 slots looked at, 40,000 mirrored a
  frame, most of them empty.
- Landing what shaders wrote: 16 MB a frame put as dwords, on a thread of its own.

## The one idea behind most of it

On the console the CPU and the graphics processor share memory. Here they do not, so
everything one writes and the other reads must be noticed and moved. Every large cost
is one of four: **noticing** writes of the title (write watch, guards), **noticing**
writes of shaders (hashes on the card, compares on the CPU), **moving** them, and
**waiting** until an order is safe. Gains came from making each of them narrower
(dwords, not pages), wider (threads, AVX2) or later (on the card, behind the frame),
in that order of size:

| What | Frame before | after |
|---|---|---|
| Descriptor records of draws' tables left to the draws | 280 ms | about 124 |
| Our hash passes over card copies, 64 threads a piece | card 79 ms | 66 |
| Preparing on a thread of its own, refreshes and records spread | 103 | 75 |
| List calls written down and translated by eight threads (`VICTORY_THIN`) | 75 | 68 |
| Submissions joined, card steps, textures sent ahead | 68 | about 63 |
| Title bytes under shader writes compared with AVX2 on six threads | 14 ms of it | 4-7 |
| Stores into guarded memory looked at first, fewer guards reset | 69 ms of threads' time | about 6 |

(Each figure is from the run in which it was measured; they overlap and do not add up.)

## Faults found, and what they were

- **Boats sank, the sea plane went under.** The ocean's height and displacement maps are
  textures that compute passes write and the title's physics sample on the CPU. They were
  never brought back to title memory. Fixed by Geckomacabre's read-back as merged in
  fxceless/victory-fork f5c03f3b; on by default here (`VICTORY_TEXTURE_BACK=0`: off).
  With it the boat stays up through a whole run; without it sank in every one.
- **A device loss about every 16 minutes**: a page fault inside a texture made in the same
  tenth of a second. A slot showed it before its upload had run.
  `VICTORY_SHOW_AFTER=1`; none in the sessions since (likely fixed, not proven).
- **A rare lost write** (6 of 200 runs of a stress test): a view was mapped between our
  look at its guards and our lock, and views were opened one after another. None in 603
  runs after.
- **Blocky patches**: indirect dispatches cut at 65,535 groups.
- **Square holes and dashed edges in shadows**: pixel shaders that read their quad's
  neighbours with wave reads; on AMD helper lanes take no part in them.
- **Cube reflections too blurred, wrong materials**: DXIL's `Log` is base 2, and
  normalized buffer codes were one step off.
- **Every ray missed**: the dispatch flag that says counts are threads, not groups.

## Measured and not worth it

- Unordered early pipelines for everything (AMD's compiler crashed).
- Series S profile, smaller textures, a second drawing queue, `ASYNC_SUBMIT` off.
- Mapping pages on sixteen threads: no more pages mapped, frames longer.
- Scalar buffer loads four dwords a call (`VICTORY_SHADER_OPT=127`): no gain on this card.
- Wave sizes, fused multiply-adds, fast reciprocals, raw picked buffers, leaving out
  barriers: nothing to 0.1 ms each. Exact RDNA float rules (`VICTORY_RDNA_FLOAT`): 0.8 ms.
- Running the console's shader code natively on the card: the Windows driver takes no raw
  code, the two generations differ, and a Vulkan host layer ends at the same compiler. What
  is left to gain from shader code was guessed at under 1 ms.
- An on-screen panel of numbers (`VICTORY_PANEL`): it helps finding where a frame goes but
  costs some, so it is off and F3 shows it.

## What is next

1. The preparing thread: split what one submission does over two threads in a row; ask
   for the next submission while this one is sent; view the card's own buffers where fills
   copy them now.
2. The card: small buffers' copies in its own memory (`VICTORY_CARD_BUFFERS_MIN_KB=64`,
   after fxceless c36971b2; not measured here yet), and the passes that run for nothing
   without rays.
3. Stalls: 270-660 s of a session were full of frames over 200 ms, almost all while new
   pipelines were made. `victory warm`, and a quarter of the workers for the slow ones.
4. Outdoor flicker: suspected to be one bad indirect record that uses up its list's
   budget, so that every later draw of it is empty. The log's frame line counts them;
   `VICTORY_INDIRECT_REST=0` is the experiment.
5. Memory, 40-46 GB private: lists' allocators 9 GB, windows 7, uploads 4
   (`VICTORY_MEMORY_MAP=1` lists it).

## The other repositories, compared

Read up to 2026-11-25: Marvisak/VIctory, fxceless/victory-fork, DexNom/VIctory,
Belobug/VIctory-Belobug-AMD, FQY55kai/ZJOU-VICTORY. What was taken is in the history,
each commit naming its source. In short:

- Marvisak's commits hardly apply any more; nearly all of what they fix is here, often
  another way. Taken: nearest normalized codes, d16 views, shader model flags.
- fxceless: the base. Newest taken: texture read-back, small buffers on the card, hashes
  over a buffer's own bytes.
- DexNom: thread counts of ray dispatches, cube gradients, joined sparse ranges.
- Belobug: pipeline workers for screen passes, the ambient stand-in's idea, pipeline and
  constant buffer bookkeeping (romainhedouin).
- ZJOU: render targets not set again; some shader corrections are being looked at.

# A 16 GB laptop (RTX 4050, 2026-11-27): where it stops

One PC: 16 GB, RTX 4050 Laptop (a budget of 5,152 MB), 16 logical processors, Windows 11,
2560x1440, `-Low`. Four runs: the first cold, then with the host pipeline library (88,567
pipelines known, 20 MB loaded), then `-Low -SeriesS`, then `VICTORY_THIN_THREADS=12`.

What holds: 30 fps from the first second to about frame 1,500 cold and frame 2,000 with the
library loaded — the opening, and it is the launcher's profile doing that, not luck. What does
not: the city, 5 to 7 fps, the same in every run.

Of a 135-190 ms frame there:

- **The submitting thread: 98-124 ms**, of which 50-57 ms in the lists' steps. It is one
  thread, and it is the wall (this doc's "what is next", 1).
- **The card: 63-78 % busy, 105-120 ms a frame.**
- List calls translated: 25,000-30,500 a frame, 570 ms of processor time on 8 threads
  (~71 ms of wall) and the same 573 ms on 12 (~48 ms): the translation is no longer the wall
  once it has threads to spare.

Why the card cannot be the pixels: `-SeriesS` renders 640x360, sixteen times fewer pixels than
`-Low`'s 1280x720, and the card's time is unchanged (105-120 ms), as is the translation
(573 against 575 ms). The cost is per command: `graphics_table` 2,137-3,668 a frame,
`compute_table` 2,029-2,637, `execute_indirect` 1,717-2,241, `set_pipeline` 1,406-2,076,
the two views and two constants families about 1,000 each — ~4,300 draws and indirect
executions and ~25,000 state sets a frame.

Measured and not worth it, on that PC:

- **Leaving out repeated calls** (`VICTORY_THIN_KINDS`' count of them): **one** call a frame
  says the same as the one before it, of 30,000. The title does not repeat itself.
- **Any one hot method**: the most frequent is 7 % of the calls. There is nothing to special-case.
- **More translating threads** (8 against 12 of the 16): the same 573 ms of work, the deep
  city 7.1 against 7.4 fps (within noise). It does help before the city: 30 fps held from
  frame 1,500 to 2,000, and frames 2,000-2,200 averaged 24.6 fps where the 8-thread run was
  already at 4.
- **The smaller picture** (`-SeriesS`): no gain at all, and a softer picture.

What was worth it, and is in the layer now:

- The tight tier's own memory lines: pressure began at 1,919 MiB and 1,519 MiB of free
  physical memory in two runs and 499 MiB in the third, never in the first minute as the
  32 GB machines' 3 GiB line did (2026-11-27 21:31, `host memory pressure` at frame 1,842 and
  no `cleared` line after it).
- `victory preload`: the in-run compile wave is gone, 30 fps from the first second, and the
  card's line is reached later (frame 1,659 and 2,213 against 1,842).

What is left is the two walls of the first list, and both are core work: what one submission
does spread over more than one thread, and the cost of a call itself (19 us a call here,
3.5 us on the 24-thread PC).
