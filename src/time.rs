use rp2040_hal::timer;

type Duration = rp2040_hal::fugit::TimerDurationU64<1_000_000>;

pub trait TimerEx {
    fn now(&self) -> timer::Instant;
    fn has_passed(&self, i: timer::Instant) -> bool;
}

impl TimerEx for timer::Timer {
    fn now(&self) -> timer::Instant {
        self.get_counter()
    }

    fn has_passed(&self, i: timer::Instant) -> bool {
        self.now().checked_duration_since(i).is_some()
    }
}

pub trait InstantEx {
    fn offset_ms(&self, ms: i64) -> timer::Instant;
}

impl InstantEx for timer::Instant {
    fn offset_ms(&self, ms: i64) -> timer::Instant {
        if ms > 0 {
            self.checked_add_duration(Duration::millis(ms as u64))
                .expect("Overflow")
        } else {
            self.checked_sub_duration(Duration::millis((-ms) as u64))
                .expect("Overflow")
        }
    }
}
