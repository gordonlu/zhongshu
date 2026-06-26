#[derive(Debug, Clone, PartialEq)]
pub enum VerificationType {
    Test,
    Check,
    Audit,
    Unknown,
}

pub fn classify_command(command: &str) -> VerificationType {
    let cmd = command.trim();
    if cmd.starts_with("cargo test")
        || cmd.starts_with("npm test")
        || cmd.starts_with("pytest")
        || cmd.starts_with("go test")
    {
        VerificationType::Test
    } else if cmd.starts_with("cargo check")
        || cmd.starts_with("cargo clippy")
        || cmd.starts_with("npm run check")
        || cmd.starts_with("ruff check")
    {
        VerificationType::Check
    } else if cmd.starts_with("cargo audit")
        || cmd.starts_with("npm audit")
    {
        VerificationType::Audit
    } else {
        VerificationType::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_cargo_test() {
        assert_eq!(classify_command("cargo test"), VerificationType::Test);
        assert_eq!(classify_command("cargo test -- --skip auth"), VerificationType::Test);
    }

    #[test]
    fn classify_cargo_check() {
        assert_eq!(classify_command("cargo check"), VerificationType::Check);
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(classify_command("ls -la"), VerificationType::Unknown);
    }
}
