use std::sync::Arc;

use texera_core::{
    Annotation, AnnotationValue, ChangeDesc, ChangeIterator, ChangeSpec, Composable, Extension,
    FacetDef, Selection, State, StateField, Transaction, TransactionSpec, UserEvent,
};

pub mod event;
#[cfg(test)]
mod tests;

pub use event::{
    add_mapping_to_branch, add_selection, conc, eq_selection_shape, is_adjacent,
    is_joinable_user_event, is_select_user_event, map_effects, map_event, pop_selection,
    update_branch, Branch, HistEvent, InvertedEffects, MAX_SELECTIONS_PER_EVENT,
};

/// Branch names for undo and redo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchName {
    Done,
    Undone,
}

/// Transaction annotation that will prevent that transaction from
/// being combined with other transactions in the undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsolateHistory {
    Before,
    After,
    Full,
}

impl AnnotationValue for IsolateHistory {}

impl IsolateHistory {
    pub fn of(self) -> Annotation {
        Annotation::new(self)
    }
}

pub fn isolate_history(iso: IsolateHistory) -> Annotation {
    Annotation::new(iso)
}

/// Transaction annotation that controls whether a transaction is stored in history.
/// Setting to `false` prevents adding the transaction to history and instead maps
/// existing history events through the transaction's changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddToHistory(pub bool);

impl AnnotationValue for AddToHistory {}

impl AddToHistory {
    pub fn of(val: bool) -> Annotation {
        Annotation::new(Self(val))
    }
}

pub fn add_to_history(val: bool) -> Annotation {
    Annotation::new(AddToHistory(val))
}

/// Explicit timestamp annotation attached to transactions for grouping logic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionTime(pub u64);

impl AnnotationValue for TransactionTime {}

impl TransactionTime {
    pub fn of(val: u64) -> Annotation {
        Annotation::new(Self(val))
    }
}

pub fn transaction_time(time: u64) -> Annotation {
    Annotation::new(TransactionTime(time))
}

/// Predicate function deciding whether a transaction should be joined to the preceding event.
pub type JoinPredicate = Arc<dyn Fn(&Transaction, bool) -> bool + Send + Sync>;

/// Configuration options for the history extension.
#[derive(Clone)]
pub struct HistoryConfig {
    pub min_depth: Option<usize>,
    pub new_group_delay: Option<u64>,
    pub join_to_event: Option<JoinPredicate>,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            min_depth: None,
            new_group_delay: None,
            join_to_event: None,
        }
    }
}

impl From<()> for HistoryConfig {
    fn from(_: ()) -> Self {
        Self::default()
    }
}

impl From<Option<HistoryConfig>> for HistoryConfig {
    fn from(opt: Option<HistoryConfig>) -> Self {
        opt.unwrap_or_default()
    }
}

impl HistoryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn min_depth(mut self, depth: usize) -> Self {
        self.min_depth = Some(depth);
        self
    }

    pub fn new_group_delay(mut self, delay: u64) -> Self {
        self.new_group_delay = Some(delay);
        self
    }

    pub fn join_to_event<F>(mut self, pred: F) -> Self
    where
        F: Fn(&Transaction, bool) -> bool + Send + Sync + 'static,
    {
        self.join_to_event = Some(Arc::new(pred));
        self
    }

    pub fn of(self) -> Extension {
        Extension::facet::<HistoryConfigFacet>(self)
    }
}

/// Resolved configuration values for history.
#[derive(Clone)]
pub struct ResolvedHistoryConfig {
    pub min_depth: usize,
    pub new_group_delay: u64,
    pub join_to_event: JoinPredicate,
}

impl Default for ResolvedHistoryConfig {
    fn default() -> Self {
        Self {
            min_depth: 100,
            new_group_delay: 500,
            join_to_event: Arc::new(|_tr, is_adjacent| is_adjacent),
        }
    }
}

/// Facet for combining [`HistoryConfig`] instances.
pub struct HistoryConfigFacet;

impl FacetDef for HistoryConfigFacet {
    type Input = HistoryConfig;
    type Output = ResolvedHistoryConfig;

    fn combine(inputs: &[Self::Input]) -> Self::Output {
        let mut min_depth: Option<usize> = None;
        let mut new_group_delay: Option<u64> = None;
        let mut join_predicates: Vec<JoinPredicate> = Vec::new();

        for cfg in inputs {
            if let Some(d) = cfg.min_depth {
                min_depth = Some(match min_depth {
                    Some(cur) => cur.max(d),
                    None => d,
                });
            }
            if let Some(delay) = cfg.new_group_delay {
                new_group_delay = Some(match new_group_delay {
                    Some(cur) => cur.min(delay),
                    None => delay,
                });
            }
            if let Some(ref pred) = cfg.join_to_event {
                join_predicates.push(Arc::clone(pred));
            }
        }

        let join_to_event: JoinPredicate = if join_predicates.is_empty() {
            Arc::new(|_tr, is_adj| is_adj)
        } else {
            Arc::new(move |tr, is_adj| join_predicates.iter().any(|pred| pred(tr, is_adj)))
        };

        ResolvedHistoryConfig {
            min_depth: min_depth.unwrap_or(100),
            new_group_delay: new_group_delay.unwrap_or(500),
            join_to_event,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FromHistory {
    pub side: BranchName,
    pub rest: Branch,
    pub selection: Selection,
}

impl AnnotationValue for FromHistory {}

/// Stores the undo and redo history state.
#[derive(Clone, Debug)]
pub struct HistoryState {
    pub done: Branch,
    pub undone: Branch,
    pub prev_time: u64,
    pub prev_user_event: Option<String>,
}

impl HistoryState {
    pub fn new(
        done: Branch,
        undone: Branch,
        prev_time: u64,
        prev_user_event: Option<String>,
    ) -> Self {
        Self {
            done,
            undone,
            prev_time,
            prev_user_event,
        }
    }

    pub fn empty() -> Self {
        Self {
            done: Vec::new(),
            undone: Vec::new(),
            prev_time: 0,
            prev_user_event: None,
        }
    }

    pub fn isolate(&self) -> HistoryState {
        if self.prev_time != 0 {
            HistoryState {
                done: self.done.clone(),
                undone: self.undone.clone(),
                prev_time: 0,
                prev_user_event: None,
            }
        } else {
            self.clone()
        }
    }

    pub fn add_changes(
        &self,
        event: HistEvent,
        time: u64,
        user_event: Option<&str>,
        config: &ResolvedHistoryConfig,
        tr: &Transaction,
    ) -> HistoryState {
        let mut done = self.done.clone();
        let last_event = done.last();

        let is_joinable = if let (Some(last), Some(ev_changes)) = (last_event, &event.changes) {
            let last_has_changes = last.changes.as_ref().is_some_and(|c| !c.is_empty());
            let ev_has_changes = !ev_changes.is_empty();
            let user_event_ok = user_event.map_or(true, is_joinable_user_event);

            let is_compose = user_event.map_or(false, |u| {
                let raw = u.strip_prefix("User.Event.").unwrap_or(u);
                raw == "input.type.compose"
            });

            if last_has_changes && ev_has_changes && user_event_ok {
                let normal_join = last.selections_after.is_empty()
                    && (time.saturating_sub(self.prev_time) < config.new_group_delay)
                    && (config.join_to_event)(
                        tr,
                        is_adjacent(
                            last.changes.as_ref().unwrap().description(),
                            ev_changes.description(),
                        ),
                    );
                normal_join || is_compose
            } else {
                false
            }
        } else {
            false
        };

        if is_joinable {
            let last = done.last().unwrap();
            let last_changes = last.changes.as_ref().unwrap();
            let ev_changes = event.changes.as_ref().unwrap();

            let composed_changes = ev_changes.compose(last_changes).ok();
            let mapped_effects = map_effects(&event.effects, last_changes.description());
            let composed_effects = conc(&mapped_effects, &last.effects);

            let merged_event = HistEvent {
                changes: composed_changes,
                effects: composed_effects,
                mapped: last.mapped.clone(),
                start_selection: last.start_selection.clone(),
                selections_after: Vec::new(),
            };

            done = update_branch(&done, done.len() - 1, config.min_depth, merged_event);
        } else {
            done = update_branch(&done, done.len(), config.min_depth, event);
        }

        HistoryState {
            done,
            undone: Vec::new(),
            prev_time: time,
            prev_user_event: user_event.map(String::from),
        }
    }

    pub fn add_selection(
        &self,
        selection: Selection,
        time: u64,
        user_event: Option<&str>,
        new_group_delay: u64,
    ) -> HistoryState {
        let last_sels = self
            .done
            .last()
            .map(|e| e.selections_after.as_slice())
            .unwrap_or(&[]);

        let is_mergeable = if !last_sels.is_empty() {
            let time_ok = time.saturating_sub(self.prev_time) < new_group_delay;
            let event_same = user_event == self.prev_user_event.as_deref();
            let event_select = user_event.map_or(false, is_select_user_event);
            let shape_eq = eq_selection_shape(last_sels.last().unwrap(), &selection);
            time_ok && event_same && event_select && shape_eq
        } else {
            false
        };

        if is_mergeable {
            return self.clone();
        }

        HistoryState {
            done: add_selection(&self.done, selection),
            undone: self.undone.clone(),
            prev_time: time,
            prev_user_event: user_event.map(String::from),
        }
    }

    pub fn add_mapping(&self, mapping: &ChangeDesc) -> HistoryState {
        HistoryState {
            done: add_mapping_to_branch(&self.done, mapping.clone()),
            undone: add_mapping_to_branch(&self.undone, mapping.clone()),
            prev_time: self.prev_time,
            prev_user_event: self.prev_user_event.clone(),
        }
    }

    pub fn pop(
        &self,
        side: BranchName,
        state: &State,
        only_selection: bool,
    ) -> Option<TransactionSpec> {
        let branch = match side {
            BranchName::Done => &self.done,
            BranchName::Undone => &self.undone,
        };
        if branch.is_empty() {
            return None;
        }
        let event = &branch[branch.len() - 1];

        let selection = if let Some(first_sel) = event.selections_after.first() {
            first_sel.clone()
        } else if let (Some(start_sel), Some(changes)) = (&event.start_selection, &event.changes) {
            let inv_desc = changes.description().invert();
            start_sel
                .map(&inv_desc, texera_core::Assoc::After)
                .unwrap_or_else(|_| start_sel.clone())
        } else {
            state.selection().clone()
        };

        if only_selection && !event.selections_after.is_empty() {
            let sel = event.selections_after.last().unwrap().clone();
            let rest = pop_selection(branch);
            let user_event = match side {
                BranchName::Done => "select.undo",
                BranchName::Undone => "select.redo",
            };
            Some(
                TransactionSpec::default()
                    .with_selection(sel)
                    .with_annotation(Annotation::new(FromHistory {
                        side,
                        rest,
                        selection,
                    }))
                    .with_user_event(user_event)
                    .is_scroll_into_view(true),
            )
        } else if event.changes.is_none() {
            None
        } else {
            let mut rest = if branch.len() == 1 {
                Vec::new()
            } else {
                branch[..branch.len() - 1].to_vec()
            };
            if let Some(ref mapped) = event.mapped {
                rest = add_mapping_to_branch(&rest, mapped.clone());
            }

            let mut spec = TransactionSpec::default();
            if let Some(ref cs) = event.changes {
                let change_specs: Vec<ChangeSpec> = cs
                    .iter_changes(false)
                    .filter_map(|c| {
                        ChangeSpec::new(c.map.old.from, Some(c.map.old.to), Some(c.text)).ok()
                    })
                    .collect();
                spec = spec.with_changes(change_specs);
            }
            if let Some(ref sel) = event.start_selection {
                spec = spec.with_selection(sel.clone());
            }
            let user_event = match side {
                BranchName::Done => "undo",
                BranchName::Undone => "redo",
            };
            Some(
                spec.with_effects(event.effects.clone())
                    .with_annotation(Annotation::new(FromHistory {
                        side,
                        rest,
                        selection,
                    }))
                    .is_filter(false)
                    .with_user_event(user_event)
                    .is_scroll_into_view(true),
            )
        }
    }
}

impl Default for HistoryState {
    fn default() -> Self {
        Self::empty()
    }
}

/// The state field type used to store history data.
pub struct HistoryField;

impl StateField for HistoryField {
    type Value = HistoryState;

    fn create(_state: &State) -> Self::Value {
        HistoryState::empty()
    }

    fn update(state: &Self::Value, tr: &Transaction) -> Self::Value {
        let config = tr.start_state().facet::<HistoryConfigFacet>();

        if let Some(from_hist) = tr.annotation::<FromHistory>() {
            let item = HistEvent::from_transaction(tr, Some(from_hist.selection.clone()));
            let from = from_hist.side;
            let other = match from {
                BranchName::Done => &state.undone,
                BranchName::Undone => &state.done,
            };
            let other = match item {
                Some(item) => update_branch(other, other.len(), config.min_depth, item),
                None => add_selection(other, tr.start_state().selection().clone()),
            };
            return match from {
                BranchName::Done => {
                    HistoryState::new(from_hist.rest.clone(), other, 0, None)
                }
                BranchName::Undone => {
                    HistoryState::new(other, from_hist.rest.clone(), 0, None)
                }
            };
        }

        let mut current_state = state.clone();

        let isolate = tr.annotation::<IsolateHistory>();
        if isolate == Some(&IsolateHistory::Full) || isolate == Some(&IsolateHistory::Before) {
            current_state = current_state.isolate();
        }

        if tr.annotation::<AddToHistory>() == Some(&AddToHistory(false)) {
            return if tr.doc_changed() {
                current_state.add_mapping(&tr.changes().description())
            } else {
                current_state
            };
        }

        let event = HistEvent::from_transaction(tr, None);
        let time = tr
            .annotation::<TransactionTime>()
            .map(|t| t.0)
            .unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0)
            });
        let user_event = tr.annotation::<UserEvent>().map(|u| u.0.as_str());

        if let Some(event) = event {
            current_state = current_state.add_changes(event, time, user_event, &config, tr);
        } else if tr.selection().is_some() {
            current_state = current_state.add_selection(
                tr.start_state().selection().clone(),
                time,
                user_event,
                config.new_group_delay,
            );
        }

        if isolate == Some(&IsolateHistory::Full) || isolate == Some(&IsolateHistory::After) {
            current_state = current_state.isolate();
        }

        current_state
    }
}

/// Helper target for state commands.
pub struct CommandTarget<'a> {
    pub state: &'a State,
    pub dispatch: &'a mut dyn FnMut(TransactionSpec),
}

impl<'a> CommandTarget<'a> {
    pub fn new(state: &'a State, dispatch: &'a mut dyn FnMut(TransactionSpec)) -> Self {
        Self { state, dispatch }
    }
}

pub fn cmd(side: BranchName, selection: bool, target: &mut CommandTarget) -> bool {
    if !selection && target.state.read_only() {
        return false;
    }
    let history_state = match target.state.field::<HistoryField>() {
        Some(s) => s,
        None => return false,
    };
    let tr = match history_state.pop(side, target.state, selection) {
        Some(tr) => tr,
        None => return false,
    };
    (target.dispatch)(tr);
    true
}

/// Undo a single group of history events. Returns false if no group was available.
pub fn undo(target: &mut CommandTarget) -> bool {
    cmd(BranchName::Done, false, target)
}

/// Redo a group of history events. Returns false if no group was available.
pub fn redo(target: &mut CommandTarget) -> bool {
    cmd(BranchName::Undone, false, target)
}

/// Undo a change or selection change.
pub fn undo_selection(target: &mut CommandTarget) -> bool {
    cmd(BranchName::Done, true, target)
}

/// Redo a change or selection change.
pub fn redo_selection(target: &mut CommandTarget) -> bool {
    cmd(BranchName::Undone, true, target)
}

/// Convenience helper to undo directly on a mutable [`State`].
pub fn undo_state(state: &mut State) -> bool {
    let mut next = None;
    let ok = undo(&mut CommandTarget {
        state,
        dispatch: &mut |spec| {
            if let Ok(res) = state.clone().update(spec) {
                next = Some(res.into_state());
            }
        },
    });
    if let Some(s) = next {
        *state = s;
        ok
    } else {
        false
    }
}

/// Convenience helper to redo directly on a mutable [`State`].
pub fn redo_state(state: &mut State) -> bool {
    let mut next = None;
    let ok = redo(&mut CommandTarget {
        state,
        dispatch: &mut |spec| {
            if let Ok(res) = state.clone().update(spec) {
                next = Some(res.into_state());
            }
        },
    });
    if let Some(s) = next {
        *state = s;
        ok
    } else {
        false
    }
}

/// The amount of undoable change events available in a given state.
pub fn undo_depth(state: &State) -> usize {
    depth(BranchName::Done, state)
}

/// The amount of redoable change events available in a given state.
pub fn redo_depth(state: &State) -> usize {
    depth(BranchName::Undone, state)
}

fn depth(side: BranchName, state: &State) -> usize {
    let hist_state = match state.field::<HistoryField>() {
        Some(s) => s,
        None => return 0,
    };
    let branch = match side {
        BranchName::Done => &hist_state.done,
        BranchName::Undone => &hist_state.undone,
    };
    if branch.is_empty() {
        0
    } else {
        branch.len() - if branch[0].changes.is_none() { 1 } else { 0 }
    }
}

/// Test whether there is an undoable selection available.
pub fn can_undo_selection(state: &State) -> bool {
    has_selection(BranchName::Done, state)
}

/// Test whether there is a redoable selection available.
pub fn can_redo_selection(state: &State) -> bool {
    has_selection(BranchName::Undone, state)
}

fn has_selection(side: BranchName, state: &State) -> bool {
    let hist_state = match state.field::<HistoryField>() {
        Some(s) => s,
        None => return false,
    };
    let branch = match side {
        BranchName::Done => &hist_state.done,
        BranchName::Undone => &hist_state.undone,
    };
    !branch.is_empty() && !branch[0].selections_after.is_empty()
}

/// Create a history extension with the given configuration.
pub fn history(config: impl Into<HistoryConfig>) -> Extension {
    let cfg = config.into();
    Extension::from([HistoryField::init(), cfg.of()])
}

pub fn history_default() -> Extension {
    history(HistoryConfig::default())
}
