use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFile {
    pub variables: Vec<InPlaceVariable>,
    pub requests: Vec<RequestBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InPlaceVariable {
    pub name: String,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestBlock {
    pub name: Option<String>,
    pub method: RequestMethod,
    pub url: String,
    pub headers: Vec<Header>,
    pub body: Option<RequestBody>,
    pub options: RequestOptions,
    #[serde(default)]
    pub assertions: Vec<ResponseAssertion>,
    pub response_redirect: Option<ResponseRedirect>,
    pub range: SourceRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseAssertion {
    Status {
        codes: Vec<u16>,
        line: usize,
    },
    Header {
        name: String,
        substring: String,
        line: usize,
    },
    JsonValue {
        pointer: String,
        expected: String,
        line: usize,
    },
}

impl ResponseAssertion {
    pub fn source_line(&self) -> usize {
        match self {
            Self::Status { line, .. }
            | Self::Header { line, .. }
            | Self::JsonValue { line, .. } => *line,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestOptions {
    pub timeout_ms: Option<u64>,
    pub connection_timeout_ms: Option<u64>,
    pub no_redirect: bool,
    #[serde(default)]
    pub fragment_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestBody {
    Inline(String),
    FromFile { path: String },
}

impl RequestBody {
    pub fn as_inline(&self) -> Option<&str> {
        match self {
            Self::Inline(text) => Some(text),
            Self::FromFile { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseRedirect {
    pub path: String,
    pub force_overwrite: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    GraphQl,
}

impl RequestMethod {
    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "PATCH" => Some(Self::Patch),
            "DELETE" => Some(Self::Delete),
            "HEAD" => Some(Self::Head),
            "OPTIONS" => Some(Self::Options),
            "GRAPHQL" => Some(Self::GraphQl),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::GraphQl => "GRAPHQL",
        }
    }

    pub fn http_method(&self) -> &'static str {
        match self {
            Self::GraphQl => "POST",
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
        }
    }
}

impl fmt::Display for RequestMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRange {
    pub start_line: usize,
    pub end_line: usize,
}
