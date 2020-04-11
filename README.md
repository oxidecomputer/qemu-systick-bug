# qemu-systick-bug

This repository demonstrates a bug with respect to QEMU's handling of
SysTick on ARM -- or at the very least, an inconsistency with respect to
hardware.  (This issue has been filed with QEMU as
<a href="https://bugs.launchpad.net/qemu/+bug/1872237">#1872237</a>.)

## Issue

Take this Rust program:

```rust
#![no_std]
#![no_main]

extern crate panic_semihosting;

use cortex_m_rt::entry;
use cortex_m_semihosting::hprintln;
use cortex_m::peripheral::syst::SystClkSource;
use cortex_m::peripheral::SYST;

fn delay(syst: &mut cortex_m::peripheral::SYST, ms: u32)
{
    /*
     * Configured for the LM3S6965, which has a default CPU clock of 12 Mhz
     */
    let reload = 12_000 * ms;

    syst.set_reload(reload);
    syst.clear_current();
    syst.enable_counter();

    hprintln!("waiting for {} ms (SYST_CVR={}) ...",
        ms, SYST::get_current()
    ).unwrap();

    while !syst.has_wrapped() {}

    hprintln!("  ... done (SYST_CVR={})\n", SYST::get_current()).unwrap();

    syst.disable_counter();
}

#[entry]
fn main() -> ! {
    let p = cortex_m::Peripherals::take().unwrap();
    let mut syst = p.SYST;

    syst.set_clock_source(SystClkSource::Core);

    loop {
        delay(&mut syst, 1000);
        delay(&mut syst, 100);
    }
}
```

This program should oscillate between waiting for one second and waiting
for 100 milliseconds.  Under hardware, this is more or less what it does
(depending on core clock frequency); e.g., from an STM32F4107 (connected via
OCD and with semi-hosting enabled):

```
waiting for 1000 ms (SYST_CVR=11999949) ...
  ... done (SYST_CVR=11999902)

waiting for 100 ms (SYST_CVR=1199949) ...
  ... done (SYST_CVR=1199897)

waiting for 1000 ms (SYST_CVR=11999949) ...
  ... done (SYST_CVR=11999885)

waiting for 100 ms (SYST_CVR=1199949) ...
  ... done (SYST_CVR=1199897)

waiting for 1000 ms (SYST_CVR=11999949) ...
  ... done (SYST_CVR=11999885)

```

Under QEMU, however, its behavior is quite different:

```
$ cargo run
    Finished dev [unoptimized + debuginfo] target(s) in 0.03s
     Running `qemu-system-arm -cpu cortex-m3 -machine lm3s6965evb -nographic -semihosting-config enable=on,target=native -kernel target/thumbv7m-none-eabi/debug/qemu-systick-bug`
waiting for 1000 ms (SYST_CVR=11999658) ...
  ... done (SYST_CVR=11986226)

waiting for 100 ms (SYST_CVR=0) ...
  ... done (SYST_CVR=1186560)

waiting for 1000 ms (SYST_CVR=1185996) ...
  ... done (SYST_CVR=11997350)

waiting for 100 ms (SYST_CVR=0) ...
  ... done (SYST_CVR=1186581)
```

In addition to the values being strangely wrong, the behavior is wrong:
the first wait correctly waits for 1000 ms -- but the subsequent wait
(which should be for 100 ms) is in fact 1000 ms, and the next wait (which
should be for 1000 ms) is in fact 100 ms.  (That is, it appears as if
the periods of the two delays have been switched.)

The problems is that the QEMU ARM emulation code does not reload SYST_CVR from
SYST_RVR if SYST_CSR.ENABLE is not set -- and moreover, that SYST_CVR is
not reloaded from SYST_RVR even when SYST_CSR.ENABLE becomes set.  This is
very explicit; from
<a
href="https://github.com/qemu/qemu/blob/8bac3ba57eecc466b7e73dabf7d19328a59f684e/hw/timer/armv7m_systick.c#L42-L60">hw/timer/armv7m_systick.c</a>:

```c
static void systick_reload(SysTickState *s, int reset)
{
    /* The Cortex-M3 Devices Generic User Guide says that "When the
     * ENABLE bit is set to 1, the counter loads the RELOAD value from the
     * SYST RVR register and then counts down". So, we need to check the
     * ENABLE bit before reloading the value.
     */
    trace_systick_reload();

    if ((s->control & SYSTICK_ENABLE) == 0) {
        return;
    }

    if (reset) {
        s->tick = qemu_clock_get_ns(QEMU_CLOCK_VIRTUAL);
    }
    s->tick += (s->reload + 1) * systick_scale(s);
    timer_mod(s->timer, s->tick);
}
```

The statement in the code is stronger than the statement in the
<a href="https://static.docs.arm.com/ddi0403/eb/DDI0403E_B_armv7m_arm.pdf">ARMv7-M Architecture Reference Manual</a> (sec B3.3.1):

> Writing to SYST_CVR clears both the register and the COUNTFLAG status
> bit to zero. This causes the SysTick logic to reload SYST_CVR from SYST_RVR
> on the next timer clock. A write to SYST_CVR does not trigger the
> SysTick exception logic.

Note that this does not mention the behavior on a write to SYST_CVR when
SYST_CSR.ENABLE is not set -- and in particular, does *not* say that writes to
SYST_CVR will be ignored if SYST_CSR.ENABLE is not set.

Section 3.3.1 does go on to say:

> The SYST_CVR value is UNKNOWN on reset. Before enabling the SysTick counter,u
> software must write the required counter value to SYST_RVR, and then write
> to SYST_CVR. This clears SYST_CVR to zero. When enabled, the counter 
> reloads the value from SYST_RVR, and counts down from that value, rather]
> than from an arbitrary value.

(This is more or less what has been quoted in the implementation of
`systick_reload`, above.)  This note does **not** say, however, that writes
to SYST_CVR will be discarded when not enabled, but rather that the counting
will only begin (and the value in SYST_RVR loaded or reloaded) when
SYST_CSR.ENABLE becomes set.

While QEMU's behavior does not match that of the hardware (and is therefore
at some level malfunctioning), there is additional behavior that is very
clearly incorrect: once SYST_CSR.ENABLE is set, not only will SYST_CVR
continue to return 0 (that is, the counter will not be enabled),
SYST_CSR.COUNTFLAG will become set when the *old* value of SYST_RVR ticks
have elapsed!  This is wrong in both regards: if SYST_CVR is not counting
down, SYST_CSR.COUNTFLAG should never become set -- and it certainly
shouldn't match a value of SYST_RVR that has been overwritten in the
interim!

In terms of fixing this, it's helpful to understand the
<a
href="https://lists.gnu.org/archive/html/qemu-devel/2015-05/msg01217.html">context
around this change</a>:

> Consider the following pseudo code to configure SYSTICK (The
> recommended programming sequence from "the definitive guide to the
> arm cortex-m3"):
>    SYSTICK Reload Value Register = 0xffff
>    SYSTICK Current Value Register = 0
>    SYSTICK Control and Status Register = 0x7
>
> The pseudo code "SYSTICK Current Value Register = 0" leads to invoking
> systick_reload(). As a consequence, the systick.tick member is updated
> and the systick timer starts to count down when the ENABLE bit of
> SYSTICK Control and Status Register is cleared.
>
> The worst case is that: during the system initialization, the reset
> value of the SYSTICK Control and Status Register is 0x00000000. 
> When the code "SYSTICK Current Value Register = 0" is executed, the
> systick.tick member is accumulated with "(s->systick.reload + 1) *
> systick_scale(s)". The systick_scale() gets the external_ref_clock
> scale because the CLKSOURCE bit of the SYSTICK Control and Status
> Register is cleared. This is the incorrect behavior because of the
> code "SYSTICK Control and Status Register = 0x7". Actually, we want
> the processor clock instead of the external reference clock.
>
> This incorrect behavior defers the generation of the first interrupt. 
>
> The patch fixes the above-mentioned issue by setting the systick.tick
> member and modifying the systick timer only if the ENABLE bit of
> the SYSTICK Control and Status Register is set.
>
> In addition, the Cortex-M3 Devices Generic User Guide mentioned that
> "When ENABLE is set to 1, the counter loads the RELOAD value from the
> SYST RVR register and then counts down". This patch adheres to the
> statement of the user guide.

This fix is simply incorrect -- or at the least, incomplete:
a write to SYST_CVR must clear any cached state
such that a subsequent write to SYST_CSR.ENABLE will correctly cause
a reload.  Here is a diff that solves the problem without re-introducing
the behavior that the original fix was trying to correct:

```diff
diff --git a/hw/timer/armv7m_systick.c b/hw/timer/armv7m_systick.c
index 74c58bcf24..3f7b267c2d 100644
--- a/hw/timer/armv7m_systick.c
+++ b/hw/timer/armv7m_systick.c
@@ -181,6 +181,15 @@ static MemTxResult systick_write(void *opaque, hwaddr addr,
         break;
     case 0x8: /* SysTick Current Value.  Writes reload the timer.  */
         systick_reload(s, 1);
+
+        if ((s->control & SYSTICK_ENABLE) == 0) {
+            /*
+             * If we're not enabled, explicitly clear our tick value to
+             * assure that when we do become enabled, we correctly reload.
+             */
+            s->tick = 0;
+        }
+
         s->control &= ~SYSTICK_COUNTFLAG;
         break;
     default:
```

## Building

Assuming that one has the Rust toolchain for the `thumbv7em-none-eabi` target
installed, it should build with `cargo build`.  For details on installing
Rust (and this tool chain), consult the (excellent) <a
href="https://rust-embedded.github.io/book/">Embedded Rust Book</a> -- 
and in particular its <a
href="https://rust-embedded.github.io/book/start/qemu.html">chapter on
QEMU</a>.

## Running under QEMU

You can run it with `cargo run`, or, via the command line:

```
qemu-system-arm -cpu cortex-m3 \
	-machine lm3s6965evb -nographic \
	-semihosting-config enable=on,target=native \
	-kernel ./target/thumbv7m-none-eabi/debug/qemu-systick-bug
```

