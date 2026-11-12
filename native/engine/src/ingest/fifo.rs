//! The decoded samples waiting to play: a fixed ring the jitter buffer fills a frame at a time
//! and empties a block at a time.

pub struct Fifo {
    buf: Vec<f32>,
    head: usize,
    len: usize,
}

impl Fifo {
    pub fn new(capacity: usize) -> Self {
        Self { buf: vec![0.0; capacity], head: 0, len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Appends what fits and returns how many samples that was.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let n = samples.len().min(self.buf.len() - self.len);
        for (i, sample) in samples[..n].iter().enumerate() {
            let at = (self.head + self.len + i) % self.buf.len();
            self.buf[at] = *sample;
        }
        self.len += n;
        n
    }

    pub fn push_silence(&mut self, count: usize) -> usize {
        let n = count.min(self.buf.len() - self.len);
        for i in 0..n {
            let at = (self.head + self.len + i) % self.buf.len();
            self.buf[at] = 0.0;
        }
        self.len += n;
        n
    }

    /// Puts silence ahead of what is waiting, as much as fits, and returns how much that was.
    pub fn push_front_silence(&mut self, count: usize) -> usize {
        let n = count.min(self.buf.len() - self.len);
        self.head = (self.head + self.buf.len() - n) % self.buf.len();
        for i in 0..n {
            let at = (self.head + i) % self.buf.len();
            self.buf[at] = 0.0;
        }
        self.len += n;
        n
    }

    /// The loudest of the next `count` samples, or of all of them when fewer wait.
    pub fn peak(&self, count: usize) -> f32 {
        (0..count.min(self.len))
            .map(|i| self.buf[(self.head + i) % self.buf.len()].abs())
            .fold(0.0, f32::max)
    }

    pub fn pop(&mut self) -> Option<f32> {
        if self.len == 0 {
            return None;
        }
        let sample = self.buf[self.head];
        self.head = (self.head + 1) % self.buf.len();
        self.len -= 1;
        Some(sample)
    }

    pub fn discard(&mut self, count: usize) {
        let n = count.min(self.len);
        self.head = (self.head + n) % self.buf.len();
        self.len -= n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_come_out_in_order_across_the_wrap() {
        let mut fifo = Fifo::new(4);
        assert_eq!(fifo.push(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(fifo.pop(), Some(1.0));
        assert_eq!(fifo.push(&[4.0, 5.0, 6.0]), 2, "only what fits");
        fifo.discard(1);
        assert_eq!(fifo.push_silence(1), 1);
        assert_eq!(fifo.peak(2), 4.0);
        let out: Vec<_> = std::iter::from_fn(|| fifo.pop()).collect();
        assert_eq!(out, vec![3.0, 4.0, 5.0, 0.0]);
        fifo.push(&[1.0, 2.0]);
        assert_eq!(fifo.push_front_silence(3), 2, "only what fits");
        let out: Vec<_> = std::iter::from_fn(|| fifo.pop()).collect();
        assert_eq!(out, vec![0.0, 0.0, 1.0, 2.0]);
        assert!(fifo.is_empty());
    }
}
