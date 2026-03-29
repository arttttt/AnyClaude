//! Model-View-Intent (MVI) architecture primitives.
//!
//! Provides [`Store`], [`Actor`], and [`ActorScope`] for implementing
//! unidirectional data flow.
//!
//! # Architecture
//!
//! ```text
//! Intent ──→ Store ──→ Actor ──→ reduce(State) ──→ View
//!                        │
//!                        └──→ SideEffect ──→ Callback
//! ```
//!
//! - **State**: immutable representation of component state
//! - **Intent**: user actions or system events
//! - **Actor**: business logic — handles intents, calls `reduce` and `side_effect`
//! - **Store**: container — owns state, actor, and side effect callback

use std::fmt::Debug;

/// Marker trait for state objects.
///
/// States should be:
/// - Immutable (Clone to create new states)
/// - Self-contained (all data needed to render the view)
/// - Comparable (PartialEq for detecting changes)
pub trait State: Clone + Debug + PartialEq + Default + Send + 'static {}

/// Marker trait for intent objects.
///
/// Intents represent:
/// - User actions (key presses, selections)
/// - System events (data loaded, timer fired)
///
/// Intents are dispatched to a Store and processed by its Actor.
pub trait Intent: Debug + Send + 'static {}

/// Actor encapsulates business logic for handling intents.
///
/// The actor receives intents and interacts with the store through
/// an [`ActorScope`], which provides access to state, reduce, and side effects.
pub trait Actor {
    type State: State;
    type Intent: Intent;
    type SideEffect;

    fn handle_intent(
        &self,
        intent: Self::Intent,
        scope: &mut ActorScope<Self::State, Self::SideEffect>,
    );
}

/// Context through which an [`Actor`] interacts with its [`Store`].
///
/// Provides read access to current state, state mutation via `reduce`,
/// and side effect emission.
pub struct ActorScope<S, E> {
    state: S,
    side_effects: Vec<E>,
}

impl<S: State, E> ActorScope<S, E> {
    fn new() -> Self {
        Self {
            state: S::default(),
            side_effects: Vec::new(),
        }
    }

    /// Current state (read-only).
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Update state via a reduce function.
    ///
    /// The function receives the current state by value and returns the new state.
    /// Note: if `f` panics, state will be left as `Default::default()`.
    pub fn reduce(&mut self, f: impl FnOnce(S) -> S) {
        self.state = f(std::mem::take(&mut self.state));
    }

    /// Emit a side effect to be handled by the store's callback.
    pub fn side_effect(&mut self, effect: E) {
        self.side_effects.push(effect);
    }
}

/// Store — container that owns state, actor, and side effect callback.
///
/// Each UI component owns its own Store. Dispatch an intent to trigger
/// the actor's business logic, which may update state and emit side effects.
pub struct Store<A: Actor> {
    scope: ActorScope<A::State, A::SideEffect>,
    actor: A,
    on_side_effect: Box<dyn FnMut(A::SideEffect)>,
}

impl<A: Actor> Store<A> {
    /// Create a new store with the given actor and side effect handler.
    ///
    /// State is initialized to `Default::default()`.
    pub fn new(actor: A, on_side_effect: impl FnMut(A::SideEffect) + 'static) -> Self {
        Self {
            scope: ActorScope::new(),
            actor,
            on_side_effect: Box::new(on_side_effect),
        }
    }

    /// Create a store with explicit initial state.
    pub fn with_state(
        state: A::State,
        actor: A,
        on_side_effect: impl FnMut(A::SideEffect) + 'static,
    ) -> Self {
        Self {
            scope: ActorScope {
                state,
                side_effects: Vec::new(),
            },
            actor,
            on_side_effect: Box::new(on_side_effect),
        }
    }

    /// Current state.
    pub fn state(&self) -> &A::State {
        &self.scope.state
    }

    /// Dispatch an intent to the actor.
    ///
    /// The actor processes the intent, potentially updating state via `reduce`
    /// and emitting side effects. Side effects are delivered to the callback
    /// registered at construction time.
    pub fn dispatch(&mut self, intent: A::Intent) {
        self.actor.handle_intent(intent, &mut self.scope);
        for effect in self.scope.side_effects.drain(..) {
            (self.on_side_effect)(effect);
        }
    }

    /// Dispatch an intent and return side effects instead of calling the callback.
    ///
    /// Use when the caller needs to handle side effects inline (e.g. when
    /// the effect requires access to resources not available to the callback).
    pub fn dispatch_collect(&mut self, intent: A::Intent) -> Vec<A::SideEffect> {
        self.actor.handle_intent(intent, &mut self.scope);
        self.scope.side_effects.drain(..).collect()
    }
}
