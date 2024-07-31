
pub enum RxValidateRes {
    Incomplete,
    Complete,
    Invalid(RxValidationError)
}

#[derive(defmt::Format)]
pub enum RxValidationError {
    TooLong,
    ChecksumErr,
    InvalidStart,
    InvalidEnd,
}

const RX_MSG_LEN: usize = 6;
const RX_MSG_BEGIN: u8 = 0xf0;
const RX_MSG_END: u8 = 0xf7;

pub fn validate_rx(buf: &[u8]) -> RxValidateRes {
    use RxValidateRes::*;
    use RxValidationError::*;

    if buf.is_empty() {
        Incomplete
    } else if buf[0] != RX_MSG_BEGIN {
        Invalid(InvalidStart)
    } else if buf.len() < RX_MSG_LEN {
        Incomplete
    } else if buf.len() > RX_MSG_LEN {
        Invalid(TooLong)
    } else if buf[buf.len() - 1] != RX_MSG_END {
        Invalid(InvalidEnd)
    } else if !validate_checksum(buf) {
        Invalid(ChecksumErr)
    } else {
        Complete
    }

}

pub fn status(footswitch: u8) -> crate::kt_uart::MsgBuf {
    let mut m: crate::kt_uart::MsgBuf = [0xf0, 0, 0, 0, 0, footswitch, 0, 0, 0xF7].into_iter().collect();

    let chksum = unsafe { checksum_unchecked(&m) };
    let len = m.len();
    m[len - 2] = chksum;
    m
}

fn validate_checksum(buf: &[u8]) -> bool {
    if buf.len() < 2 {
        return false
    }

    let theirs = buf[buf.len() - 2];
    // SAFETY: length checked above
    let ours = unsafe { checksum_unchecked(buf) };

    theirs == ours
}

unsafe fn checksum_unchecked(full_msg: &[u8]) -> u8 {
    0x80 - full_msg[..full_msg.len() - 2].iter().fold(0u8, |acc, x| (acc + x) & 0x7f)
}

#[cfg(test)]
mod test {
    use crate::kt_sysex::validate_checksum;

    #[test]
    fn test_chksum() {
        let msg = [0xf0, 0, 0, 0x7f, 0x7f, 0, 0, 0x02, 0xf7];
        assert!(validate_checksum(&msg));

        let msg = [0xf0, 0, 0, 2, 0x7e, 0xf7];
        assert!(validate_checksum(&msg));
    }
}