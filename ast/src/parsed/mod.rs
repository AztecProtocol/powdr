pub mod asm;
pub mod build;
pub mod display;
pub mod folder;
pub mod utils;
pub mod visitor;

use std::ops;

use number::{DegreeType, FieldElement};

#[derive(Debug, PartialEq, Eq)]
pub struct PILFile<T>(pub Vec<PilStatement<T>>);

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum PilStatement<T> {
    /// File name
    Include(usize, String),
    /// Name of namespace and polynomial degree (constant)
    Namespace(usize, String, Expression<T>),
    LetStatement(usize, String, Option<Expression<T>>),
    PolynomialDefinition(usize, String, Expression<T>),
    PublicDeclaration(
        usize,
        String,
        NamespacedPolynomialReference<T>,
        Expression<T>,
    ),
    PolynomialConstantDeclaration(usize, Vec<PolynomialName<T>>),
    PolynomialConstantDefinition(usize, String, FunctionDefinition<T>),
    PolynomialCommitDeclaration(usize, Vec<PolynomialName<T>>, Option<FunctionDefinition<T>>),
    PolynomialIdentity(usize, Expression<T>),
    PlookupIdentity(
        usize,
        SelectedExpressions<Expression<T>>,
        SelectedExpressions<Expression<T>>,
    ),
    PermutationIdentity(
        usize,
        SelectedExpressions<Expression<T>>,
        SelectedExpressions<Expression<T>>,
    ),
    ConnectIdentity(usize, Vec<Expression<T>>, Vec<Expression<T>>),
    ConstantDefinition(usize, String, Expression<T>),
    MacroDefinition(
        usize,
        String,
        Vec<String>,
        Vec<PilStatement<T>>,
        Option<Expression<T>>,
    ),
    FunctionCall(usize, String, Vec<Expression<T>>),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct SelectedExpressions<Expr> {
    pub selector: Option<Expr>,
    pub expressions: Vec<Expr>,
}

impl<Expr> Default for SelectedExpressions<Expr> {
    fn default() -> Self {
        Self {
            selector: Default::default(),
            expressions: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum Expression<T, Ref = NamespacedPolynomialReference<T>> {
    Reference(Ref),
    PublicReference(String),
    Number(T),
    String(String),
    Tuple(Vec<Expression<T, Ref>>),
    LambdaExpression(LambdaExpression<T, Ref>),
    ArrayLiteral(ArrayLiteral<T, Ref>),
    BinaryOperation(
        Box<Expression<T, Ref>>,
        BinaryOperator,
        Box<Expression<T, Ref>>,
    ),
    UnaryOperation(UnaryOperator, Box<Expression<T, Ref>>),

    FunctionCall(FunctionCall<T, Ref>),
    FreeInput(Box<Expression<T, Ref>>),
    MatchExpression(Box<Expression<T, Ref>>, Vec<MatchArm<T, Ref>>),
}

impl<T, Ref> Expression<T, Ref> {
    pub fn new_binary(left: Self, op: BinaryOperator, right: Self) -> Self {
        Expression::BinaryOperation(Box::new(left), op, Box::new(right))
    }

    /// Visits this expression and all of its sub-expressions and returns true
    /// if `f` returns true on any of them.
    pub fn any(&self, mut f: impl FnMut(&Self) -> bool) -> bool {
        use std::ops::ControlFlow;
        use visitor::ExpressionVisitable;
        self.pre_visit_expressions_return(&mut |e| {
            if f(e) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .is_break()
    }
}

impl<T, Ref> ops::Add for Expression<T, Ref> {
    type Output = Expression<T, Ref>;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new_binary(self, BinaryOperator::Add, rhs)
    }
}

impl<T, Ref> ops::Sub for Expression<T, Ref> {
    type Output = Expression<T, Ref>;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new_binary(self, BinaryOperator::Sub, rhs)
    }
}
impl<T, Ref> ops::Mul for Expression<T, Ref> {
    type Output = Expression<T, Ref>;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::new_binary(self, BinaryOperator::Mul, rhs)
    }
}

impl<T: FieldElement, Ref> std::iter::Sum for Expression<T, Ref> {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.reduce(|a, b| a + b)
            .unwrap_or_else(|| T::zero().into())
    }
}

impl<T: FieldElement, Ref> From<T> for Expression<T, Ref> {
    fn from(value: T) -> Self {
        Expression::Number(value)
    }
}

impl<T> From<NamespacedPolynomialReference<T>> for Expression<T> {
    fn from(value: NamespacedPolynomialReference<T>) -> Self {
        Self::Reference(value)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Default, Clone)]
pub struct PolynomialName<T> {
    pub name: String,
    pub array_size: Option<Expression<T>>,
}

#[derive(Debug, PartialEq, Eq, Default, Clone, PartialOrd, Ord)]
/// A polynomial with an optional namespace
pub struct NamespacedPolynomialReference<T> {
    /// The optional namespace, if `None` then this polynomial inherits the next enclosing namespace, if any
    namespace: Option<String>,
    /// The underlying polynomial
    pol: IndexedPolynomialReference<T>,
}

impl<T> NamespacedPolynomialReference<T> {
    /// Returns the optional namespace of this polynomial
    pub fn namespace(&self) -> &Option<String> {
        &self.namespace
    }

    /// Returns the optional index of the underlying polynomial in its declaration array
    pub fn index(&self) -> &Option<Box<Expression<T>>> {
        self.pol.index()
    }

    /// Returns the name of the declared polynomial or array of polynomials
    pub fn name(&self) -> &str {
        self.pol.name()
    }

    /// Returns a mutable reference to the declared polynomial or array of polynomials
    pub fn name_mut(&mut self) -> &mut String {
        self.pol.name_mut()
    }
}

#[derive(Debug, PartialEq, Eq, Default, Clone, PartialOrd, Ord)]
/// A polynomial with an optional index to support unidimensional arrays of polynomials
pub struct IndexedPolynomialReference<T> {
    /// The optional index, is `Some` iff the declaration of this polynomial is an array
    index: Option<Box<Expression<T>>>,
    /// The underlying polynomial
    pol: PolynomialReference,
}

impl<T> IndexedPolynomialReference<T> {
    /// Return a namespaced polynomial based on this polynomial and an optional namespace
    pub fn with_namespace(self, namespace: Option<String>) -> NamespacedPolynomialReference<T> {
        NamespacedPolynomialReference {
            pol: self,
            namespace,
        }
    }

    /// Returns a mutable reference to the name of the declared polynomial or array of polynomials
    pub fn name_mut(&mut self) -> &mut String {
        self.pol.name_mut()
    }

    /// Returns the optional index of this polynomial in its declaration array
    pub fn index(&self) -> &Option<Box<Expression<T>>> {
        &self.index
    }

    /// Returns the name of the declared polynomial or array of polynomials
    pub fn name(&self) -> &str {
        self.pol.name()
    }

    /// Return a namespaced polynomial based on this polynomial and a namespace
    pub fn namespaced(self, namespace: String) -> NamespacedPolynomialReference<T> {
        self.with_namespace(Some(namespace))
    }

    /// Return a namespaced polynomial based on this polynomial and no namespace, defaulting to the closest enclosing namespace, if any
    pub fn local(self) -> NamespacedPolynomialReference<T> {
        self.with_namespace(None)
    }
}

#[derive(Debug, PartialEq, Eq, Default, Clone, PartialOrd, Ord)]
/// A polynomial or array of polynomials
pub struct PolynomialReference {
    /// The name of this polynomial or array of polynomials
    name: String,
}

impl PolynomialReference {
    /// Returns an indexed polynomial using this polynomial and an optional index
    pub fn with_index<T>(self, index: Option<Expression<T>>) -> IndexedPolynomialReference<T> {
        IndexedPolynomialReference {
            pol: self,
            index: index.map(Box::new),
        }
    }

    /// Returns the name of this polynomial or array of polynomials
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a mutable reference to the name of this polynomial or array of polynomials
    pub fn name_mut(&mut self) -> &mut String {
        &mut self.name
    }

    /// Returns a new polynomial or array of polynomials given a name
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self { name: name.into() }
    }

    /// Returns an indexed polynomial using this polynomial and an index. Used for polynomial array members.
    pub fn indexed<T>(self, index: Expression<T>) -> IndexedPolynomialReference<T> {
        self.with_index(Some(index))
    }

    /// Returns an indexed polynomial using this polynomial and an index. Used for polynomials which are not declared in arrays.
    pub fn single<T>(self) -> IndexedPolynomialReference<T> {
        self.with_index(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LambdaExpression<T, Ref = NamespacedPolynomialReference<T>> {
    pub params: Vec<String>,
    pub body: Box<Expression<T, Ref>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArrayLiteral<T, Ref = NamespacedPolynomialReference<T>> {
    pub items: Vec<Expression<T, Ref>>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum UnaryOperator {
    Plus,
    Minus,
    LogicalNot,
    Next,
}

impl UnaryOperator {
    /// Returns true if the operator is a prefix-operator and false if it is a postfix operator.
    pub fn is_prefix(&self) -> bool {
        match self {
            UnaryOperator::Plus | UnaryOperator::Minus | UnaryOperator::LogicalNot => true,
            UnaryOperator::Next => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    BinaryAnd,
    BinaryXor,
    BinaryOr,
    ShiftLeft,
    ShiftRight,
    LogicalOr,
    LogicalAnd,
    Less,
    LessEqual,
    Equal,
    NotEqual,
    GreaterEqual,
    Greater,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct FunctionCall<T, Ref = NamespacedPolynomialReference<T>> {
    pub id: String,
    pub arguments: Vec<Expression<T, Ref>>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct MatchArm<T, Ref = NamespacedPolynomialReference<T>> {
    pub pattern: MatchPattern<T, Ref>,
    pub value: Expression<T, Ref>,
}

/// A pattern for a match arm. We could extend this in the future.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum MatchPattern<T, Ref = NamespacedPolynomialReference<T>> {
    CatchAll,
    Pattern(Expression<T, Ref>),
}

/// The definition of a function (excluding its name):
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum FunctionDefinition<T> {
    /// Parameter-value-mapping.
    Mapping(Vec<String>, Expression<T>),
    /// Array expression.
    Array(ArrayExpression<T>),
    /// Prover query.
    Query(Vec<String>, Expression<T>),
    /// Expression, for intermediate polynomials
    Expression(Expression<T>),
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub enum ArrayExpression<T> {
    Value(Vec<Expression<T>>),
    RepeatedValue(Vec<Expression<T>>),
    Concat(Box<ArrayExpression<T>>, Box<ArrayExpression<T>>),
}

impl<T: FieldElement> ArrayExpression<T> {
    pub fn value(v: Vec<Expression<T>>) -> Self {
        Self::Value(v)
    }

    pub fn repeated_value(v: Vec<Expression<T>>) -> Self {
        Self::RepeatedValue(v)
    }

    pub fn concat(self, other: Self) -> Self {
        Self::Concat(Box::new(self), Box::new(other))
    }

    fn pad_with(self, pad: Expression<T>) -> Self {
        Self::concat(self, Self::repeated_value(vec![pad]))
    }

    pub fn pad_with_zeroes(self) -> Self {
        self.pad_with(Expression::Number(0.into()))
    }

    fn last(&self) -> Option<&Expression<T>> {
        match self {
            ArrayExpression::Value(v) => v.last(),
            ArrayExpression::RepeatedValue(v) => v.last(),
            ArrayExpression::Concat(_, right) => right.last(),
        }
    }

    // return None if `self` is empty
    pub fn pad_with_last(self) -> Option<Self> {
        self.last().cloned().map(|last| self.pad_with(last))
    }
}

impl<T> ArrayExpression<T> {
    /// solve for `*`
    pub fn solve(&self, degree: DegreeType) -> DegreeType {
        assert!(
            self.number_of_repetitions() <= 1,
            "`*` can be used only once in rhs of array definition"
        );
        let len = self.constant_length();
        assert!(
            len <= degree,
            "Array literal is too large ({len}) for degree ({degree})."
        );
        // Fill up the remaining space with the repeated array
        degree - len
    }

    /// The number of times the `*` operator is used
    fn number_of_repetitions(&self) -> usize {
        match self {
            ArrayExpression::RepeatedValue(_) => 1,
            ArrayExpression::Value(_) => 0,
            ArrayExpression::Concat(left, right) => {
                left.number_of_repetitions() + right.number_of_repetitions()
            }
        }
    }

    /// The combined length of the constant-size parts of the array expression.
    fn constant_length(&self) -> DegreeType {
        match self {
            ArrayExpression::RepeatedValue(_) => 0,
            ArrayExpression::Value(e) => e.len() as DegreeType,
            ArrayExpression::Concat(left, right) => {
                left.constant_length() + right.constant_length()
            }
        }
    }
}
