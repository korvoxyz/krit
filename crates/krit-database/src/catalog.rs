use rusqlite::Connection;

use crate::error::DatabaseError;

/// Hard bound on statements one logical database may expose.
pub const MAX_CATALOG_STATEMENTS: usize = 64;
/// Hard bound on bound parameters for one statement.
pub const MAX_PARAMETERS: usize = 16;
/// Hard bound on result columns for one query statement.
pub const MAX_RESULT_COLUMNS: usize = 32;
/// Hard bound on a column identifier.
const MAX_COLUMN_NAME_BYTES: usize = 64;
/// Hard bound on the SQL text one catalog entry may carry.
pub(crate) const MAX_STATEMENT_BYTES: usize = 4096;

/// Operation kind an operator declares for a catalog statement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementKind {
    /// Returns rows and must be read-only according to SQLite.
    Query,
    /// Mutates rows, returns an affected-row count, and must not return rows.
    Execute,
}

impl StatementKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Execute => "execute",
        }
    }
}

/// Declared type the host binds a guest-supplied parameter string as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParameterType {
    Text,
    Integer,
}

impl ParameterType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Integer => "integer",
        }
    }
}

/// Validated result column identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnName(String);

impl ColumnName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn for_tests(name: &str) -> Self {
        Self(name.to_owned())
    }
}

/// One validated catalog entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatementDefinition {
    pub kind: StatementKind,
    pub sql: String,
    pub parameters: Vec<ParameterType>,
    pub columns: Vec<ColumnName>,
}

/// SQL leading keywords a catalog statement may never use.
///
/// Schema ownership, attachment, pragmas, vacuuming, and transaction control all
/// belong to the operator outside Krit, never to a catalog entry the guest can
/// name.
const FORBIDDEN_KEYWORDS: [&str; 15] = [
    "alter",
    "analyze",
    "attach",
    "begin",
    "commit",
    "create",
    "detach",
    "drop",
    "end",
    "pragma",
    "reindex",
    "release",
    "rollback",
    "savepoint",
    "vacuum",
];

/// Validates one catalog entry against the live schema of `connection`.
///
/// Preparation happens on the real database so a statement that does not match
/// the operator's schema fails at configuration time rather than at request
/// time.
pub(crate) fn validate_statement(
    connection: &Connection,
    kind: StatementKind,
    sql: &str,
    parameters: &[ParameterType],
    columns: &[String],
    max_columns: usize,
) -> Result<StatementDefinition, DatabaseError> {
    if sql.is_empty() || sql.len() > MAX_STATEMENT_BYTES {
        return Err(DatabaseError::catalog(
            "catalog statement SQL is empty or exceeds its byte bound",
        ));
    }
    if sql.contains('\0') {
        return Err(DatabaseError::catalog(
            "catalog statement SQL contains a NUL byte",
        ));
    }
    if parameters.len() > MAX_PARAMETERS {
        return Err(DatabaseError::catalog(
            "catalog statement declares too many parameters",
        ));
    }
    if columns.len() > max_columns.min(MAX_RESULT_COLUMNS) {
        return Err(DatabaseError::catalog(
            "catalog statement declares too many result columns",
        ));
    }
    reject_forbidden_keyword(sql)?;

    let statement = connection.prepare(sql).map_err(|_| {
        DatabaseError::catalog("catalog statement is not valid against the configured schema")
    })?;
    reject_trailing_statement(sql)?;

    if statement.parameter_count() != parameters.len() {
        return Err(DatabaseError::catalog(
            "catalog statement placeholder count does not match its declared parameters",
        ));
    }
    for index in 1..=statement.parameter_count() {
        let name = statement.parameter_name(index);
        // Only ordinal `?N` placeholders are allowed so binding order is exact.
        if name.is_some_and(|name| !name.starts_with('?')) {
            return Err(DatabaseError::catalog(
                "catalog statement must use ordinal `?N` placeholders",
            ));
        }
    }

    let readonly = statement.readonly();
    let actual_columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    match kind {
        StatementKind::Query => {
            if !readonly {
                return Err(DatabaseError::catalog(
                    "a `query` catalog statement must be read only",
                ));
            }
            if actual_columns.is_empty() {
                return Err(DatabaseError::catalog(
                    "a `query` catalog statement must return at least one column",
                ));
            }
        }
        StatementKind::Execute => {
            if readonly {
                return Err(DatabaseError::catalog(
                    "an `execute` catalog statement must mutate the database",
                ));
            }
            if !actual_columns.is_empty() {
                return Err(DatabaseError::catalog(
                    "an `execute` catalog statement must not return rows",
                ));
            }
        }
    }
    if actual_columns != *columns {
        return Err(DatabaseError::catalog(
            "catalog statement columns do not match its declared result contract",
        ));
    }
    if actual_columns.len() > max_columns.min(MAX_RESULT_COLUMNS) {
        return Err(DatabaseError::catalog(
            "catalog statement returns more columns than the configured bound",
        ));
    }
    let mut validated = Vec::with_capacity(actual_columns.len());
    for column in actual_columns {
        if column.is_empty() || column.len() > MAX_COLUMN_NAME_BYTES {
            return Err(DatabaseError::catalog(
                "catalog statement column name is empty or exceeds its byte bound",
            ));
        }
        validated.push(ColumnName(column));
    }
    drop(statement);
    Ok(StatementDefinition {
        kind,
        sql: sql.to_owned(),
        parameters: parameters.to_vec(),
        columns: validated,
    })
}

/// Rejects an entry whose leading keyword is outside the allowed surface.
fn reject_forbidden_keyword(sql: &str) -> Result<(), DatabaseError> {
    let leading = leading_keyword(sql)?;
    if FORBIDDEN_KEYWORDS.contains(&leading.as_str()) {
        return Err(DatabaseError::catalog(
            "catalog statement uses a forbidden leading SQL keyword",
        ));
    }
    if !matches!(
        leading.as_str(),
        "select" | "with" | "insert" | "update" | "delete" | "replace"
    ) {
        return Err(DatabaseError::catalog(
            "catalog statement must start with SELECT, WITH, INSERT, UPDATE, DELETE, or REPLACE",
        ));
    }
    Ok(())
}

/// Returns the first real SQL token, skipping whitespace and comments.
///
/// A naive scan for the first alphabetic run treats `-- SELECT\nPRAGMA ...` as
/// a `SELECT`, which would let a comment smuggle a forbidden statement past the
/// allow-list. This skips exactly what SQLite's tokenizer skips, so the token
/// returned is the one SQLite will actually execute.
fn leading_keyword(sql: &str) -> Result<String, DatabaseError> {
    let bytes = sql.as_bytes();
    let mut index = skip_ignorable(sql, 0)?;
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
        index += 1;
    }
    if index == start {
        // A quoted identifier, bracket, parenthesis, or punctuation in leading
        // position is never one of the allowed statement keywords.
        return Err(DatabaseError::catalog(
            "catalog statement must start with SELECT, WITH, INSERT, UPDATE, DELETE, or REPLACE",
        ));
    }
    Ok(sql[start..index].to_ascii_lowercase())
}

/// Advances past whitespace and complete SQL comments from `index`.
fn skip_ignorable(sql: &str, mut index: usize) -> Result<usize, DatabaseError> {
    let bytes = sql.as_bytes();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) == Some(&b'-') && bytes.get(index + 1) == Some(&b'-') {
            index = bytes[index..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |position| index + position + 1);
            continue;
        }
        if bytes.get(index) == Some(&b'/') && bytes.get(index + 1) == Some(&b'*') {
            let Some(position) = sql[index + 2..].find("*/") else {
                return Err(DatabaseError::catalog(
                    "catalog statement has an unterminated block comment",
                ));
            };
            index += position + 4;
            continue;
        }
        return Ok(index);
    }
}

/// Rejects catalog SQL that hides a second statement after the first.
///
/// SQLite's own preparation stops at the first statement and silently ignores
/// the remainder, so the catalog scans the text with the same lexical rules and
/// refuses any separator that is not a single optional trailing `;`.
fn reject_trailing_statement(sql: &str) -> Result<(), DatabaseError> {
    let bytes = sql.as_bytes();
    let mut index = 0usize;
    let mut separator = None;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[index];
                index += 1;
                loop {
                    let Some(position) = bytes[index..].iter().position(|byte| *byte == quote)
                    else {
                        return Err(DatabaseError::catalog(
                            "catalog statement has an unterminated quoted literal",
                        ));
                    };
                    index += position + 1;
                    if bytes.get(index) == Some(&quote) {
                        index += 1;
                        continue;
                    }
                    break;
                }
            }
            b'[' => {
                let Some(position) = bytes[index..].iter().position(|byte| *byte == b']') else {
                    return Err(DatabaseError::catalog(
                        "catalog statement has an unterminated bracketed identifier",
                    ));
                };
                index += position + 1;
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = bytes[index..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |position| index + position + 1);
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let Some(position) = sql[index + 2..].find("*/") else {
                    return Err(DatabaseError::catalog(
                        "catalog statement has an unterminated block comment",
                    ));
                };
                index += position + 4;
            }
            b';' => {
                if separator.is_some() {
                    return Err(DatabaseError::catalog(
                        "catalog statement must contain exactly one SQL statement",
                    ));
                }
                separator = Some(index);
                index += 1;
            }
            _ => index += 1,
        }
    }
    if let Some(separator) = separator
        && !sql[separator + 1..].trim().is_empty()
    {
        return Err(DatabaseError::catalog(
            "catalog statement must contain exactly one SQL statement",
        ));
    }
    Ok(())
}

/// Converts a guest-supplied parameter string to its declared SQLite type.
pub(crate) fn bind_value(
    declared: ParameterType,
    value: &str,
) -> Result<rusqlite::types::Value, DatabaseError> {
    match declared {
        ParameterType::Text => Ok(rusqlite::types::Value::Text(value.to_owned())),
        ParameterType::Integer => value
            .parse::<i64>()
            .map(rusqlite::types::Value::Integer)
            .map_err(|_| {
                DatabaseError::limit("database parameter is not a valid signed 64-bit integer")
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_allowed_leading_keywords_are_accepted() {
        for accepted in [
            "SELECT 1",
            "  with x as (select 1) select * from x",
            "insert into t values(1)",
            "UPDATE t SET a = 1",
            "delete from t",
            "REPLACE INTO t VALUES(1)",
        ] {
            reject_forbidden_keyword(accepted).expect("statement should be accepted");
        }
        for rejected in [
            "PRAGMA journal_mode = WAL",
            "attach database 'x' as y",
            "DETACH y",
            "VACUUM",
            "BEGIN",
            "commit",
            "ROLLBACK",
            "savepoint s",
            "RELEASE s",
            "create table t(a)",
            "DROP TABLE t",
            "alter table t rename to u",
            "reindex",
            "ANALYZE",
            "explain select 1",
            "",
        ] {
            reject_forbidden_keyword(rejected).expect_err("statement should be rejected");
        }
    }

    #[test]
    fn comments_cannot_disguise_the_real_leading_keyword() {
        // A scan for the first alphabetic run would read `SELECT` out of the
        // comment and admit the statement that follows it.
        for smuggled in [
            "-- SELECT\nPRAGMA journal_mode = WAL",
            "-- INSERT\nATTACH DATABASE 'other.db' AS other",
            "/* INSERT */ DROP TABLE users",
            "/* select */ VACUUM",
            "  -- delete\n  -- update\n  BEGIN IMMEDIATE",
            "/* with */\n/* select */\nCREATE TABLE t(a)",
            "-- replace\nDETACH other",
            "/* SELECT */ ALTER TABLE users RENAME TO people",
            "-- select\nreindex",
            "/*select*/savepoint s",
        ] {
            let error = reject_forbidden_keyword(smuggled)
                .expect_err("a comment must never disguise the leading keyword");
            assert_eq!(error.kind(), crate::DatabaseErrorKind::Catalog);
        }
    }

    #[test]
    fn benign_comments_before_an_allowed_statement_are_accepted() {
        for accepted in [
            "-- count every user\nSELECT COUNT(*) FROM users",
            "/* audited 2026-01-01 */ INSERT INTO t VALUES(1)",
            "/* one */ /* two */\n-- three\nUPDATE t SET a = 1",
            "\n\n   -- leading blank lines\n   delete from t",
        ] {
            reject_forbidden_keyword(accepted).expect("a benign comment should be accepted");
        }
    }

    #[test]
    fn a_leading_quote_or_bracket_is_never_a_statement_keyword() {
        for rejected in [
            "\"select\" 1",
            "[select] 1",
            "`select` 1",
            "(SELECT 1)",
            "/* unterminated",
        ] {
            reject_forbidden_keyword(rejected).expect_err("statement should be rejected");
        }
    }

    #[test]
    fn only_one_statement_survives_the_separator_scan() {
        for accepted in [
            "SELECT 1",
            "SELECT 1;",
            "SELECT 1;   ",
            "SELECT ';' AS marker",
            "SELECT \"a;b\" FROM t",
            "SELECT [a;b] FROM t",
            "SELECT 1 -- trailing ; comment",
            "SELECT 1 /* inner ; comment */",
        ] {
            reject_trailing_statement(accepted)
                .unwrap_or_else(|error| panic!("`{accepted}` should be accepted: {error}"));
        }
        for rejected in [
            "SELECT 1; DROP TABLE users",
            "SELECT 1;;",
            "INSERT INTO t VALUES(1); INSERT INTO t VALUES(2)",
            "SELECT 'unterminated",
            "SELECT 1 /* unterminated",
            "SELECT [unterminated",
        ] {
            reject_trailing_statement(rejected)
                .expect_err(&format!("`{rejected}` should be rejected"));
        }
    }

    #[test]
    fn declared_parameter_types_convert_or_fail_closed() {
        assert_eq!(
            bind_value(ParameterType::Integer, "42").unwrap(),
            rusqlite::types::Value::Integer(42)
        );
        assert_eq!(
            bind_value(ParameterType::Text, "42").unwrap(),
            rusqlite::types::Value::Text("42".to_owned())
        );
        assert!(bind_value(ParameterType::Integer, "4.2").is_err());
        assert!(bind_value(ParameterType::Integer, "").is_err());
    }
}
