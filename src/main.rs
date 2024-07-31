//! Blinks the LED on a Pico board
//!
//! This will blink an LED attached to GP25, which is the pin the Pico uses for the on-board LED.
#![no_std]
#![no_main]

use bsp::entry;
use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use rp_pico as bsp;

use bsp::hal::{
    clocks::{init_clocks_and_plls, Clock, ClocksManager},
    fugit::RateExtU32,
    pac,
    sio::Sio,
    timer, uart,
    watchdog::Watchdog,
};
use time::InstantEx;
use time::*;

mod kt_sysex;
mod kt_uart;
mod time;

fn init_uart(
    uart: pac::UART1,
    resets: &mut pac::RESETS,
    pins: impl uart::ValidUartPinout<pac::UART1>,
    clocks: &ClocksManager,
) -> Result<
    uart::UartPeripheral<uart::Enabled, pac::UART1, impl uart::ValidUartPinout<pac::UART1>>,
    uart::Error,
> {
    let uart = uart::UartPeripheral::new(uart, pins, resets).enable(
        uart::UartConfig::new(62500.Hz(), uart::DataBits::Eight, None, uart::StopBits::One),
        clocks.peripheral_clock.freq(),
    )?;

    Ok(uart)
}

#[entry]
fn main() -> ! {
    info!("Program start");
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
    let uart = unwrap!(init_uart(
        pac.UART1,
        &mut pac.RESETS,
        (pins.gpio4.into_function(), pins.gpio5.into_function()),
        &clocks
    ));

    let mut ktuart = kt_uart::KatanaUart::new(uart, &timer);

    let mut next_status_send = timer.now().offset_ms(300);

    loop {
        delay.delay_ms(1);

        if timer.has_passed(next_status_send) {
            ktuart.enqueue_send(kt_sysex::status(0));
            next_status_send = next_status_send.offset_ms(300);
        }

        ktuart.tick();

        while let Some(rx) = ktuart.pop_rx() {
            defmt::info!("Got msg {}", rx);
        }
    }
}

// End of file
