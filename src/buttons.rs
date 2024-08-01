extern crate alloc;

use alloc::boxed::Box;
use core::cell::{Cell, RefCell};
use critical_section::Mutex;
use heapless::Deque;
use pio::Program;
use pio_proc::pio_asm;
use rp2040_hal::{
    fugit::HertzU32,
    gpio::{DynPinId, Pin, PullUp},
    pio::{
        InstallError, PIOBuilder, PinDir, PioIRQ, Rx, UninitStateMachine, ValidStateMachine, PIO,
    },
};

struct ButtonDebouncePioProgram {
    program: Program<32>,
    debounce_cycle_instructions: u8,
    debounce_cycles: u8,
}

static BUTTONS_PIO_SM_RX: Mutex<RefCell<Option<Box<dyn PioFifoRead + Send>>>> =
    Mutex::new(RefCell::new(None));
static BUTTON_CHANGE_QUEUE: Mutex<RefCell<Deque<u8, 5>>> = Mutex::new(RefCell::new(Deque::new()));
static CURRENT_BUTTONS: Mutex<Cell<u8>> = Mutex::new(Cell::new(0));

trait PioFifoRead {
    fn read(&mut self) -> Option<u32>;
}

impl<SM: ValidStateMachine> PioFifoRead for Rx<SM> {
    fn read(&mut self) -> Option<u32> {
        Rx::read(self)
    }
}

fn program() -> ButtonDebouncePioProgram {
    let p = pio_asm!(
        "
.define public debounce_cycle_instructions 68
.define public debounce_cycles 32

start:
    // Read the input pins
    in pins, 4

    // Check if the value changed from previous iteration
    mov y isr
    jmp x!=y changed

    // If we're in debounce period, jump to the debounce check
    jmp !osre debounce

    // No change detected, clear isr and start from beginning
    jmp reset

changed:
    mov x y       // Store new button states to x
    mov osr null  // Clear osr counter
    jmp reset

debounce:
    out null, 1   [31]     // Shift one bit from the debounce counter
    jmp !osre reset [31]   // Counter not empty, so still debouncing

emit:
    // Debouncing done
    push noblock

reset:
    // Clear isr
    in null, 32
",
    );

    ButtonDebouncePioProgram {
        program: p.program,
        debounce_cycle_instructions: p
            .public_defines
            .debounce_cycle_instructions
            .try_into()
            .unwrap(),
        debounce_cycles: p.public_defines.debounce_cycles.try_into().unwrap(),
    }
}

fn validate_pins(pin_ids: impl IntoIterator<Item = u8>) -> (u8, u8) {
    let mut base_pin = None;
    let mut prev_pin = None;
    let mut pin_count = 0;

    for pin_id in pin_ids {
        // Assert pins are consecutive
        if let Some(prev) = prev_pin {
            assert_eq!(prev + 1, pin_id);
        }
        prev_pin = Some(pin_id);
        if base_pin.is_none() {
            base_pin = Some(pin_id);
        }
        pin_count += 1;
    }
    // If we don't have base_pin here, there were no pins passed in - error
    let base_pin = base_pin.unwrap();

    (base_pin, pin_count)
}

pub fn init_buttons<
    P: rp2040_hal::pio::PIOExt + 'static,
    SM: rp2040_hal::pio::StateMachineIndex + 'static,
    const IRQ: usize,
>(
    sm: UninitStateMachine<(P, SM)>,
    pio: &mut PIO<P>,
    sys_freq: HertzU32,
    pins: impl IntoIterator<Item = Pin<DynPinId, P::PinFunction, PullUp>>,
) -> Result<(), InstallError> {
    // By taking the strongly typed pins in, they can be automatically reconfigured
    // and moved to be owned by this button module.
    // We don't actually need to store them though, just get / validate the ids.
    let (base_pin, pin_count) = validate_pins(pins.into_iter().map(|p| p.id().num));

    let prog = program();

    let total_debounce_instructions = prog.debounce_cycle_instructions * prog.debounce_cycles;
    const TARGET_DEBOUNCE_TIME_MS: u32 = 5;
    let target_clk_div: u32 = (TARGET_DEBOUNCE_TIME_MS * (sys_freq.to_Hz() / 1000)) // instructions during the target time
        / total_debounce_instructions as u32;
    let clk_div = target_clk_div.min(u16::MAX as u32) as u16;

    let installed = pio.install(&prog.program)?;

    let (mut sm, rx, _tx) = PIOBuilder::from_installed_program(installed)
        .in_pin_base(base_pin)
        .in_shift_direction(rp2040_hal::pio::ShiftDirection::Left)
        .autopush(false)
        .clock_divisor_fixed_point(clk_div, 0)
        .build(sm);

    sm.set_pindirs((base_pin..(base_pin + pin_count)).map(|i| (i, PinDir::Input)));

    let pio_irq = match IRQ {
        0 => PioIRQ::Irq0,
        1 => PioIRQ::Irq1,
        _ => unreachable!(),
    };

    rx.enable_rx_not_empty_interrupt(pio_irq);

    defmt::unwrap!(critical_section::with(|cs| BUTTONS_PIO_SM_RX
        .borrow(cs)
        .replace(Some(Box::new(rx)))));

    Ok(())
}

pub fn on_interrupt() {
    critical_section::with(|cs| {
        if let Some(rx) = BUTTONS_PIO_SM_RX.borrow_ref_mut(cs).as_mut() {
            while let Some(b) = rx.read() {
                let b = (b & 0xFF) as u8;
                CURRENT_BUTTONS.borrow(cs).set(b);
                if BUTTON_CHANGE_QUEUE.borrow_ref_mut(cs).push_back(b).is_err() {
                    defmt::warn!("BUTTON_CHANGE_QUEUE full");
                }
            }
        }
    })
}

pub fn current() -> u8 {
    critical_section::with(|cs| CURRENT_BUTTONS.borrow(cs).get())
}

pub fn pop_change_queue() -> Option<u8> {
    critical_section::with(|cs| BUTTON_CHANGE_QUEUE.borrow_ref_mut(cs).pop_front())
}