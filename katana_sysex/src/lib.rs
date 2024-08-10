#![no_std]

use defmt::{write, Format};

const RX_MSG_LEN: usize = 6;
const MSG_BEGIN: u8 = 0xf0;
const MSG_END: u8 = 0xf7;

#[derive(Clone)]
pub struct Message<const LEN: usize> {
    buf: [u8; LEN]
}
pub type RxMessage = Message<RX_MSG_LEN>;

impl<const LEN: usize> Format for Message<LEN> {
    fn format(&self, fmt: defmt::Formatter) {
        if self.buf.is_empty() {
            write!(fmt, "[]");
        } else {
            write!(fmt, "[{:02x}", self.buf[0]);
            for x in &self.buf[1..] {
                write!(fmt, ", {:02x}", x);
            }
            write!(fmt, "]");
        }
    }
}

#[derive(Clone)]
pub struct IncompleteMessage<const EXPECTED_LEN: usize> {
    buf: [u8; EXPECTED_LEN],
    len: usize
}
pub type IncompleteRxMessage = IncompleteMessage<RX_MSG_LEN>;

impl<const LEN: usize> Format for IncompleteMessage<LEN> {
    fn format(&self, fmt: defmt::Formatter) {
        if self.len == 0 {
            write!(fmt, "[]");
        } else {
            write!(fmt, "[{:02x}", self.buf[0]);
            for x in 1..self.len {
                write!(fmt, ", {:02x}", self.buf[x]);
            }
            write!(fmt, "]");
        }
    }
}

pub enum IncompleteMessageUpdateRes<const LEN: usize> {
    Incomplete(IncompleteMessage<LEN>),
    Complete(Message<LEN>),
    Invalid(RxValidationError),
}

#[derive(defmt::Format)]
pub enum RxValidationError {
    TooLong,
    ChecksumErr,
    InvalidStart,
    InvalidEnd,
}

struct MessageTooShort;



impl<const EXPECTED_LEN: usize> IncompleteMessage<EXPECTED_LEN> {
    fn validate(self) -> IncompleteMessageUpdateRes<EXPECTED_LEN> {
        use IncompleteMessageUpdateRes::*;
        use RxValidationError::*;
    
        let buf = &self.buf;
        if buf.is_empty() {
            Incomplete(self)
        } else if buf[0] != MSG_BEGIN {
            Invalid(InvalidStart)
        } else if self.len < RX_MSG_LEN {
            Incomplete(self)
        } else if self.len > RX_MSG_LEN {
            Invalid(TooLong)
        } else if buf[self.len - 1] != MSG_END {
            Invalid(InvalidEnd)
        } else if !validate_checksum(buf) {
            Invalid(ChecksumErr)
        } else {
            Complete(Message { buf: self.buf })
        }
    }

    pub fn start_rx() -> Self {
        IncompleteMessage { buf: [0u8; EXPECTED_LEN], len: 0 }
    }

    pub fn update(mut self, b: u8) -> IncompleteMessageUpdateRes<EXPECTED_LEN> {
        debug_assert!(self.len < EXPECTED_LEN);
        self.buf[self.len] = b;
        self.len += 1;
        self.validate()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const LEN: usize> Message<LEN> {
    pub fn as_bytes(self) -> [u8; LEN] {
        self.buf
    }
}

impl RxMessage {
    pub fn led_status(&self) -> Option<u8> {
        if self.buf[1] != 0x00 || self.buf[2] != 0x00 {
            None
        } else {
            Some(self.buf[3])
        }
    }
}

impl<const LEN: usize> IntoIterator for Message<LEN> {
    type Item = u8;

    type IntoIter = <[u8; LEN] as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.buf.into_iter()
    }
}

pub fn status(footswitch: u8) -> Message<9> {
    let mut buf = [MSG_BEGIN, 0, 0, 0, 0, footswitch, 0, 0, MSG_END];
    _ = set_checksum(&mut buf[..]);
    Message { buf }
}

pub fn footswitch_change(footswitch: u8) -> Message<7> {
    let mut buf = [MSG_BEGIN, 0, 0x02, footswitch, 0, 0, MSG_END];
    _ = set_checksum(&mut buf[..]);
    Message { buf }
}

fn set_checksum(msg: &mut [u8]) -> Result<(), MessageTooShort> {
    let chksum = checksum(msg)?;
    let len = msg.len();
    msg[len - 2] = chksum;
    Ok(())
}

fn validate_checksum(msg: &[u8]) -> bool {
    let Ok(ours) = checksum(msg) else {
        return false;
    };
    let theirs = msg[msg.len() - 2];

    theirs == ours
}

fn checksum(msg: &[u8]) -> Result<u8, MessageTooShort> {
    if msg.len() < 3 {
        return Err(MessageTooShort);
    }

    Ok((0x80
        - msg[1..msg.len() - 2]
            .iter()
            .fold(0u8, |acc, x| (acc + x) & 0x7f)) & 0x7f)
}

#[cfg(test)]
mod test {
    use super::validate_checksum;

    #[test]
    fn test_chksum() {
        let msg = [0xf0, 0, 0, 0x7f, 0x7f, 0, 0, 0x02, 0xf7];
        assert!(validate_checksum(&msg));

        let msg = [0xf0, 0, 0, 2, 0x7e, 0xf7];
        assert!(validate_checksum(&msg));
    }
}
