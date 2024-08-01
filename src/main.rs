//! Blinks the LED on a Pico board
//!
//! This will blink an LED attached to GP25, which is the pin the Pico uses for the on-board LED.
#![no_std]
#![no_main]

use bsp::entry;
use defmt::*;
use defmt_rtt as _;
use embedded_alloc::Heap;
use panic_probe as _;

use rp2040_hal::gpio::PullNone;
use rp_pico as bsp;

use bsp::hal::{
    clocks::{init_clocks_and_plls, Clock},
    pac::{self, interrupt},
    pio::PIOExt,
    sio::Sio,
    timer,
    watchdog::Watchdog,
};
use time::{InstantEx, *};

extern crate alloc;

mod buttons;
mod kt_sysex;
mod kt_uart;
mod time;

#[global_allocator]
static HEAP: Heap = Heap::empty();

#[interrupt]
fn PIO0_IRQ_0() {
    buttons::on_interrupt();
}

fn init_allocator() {
    use core::mem::MaybeUninit;
    const HEAP_SIZE: usize = 2048;
    static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
    unsafe { HEAP.init(HEAP_MEM.as_ptr() as usize, HEAP_SIZE) };
}

#[entry]
fn main() -> ! {
    init_allocator();

    let mut pac = pac::Peripherals::take().unwrap();
    let core = pac::CorePeripherals::take().unwrap();
    let mut watchdog = Watchdog::new(pac.WATCHDOG);
    let sio = Sio::new(pac.SIO);

    // External high-speed crystal on the pico board is 12Mhz
    let external_xtal_freq_hz = 12_000_000u32;
    let clocks = init_clocks_and_plls(
        external_xtal_freq_hz,
        pac.XOSC,
        pac.CLOCKS,
        pac.PLL_SYS,
        pac.PLL_USB,
        &mut pac.RESETS,
        &mut watchdog,
    )
    .ok()
    .unwrap();

    let mut delay = cortex_m::delay::Delay::new(core.SYST, clocks.system_clock.freq().to_Hz());

    let timer = timer::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);

    let pins = bsp::Pins::new(
        pac.IO_BANK0,
        pac.PADS_BANK0,
        sio.gpio_bank0,
        &mut pac.RESETS,
    );

    let uart_pins = (
        pins.gpio4.into_function().into_pull_type::<PullNone>(),
        pins.gpio5.into_function().into_pull_type::<PullNone>(),
    );
    let mut ktuart = unwrap!(kt_uart::KatanaUart::new(
        pac.UART1,
        &mut pac.RESETS,
        uart_pins,
        &clocks,
        &timer
    ));

    let button_pins = [
        pins.gpio6.reconfigure().into_dyn_pin(),
        pins.gpio7.reconfigure().into_dyn_pin(),
        pins.gpio8.reconfigure().into_dyn_pin(),
        pins.gpio9.reconfigure().into_dyn_pin(),
        pins.gpio10.reconfigure().into_dyn_pin(),
    ];

    let (mut pio0, sm0, _, _, _) = pac.PIO0.split(&mut pac.RESETS);

    unwrap!(buttons::init_buttons::<_, _, 0>(
        sm0,
        &mut pio0,
        clocks.system_clock.freq(),
        button_pins
    )
    .map_err(|_| "PIO install error"));

    let mut next_status_send = timer.now().offset_ms(300);

    loop {
        delay.delay_ms(1);

        while let Some(ch) = buttons::pop_change_queue() {
            ktuart.enqueue_send(kt_sysex::status(ch));
        }

        if timer.has_passed(next_status_send) {
            let btn = buttons::current();
            ktuart.enqueue_send(kt_sysex::status(btn));
            next_status_send = next_status_send.offset_ms(300);
        }

        ktuart.tick();

        while let Some(rx) = ktuart.pop_rx() {
            defmt::info!("Got msg {}", rx);
        }
    }
}

// End of file
