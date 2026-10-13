use serde::{Deserialize, Serialize};

pub const MIN_SEGMENT_SIZE: u64 = 256 * 1024;

/// A contiguous inclusive byte range of the target file and how much of it is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    #[serde(default)]
    pub written: u64,
}

impl Segment {
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end, written: 0 }
    }

    pub fn next_offset(&self) -> u64 {
        self.start + self.written
    }

    pub fn is_done(&self) -> bool {
        self.next_offset() > self.end
    }

    pub fn remaining(&self) -> u64 {
        (self.end + 1).saturating_sub(self.next_offset())
    }
}

/// Splits `length` bytes into at most `count` segments, none smaller than `min_size`
/// (except when the whole file is smaller than that).
pub fn plan(length: u64, count: usize, min_size: u64) -> Vec<Segment> {
    if length == 0 {
        return Vec::new();
    }
    let by_min_size = (length / min_size.max(1)).max(1);
    let n = (count as u64).min(by_min_size).max(1);
    let chunk = length.div_ceil(n);
    let mut segments = Vec::with_capacity(n as usize);
    let mut start = 0;
    while start < length {
        let end = (start + chunk - 1).min(length - 1);
        segments.push(Segment::new(start, end));
        start = end + 1;
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_covers_range_exactly_without_gaps() {
        for length in [1u64, 100, 262_144, 1_000_001, 50_000_000] {
            for count in [1usize, 3, 8, 32] {
                let segs = plan(length, count, MIN_SEGMENT_SIZE);
                assert_eq!(segs.first().unwrap().start, 0);
                assert_eq!(segs.last().unwrap().end, length - 1);
                for w in segs.windows(2) {
                    assert_eq!(w[0].end + 1, w[1].start);
                }
                assert!(segs.len() <= count);
            }
        }
    }

    #[test]
    fn small_file_gets_one_segment() {
        assert_eq!(plan(1000, 8, MIN_SEGMENT_SIZE).len(), 1);
    }
}
