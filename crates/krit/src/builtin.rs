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
    StateGet,
    StatePut,
    StateDelete,
    CheckpointGet,
    CheckpointPut,
    ReplayHttp,
    ReplayAi,
    QueuePublish,
    ObjectGet,
    ObjectPut,
    ObjectDelete,
    DatabaseBeginRead,
    DatabaseBeginWrite,
    DatabaseQuery,
    DatabaseExecute,
    DatabaseCommit,
    DatabaseRollback,
}

impl Builtin {
    pub const ALL: [Self; 31] = [
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
        Self::StateDelete,
        Self::StateGet,
        Self::StatePut,
        Self::CheckpointGet,
        Self::CheckpointPut,
        Self::ReplayAi,
        Self::ReplayHttp,
        Self::QueuePublish,
        Self::ObjectDelete,
        Self::ObjectGet,
        Self::ObjectPut,
        Self::DatabaseBeginRead,
        Self::DatabaseBeginWrite,
        Self::DatabaseCommit,
        Self::DatabaseExecute,
        Self::DatabaseQuery,
        Self::DatabaseRollback,
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
            "state_get" => Some(Self::StateGet),
            "state_put" => Some(Self::StatePut),
            "state_delete" => Some(Self::StateDelete),
            "checkpoint_get" => Some(Self::CheckpointGet),
            "checkpoint_put" => Some(Self::CheckpointPut),
            "replay_http" => Some(Self::ReplayHttp),
            "replay_ai" => Some(Self::ReplayAi),
            "queue_publish" => Some(Self::QueuePublish),
            "object_get" => Some(Self::ObjectGet),
            "object_put" => Some(Self::ObjectPut),
            "object_delete" => Some(Self::ObjectDelete),
            "db_begin_read" => Some(Self::DatabaseBeginRead),
            "db_begin_write" => Some(Self::DatabaseBeginWrite),
            "db_query" => Some(Self::DatabaseQuery),
            "db_execute" => Some(Self::DatabaseExecute),
            "db_commit" => Some(Self::DatabaseCommit),
            "db_rollback" => Some(Self::DatabaseRollback),
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
            Self::StateGet => "state_get",
            Self::StatePut => "state_put",
            Self::StateDelete => "state_delete",
            Self::CheckpointGet => "checkpoint_get",
            Self::CheckpointPut => "checkpoint_put",
            Self::ReplayHttp => "replay_http",
            Self::ReplayAi => "replay_ai",
            Self::QueuePublish => "queue_publish",
            Self::ObjectGet => "object_get",
            Self::ObjectPut => "object_put",
            Self::ObjectDelete => "object_delete",
            Self::DatabaseBeginRead => "db_begin_read",
            Self::DatabaseBeginWrite => "db_begin_write",
            Self::DatabaseQuery => "db_query",
            Self::DatabaseExecute => "db_execute",
            Self::DatabaseCommit => "db_commit",
            Self::DatabaseRollback => "db_rollback",
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
            | Self::LogError
            | Self::StateGet
            | Self::StatePut
            | Self::StateDelete
            | Self::CheckpointGet
            | Self::CheckpointPut
            | Self::ReplayHttp
            | Self::ReplayAi
            | Self::QueuePublish
            | Self::ObjectGet
            | Self::ObjectPut
            | Self::ObjectDelete
            | Self::DatabaseBeginRead
            | Self::DatabaseBeginWrite
            | Self::DatabaseQuery
            | Self::DatabaseExecute
            | Self::DatabaseCommit
            | Self::DatabaseRollback => BuiltinCategory::HostEffect,
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
            Self::StateGet | Self::CheckpointGet => {
                "fn(String, String) -> Result<Option<String>, String> effects {state.transaction}"
            }
            Self::StatePut | Self::CheckpointPut => {
                "fn(String, String, String) -> Result<Unit, String> effects {state.transaction}"
            }
            Self::StateDelete => {
                "fn(String, String) -> Result<Unit, String> effects {state.transaction}"
            }
            Self::ReplayHttp => {
                "fn(String, String, String, HttpRequest) -> Result<HttpResponse, String> effects {state.transaction}"
            }
            Self::ReplayAi => {
                "fn(String, String, String, String) -> Result<String, String> effects {state.transaction}"
            }
            Self::QueuePublish => {
                "fn(String, String) -> Result<String, String> effects {queue.publish}"
            }
            Self::ObjectGet => {
                "fn(String, String) -> Result<Option<String>, String> effects {object.read}"
            }
            Self::ObjectPut => {
                "fn(String, String, String) -> Result<Unit, String> effects {object.write}"
            }
            Self::ObjectDelete => {
                "fn(String, String) -> Result<Unit, String> effects {object.write}"
            }
            Self::DatabaseBeginRead => {
                "fn(String) -> Result<DatabaseTransaction, String> effects {database.read}"
            }
            Self::DatabaseBeginWrite => {
                "fn(String) -> Result<DatabaseTransaction, String> effects {database.write}"
            }
            Self::DatabaseQuery => {
                "fn(DatabaseTransaction, String, List<String>) -> Result<String, String> effects {}"
            }
            Self::DatabaseExecute => {
                "fn(DatabaseTransaction, String, List<String>) -> Result<Int, String> effects {}"
            }
            Self::DatabaseCommit | Self::DatabaseRollback => {
                "fn(DatabaseTransaction) -> Result<Unit, String> effects {}"
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
            Self::StateGet => "Reads one bounded string value from a named durable store.",
            Self::StatePut => "Stages one bounded string value for successful invocation commit.",
            Self::StateDelete => {
                "Stages deletion of one durable key for successful invocation commit."
            }
            Self::CheckpointGet => "Reads one explicit named durable workflow checkpoint.",
            Self::CheckpointPut => {
                "Stages one named workflow checkpoint for successful invocation commit."
            }
            Self::ReplayHttp => {
                "Performs or reuses one completed anonymous HTTP operation under a stable durable identity."
            }
            Self::ReplayAi => {
                "Performs or reuses one completed AI operation under a stable durable identity."
            }
            Self::QueuePublish => {
                "Stages one bounded job for a manifest-granted durable queue and returns its durable identity."
            }
            Self::ObjectGet => {
                "Reads one bounded object from a manifest-granted capability-scoped bucket."
            }
            Self::ObjectPut => "Stages one bounded object write for the successful outcome commit.",
            Self::ObjectDelete => {
                "Stages one bounded object deletion for the successful outcome commit."
            }
            Self::DatabaseBeginRead => {
                "Begins one bounded read transaction on a manifest-granted database. The name must be a direct string literal."
            }
            Self::DatabaseBeginWrite => {
                "Begins one bounded write transaction on a manifest-granted database. The name must be a direct string literal."
            }
            Self::DatabaseQuery => {
                "Runs one host-catalogued parameterized query and returns bounded deterministic JSON rows."
            }
            Self::DatabaseExecute => {
                "Runs one host-catalogued parameterized mutation and returns its affected-row count."
            }
            Self::DatabaseCommit => "Commits and closes one open database transaction.",
            Self::DatabaseRollback => "Rolls back and closes one open database transaction.",
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
