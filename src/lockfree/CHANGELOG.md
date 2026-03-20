# 0.5.2 (release)
* Added get_cloned adapter for map instead of get(key).map(|kv| kv.val().clone())

# 0.5.1 (release)
* Performance improvement.
* Sanity checks added.

# 0.5.0 (release)
* Introduced `CachedId` for TLS.
* TLS now lets the reference "escape" (read closure is not required anymore).
* TLS now has a immutable iterator when `T: Sync`.
* Removable accepts `Ordering` now.
* Removed `atomic` and `Darc`.

# 0.4.1 (release)
* Improved comparison of TLS's id.

# 0.4.0 (release)
* Added SPSC, MPSC, SPMC and MPMC channels.
* Added `removable`
* Fixed bad `Send` and `Sync` implementations.
* Added some iterators.
* Added some `FromIterator` and `Extend` implementations.
* Renamed `Queue` and `Stack` iterators to `PopIter`.
* Deprecated `atomic`, and `Darc`.

# 0.3.2 (release)
* Fixed a queue bug.

# 0.3.1 (release)
* Performance improvement.

# 0.3.0 (release)
* Added `AtomicOptionBox`.
* Added per-object `ThreadLocal`.
* Rewrote `Incinerator` so it would be per-object.
* Rewrote `Map` and `Set`. Now They have more flexible reading.

# 0.2.0 (release)
* Introduced Map.
* Introduced Set.
* Fixed design of Queue in order to make it really lockfree.
* Removed `Ordering`s from Darc.

# 0.1.X (release)
* Introduced Incinerator.
* Introduced Queue.
* Introduced Stack.
* Introduced Doubly-Atomic Reference Counter (Darc).
