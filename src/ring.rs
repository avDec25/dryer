//! Fixed-capacity clip history. Slot 0 is the newest clip.

use crate::clipboard::Clip;

pub const CAPACITY: usize = 10;

pub struct Ring {
    slots: [Option<Clip>; CAPACITY],
    len: usize,
}

impl Ring {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn get(&self, i: usize) -> Option<&Clip> {
        if i < self.len { self.slots[i].as_ref() } else { None }
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut Clip> {
        if i < self.len { self.slots[i].as_mut() } else { None }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Clip> {
        self.slots[..self.len].iter_mut().flatten()
    }

    /// Content hashes in slot order (newest first).
    #[cfg(test)]
    pub fn hashes(&self) -> Vec<u64> {
        self.slots[..self.len].iter().flatten().map(|c| c.hash).collect()
    }

    /// Insert as the newest clip. An identical clip already on the line is moved
    /// to the front instead of duplicated. At capacity, the oldest clip is removed
    /// and returned so it can animate away.
    pub fn insert(&mut self, clip: Clip) -> Option<Clip> {
        let dup = self.slots[..self.len]
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.hash == clip.hash));
        let mut evicted = None;
        match dup {
            Some(p) => self.slots[..=p].rotate_right(1),
            None => {
                if self.len == CAPACITY {
                    evicted = self.slots[CAPACITY - 1].take();
                } else {
                    self.len += 1;
                }
                self.slots[..self.len].rotate_right(1);
            }
        }
        self.slots[0] = Some(clip);
        evicted
    }

    /// Move the clip at `i` to the front (newest).
    pub fn move_to_front(&mut self, i: usize) {
        if i < self.len {
            self.slots[..=i].rotate_right(1);
        }
    }

    /// Drop cached GPU bitmaps (after a device loss or resize).
    pub fn clear_gpu_cache(&mut self) {
        for c in self.iter_mut() {
            c.thumb = None;
            c.sprite = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::MIN_CARD_H;
    use crate::clipboard::Kind;
    use crate::physics::Motion;

    fn clip(hash: u64) -> Clip {
        Clip {
            kind: Kind::Text,
            formats: Vec::new(),
            hash,
            preview: String::new(),
            thumb_px: None,
            thumb: None,
            sprite: None,
            layout: None,
            time_text: String::new(),
            time_layout: None,
            text_h: 0.0,
            card_h: MIN_CARD_H,
            motion: Motion::default(),
        }
    }

    fn hashes(r: &Ring) -> Vec<u64> {
        r.hashes()
    }

    #[test]
    fn keeps_only_ten_newest_and_returns_evicted() {
        let mut r = Ring::new();
        let mut evicted = Vec::new();
        for h in 1..=12 {
            if let Some(c) = r.insert(clip(h)) {
                evicted.push(c.hash);
            }
        }
        assert_eq!(r.len(), CAPACITY);
        assert_eq!(hashes(&r), vec![12, 11, 10, 9, 8, 7, 6, 5, 4, 3]);
        assert_eq!(evicted, vec![1, 2]);
    }

    #[test]
    fn duplicate_moves_to_front_without_growing() {
        let mut r = Ring::new();
        for h in 1..=4 {
            r.insert(clip(h));
        }
        assert_eq!(hashes(&r), vec![4, 3, 2, 1]);
        assert!(r.insert(clip(2)).is_none());
        assert_eq!(r.len(), 4);
        assert_eq!(hashes(&r), vec![2, 4, 3, 1]);
    }

    #[test]
    fn move_to_front_reorders() {
        let mut r = Ring::new();
        for h in 1..=3 {
            r.insert(clip(h));
        }
        r.move_to_front(2);
        assert_eq!(hashes(&r), vec![1, 3, 2]);
    }
}
