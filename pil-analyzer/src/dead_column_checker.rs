use std::collections::HashSet;

use powdr_ast::analyzed::{AlgebraicExpression, Analyzed, PolyID, PolynomialType, PublicDeclaration};
use powdr_number::FieldElement;
use powdr_parser_util::SourceRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadColumn {
    pub name: String,
    pub source: SourceRef,
}

/// Returns committed/witness columns (including array elements) that are declared but never used
/// by any identity (constraints/lookups/permutations), nor by public declarations.
pub fn dead_committed_columns<T: FieldElement>(analyzed: &Analyzed<T>) -> Vec<DeadColumn> {
    let used_poly_ids = used_poly_ids(analyzed);

    // Get all committed polynomials, filtering out auto-added symbols and non-committed polynomials
    let committed_polynomials = analyzed
        .definitions
        .iter()
        .filter(|(name, (sym, _))| {
            !analyzed.auto_added_symbols.contains(*name)
                && matches!(sym.kind, powdr_ast::analyzed::SymbolKind::Poly(PolynomialType::Committed))
        });

    // Filter all used polynomials from the committed polynomials
    // Return vector of DeadColumn
    committed_polynomials.flat_map(|(_name, (sym, _def))| {
            sym.array_elements()
                .map(|(elem_name, poly_id)| (elem_name, sym.source.clone(), poly_id))
                .collect::<Vec<_>>()
        })
        .filter(|(_name, _src, poly_id)| !used_poly_ids.contains(poly_id))
        .map(|(name, source, _poly_id)| DeadColumn { name, source })
        .collect()
}

pub fn check_no_dead_committed_columns<T: FieldElement>(analyzed: &Analyzed<T>) -> Result<(), String> {
    let dead = dead_committed_columns(analyzed);
    if dead.is_empty() {
        return Ok(());
    }

    let formatted = dead
        .iter()
        .map(|d| format!("- {} ({})", d.name, format_source(&d.source)))
        .collect::<Vec<_>>()
        .join("\n");

    Err(format!(
        "Dead committed columns detected (declared but never referenced by any constraint/lookup/permutation/public declaration):\n{formatted}"
    ))
}

fn used_poly_ids<T: FieldElement>(analyzed: &Analyzed<T>) -> HashSet<PolyID> {
    let mut used: HashSet<PolyID> = HashSet::new();

    // Count usage from public declarations.
    for decl in analyzed.public_declarations.values() {
        used_from_public_declaration(decl, &mut used);
    }

    // Count usage from identities, after inlining intermediates.
    for identity in analyzed.identities_with_inlined_intermediate_polynomials() {
        for expr in identity
            .left
            .selector
            .iter()
            .chain(identity.left.expressions.iter())
            .chain(identity.right.selector.iter())
            .chain(identity.right.expressions.iter())
        {
            collect_poly_ids(expr, &mut used);
        }
    }

    used
}

fn used_from_public_declaration(decl: &PublicDeclaration, used: &mut HashSet<PolyID>) {
    // Note: poly_id is filled in during condensation.
    if let Some(poly_id) = decl.polynomial.poly_id {
        used.insert(poly_id);
    }
}

fn collect_poly_ids<T>(expr: &AlgebraicExpression<T>, used: &mut HashSet<PolyID>) {
    match expr {
        AlgebraicExpression::Reference(r) => {
            used.insert(r.poly_id);
        }
        AlgebraicExpression::BinaryOperation(op) => {
            collect_poly_ids(&op.left, used);
            collect_poly_ids(&op.right, used);
        }
        AlgebraicExpression::UnaryOperation(op) => {
            collect_poly_ids(&op.expr, used);
        }
        AlgebraicExpression::PublicReference(_)
        | AlgebraicExpression::Challenge(_)
        | AlgebraicExpression::Number(_) => {}
    }
}

fn format_source(source: &SourceRef) -> String {
    let file = source.file_name.as_ref().map(|s| s.as_ref()).unwrap_or("<unknown>");
    format!("{file}:{}..{}", source.start, source.end)
}


