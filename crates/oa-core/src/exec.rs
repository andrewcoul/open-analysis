//! Thread budget for one analysis. A request with `threads > 0` runs every
//! stage, from element preparation through factorization and combination
//! solves, inside a pool of that size; faer's kernels read the current pool's
//! size, so the budget covers them too. Pools are cached by size, so repeated
//! calls do not pay thread creation. Zero uses whatever pool the caller is in.

#[cfg(feature = "parallel")]
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(feature = "parallel")]
use crate::Error;
use crate::Result;

/// Element loops shorter than this run serially; the work is too small to
/// amortise scheduling.
#[cfg(feature = "parallel")]
pub(crate) const PARALLEL_THRESHOLD: usize = 256;
/// Elements per chunk-local triplet buffer during assembly.
#[cfg(feature = "parallel")]
pub(crate) const ASSEMBLY_CHUNK: usize = 256;

#[cfg(feature = "parallel")]
pub(crate) struct Exec(Option<Arc<rayon::ThreadPool>>);
#[cfg(not(feature = "parallel"))]
pub(crate) struct Exec;

#[cfg(feature = "parallel")]
impl Exec {
    pub fn new(threads: usize) -> Result<Self> {
        if threads == 0 {
            return Ok(Self(None));
        }
        static POOLS: OnceLock<Mutex<HashMap<usize, Arc<rayon::ThreadPool>>>> = OnceLock::new();
        let mut pools = POOLS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| Error::Request("thread pool cache poisoned".into()))?;
        if let Some(pool) = pools.get(&threads) {
            return Ok(Self(Some(pool.clone())));
        }
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(move |i| format!("oa-{threads}-{i}"))
                .build()
                .map_err(|e| Error::Request(e.to_string()))?,
        );
        pools.insert(threads, pool.clone());
        Ok(Self(Some(pool)))
    }
    /// Runs `f` on a worker of the selected pool, so nested parallel iterators
    /// and faer kernels use that pool.
    pub fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        match &self.0 {
            Some(pool) => pool.install(f),
            None => f(),
        }
    }
    /// Runs `f` on the calling thread; work spawned on the scope runs in the
    /// selected pool. Lets a non-`Send` consumer overlap with pool work.
    pub fn in_place_scope<'scope, R>(&self, f: impl FnOnce(&rayon::Scope<'scope>) -> R) -> R {
        match &self.0 {
            Some(pool) => pool.in_place_scope(f),
            None => rayon::in_place_scope(f),
        }
    }
}
#[cfg(not(feature = "parallel"))]
impl Exec {
    pub fn new(_threads: usize) -> Result<Self> {
        Ok(Self)
    }
    pub fn install<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}
