use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Builtin {
    AiInvoke,
    Print,
    Println,
    Some,
    None,
    Ok,
    Err,
    JsonEncode,
    JsonDecode,
    ConfigString,
    Secret,
    HttpRequest,
    LogInfo,
    LogError,
}

impl Builtin {
    pub const ALL: [Self; 14] = [
        Self::AiInvoke,
        Self::ConfigString,
        Self::Err,
        Self::HttpRequest,
        Self::JsonDecode,
        Self::JsonEncode,
        Self::LogError,
        Self::LogInfo,
        Self::None,
        Self::Ok,
        Self::Print,
        Self::Println,
        Self::Secret,
        Self::Some,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ai_invoke" => Some(Self::AiInvoke),
            "print" => Some(Self::Print),
            "println" => Some(Self::Println),
            "Some" => Some(Self::Some),
            "None" => Some(Self::None),
            "Ok" => Some(Self::Ok),
            "Err" => Some(Self::Err),
            "json_encode" => Some(Self::JsonEncode),
            "json_decode" => Some(Self::JsonDecode),
            "config_string" => Some(Self::ConfigString),
            "secret" => Some(Self::Secret),
            "http_request" => Some(Self::HttpRequest),
            "log_info" => Some(Self::LogInfo),
            "log_error" => Some(Self::LogError),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AiInvoke => "ai_invoke",
            Self::Print => "print",
            Self::Println => "println",
            Self::Some => "Some",
            Self::None => "None",
            Self::Ok => "Ok",
            Self::Err => "Err",
            Self::JsonEncode => "json_encode",
            Self::JsonDecode => "json_decode",
            Self::ConfigString => "config_string",
            Self::Secret => "secret",
            Self::HttpRequest => "http_request",
            Self::LogInfo => "log_info",
            Self::LogError => "log_error",
        }
    }

    pub const fn category(self) -> BuiltinCategory {
        match self {
            Self::AiInvoke | Self::Print | Self::Println => BuiltinCategory::HostEffect,
            Self::Some | Self::None | Self::Ok | Self::Err => BuiltinCategory::Constructor,
            Self::JsonEncode | Self::JsonDecode => BuiltinCategory::Conversion,
            Self::ConfigString
            | Self::Secret
            | Self::HttpRequest
            | Self::LogInfo
            | Self::LogError => BuiltinCategory::HostEffect,
        }
    }

    pub const fn signature(self) -> &'static str {
        match self {
            Self::AiInvoke => "fn(String, String) -> Result<String, String> effects {ai.invoke}",
            Self::Print | Self::Println => "fn('a) -> Unit effects {io.stdout}",
            Self::Some => "fn('a) -> Option<'a> effects {}",
            Self::None => "Option<'a>",
            Self::Ok => "fn('a) -> Result<'a, 'b> effects {}",
            Self::Err => "fn('b) -> Result<'a, 'b> effects {}",
            Self::JsonEncode => "fn('a) -> String effects {}",
            Self::JsonDecode => "fn(String) -> 'a effects {}",
            Self::ConfigString => "fn(String) -> Result<String, String> effects {config.read}",
            Self::Secret => "fn(String) -> Result<Secret, String> effects {secret.read}",
            Self::HttpRequest => {
                "fn(String, HttpRequest, Option<Secret>) -> Result<HttpResponse, String> effects {http.request}"
            }
            Self::LogInfo | Self::LogError => {
                "fn(String, List<LogField>) -> Result<Unit, String> effects {observe.log}"
            }
        }
    }

    pub const fn documentation(self) -> &'static str {
        match self {
            Self::AiInvoke => {
                "Invokes a manifest-granted provider-neutral AI adapter. The adapter name must be a direct string literal."
            }
            Self::Print => {
                "Writes one value to bounded standard output without a trailing newline."
            }
            Self::Println => "Writes one value to bounded standard output followed by a newline.",
            Self::Some => "Constructs the present case of Option.",
            Self::None => "The absent case of Option.",
            Self::Ok => "Constructs the success case of Result.",
            Self::Err => "Constructs the error case of Result.",
            Self::JsonEncode => "Encodes a statically JSON-compatible value deterministically.",
            Self::JsonDecode => {
                "Decodes JSON into the type required by the surrounding expression."
            }
            Self::ConfigString => {
                "Reads a manifest-granted configuration key. The key must be a direct string literal."
            }
            Self::Secret => {
                "Acquires an opaque manifest-granted secret handle. The name must be a direct string literal."
            }
            Self::HttpRequest => {
                "Performs a bounded request to a manifest-granted exact origin. The origin must be a direct normalized string literal."
            }
            Self::LogInfo => "Emits a bounded structured informational event.",
            Self::LogError => "Emits a bounded structured error event.",
        }
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BuiltinCategory {
    HostEffect,
    Constructor,
    Conversion,
}

impl BuiltinCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HostEffect => "host-effect",
            Self::Constructor => "constructor",
            Self::Conversion => "conversion",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn public_builtin_facts_cover_each_reserved_builtin_once() {
        let names = Builtin::ALL
            .into_iter()
            .map(Builtin::as_str)
            .collect::<BTreeSet<_>>();

        assert_eq!(names.len(), Builtin::ALL.len());
        for builtin in Builtin::ALL {
            assert_eq!(Builtin::from_name(builtin.as_str()), Some(builtin));
            assert!(!builtin.signature().is_empty());
            assert!(!builtin.documentation().is_empty());
        }
    }
}
