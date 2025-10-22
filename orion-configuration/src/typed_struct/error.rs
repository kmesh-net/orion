use crate::config::common::GenericError;
use std::fmt;

#[derive(Debug, Clone)]
pub enum TypedStructError {
    DecodeFailed(String),
    
    InvalidTypeUrl {
        expected: String,
        actual: String,
    },
    
    MissingValue,
    
    JsonConversionFailed(String),
    
    UnsupportedFilter {
        type_url: String,
        available: Vec<String>,
    },
    
    InvalidConfiguration {
        type_url: String,
        reason: String,
    },
}

impl fmt::Display for TypedStructError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecodeFailed(msg) => {
                write!(f, "Failed to decode TypedStruct: {}", msg)
            }
            Self::InvalidTypeUrl { expected, actual } => {
                write!(
                    f,
                    "TypedStruct type URL mismatch: expected '{}', got '{}'",
                    expected, actual
                )
            }
            Self::MissingValue => {
                write!(f, "TypedStruct is missing value field")
            }
            Self::JsonConversionFailed(msg) => {
                write!(f, "Failed to convert TypedStruct value to JSON: {}", msg)
            }
            Self::UnsupportedFilter { type_url, available } => {
                write!(
                    f,
                    "Unsupported TypedStruct filter type '{}'. Supported types: {}",
                    type_url,
                    available.join(", ")
                )
            }
            Self::InvalidConfiguration { type_url, reason } => {
                write!(
                    f,
                    "Invalid configuration for TypedStruct filter '{}': {}",
                    type_url, reason
                )
            }
        }
    }
}

impl std::error::Error for TypedStructError {}

impl From<TypedStructError> for GenericError {
    fn from(err: TypedStructError) -> Self {
        GenericError::from_msg(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = TypedStructError::InvalidTypeUrl {
            expected: "type.googleapis.com/test.Config".to_string(),
            actual: "type.googleapis.com/wrong.Config".to_string(),
        };
        
        let display = format!("{}", err);
        assert!(display.contains("expected"));
        assert!(display.contains("test.Config"));
        assert!(display.contains("wrong.Config"));
    }

    #[test]
    fn test_unsupported_filter_error() {
        let err = TypedStructError::UnsupportedFilter {
            type_url: "unknown.filter".to_string(),
            available: vec!["filter1".to_string(), "filter2".to_string()],
        };
        
        let display = format!("{}", err);
        assert!(display.contains("unknown.filter"));
        assert!(display.contains("filter1"));
        assert!(display.contains("filter2"));
    }

    #[test]
    fn test_convert_to_generic_error() {
        let typed_err = TypedStructError::MissingValue;
        let generic_err: GenericError = typed_err.into();
        
        let msg = format!("{:?}", generic_err);
        assert!(msg.contains("missing value"));
    }
}
