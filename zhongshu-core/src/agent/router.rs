/// Routes user input to the appropriate DeepSeek V4 model
/// based on complexity heuristics.
///
/// Both Flash and Pro support `reasoning_effort` (`"high"` default, also `"max"`).
/// Flash is the fast version with full reasoning capability — not a gimped mini.
/// Pro is the thorough version, reserved for deep analysis and agent workflows.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Complexity {
    Simple,
    Complex,
    Agent,
}

#[derive(Debug, Clone)]
pub struct ModelRouter {
    pub flash_model: String,
    pub pro_model: String,
}

impl ModelRouter {
    pub fn new(flash: impl Into<String>, pro: impl Into<String>) -> Self {
        ModelRouter {
            flash_model: flash.into(),
            pro_model: pro.into(),
        }
    }

    /// Classify input and return (model_name, reasoning_effort).
    /// Flash has full reasoning (default `"high"`, also `"max"`).
    ///
    /// - Simple → Flash (uses default thinking, no explicit effort set)
    /// - Complex → Pro, `"high"` (deep analysis)
    /// - Agent → Pro, `"max"` (multi-step tool workflows)
    pub fn route(&self, input: &str) -> (String, Option<&str>) {
        match self.classify(input) {
            Complexity::Simple => (self.flash_model.clone(), None),
            Complexity::Complex => (self.pro_model.clone(), Some("high")),
            Complexity::Agent => (self.pro_model.clone(), Some("max")),
        }
    }

    pub fn classify(&self, input: &str) -> Complexity {
        let input_lower = input.to_lowercase();
        let char_count = input.chars().count();

        if char_count > 200 {
            return Complexity::Complex;
        }

        let complex_keywords = [
            "分析",
            "对比",
            "比较",
            "总结",
            "研究",
            "调查",
            "implement",
            "refactor",
            "analyze",
            "compare",
            "debug",
            "optimize",
            "design",
            "architecture",
            "plan",
            "strategy",
            "review",
            "investigate",
            "code",
            "编写",
            "调优",
            "重构",
            "架构",
        ];
        let agent_keywords = [
            "search",
            "browse",
            "fetch",
            "web",
            "搜索",
            "浏览",
            "抓取",
            "调用工具",
            "multi-step",
            "automate",
            "自动",
        ];

        let has_complex = complex_keywords.iter().any(|k| input_lower.contains(k));
        let has_agent = agent_keywords.iter().any(|k| input_lower.contains(k));

        match (has_agent, has_complex) {
            (true, _) => Complexity::Agent,
            (_, true) => Complexity::Complex,
            _ => Complexity::Simple,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_query() {
        let router = ModelRouter::new("flash", "pro");
        assert_eq!(router.route("你好"), ("flash".to_string(), None));
        assert_eq!(router.route("Hello world"), ("flash".to_string(), None));
    }

    #[test]
    fn test_complex_query() {
        let router = ModelRouter::new("flash", "pro");
        let (model, effort) = router.route("请分析这段代码的性能瓶颈");
        assert_eq!(model, "pro");
        assert_eq!(effort, Some("high"));
    }

    #[test]
    fn test_agent_query() {
        let router = ModelRouter::new("flash", "pro");
        let (model, effort) = router.route("搜索最新的 Rust 新闻并总结");
        assert_eq!(model, "pro");
        assert_eq!(effort, Some("max"));
    }

    #[test]
    fn test_long_input() {
        let router = ModelRouter::new("flash", "pro");
        let long = "x".repeat(201);
        let (model, effort) = router.route(&long);
        assert_eq!(model, "pro");
        assert_eq!(effort, Some("high"));
    }
}
