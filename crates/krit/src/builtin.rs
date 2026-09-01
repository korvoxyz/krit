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
