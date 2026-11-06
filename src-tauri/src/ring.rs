use std::collections::VecDeque;

/// Keeps the last `cap` bytes of a pane's output (to repaint whoever reconnects).
pub struct Ring {
    buf: VecDeque<u8>,
    cap: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Self {
        Self { buf: VecDeque::with_capacity(cap.min(1 << 16)), cap }
    }

    pub fn push(&mut self, data: &[u8]) {
        let _ = self.push_evicted(data);
    }

    /// Like `push`, but returns the oldest bytes that were pushed out (for the disk spill, see `spill`).
    pub fn push_evicted(&mut self, data: &[u8]) -> Vec<u8> {
        if data.len() >= self.cap {
            let mut out: Vec<u8> = self.buf.drain(..).collect();
            let cut = data.len() - self.cap;
            out.extend_from_slice(&data[..cut]);
            self.buf.extend(&data[cut..]);
            return out;
        }
        let overflow = (self.buf.len() + data.len()).saturating_sub(self.cap);
        let out: Vec<u8> = self.buf.drain(..overflow).collect();
        self.buf.extend(data);
        out
    }

    /// When full, the oldest bytes were cut arbitrarily: start at the first newline (skipped) or
    /// ESC (kept), whichever comes first, so a replay doesn't begin mid-escape / mid-UTF-8.
    pub fn snapshot(&self) -> Vec<u8> {
        let skip = if self.buf.len() == self.cap {
            match self.buf.iter().position(|b| *b == b'\n' || *b == 0x1b) {
                Some(i) if self.buf[i] == b'\n' => i + 1,
                Some(i) => i,
                None => 0,
            }
        } else {
            0
        };
        self.buf.iter().skip(skip).copied().collect()
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_last_bytes() {
        let mut r = Ring::new(5);
        r.push(b"abc");
        r.push(b"def");
        assert_eq!(r.snapshot(), b"bcdef");
    }
    #[test]
    fn full_ring_snapshot_starts_at_newline_or_escape() {
        // newline first: skipped (a later ESC stays)
        let mut r = Ring::new(8);
        r.push(b"0[3m\nab\x1bc");
        assert_eq!(r.snapshot(), b"ab\x1bc");
        // ESC first: kept, so the sequence replays intact
        let mut r = Ring::new(8);
        r.push(b"zzz\x1b[1m\nab");
        assert_eq!(r.snapshot(), b"\x1b[1m\nab");
        // not full: untouched
        let mut r = Ring::new(8);
        r.push(b"ab\ncd");
        assert_eq!(r.snapshot(), b"ab\ncd");
    }
    #[test]
    fn chunk_bigger_than_cap() {
        let mut r = Ring::new(3);
        r.push(b"123456");
        assert_eq!(r.snapshot(), b"456");
        assert_eq!(r.len(), 3);
    }
    #[test]
    fn push_evicted_returns_the_bytes_that_fell_out_in_order() {
        let mut r = Ring::new(5);
        assert!(r.push_evicted(b"abc").is_empty());
        assert_eq!(r.push_evicted(b"def"), b"a");
        assert_eq!(r.snapshot(), b"bcdef");
        // chunk bigger than the cap: the old buffer and the head of the chunk are evicted
        assert_eq!(r.push_evicted(b"0123456789"), b"bcdef01234");
        assert_eq!(r.snapshot(), b"56789");
    }
}
