use std::collections::HashMap;
use std::fmt::Display;

use crate::analyzer::{Analyzed, Expression, FunctionValueDefinition};
use crate::number::{DegreeType, FieldElement};

use self::bit_constraints::BitConstraint;
use self::eval_error::EvalError;
use self::util::WitnessColumnNamer;

mod affine_expression;
mod bit_constraints;
mod eval_error;
mod expression_evaluator;
pub mod fixed_evaluator;
mod generator;
mod machines;
pub mod symbolic_evaluator;
mod symbolic_witness_evaluator;
mod util;

/// Generates the committed polynomial values
/// @returns the values (in source order) and the degree of the polynomials.
pub fn generate<'a>(
    analyzed: &'a Analyzed,
    degree: DegreeType,
    fixed_cols: &[(&str, Vec<FieldElement>)],
    query_callback: Option<impl FnMut(&str) -> Option<FieldElement>>,
) -> Vec<(&'a str, Vec<FieldElement>)> {
    let witness_cols: Vec<WitnessColumn> = analyzed
        .committed_polys_in_source_order()
        .iter()
        .enumerate()
        .map(|(i, (poly, value))| {
            if poly.length.is_some() {
                unimplemented!("Committed arrays not implemented.")
            }
            WitnessColumn::new(i, &poly.absolute_name, value)
        })
        .collect();
    let fixed = FixedData::new(
        degree,
        &analyzed.constants,
        fixed_cols.iter().map(|(n, v)| (*n, v)).collect(),
        &witness_cols,
        witness_cols.iter().map(|w| (w.name, w.id)).collect(),
    );
    let (global_bit_constraints, identities) =
        bit_constraints::determine_global_constraints(&fixed, analyzed.identities.iter().collect());
    let (mut fixed_lookup, machines, identities) = machines::machine_extractor::split_out_machines(
        &fixed,
        identities,
        &witness_cols,
        &global_bit_constraints,
    );
    let mut generator = generator::Generator::new(
        &fixed,
        &mut fixed_lookup,
        &identities,
        global_bit_constraints,
        machines,
        query_callback,
    );

    let mut values: Vec<(&str, Vec<FieldElement>)> =
        witness_cols.iter().map(|p| (p.name, Vec::new())).collect();
    for row in 0..degree as DegreeType {
        let row_values = generator.compute_next_row(row);
        for (col, v) in row_values.into_iter().enumerate() {
            values[col].1.push(v);
        }
    }
    for (col, v) in generator.compute_next_row(0).into_iter().enumerate() {
        if v != values[col].1[0] {
            eprintln!("Wrap-around value for column {} does not match: {} (wrap-around) vs. {} (first row).",
            witness_cols[col].name, v, values[col].1[0]);
        }
    }
    for (name, data) in generator.machine_witness_col_values() {
        let (_, col) = values.iter_mut().find(|(n, _)| *n == name).unwrap();
        *col = data;
    }
    values
}

/// Result of evaluating an expression / lookup.
/// New assignments or constraints for witness columns identified by an ID.
type EvalResult = Result<Vec<(usize, Constraint)>, EvalError>;

#[derive(Debug, Clone, PartialEq)]
pub enum Constraint {
    Assignment(FieldElement),
    BitConstraint(BitConstraint),
}

impl Display for Constraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Constraint::Assignment(a) => write!(f, " = {a}"),
            Constraint::BitConstraint(bc) => write!(f, ":& {bc}"),
        }
    }
}

/// Data that is fixed for witness generation.
pub struct FixedData<'a> {
    degree: DegreeType,
    constants: &'a HashMap<String, FieldElement>,
    fixed_cols: HashMap<&'a str, &'a Vec<FieldElement>>,
    witness_cols: &'a Vec<WitnessColumn<'a>>,
    witness_ids: HashMap<&'a str, usize>,
}

impl<'a> FixedData<'a> {
    pub fn new(
        degree: DegreeType,
        constants: &'a HashMap<String, FieldElement>,
        fixed_cols: HashMap<&'a str, &'a Vec<FieldElement>>,
        witness_cols: &'a Vec<WitnessColumn<'a>>,
        witness_ids: HashMap<&'a str, usize>,
    ) -> Self {
        FixedData {
            degree,
            constants,
            fixed_cols,
            witness_cols,
            witness_ids,
        }
    }

    fn witness_cols(&self) -> impl Iterator<Item = &WitnessColumn> {
        self.witness_cols.iter()
    }
}

impl<'a> WitnessColumnNamer for FixedData<'a> {
    fn name(&self, i: usize) -> String {
        self.witness_cols[i].name.to_string()
    }
}

pub struct WitnessColumn<'a> {
    id: usize,
    name: &'a str,
    query: Option<&'a Expression>,
}

impl<'a> WitnessColumn<'a> {
    pub fn new(
        id: usize,
        name: &'a str,
        value: &'a Option<FunctionValueDefinition>,
    ) -> WitnessColumn<'a> {
        let query = if let Some(FunctionValueDefinition::Query(query)) = value {
            Some(query)
        } else {
            None
        };
        WitnessColumn { id, name, query }
    }
}
