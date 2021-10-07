use crate::error::Error;
use crate::types::Value;
use canadensis_dsdl_parser::Span;

/// Evaluates the unary minus operator `-expr`
pub fn evaluate(inner: Value, span: Span<'_>) -> Result<Value, Error> {
    match inner {
        Value::Rational(_) => Err(span_error!(span, "Can't apply unary ! to a rational")),
        Value::String(_) => Err(span_error!(span, "Can't apply unary ! to a string")),
        Value::Set { .. } => Err(span_error!(span, "Can't apply unary ! to a set")),
        Value::Boolean(value) => Ok(Value::Boolean(!value)),
        Value::Type(_) => Err(span_error!(span, "Can't apply unary ! to a type")),
        Value::Identifier(_) => Err(span_error!(span, "Can't apply unary ! to an identifier")),
    }
}
