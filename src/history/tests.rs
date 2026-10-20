use texera_core::{
    AllowMultipleSelections, ChangeDesc, ChangeIterator, Effect, Extension, IntoExtension, MapMode,
    Selection, State, StateEffect, StateField, Transaction,
};

use super::{
    can_redo_selection, can_undo_selection, history, isolate_history, redo, redo_depth,
    redo_selection, undo, undo_depth, undo_selection, AddToHistory, CommandTarget, HistoryConfig,
    InvertedEffects, IsolateHistory, TransactionTime,
};

macro_rules! spec {
    ($($body:tt)*) => {
        (|| -> Result<texera_core::TransactionSpec, mynox::Error> {
            Ok(texera_core::tr!($($body)*))
        })().unwrap()
    };
}

fn mk_state(config: impl Into<HistoryConfig>, doc: Option<&str>) -> State {
    State::from(doc.unwrap_or("")).with_extensions(Extension::from([
        history(config.into()),
        AllowMultipleSelections(true).into_ext(),
    ]))
}

fn type_text(state: State, text: &str, at: Option<usize>) -> State {
    let at = at.unwrap_or_else(|| state.doc().len_chars());
    let s = spec!({
        changes: { from: at, insert: text }
    });
    state.update(s).unwrap().into_state()
}

fn timed_type(state: State, text: &str, at_time: u64) -> State {
    let at = state.doc().len_chars();
    let s = spec!({
        changes: { from: at, insert: text },
        annotations: [TransactionTime::of(at_time)]
    });
    state.update(s).unwrap().into_state()
}

fn receive(state: State, text: &str, from: usize, to: Option<usize>) -> State {
    let to = to.unwrap_or(from);
    let s = spec!({
        changes: { from: from, to: to, insert: text },
        annotations: [AddToHistory::of(false)]
    });
    state.update(s).unwrap().into_state()
}

fn command(state: State, cmd: impl Fn(&mut CommandTarget) -> bool, success: bool) -> State {
    let mut next_state = None;
    let ok = cmd(&mut CommandTarget {
        state: &state,
        dispatch: &mut |spec| {
            next_state = state.clone().update(spec).ok().map(|r| r.into_state());
        },
    });
    assert_eq!(ok, success);
    if let Some(s) = next_state {
        s
    } else {
        state
    }
}

#[test]
fn allows_to_undo_a_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "newtext", None);
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "");
}

#[test]
fn allows_to_undo_nearby_changes_in_one_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "new", None);
    state = type_text(state, "text", None);
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "");
}

#[test]
fn allows_to_redo_a_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "newtext", None);
    state = command(state, undo, true);
    state = command(state, redo, true);
    assert_eq!(state.doc().to_string(), "newtext");
}

#[test]
fn allows_to_redo_nearby_changes_in_one_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "new", None);
    state = type_text(state, "text", None);
    state = command(state, undo, true);
    state = command(state, redo, true);
    assert_eq!(state.doc().to_string(), "newtext");
}

#[test]
fn puts_the_cursor_after_the_change_on_redo() {
    let mut state = mk_state(None, Some("one\n\ntwo"));
    state = state
        .update(spec!({
            changes: { from: 3, insert: "!" },
            selection: Selection::cursor(4)
        }))
        .unwrap()
        .into_state();
    let doc_len = state.doc().len_chars();
    state = state
        .update(spec!({ selection: Selection::cursor(doc_len) }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    state = command(state, redo, true);
    assert_eq!(state.selection().main().head(), 4);
}

#[test]
fn restores_selection_on_undo_redo_undo() {
    let mut state = mk_state(None, Some("1\n2\n3"));
    state = state
        .update(spec!({ selection: Selection::cursor(5) }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: { from: 5, insert: "." },
            selection: Selection::cursor(6)
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ selection: Selection::cursor(0) }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.selection().main().head(), 5);
    state = state
        .update(spec!({ selection: Selection::cursor(0) }))
        .unwrap()
        .into_state();
    state = command(state, redo, true);
    assert_eq!(state.selection().main().head(), 6);
    state = state
        .update(spec!({ selection: Selection::cursor(0) }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.selection().main().head(), 5);
}

#[test]
fn tracks_multiple_levels_of_history() {
    let mut state = mk_state(None, Some("one"));
    state = type_text(state, "new", None);
    state = type_text(state, "text", None);
    state = type_text(state, "some", Some(0));
    assert_eq!(state.doc().to_string(), "someonenewtext");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "onenewtext");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "one");
    state = command(state, redo, true);
    assert_eq!(state.doc().to_string(), "onenewtext");
    state = command(state, redo, true);
    assert_eq!(state.doc().to_string(), "someonenewtext");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "onenewtext");
}

#[test]
fn starts_a_new_event_when_new_group_delay_elapses() {
    let mut state = mk_state(HistoryConfig::new().new_group_delay(1000), None);
    state = timed_type(state, "a", 1000);
    state = timed_type(state, "b", 1600);
    assert_eq!(undo_depth(&state), 1);
    state = timed_type(state, "c", 2700);
    assert_eq!(undo_depth(&state), 2);
    state = command(state, undo, true);
    state = timed_type(state, "d", 2800);
    assert_eq!(undo_depth(&state), 2);
}

#[test]
fn supports_a_custom_join_predicate() {
    let cfg = HistoryConfig::new().join_to_event(|tr, adj| {
        if !adj {
            return false;
        }
        let mut space = false;
        for change in tr.changes().iter_changes(false) {
            if change.text.len_chars() > 0 && change.text.slice_string(0, Some(1), None) == " " {
                space = true;
            }
        }
        !space
    });
    let mut state = mk_state(cfg, None);
    for ch in ["a", "b", " ", "c", "d"] {
        state = type_text(state, ch, None);
    }
    assert_eq!(state.slice_doc(None, None), "ab cd");
    state = command(state, undo, true);
    assert_eq!(state.slice_doc(None, None), "ab");
    state = command(state, undo, true);
    assert_eq!(state.slice_doc(None, None), "");
}

#[test]
fn allows_changes_that_arent_part_of_the_history() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hello", None);
    state = receive(state, "oops", 0, None);
    state = receive(state, "!", 9, None);
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "oops!");
}

#[test]
fn doesnt_get_confused_by_an_undo_not_adding_any_redo_item() {
    let mut state = mk_state(None, Some("ab"));
    state = type_text(state, "cd", Some(1));
    state = receive(state, "123", 0, Some(4));
    state = command(state, undo, false);
    command(state, redo, false);
}

#[test]
fn accurately_maps_changes_through_each_other() {
    let mut state = mk_state(None, Some("123"));
    state = state
        .update(spec!({
            changes: [
                { from: 0, to: 1, insert: "ab" },
                { from: 1, to: 2, insert: "cd" },
                { from: 2, to: 3, insert: "ef" }
            ]
        }))
        .unwrap()
        .into_state();
    state = receive(state, "!!!!!!!!", 2, Some(2));
    state = command(state, undo, true);
    state = command(state, redo, true);
    assert_eq!(state.doc().to_string(), "ab!!!!!!!!cdef");
}

#[test]
fn can_handle_complex_editing_sequences() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hello", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = type_text(state, "!", None);
    state = receive(state, "....", 0, None);
    state = type_text(state, "\n\n", Some(2));
    assert_eq!(state.doc().to_string(), "..\n\n..hello!");
    state = receive(state, "\n\n", 1, None);
    state = command(state, undo, true);
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), ".\n\n...hello");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), ".\n\n...");
}

#[test]
fn supports_overlapping_edits() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hello", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 0, to: 5 } }))
        .unwrap()
        .into_state();
    assert_eq!(state.doc().to_string(), "");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "hello");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "");
}

#[test]
fn supports_overlapping_edits_that_arent_collapsed() {
    let mut state = mk_state(None, None);
    state = receive(state, "h", 0, None);
    state = type_text(state, "ello", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 0, to: 5 } }))
        .unwrap()
        .into_state();
    assert_eq!(state.doc().to_string(), "");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "hello");
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "h");
}

#[test]
fn supports_overlapping_unsynced_deletes() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hi", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = type_text(state, "hello", None);
    state = state
        .update(spec!({
            changes: { from: 0, to: 7 },
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();
    assert_eq!(state.doc().to_string(), "");
    state = command(state, undo, false);
    assert_eq!(state.doc().to_string(), "");
}

#[test]
fn can_go_back_and_forth_through_history_multiple_times() {
    let mut state = mk_state(None, None);
    state = type_text(state, "one", None);
    state = type_text(state, " two", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = type_text(state, " three", None);
    state = type_text(state, "zero ", Some(0));
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = type_text(state, "\n\n", Some(0));
    state = type_text(state, "top", Some(0));
    for i in 0..6 {
        let re = i % 2 != 0;
        for _ in 0..4 {
            state = command(state, if re { redo } else { undo }, true);
        }
        assert_eq!(
            state.doc().to_string(),
            if re { "top\n\nzero one two three" } else { "" }
        );
    }
}

#[test]
fn supports_non_tracked_changes_next_to_tracked_changes() {
    let mut state = mk_state(None, None);
    state = type_text(state, "o", None);
    state = type_text(state, "\n\n", Some(0));
    state = receive(state, "zzz", 3, None);
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "zzz");
}

#[test]
fn can_go_back_and_forth_through_history_when_preserving_items() {
    let mut state = mk_state(None, None);
    state = type_text(state, "one", None);
    state = type_text(state, " two", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    let doc_len = state.doc().len_chars();
    state = receive(state, "xxx", doc_len, None);
    state = type_text(state, " three", None);
    state = type_text(state, "zero ", Some(0));
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = type_text(state, "\n\n", Some(0));
    state = type_text(state, "top", Some(0));
    state = receive(state, "yyy", 0, None);
    for _ in 0..3 {
        for _ in 0..4 {
            state = command(state, undo, true);
        }
        assert_eq!(state.doc().to_string(), "yyyxxx");
        for _ in 0..4 {
            state = command(state, redo, true);
        }
        assert_eq!(state.doc().to_string(), "yyytop\n\nzero one twoxxx three");
    }
}

#[test]
fn restores_selection_on_undo() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hi", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ selection: Selection::range(0, 2) }))
        .unwrap()
        .into_state();
    let selection = state.selection().clone();
    state = state
        .update(spec!({
            changes: [
                { from: 0, to: 2 },
                { from: 0, insert: "hello" }
            ]
        }))
        .unwrap()
        .into_state();
    let selection2 = state.selection().clone();
    state = command(state, undo, true);
    assert_eq!(state.selection(), &selection);
    state = command(state, redo, true);
    assert_eq!(state.selection(), &selection2);
}

#[test]
fn restores_the_selection_before_the_first_change_in_an_item() {
    let mut state = mk_state(None, None);
    state = state
        .update(spec!({
            changes: { from: 0, insert: "a" },
            selection: Selection::cursor(1)
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: { from: 1, insert: "b" },
            selection: Selection::cursor(2)
        }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "");
    assert_eq!(state.selection().main().anchor(), 0);
}

#[test]
fn doesnt_merge_document_changes_if_theres_a_selection_change_in_between() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hi", None);
    state = state
        .update(spec!({ selection: Selection::range(0, 2) }))
        .unwrap()
        .into_state();
    let repl = state.replace_selection("hello");
    state = state.update(repl).unwrap().into_state();
    assert_eq!(undo_depth(&state), 2);
}

#[test]
fn rebases_selection_on_undo() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hi", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ selection: Selection::range(0, 2) }))
        .unwrap()
        .into_state();
    state = type_text(state, "hello", Some(0));
    state = receive(state, "---", 0, None);
    state = command(state, undo, true);
    assert_eq!(state.selection().ranges_ref()[0].head(), 5);
}

#[test]
fn supports_querying_for_the_undo_and_redo_depth() {
    let mut state = mk_state(None, None);
    state = type_text(state, "a", None);
    assert_eq!(undo_depth(&state), 1);
    assert_eq!(redo_depth(&state), 0);
    state = receive(state, "b", 0, None);
    assert_eq!(undo_depth(&state), 1);
    assert_eq!(redo_depth(&state), 0);
    state = command(state, undo, true);
    assert_eq!(undo_depth(&state), 0);
    assert_eq!(redo_depth(&state), 1);
    state = command(state, redo, true);
    assert_eq!(undo_depth(&state), 1);
    assert_eq!(redo_depth(&state), 0);
}

#[test]
fn all_functions_gracefully_handle_editor_states_without_history() {
    let state = State::default();
    assert_eq!(undo_depth(&state), 0);
    assert_eq!(redo_depth(&state), 0);
    assert!(!can_undo_selection(&state));
    assert!(!can_redo_selection(&state));
    let state = command(state, undo, false);
    command(state, redo, false);
}

#[test]
fn truncates_history() {
    let mut state = mk_state(HistoryConfig::new().min_depth(10), None);
    for _ in 0..40 {
        state = type_text(state, "a", None);
        state = state
            .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
            .unwrap()
            .into_state();
    }
    assert!(undo_depth(&state) < 40);
}

#[test]
fn doesnt_undo_selection_only_transactions() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({ selection: Selection::cursor(2) }))
        .unwrap()
        .into_state();
    state = command(state, undo, false);
    assert_eq!(state.selection().main().head(), 2);
}

#[test]
fn isolates_transactions_when_asked_to() {
    let mut state = mk_state(None, None);
    state = state
        .update(spec!({
            changes: { from: 0, insert: "a" },
            annotations: [isolate_history(IsolateHistory::After)]
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 1, insert: "a" } }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: { from: 2, insert: "c" },
            annotations: [isolate_history(IsolateHistory::After)]
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 3, insert: "d" } }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: { from: 4, insert: "e" },
            annotations: [isolate_history(IsolateHistory::Full)]
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 5, insert: "f" } }))
        .unwrap()
        .into_state();
    assert_eq!(undo_depth(&state), 5);
}

#[test]
fn can_group_events_around_a_non_history_transaction() {
    let mut state = mk_state(None, None);
    state = state
        .update(spec!({ changes: { from: 0, insert: "a" } }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: { from: 1, insert: "b" },
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 1, insert: "c" } }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "b");
}

#[test]
fn properly_maps_selections_through_non_history_changes() {
    let mut state = mk_state(None, Some("abc"));
    state = state
        .update(spec!({
            selection: Selection::from([
                Selection::cursor(0),
                Selection::cursor(1),
                Selection::cursor(2),
            ])
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ changes: { from: 0, to: 3, insert: "d" } }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            changes: [
                { from: 0, insert: "x" },
                { from: 1, insert: "y" }
            ],
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "xabcy");
    let froms: Vec<usize> = state
        .selection()
        .ranges_ref()
        .iter()
        .map(|r| r.from())
        .collect();
    assert_eq!(froms, vec![0, 2, 3]);
}

#[test]
fn restores_selection_on_redo() {
    let mut state = mk_state(None, Some("a\nb\nc\n"));
    state = state
        .update(spec!({
            selection: Selection::from([
                Selection::cursor(1),
                Selection::cursor(3),
                Selection::cursor(5),
            ])
        }))
        .unwrap()
        .into_state();
    let repl = state.replace_selection("-");
    state = state.update(repl).unwrap().into_state();
    state = state
        .update(spec!({ selection: Selection::cursor(0) }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    state = state
        .update(spec!({ selection: Selection::cursor(0) }))
        .unwrap()
        .into_state();
    state = command(state, redo, true);
    let heads: Vec<usize> = state
        .selection()
        .ranges_ref()
        .iter()
        .map(|r| r.head())
        .collect();
    assert_eq!(heads, vec![2, 5, 8]);
}

// ---------------------------------------------------------------------------
// undoSelection
// ---------------------------------------------------------------------------

#[test]
fn undo_selection_allows_to_undo_a_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "newtext", None);
    state = command(state, undo_selection, true);
    assert_eq!(state.doc().to_string(), "");
}

#[test]
fn undo_selection_allows_to_undo_selection_only_transactions() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    assert!(!can_undo_selection(&state));
    state = state
        .update(spec!({ selection: Selection::cursor(2) }))
        .unwrap()
        .into_state();
    assert!(can_undo_selection(&state));
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_merges_selection_only_transactions_from_keyboard() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({
            selection: Selection::cursor(2),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::cursor(3),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::cursor(1),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_selection_only_transactions_from_other_sources() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({ selection: Selection::cursor(2) }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ selection: Selection::cursor(3) }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({ selection: Selection::cursor(1) }))
        .unwrap()
        .into_state();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 3);
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 2);
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_if_they_change_the_number_of_selections() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({
            selection: Selection::cursor(2),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::from([Selection::cursor(1), Selection::cursor(3)]),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::cursor(1),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().count(), 2);
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_doesnt_merge_if_a_selection_changes_empty_state() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({
            selection: Selection::cursor(2),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::range(2, 3),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            selection: Selection::cursor(1),
            user_event: "select"
        }))
        .unwrap()
        .into_state();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().anchor(), 2);
    assert_eq!(state.selection().main().head(), 3);
    state = command(state, undo_selection, true);
    assert_eq!(state.selection().main().head(), 0);
}

#[test]
fn undo_selection_allows_to_redo_a_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "newtext", None);
    state = command(state, undo_selection, true);
    state = command(state, redo_selection, true);
    assert_eq!(state.doc().to_string(), "newtext");
}

#[test]
fn undo_selection_allows_to_redo_selection_only_transactions() {
    let mut state = mk_state(None, Some("abc"));
    assert_eq!(state.selection().main().head(), 0);
    state = state
        .update(spec!({ selection: Selection::cursor(2) }))
        .unwrap()
        .into_state();
    state = command(state, undo_selection, true);
    state = command(state, redo_selection, true);
    assert_eq!(state.selection().main().head(), 2);
}

#[test]
fn undo_selection_only_changes_selection() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hi", None);
    state = state
        .update(spec!({ annotations: [isolate_history(IsolateHistory::Before)] }))
        .unwrap()
        .into_state();
    let selection = state.selection().clone();
    state = state
        .update(spec!({ selection: Selection::range(0, 2) }))
        .unwrap()
        .into_state();
    let selection2 = state.selection().clone();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection(), &selection);
    assert_eq!(state.doc().to_string(), "hi");
    state = command(state, redo_selection, true);
    assert_eq!(state.selection(), &selection2);
    let repl = state.replace_selection("hello");
    state = state.update(repl).unwrap().into_state();
    state = command(state, undo_selection, true);
    assert_eq!(state.selection(), &selection2);
    state = command(state, redo, true);
    assert_eq!(state.selection().main().head(), 5);
}

#[test]
fn undo_selection_can_undo_a_selection_through_remote_changes() {
    let mut state = mk_state(None, None);
    state = type_text(state, "hello", None);
    state = state
        .update(spec!({ selection: Selection::range(0, 2) }))
        .unwrap()
        .into_state();
    state = receive(state, "oops", 0, None);
    state = receive(state, "!", 9, None);
    assert_eq!(state.selection(), &Selection::from([Selection::range(4, 6)]));
    state = command(state, undo_selection, true);
    assert_eq!(state.doc().to_string(), "oopshello!");
    assert_eq!(state.selection(), &Selection::from([Selection::range(9, 9)]));
}

#[test]
fn preserves_text_inserted_inside_a_change() {
    let mut state = mk_state(None, None);
    state = type_text(state, "1234", None);
    state = state
        .update(spec!({
            changes: { from: 2, insert: "x" },
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();
    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "x");
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct SetNumber(usize);
impl Effect for SetNumber {}

struct NumberField;
impl StateField for NumberField {
    type Value = usize;
    fn create(_: &State) -> usize {
        0
    }
    fn update(val: &usize, tr: &Transaction) -> usize {
        let mut v = *val;
        for effect in tr.effects() {
            if let Some(set) = effect.get::<SetNumber>() {
                v = set.0;
            }
        }
        v
    }
}

#[test]
fn includes_inverted_effects_in_the_history() {
    let invert = InvertedEffects::new(|tr| {
        for e in tr.effects() {
            if e.is::<SetNumber>() {
                let prev = *tr.start_state().field::<NumberField>().unwrap_or(&0);
                return vec![StateEffect::new(SetNumber(prev))];
            }
        }
        vec![]
    });

    let mut state = State::default().with_extensions(Extension::from([
        history(()),
        NumberField::init(),
        invert,
    ]));

    state = state
        .update(spec!({
            effects: [StateEffect::new(SetNumber(10))],
            annotations: [isolate_history(IsolateHistory::Before)]
        }))
        .unwrap()
        .into_state();
    state = state
        .update(spec!({
            effects: [StateEffect::new(SetNumber(20))],
            annotations: [isolate_history(IsolateHistory::Before)]
        }))
        .unwrap()
        .into_state();
    assert_eq!(*state.field::<NumberField>().unwrap(), 20);

    state = command(state, undo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 10);
    state = command(state, undo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 0);
    state = command(state, redo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 10);
    state = command(state, redo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 20);
    state = command(state, undo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 10);
    state = command(state, redo, true);
    assert_eq!(*state.field::<NumberField>().unwrap(), 20);
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Comment {
    from: usize,
    to: usize,
    text: String,
}

fn map_comment(comment: &Comment, mapping: &ChangeDesc) -> Option<Comment> {
    let from = mapping
        .map_pos(comment.from, texera_core::Assoc::After, MapMode::TrackDel)
        .ok()
        .flatten()?;
    let to = mapping
        .map_pos(comment.to, texera_core::Assoc::Before, MapMode::TrackDel)
        .ok()
        .flatten()?;
    if from >= to {
        None
    } else {
        Some(Comment {
            from,
            to,
            text: comment.text.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AddComment(Comment);

impl Effect for AddComment {
    fn map(&self, changes: &ChangeDesc) -> Option<Self> {
        map_comment(&self.0, changes).map(AddComment)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RmComment(Comment);

impl Effect for RmComment {
    fn map(&self, changes: &ChangeDesc) -> Option<Self> {
        map_comment(&self.0, changes).map(RmComment)
    }
}

struct CommentsField;

impl StateField for CommentsField {
    type Value = Vec<Comment>;

    fn create(_: &State) -> Vec<Comment> {
        Vec::new()
    }

    fn update(value: &Self::Value, tr: &Transaction) -> Self::Value {
        let binding = tr.changes();
        let desc = binding.description();
        let mut list: Vec<Comment> = value.iter().filter_map(|c| map_comment(c, desc)).collect();
        for effect in tr.effects() {
            if let Some(add) = effect.get::<AddComment>() {
                list.push(add.0.clone());
            } else if let Some(rm) = effect.get::<RmComment>() {
                list.retain(|c| c != &rm.0);
            }
        }
        list.sort_by_key(|c| c.from);
        list
    }
}

fn make_invert_comments() -> Extension {
    InvertedEffects::new(|tr| {
        let mut effects = Vec::new();
        let inv_desc = tr.changes().description().invert();
        for effect in tr.effects() {
            if let Some(add) = effect.get::<AddComment>() {
                if let Some(src) = map_comment(&add.0, &inv_desc) {
                    effects.push(StateEffect::new(RmComment(src)));
                }
            } else if let Some(rm) = effect.get::<RmComment>() {
                if let Some(src) = map_comment(&rm.0, &inv_desc) {
                    effects.push(StateEffect::new(AddComment(src)));
                }
            }
        }
        if let Some(start_comments) = tr.start_state().field::<CommentsField>() {
            let binding = tr.changes();
            let desc = binding.description();
            for comment in start_comments {
                if map_comment(comment, desc).is_none() {
                    effects.push(StateEffect::new(AddComment(comment.clone())));
                }
            }
        }
        effects
    })
}

fn comment_str(state: &State) -> String {
    state
        .field::<CommentsField>()
        .unwrap()
        .iter()
        .map(|c| format!("{}@{}", c.text, c.from))
        .collect::<Vec<_>>()
        .join(",")
}

#[test]
fn can_map_effects() {
    let mut state = State::from("one two foo").with_extensions(Extension::from([
        history(()),
        CommentsField::init(),
        make_invert_comments(),
    ]));

    state = state
        .update(spec!({
            effects: [StateEffect::new(AddComment(Comment {
                from: 0,
                to: 3,
                text: "c1".into()
            }))],
            annotations: [isolate_history(IsolateHistory::Full)]
        }))
        .unwrap()
        .into_state();
    assert_eq!(comment_str(&state), "c1@0");

    state = state
        .update(spec!({
            changes: { from: 3, to: 4, insert: "---" },
            annotations: [isolate_history(IsolateHistory::Full)],
            effects: [StateEffect::new(AddComment(Comment {
                from: 6,
                to: 9,
                text: "c2".into()
            }))]
        }))
        .unwrap()
        .into_state();
    assert_eq!(comment_str(&state), "c1@0,c2@6");

    state = state
        .update(spec!({
            changes: { from: 0, insert: "---" },
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();
    assert_eq!(comment_str(&state), "c1@3,c2@9");

    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "---one two foo");
    assert_eq!(comment_str(&state), "c1@3");

    state = command(state, undo, true);
    assert_eq!(comment_str(&state), "");

    state = command(state, redo, true);
    assert_eq!(comment_str(&state), "c1@3");

    state = command(state, redo, true);
    assert_eq!(comment_str(&state), "c1@3,c2@9");
    assert_eq!(state.doc().to_string(), "---one---two foo");

    state = command(state, undo, true);
    state = state
        .update(spec!({
            changes: { from: 10, to: 11, insert: "---" },
            annotations: [AddToHistory::of(false)]
        }))
        .unwrap()
        .into_state();

    state = state
        .update(spec!({
            effects: [StateEffect::new(AddComment(Comment {
                from: 13,
                to: 16,
                text: "c3".into()
            }))],
            annotations: [isolate_history(IsolateHistory::Full)]
        }))
        .unwrap()
        .into_state();
    assert_eq!(comment_str(&state), "c1@3,c3@13");

    state = command(state, undo, true);
    assert_eq!(state.doc().to_string(), "---one two---foo");
    assert_eq!(comment_str(&state), "c1@3");

    state = command(state, redo, true);
    assert_eq!(comment_str(&state), "c1@3,c3@13");
}

#[test]
fn can_restore_comments_lost_through_deletion() {
    let mut state = State::from("123456").with_extensions(Extension::from([
        history(()),
        CommentsField::init(),
        make_invert_comments(),
    ]));

    state = state
        .update(spec!({
            effects: [StateEffect::new(AddComment(Comment {
                from: 3,
                to: 5,
                text: "c1".into()
            }))],
            annotations: [isolate_history(IsolateHistory::Full)]
        }))
        .unwrap()
        .into_state();

    state = state
        .update(spec!({ changes: { from: 2, to: 6 } }))
        .unwrap()
        .into_state();
    assert_eq!(comment_str(&state), "");

    state = command(state, undo, true);
    assert_eq!(comment_str(&state), "c1@3");
}
