//! Thin wrapper over the work-stealing thread pool. The only module that
//! imports `rayon`, so the rest of the crate depends on a two-function API.

use rayon::prelude::*;

/// Apply `f` to every item in parallel; results keep input order.
pub fn map<I, R, F>(items: Vec<I>, f: F) -> Vec<R>
where
    I: Send,
    R: Send,
    F: Fn(I) -> R + Sync + Send,
{
    items.into_par_iter().map(f).collect()
}

/// Run `f` on every item in parallel, discarding results.
#[allow(dead_code)]
pub fn for_each<I, F>(items: Vec<I>, f: F)
where
    I: Send,
    F: Fn(I) + Sync + Send,
{
    items.into_par_iter().for_each(f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn map_preserves_order() {
        let got = map((0..100).collect(), |i| i * 2);
        assert_eq!(got, (0..100).map(|i| i * 2).collect::<Vec<_>>());
    }

    #[test]
    fn for_each_visits_every_item() {
        let n = AtomicUsize::new(0);
        for_each((0..50).collect::<Vec<u32>>(), |_| {
            n.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(n.load(Ordering::SeqCst), 50);
    }
}
