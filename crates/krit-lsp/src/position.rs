use krit::Span;
use lsp_types::{Position, Range};

#[derive(Clone, Debug)]
pub(crate) struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self { starts }
    }

    pub(crate) fn position(&self, text: &str, byte: usize) -> Position {
        let mut byte = byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte = byte.saturating_sub(1);
        }
        let line = self.starts.partition_point(|start| *start <= byte) - 1;
        let character = text[self.starts[line]..byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Position::new(line as u32, character as u32)
    }

    pub(crate) fn offset(&self, text: &str, position: Position) -> Result<usize, String> {
        let line = position.line as usize;
        let Some(&start) = self.starts.get(line) else {
            return Err(format!("line {} is outside the document", position.line));
        };
        let end = self.line_content_end(text, line);
        let target = position.character as usize;
        let mut utf16 = 0;
        for (relative, character) in text[start..end].char_indices() {
            if utf16 == target {
                return Ok(start + relative);
            }
            let next = utf16 + character.len_utf16();
            if target < next {
                return Err(format!(
                    "UTF-16 character {} splits a surrogate pair",
                    position.character
                ));
            }
            utf16 = next;
        }
        Ok(end)
    }

    pub(crate) fn range(&self, text: &str, span: Span) -> Range {
        Range::new(
            self.position(text, span.start),
            self.position(text, span.end),
        )
    }

    pub(crate) fn full_range(&self, text: &str) -> Range {
        self.range(text, Span::new(0, text.len()))
    }

    fn line_content_end(&self, text: &str, line: usize) -> usize {
        let Some(mut end) = self.starts.get(line + 1).copied() else {
            return text.len();
        };
        if end > 0 && text.as_bytes()[end - 1] == b'\n' {
            end -= 1;
            if end > 0 && text.as_bytes()[end - 1] == b'\r' {
                end -= 1;
            }
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_utf16_positions_without_splitting_surrogates() {
        let text = "a🤖b\n";
        let index = LineIndex::new(text);

        assert_eq!(index.position(text, 1), Position::new(0, 1));
        assert_eq!(index.position(text, 5), Position::new(0, 3));
        assert_eq!(index.offset(text, Position::new(0, 3)), Ok(5));
        assert!(
            index
                .offset(text, Position::new(0, 2))
                .expect_err("a surrogate split should fail")
                .contains("surrogate")
        );
        assert_eq!(
            index.position(text, text.len()),
            Position::new(1, 0),
            "a trailing newline creates an empty final LSP line"
        );
    }

    #[test]
    fn clamps_positions_past_line_content_and_rejects_missing_lines() {
        let text = "one\r\ntwo";
        let index = LineIndex::new(text);

        assert_eq!(index.offset(text, Position::new(1, 3)), Ok(text.len()));
        assert_eq!(index.offset(text, Position::new(0, 4)), Ok(3));
        assert_eq!(index.offset(text, Position::new(0, u32::MAX)), Ok(3));
        assert!(index.offset(text, Position::new(3, 0)).is_err());
    }

    #[test]
    fn accepts_the_empty_final_line_after_lf_or_crlf() {
        for text in ["one\n", "one\r\n"] {
            let index = LineIndex::new(text);

            assert_eq!(index.offset(text, Position::new(1, 0)), Ok(text.len()));
            assert_eq!(
                index.offset(text, Position::new(1, u32::MAX)),
                Ok(text.len())
            );
        }
    }
}
