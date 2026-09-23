//! Linux x86-64 repro for stack capture, FP gaps, recursion, and tail calls.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux_x86_64 {
    use std::arch::global_asm;
    use std::ffi::c_void;
    use std::hint::black_box;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    static BREAK_FP: AtomicBool = AtomicBool::new(false);

    // Model a native dependency that uses RBP as a general register but has correct CFI.
    global_asm!(
        ".text",
        ".global frame_pointer_gap",
        ".type frame_pointer_gap,@function",
        "frame_pointer_gap:",
        ".cfi_startproc",
        "push rbp",
        ".cfi_def_cfa_offset 16",
        ".cfi_offset rbp, -16",
        "xor ebp, ebp",
        "call {leaf}",
        "pop rbp",
        ".cfi_def_cfa_offset 8",
        ".cfi_restore rbp",
        "ret",
        ".cfi_endproc",
        ".size frame_pointer_gap, .-frame_pointer_gap",
        leaf = sym gap_leaf,
    );

    unsafe extern "C" {
        fn frame_pointer_gap(deadline: *const c_void) -> u64;
    }

    extern "C" fn gap_leaf(deadline: *const c_void) -> u64 {
        // The assembly wrapper passes the live deadline through unchanged.
        hot_leaf(unsafe { *deadline.cast::<Instant>() })
    }

    #[inline(never)]
    fn tail_caller(deadline: Instant) -> u64 {
        hot_leaf(deadline)
    }

    #[inline(never)]
    fn hot_leaf(deadline: Instant) -> u64 {
        let mut value = 1_u64;
        while Instant::now() < deadline {
            for _ in 0..100_000 {
                value = black_box(value.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        }
        black_box(value)
    }

    #[inline(never)]
    fn stack_frame<const N: usize>(depth: usize, deadline: Instant) -> u64 {
        let mut padding = [0_u8; N];
        black_box(&mut padding);
        let result = if depth == 0 {
            if BREAK_FP.load(Ordering::Relaxed) {
                // The wrapper restores RBP and leaves the pointer valid.
                unsafe { frame_pointer_gap((&deadline as *const Instant).cast()) }
            } else {
                hot_leaf(deadline)
            }
        } else {
            stack_frame::<N>(depth - 1, deadline)
        };
        black_box(&mut padding);
        black_box(result ^ u64::from(padding[0]))
    }

    #[inline(never)]
    fn workload_root(bytes: usize, depth: usize, deadline: Instant) -> u64 {
        let result = match bytes {
            1024 => stack_frame::<1024>(depth, deadline),
            16384 => stack_frame::<16384>(depth, deadline),
            49152 => stack_frame::<49152>(depth, deadline),
            _ => panic!("use 1024, 16384, or 49152 bytes per frame"),
        };
        black_box(result)
    }

    pub fn run() {
        let args = std::env::args().collect::<Vec<_>>();
        let bytes = args[1].parse().unwrap();
        let depth = args[2].parse().unwrap();
        let seconds = args.get(3).map_or(2, |s| s.parse().unwrap());
        BREAK_FP.store(
            args.get(4).is_some_and(|mode| mode == "gap"),
            Ordering::Relaxed,
        );
        if args.get(4).is_some_and(|mode| mode == "tail") {
            println!(
                "{}",
                tail_caller(Instant::now() + Duration::from_secs(seconds))
            );
            return;
        }
        println!(
            "{}",
            workload_root(bytes, depth, Instant::now() + Duration::from_secs(seconds))
        );
    }
}

fn main() {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    linux_x86_64::run();
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    eprintln!("This reproduction requires Linux x86-64.");
}
