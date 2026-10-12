//! The rules that decide which `match` arms may stand together.
//!
//! Every pattern constrains a single path into the matched value: a variant
//! pattern's payload is itself a pattern, while `None`, `[]`, and
//! `[head, ..tail]` constrain nothing below them. A pattern is therefore
//! described by its *path*, the variant and list constructors it requires
//! from the outside in, and it matches exactly the values whose own
//! constructors start with that path. Two patterns overlap exactly when one
//! path is a prefix of the other, and a set of pairwise disjoint paths is
//! exhaustive exactly when, at every proper prefix of a path, each
//! constructor of that position's family continues some path.
//!
//! Analysis applies these rules, in this order: within the arms that are not
//! catch-alls, one family at each position and no overlap (`K1003`, in arm
//! order); no top-level catch-all arm (`K3005`); then, after typing the
//! patterns against the subject, exhaustiveness (`K3005`). Each match
//! therefore names every outcome exactly once, so the order of its arms never
//! changes its meaning, and Core IR decides it with one nested match per
//! position without duplicating any arm.

use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
};

use crate::{Diagnostic, MatchArm, Pattern, PatternKind, Span, VariantName};

/// A variant or list constructor on a pattern's path.
///
/// The declaration order is each family's canonical arm order, so sorting
/// paths lists missing patterns in a stable, readable order.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Constructor {
    Some,
    None,
    Ok,
    Err,
    Empty,
    Cons,
}

/// The built-in family of a constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Family {
    Option,
    Result,
    List,
}

impl Family {
    const fn constructors(self) -> [Constructor; 2] {
        match self {
            Self::Option => [Constructor::Some, Constructor::None],
            Self::Result => [Constructor::Ok, Constructor::Err],
            Self::List => [Constructor::Empty, Constructor::Cons],
        }
    }

    const fn described(self) -> &'static str {
        match self {
            Self::Option => "an `Option`",
            Self::Result => "a `Result`",
            Self::List => "a `List`",
        }
    }
}

impl Constructor {
    const fn from_variant(variant: VariantName) -> Self {
        match variant {
            VariantName::Some => Self::Some,
            VariantName::None => Self::None,
            VariantName::Ok => Self::Ok,
            VariantName::Err => Self::Err,
        }
    }

    /// The variant this constructor is, or `None` for a list constructor.
    pub(crate) const fn variant(self) -> Option<VariantName> {
        match self {
            Self::Some => Some(VariantName::Some),
            Self::None => Some(VariantName::None),
            Self::Ok => Some(VariantName::Ok),
            Self::Err => Some(VariantName::Err),
            Self::Empty | Self::Cons => None,
        }
    }

    pub(crate) const fn family(self) -> Family {
        match self {
            Self::Some | Self::None => Family::Option,
            Self::Ok | Self::Err => Family::Result,
            Self::Empty | Self::Cons => Family::List,
        }
    }

    /// The constructor as a pattern that matches all of its values.
    const fn open_pattern(self) -> &'static str {
        match self {
            Self::Some => "Some(_)",
            Self::None => "None",
            Self::Ok => "Ok(_)",
            Self::Err => "Err(_)",
            Self::Empty => "[]",
            Self::Cons => "[_, .._]",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Some => "Some",
            Self::None => "None",
            Self::Ok => "Ok",
            Self::Err => "Err",
            Self::Empty => "[]",
            Self::Cons => "[_, .._]",
        }
    }
}

/// The constructor `pattern` requires of the value it matches, or `None` for
/// a name or `_`.
pub(crate) const fn constructor(pattern: &Pattern) -> Option<Constructor> {
    match &pattern.kind {
        PatternKind::Binder(_) | PatternKind::Wildcard => None,
        PatternKind::EmptyList => Some(Constructor::Empty),
        PatternKind::Cons { .. } => Some(Constructor::Cons),
        PatternKind::Variant { variant, .. } => Some(Constructor::from_variant(*variant)),
    }
}

/// One step of a pattern's path: a constructor and the span of the pattern
/// that requires it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    pub(crate) constructor: Constructor,
    pub(crate) span: Span,
}

/// Returns the path of `pattern`, rejecting shapes the parser never builds
/// (`K1003`): a payload on `None`, a variant without one, or a list pattern
/// whose head or tail is not a name or `_`.
pub(crate) fn path(pattern: &Pattern) -> Result<Vec<Step>, Diagnostic> {
    let mut steps = Vec::new();
    let mut current = pattern;
    loop {
        match &current.kind {
            PatternKind::Binder(_) | PatternKind::Wildcard => return Ok(steps),
            PatternKind::EmptyList => {
                steps.push(Step {
                    constructor: Constructor::Empty,
                    span: current.span,
                });
                return Ok(steps);
            }
            PatternKind::Cons { head, tail } => {
                if !head.is_catch_all() || !tail.is_catch_all() {
                    return Err(Diagnostic::new(
                        "K1003",
                        "the head and tail of a list pattern are names or `_`",
                        current.span,
                    ));
                }
                steps.push(Step {
                    constructor: Constructor::Cons,
                    span: current.span,
                });
                return Ok(steps);
            }
            PatternKind::Variant { variant, payload } => {
                steps.push(Step {
                    constructor: Constructor::from_variant(*variant),
                    span: current.span,
                });
                match (variant, payload) {
                    (VariantName::None, None) => return Ok(steps),
                    (VariantName::None, Some(_)) => {
                        return Err(Diagnostic::new(
                            "K1003",
                            "`None` does not bind a value",
                            current.span,
                        ));
                    }
                    (_, Some(payload)) => current = payload,
                    (_, None) => {
                        return Err(Diagnostic::new(
                            "K1003",
                            format!("`{}` must bind one value", variant.as_str()),
                            current.span,
                        ));
                    }
                }
            }
        }
    }
}

fn constructors(steps: &[Step]) -> Vec<Constructor> {
    steps.iter().map(|step| step.constructor).collect()
}

/// Renders the pattern that requires `path` and matches everything below it,
/// such as `Ok(None)`, `Ok(Some(_))`, or `Err([_, .._])`.
pub(crate) fn render_path(path: &[Constructor]) -> String {
    let Some((last, outer)) = path.split_last() else {
        return "_".to_owned();
    };
    let mut rendered = String::new();
    for constructor in outer {
        rendered.push_str(constructor.name());
        rendered.push('(');
    }
    rendered.push_str(last.open_pattern());
    rendered.push_str(&")".repeat(outer.len()));
    rendered
}

/// The patterns missing from a family-consistent set of disjoint `paths`,
/// as paths: for every proper prefix of a path, each constructor of the next
/// position's family that continues no path. They are sorted as paths, so
/// depth first and in each family's canonical order.
fn missing(paths: &[Vec<Constructor>]) -> Vec<Vec<Constructor>> {
    let mut continued = BTreeSet::new();
    let mut prefixes = BTreeMap::new();
    for path in paths {
        for length in 0..path.len() {
            prefixes
                .entry(&path[..length])
                .or_insert(path[length].family());
            continued.insert(&path[..=length]);
        }
    }
    let mut missing = Vec::new();
    for (prefix, family) in prefixes {
        for constructor in family.constructors() {
            let mut candidate = prefix.to_vec();
            candidate.push(constructor);
            if !continued.contains(candidate.as_slice()) {
                missing.push(candidate);
            }
        }
    }
    missing.sort();
    missing
}

fn quoted_list(paths: &[Vec<Constructor>]) -> String {
    paths
        .iter()
        .map(|path| format!("`{}`", render_path(path)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn arm_list(paths: &[Vec<Constructor>]) -> String {
    let arms = paths
        .iter()
        .map(|path| format!("`{} => ...`", render_path(path)))
        .collect::<Vec<_>>();
    match arms.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Checks the arms of a match against each other: within the arms that are
/// not catch-alls, one family at each position and no overlap (`K1003`, in
/// arm order), and then no top-level catch-all arm (`K3005`).
///
/// `subject` is the family of the subject's type when it is already known;
/// it only names the outcomes a lone catch-all arm should be replaced with.
pub(crate) fn check_arms(arms: &[MatchArm], subject: Option<Family>) -> Result<(), Diagnostic> {
    // The path each earlier arm ends at, with its position and the arm, and
    // for each proper prefix of those paths, the family of the constructor
    // that continues it and the first arm that did. Both are keyed by path,
    // so checking an arm takes time proportional to its own path, plus the
    // earlier paths that extend it, which are one contiguous range of `ends`.
    let mut ends: BTreeMap<Vec<Constructor>, (usize, &MatchArm)> = BTreeMap::new();
    let mut families: BTreeMap<Vec<Constructor>, (Family, &MatchArm)> = BTreeMap::new();
    for (index, arm) in arms.iter().enumerate() {
        if arm.pattern.is_catch_all() {
            continue;
        }
        let steps = path(&arm.pattern)?;
        let own = constructors(&steps);
        for depth in 0..=own.len() {
            let prefix = &own[..depth];
            if let Some((_, other)) = ends.get(prefix) {
                // An earlier arm's path is a prefix of this one's.
                return Err(if depth == own.len() {
                    Diagnostic::new(
                        "K1003",
                        format!(
                            "duplicate match arm: `{}` matches exactly what the earlier arm \
                             `{}` matches",
                            arm.pattern, other.pattern
                        ),
                        arm.pattern.span,
                    )
                    .with_note("remove one of the two arms")
                } else {
                    Diagnostic::new(
                        "K1003",
                        format!(
                            "unreachable match arm: the earlier arm `{}` already matches every \
                             value `{}` matches",
                            other.pattern, arm.pattern
                        ),
                        arm.pattern.span,
                    )
                    .with_note("remove this arm")
                });
            }
            let Some(&constructor) = own.get(depth) else {
                continue;
            };
            if let Some((family, other)) = families.get(prefix)
                && *family != constructor.family()
            {
                return Err(Diagnostic::new(
                    "K1003",
                    format!(
                        "`{}` is {} pattern, but the earlier arm `{}` matches {} value here",
                        sub_pattern(&arm.pattern, depth),
                        constructor.family().described(),
                        other.pattern,
                        family.described(),
                    ),
                    steps[depth].span,
                )
                .with_note(
                    "each position of a match takes the patterns of one family: `Some` and \
                     `None`, `Ok` and `Err`, or `[]` and `[head, ..tail]`",
                ));
            }
        }
        // Earlier arms whose paths extend this one's are more specific, so
        // this arm would only catch what they leave over.
        let narrower = ends
            .range::<[Constructor], _>((Bound::Excluded(own.as_slice()), Bound::Unbounded))
            .take_while(|(other, _)| other.starts_with(&own))
            .map(|(other, (position, other_arm))| (other.clone(), *position, *other_arm))
            .collect::<Vec<_>>();
        if let Some((_, _, first)) = narrower.iter().min_by_key(|(_, position, _)| *position) {
            let paths = narrower
                .iter()
                .map(|(other, _, _)| other.clone())
                .collect::<Vec<_>>();
            let remainder = missing(&paths)
                .into_iter()
                .filter(|path| path.starts_with(&own))
                .collect::<Vec<_>>();
            if remainder.is_empty() {
                return Err(Diagnostic::new(
                    "K1003",
                    format!(
                        "unreachable match arm: earlier arms already match every value `{}` \
                         matches",
                        arm.pattern
                    ),
                    arm.pattern.span,
                )
                .with_note("remove this arm"));
            }
            return Err(Diagnostic::new(
                "K1003",
                format!(
                    "match arm `{}` overlaps the earlier arm `{}`: arms may not overlap",
                    arm.pattern, first.pattern
                ),
                arm.pattern.span,
            )
            .with_note(format!(
                "name each remaining outcome instead: replace `{} => ...` with {}",
                arm.pattern,
                arm_list(&remainder)
            )));
        }
        for depth in 0..own.len() {
            families
                .entry(own[..depth].to_vec())
                .or_insert((own[depth].family(), arm));
        }
        ends.insert(own, (index, arm));
    }

    let Some(catch_all) = arms.iter().find(|arm| arm.pattern.is_catch_all()) else {
        return Ok(());
    };
    let named = ends.into_keys().collect::<Vec<_>>();
    let remainder = if named.is_empty() {
        subject.map(|family| {
            family
                .constructors()
                .iter()
                .map(|constructor| vec![*constructor])
                .collect::<Vec<_>>()
        })
    } else {
        Some(missing(&named))
    };
    let note = match remainder {
        Some(remainder) if remainder.is_empty() => format!(
            "name every outcome: the other arms already do, so remove `{} => ...`",
            catch_all.pattern
        ),
        Some(remainder) => format!(
            "name every outcome: replace `{} => ...` with {}",
            catch_all.pattern,
            arm_list(&remainder)
        ),
        None => "name every outcome with its own arm, such as `Some(value)` and `None`, \
                 `Ok(value)` and `Err(error)`, or `[]` and `[head, ..tail]`"
            .to_owned(),
    };
    let what = match &catch_all.pattern.kind {
        PatternKind::Wildcard => "`_`".to_owned(),
        _ => format!("the name `{}`", catch_all.pattern),
    };
    Err(Diagnostic::new(
        "K3005",
        format!(
            "a match arm cannot match every value: {what} would stand for outcomes it does not name"
        ),
        catch_all.pattern.span,
    )
    .with_note(note))
}

/// Checks that the arms of the match at `span`, which [`check_arms`]
/// accepted, name every outcome (`K3005`, listing each missing pattern).
pub(crate) fn check_exhaustive(arms: &[MatchArm], span: Span) -> Result<(), Diagnostic> {
    let paths = arms
        .iter()
        .map(|arm| path(&arm.pattern).map(|steps| constructors(&steps)))
        .collect::<Result<Vec<_>, _>>()?;
    let missing = missing(&paths);
    if missing.is_empty() {
        return Ok(());
    }
    Err(Diagnostic::new(
        "K3005",
        format!("non-exhaustive match; missing: {}", quoted_list(&missing)),
        span,
    )
    .with_note(format!(
        "add an arm for each missing pattern: {}",
        arm_list(&missing)
    )))
}

/// The sub-pattern of `pattern` at `depth` steps down its path.
fn sub_pattern(pattern: &Pattern, depth: usize) -> &Pattern {
    let mut current = pattern;
    for _ in 0..depth {
        match &current.kind {
            PatternKind::Variant {
                payload: Some(payload),
                ..
            } => current = payload,
            _ => break,
        }
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(patterns: &[&[Constructor]]) -> Vec<Vec<Constructor>> {
        patterns.iter().map(|path| path.to_vec()).collect()
    }

    #[test]
    fn renders_missing_paths_in_canonical_order() {
        use Constructor::{Cons, Empty, Err, None, Ok, Some};
        let found = missing(&paths(&[&[Ok, Some]]));
        let rendered = found
            .iter()
            .map(|path| render_path(path))
            .collect::<Vec<_>>();
        assert_eq!(rendered, ["Ok(None)", "Err(_)"]);

        let found = missing(&paths(&[&[Err, Cons], &[Ok]]));
        let rendered = found
            .iter()
            .map(|path| render_path(path))
            .collect::<Vec<_>>();
        assert_eq!(rendered, ["Err([])"]);

        assert!(
            missing(&paths(&[
                &[Some, Ok],
                &[Some, Err, Empty],
                &[Some, Err, Cons],
                &[None]
            ]))
            .is_empty()
        );
        assert_eq!(render_path(&[]), "_");
    }
}
