use std::sync::Arc;

use mynox::Error;
use texera_core::{
    Assoc, ChangeDesc, ChangeIterator, ChangeSet, Composable, Extension, Facet, Mapable, Multi,
    Selection, StateEffect, Transaction,
};

/// Facet that registers functions providing effects to be stored when inverting a transaction.
#[derive(Clone)]
pub struct InvertedEffects(pub Arc<dyn Fn(&Transaction) -> Vec<StateEffect> + Send + Sync>);

impl Facet for InvertedEffects {
    type Mode = Multi;
}

impl InvertedEffects {
    pub fn new<F>(f: F) -> Extension
    where
        F: Fn(&Transaction) -> Vec<StateEffect> + Send + Sync + 'static,
    {
        Extension::from(Self(Arc::new(f)))
    }

    pub fn of<F>(f: F) -> Extension
    where
        F: Fn(&Transaction) -> Vec<StateEffect> + Send + Sync + 'static,
    {
        Self::new(f)
    }
}

/// History event storing groups of changes, effects, and selections to be undone/redone together.
#[derive(Clone, Debug)]
pub struct HistEvent {
    pub changes: Option<ChangeSet>,
    pub effects: Vec<StateEffect>,
    pub mapped: Option<ChangeDesc>,
    pub start_selection: Option<Arc<Selection>>,
    pub selections_after: Vec<Arc<Selection>>,
}

pub type Branch = Vec<HistEvent>;

pub const MAX_SELECTIONS_PER_EVENT: usize = 200;

impl HistEvent {
    pub fn new(
        changes: Option<ChangeSet>,
        effects: Vec<StateEffect>,
        mapped: Option<ChangeDesc>,
        start_selection: Option<Arc<Selection>>,
        selections_after: Vec<Arc<Selection>>,
    ) -> Self {
        Self {
            changes,
            effects,
            mapped,
            start_selection,
            selections_after,
        }
    }

    pub fn set_sel_after(&self, after: Vec<Arc<Selection>>) -> Self {
        Self {
            changes: self.changes.clone(),
            effects: self.effects.clone(),
            mapped: self.mapped.clone(),
            start_selection: self.start_selection.clone(),
            selections_after: after,
        }
    }

    pub fn from_transaction(tr: &Transaction, selection: Option<Selection>) -> Option<Self> {
        let mut effects = Vec::new();
        for inv in tr.start_state().facet::<InvertedEffects>() {
            let res = (inv.0)(tr);
            effects.extend(res);
        }
        if effects.is_empty() && !tr.doc_changed() {
            return None;
        }
        let inverted = tr.inverted_changes();
        let start_sel = selection
            .map(Arc::new)
            .unwrap_or_else(|| Arc::new(tr.start_state().selection().clone()));
        Some(Self {
            changes: Some(inverted),
            effects,
            mapped: None,
            start_selection: Some(start_sel),
            selections_after: Vec::new(),
        })
    }

    pub fn selection(selections: Vec<Arc<Selection>>) -> Self {
        Self {
            changes: None,
            effects: Vec::new(),
            mapped: None,
            start_selection: None,
            selections_after: selections,
        }
    }
}

pub fn update_branch(
    branch: &[HistEvent],
    to: usize,
    max_len: usize,
    new_event: HistEvent,
) -> Branch {
    let start = if to + 1 > max_len + 20 {
        to - max_len - 1
    } else {
        0
    };
    let mut new_branch = branch[start..to].to_vec();
    new_branch.push(new_event);
    new_branch
}

pub fn is_adjacent(a: &ChangeDesc, b: &ChangeDesc) -> bool {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for change in a.iter_changes(false) {
        ranges.push((change.old.from, change.old.to));
    }
    for change in b.iter_changes(false) {
        let f = change.new.from;
        let t = change.new.to;
        for &(from, to) in &ranges {
            if t >= from && f <= to {
                return true;
            }
        }
    }
    false
}

pub fn eq_selection_shape(a: &Selection, b: &Selection) -> bool {
    let a_ranges = a.ranges_ref();
    let b_ranges = b.ranges_ref();
    if a_ranges.len() != b_ranges.len() {
        return false;
    }
    a_ranges
        .iter()
        .zip(b_ranges.iter())
        .all(|(r1, r2)| r1.empty() == r2.empty())
}

pub fn conc<T: Clone>(a: &[T], b: &[T]) -> Vec<T> {
    if a.is_empty() {
        b.to_vec()
    } else if b.is_empty() {
        a.to_vec()
    } else {
        let mut res = Vec::with_capacity(a.len() + b.len());
        res.extend_from_slice(a);
        res.extend_from_slice(b);
        res
    }
}

pub fn map_effects(effects: &[StateEffect], mapping: &ChangeDesc) -> Vec<StateEffect> {
    effects.iter().filter_map(|e| e.map(mapping)).collect()
}

pub fn add_selection(branch: &[HistEvent], selection: impl Into<Arc<Selection>>) -> Branch {
    let selection: Arc<Selection> = selection.into();
    if branch.is_empty() {
        vec![HistEvent::selection(vec![selection])]
    } else {
        let last_event = &branch[branch.len() - 1];
        let start = last_event
            .selections_after
            .len()
            .saturating_sub(MAX_SELECTIONS_PER_EVENT);
        let mut sels = last_event.selections_after[start..].to_vec();
        if let Some(last_sel) = sels.last() {
            if Arc::ptr_eq(last_sel, &selection) || last_sel.as_ref() == selection.as_ref() {
                return branch.to_vec();
            }
        }
        sels.push(selection);
        update_branch(
            branch,
            branch.len() - 1,
            1_000_000_000,
            last_event.set_sel_after(sels),
        )
    }
}

pub fn pop_selection(branch: &[HistEvent]) -> Branch {
    let mut new_branch = branch.to_vec();
    if let Some(last) = new_branch.last_mut() {
        let new_len = last.selections_after.len().saturating_sub(1);
        let mut sels = last.selections_after.clone();
        sels.truncate(new_len);
        *last = last.set_sel_after(sels);
    }
    new_branch
}

pub fn map_event(
    event: &HistEvent,
    mapping: &ChangeDesc,
    extra_selections: &[Arc<Selection>],
) -> Result<HistEvent, Error> {
    let mut mapped_selections = Vec::with_capacity(event.selections_after.len());
    for s in &event.selections_after {
        mapped_selections.push(Arc::new(s.map(mapping, Assoc::Before)?));
    }
    let selections = conc(&mapped_selections, extra_selections);

    let changes = match &event.changes {
        Some(c) => c,
        None => return Ok(HistEvent::selection(selections)),
    };

    let mapped_changes = changes.map(mapping, false)?;
    let before = mapping.map(changes, true)?;
    let full_mapping = match &event.mapped {
        Some(m) => Some(m.compose(&before)?),
        None => Some(before.clone()),
    };
    let start_selection = match &event.start_selection {
        Some(s) => Some(Arc::new(s.map(&before, Assoc::Before)?)),
        None => None,
    };
    let effects = map_effects(&event.effects, mapping);

    Ok(HistEvent {
        changes: Some(mapped_changes),
        effects,
        mapped: full_mapping,
        start_selection,
        selections_after: selections,
    })
}

pub fn add_mapping_to_branch(branch: &[HistEvent], mut mapping: ChangeDesc) -> Branch {
    if branch.is_empty() {
        return Vec::new();
    }
    let mut length = branch.len();
    let mut selections: Vec<Arc<Selection>> = Vec::new();
    while length > 0 {
        let event = match map_event(&branch[length - 1], &mapping, &selections) {
            Ok(ev) => ev,
            Err(_) => break,
        };
        let survived = event
            .changes
            .as_ref()
            .is_some_and(|c| !c.is_empty())
            || !event.effects.is_empty();
        if survived {
            let mut result = branch[..length].to_vec();
            result[length - 1] = event;
            return result;
        } else {
            mapping = match event.mapped {
                Some(m) => m,
                None => break,
            };
            length -= 1;
            selections = event.selections_after;
        }
    }
    if !selections.is_empty() {
        vec![HistEvent::selection(selections)]
    } else {
        Vec::new()
    }
}

pub fn is_joinable_user_event(event: &str) -> bool {
    let raw = event.strip_prefix("User.Event.").unwrap_or(event);
    if let Some(rest) = raw.strip_prefix("input.type") {
        rest.is_empty() || rest.starts_with('.')
    } else if let Some(rest) = raw.strip_prefix("delete") {
        rest.is_empty() || rest.starts_with('.')
    } else {
        false
    }
}

pub fn is_select_user_event(event: &str) -> bool {
    let raw = event.strip_prefix("User.Event.").unwrap_or(event);
    if let Some(rest) = raw.strip_prefix("select") {
        rest.is_empty() || rest.starts_with('.')
    } else {
        false
    }
}
