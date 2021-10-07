use crate::compile::CompileContext;
use crate::compiled::DsdlKind;
use crate::error::Error;
use crate::types::set::Set;
use crate::types::{ExprType, ScalarType, Type, Value};
use canadensis_dsdl_parser::Span;
use num_rational::BigRational;

/// Evaluates the attribute operator `expr.attribute`
pub(crate) fn evaluate(
    cx: &mut CompileContext<'_>,
    lhs: Value,
    rhs: &str,
    span: Span<'_>,
) -> Result<Value, Error> {
    match lhs {
        Value::Set(lhs) => evaluate_set_attr(lhs, rhs, span),
        Value::Type(ty) => evaluate_type_attr(cx, ty, rhs, span),
        _ => Err(span_error!(span, "{} has no attribute {}", lhs.ty(), rhs)),
    }
}

/// Evaluates an attribute of a set
fn evaluate_set_attr(lhs: Set, rhs: &str, span: Span<'_>) -> Result<Value, Error> {
    // Sets have min, max, and count attributes
    match rhs {
        "min" => evaluate_set_min(lhs, span),
        "max" => evaluate_set_max(lhs, span),
        "count" => Ok(Value::Rational(BigRational::from_integer(lhs.len().into()))),
        _ => Err(span_error!(span, "Set does not have a {} attribute", rhs)),
    }
}

fn evaluate_set_min(lhs: Set, span: Span<'_>) -> Result<Value, Error> {
    match lhs.min_value() {
        Some(value) => Ok(value),
        None => match lhs.ty() {
            None => Err(span_error!(
                span,
                "Set does not have a min attribute because it is empty",
            )),
            Some(element_ty) => Err(make_set_min_max_gt_undefined_error("min", element_ty, span)),
        },
    }
}

fn evaluate_set_max(lhs: Set, span: Span<'_>) -> Result<Value, Error> {
    match lhs.max_value() {
        Some(value) => Ok(value),
        None => match lhs.ty() {
            None => Err(span_error!(
                span,
                "Set does not have a min attribute because it is empty",
            )),
            Some(element_ty) => Err(make_set_min_max_gt_undefined_error("max", element_ty, span)),
        },
    }
}

fn make_set_min_max_gt_undefined_error(
    attribute: &str,
    element_ty: ExprType,
    span: Span<'_>,
) -> Error {
    span_error!(span,
            "Set does not have a {} attribute because the < operator is not defined for its element type ({})",
            attribute,
            element_ty)
}

fn evaluate_type_attr(
    cx: &mut CompileContext<'_>,
    ty: Type,
    rhs: &str,
    span: Span<'_>,
) -> Result<Value, Error> {
    // The _bit_length_ special attribute is not part of the specification (v1.0-beta),
    // but pyuavcan implements it and some of the public regulated data types use it.
    match rhs {
        "_bit_length_" => {
            // TODO: Push bit length set ... something ... optimizaion
            let bit_length = ty.bit_length_set(cx, span)?.expand();
            Ok(Value::Set(
                bit_length
                    .into_iter()
                    .map(|length| Value::Rational(BigRational::from_integer(length.into())))
                    .collect::<Result<Set, _>>()
                    .unwrap(),
            ))
        }
        _ => match ty {
            Type::Scalar(ty) => {
                match ty {
                    ScalarType::Versioned(ty) => {
                        // Recursion!
                        // Look up the type that this refers to and check its properties
                        let ty_compiled = cx.get_by_key(&ty)?;

                        match &ty_compiled.kind {
                            DsdlKind::Message { constants, .. } => {
                                // Look up the constant
                                match constants.get(rhs) {
                                    Some(constant) => Ok(constant.value().clone()),
                                    None => Err(span_error!(
                                        span,
                                        "Type {} has no attribute {}",
                                        ty,
                                        rhs
                                    )),
                                }
                            }
                            DsdlKind::Service { .. } => {
                                // A service type can't be named
                                Err(span_error!(
                                    span,
                                    "Type {} has no attributes because it is a service",
                                    ty
                                ))
                            }
                        }
                    }
                    _ => Err(span_error!(span, "Type {} has no attribute {}", ty, rhs)),
                }
            }
            _ => Err(span_error!(span, "Type {} has no attribute {}", ty, rhs)),
        },
    }
}
