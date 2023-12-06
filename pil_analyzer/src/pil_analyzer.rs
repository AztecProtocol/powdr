use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ast::parsed::PilStatement;
use number::{DegreeType, FieldElement};

use ast::analyzed::{
    Analyzed, Expression, FunctionValueDefinition, Identity, PublicDeclaration, SourceRef,
    StatementIdentifier, Symbol,
};

use crate::AnalysisDriver;

use crate::statement_processor::{Counters, PILItem, StatementProcessor};
use crate::{condenser, evaluator, expression_processor::ExpressionProcessor};

pub fn process_pil_file<T: FieldElement>(path: &Path) -> Analyzed<T> {
    let mut analyzer = PILAnalyzer::new();
    analyzer.process_file(path);
    analyzer.condense()
}

pub fn process_pil_file_contents<T: FieldElement>(contents: &str) -> Analyzed<T> {
    let mut analyzer = PILAnalyzer::new();
    analyzer.process_file_contents(Path::new("input"), contents);
    analyzer.condense()
}

// TODO we could further extract a component that is only responsible for
// collecting definitions, assigning IDs and maintaining the source order.

struct PILAnalyzer<T> {
    namespace: String,
    polynomial_degree: Option<DegreeType>,
    definitions: HashMap<String, (Symbol, Option<FunctionValueDefinition<T>>)>,
    public_declarations: HashMap<String, PublicDeclaration>,
    identities: Vec<Identity<Expression<T>>>,
    /// The order in which definitions and identities
    /// appear in the source.
    source_order: Vec<StatementIdentifier>,
    symbol_counters: Option<Counters>,
    included_files: HashSet<PathBuf>,
    line_starts: Vec<usize>,
    current_file: PathBuf,
}

impl<T: FieldElement> PILAnalyzer<T> {
    pub fn new() -> PILAnalyzer<T> {
        PILAnalyzer {
            namespace: "Global".to_string(),
            polynomial_degree: None,
            definitions: Default::default(),
            public_declarations: Default::default(),
            identities: vec![],
            source_order: vec![],
            included_files: Default::default(),
            line_starts: Default::default(),
            current_file: Default::default(),
            symbol_counters: Some(Default::default()),
        }
    }

    pub fn process_file(&mut self, path: &Path) {
        let path = path
            .canonicalize()
            .unwrap_or_else(|e| panic!("File {path:?} not found: {e}"));
        if !self.included_files.insert(path.clone()) {
            return;
        }
        let contents = fs::read_to_string(path.clone()).unwrap();

        self.process_file_contents(&path, &contents);
    }

    #[allow(clippy::print_stderr)]
    pub fn process_file_contents(&mut self, path: &Path, contents: &str) {
        let old_current_file = std::mem::take(&mut self.current_file);
        let old_line_starts = std::mem::take(&mut self.line_starts);

        // TODO make this work for other line endings
        self.line_starts = parser_util::lines::compute_line_starts(contents);
        self.current_file = path.to_path_buf();
        let pil_file =
            parser::parse(Some(path.to_str().unwrap()), contents).unwrap_or_else(|err| {
                eprintln!("Error parsing .pil file:");
                err.output_to_stderr();
                panic!();
            });

        for statement in pil_file.0 {
            self.handle_statement(statement);
        }

        self.current_file = old_current_file;
        self.line_starts = old_line_starts;
    }

    pub fn condense(self) -> Analyzed<T> {
        condenser::condense(
            self.polynomial_degree,
            self.definitions,
            self.public_declarations,
            &self.identities,
            self.source_order,
        )
    }

    fn handle_statement(&mut self, statement: PilStatement<T>) {
        match statement {
            PilStatement::Include(_, include) => self.handle_include(include),
            PilStatement::Namespace(_, name, degree) => self.handle_namespace(name, degree),
            _ => {
                // We need a mutable reference to the counter, but it is short-lived.
                let mut counters = self.symbol_counters.take().unwrap();
                let items =
                    StatementProcessor::new(self.driver(), &mut counters, self.polynomial_degree)
                        .handle_statement(statement);
                self.symbol_counters = Some(counters);
                for item in items {
                    match item {
                        PILItem::Definition(symbol, value) => {
                            let name = symbol.absolute_name.clone();
                            let is_new = self
                                .definitions
                                .insert(name.clone(), (symbol, value))
                                .is_none();
                            assert!(is_new, "{name} already defined.");
                            self.source_order
                                .push(StatementIdentifier::Definition(name));
                        }
                        PILItem::PublicDeclaration(decl) => {
                            let name = decl.name.clone();
                            self.public_declarations.insert(name.clone(), decl);
                            self.source_order
                                .push(StatementIdentifier::PublicDeclaration(name));
                        }
                        PILItem::Identity(identity) => {
                            let index = self.identities.len();
                            self.source_order.push(StatementIdentifier::Identity(index));
                            self.identities.push(identity)
                        }
                    }
                }
            }
        }
    }

    fn handle_include(&mut self, path: String) {
        let mut dir = self.current_file.parent().unwrap().to_owned();
        dir.push(path);
        self.process_file(&dir);
    }

    fn handle_namespace(&mut self, name: String, degree: ::ast::parsed::Expression<T>) {
        // TODO: the polynomial degree should be handled without going through a field element. This requires having types in Expression
        let degree = ExpressionProcessor::new(self.driver()).process_expression(degree);
        let namespace_degree = evaluator::evaluate_expression(&degree, &self.definitions)
            .unwrap()
            .try_to_number()
            .unwrap()
            .to_degree();
        if let Some(degree) = self.polynomial_degree {
            assert_eq!(
                degree, namespace_degree,
                "all namespaces must have the same degree"
            );
        } else {
            self.polynomial_degree = Some(namespace_degree);
        }
        self.namespace = name;
    }

    fn driver(&self) -> Driver<T> {
        Driver(self)
    }
}

#[derive(Clone, Copy)]
struct Driver<'a, T>(&'a PILAnalyzer<T>);

impl<'a, T: FieldElement> AnalysisDriver<T> for Driver<'a, T> {
    fn resolve_decl(&self, name: &str) -> String {
        if name.starts_with('%') {
            // Constants are not namespaced
            name.to_string()
        } else {
            format!("{}.{name}", self.0.namespace)
        }
    }

    fn resolve_ref(&self, namespace: &Option<String>, name: &str) -> String {
        let definitions = &self.0.definitions;
        if name.starts_with('%') || definitions.contains_key(&name.to_string()) {
            assert!(namespace.is_none());
            // Constants are not namespaced
            name.to_string()
        } else if namespace.is_none() && definitions.contains_key(&format!("Global.{name}")) {
            format!("Global.{name}")
        } else {
            format!("{}.{name}", namespace.as_ref().unwrap_or(&self.0.namespace))
        }
    }

    fn source_position_to_source_ref(&self, pos: usize) -> SourceRef {
        let file = self.0.current_file.file_name().unwrap().to_str().unwrap();
        SourceRef {
            line: parser_util::lines::offset_to_line(pos, &self.0.line_starts),
            file: file.to_string(),
        }
    }

    fn definitions(&self) -> &HashMap<String, (Symbol, Option<FunctionValueDefinition<T>>)> {
        &self.0.definitions
    }
}

#[cfg(test)]
mod test {
    use number::GoldilocksField;
    use test_log::test;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_print_analyzed() {
        // This is rather a test for the Display trait than for the analyzer.
        let input = r#"constant %N = 65536;
    public P = T.pc(2);
namespace Bin(65536);
    col witness bla;
namespace T(65536);
    col fixed first_step = [1] + [0]*;
    col fixed line(i) { i };
    col fixed ops(i) { ((i < 7) && (6 >= !i)) };
    col witness pc;
    col witness XInv;
    col witness XIsZero;
    T.XIsZero = (1 - (T.X * T.XInv));
    (T.XIsZero * T.X) = 0;
    (T.XIsZero * (1 - T.XIsZero)) = 0;
    col witness instr_jmpz;
    col witness instr_jmpz_param_l;
    col witness instr_jmp;
    col witness instr_jmp_param_l;
    col witness instr_dec_CNT;
    col witness instr_assert_zero;
    (T.instr_assert_zero * (T.XIsZero - 1)) = 0;
    col witness X;
    col witness X_const;
    col witness X_read_free;
    col witness A;
    col witness CNT;
    col witness read_X_A;
    col witness read_X_CNT;
    col witness reg_write_X_CNT;
    col witness read_X_pc;
    col witness reg_write_X_A;
    T.X = ((((T.read_X_A * T.A) + (T.read_X_CNT * T.CNT)) + T.X_const) + (T.X_read_free * T.X_free_value));
    T.A' = (((T.first_step' * 0) + (T.reg_write_X_A * T.X)) + ((1 - (T.first_step' + T.reg_write_X_A)) * T.A));
    col witness X_free_value(i) query match T.pc { 0 => ("input", 1), 3 => ("input", (T.CNT(i) + 1)), 7 => ("input", 0), };
    col fixed p_X_const = [0, 0, 0, 0, 0, 0, 0, 0, 0] + [0]*;
    col fixed p_X_read_free = [1, 0, 0, 1, 0, 0, 0, -1, 0] + [0]*;
    col fixed p_read_X_A = [0, 0, 0, 1, 0, 0, 0, 1, 1] + [0]*;
    col fixed p_read_X_CNT = [0, 0, 1, 0, 0, 0, 0, 0, 0] + [0]*;
    col fixed p_read_X_pc = [0, 0, 0, 0, 0, 0, 0, 0, 0] + [0]*;
    col fixed p_reg_write_X_A = [0, 0, 0, 1, 0, 0, 0, 1, 0] + [0]*;
    col fixed p_reg_write_X_CNT = [1, 0, 0, 0, 0, 0, 0, 0, 0] + [0]*;
    { T.pc, T.reg_write_X_A, T.reg_write_X_CNT } in (1 - T.first_step) { T.line, T.p_reg_write_X_A, T.p_reg_write_X_CNT };
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(input, formatted);
    }

    #[test]
    fn intermediate() {
        let input = r#"namespace N(65536);
    col witness x;
    col intermediate = x;
    intermediate = intermediate;
"#;
        let expected = r#"namespace N(65536);
    col witness x;
    col intermediate = N.x;
    N.intermediate = N.intermediate;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }

    #[test]
    fn intermediate_nested() {
        let input = r#"namespace N(65536);
    col witness x;
    col intermediate = x;
    col int2 = intermediate;
    col int3 = int2 + intermediate;
    int3 = 2 * x;
"#;
        let expected = r#"namespace N(65536);
    col witness x;
    col intermediate = N.x;
    col int2 = N.intermediate;
    col int3 = (N.int2 + N.intermediate);
    N.int3 = (2 * N.x);
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }

    #[test]
    fn let_definitions() {
        let input = r#"constant %r = 65536;
namespace N(%r);
    let x;
    let z = 2;
    let t = |i| i + z;
    let other = [1, z];
    let other_fun = |i, j| (i + 7, (|k| k - i));
"#;
        let expected = r#"constant %r = 65536;
namespace N(65536);
    col witness x;
    constant z = 2;
    col fixed t(i) { (i + N.z) };
    let other = [1, N.z];
    let other_fun = |i, j| ((i + 7), |k| (k - i));
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }

    #[test]
    fn reparse_arrays() {
        let input = r#"namespace N(16);
    col witness y[3];
    (N.y[1] - 2) = 0;
    (N.y[2]' - 2) = 0;
    public out = N.y[1](2);
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, input);
    }

    #[test]
    #[should_panic = "Operator - not supported on types"]
    fn no_direct_array_references() {
        let input = r#"namespace N(16);
    col witness y[3];
    (N.y - 2) = 0;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, input);
    }

    #[test]
    #[should_panic = "Tried to access element 3 of array of size 3"]
    fn no_out_of_bounds() {
        let input = r#"namespace N(16);
    col witness y[3];
    (N.y[3] - 2) = 0;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, input);
    }

    #[test]
    fn namespaced_call() {
        let input = r#"namespace Assembly(2);
    col fixed A = [0]*;
    col fixed C(i) { (Assembly.A((i + 2)) + 3) };
    col fixed D(i) { Assembly.C((i + 3)) };
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, input);
    }

    #[test]
    fn if_expr() {
        let input = r#"namespace Assembly(2);
    col fixed A = [0]*;
    col fixed C(i) { if (i < 3) { Assembly.A(i) } else { (i + 9) } };
    col fixed D(i) { if Assembly.C(i) { 3 } else { 2 } };
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, input);
    }

    #[test]
    fn symbolic_functions() {
        let input = r#"namespace N(16);
    let last_row = 15;
    let ISLAST = |i| match i { last_row => 1, _ => 0 };
    let x;
    let y;
    let constrain_equal_expr = |A, B| A - B;
    let on_regular_row = |cond| (1 - ISLAST) * cond;
    on_regular_row(constrain_equal_expr(x', y)) = 0;
    on_regular_row(constrain_equal_expr(y', x + y)) = 0;
    "#;
        let expected = r#"namespace N(16);
    constant last_row = 15;
    col fixed ISLAST(i) { match i { N.last_row => 1, _ => 0, } };
    col witness x;
    col witness y;
    let constrain_equal_expr = |A, B| (A - B);
    col fixed on_regular_row(cond) { ((1 - N.ISLAST) * cond) };
    ((1 - N.ISLAST) * (N.x' - N.y)) = 0;
    ((1 - N.ISLAST) * (N.y' - (N.x + N.y))) = 0;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }

    #[test]
    fn next_op_on_param() {
        let input = r#"namespace N(16);
    let last_row = 15;
    let ISLAST = |i| match i { last_row => 1, _ => 0 };
    let x;
    let y;
    let next_is_seven = |t| t' - 7;
    next_is_seven(y) = 0;
    "#;
        let expected = r#"namespace N(16);
    constant last_row = 15;
    col fixed ISLAST(i) { match i { N.last_row => 1, _ => 0, } };
    col witness x;
    col witness y;
    col fixed next_is_seven(t) { (t' - 7) };
    (N.y' - 7) = 0;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }

    #[test]
    fn fixed_concrete_and_symbolic() {
        let input = r#"namespace N(16);
    let last_row = 15;
    let ISLAST = |i| match i { last_row => 1, _ => 0, };
    let x;
    let y;
    y - ISLAST(3) = 0;
    x - ISLAST = 0;
    "#;
        let expected = r#"namespace N(16);
    constant last_row = 15;
    col fixed ISLAST(i) { match i { N.last_row => 1, _ => 0, } };
    col witness x;
    col witness y;
    (N.y - 0) = 0;
    (N.x - N.ISLAST) = 0;
"#;
        let formatted = process_pil_file_contents::<GoldilocksField>(input).to_string();
        assert_eq!(formatted, expected);
    }
}
