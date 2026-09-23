# Stack reconstruction reproduction

`stack_truncation` exercises Linux x86-64 stack capture and frame-pointer
fallback. Build it with release optimization and debug information:

```sh
cargo rustc --release -p samply --example stack_truncation -- \
  -C debuginfo=2 -C force-frame-pointers=yes
samply record -r 5000 --presymbolicate --save-only -o recursion.json.gz -- \
  ./target/release/examples/stack_truncation 1024 40 2
```

Arguments are bytes of padding per recursive frame, recursion depth, duration
in seconds, and an optional mode (`normal`, `gap`, or `tail`). Padding sizes
are 1024, 16384, and 49152 bytes. Depth 40 produces 41 `stack_frame` frames;
depth 2 produces three. Tail mode bypasses the padded recursion.

Useful cases with frame pointers enabled:

| Arguments | Purpose |
|---|---|
| `1024 2 2` | Small-stack control. |
| `1024 40 2` | Repeated return addresses; must retain exactly 41 recursive frames. |
| `1024 200 2` | Kernel callchain depth limit; root may remain missing. |
| `49152 2 2` | FP continuation beyond the captured stack bytes. |
| `1024 2 2 gap` | Correct DWARF metadata recovers across an omitted FP chain. |
| `16384 2 2 gap` | A broken FP chain cannot extend an exhausted DWARF capture. |
| `1024 2 2 tail` | Optimized tail call has no physical caller frame to recover. |

The gap mode uses a small assembly wrapper that saves and restores RBP, but
uses it as a general register during the call. Its CFI describes the saved
register correctly. This models native code built without frame pointers.

Rebuild with `-C force-frame-pointers=no` to isolate bounded DWARF capture.
The default modern Rust Linux target already emits unwind tables, including
with `panic=abort`. Keep the matching executables when saving profiles.

The merge accepts a continuation only when the FP and DWARF sequences agree
from the sampled instruction through the last DWARF caller. An address match
at one recursion level does not establish a valid join. Disagreement retains
the DWARF fragment and its truncation label. Accepted FP continuations carry
an explicit completeness-unknown label because perf does not report why its
FP walk ended.
