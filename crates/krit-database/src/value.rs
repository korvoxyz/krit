use rusqlite::types::ValueRef;

use crate::{ColumnName, error::DatabaseError};

/// Hard bound on one encoded result document.
pub const MAX_RESULT_BYTES: usize = 256 * 1024;

/// Bytes reserved for the `]}` document terminator.
const TERMINATOR_BYTES: usize = 2;

/// Incremental encoder for a bounded result set.
///
/// Protocol 1 has no richer typed row value in the guest language, so query
/// results are bounded JSON text with a fixed shape:
///
/// ```json
/// {"columns":["id","name"],"rows":[[1,"alice"]]}
/// ```
///
/// Column order follows the statement, row order follows the query, and only
/// INTEGER, TEXT, and NULL are representable. REAL and BLOB values fail closed
/// rather than being rendered with an implementation-defined spelling.
///
/// The encoder is deliberately incremental. A caller appends one value at a
/// time while stepping the SQLite statement, and every append is budget-checked
/// *before* any byte is copied. Native memory is therefore bounded by
/// `max_bytes` plus one in-flight column value, regardless of how many rows or
/// how much text the query would otherwise produce.
pub(crate) struct RowEncoder {
    buffer: String,
    budget: usize,
    columns: usize,
    rows: usize,
    max_rows: usize,
    row_open: bool,
    values_in_row: usize,
}

impl RowEncoder {
    pub(crate) fn new(
        columns: &[ColumnName],
        max_bytes: usize,
        max_rows: usize,
    ) -> Result<Self, DatabaseError> {
        let budget = max_bytes.min(MAX_RESULT_BYTES);
        let mut encoder = Self {
            buffer: String::new(),
            budget,
            columns: columns.len(),
            rows: 0,
            max_rows,
            row_open: false,
            values_in_row: 0,
        };
        encoder.push_literal("{\"columns\":[")?;
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                encoder.push_literal(",")?;
            }
            encoder.push_json_string(column.as_str())?;
        }
        encoder.push_literal("],\"rows\":[")?;
        Ok(encoder)
    }

    /// Opens one row, enforcing the configured row bound before any value is
    /// read out of SQLite.
    pub(crate) fn begin_row(&mut self) -> Result<(), DatabaseError> {
        if self.rows >= self.max_rows {
            return Err(DatabaseError::limit(
                "database query returned more rows than its configured bound",
            ));
        }
        if self.rows > 0 {
            self.push_literal(",")?;
        }
        self.push_literal("[")?;
        self.row_open = true;
        self.values_in_row = 0;
        Ok(())
    }

    /// Appends one column value directly from SQLite without materialising an
    /// owned copy first.
    pub(crate) fn push_column(&mut self, value: ValueRef<'_>) -> Result<(), DatabaseError> {
        if self.values_in_row > 0 {
            self.push_literal(",")?;
        }
        self.values_in_row += 1;
        match value {
            ValueRef::Null => self.push_literal("null"),
            ValueRef::Integer(value) => {
                let digits = Digits::from_i64(value);
                self.push_literal(digits.as_str())
            }
            ValueRef::Text(bytes) => {
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| DatabaseError::limit("database text value is not valid UTF-8"))?;
                self.push_json_string(text)
            }
            ValueRef::Real(_) => Err(DatabaseError::limit(
                "database REAL values are not representable in protocol 1",
            )),
            ValueRef::Blob(_) => Err(DatabaseError::limit(
                "database BLOB values are not representable in protocol 1",
            )),
        }
    }

    pub(crate) fn end_row(&mut self) -> Result<(), DatabaseError> {
        if self.values_in_row != self.columns {
            return Err(DatabaseError::limit(
                "database row column count does not match the catalog statement",
            ));
        }
        self.push_literal("]")?;
        self.row_open = false;
        self.rows += 1;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<String, DatabaseError> {
        if self.row_open {
            return Err(DatabaseError::limit(
                "database result encoding ended inside a row",
            ));
        }
        // `reserve` always keeps room for this terminator.
        self.buffer.push_str("]}");
        Ok(self.buffer)
    }

    /// Confirms an append, plus the document terminator, still fits the budget.
    fn reserve(&self, additional: usize) -> Result<(), DatabaseError> {
        let required = self
            .buffer
            .len()
            .checked_add(additional)
            .and_then(|length| length.checked_add(TERMINATOR_BYTES))
            .ok_or_else(|| DatabaseError::limit("database result length overflowed"))?;
        if required > self.budget {
            return Err(DatabaseError::limit(
                "database result exceeds its configured byte bound",
            ));
        }
        Ok(())
    }

    fn push_literal(&mut self, text: &str) -> Result<(), DatabaseError> {
        self.reserve(text.len())?;
        self.buffer.push_str(text);
        Ok(())
    }

    /// Escapes one string using the same rules as the language's `json_encode`.
    ///
    /// Each escape is budget-checked before it is written, so an oversized
    /// value stops the encoding at the bound instead of allocating the whole
    /// escaped form first.
    fn push_json_string(&mut self, value: &str) -> Result<(), DatabaseError> {
        self.push_literal("\"")?;
        for character in value.chars() {
            match character {
                '"' => self.push_literal("\\\"")?,
                '\\' => self.push_literal("\\\\")?,
                '\n' => self.push_literal("\\n")?,
                '\r' => self.push_literal("\\r")?,
                '\t' => self.push_literal("\\t")?,
                character if (character as u32) < 0x20 => {
                    let mut escape = [0u8; 6];
                    let encoded = unicode_escape(character as u32, &mut escape);
                    self.push_literal(encoded)?;
                }
                character => {
                    let mut buffer = [0u8; 4];
                    self.push_literal(character.encode_utf8(&mut buffer))?;
                }
            }
        }
        self.push_literal("\"")
    }

    #[cfg(test)]
    fn buffered_bytes(&self) -> usize {
        self.buffer.len()
    }
}

/// Renders `\u00XX` for one control character without allocating.
fn unicode_escape(code: u32, buffer: &mut [u8; 6]) -> &str {
    const HEX: [u8; 16] = *b"0123456789abcdef";
    buffer[0] = b'\\';
    buffer[1] = b'u';
    buffer[2] = HEX[((code >> 12) & 0xf) as usize];
    buffer[3] = HEX[((code >> 8) & 0xf) as usize];
    buffer[4] = HEX[((code >> 4) & 0xf) as usize];
    buffer[5] = HEX[(code & 0xf) as usize];
    // Every byte written above is ASCII.
    std::str::from_utf8(buffer).unwrap_or("\\u0000")
}

/// Fixed-capacity ASCII rendering of one `i64`, avoiding a heap allocation per
/// value in a large result set.
struct Digits {
    bytes: [u8; 20],
    length: usize,
}

impl Digits {
    fn from_i64(value: i64) -> Self {
        let mut digits = Self {
            bytes: [0; 20],
            length: 0,
        };
        // `unsigned_abs` keeps `i64::MIN` representable.
        let mut magnitude = value.unsigned_abs();
        let mut scratch = [0u8; 20];
        let mut length = 0usize;
        loop {
            scratch[length] = b'0' + u8::try_from(magnitude % 10).unwrap_or(0);
            length += 1;
            magnitude /= 10;
            if magnitude == 0 {
                break;
            }
        }
        if value < 0 {
            digits.push(b'-');
        }
        while length > 0 {
            length -= 1;
            digits.push(scratch[length]);
        }
        digits
    }

    fn push(&mut self, byte: u8) {
        if self.length < self.bytes.len() {
            self.bytes[self.length] = byte;
            self.length += 1;
        }
    }

    fn as_str(&self) -> &str {
        // Every byte pushed above is ASCII.
        std::str::from_utf8(&self.bytes[..self.length]).unwrap_or("0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> ColumnName {
        ColumnName::for_tests(name)
    }

    fn encode(
        columns: &[ColumnName],
        rows: &[Vec<ValueRef<'_>>],
        max_bytes: usize,
        max_rows: usize,
    ) -> Result<String, DatabaseError> {
        let mut encoder = RowEncoder::new(columns, max_bytes, max_rows)?;
        for row in rows {
            encoder.begin_row()?;
            for value in row {
                encoder.push_column(*value)?;
            }
            encoder.end_row()?;
        }
        encoder.finish()
    }

    #[test]
    fn rows_encode_deterministically_in_statement_order() {
        let columns = [column("id"), column("name")];
        let rows = vec![
            vec![ValueRef::Integer(1), ValueRef::Text(b"alice")],
            vec![ValueRef::Integer(2), ValueRef::Null],
        ];

        let encoded = encode(&columns, &rows, MAX_RESULT_BYTES, 16).unwrap();

        assert_eq!(
            encoded,
            "{\"columns\":[\"id\",\"name\"],\"rows\":[[1,\"alice\"],[2,null]]}"
        );
        assert_eq!(
            encode(&columns, &rows, MAX_RESULT_BYTES, 16).unwrap(),
            encoded
        );
        assert!(serde_json::from_str::<serde_json::Value>(&encoded).is_ok());
    }

    #[test]
    fn control_characters_and_quotes_are_escaped() {
        let columns = [column("value")];
        let rows = vec![vec![ValueRef::Text("a\"b\\c\nd\u{1}".as_bytes())]];

        let encoded = encode(&columns, &rows, MAX_RESULT_BYTES, 16).unwrap();

        assert_eq!(
            encoded,
            "{\"columns\":[\"value\"],\"rows\":[[\"a\\\"b\\\\c\\nd\\u0001\"]]}"
        );
        assert!(serde_json::from_str::<serde_json::Value>(&encoded).is_ok());
    }

    #[test]
    fn extreme_integers_render_without_allocation() {
        let columns = [column("value")];
        let rows = vec![
            vec![ValueRef::Integer(i64::MIN)],
            vec![ValueRef::Integer(i64::MAX)],
            vec![ValueRef::Integer(0)],
        ];

        let encoded = encode(&columns, &rows, MAX_RESULT_BYTES, 16).unwrap();

        assert_eq!(
            encoded,
            format!(
                "{{\"columns\":[\"value\"],\"rows\":[[{}],[{}],[0]]}}",
                i64::MIN,
                i64::MAX
            )
        );
    }

    #[test]
    fn oversized_results_fail_closed() {
        let columns = [column("value")];
        let wide = b"x".repeat(64);
        let rows = vec![vec![ValueRef::Text(&wide)]];

        assert!(encode(&columns, &rows, 16, 16).is_err());
    }

    #[test]
    fn the_buffer_never_grows_past_the_budget_for_a_huge_value() {
        let columns = [column("value")];
        // Each quote escapes to two bytes, so an encoder that materialised the
        // escaped form first would allocate two megabytes before noticing.
        let giant = "\"".repeat(1024 * 1024);
        let mut encoder = RowEncoder::new(&columns, 512, 16).unwrap();
        encoder.begin_row().unwrap();

        let outcome = encoder.push_column(ValueRef::Text(giant.as_bytes()));

        assert!(outcome.is_err());
        assert!(
            encoder.buffered_bytes() <= 512,
            "buffer grew to {} bytes",
            encoder.buffered_bytes()
        );
    }

    #[test]
    fn escaping_is_accounted_for_before_the_budget_is_exceeded() {
        let columns = [column("v")];
        let quotes = b"\"".repeat(20);
        let rows = vec![vec![ValueRef::Text(&quotes)]];

        // Twenty quotes escape to forty bytes: the raw form fits the smaller
        // budget but the escaped form does not.
        assert!(encode(&columns, &rows, 44, 16).is_err());
        assert!(encode(&columns, &rows, 128, 16).is_ok());
    }

    #[test]
    fn row_bounds_stop_encoding_immediately() {
        let columns = [column("v")];
        let rows = vec![
            vec![ValueRef::Integer(1)],
            vec![ValueRef::Integer(2)],
            vec![ValueRef::Integer(3)],
        ];

        assert!(encode(&columns, &rows, MAX_RESULT_BYTES, 2).is_err());
        assert!(encode(&columns, &rows, MAX_RESULT_BYTES, 3).is_ok());
    }

    #[test]
    fn unrepresentable_column_types_fail_closed() {
        let columns = [column("v")];

        for value in [ValueRef::Real(1.5), ValueRef::Blob(&[1, 2])] {
            assert!(encode(&columns, &[vec![value]], MAX_RESULT_BYTES, 16).is_err());
        }
        assert!(
            encode(
                &columns,
                &[vec![ValueRef::Text(&[0xff])]],
                MAX_RESULT_BYTES,
                16
            )
            .is_err()
        );
    }
}
