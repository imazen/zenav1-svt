//! Disjoint mutable access for frame threading.
//!
//! Allows multiple threads to write to non-overlapping regions of a shared
//! buffer simultaneously. Essential for tile-parallel and segment-parallel
//! encoding.
//!
//! Pattern adapted from rav1d-disjoint-mut.
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// A buffer that allows index-based mutation with optional borrow tracking.
///
/// In debug builds (without `unchecked` feature), tracks active borrows
/// to detect overlapping access at runtime.
pub struct DisjointMut<T> {
    data: Vec<T>,
}

impl<T: Default + Clone> DisjointMut<T> {
    /// Create a new buffer filled with default values.
    pub fn new(size: usize) -> Self {
        Self {
            data: vec![T::default(); size],
        }
    }

    /// Create from existing data.
    pub fn from_vec(data: Vec<T>) -> Self {
        Self { data }
    }

    /// Get the total length.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Read a value at the given index.
    pub fn read(&self, index: usize) -> &T {
        &self.data[index]
    }

    /// Read a range of values.
    pub fn read_range(&self, start: usize, end: usize) -> &[T] {
        &self.data[start..end]
    }

    /// Write a value at the given index.
    pub fn write(&mut self, index: usize, value: T) {
        self.data[index] = value;
    }

    /// Write a range of values.
    pub fn write_range(&mut self, start: usize, values: &[T]) {
        self.data[start..start + values.len()].clone_from_slice(values);
    }

    /// Get a mutable slice of the entire buffer.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Get an immutable slice of the entire buffer.
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    /// Get a mutable sub-slice for a specific region.
    pub fn region_mut(&mut self, start: usize, end: usize) -> &mut [T] {
        &mut self.data[start..end]
    }
}

/// Region descriptor for disjoint access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

impl Region {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Self { start, end }
    }

    /// Check if two regions overlap.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Length of the region.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Borrow tracker for debug mode — verifies no overlapping borrows.
#[derive(Default)]
pub struct BorrowTracker {
    active: Vec<Region>,
}

impl BorrowTracker {
    pub fn new() -> Self {
        Self { active: Vec::new() }
    }

    /// Attempt to borrow a region. Panics if it overlaps an active borrow.
    pub fn borrow(&mut self, region: Region) {
        for active in &self.active {
            assert!(
                !region.overlaps(active),
                "Overlapping borrow: new {:?} overlaps active {:?}",
                region,
                active
            );
        }
        self.active.push(region);
    }

    /// Release a previously borrowed region.
    pub fn release(&mut self, region: Region) {
        if let Some(pos) = self.active.iter().position(|r| *r == region) {
            self.active.swap_remove(pos);
        }
    }

    /// Check if any borrows are active.
    pub fn has_active_borrows(&self) -> bool {
        !self.active.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_mut_basic() {
        let mut buf = DisjointMut::<u8>::new(64);
        buf.write(0, 42);
        buf.write(63, 99);
        assert_eq!(*buf.read(0), 42);
        assert_eq!(*buf.read(63), 99);
    }

    #[test]
    fn disjoint_mut_range() {
        let mut buf = DisjointMut::<u8>::new(16);
        buf.write_range(4, &[10, 20, 30, 40]);
        assert_eq!(buf.read_range(4, 8), &[10, 20, 30, 40]);
    }

    #[test]
    fn region_overlap() {
        let a = Region::new(0, 10);
        let b = Region::new(5, 15);
        let c = Region::new(10, 20);
        let d = Region::new(20, 30);

        assert!(a.overlaps(&b));
        assert!(b.overlaps(&a));
        assert!(!a.overlaps(&c));
        assert!(!a.overlaps(&d));
        assert!(!c.overlaps(&d));
    }

    #[test]
    fn region_adjacent_no_overlap() {
        let a = Region::new(0, 64);
        let b = Region::new(64, 128);
        assert!(!a.overlaps(&b));
        assert!(!b.overlaps(&a));
    }

    #[test]
    fn borrow_tracker_disjoint() {
        let mut tracker = BorrowTracker::new();
        tracker.borrow(Region::new(0, 64));
        tracker.borrow(Region::new(64, 128));
        assert!(tracker.has_active_borrows());
        tracker.release(Region::new(0, 64));
        tracker.release(Region::new(64, 128));
        assert!(!tracker.has_active_borrows());
    }

    #[test]
    #[should_panic(expected = "Overlapping borrow")]
    fn borrow_tracker_overlapping_panics() {
        let mut tracker = BorrowTracker::new();
        tracker.borrow(Region::new(0, 64));
        tracker.borrow(Region::new(32, 96));
    }

    #[test]
    fn disjoint_mut_from_vec() {
        let data = vec![1u8, 2, 3, 4];
        let buf = DisjointMut::from_vec(data);
        assert_eq!(buf.len(), 4);
        assert_eq!(*buf.read(2), 3);
    }

    #[test]
    fn region_mut_access() {
        let mut buf = DisjointMut::<u8>::new(16);
        let region = buf.region_mut(4, 8);
        region[0] = 100;
        region[3] = 200;
        assert_eq!(*buf.read(4), 100);
        assert_eq!(*buf.read(7), 200);
    }
}
