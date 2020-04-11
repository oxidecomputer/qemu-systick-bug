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
