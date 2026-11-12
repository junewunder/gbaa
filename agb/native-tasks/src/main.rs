//! gbaa test ROM for lifted task switchers (native/task-coroutines;
//! docs/research/native-holes.md, "Kirby: Nightmare's task switcher"): a
//! scheduler that runs tasks on their own IWRAM stacks through a small
//! ARM switcher of Nightmare's shape, which the native lift runs on host
//! coroutines (the prelude's `task_resume`, `task_yield` and `task_exit`,
//! from the stand-ins in holes/native-tasks.toml).
//!
//! The switcher, ARM in `global_asm!`, each routine entered from Thumb by
//! a `bx pc` veneer as a linker makes them:
//!
//! - `ts_resume(task, pc, sp)` pushes r4-r11 and lr, stores the main sp at
//!   `ts_main_sp` and the task at `ts_cur`, takes the task's sp (`mov sp,
//!   r2`), pops its r4-r11 and jumps to `pc` (`bx r1`).
//! - `ts_yield(frames)` pushes r4-r11, stores sp, lr (where the task goes
//!   on) and `frames` in the task, takes the main sp back (`ldr sp, [r3]`)
//!   and returns to the scheduler past its call of `ts_resume`.
//! - `ts_exit()` marks the task dead and returns to the scheduler the
//!   same way.
//!
//! Three tasks, each with a stack of 0x400 bytes at 0x0300_5c00 + 0x400 *
//! its index (below the main stack, above agb's IWRAM statics), started
//! with eight words on it that resume pops as their r4-r11:
//!
//! - `task_tick` adds 3 to its `out` and waits `1 + out / 3 % 3` frames in a
//!   helper that yields one frame at a time (yields from a callee);
//! - `task_fib` keeps two Fibonacci numbers in registers across
//!   `ts_yield(2)`, storing the smaller to `out`;
//! - `task_burst` does five steps of `out = out * 5 + step + 1`, a
//!   `ts_yield(3)` after each, then exits; the scheduler starts it again
//!   the next frame with `out = generation * 100`.
//!
//! Each frame `f` (from 0) the scheduler counts down each live task's
//! wait and resumes the ones whose wait ran out, starts dead ones again,
//! then copies each task's `out` to `RESULTS` and draws it as a pixel at
//! (f % 240, f / 240 % 50 + 50 * task), mode 3, the low 15 bits. What it
//! draws: `SCENE.md`.
#![no_std]
#![no_main]

use core::cell::UnsafeCell;
use core::ptr::{addr_of_mut, read_volatile, write_volatile};

core::arch::global_asm!(
    ".section .text.native_tasks, \"ax\", %progbits",
    ".syntax unified",
    // void ts_resume(Task *task, u32 pc, u32 sp)
    ".balign 4",
    ".thumb",
    ".global ts_resume",
    ".type ts_resume, %function",
    ".thumb_func",
    "ts_resume:",
    "    bx pc",
    "    nop",
    ".arm",
    "ts_resume_arm:",
    "    push {{r4-r11, lr}}",
    "    ldr r3, =ts_main_sp",
    "    str sp, [r3]",
    "    ldr r3, =ts_cur",
    "    str r0, [r3]",
    "    mov sp, r2",
    "    pop {{r4-r11}}",
    "    bx r1",
    // void ts_yield(u32 frames)
    ".balign 4",
    ".thumb",
    ".global ts_yield",
    ".type ts_yield, %function",
    ".thumb_func",
    "ts_yield:",
    "    bx pc",
    "    nop",
    ".arm",
    "ts_yield_arm:",
    "    push {{r4-r11}}",
    "    ldr r3, =ts_cur",
    "    ldr r3, [r3]",
    "    str sp, [r3, #0]",
    "    str lr, [r3, #4]",
    "    str r0, [r3, #8]",
    "    ldr r3, =ts_main_sp",
    "    ldr sp, [r3]",
    "    pop {{r4-r11, lr}}",
    "    bx lr",
    // void ts_exit(void)
    ".balign 4",
    ".thumb",
    ".global ts_exit",
    ".type ts_exit, %function",
    ".thumb_func",
    "ts_exit:",
    "    bx pc",
    "    nop",
    ".arm",
    "ts_exit_arm:",
    "    ldr r3, =ts_cur",
    "    ldr r3, [r3]",
    "    mov r0, #0",
    "    str r0, [r3, #12]",
    "    ldr r3, =ts_main_sp",
    "    ldr sp, [r3]",
    "    pop {{r4-r11, lr}}",
    "    bx lr",
    ".ltorg",
    ".thumb",
);

unsafe extern "C" {
    fn ts_resume(task: *mut Task, pc: u32, sp: u32);
    fn ts_yield(frames: u32);
    /// Never comes back; declared returning so that a closed loop follows
    /// its call, as discovery's pointer scan takes a task body that
    /// ends (a body that runs into the next function it doesn't).
    fn ts_exit();
}

/// A task, as the switcher reads and writes it.
#[repr(C)]
pub struct Task {
    /// Its saved sp: where `ts_yield` left its r4-r11.
    sp: u32,
    /// Where it goes on (Thumb, bit 0 set).
    pc: u32,
    /// The frames left to wait.
    wait: u32,
    /// 0 once it has exited.
    alive: u32,
    /// What it computes, drawn each frame.
    out: u32,
    /// How many times it has been started.
    generation: u32,
}

const N: usize = 3;

struct Shared<T>(UnsafeCell<T>);

// SAFETY: only the main loop and the tasks it runs touch them, one at a
// time; there are no interrupt handlers but agb's own.
unsafe impl<T> Sync for Shared<T> {}

static TASKS: Shared<[Task; N]> = Shared(UnsafeCell::new(
    [const {
        Task {
            sp: 0,
            pc: 0,
            wait: 0,
            alive: 0,
            out: 0,
            generation: 0,
        }
    }; N],
));

/// The last frame's outs, for the native check to compare.
static RESULTS: Shared<[u32; N]> = Shared(UnsafeCell::new([0; N]));

/// The switcher's: the scheduler's sp while a task runs, and that task.
#[unsafe(no_mangle)]
static mut ts_main_sp: u32 = 0;
#[unsafe(no_mangle)]
static mut ts_cur: u32 = 0;

const STACKS: u32 = 0x0300_5C00;
const STACK: u32 = 0x400;

const DISPCNT: *mut u16 = 0x0400_0000 as *mut u16;
const VRAM: *mut u16 = 0x0600_0000 as *mut u16;

/// Waits `n` frames, a yield each: yields from below the task's own
/// function.
#[inline(never)]
fn wait_frames(n: u32) {
    for _ in 0..n {
        // SAFETY: called on a task's stack (see `ts_yield`).
        unsafe { ts_yield(1) };
    }
}

extern "C" fn task_tick(t: *mut Task) -> ! {
    loop {
        // SAFETY: `t` is this task's entry of TASKS (see Shared's Sync).
        let out = unsafe {
            let o = read_volatile(&raw const (*t).out).wrapping_add(3);
            write_volatile(&raw mut (*t).out, o);
            o
        };
        wait_frames(1 + out / 3 % 3);
    }
}

extern "C" fn task_fib(t: *mut Task) -> ! {
    let (mut a, mut b) = (0u32, 1u32);
    loop {
        // SAFETY: as task_tick's; the yield keeps r4-r11 and sp.
        unsafe {
            write_volatile(&raw mut (*t).out, a);
            ts_yield(2);
        }
        (a, b) = (b, a.wrapping_add(b));
    }
}

extern "C" fn task_burst(t: *mut Task) -> ! {
    for step in 0..5u32 {
        // SAFETY: as task_tick's.
        unsafe {
            let o = read_volatile(&raw const (*t).out)
                .wrapping_mul(5)
                .wrapping_add(step + 1);
            write_volatile(&raw mut (*t).out, o);
            ts_yield(3);
        }
    }
    // SAFETY: on the task's stack; never comes back.
    unsafe { ts_exit() };
    #[allow(clippy::empty_loop)]
    loop {}
}

/// Start task `i` (again): its stack holds the r4-r11 resume pops.
fn start(i: usize, t: *mut Task) {
    let entry: [extern "C" fn(*mut Task) -> !; N] = [task_tick, task_fib, task_burst];
    let sp = STACKS + STACK * (i as u32 + 1) - 32;
    // SAFETY: the stack is IWRAM no static reaches (see the module's doc);
    // `t` is the task's entry of TASKS.
    unsafe {
        for k in 0..8u32 {
            write_volatile((sp + 4 * k) as *mut u32, 0x1000 * i as u32 + k);
        }
        let g = read_volatile(&raw const (*t).generation) + 1;
        write_volatile(
            t,
            Task {
                sp,
                pc: entry[i] as usize as u32,
                wait: 1,
                alive: 1,
                out: if i == 2 { g * 100 } else { 0 },
                generation: g,
            },
        );
    }
}

#[agb::entry]
fn main(_gba: agb::Gba) -> ! {
    let vblank = agb::interrupt::VBlank::get();
    // SAFETY: DISPCNT is the display's control register: mode 3, BG2 on.
    unsafe { write_volatile(DISPCNT, 0x0403) };
    let tasks = TASKS.0.get() as *mut Task;
    let mut f: u32 = 0;
    loop {
        for i in 0..N {
            // SAFETY: TASKS is ours (see Shared's Sync); a task runs only
            // inside ts_resume, which comes back when it yields or exits.
            unsafe {
                let t = tasks.add(i);
                if read_volatile(&raw const (*t).alive) == 0 {
                    start(i, t);
                }
                let w = read_volatile(&raw const (*t).wait);
                if w > 1 {
                    write_volatile(&raw mut (*t).wait, w - 1);
                } else {
                    ts_resume(t, read_volatile(&raw const (*t).pc), read_volatile(&raw const (*t).sp));
                }
            }
        }
        let (x, y) = ((f % 240) as usize, (f / 240 % 50) as usize);
        for i in 0..N {
            // SAFETY: as above; the pixel is in mode 3's frame buffer.
            unsafe {
                let out = read_volatile(&raw const (*tasks.add(i)).out);
                write_volatile(addr_of_mut!((*RESULTS.0.get())[i]), out);
                write_volatile(VRAM.add((y + 50 * i) * 240 + x), out as u16 & 0x7FFF);
            }
        }
        vblank.wait_for_vblank();
        f += 1;
    }
}
