use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use obi_core::graph::KnowledgeGraph;
use obi_core::node::NodeId;

use super::layout::{ForceLayout, LayoutConfig};
use super::snapshot::{GraphSnapshot, ViewMode};

/// Shared state between layout thread and TUI thread.
struct SharedState {
    /// Latest snapshot for rendering.
    snapshot: GraphSnapshot,
    /// Pending graph to load (set by TUI thread, consumed by layout thread).
    pending_graph: Option<KnowledgeGraph>,
    /// Pending view mode change.
    pending_view_mode: Option<(ViewMode, Option<NodeId>)>,
    /// Should the layout thread stop?
    shutdown: bool,
    /// Wake signal: layout thread sleeps when converged, wakes on this.
    wake: bool,
    /// Pinned node IDs.
    pinned: HashSet<NodeId>,
    /// Excluded node IDs.
    excluded: HashSet<NodeId>,
    /// Anchor node IDs from context builder.
    context_anchors: HashSet<NodeId>,
    /// In-context node IDs from context builder.
    context_in_context: HashSet<NodeId>,
}

/// Lock a mutex, recovering from poison (the layout thread may have panicked).
fn lock_or_recover(mutex: &Mutex<SharedState>) -> MutexGuard<'_, SharedState> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Handle for the TUI thread to interact with the background layout thread.
pub struct LayoutThread {
    state: Arc<Mutex<SharedState>>,
    condvar: Arc<Condvar>,
    _handle: std::thread::JoinHandle<()>,
}

impl LayoutThread {
    /// Spawn the background layout thread.
    pub fn spawn(config: LayoutConfig) -> Self {
        let state = Arc::new(Mutex::new(SharedState {
            snapshot: GraphSnapshot::default(),
            pending_graph: None,
            pending_view_mode: None,
            shutdown: false,
            wake: false,
            pinned: HashSet::new(),
            excluded: HashSet::new(),
            context_anchors: HashSet::new(),
            context_in_context: HashSet::new(),
        }));
        let condvar = Arc::new(Condvar::new());

        let thread_state = Arc::clone(&state);
        let thread_condvar = Arc::clone(&condvar);

        let handle = std::thread::Builder::new()
            .name("graph-layout".into())
            .stack_size(4 * 1024 * 1024) // 4MB stack for large graphs
            .spawn(move || {
                run_layout_loop(thread_state, thread_condvar, config);
            })
            .expect("failed to spawn graph layout thread");

        Self {
            state,
            condvar,
            _handle: handle,
        }
    }

    /// Send a new graph to the layout thread.
    pub fn load_graph(&self, graph: KnowledgeGraph) {
        let mut state = lock_or_recover(&self.state);
        state.pending_graph = Some(graph);
        state.wake = true;
        self.condvar.notify_one();
    }

    /// Request a view mode change.
    pub fn set_view_mode(&self, mode: ViewMode, selected: Option<NodeId>) {
        let mut state = lock_or_recover(&self.state);
        state.pending_view_mode = Some((mode, selected));
        state.wake = true;
        self.condvar.notify_one();
    }

    /// Wake the layout thread (e.g., after user drag).
    pub fn wake(&self) {
        let mut state = lock_or_recover(&self.state);
        state.wake = true;
        self.condvar.notify_one();
    }

    /// Get the latest snapshot for rendering.
    pub fn snapshot(&self) -> GraphSnapshot {
        let state = lock_or_recover(&self.state);
        state.snapshot.clone()
    }

    /// Update all context state sets (pinned, excluded, anchors, in-context).
    pub fn update_context_states(
        &self,
        pinned: HashSet<NodeId>,
        excluded: HashSet<NodeId>,
        anchors: HashSet<NodeId>,
        in_context: HashSet<NodeId>,
    ) {
        let mut state = lock_or_recover(&self.state);
        state.pinned = pinned;
        state.excluded = excluded;
        state.context_anchors = anchors;
        state.context_in_context = in_context;
    }

    /// Signal the layout thread to shut down.
    pub fn shutdown(&self) {
        let mut state = lock_or_recover(&self.state);
        state.shutdown = true;
        state.wake = true;
        self.condvar.notify_one();
    }
}

impl Drop for LayoutThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Main loop for the background layout thread.
/// Wrapped in catch_unwind to prevent poisoning the mutex on panic.
fn run_layout_loop(
    state: Arc<Mutex<SharedState>>,
    condvar: Arc<Condvar>,
    config: LayoutConfig,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_layout_loop_inner(&state, &condvar, config);
    }));

    if let Err(e) = result {
        // Layout thread panicked — mark shutdown so the main thread doesn't hang
        if let Ok(mut s) = state.lock() {
            s.shutdown = true;
        }
        eprintln!("graph-layout thread panicked: {:?}", e);
    }
}

fn run_layout_loop_inner(
    state: &Arc<Mutex<SharedState>>,
    condvar: &Arc<Condvar>,
    config: LayoutConfig,
) {
    let tick_interval = Duration::from_millis(config.tick_interval_ms);
    let mut layout = ForceLayout::new(config);

    loop {
        // Check for pending commands
        {
            let mut s = lock_or_recover(state);
            if s.shutdown {
                return;
            }

            // Load new graph if pending
            if let Some(graph) = s.pending_graph.take() {
                layout.load_graph(&graph);
            }

            // Apply view mode if pending
            if let Some((mode, selected)) = s.pending_view_mode.take() {
                layout.apply_view_mode(mode, selected);
            }

            s.wake = false;
        }

        // Run one simulation tick
        let changed = layout.tick();

        // Publish snapshot
        {
            let s = lock_or_recover(state);
            let snapshot = layout.snapshot(
                &s.pinned, &s.excluded, &s.context_anchors, &s.context_in_context,
            );
            drop(s);

            let mut s = lock_or_recover(state);
            s.snapshot = snapshot;
        }

        if !changed || layout.is_converged() {
            // Sleep until woken (graph change, user action, or shutdown)
            let mut s = lock_or_recover(state);
            // Publish final converged snapshot before sleeping
            s.snapshot = layout.snapshot(
                &s.pinned, &s.excluded, &s.context_anchors, &s.context_in_context,
            );

            while !s.wake && !s.shutdown {
                let result = condvar.wait(s);
                s = match result {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
            }

            if s.shutdown {
                return;
            }
            continue;
        }

        // Sleep for tick interval
        std::thread::sleep(tick_interval);
    }
}
