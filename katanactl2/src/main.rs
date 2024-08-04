//! Blinks the LED on a Pico board
//!
//! This will blink an LED attached to GP25, which is the pin the Pico uses for the on-board LED.
#![no_std]
#![no_main]

use core::cell::RefCell;

use bsp::entry;
use critical_section::Mutex;
use defmt::*;
use defmt_rtt as _;
use embedded_alloc::Heap;
use panic_probe as _;

use rp2040_hal::{
    fugit::ExtU32,
    gpio::PullNone,
    timer::{Alarm, Alarm0},
};
use rp_pico as bsp;

use bsp::hal::{
    clocks::{init_clocks_and_plls, Clock},
    pac::{self, interrupt},
    pio::PIOExt,
    sio::Sio,
    timer,
    watchdog::Watchdog,
};
use static_cell::StaticCell;
use time::{InstantEx, *};

extern crate alloc;

mod buttons;
mod kt_uart;
mod time;

#[global_allocator]
static HEAP: Heap = Heap::empty();

static STATUS_MSG_ALARM: Mutex<RefCell<Option<Alarm0>>> = Mutex::new(RefCell::new(None));

static mut TIMER_REF: Option<&timer::Timer> = None;
static TIMER: StaticCell<timer::Timer> = StaticCell::new();

#[interrupt]
fn PIO0_IRQ_0() {
    buttons::on_interrupt();
}

#[interrupt]
fn TIMER_IRQ_0() {
    // Reschedule
    critical_section::with(|cs| {
        if let Some(al) = STATUS_MSG_ALARM.borrow_ref_mut(cs).as_mut() {
            unwrap!(al.schedule(100u32.millis()).map_err(|_| "Schedule error"));
            al.clear_interrupt();
        }
    });
}

#[interrupt]
fn UART1_IRQ() {
    // Just clear the flag, this is only used for wfi wake
    // The UartPeripheral::enable_rx_interrupts function enables
    // both the rx interrupt and the receive timeout interrupt, clear both.
    let s = unsafe { pac::UART1::steal() };
    s.uarticr()
        .write(|f| f.rxic().clear_bit_by_one().rtic().clear_bit_by_one());
}

defmt::timestamp!("{=u64:us}", unsafe {
    TIMER_REF.map(|t| t.get_counter().ticks()).unwrap_or(0)
});

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

    let mut timer = timer::Timer::new(pac.TIMER, &mut pac.RESETS, &clocks);

    let mut status_alarm = defmt::unwrap!(timer.alarm_0());
    status_alarm.enable_interrupt();
    unwrap!(status_alarm
        .schedule(100u32.millis())
        .map_err(|_| "Schedule error for status alarm"));
    critical_section::with(|cs| STATUS_MSG_ALARM.borrow_ref_mut(cs).replace(status_alarm));

    let timer = TIMER.init(timer);
    unsafe { TIMER_REF = Some(timer) };

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
        timer
    ));

    let button_pins = [
        pins.gpio16.reconfigure().into_dyn_pin(),
        pins.gpio17.reconfigure().into_dyn_pin(),
        pins.gpio18.reconfigure().into_dyn_pin(),
        pins.gpio19.reconfigure().into_dyn_pin(),
        pins.gpio20.reconfigure().into_dyn_pin(),
        pins.gpio21.reconfigure().into_dyn_pin(),
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

    unsafe {
        pac::NVIC::unmask(pac::Interrupt::TIMER_IRQ_0);
        pac::NVIC::unmask(pac::Interrupt::PIO0_IRQ_0);
        pac::NVIC::unmask(pac::Interrupt::UART1_IRQ);
    }

    loop {
        // Wait until woken by an interrupt
        // - UART data receive
        // - Button pressed / PIO interrupt
        // - Timer trigger
        cortex_m::asm::wfi();
        trace!("Main loop woke (interrupt)");

        while let Some(ch) = buttons::pop_change_queue() {
            ktuart.enqueue_send(katana_sysex::status(ch).as_bytes().into_iter().collect());
        }

        if timer.has_passed(next_status_send) {
            let btn = buttons::current();
            ktuart.enqueue_send(katana_sysex::status(btn).as_bytes().into_iter().collect());
            next_status_send = next_status_send.offset_ms(300);
        }

        ktuart.tick(&mut delay);

        while let Some(rx) = ktuart.pop_rx() {
            defmt::info!("Got msg {}", rx);
        }
    }
}

// End of file
