use powdr_number::GoldilocksField;
use powdr_pil_analyzer::{analyze_string, dead_committed_columns};

#[test]
fn dead_committed_columns_basic() {
    let input = r#"
        namespace N(16);
        pol commit a;
        pol commit b;
        a = 1;
    "#;

    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);

    assert!(dead.len() == 1, "expected 1 dead column, got: {dead:?}");
    assert!(dead[0].name == "N.b", "expected N.b to be dead, got: {dead:?}");
}


#[test]
fn dead_commited_test_1() {
    let input = r#"
        pol commit secret_value;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);

    assert!(dead.len() == 1, "expected 1 dead column, got: {dead:?}");
    assert!(dead[0].name == "secret_value", "expected secret_value to be dead, got: {dead:?}");
}

#[test]
fn dead_commited_test_2() {
    let input = r#"
        pol commit secret_value;
        pol commit other_value;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);
    assert!(dead.len() == 2, "expected 1 dead column, got: {dead:?}");
    assert!(dead[0].name == "secret_value", "expected secret_value to be dead, got: {dead:?}");
    assert!(dead[1].name == "other_value", "expected other_value to be dead, got: {dead:?}");
}

#[test]
fn used_in_itermidiate_then_constrained() {
    let input = r#"
        pol commit sel;
        pol commit expected;
        pol commit raw_value;
        pol PROCESSED = raw_value * raw_value;
        sel * (PROCESSED - expected) = 0;  // raw_value is constrained through PROCESSED
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);

    assert!(dead.len() == 0, "expected no dead columns, got: {dead:?}");
}

#[test]
fn used_in_intermediate_then_not_constrained() {
    let input = r#"
        pol commit raw_value;
        pol PROCESSED = raw_value * raw_value;
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);

    assert!(dead.len() == 1, "expected 1 dead column, got: {dead:?}");
    assert!(dead[0].name == "raw_value", "expected raw_value to be dead, got: {dead:?}");
}

#[test]
fn used_as_lookup_key_or_destination_is_not_dead() {
    let input = r#"
        namespace N(16);
        pol commit sel;
        pol commit query_value;
        pol commit precomputed_value;

        // If sel = 1, require query_value to appear in the "table" precomputed_value.
        // This uses both committed columns only through the lookup identity.
        sel { query_value } in precomputed_value { precomputed_value };
    "#;
    let analyzed = analyze_string::<GoldilocksField>(input);
    let dead = dead_committed_columns(&analyzed);

    assert!(
        dead.len() == 0,
        "expected no dead columns, got: {dead:?}"
    );
}
