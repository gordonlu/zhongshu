/// Built-in equipment packages shipped with zhongshu.
/// Written to the equipment directory on first launch.

pub const SEARCH_FILES_NAME: &str = "search-files";
pub const SEARCH_FILES_VERSION: &str = "1.0.0";

pub const SEARCH_FILES_MANIFEST: &str = r#"{
  "name": "search-files",
  "version": "1.0.0",
  "type": "skill",
  "description": "跨平台文件搜索，自动选择最优引擎",
  "tools": ["shell"],
  "permissions": {
    "shell": {
      "allowed_commands": ["locate", "fd", "find", "es", "dir"]
    }
  }
}"#;

pub const SEARCH_FILES_PROMPT: &str = "\
## 技能：文件搜索

当用户需要查找文件时，优先使用本技能，不要问用户更多信息。

### 第一步：检查最优工具是否安装

```
Linux:  which locate  → plocate/mlocate（最快，基于索引）
        which fd      → fd（快速，模糊匹配）

Windows: where es.exe  → Everything（最快，实时索引）
```

如果最优工具（locate / es.exe）未安装：
- 询问用户：\"需要安装 <工具> 来加速文件搜索吗？\"
- Linux 安装命令：`sudo apt install plocate`
- Windows 安装命令：建议用户从 voidtools.com 下载 Everything
- 用户同意 → 执行安装 → 安装后使用最优工具
- 用户拒绝 → 走第二步兜底

### 第二步：执行搜索

**Linux 优先级：**
1. `locate -i <关键词>` — 已确认可用时用
2. `fd -i '<模式>' <路径>` — locate 不可用时尝试
3. `find <路径> -iname '*<模式>*' 2>/dev/null` — 兜底

**Windows 优先级：**
1. `es.exe <关键词>` — 已确认可用时用
2. `dir /s /b <路径>\\*<模式>*` — 兜底

### 技巧
- 搜索结果过多用 `| head -20` 限制行数
- 找不到时换更宽泛的关键词重试
- Linux 也推荐安装 fd（`sudo apt install fd-find`）
";

/// List of all built-in equipment: (name, version, manifest_json, prompt_md).
pub fn all_builtins() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (SEARCH_FILES_NAME, SEARCH_FILES_VERSION, SEARCH_FILES_MANIFEST, SEARCH_FILES_PROMPT),
    ]
}
