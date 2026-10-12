//! Repair hints for constructs carried over from other languages.
//!
//! Authors, and language models in particular, write `return`, loops,
//! assignment, classes, imports, `async`, exceptions, indexing, arrow
//! functions, the conditional operator, and `switch` out of habit. None of
//! them is Krit syntax and none of their words is a Krit keyword, so the
//! parser recognizes each construct only from a token sequence that no valid
//! program contains, and reports it with its own code (`K1101`–`K1112`) and a
//! one-line fix in the diagnostic's notes. Analysis attaches hints of the same
//! kind to undefined names (`K2001`), method calls (`K3011`), and `None()`.
//!
//! Hints are fixed text plus identifiers taken from the source, so they are
//! deterministic and never contain a line break.

use crate::{Diagnostic, Span};

/// A construct from another language that Krit spells differently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Habit {
    /// `return value` (`K1101`).
    Return,
    /// `for`, `while`, `loop`, and `do` loops (`K1102`).
    Loop,
    /// Assignment, compound assignment, `++`/`--`, `mut`, `var`, and `const`
    /// (`K1103`).
    Mutation,
    /// `struct`, `class`, `impl`, `trait`, and `interface` (`K1104`).
    TypeDeclaration,
    /// `import`, `use`, `require`, and `from` (`K1105`).
    Import,
    /// `async` and `await` (`K1106`).
    Async,
    /// `try`, `catch`, `throw`, `raise`, `except`, and `finally` (`K1107`).
    Exception,
    /// Indexing `a[i]` (`K1108`).
    Indexing,
    /// Arrow functions `(x) => e` and `x => e` (`K1109`).
    ArrowFunction,
    /// The conditional operator `c ? a : b` (`K1110`).
    Ternary,
    /// `switch`, `case`, and `default` (`K1111`).
    Switch,
    /// `return` opening an `else` block (`K1112`).
    ElseReturn,
}

impl Habit {
    /// The habit a word introduces in another language.
    ///
    /// These words are ordinary Krit identifiers; the parser reports the habit
    /// only where the tokens around the word cannot be a valid program.
    pub(crate) fn of_word(word: &str) -> Option<Self> {
        Some(match word {
            "return" => Self::Return,
            "for" | "while" | "loop" | "do" => Self::Loop,
            "mut" | "var" | "const" => Self::Mutation,
            "struct" | "class" | "impl" | "trait" | "interface" => Self::TypeDeclaration,
            "import" | "use" | "require" | "from" => Self::Import,
            "async" | "await" => Self::Async,
            "try" | "catch" | "throw" | "raise" | "except" | "finally" => Self::Exception,
            "switch" | "case" | "default" => Self::Switch,
            _ => return None,
        })
    }

    /// The habit an undefined name most likely comes from, for the words that
    /// other languages use as a value or an operator on one (`return x`,
    /// `await x`, `throw x`, `require("x")`).
    ///
    /// Other habit words are common binding names (`from`, `case`, `default`),
    /// so an undefined one is more likely a missing binding or a typo.
    pub(crate) fn of_undefined_name(name: &str) -> Option<Self> {
        match name {
            "return" | "await" | "throw" | "raise" | "require" | "import" => Self::of_word(name),
            _ => None,
        }
    }

    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Return => "K1101",
            Self::Loop => "K1102",
            Self::Mutation => "K1103",
            Self::TypeDeclaration => "K1104",
            Self::Import => "K1105",
            Self::Async => "K1106",
            Self::Exception => "K1107",
            Self::Indexing => "K1108",
            Self::ArrowFunction => "K1109",
            Self::Ternary => "K1110",
            Self::Switch => "K1111",
            Self::ElseReturn => "K1112",
        }
    }

    /// The one-line fix carried in the diagnostic's notes.
    pub(crate) const fn hint(self) -> &'static str {
        match self {
            Self::Return => {
                "a block's value is its final expression, written without `return` or `;`; \
                 for early exit, `?` and let-else arrive in Krit 0.3"
            }
            Self::Loop => {
                "Krit has no loops; write a recursive function that matches `[]` and \
                 `[head, ..tail]` (list functions arrive in Krit 0.4)"
            }
            Self::Mutation => {
                "Krit values are immutable; bind each new value with `let` under a new name"
            }
            Self::TypeDeclaration => {
                "Krit has no classes, methods, or traits; use a `record { field: value }` \
                 value and plain functions"
            }
            Self::Import => {
                "modules are not available yet; a Krit program is one file, and built-in \
                 functions need no import"
            }
            Self::Async => {
                "Krit has no async functions; host calls such as `http_request` finish before \
                 they return, so call them directly"
            }
            Self::Exception => {
                "Krit has no exceptions; return `Ok(value)` or `Err(error)` and handle the \
                 `Result` with `match`"
            }
            Self::Indexing => {
                "match the list with `[]` and `[head, ..tail]` patterns (`list_get` arrives \
                 in Krit 0.4)"
            }
            Self::ArrowFunction => "write an anonymous function as `fn(x) { e }`",
            Self::Ternary => "`if` is an expression: write `if condition { a } else { b }`",
            Self::Switch => {
                "use `match` for `Option`, `Result`, and list values, or an \
                 `if ... else if ... else` chain"
            }
            Self::ElseReturn => {
                "to exit early, use let-else, arriving in Krit 0.3: \
                 `let Some(x) = value else None => fallback;`"
            }
        }
    }

    /// Reports this habit at `span` with its fix.
    pub(crate) fn diagnostic(self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(self.code(), message, span).with_note(self.hint())
    }

    /// Reports this habit, introduced by `word` at `span`.
    pub(crate) fn word_diagnostic(self, word: &str, span: Span) -> Diagnostic {
        self.diagnostic(format!("`{word}` is not Krit syntax"), span)
    }
}

/// The note for an undefined name (`K2001`), if one applies.
///
/// A spelling of "no value" from another language suggests `None`; a word
/// that other languages use as a value or operator gets its habit's fix;
/// anything else gets the closest of `candidates` (see [`closest_name`]).
pub(crate) fn undefined_name_note<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    if matches!(
        name,
        "null" | "NULL" | "Null" | "nil" | "Nil" | "NIL" | "undefined" | "nullptr"
    ) {
        return Some(format!(
            "Krit has no `{name}`; write `None` for an absent `Option` value"
        ));
    }
    if let Some(habit) = Habit::of_undefined_name(name) {
        return Some(habit.hint().to_owned());
    }
    closest_name(name, candidates).map(|candidate| format!("did you mean `{candidate}`?"))
}

/// The most edits a did-you-mean suggestion may be from the written name.
const MAX_SUGGESTION_DISTANCE: usize = 2;

/// The candidate closest to `name`, for a did-you-mean suggestion.
///
/// A candidate qualifies when it is at most [`MAX_SUGGESTION_DISTANCE`]
/// edits from `name` (insertions, deletions, substitutions, and swaps of two
/// adjacent characters) and fewer edits than `name` has characters, so a
/// one-letter name is never matched to an unrelated one. Ties go to the
/// fewest edits, then to the bytewise smallest name, so the result does not
/// depend on the candidates' order. The discard name `_` never qualifies.
pub(crate) fn closest_name<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best: Option<(usize, &'a str)> = None;
    for candidate in candidates {
        if candidate == name || candidate == "_" {
            continue;
        }
        let Some(distance) = edit_distance(
            name.as_bytes(),
            candidate.as_bytes(),
            MAX_SUGGESTION_DISTANCE,
        ) else {
            continue;
        };
        if distance < name.len() && best.is_none_or(|current| (distance, candidate) < current) {
            best = Some((distance, candidate));
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// The optimal-string-alignment distance between `left` and `right`, if it is
/// at most `limit`.
///
/// Runs in time linear in the inputs for the small limits used here: it skips
/// the common prefix and tries each edit at the first difference with one
/// fewer edit left, so it never builds a distance matrix over long names.
fn edit_distance(left: &[u8], right: &[u8], limit: usize) -> Option<usize> {
    (0..=limit).find(|&edits| within_edits(left, right, edits))
}

fn within_edits(left: &[u8], right: &[u8], edits: usize) -> bool {
    if left.len().abs_diff(right.len()) > edits {
        return false;
    }
    let common = left
        .iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count();
    let (left, right) = (&left[common..], &right[common..]);
    if left.is_empty() && right.is_empty() {
        return true;
    }
    if edits == 0 {
        return false;
    }
    let remaining = edits - 1;
    (!left.is_empty() && !right.is_empty() && within_edits(&left[1..], &right[1..], remaining))
        || (!left.is_empty() && within_edits(&left[1..], right, remaining))
        || (!right.is_empty() && within_edits(left, &right[1..], remaining))
        || (left.len() >= 2
            && right.len() >= 2
            && left[0] == right[1]
            && left[1] == right[0]
            && within_edits(&left[2..], &right[2..], remaining))
}

/// What a method call's receiver is, as far as its hint is concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MethodReceiver {
    String,
    Int,
    List,
    /// An `Option` or `Result`.
    Variant,
    Other,
}

/// The fix for calling `.method()` on `receiver`, a value written as `value`
/// (`K3011`): the standard-library function that does the job when one exists,
/// otherwise how to write the call without a method.
pub(crate) fn method_hint(receiver: MethodReceiver, method: &str, value: &str) -> String {
    match receiver {
        MethodReceiver::String => match method {
            "len" | "length" | "size" | "count" | "byte_len" => {
                format!("call `string_byte_len({value})` instead; it counts UTF-8 bytes")
            }
            "is_empty" | "isEmpty" | "empty" => format!("call `string_is_empty({value})` instead"),
            "contains" | "includes" => format!("call `string_contains({value}, part)` instead"),
            "starts_with" | "startsWith" | "startswith" | "hasPrefix" => {
                format!("call `string_starts_with({value}, prefix)` instead")
            }
            "ends_with" | "endsWith" | "endswith" | "hasSuffix" => {
                format!("call `string_ends_with({value}, suffix)` instead")
            }
            _ => "pass the string to a function instead; the string functions are \
                  `string_byte_len`, `string_is_empty`, `string_contains`, \
                  `string_starts_with`, and `string_ends_with`"
                .to_owned(),
        },
        MethodReceiver::Int => match method {
            "to_string" | "toString" | "string" | "str" => {
                format!("call `int_to_string({value})` instead")
            }
            _ => format!("pass the integer to a function instead, as in `f({value})`"),
        },
        MethodReceiver::List => "match the list with `[]` and `[head, ..tail]` patterns instead \
                                 (list functions arrive in Krit 0.4)"
            .to_owned(),
        MethodReceiver::Variant => "handle both outcomes with `match` instead \
                                    (`?` and let-else arrive in Krit 0.3)"
            .to_owned(),
        MethodReceiver::Other => {
            format!("pass the value to a function instead, as in `f({value})`")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Habit; 12] = [
        Habit::Return,
        Habit::Loop,
        Habit::Mutation,
        Habit::TypeDeclaration,
        Habit::Import,
        Habit::Async,
        Habit::Exception,
        Habit::Indexing,
        Habit::ArrowFunction,
        Habit::Ternary,
        Habit::Switch,
        Habit::ElseReturn,
    ];

    #[test]
    fn every_habit_has_its_own_code_and_a_one_line_hint() {
        let codes = ALL.map(Habit::code);
        let expected = (1101..=1112)
            .map(|code| format!("K{code}"))
            .collect::<Vec<_>>();
        assert_eq!(codes.to_vec(), expected);
        for habit in ALL {
            let hint = habit.hint();
            assert!(
                !hint.is_empty() && !hint.contains(['\n', '\r']),
                "{habit:?}"
            );
            assert!(!hint.contains("  "), "{habit:?}: {hint}");
            let diagnostic = habit.diagnostic("message", Span::new(0, 1));
            assert_eq!(diagnostic.code(), habit.code());
            assert_eq!(diagnostic.notes(), [hint.to_owned()]);
        }
    }

    #[test]
    fn maps_foreign_words_to_their_habits() {
        for (word, habit) in [
            ("return", Habit::Return),
            ("while", Habit::Loop),
            ("const", Habit::Mutation),
            ("interface", Habit::TypeDeclaration),
            ("from", Habit::Import),
            ("await", Habit::Async),
            ("raise", Habit::Exception),
            ("default", Habit::Switch),
        ] {
            assert_eq!(Habit::of_word(word), Some(habit), "{word}");
        }
        assert_eq!(Habit::of_word("value"), None);
        assert_eq!(Habit::of_word("Return"), None);
        assert_eq!(Habit::of_undefined_name("await"), Some(Habit::Async));
        assert_eq!(Habit::of_undefined_name("from"), None);
    }

    #[test]
    fn measures_edits_including_adjacent_swaps() {
        assert_eq!(edit_distance(b"println", b"println", 2), Some(0));
        assert_eq!(edit_distance(b"pritnln", b"println", 2), Some(1));
        assert_eq!(edit_distance(b"totl", b"total", 2), Some(1));
        assert_eq!(edit_distance(b"lenght", b"length", 2), Some(1));
        assert_eq!(edit_distance(b"kitten", b"sitting", 2), None);
        assert_eq!(edit_distance(b"abc", b"", 2), None);
        assert_eq!(edit_distance(b"ab", b"", 2), Some(2));
        assert_eq!(edit_distance(b"value", b"valeu", 2), Some(1));
        assert_eq!(edit_distance(b"Some", b"some", 2), Some(1));

        // Linear in the name length: a near-miss of a long name is cheap.
        let long = "a".repeat(100_000);
        let mut near = long.clone();
        near.replace_range(50_000..50_001, "b");
        assert_eq!(edit_distance(long.as_bytes(), near.as_bytes(), 2), Some(1));
    }

    #[test]
    fn suggests_the_closest_name_deterministically() {
        let candidates = ["total", "token", "println", "_"];
        assert_eq!(closest_name("totl", candidates), Some("total"));
        assert_eq!(closest_name("pritnln", candidates), Some("println"));
        assert_eq!(closest_name("unrelated", candidates), None);
        // Equal distances go to the bytewise smallest name, in any order.
        assert_eq!(closest_name("tok", ["toks", "tokb", "toka"]), Some("toka"));
        assert_eq!(closest_name("tok", ["toka", "tokb", "toks"]), Some("toka"));
        // Fewer edits win over a smaller name.
        assert_eq!(closest_name("tokn", ["aaokn", "token"]), Some("token"));
        // A one-letter name is never matched to another one-letter name.
        assert_eq!(closest_name("x", ["y", "_"]), None);
        assert_eq!(closest_name("y", ["_"]), None);
    }

    #[test]
    fn notes_absent_values_habit_words_and_typos() {
        assert_eq!(
            undefined_name_note("null", ["nil"]).as_deref(),
            Some("Krit has no `null`; write `None` for an absent `Option` value")
        );
        assert_eq!(
            undefined_name_note("return", ["result"]).as_deref(),
            Some(Habit::Return.hint())
        );
        assert_eq!(
            undefined_name_note("totl", ["total"]).as_deref(),
            Some("did you mean `total`?")
        );
        assert_eq!(undefined_name_note("missing", ["total"]), None);
    }

    #[test]
    fn names_the_standard_library_function_for_common_methods() {
        assert_eq!(
            method_hint(MethodReceiver::String, "len", "s"),
            "call `string_byte_len(s)` instead; it counts UTF-8 bytes"
        );
        assert!(
            method_hint(MethodReceiver::String, "startsWith", "path")
                .starts_with("call `string_starts_with(path, prefix)`")
        );
        assert!(method_hint(MethodReceiver::Int, "toString", "n").contains("int_to_string(n)"));
        assert!(method_hint(MethodReceiver::List, "len", "xs").contains("[head, ..tail]"));
        assert!(method_hint(MethodReceiver::Variant, "unwrap", "value").contains("`match`"));
        assert!(method_hint(MethodReceiver::Other, "send", "value").contains("`f(value)`"));
        for receiver in [
            MethodReceiver::String,
            MethodReceiver::Int,
            MethodReceiver::List,
            MethodReceiver::Variant,
            MethodReceiver::Other,
        ] {
            let hint = method_hint(receiver, "unknown", "value");
            assert!(
                !hint.contains(['\n', '\r']) && !hint.contains("  "),
                "{hint}"
            );
        }
    }
}
