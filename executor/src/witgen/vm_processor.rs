use ast::analyzed::{
    AlgebraicExpression as Expression, AlgebraicReference, Identity, IdentityKind, PolyID,
};
use itertools::Itertools;
use number::{DegreeType, FieldElement};
use parser_util::lines::indent;
use std::cmp::max;
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use crate::witgen::identity_processor::{self, IdentityProcessor};
use crate::witgen::rows::RowUpdater;
use crate::witgen::util::try_to_simple_poly;
use crate::witgen::Constraint;
use crate::witgen::IncompleteCause;

use super::data_structures::column_map::WitnessColumnMap;
use super::data_structures::finalizable_data::FinalizableData;
use super::processor::OuterQuery;
use super::query_processor::QueryProcessor;

use super::rows::{CellValue, Row, RowFactory, RowPair, UnknownStrategy};
use super::{
    Constraints, EvalError, EvalResult, EvalValue, FixedData, MutableState, QueryCallback,
};

/// Maximal period checked during loop detection.
const MAX_PERIOD: usize = 4;

/// A list of identities with a flag whether it is complete.
struct CompletableIdentities<'a, T: FieldElement> {
    identities_with_complete: Vec<(&'a Identity<Expression<T>>, bool)>,
}

impl<'a, T: FieldElement> CompletableIdentities<'a, T> {
    fn new(identities: impl Iterator<Item = &'a Identity<Expression<T>>>) -> Self {
        Self {
            identities_with_complete: identities.map(|identity| (identity, false)).collect(),
        }
    }

    /// Yields immutable references to the identity and mutable references to the complete flag.
    fn iter_mut(&mut self) -> impl Iterator<Item = (&'a Identity<Expression<T>>, &mut bool)> {
        self.identities_with_complete
            .iter_mut()
            .map(|(identity, complete)| (*identity, complete))
    }
}

pub struct VmProcessor<'a, T: FieldElement> {
    /// The global index of the first row of [VmProcessor::data].
    row_offset: DegreeType,
    /// The witness columns belonging to this machine
    witnesses: HashSet<PolyID>,
    /// Whether a given witness column is relevant for this machine (faster than doing a contains check on witnesses)
    is_relevant_witness: WitnessColumnMap<bool>,
    fixed_data: &'a FixedData<'a, T>,
    /// The subset of identities that contains a reference to the next row
    /// (precomputed once for performance reasons)
    identities_with_next_ref: Vec<&'a Identity<Expression<T>>>,
    /// The subset of identities that does not contain a reference to the next row
    /// (precomputed once for performance reasons)
    identities_without_next_ref: Vec<&'a Identity<Expression<T>>>,
    data: FinalizableData<'a, T>,
    last_report: DegreeType,
    last_report_time: Instant,
    row_factory: RowFactory<'a, T>,
    outer_query: Option<OuterQuery<'a, T>>,
    inputs: BTreeMap<PolyID, T>,
    previously_set_inputs: BTreeMap<PolyID, DegreeType>,
}

impl<'a, T: FieldElement> VmProcessor<'a, T> {
    pub fn new(
        row_offset: DegreeType,
        fixed_data: &'a FixedData<'a, T>,
        identities: &[&'a Identity<Expression<T>>],
        witnesses: HashSet<PolyID>,
        data: FinalizableData<'a, T>,
        row_factory: RowFactory<'a, T>,
    ) -> Self {
        let (identities_with_next, identities_without_next): (Vec<_>, Vec<_>) = identities
            .iter()
            .partition(|identity| identity.contains_next_ref());

        let is_relevant_witness = WitnessColumnMap::from(
            fixed_data
                .witness_cols
                .keys()
                .map(|poly_id| witnesses.contains(&poly_id)),
        );

        VmProcessor {
            row_offset,
            witnesses,
            is_relevant_witness,
            fixed_data,
            identities_with_next_ref: identities_with_next,
            identities_without_next_ref: identities_without_next,
            data,
            row_factory,
            last_report: 0,
            last_report_time: Instant::now(),
            outer_query: None,
            inputs: BTreeMap::new(),
            previously_set_inputs: BTreeMap::new(),
        }
    }

    pub fn with_outer_query(self, outer_query: OuterQuery<'a, T>) -> Self {
        log::trace!("  Extracting inputs:");
        let mut inputs = BTreeMap::new();
        for (l, r) in outer_query.left.iter().zip(&outer_query.right.expressions) {
            if let Some(right_poly) = try_to_simple_poly(r).map(|p| p.poly_id) {
                if let Some(l) = l.constant_value() {
                    log::trace!("    {} = {}", r, l);
                    inputs.insert(right_poly, l);
                }
            }
        }
        Self {
            outer_query: Some(outer_query),
            inputs,
            ..self
        }
    }

    pub fn finish(self) -> FinalizableData<'a, T> {
        self.data
    }

    /// Starting out with a single row (at a given offset), iteratively append rows
    /// until we have exhausted the rows or the latch expression (if available) evaluates to 1.
    pub fn run<Q: QueryCallback<T>>(
        &mut self,
        mutable_state: &mut MutableState<'a, '_, T, Q>,
    ) -> EvalValue<&'a AlgebraicReference, T> {
        assert!(self.data.len() == 1);

        let mut outer_assignments = vec![];

        // Are we in an infinite loop and can just re-use the old values?
        let mut looping_period = None;
        let mut loop_detection_log_level = log::Level::Info;
        let rows_left = self.fixed_data.degree - self.row_offset + 1;
        let mut finalize_start = 1;
        for row_index in 0..rows_left {
            self.maybe_log_performance(row_index);

            if (row_index + 1) % 10000 == 0 {
                // Periodically make sure most rows are finalized.
                // Row 0 and the last MAX_PERIOD rows might be needed later, so they are not finalized.
                let finalize_end = row_index as usize - MAX_PERIOD;
                self.data.finalize_range(finalize_start..finalize_end);
                finalize_start = finalize_end;
            }

            // Check if we are in a loop.
            if looping_period.is_none() && row_index % 100 == 0 && row_index > 0 {
                looping_period = self.rows_are_repeating(row_index);
                if let Some(p) = looping_period {
                    log::log!(
                        loop_detection_log_level,
                        "Found loop with period {p} starting at row {row_index}"
                    );
                }
            }
            if let Some(period) = looping_period {
                let proposed_row = self.data[row_index as usize - period].clone();
                if !self.try_proposed_row(row_index, proposed_row, mutable_state) {
                    log::log!(
                        loop_detection_log_level,
                        "Looping failed. Trying to generate regularly again. (Use RUST_LOG=debug to see whether this happens more often.)"
                    );
                    looping_period = None;
                    // For some programs, loop detection will often find loops and then fail.
                    // In this case, we don't want to spam the user with debug messages.
                    loop_detection_log_level = log::Level::Debug;
                }
            }
            // Note that we exit one iteration early in the non-loop case,
            // because ensure_has_next_row() + compute_row() will already
            // add and compute some values for the next row as well.
            if looping_period.is_none() && row_index != rows_left - 1 {
                self.ensure_has_next_row(row_index);
                outer_assignments.extend(self.compute_row(row_index, mutable_state).into_iter());

                // Evaluate latch expression and return if it evaluates to 1.
                if let Some(latch) = self.latch_value(row_index) {
                    if latch {
                        log::trace!("Machine returns!");
                        if self.outer_query.as_ref().unwrap().is_complete() {
                            return EvalValue::complete(outer_assignments);
                        } else {
                            return EvalValue::incomplete_with_constraints(
                                outer_assignments,
                                IncompleteCause::BlockMachineLookupIncomplete,
                            );
                        }
                    }
                } else if self.outer_query.is_some() {
                    // If we have an outer query (and therefore a latch expression),
                    // its value should be known at this point.
                    // Probably, we don't have all the necessary inputs.
                    return EvalValue::incomplete(IncompleteCause::UnknownLatch);
                }
            };
        }

        assert_eq!(
            self.data.len() as DegreeType + self.row_offset,
            self.fixed_data.degree + 1
        );

        EvalValue::complete(outer_assignments)
    }

    /// Checks if the last rows are repeating and returns the period.
    /// Only checks for periods of 1, ..., MAX_PERIOD.
    fn rows_are_repeating(&self, row_index: DegreeType) -> Option<usize> {
        if row_index < MAX_PERIOD as DegreeType {
            return None;
        }

        let row = row_index as usize;
        (1..MAX_PERIOD).find(|&period| {
            (1..=period).all(|i| {
                self.data[row - i - period]
                    .values()
                    .zip(self.data[row - i].values())
                    .all(|(a, b)| a.value == b.value)
            })
        })
    }

    fn latch_value(&self, row_index: DegreeType) -> Option<bool> {
        let row_pair = RowPair::from_single_row(
            self.row(row_index),
            row_index + self.row_offset,
            self.fixed_data,
            UnknownStrategy::Unknown,
        );
        self.outer_query
            .as_ref()
            .and_then(|outer_query| outer_query.right.selector.as_ref())
            .and_then(|latch| row_pair.evaluate(latch).ok())
            .and_then(|l| l.constant_value())
            .map(|l| l.is_one())
    }

    // Returns a reference to a given row
    fn row(&self, row_index: DegreeType) -> &Row<'a, T> {
        &self.data[row_index as usize]
    }

    fn ensure_has_next_row(&mut self, row_index: DegreeType) {
        assert!(self.data.len() as DegreeType > row_index);
        if row_index == self.data.len() as DegreeType - 1 {
            self.data.push(self.row_factory.fresh_row(row_index + 1));
        }
    }

    fn compute_row<Q: QueryCallback<T>>(
        &mut self,
        row_index: DegreeType,
        mutable_state: &mut MutableState<'a, '_, T, Q>,
    ) -> Constraints<&'a AlgebraicReference, T> {
        log::trace!("Row: {}", row_index + self.row_offset);

        log::trace!("  Going over all identities until no more progress is made");
        // First, go over identities that don't reference the next row,
        // Second, propagate values to the next row by going over identities that do reference the next row.
        let mut identities_without_next_ref =
            CompletableIdentities::new(self.identities_without_next_ref.iter().cloned());
        let mut identities_with_next_ref =
            CompletableIdentities::new(self.identities_with_next_ref.iter().cloned());
        let outer_assignments = self
            .loop_until_no_progress(row_index, &mut identities_without_next_ref, mutable_state)
            .and_then(|outer_assignments| {
                Ok(outer_assignments
                    .into_iter()
                    .chain(self.loop_until_no_progress(
                        row_index,
                        &mut identities_with_next_ref,
                        mutable_state,
                    )?)
                    .collect::<Vec<_>>())
            })
            .map_err(|e| self.report_failure_and_panic_unsatisfiable(row_index, e))
            .unwrap();

        // Check that the computed row is "final" by asserting that all unknown values can
        // be set to 0.
        // This check is only done for the primary machine, as secondary machines might simply
        // not have all the inputs yet and therefore be underconstrained.
        if self.outer_query.is_none() {
            log::trace!(
                "  Checking that remaining identities hold when unknown values are set to 0"
            );
            let mut identity_processor = IdentityProcessor::new(self.fixed_data, mutable_state);
            self.process_identities(
                row_index,
                &mut identities_without_next_ref,
                UnknownStrategy::Zero,
                &mut identity_processor,
            )
            .and_then(|_| {
                self.process_identities(
                    row_index,
                    &mut identities_with_next_ref,
                    UnknownStrategy::Zero,
                    &mut identity_processor,
                )
            })
            .map_err(|e| self.report_failure_and_panic_underconstrained(row_index, e))
            .unwrap();
        }

        log::trace!(
            "{}",
            self.row(row_index).render(
                &format!("===== Row {}", row_index as DegreeType + self.row_offset),
                true,
                &self.witnesses
            )
        );

        outer_assignments
    }

    /// Loops over all identities and queries, until no further progress is made.
    /// @returns the "incomplete" identities, i.e. identities that contain unknown values.
    fn loop_until_no_progress<Q: QueryCallback<T>>(
        &mut self,
        row_index: DegreeType,
        identities: &mut CompletableIdentities<'a, T>,
        mutable_state: &mut MutableState<'a, '_, T, Q>,
    ) -> Result<Constraints<&'a AlgebraicReference, T>, Vec<EvalError<T>>> {
        let mut outer_assignments = vec![];
        loop {
            let mut identity_processor = IdentityProcessor::new(self.fixed_data, mutable_state);
            let mut progress = self.process_identities(
                row_index,
                identities,
                UnknownStrategy::Unknown,
                &mut identity_processor,
            )?;
            if let Some(true) = self.latch_value(row_index) {
                let (outer_query_progress, new_outer_assignments) = self
                    .process_outer_query(row_index, mutable_state)
                    .map_err(|e| vec![e])?;
                progress |= outer_query_progress;
                outer_assignments.extend(new_outer_assignments);
            }

            progress |= self.set_inputs_if_unset(row_index);

            let mut updates = EvalValue::complete(vec![]);
            let mut query_processor =
                QueryProcessor::new(self.fixed_data, mutable_state.query_callback);
            let row_pair = RowPair::new(
                self.row(row_index),
                self.row(row_index + 1),
                row_index + self.row_offset,
                self.fixed_data,
                UnknownStrategy::Unknown,
            );
            for poly_id in self.fixed_data.witness_cols.keys() {
                if self.is_relevant_witness[&poly_id] {
                    updates.combine(query_processor.process_query(&row_pair, &poly_id));
                }
            }
            progress |= self.apply_updates(row_index, &updates, || "queries".to_string());

            if !progress {
                break;
            }
        }
        Ok(outer_assignments)
    }

    /// Loops over all identities once and updates the current row and next row.
    /// Arguments:
    /// * `identities`: Identities to process. Completed identities are removed from the list.
    /// * `unknown_strategy`: How to process unknown variables. Either use zero or keep it symbolic.
    /// Returns:
    /// * `Ok(true)`: If progress was made.
    /// * `Ok(false)`: If no progress was made.
    /// * `Err(errors)`: If an error occurred.
    fn process_identities<Q: QueryCallback<T>>(
        &mut self,
        row_index: DegreeType,
        identities: &mut CompletableIdentities<'a, T>,
        unknown_strategy: UnknownStrategy,
        identity_processor: &mut IdentityProcessor<'a, '_, '_, T, Q>,
    ) -> Result<bool, Vec<EvalError<T>>> {
        let mut progress = false;
        let mut errors = vec![];

        for (identity, is_complete) in identities.iter_mut() {
            if *is_complete {
                continue;
            }

            let is_machine_call = matches!(
                identity.kind,
                IdentityKind::Plookup | IdentityKind::Permutation
            );
            if is_machine_call && unknown_strategy == UnknownStrategy::Zero {
                // The fact that we got to the point where we assume 0 for unknown cells, but this identity
                // is still not complete, means that either the inputs or the machine is under-constrained.
                errors.push(format!("{identity}:\n{}",
                    indent("This machine call could not be completed. Either some inputs are missing or the machine is under-constrained.", "    ")).into());
                continue;
            }

            let row_pair = RowPair::new(
                self.row(row_index),
                self.row(row_index + 1),
                row_index + self.row_offset,
                self.fixed_data,
                unknown_strategy,
            );
            let result: EvalResult<'a, T> = identity_processor
                .process_identity(identity, &row_pair)
                .map_err(|err| {
                    format!("{identity}:\n{}", indent(&format!("{err}"), "    ")).into()
                });

            match result {
                Ok(eval_value) => {
                    if unknown_strategy == UnknownStrategy::Zero {
                        assert!(eval_value.constraints.is_empty())
                    } else {
                        *is_complete = eval_value.is_complete();
                        progress |=
                            self.apply_updates(row_index, &eval_value, || format!("{identity}"));
                    }
                }
                Err(e) => {
                    errors.push(e);
                }
            };
        }

        if errors.is_empty() {
            Ok(progress)
        } else {
            Err(errors)
        }
    }

    fn process_outer_query<Q: QueryCallback<T>>(
        &mut self,
        row_index: DegreeType,
        mutable_state: &mut MutableState<'a, '_, T, Q>,
    ) -> Result<(bool, Constraints<&'a AlgebraicReference, T>), EvalError<T>> {
        let OuterQuery { left, right } = self
            .outer_query
            .as_ref()
            .expect("Asked to process outer query, but it was not set!");

        let row_pair = RowPair::new(
            &self.data[row_index as usize],
            &self.data[row_index as usize + 1],
            (row_index + self.row_offset) % self.fixed_data.degree,
            self.fixed_data,
            UnknownStrategy::Unknown,
        );

        let mut identity_processor = IdentityProcessor::new(self.fixed_data, mutable_state);
        let updates = identity_processor
            .process_link(left, right, &row_pair)
            .map_err(|e| {
                log::warn!("Error in outer query: {e}");
                log::warn!("Some of the following entries could not be matched:");
                for (l, r) in left.iter().zip(right.expressions.iter()) {
                    if let Ok(r) = row_pair.evaluate(r) {
                        log::warn!("  => {} = {}", l, r);
                    }
                }
                e
            })?;

        let progress = self.apply_updates(row_index, &updates, || "outer query".to_string());

        let outer_assignments = updates
            .constraints
            .into_iter()
            .filter(|(poly, _)| !self.witnesses.contains(&poly.poly_id))
            .collect::<Vec<_>>();

        Ok((progress, outer_assignments))
    }

    /// Sets the inputs to the values given in [VmProcessor::inputs] if they are not already set.
    /// Typically, inputs will have a constraint of the form: `((1 - instr__reset) * (_input' - _input)) = 0;`
    /// So, once the value of `_input` is set, this function will do nothing until the next reset instruction.
    /// However, if `_input` does become unconstrained, we need to undo all changes we've done so far.
    /// For this reason, we keep track of all changes we've done to inputs in [VmProcessor::previously_set_inputs].
    fn set_inputs_if_unset(&mut self, row_index: DegreeType) -> bool {
        let mut input_updates = EvalValue::complete(vec![]);
        for (poly_id, value) in self.inputs.iter() {
            match &self.data[row_index as usize][poly_id].value {
                CellValue::Known(_) => {}
                CellValue::RangeConstraint(_) | CellValue::Unknown => {
                    input_updates.combine(EvalValue::complete([(
                        &self.fixed_data.witness_cols[poly_id].poly,
                        Constraint::Assignment(*value),
                    )]));
                }
            };
        }

        for (poly, _) in &input_updates.constraints {
            let poly_id = poly.poly_id;
            if let Some(start_row) = self.previously_set_inputs.remove(&poly_id) {
                log::trace!(
                    "    Resetting previously set inputs for column: {}",
                    self.fixed_data.column_name(&poly_id)
                );
                for row_index in start_row..row_index {
                    self.data[row_index as usize][&poly_id].value = CellValue::Unknown;
                }
            }
        }
        for (poly, _) in &input_updates.constraints {
            self.previously_set_inputs.insert(poly.poly_id, row_index);
        }
        self.apply_updates(row_index, &input_updates, || "inputs".to_string())
    }

    fn apply_updates(
        &mut self,
        row_index: DegreeType,
        updates: &EvalValue<&'a AlgebraicReference, T>,
        source_name: impl Fn() -> String,
    ) -> bool {
        if updates.constraints.is_empty() {
            return false;
        }

        log::trace!("    Updates from: {}", source_name());

        // Build RowUpdater
        // (a bit complicated, because we need two mutable
        // references to elements of the same vector)
        let (current, next) = self.data.mutable_row_pair(row_index as usize);
        let mut row_updater = RowUpdater::new(current, next, row_index + self.row_offset);

        for (poly, c) in &updates.constraints {
            if self.witnesses.contains(&poly.poly_id) {
                row_updater.apply_update(poly, c);
            } else if let Constraint::Assignment(v) = c {
                let left = &mut self.outer_query.as_mut().unwrap().left;
                log::trace!("      => {} (outer) = {}", poly, v);
                for l in left.iter_mut() {
                    l.assign(poly, *v);
                }
            };
        }

        true
    }

    fn report_failure_and_panic_unsatisfiable(
        &self,
        row_index: DegreeType,
        failures: Vec<EvalError<T>>,
    ) -> ! {
        log::error!(
            "\nError: Row {} failed. Set RUST_LOG=debug for more information.\n",
            row_index + self.row_offset
        );
        log::debug!("Some identities where not satisfiable after the following values were uniquely determined (known nonzero first, then zero, unknown omitted):");
        log::debug!(
            "{}",
            self.row(row_index)
                .render("Current Row", false, &self.witnesses)
        );
        log::debug!(
            "{}",
            self.row(row_index + 1)
                .render("Next Row", false, &self.witnesses)
        );
        log::debug!("Set RUST_LOG=trace to understand why these values were chosen.");
        log::debug!(
            "Assuming these values are correct, the following identities fail:\n{}\n",
            failures
                .iter()
                .map(|r| indent(&r.to_string(), "    "))
                .join("\n")
        );
        panic!("Witness generation failed.");
    }

    fn report_failure_and_panic_underconstrained(
        &self,
        row_index: DegreeType,
        failures: Vec<EvalError<T>>,
    ) -> ! {
        log::error!(
            "\nError: Row {} failed. Set RUST_LOG=debug for more information.\n",
            row_index + self.row_offset
        );

        log::debug!("Some columns could not be determined, but setting them to zero does not satisfy the constraints. This typically means that the system is underconstrained!");
        log::debug!(
            "{}",
            self.row(row_index)
                .render("Current Row", true, &self.witnesses)
        );
        log::debug!(
            "{}",
            self.row(row_index)
                .render("Next Row", true, &self.witnesses)
        );
        log::debug!("\nSet RUST_LOG=trace to understand why these values were (not) chosen.");
        log::debug!(
            "Assuming zero for unknown values, the following identities fail:\n{}\n",
            failures
                .iter()
                .map(|r| indent(&r.to_string(), "    "))
                .join("\n")
        );
        panic!("Witness generation failed.");
    }

    /// Verifies the proposed values for the next row.
    /// TODO this is bad for machines because we might introduce rows in the machine that are then
    /// not used.
    fn try_proposed_row<'b, Q: QueryCallback<T>>(
        &mut self,
        row_index: DegreeType,
        proposed_row: Row<'a, T>,
        mutable_state: &mut MutableState<'a, 'b, T, Q>,
    ) -> bool {
        let constraints_valid = {
            let mut identity_processor = IdentityProcessor::new(self.fixed_data, mutable_state);
            self.check_row_pair(row_index, &proposed_row, false, &mut identity_processor)
                && self.check_row_pair(row_index, &proposed_row, true, &mut identity_processor)
        };

        if constraints_valid {
            if row_index as usize == self.data.len() - 1 {
                // We might already have added the row in [VmProcessor::ensure_has_next_row]
                self.data[row_index as usize] = proposed_row;
            } else {
                // If the previous row was also added by [VmProcessor::try_propose_row], we won't have an entry
                // for the current row yet.
                assert_eq!(row_index as usize, self.data.len());
                self.data.push(proposed_row);
            }
        } else {
            // Note that we never update the next row if proposing a row succeeds (the happy path).
            // If it doesn't, we re-run compute_next_row on the previous row in order to
            // correctly forward-propagate values via next references.
            self.ensure_has_next_row(row_index - 1);
            self.compute_row(row_index - 1, mutable_state);
        }
        constraints_valid
    }

    fn check_row_pair<Q: QueryCallback<T>>(
        &self,
        row_index: DegreeType,
        proposed_row: &Row<'a, T>,
        previous: bool,
        identity_processor: &mut IdentityProcessor<'a, '_, '_, T, Q>,
    ) -> bool {
        let row_pair = match previous {
            // Check whether identities with a reference to the next row are satisfied
            // when applied to the previous row and the proposed row.
            true => RowPair::new(
                self.row(row_index - 1),
                proposed_row,
                row_index + self.row_offset - 1,
                self.fixed_data,
                UnknownStrategy::Zero,
            ),
            // Check whether identities without a reference to the next row are satisfied
            // when applied to the proposed row.
            // Because we never access the next row, we can use [RowPair::from_single_row] here.
            false => RowPair::from_single_row(
                proposed_row,
                row_index + self.row_offset,
                self.fixed_data,
                UnknownStrategy::Zero,
            ),
        };

        // Check identities depending on whether or not they have a reference to the next row:
        // - Those that do should be checked on the previous and proposed row
        // - All others should be checked on the proposed row
        let identities = match previous {
            true => &self.identities_with_next_ref,
            false => &self.identities_without_next_ref,
        };

        for identity in identities.iter() {
            if identity_processor
                .process_identity(identity, &row_pair)
                .is_err()
            {
                log::debug!("Previous {:?}", self.row(row_index - 1));
                log::debug!("Proposed {:?}", proposed_row);
                log::debug!("Failed on identity: {}", identity);

                return false;
            }
        }
        true
    }

    fn maybe_log_performance(&mut self, row_index: DegreeType) {
        if row_index >= self.last_report + 1000 {
            let duration = self.last_report_time.elapsed();
            self.last_report_time = Instant::now();

            let identity_statistics = identity_processor::get_and_reset_solving_statistics();
            let identities_per_sec =
                ((identity_statistics.values().map(|s| s.success).sum::<u64>() as u128 * 1000)
                    / duration.as_micros()) as u64;
            let identities_count = max(identity_statistics.len() as u64, 1);
            let progress_percentage = identity_statistics
                .values()
                .map(|s| s.success * 100 / s.invocations)
                .sum::<u64>()
                / identities_count;

            let row = row_index + self.row_offset;
            log::info!(
                "{row} of {} rows ({}%) - {} rows/s, {identities_per_sec}k identities/s, {progress_percentage}% progress",
                self.fixed_data.degree,
                row * 100 / self.fixed_data.degree,
                1_000_000_000 / duration.as_micros()
            );
            self.last_report = row_index;
        }
    }
}
