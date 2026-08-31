use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub const fn join(self, other: Self) -> Self {
        Self {
            start: self.start,
            end: other.end,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
    pub byte: usize,
}

#[derive(Clone, Debug)]
pub struct Source {
    name: Arc<str>,
    text: Arc<str>,
    line_starts: Arc<[usize]>,
}

impl Source {
    pub fn new(name: impl Into<Arc<str>>, text: impl Into<Arc<str>>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Self {
            name: name.into(),
            text,
            line_starts: line_starts.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn position(&self, byte: usize) -> Position {
        let byte = byte.min(self.text.len());
        let line_index = self.line_starts.partition_point(|start| *start <= byte) - 1;
        let line_start = self.line_starts[line_index];
        let column = self.text[line_start..byte].chars().count() + 1;
        Position {
            line: line_index + 1,
            column,
            byte,
        }
    }
}
