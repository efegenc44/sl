use std::collections::{HashMap, HashSet};

use crate::parser::{Branch, Constructor, Expr, Pattern, TopLevel, TypeExpr};

pub struct TypeChecker {
    types: HashSet<String>,
    ctx: HashMap<String, Type>,
    locals: Vec<(String, Type)>
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            types: HashSet::new(),
            ctx: HashMap::new(),
            locals: vec![],
        }
    }

    fn type_expr(&self, type_expr: &TypeExpr) -> Type {
        match type_expr {
            TypeExpr::Word(word) => Type::Basic(word.clone()),
            TypeExpr::Quotation { inputs, outputs } => Type::Quotation {
                inputs: inputs.iter().map(|ty| self.type_expr(ty)).collect(),
                outputs: outputs.iter().map(|ty| self.type_expr(ty)).collect()
            },
        }
    }

    fn resolve_word(&self, word: &str) -> TypeCheckResult<Type> {
        match self.locals.iter().rev().find(|(name, _)| name == word) {
            Some((_, ty)) => Ok(ty.clone()),
            None => match self.ctx.get(word) {
                Some(ty) => Ok(ty.clone()),
                None => Err(TypeCheckError::UnboundSymbol),
            }
        }
    }

    fn collect_types(&mut self, top_levels: &[TopLevel]) -> TypeCheckResult<()> {
        for top_level in top_levels {
            if let TopLevel::Data { name, .. } = top_level {
                if !self.types.insert(name.clone()) {
                    return Err(TypeCheckError::TypeAlreadyDefined);
                }
            }
        }
        Ok(())
    }

    fn collect_constructors(&mut self, top_levels: &[TopLevel]) -> TypeCheckResult<()> {
        for top_level in top_levels {
            if let TopLevel::Data { name: type_name, constructors } = top_level {
                for Constructor { name, argument_types } in constructors {
                    let inputs = argument_types
                        .iter()
                        .map(|argument_type| self.type_expr(argument_type))
                        .collect();
                    if self.ctx.insert(name.clone(), Type::Function {
                        inputs, outputs: vec![Type::Basic(type_name.clone())],
                    }).is_some() {
                        return Err(TypeCheckError::SymbolAlreadyDefined)
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_defs(&mut self, top_levels: &[TopLevel]) -> TypeCheckResult<()> {
        for top_level in top_levels {
            if let TopLevel::Def { name, inputs, outputs, branches: _ } = top_level {
                let ty = Type::Function {
                    inputs: inputs.iter().map(|ty| self.type_expr(ty)).collect(),
                    outputs: outputs.iter().map(|ty| self.type_expr(ty)).collect()
                };
                if self.ctx.insert(name.clone(), ty).is_some() {
                    return Err(TypeCheckError::SymbolAlreadyDefined)
                }
            }
        }
        Ok(())
    }

    fn pattern_fits(&self, input: &Type, pattern: &Pattern) -> bool {
        match (input, pattern) {
            (input_type, Pattern::Constructor { name, arguments }) => {
                let Some(Type::Function { inputs, outputs }) = self.ctx.get(name) else {
                    return false;
                };

                let [output_type] = &outputs[..] else {
                    unreachable!()
                };

                if output_type != input_type {
                    return false;
                }

                if inputs.len() != arguments.len() {
                    return false;
                }

                if !inputs.iter().zip(arguments)
                    .all(|(input, pattern)| self.pattern_fits(input, pattern)) {
                    return false;
                }
                true
            },
            (_, Pattern::All(_)) => true,
        }
    }

    fn define_pattern_locals(&mut self, input: Type, pattern: Pattern) {
        match pattern {
            Pattern::All(name) => {
                self.locals.push((name, input));
            }
            Pattern::Constructor { name, arguments } => {
                let Some(Type::Function { inputs, outputs: _ }) = self.ctx.get(&name) else {
                    unreachable!();
                };

                for (input, pattern) in inputs.clone().into_iter().zip(arguments) {
                    self.define_pattern_locals(input, pattern);
                }
            },
        }
    }

    fn type_check_expr(&self, expr: &Expr, stack: &mut Vec<Type>) -> TypeCheckResult<()> {
        match expr {
            Expr::Word(word) => {
                match self.resolve_word(word)? {
                    ty@Type::Basic(_) => stack.push(ty),
                    ty@Type::Quotation { .. } => stack.push(ty),
                    Type::Function { inputs, outputs } => {
                        if inputs.len() > stack.len() {
                            return Err(TypeCheckError::TypeMismatch);
                        }

                        if stack[stack.len() - inputs.len()..] != inputs {
                            return Err(TypeCheckError::TypeMismatch);
                        }

                        stack.truncate(stack.len() - inputs.len());
                        stack.extend(outputs);
                    },
                }
            },
            Expr::Quotation { inputs, quotation } => {
                let inputs: Vec<_> = inputs.iter().map(|ty| self.type_expr(ty)).collect();
                let mut outputs = inputs.clone();
                for expr in quotation {
                    self.type_check_expr(expr, &mut outputs)?;
                }

                stack.push(Type::Quotation { inputs, outputs })
            },
            Expr::Unquote => {
                let Some(Type::Quotation { inputs, outputs }) = stack.pop() else {
                    return Err(TypeCheckError::TypeMismatch)
                };

                if stack[stack.len() - inputs.len()..] != inputs {
                    return Err(TypeCheckError::TypeMismatch);
                }

                stack.truncate(stack.len() - inputs.len());
                stack.extend(outputs);
            },
        }
        Ok(())
    }

    fn type_check_defs(&mut self, top_levels: &[TopLevel]) -> TypeCheckResult<()> {
        for top_level in top_levels {
            if let TopLevel::Def { name, inputs: _, outputs: _, branches } = top_level {
                let (inputs, outputs) = match self.ctx.get(name).unwrap().clone() {
                    ty@Type::Basic(_) => (vec![], vec![ty]),
                    ty@Type::Quotation { .. } => (vec![], vec![ty]),
                    Type::Function { inputs, outputs } => (inputs, outputs),
                };

                for Branch { patterns, body } in branches {
                    let mut inputs = inputs.clone();
                    if inputs.len() < patterns.len() {
                        return Err(TypeCheckError::TypeMismatch);
                    }

                    let leftover = inputs.split_off(patterns.len());
                    if !inputs.iter().zip(patterns)
                        .all(|(input, pattern)| self.pattern_fits(input, pattern)) {
                        return Err(TypeCheckError::TypeMismatch)
                    }

                    let locals_len = self.locals.len();
                    for (input, pattern) in inputs.iter().zip(patterns) {
                        self.define_pattern_locals(input.clone(), pattern.clone());
                    }

                    let mut stack = leftover;
                    for expr in body {
                        self.type_check_expr(expr, &mut stack)?;
                    }
                    self.locals.truncate(locals_len);

                    if outputs != stack {
                        return Err(TypeCheckError::TypeMismatch);
                    }
                }
            }
        }
        Ok(())
    }

    pub fn type_check(&mut self, top_levels: &[TopLevel]) -> TypeCheckResult<()> {
        self.collect_types(top_levels)?;
        self.collect_constructors(top_levels)?;
        self.collect_defs(top_levels)?;
        self.type_check_defs(top_levels)
    }
}

type TypeCheckResult<T> = Result<T, TypeCheckError>;
#[derive(Debug)]
pub enum TypeCheckError {
    TypeAlreadyDefined,
    SymbolAlreadyDefined,
    TypeMismatch,
    UnboundSymbol,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Basic(String),
    Function {
        inputs: Vec<Type>,
        outputs: Vec<Type>,
    },
    Quotation {
        inputs: Vec<Type>,
        outputs: Vec<Type>,
    },
}

