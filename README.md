# Wasserglas

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/superfluffy/wasserglas)
[![Cargo](https://img.shields.io/crates/v/wasserglas.svg)](
https://crates.io/crates/wasserglas)
[![Documentation](https://docs.rs/wasserglas/badge.svg)](
https://docs.rs/wasserglas)

Wasserglas is a fixed-size thread-safe object pool with automatic reattachment
of dropped object.

The goal of an object pool is to reuse expensive to allocate objects or
frequently allocated objects. It was written when folding over a large number
of objects with [rayon] where the result of each job was to be stored in a
large structure, which was a) expensive to initialize and b) where the
job-stealing nature of rayon workers could potentially create too many such
structures, exceeding RAM.

This started out as a fork of CJP10's [object-pool](https://github.com/CJP10/object-pool),
but due to the fixed size changing the synchronization semantics significantly, I decided to
create a new crate.

## Usage

```toml
[dependencies]
object-pool = "0.1"
```

```rust
use wasserglas::pool;
```

## Examples

### Creating a Pool

A pool is created with a certain capacity. There should be about as many
objects in the pool as threads being used so that threads don't wait for an
object to be available. If there are less objects than threads trying to pull
from the pool, threads will block until an object is available.

Example pool with 16 `Vec<u8>`:

```rust
let capacity: usize = 16;
let pool: Pool<Vec<u32>> = Pool::new(capacity);

for _ in 0..capacity {
    assert!(pool.push(Vec::new()).is_ok());
}

// Pushing another object exceeding the capacity returns the object in error position.
assert_eq!(pool.push(Vec::new()), Err(Vec::new()));

// Pulling an object from the pool and dropping it reattaches it to the pool
assert_eq!(pool.n_available(), 16);
assert_eq!(pool.len(), 16);

let vec = pool.pull();

assert_eq!(pool.n_available(), 15);
assert_eq!(pool.len(), 16);

std::me::drop(vec);

assert_eq!(pool.n_available(), 16);
assert_eq!(pool.len(), 16);

// Detaching an object removes it from the pool permanently
let vec = pool.pull();

assert_eq!(pool.n_available(), 15);
assert_eq!(pool.len(), 16);

let vec = vec.detach();

assert_eq!(pool.n_available(), 15);
assert_eq!(pool.len(), 15);
```

### Using a Pool

Since the intended use for the pool is in a multi threaded environment, it
should usually be wrapped in an Arc:

```rust
use std::sync::Arc;

use ray::prelude::*;

let num_threads = rayon::current_num_threads();
let pool: Arc<Pool<Vec<u32>>> = Arc::new(Pool::new(num_threads));
for _ in 0..num_threads {
    assert!(pool.push(Vec::new()).is_ok());
}

data.par_iter().for_each_init(|| pool.clone(), |pool, item| {
    let mut buffer = pool.pull();
    perform_calculation(&item, &mut buffer);
    // Upon finishing, the buffer is collected and put into the pool.
});
```

If construction of the pools objects should be spread out over all threads instead of being
preloaded sequentially:

```rust
data.par_iter().for_each_init(|| pool.clone(), |pool, item| {
    let mut buffer = pool.pull_or_else(|| Vec::new());
    perform_calculation(&item, &mut buffer);

    // The buffer needs to be explicitly cleared if that's required.
    buffer.clear();
});
```
