/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # Controller task-posting seam
//!
//! [`ControllerQueue<C>`] is a generic, processor-agnostic handle that lets a
//! controller schedule deferred / async follow-up work targeting its own state
//! `&mut C` — **without ever naming `SessionProcessor`**.
//!
//! ## Borrowing backend view
//!
//! Deferred work usually needs a few reads/effects that live on
//! `SessionProcessor` (e.g. the finalized head, or lowering the main-loop wake).
//! Capturing those would force shared / `'static` handles; instead each
//! controller declares — via [`Controlled`] — a *borrowing* backend view
//! `Backend<'b>` (typically `dyn SomeBackend + 'b`). A task is therefore
//! `for<'b> FnOnce(&mut C, &'b mut C::Backend<'b>)`: at drain time the
//! composition root builds a fresh borrowing backend from `&mut
//! SessionProcessor`, runs the task, and drops it (RAII). The backend ref is
//! `&mut`, so the split-borrow adapter can hold `&mut` references to disjoint
//! `SessionProcessor` fields and the task can drive backend *effects* (not just
//! reads) without interior mutability. Nothing is captured, there is no shared
//! mutable state, and every backend read is current. A new controller reuses
//! the entire seam by implementing [`Controlled`] with its own backend trait.
//!
//! The abstraction names only the controller type `C`. The concrete adapter
//! that knows how to get from `&mut SessionProcessor` to `&mut C` (the field
//! projection) is built once at the composition root
//! (`session_processor.rs`); controllers — and, crucially, their unit tests —
//! depend on this trait alone. A test can therefore drive a controller that
//! posts work by supplying a recording fake in place of the real session main
//! loop, with no `SessionProcessor` anywhere (see
//! `tests/test_controller_queue.rs`).
//!
//! Object-safety: the vtable method [`ControllerQueue::post_boxed`] takes a
//! *boxed* closure (no generics); the ergonomic generic
//! [`ControllerQueueExt::post`] is layered on via a blanket-implemented
//! extension trait. The trait is `Send + Sync` so a handle clone can be moved
//! into an off-thread async callback (e.g. the higher-layer validation
//! decision callback) and post back onto the main loop.

use std::{sync::Arc, time::SystemTime};

/// A controller that can be re-entered by the queue with a transient backend.
///
/// `Backend<'b>` is the borrowing view a deferred task receives (by `&mut`)
/// alongside `&mut C` — typically a `dyn SomeBackend + 'b` trait object built
/// fresh from `&mut SessionProcessor` at drain time. Implemented once per
/// controller; the
/// queue, the [`ControllerTask`] alias, and the recording test fake are all
/// generic over it, so a new controller reuses the whole seam by implementing
/// this trait alone.
pub(crate) trait Controlled {
    /// Transient, borrow-scoped backend view handed to deferred tasks.
    ///
    /// `?Sized` so it can be a `dyn SomeBackend + 'b` trait object.
    type Backend<'b>: ?Sized;
}

/// A boxed unit of deferred work targeting controller state `C`, re-entered with
/// a freshly-built backend view `C::Backend<'b>` (RAII: dropped after the call).
///
/// Higher-ranked over the backend lifetime `'b`: a single `'static` boxed task
/// accepts a backend borrowed only for the drain. A plain backend *type
/// parameter* would instead pin the view to `'static` and bar a borrowing
/// adapter — the constraint that shaped this seam.
///
/// Aliased so the nested queue storage in fakes and the vtable signatures stay
/// readable (and below clippy's type-complexity threshold).
pub(crate) type ControllerTask<C> =
    Box<dyn for<'b> FnOnce(&mut C, &'b mut <C as Controlled>::Backend<'b>) + Send>;

/// A queue that accepts deferred work targeting controller state `C`.
///
/// Object-safe: the vtable methods take boxed closures. Use the generic
/// [`ControllerQueueExt`] methods (`post` / `post_delayed`) at call sites.
pub(crate) trait ControllerQueue<C: Controlled>: Send + Sync {
    /// Schedule `task` to run against `&mut C` (with a fresh backend view) on
    /// the session main loop as soon as the queue is drained.
    fn post_boxed(&self, task: ControllerTask<C>);

    /// Schedule `task` to run against `&mut C` (with a fresh backend view) once
    /// `at` is reached.
    fn post_delayed_boxed(&self, at: SystemTime, task: ControllerTask<C>);
}

/// Cheap-to-clone handle a controller stores. Names only `C`.
pub(crate) type ControllerQueuePtr<C> = Arc<dyn ControllerQueue<C>>;

/// Ergonomic generic posting layered over the object-safe [`ControllerQueue`].
///
/// Blanket-implemented for every `ControllerQueue<C>` (including
/// `dyn ControllerQueue<C>` behind an `Arc`), so callers write
/// `queue.post(|c, b| …)` without boxing by hand or spelling the `for<'b>`.
///
/// Both methods are live: `post_delayed` drives the validation-retry re-entry
/// ([`ValidationController::candidate_decision_fail`](crate::validation_controller::ValidationController))
/// and the collation failure-retry; the immediate `post` drives the off-thread
/// collation completion callback (`SessionProcessor::make_collation_callback`).
pub(crate) trait ControllerQueueExt<C: Controlled>: ControllerQueue<C> {
    /// Post `f` to run against `&mut C` (with a fresh backend view) on the
    /// session main loop.
    fn post<F>(&self, f: F)
    where
        F: for<'b> FnOnce(&mut C, &'b mut C::Backend<'b>) + Send + 'static,
    {
        self.post_boxed(Box::new(f));
    }

    /// Post `f` to run against `&mut C` (with a fresh backend view) once `at`
    /// is reached.
    fn post_delayed<F>(&self, at: SystemTime, f: F)
    where
        F: for<'b> FnOnce(&mut C, &'b mut C::Backend<'b>) + Send + 'static,
    {
        self.post_delayed_boxed(at, Box::new(f));
    }
}

impl<C: Controlled, Q: ControllerQueue<C> + ?Sized> ControllerQueueExt<C> for Q {}

#[cfg(test)]
#[path = "tests/test_controller_queue.rs"]
mod tests;
