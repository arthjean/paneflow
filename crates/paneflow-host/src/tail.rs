use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailEvicted {
    pub requested: u64,
    pub tail_start: u64,
    pub tail_end: u64,
}

pub struct OutputTail {
    capacity: usize,
    start: u64,
    bytes: VecDeque<u8>,
}

impl OutputTail {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            start: 0,
            bytes: VecDeque::new(),
        }
    }

    pub fn start_offset(&self) -> u64 {
        self.start
    }

    pub fn end_offset(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    pub fn append(&mut self, chunk: &[u8]) {
        if chunk.len() >= self.capacity {
            let keep = &chunk[chunk.len() - self.capacity..];
            self.start = self.end_offset() + (chunk.len() - keep.len()) as u64;
            self.bytes.clear();
            self.bytes.extend(keep);
            return;
        }
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(self.capacity);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.start += excess as u64;
        }
    }

    pub fn read_from(&self, offset: u64, max: usize) -> Result<(u64, Vec<u8>), TailEvicted> {
        let end = self.end_offset();
        if offset < self.start || offset > end {
            return Err(TailEvicted {
                requested: offset,
                tail_start: self.start,
                tail_end: end,
            });
        }
        let skip = (offset - self.start) as usize;
        let take = (self.bytes.len() - skip).min(max);
        let data: Vec<u8> = self.bytes.iter().skip(skip).take(take).copied().collect();
        Ok((offset, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_monotonic_and_eviction_moves_the_start() {
        let mut tail = OutputTail::new(8);
        tail.append(b"abcd");
        assert_eq!((tail.start_offset(), tail.end_offset()), (0, 4));
        tail.append(b"efgh");
        assert_eq!((tail.start_offset(), tail.end_offset()), (0, 8));
        tail.append(b"ij");
        assert_eq!((tail.start_offset(), tail.end_offset()), (2, 10));
        assert_eq!(tail.read_from(2, 100).unwrap(), (2, b"cdefghij".to_vec()));
        assert_eq!(tail.read_from(9, 100).unwrap(), (9, b"j".to_vec()));
        assert_eq!(tail.read_from(10, 100).unwrap(), (10, Vec::new()));
        assert_eq!(
            tail.read_from(1, 100).unwrap_err(),
            TailEvicted {
                requested: 1,
                tail_start: 2,
                tail_end: 10
            }
        );
        assert!(
            tail.read_from(11, 100).is_err(),
            "the future is not readable"
        );
    }

    #[test]
    fn a_chunk_larger_than_the_tail_keeps_only_its_end() {
        let mut tail = OutputTail::new(4);
        tail.append(b"ab");
        tail.append(b"0123456789");
        assert_eq!((tail.start_offset(), tail.end_offset()), (8, 12));
        assert_eq!(tail.read_from(8, 2).unwrap(), (8, b"67".to_vec()));
    }
}
