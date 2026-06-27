#[derive(Debug, Clone, PartialEq)]
pub enum VerificationType {
    Test,
    Check,
    Audit,
    Unknown,
}

pub fn classify_command(command: &str) -> VerificationType {
    let semantics = crate::tool::shell_semantics::ShellSemantics::analyze(command);
    if !semantics.is_verification {
        return VerificationType::Unknown;
    }

    let cmd = semantics.normalized_command.to_lowercase();
    if cmd.contains(" audit") || cmd.starts_with("cargo audit") || cmd.starts_with("npm audit") {
        VerificationType::Audit
    } else if cmd.contains(" test")
        || cmd.starts_with("pytest")
        || cmd.starts_with("vitest")
        || cmd.starts_with("jest")
        || cmd.starts_with("mocha")
        || cmd.starts_with("go test")
    {
        VerificationType::Test
    } else {
        VerificationType::Check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_cargo_test() {
        assert_eq!(classify_command("cargo test"), VerificationType::Test);
        assert_eq!(
            classify_command("cargo test -- --skip auth"),
            VerificationType::Test
        );
    }

    #[test]
    fn classify_cargo_check() {
        assert_eq!(classify_command("cargo check"), VerificationType::Check);
    }

    #[test]
    fn classify_compound_verification() {
        assert_eq!(
            classify_command("rg TODO zhongshu-core && cargo test -p zhongshu-core"),
            VerificationType::Test
        );
    }

    #[test]
    fn classify_unknown() {
        assert_eq!(classify_command("ls -la"), VerificationType::Unknown);
    }
}
