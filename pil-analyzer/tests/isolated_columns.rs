use powdr_number::GoldilocksField;
use powdr_pil_analyzer::{analyze_string, isolated_committed_columns};

#[test]
fn isolated_if_only_self_constrained() {
    let input = r#"
        namespace N(16);
        pol commit a;
        (a - 1) * a = 0;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let isolated = isolated_committed_columns(&analyzed);

    assert!(isolated.len() == 1, "expected 1 isolated column, got: {isolated:?}");
    assert!(isolated[0].name == "N.a", "expected N.a to be isolated, got: {isolated:?}");
}

#[test]
fn not_isolated_when_constrained_with_other_committed() {
    let input = r#"
        namespace N(16);
        pol commit a;
        pol commit b;
        a - b = 0;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let isolated = isolated_committed_columns(&analyzed);

    assert!(isolated.len() == 0, "expected no isolated columns, got: {isolated:?}");
}

#[test]
fn not_isolated_when_constrained_with_fixed() {
    let input = r#"
        namespace N(16);
        pol constant c;
        pol commit a;
        a - c = 0;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let isolated = isolated_committed_columns(&analyzed);

    assert!(isolated.len() == 0, "expected no isolated columns, got: {isolated:?}");
}

#[test]
fn isolated_if_only_used_in_unused_intermediate() {
    let input = r#"
        namespace N(16);
        pol commit a;
        pol X = a + 1;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let isolated = isolated_committed_columns(&analyzed);

    assert!(isolated.len() == 1, "expected 1 isolated column, got: {isolated:?}");
    assert!(isolated[0].name == "N.a", "expected N.a to be isolated, got: {isolated:?}");
}
 
#[test]
fn not_isolated_when_used_in_lookup_identity() {
    let input = r#"
        namespace N(16);
        pol commit sel;
        pol commit a;
        pol commit table;

        // Use `a` only through a lookup identity; this should count as co-occurrence.
        sel { a } in table { table };
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let isolated = isolated_committed_columns(&analyzed);

    assert!(isolated.len() == 0, "expected no isolated columns, got: {isolated:?}");
}


