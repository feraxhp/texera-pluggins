# Texera Plugins (`texera-pluggins`)

> Official plugins and extensions for the [Texera](https://github.com/feraxhp/texera-core) editor ecosystem.

This crate provides extensions and plugins ported from the CodeMirror 6 ecosystem to Rust, built on top of [`texera-core`](https://crates.io/crates/texera-core).

All credits for the original design and architecture go to `@Marijn Haverbeke` and the CodeMirror team.

---

## Available Plugins & Features

Each plugin in this crate is gated behind an optional Cargo feature so you only pay for what you use:

| Feature | Description | CodeMirror Equivalent |
| :--- | :--- | :--- |
| `history` | Undo/redo history management, selection history, inverted effects, and event grouping | `@codemirror/commands` (history plugin) |

---

## The `history` Plugin

The `history` plugin provides full undo/redo state management without requiring any view or DOM layer:

- **Configurable Grouping**:
  - Automatically merges adjacent typing events within a configurable delay (`new_group_delay`, default 500ms).
  - Configurable minimum history depth (`min_depth`, default 100).
  - Custom adjacency logic via `join_to_event` predicates.
- **Transaction Isolation**:
  - Prevent transactions from merging with previous or subsequent edits using `isolate_history(IsolateHistory::Before | After | Full)`.
- **Untracked Remote Changes**:
  - Transactions marked with `add_to_history(false)` (such as collaborative remote updates) are not added as undo items, but existing history items are accurately mapped through them.
- **Selection Undo/Redo**:
  - Undo and redo selection movements independently with `undo_selection` and `redo_selection`.
- **Inverted State Effects**:
  - Register callbacks via `InvertedEffects` facet to produce inverse `StateEffect`s that will be reverted on undo and re-applied on redo.
- **Flexible Command Dispatch**:
  - Direct state manipulation via `undo_state(&mut state)` / `redo_state(&mut state)`.
  - Classic dispatch target via `CommandTarget::new(&state, &mut dispatch)` for custom transaction pipelines.

---

## Quick Start

Add `texera-core` and `texera-pluggins` with the `history` feature to your `Cargo.toml`:

```toml
[dependencies]
texera-core = "0.99.0"
texera-pluggins = { version = "0.1.0", features = ["history"] }
```

### Example Usage

```rust
use texera_core::{tr, AllowMultipleSelections, Extension, IntoExtension, State};
use texera_pluggins::{
    add_to_history, history, isolate_history, redo_depth, redo_state, undo_depth, undo_state,
    HistoryConfig, IsolateHistory,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize state with the history extension
    let config = HistoryConfig::new()
        .min_depth(100)
        .new_group_delay(500);

    let mut state = State::from("Hello").with_extensions(Extension::from([
        history(config),
        AllowMultipleSelections(true).into_ext(),
    ]));

    // 2. Perform edits
    let doc_len = state.doc().len_chars();
    state = state
        .update(tr!({ changes: { from: doc_len, insert: ", world!" } }))?
        .into_state();

    assert_eq!(state.doc().to_string(), "Hello, world!");
    assert_eq!(undo_depth(&state), 1);

    // 3. Undo the change
    let undid = undo_state(&mut state);
    assert!(undid);
    assert_eq!(state.doc().to_string(), "Hello");
    assert_eq!(redo_depth(&state), 1);

    // 4. Redo the change
    let redid = redo_state(&mut state);
    assert!(redid);
    assert_eq!(state.doc().to_string(), "Hello, world!");

    // 5. Remote / non-tracked change
    state = state
        .update(tr!({
            changes: { from: 0, insert: ">>> " },
            annotations: [add_to_history(false)]
        }))?
        .into_state();

    // History restores tracked changes accurately through remote changes!
    undo_state(&mut state);
    assert_eq!(state.doc().to_string(), ">>> Hello");

    Ok(())
}
```

---

## Examples

Run the included example demonstrating history features:

```bash
cargo run --example history --features history
```

---

## Testing

Run the test suite:

```bash
cargo test --features history
```

---

## License

~~~
// Copyright (C) 2026 feraxhp
// 
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
// 
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU General Public License for more details.
// 
// You should have received a copy of the GNU General Public License
// along with this program. If not, see <https://www.gnu.org/licenses/>.
~~~
