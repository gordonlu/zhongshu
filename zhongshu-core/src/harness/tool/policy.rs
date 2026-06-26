pub fn check_policy(tool_name: &str) -> bool {
    !matches!(
        tool_name,
        "shell" | "screenshot" | "automation" | "browser_automation"
    )
}
