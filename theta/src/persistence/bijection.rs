use std::hash::Hash;

use rustc_hash::FxHashMap;

pub struct Bijection<L, R> {
    left_to_right: FxHashMap<L, R>,
    right_to_left: FxHashMap<R, L>,
}

impl<L, R> Default for Bijection<L, R>
where
    L: Clone + Eq + Hash,
    R: Clone + Eq + Hash,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<L, R> Bijection<L, R>
where
    L: Clone + Eq + Hash,
    R: Clone + Eq + Hash,
{
    pub fn new() -> Self {
        Self {
            left_to_right: FxHashMap::default(),
            right_to_left: FxHashMap::default(),
        }
    }

    pub fn insert(&mut self, left: L, right: R) -> Option<(L, R)> {
        let old_right = self.left_to_right.insert(left.clone(), right.clone());
        let old_left = self.right_to_left.insert(right, left.clone());

        match (old_right, old_left) {
            (Some(old_right), Some(old_left)) => Some((old_left, old_right)),
            _ => None,
        }
    }

    pub fn get_left(&self, right: &R) -> Option<&L> {
        self.right_to_left.get(right)
    }

    pub fn get_right(&self, left: &L) -> Option<&R> {
        self.left_to_right.get(left)
    }

    pub fn contains_left(&self, left: &L) -> bool {
        self.left_to_right.contains_key(left)
    }

    pub fn contains_right(&self, right: &R) -> bool {
        self.right_to_left.contains_key(right)
    }

    pub fn remove_left(&mut self, left: &L) -> Option<R> {
        if let Some(right) = self.left_to_right.remove(left) {
            self.right_to_left.remove(&right);
            Some(right)
        } else {
            None
        }
    }

    pub fn remove_right(&mut self, right: &R) -> Option<L> {
        if let Some(left) = self.right_to_left.remove(right) {
            self.left_to_right.remove(&left);
            Some(left)
        } else {
            None
        }
    }
}
