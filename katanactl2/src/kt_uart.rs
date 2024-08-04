use heapless::Deque;
use katana_sysex::IncompleteRxMessage;
use rp2040_hal::{
    clocks::ClocksManager,
    fugit::RateExtU32,
    pac,
    timer::{self, Instant},
    uart::{self, UartDevice},
    Clock,
};

use crate::time::{InstantEx, TimerEx};

pub type MsgBuf = heapless::Vec<u8, 16>;

pub struct KatanaUart<'t, UART: UartDevice, Pins: uart::ValidUartPinout<UART>> {
    uart: uart::UartPeripheral<uart::Enabled, UART, Pins>,
    timer: &'t timer::Timer,
    state: State,
    tx_queue: Deque<MsgBuf, 5>,
    rx_queue: Deque<MsgBuf, 2>,
}

impl<'t, UART: UartDevice, Pins: uart::ValidUartPinout<UART>> KatanaUart<'t, UART, Pins> {
    pub fn new(
        u: UART,
        resets: &mut pac::RESETS,
        pins: Pins,
        clocks: &ClocksManager,
        timer: &'t timer::Timer,
    ) -> Result<Self, uart::Error> {
        let mut uart = uart::UartPeripheral::new(u, pins, resets).enable(
            uart::UartConfig::new(62500.Hz(), uart::DataBits::Eight, None, uart::StopBits::One),
            clocks.peripheral_clock.freq(),
        )?;

        uart.enable_rx_interrupt();

        Ok(Self {
            uart,
            timer,
            state: State::Idle,
            tx_queue: Default::default(),
            rx_queue: Default::default(),
        })
    }

    pub fn enqueue_send(&mut self, msg: MsgBuf) {
        if self.tx_queue.push_back(msg).is_err() {
            defmt::error!("Could not enqueue message, tx buffer full")
        }
    }

    pub fn pop_rx(&mut self) -> Option<MsgBuf> {
        self.rx_queue.pop_front()
    }

    pub fn tick(&mut self, delay: &mut cortex_m::delay::Delay) {
        let mut wait_done = false;
        loop {
            let new_state = match &self.state {
                State::Idle => self.tick_idle(),
                State::Sending(ss) => self.tick_sending(ss.clone()),
                State::Receiving(buf) => self.tick_receiving(buf.clone()),
                State::WaitReply(wait_start) => self.tick_wait_reply(*wait_start),
            };
            match new_state {
                Some(ns) => {
                    defmt::trace!("New state: {}", &ns);
                    wait_done = false;
                    self.state = ns;
                }
                None => {
                    if wait_done {
                        break;
                    } else {
                        // Wait a bit (~ 2x uart msg time) and try again
                        const DELAY_TIME_US: u32 = 2 * 1_000_000 / (62500 / 9);
                        delay.delay_us(DELAY_TIME_US);
                        wait_done = true;
                        continue;
                    }
                }
            }
        }
    }

    fn tick_idle(&mut self) -> Option<State> {
        if self.uart.uart_is_readable() {
            // Start a new receive
            Some(State::Receiving(IncompleteRxMessage::start_rx()))
        } else if !self.tx_queue.is_empty() {
            // If not receiving anything, start a new send
            if self.safe_to_start_send() {
                let msg = self.tx_queue.pop_front().unwrap();
                Some(State::Sending(SendState::Send(msg, 0)))
            } else {
                None
            }
        } else {
            None
        }
    }

    fn tick_wait_reply(&mut self, wait_start: timer::Instant) -> Option<State> {
        if self.uart.uart_is_readable() {
            Some(State::Receiving(IncompleteRxMessage::start_rx()))
        } else if self.timer.has_passed(wait_start.offset_ms(100)) {
            defmt::error!("Reply wait timed out");
            Some(State::Idle)
        } else {
            None
        }
    }

    fn tick_receiving(&mut self, mut msg: IncompleteRxMessage) -> Option<State> {
        use katana_sysex::IncompleteMessageUpdateRes::*;

        let mut changed = false;
        while self.uart.uart_is_readable() {
            changed = true;
            // Read byte
            let mut b = [0u8; 1];
            let read_byte = match self.uart.read_full_blocking(&mut b) {
                Ok(_) => b[0],
                Err(e) => {
                    defmt::error!("Uart read error: {}", e);
                    return Some(State::Idle);
                }
            };

            msg = match msg.update(read_byte) {
                Incomplete(im) => im,
                Complete(m) => {
                    defmt::debug!("Received: {}", &m);
                    if self.rx_queue.push_back(m.into_iter().collect()).is_err() {
                        defmt::error!("Rx queue full!")
                    }
                    return Some(State::Idle);
                }
                Invalid(reason) => {
                    defmt::error!("Rx msg invalid: {}", reason);
                    // TODO: drop first byte and try again?
                    return Some(State::Idle);
                }
            }
        }

        if changed {
            Some(State::Receiving(msg))
        } else {
            None
        }
    }

    fn tick_sending(&mut self, ss: SendState) -> Option<State> {
        match ss {
            SendState::Send(buf, pos) => {
                self.uart.write_full_blocking(&buf[pos..pos + 1]);
                Some(State::Sending(SendState::WaitingEcho(
                    buf,
                    pos,
                    self.timer.now(),
                )))
            }
            SendState::WaitingEcho(buf, pos, wait_started) => {
                if self.uart.uart_is_readable() {
                    let mut b = [0u8; 1];
                    match self.uart.read_full_blocking(&mut b) {
                        Ok(_) => {
                            if b[0] == buf[pos] {
                                if pos + 1 == buf.len() {
                                    // Complete
                                    defmt::debug!("Sent msg {}", buf);
                                    Some(State::WaitReply(self.timer.now()))
                                } else {
                                    Some(State::Sending(SendState::Send(buf, pos + 1)))
                                }
                            } else {
                                // Something went wrong
                                defmt::error!("Send byte was read back differently");
                                Some(State::Idle)
                            }
                        }
                        Err(e) => {
                            defmt::error!("Read error while waiting for echo: {}", e);
                            Some(State::Idle)
                        }
                    }
                } else if self.timer.has_passed(wait_started.offset_ms(20)) {
                    defmt::error!("Echo wait timed out");
                    Some(State::Idle)
                } else {
                    None
                }
            }
        }
    }

    fn safe_to_start_send(&self) -> bool {
        !self.uart.uart_is_readable() && !self.uart.uart_is_busy()
    }
}

enum State {
    Idle,
    Sending(SendState),
    WaitReply(Instant),
    Receiving(IncompleteRxMessage),
}

#[derive(Clone)]
enum SendState {
    Send(MsgBuf, usize),
    WaitingEcho(MsgBuf, usize, Instant),
}

impl defmt::Format for State {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            State::Idle => defmt::write!(fmt, "Idle"),
            State::Sending(ss) => defmt::write!(fmt, "Sending({})", ss),
            State::WaitReply(t) => defmt::write!(fmt, "WaitReply(started: {})", t.ticks()),
            State::Receiving(msg) => defmt::write!(fmt, "Receiving(len: {})", msg.len()),
        }
    }
}

impl defmt::Format for SendState {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            SendState::Send(_msg, pos) => defmt::write!(fmt, "Send(pos: {})", pos),
            SendState::WaitingEcho(_msg, pos, t) => defmt::write!(
                fmt,
                "WaitingEcho(pos: {}, wait_started: {})",
                pos,
                t.ticks()
            ),
        }
    }
}
