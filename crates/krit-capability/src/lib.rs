pub fn is_valid_resource_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && !name.ends_with(['.', '-'])
        && !name.contains("..")
        && !name.contains("--")
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::is_valid_resource_name;

    #[test]
    fn accepts_only_canonical_resource_names() {
        for valid in ["a", "agent.model", "github-token", "model2"] {
            assert!(is_valid_resource_name(valid), "{valid}");
        }
        for invalid in [
            "",
            ".agent",
            "-token",
            "agent.",
            "token-",
            "agent..model",
            "github--token",
            "Agent",
            "github/token",
        ] {
            assert!(!is_valid_resource_name(invalid), "{invalid}");
        }
        assert!(!is_valid_resource_name(&"a".repeat(65)));
    }
}
