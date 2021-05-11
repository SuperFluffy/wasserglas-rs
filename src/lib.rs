use std::{
    collections::VecDeque,
    mem::{
        ManuallyDrop,
        forget
    },
    ops::{
        Deref,
        DerefMut,
    },
    sync::atomic::{
        AtomicUsize,
        Ordering,
    },
};

use parking_lot::{
    Condvar,
    Mutex,
};

/// A thread safe object pool whose objects get automatically reattached upon drop.
pub struct Pool<T> {
    objects: Mutex<VecDeque<T>>,
    object_available: Condvar,
    n_live_objects: AtomicUsize,
    max_capacity: usize,
}

impl<T> Pool<T> {
    fn attach(&self, object: T) {
        let mut object_lock = self.objects.lock();
        object_lock.push_back(object);
        self.object_available.notify_one();
    }

    /// Associates an object with the pool without immediately pushing into it, but returns the
    /// object wrapped in a `Reattach` so that it gets pushed into the pool once it goes
    /// out of scope.
    ///
    /// Returns the object in error position if the pool is full.
    pub fn associate(&self, object: T) -> Result<Reattach<'_, T>, T> {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return Err(object);
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        Ok(Reattach::new(self, object))
    }

    /// Apply a function to all objects in the pool. If the function is to be applied to all
    /// functions in the pool, the caller should ensure that the pool is not currently used in a 
    /// parallel context.
    pub fn apply<F>(&self, f: F)
    where
        F: Fn(&mut T),
    {
        self.objects.lock().iter_mut().for_each(f);
    }

    /// Apply a fallible function to all objects in the pool. If the function is to be applied to all
    /// functions in the pool, the caller should ensure that the pool is not currently used in a 
    /// parallel context.
    pub fn try_apply<E, F>(&self, f: F) -> Result<(), E>
    where
        F: Fn(&mut T) -> Result<(), E>,
    {
        self.objects.lock().iter_mut().try_for_each(f)
    }

    /// Constructs an object from a closure and associates it with the pool without immediately
    /// pushing into it, but returns the object immediately wrapped in a `Reattach` so that it gets
    /// pushed into the pool once it goes out of scope.
    ///
    /// Use this method if construction of the object should be dynamic (or is expensive) and should
    /// only be performed if the pool has space left.
    ///
    /// Returns `None` if the pool was at capacity and no object was constructed.
    pub fn associate_with<F>(&self, f: F) -> Option<Reattach<'_, T>>
    where
        F: Fn() -> T,
    {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return None;
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        let object = f();
        Some(Reattach::new(self, object))
    }

    /// Constructs an object from a fallible closure and associates it with the pool without
    /// immediately pushing into it, but returns the object immediately wrapped in a `Reattach` so
    /// that it gets pushed into the pool once it goes out of scope.
    ///
    /// Use this method if construction of the object can fail, should be dynamic (oriis expensive)
    /// and should only be performed if the pool has space left.
    ///
    /// Returns `E` in error position if construction fails, and `None` in okay position if the pool
    /// was at capacity and no object was constructed.
    pub fn try_associate_with<E, F>(&self, f: F) -> Result<Option<Reattach<'_, T>>, E>
    where
        F: Fn() -> Result<T, E>,
    {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return Ok(None);
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        let object = f()?;
        Ok(Some(Reattach::new(self, object)))
    }

    /// Returns the maximum capacity of the object pool.
    pub fn capacity(&self) -> usize {
        self.max_capacity
    }

    /// Consume the pool and return the inner `VecDeque` with all objects.
    pub fn into_inner(self) -> VecDeque<T> {
        self.objects.into_inner()
    }

    /// Returns the length of the pool, i.e. the number of objects currently associated with it.
    /// Note that this means all objects associated with the thread pool, both immediately
    /// available to be pulled from the pool and currently in flight.
    pub fn len(&self) -> usize {
        let n_live = self.n_live_objects.load(Ordering::Acquire);
        n_live
    }

    /// Returns the number of objects available right now in the pool for pulling.
    pub fn n_available(&self) -> usize {
        self.objects.lock().len()
    }

    /// Returns the number of objects currently in flight, which is the the difference between all
    /// objects associated with the threadpool and those currently in the pool.
    pub fn n_in_flight(&self) -> usize {
        self.len() - self.n_available()
    }


    /// Creates a new fixed size object pool of `max_capacity`.
    ///
    /// Panics if `max_capacity < 1`
    pub fn new(max_capacity: usize) -> Self {
        assert!(max_capacity > 0);

        Self {
            objects: Mutex::new(VecDeque::with_capacity(max_capacity)),
            object_available: Condvar::new(),
            n_live_objects: AtomicUsize::new(0),
            max_capacity, 
        }
    }

    /// Push an object into the pool. Returns the object in error position if the pool was at max
    /// capacity.
    pub fn push(&self, object: T) -> Result<(), T> {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return Err(object);
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        self.attach(object);
        Ok(())
    }

    /// Constructs an object `T` using a closure if the object pool is not at capacity and pushes
    /// it into the pool. Returns `true` if the object was sucessfully pushed into the pool, and
    /// `false` otherwise.
    ///
    /// This method is useful if the object is supposed to be constructed dynamically (if for
    /// example construction of the object is expensive).
    pub fn push_with<F>(&self, f: F) -> bool
    where
        F: Fn() -> T,
    {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return false;
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        let object = f();
        self.attach(object);
        true
    }

    /// Constructs an object `T` using a fallible closure if the object pool is not at capacity,
    /// and pushes it into the pool. Returns `true` in okay position if the push was successful,
    /// `false` if it failed, and `E` in error position if constructing the object failed.
    ///
    /// This method is useful if the object is supposed to be constructed dynamically (if for
    /// example construction of the object is expensive) or if it can fail.
    pub fn try_push_with<F, E>(&self, f: F) -> Result<bool, E>
    where
        F: Fn() -> Result<T, E>,
    {
        let mut n_live_objects = self.n_live_objects.load(Ordering::Relaxed);
        loop {
            if n_live_objects >= self.max_capacity {
                return Ok(false);
            }
            let one_more_object = n_live_objects + 1;
            match self.n_live_objects.compare_exchange_weak(
                n_live_objects,
                one_more_object,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => break one_more_object,
                Err(x) => n_live_objects = x,
            }
        };
        let object = match f() {
            Ok(object) => object,
            Err(e) => {
                self.n_live_objects.fetch_sub(1, Ordering::Release);
                return Err(e);
            }
        };
        self.attach(object);
        Ok(true)
    }

    /// Pull an object from the pool. Blocks the current thread until an object becomes available
    /// to be pulled.
    pub fn pull(&self) -> Reattach<'_, T> {
        let mut objects_lock = self.objects.lock();
        while objects_lock.is_empty() {
            self.object_available.wait(&mut objects_lock);
        }
        objects_lock.pop_front().map(|object| Reattach::new(self, object)).unwrap()
    }

    /// Pull an object from the pool. Attempts exactly one pull and returns `None` if the pool
    /// was empty.
    pub fn pull_once(&self) -> Option<Reattach<'_, T>> {
        self.objects.lock().pop_front().map(|object| Reattach::new(self, object))
    }

    /// Pull an object from the pool or associate the supplied object with the pool if the
    /// pool has no available objects and is not yet at capacity. If the pool is at capacity the
    /// current thread will be blocked until an object becomes available to be pulled.
    pub fn pull_or(&self, object: T) -> Result<Reattach<'_, T>, T> {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.associate(object) {
                reusable_object @ Ok(_) => reusable_object,
                Err(_) => Ok(self.pull()),
            }
            Some(object) => Ok(object),
        }
    }

    /// Pull an object from the pool or associate the supplied object with the pool if the
    /// pool has no available objects an dis not yet at capacity.
    ///
    /// Attempts exactly one pull and returns `None` if the pool was empty.
    pub fn pull_or_once(&self, object: T) -> Result<Option<Reattach<'_, T>>, T> {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.associate(object) {
                Ok(reusable_object) => Ok(Some(reusable_object)),
                Err(object) => Err(object),
            }
            object @ Some(_) => Ok(object),
        }
    }

    /// Pull an object from the pool or construct an object with the supplied closure and associate
    /// it with the pool if the pool has no available objects and is not yet at capacity. If the
    /// pool is at capacity the current thread will be blocked until an object becomes available to
    /// be pulled.
    pub fn pull_or_else<F>(&self, f: F) -> Reattach<'_, T>
    where
        F: Fn() -> T,
    {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.associate_with(f) {
                Some(reusable_object) => reusable_object,
                None => self.pull(),
            }
            Some(object) => object,
        }
    }

    /// Pull an object from the pool or construct an object with the supplied closure and associate
    /// it with the pool if the pool has no available objects and is not yet at capacity.
    ///
    /// Attempts exactly one pull and returns `None` if the pool was empty.
    pub fn pull_or_else_once<F>(&self, f: F) -> Option<Reattach<'_, T>>
    where
        F: Fn() -> T,
    {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.associate_with(f) {
                reusable_object @ Some(_) => reusable_object,
                None => None,
            }
            object @ Some(_) => object,
        }
    }

    /// Pull an object from the pool or construct an object with the supplied fallible closure and
    /// associate it with the pool if the pool has no available objects and is not yet at capacity.
    /// If the pool is at capacity the current thread will be blocked until an object becomes
    /// available to be pulled.
    ///
    /// Returns the closure's error if object construction failed.
    pub fn try_pull_or_else<E, F>(&self, f: F) -> Result<Reattach<'_, T>, E>
    where
        F: Fn() -> Result<T, E>,
    {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.try_associate_with(f)? {
                Some(reusable_object) => Ok(reusable_object),
                None => Ok(self.pull()),
            }
            Some(object) => Ok(object),
        }
    }

    /// Pull an object from the pool or construct an object with the supplied fallible closure and
    /// associate it with the pool if the pool has no available objects and is not yet at capacity.
    ///
    /// Returns the closure's error if object construction failed.
    ///
    /// Attempts exactly one pull and returns `None` if the pool was empty.
    pub fn try_pull_or_else_once<E, F>(&self, f: F) -> Result<Option<Reattach<'_, T>>, E>
    where
        F: Fn() -> Result<T, E>,
    {
        // Attempt to pull an object from the pool; if this failed, construct a new object if
        // the pool is not yet at capacity. Otherwise wait until a new object is available.
        match self.objects.lock().pop_front().map(|object| Reattach::new(self, object)) {
            None => match self.try_associate_with(f)? {
                reusable_object @ Some(_) => Ok(reusable_object),
                None => Ok(None),
            }
            object @ Some(_) => Ok(object),
        }
    }
}

pub struct Reattach<'a, T> {
    data: ManuallyDrop<T>,
    pool: &'a Pool<T>,
}

impl<'a, T> Reattach<'a, T> {
    fn new(pool: &'a Pool<T>, t: T) -> Self {
        Self {
            data: ManuallyDrop::new(t),
            pool,
        }
    }

    pub fn detach(mut self) -> T {
        let (pool, object) = (self.pool, unsafe { ManuallyDrop::take(&mut self.data) });
        pool.n_live_objects.fetch_sub(1, Ordering::Relaxed);
        forget(self);
        object
    }
}

impl<'a, T> Deref for Reattach<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<'a, T> DerefMut for Reattach<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<'a, T> Drop for Reattach<'a, T> {
    fn drop(&mut self) {
        let inner = unsafe { ManuallyDrop::take(&mut self.data) };
        self.pool.attach(inner);
    }
}

#[cfg(test)]
mod tests {
    use crate::Pool;

    #[test]
    fn drop_reattaches() {
        let pool: Pool<Vec<usize>> = Pool::new(2);
        assert!(pool.push(Vec::new()).is_ok());
        let mut object = pool.pull().detach();
        object.push(1);
        // Not binding the return value of associate causes the object to be dropped.
        assert!(pool.associate(object).is_ok());
        assert_eq!(pool.pull()[0], 1);

        let object1 = pool.pull_once();
        let object2 = pool.pull_once();
        let object3 = pool.pull_or_else(|| Vec::new());

        assert!(object1.is_some());
        assert!(object2.is_none());
        drop(object1);
        drop(object2);
        drop(object3);
        assert_eq!(pool.n_available(), 2);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn dropping_intermediate_store_for_objects_reattaches_all_objects() {
        let pool: Pool<Vec<usize>> = Pool::new(8);
        let mut objects = Vec::new();
        for _ in 0..8 {
            assert!(pool.push_with(|| Vec::new()));
        }
        assert!(!pool.push_with(|| Vec::new()));

        for i in 0..8 {
            let mut object = pool.pull();
            object.push(i);
            objects.push(object);
        }

        assert!(pool.pull_once().is_none());
        assert_eq!(pool.n_available(), 0);
        std::mem::drop(objects);
        assert!(pool.pull_once().is_some());
        assert_eq!(pool.n_available(), pool.len());

        for i in 8..0 {
            let mut object = pool.pull_once().unwrap();
            assert_eq!(object.pop(), Some(i));
        }
    }
}
