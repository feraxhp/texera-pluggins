use texera_core::{
    tr, AllowMultipleSelections, Extension, IntoExtension, Selection, State,
};
use texera_pluggins::{
    add_to_history, history, isolate_history, redo_depth, redo_state, undo_depth, undo_selection,
    undo_state, CommandTarget, HistoryConfig, IsolateHistory,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Texera History Plugin Example ===");

    // 1. Create editor state with history extension configured
    let config = HistoryConfig::new()
        .min_depth(100)
        .new_group_delay(500);

    let mut state = State::from("Hello").with_extensions(Extension::from([
        history(config),
        AllowMultipleSelections(true).into_ext(),
    ]));

    println!("Initial text: {:?}", state.doc().to_string());
    println!("Initial undo depth: {}", undo_depth(&state));

    // 2. Perform some edits
    let doc_len = state.doc().len_chars();
    state = state
        .update(tr!({ changes: { from: doc_len, insert: ", world!" } }))?
        .into_state();
    println!("After edit 1: {:?}", state.doc().to_string());
    println!("Undo depth: {}", undo_depth(&state));

    // 3. Perform an isolated edit (won't merge with subsequent or previous edits)
    let doc_len = state.doc().len_chars();
    state = state
        .update(tr!({
            changes: { from: doc_len, insert: " How are you?" },
            annotations: [isolate_history(IsolateHistory::Before)]
        }))?
        .into_state();
    println!("After edit 2 (isolated): {:?}", state.doc().to_string());
    println!("Undo depth: {}", undo_depth(&state));

    // 4. Undo the last edit using `undo_state`
    let undid = undo_state(&mut state);
    println!("\nUndo performed? {}", undid);
    println!("After undo: {:?}", state.doc().to_string());
    println!("Undo depth: {}, Redo depth: {}", undo_depth(&state), redo_depth(&state));

    // 5. Redo the edit using `redo_state`
    let redid = redo_state(&mut state);
    println!("\nRedo performed? {}", redid);
    println!("After redo: {:?}", state.doc().to_string());

    // 6. Non-tracked change (e.g. remote synchronization) using `add_to_history(false)`
    state = state
        .update(tr!({
            changes: { from: 0, insert: ">>> " },
            annotations: [add_to_history(false)]
        }))?
        .into_state();
    println!("\nAfter untracked edit (remote change): {:?}", state.doc().to_string());

    // History can still undo tracked edits through untracked remote changes!
    undo_state(&mut state);
    println!("After undo through untracked changes: {:?}", state.doc().to_string());

    // 7. Using CommandTarget with standard CM6 dispatch pattern
    println!("\nUsing CommandTarget with custom dispatch:");
    state = state
        .update(tr!({ selection: Selection::cursor(0) }))?
        .into_state();

    let mut next_state = None;
    let mut dispatch = |spec| {
        println!("  Dispatch callback received transaction spec!");
        if let Ok(res) = state.clone().update(spec) {
            next_state = Some(res.into_state());
        }
    };

    let mut target = CommandTarget::new(&state, &mut dispatch);
    let ok = undo_selection(&mut target);
    if let Some(s) = next_state {
        state = s;
    }
    println!("Selection undo result: {}", ok);
    println!("Final text: {:?}", state.doc().to_string());

    println!("\nAll example steps completed successfully!");
    Ok(())
}
