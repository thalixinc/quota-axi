//! Structured error type + exit-code mapping (AXI convention: 0 success / 1 operational / 2 usage).

use std::fmt;

#[derive(Debug)]
pub struct Error {
    pub message: String,
    pub code: &'static str,
    /// Usage vs operational family. Usage-family errors exit 2 (a wrong command/input, the
    /// caller's mistake); operational errors exit 1.
    pub usage: bool,
    pub suggestions: Vec<String>,
}

impl Error {
    pub fn operational(message: impl Into<String>, code: &'static str) -> Self {
        Error {
            message: message.into(),
            code,
            usage: false,
            suggestions: Vec::new(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Error {
            message: message.into(),
            code: "VALIDATION_ERROR",
            usage: true,
            suggestions: Vec::new(),
        }
    }

    pub fn with_suggestions(mut self, suggestions: Vec<String>) -> Self {
        self.suggestions = suggestions;
        self
    }

    pub fn exit_code(&self) -> i32 {
        if self.usage {
            2
        } else {
            1
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = std::result::Result<T, E>;
