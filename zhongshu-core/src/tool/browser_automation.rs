use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use crate::authority::{self, CheckResult};
use crate::tool::{sanitize_web_content, Tool, ToolOutput};

const DEFAULT_PORT: u16 = 9223;

static BROWSER: Lazy<Mutex<Option<ManagedBrowser>>> = Lazy::new(|| Mutex::new(None));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct BrowserAutomationTool;

/// Wrapper that kills the child process on drop.
struct KillOnDrop(Option<Child>);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(ref mut c) = self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

struct ManagedBrowser {
    port: u16,
    child: KillOnDrop,
    client: reqwest::Client,
}

#[async_trait]
impl Tool for BrowserAutomationTool {
    fn name(&self) -> &str {
        "browser_automation"
    }

    fn description(&self) -> &str {
        "Managed Chrome automation via DevTools Protocol. Opens Zhongshu's own Chrome profile and can open pages, inspect DOM text, run JavaScript, click selectors, type into selectors, and read console messages captured after hooks are installed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open", "snapshot", "eval", "click", "type", "console", "wait", "scroll", "back", "forward", "new_tab", "press", "wait_for_selector", "select_option", "screenshot", "network_start", "network_events", "page_errors"]
                },
                "url": {"type": "string", "description": "URL for open"},
                "selector": {"type": "string", "description": "CSS selector for click/type"},
                "text": {"type": "string", "description": "Text for type"},
                "clear": {"type": "boolean", "description": "Clear existing value before typing", "default": false},
                "js": {"type": "string", "description": "JavaScript expression or async function body for eval"},
                "max_length": {"type": "integer", "description": "Max text length for snapshot/eval string output", "default": 8000},
                "ms": {"type": "integer", "description": "Milliseconds to wait", "default": 1000},
                "x": {"type": "integer", "description": "X position for scroll (pixels)"},
                "y": {"type": "integer", "description": "Y position for scroll (pixels)"}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, arguments: &Value) -> ToolOutput {
        match authority::check_tool("browser_automation") {
            CheckResult::Deny { reason } => {
                return ToolOutput::error(format!("[BLOCKED] {reason}"))
            }
            CheckResult::RequireAuth { request } => {
                authority::set_pending(&request.tool, &request.program, &request.command, "");
                return ToolOutput::auth_required(&request.program, &request.command);
            }
            CheckResult::Allow => {}
        }

        let action = match arguments["action"].as_str() {
            Some(a) => a,
            None => return ToolOutput::error("'action' must be a string"),
        };

        let result = match action {
            "open" => open_page(arguments).await,
            "snapshot" => snapshot(arguments).await,
            "eval" => eval_js(arguments).await,
            "click" => click(arguments).await,
            "type" => type_text(arguments).await,
            "console" => console_messages(arguments).await,
            "wait" => wait(arguments).await,
            "scroll" => eval_js(&json!({"js":"window.scrollBy(0, arguments[0] || window.innerHeight/2)","max_length":100})).await,
            "back" => eval_js(&json!({"js":"window.history.back()","max_length":100})).await,
            "forward" => eval_js(&json!({"js":"window.history.forward()","max_length":100})).await,
            "new_tab" => open_page(&json!({"url": arguments["url"].as_str().unwrap_or("about:blank")})).await,
            "press" => {
                let k = arguments["text"].as_str().unwrap_or("");
                eval_js(&json!({"js": format!("document.activeElement?.dispatchEvent(new KeyboardEvent('keydown',{{key:'{k}'}}));document.activeElement?.dispatchEvent(new KeyboardEvent('keyup',{{key:'{k}'}}))"),"max_length":100})).await
            },
            "wait_for_selector" => wait_for_selector(arguments).await,
            "select_option" => select_option(arguments).await,
            "network_start" => network_start(arguments).await,
            "network_events" => network_events(arguments).await,
            "page_errors" => page_errors(arguments).await,
            "screenshot" => screenshot(arguments).await,
            other => Err(anyhow::anyhow!(
                "unknown browser_automation action '{other}'"
            )),
        };

        match result {
            Ok(v) => ToolOutput::success(v),
            Err(e) => ToolOutput::error(e.to_string()),
        }
    }
}

async fn open_page(args: &Value) -> anyhow::Result<Value> {
    let url = args["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'url' must be a string"))?;
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.open_tab(url).await?;
    let _ = browser
        .command(&tab.websocket_url, "Page.enable", json!({}))
        .await;
    let _ = install_console_hook(browser, &tab.websocket_url).await;
    Ok(sanitize_external_value(json!({
        "action": "open",
        "url": tab.url,
        "title": tab.title,
        "tab_id": tab.id,
        "debug_port": browser.port,
        "profile_dir": profile_dir(),
    })))
}

async fn snapshot(args: &Value) -> anyhow::Result<Value> {
    let max_len = args["max_length"].as_u64().unwrap_or(8000).min(30000) as usize;
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let script = r#"
(() => {
  const visible = Array.from(document.querySelectorAll('a,button,input,textarea,select,[contenteditable="true"],[role="button"]'))
    .slice(0, 120)
    .map((el, i) => {
      const r = el.getBoundingClientRect();
      const label = (el.innerText || el.value || el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.name || el.id || '').trim().replace(/\s+/g, ' ');
      return {i, tag: el.tagName.toLowerCase(), selector: stableSelector(el), label, x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height)};
    });
  function stableSelector(el) {
    if (el.id) return '#' + CSS.escape(el.id);
    const name = el.getAttribute('name');
    if (name) return el.tagName.toLowerCase() + '[name="' + CSS.escape(name) + '"]';
    const aria = el.getAttribute('aria-label');
    if (aria) return el.tagName.toLowerCase() + '[aria-label="' + CSS.escape(aria) + '"]';
    let path = [];
    while (el && el.nodeType === 1 && path.length < 4) {
      let part = el.tagName.toLowerCase();
      let sib = el, nth = 1;
      while ((sib = sib.previousElementSibling)) if (sib.tagName === el.tagName) nth++;
      part += ':nth-of-type(' + nth + ')';
      path.unshift(part);
      el = el.parentElement;
    }
    return path.join(' > ');
  }
  return {url: location.href, title: document.title, text: document.body ? document.body.innerText : '', elements: visible};
})()
"#;
    let mut value = browser.evaluate(&tab.websocket_url, script, true).await?;
    if let Some(text) = value
        .get_mut("text")
        .and_then(|v| v.as_str().map(str::to_string))
    {
        if text.len() > max_len {
            value["text"] = Value::String(format!(
                "{}...\n\n[truncated to {max_len} chars]",
                &text[..text.floor_char_boundary(max_len)]
            ));
        }
    }
    Ok(sanitize_external_value(
        json!({"action": "snapshot", "tab_id": tab.id, "page": value}),
    ))
}

async fn eval_js(args: &Value) -> anyhow::Result<Value> {
    let js = args["js"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'js' must be a string"))?;
    let max_len = args["max_length"].as_u64().unwrap_or(8000).min(30000) as usize;
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let mut result = browser.evaluate(&tab.websocket_url, js, true).await?;
    if let Some(s) = result.as_str() {
        if s.len() > max_len {
            result = Value::String(format!(
                "{}...\n\n[truncated to {max_len} chars]",
                &s[..s.floor_char_boundary(max_len)]
            ));
        }
    }
    Ok(sanitize_external_value(
        json!({"action": "eval", "tab_id": tab.id, "result": result}),
    ))
}

async fn click(args: &Value) -> anyhow::Result<Value> {
    let selector = args["selector"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'selector' must be a string"))?;
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let script = format!(
        r#"
(() => {{
  const selector = {};
  const el = document.querySelector(selector);
  if (!el) throw new Error('selector not found: ' + selector);
  el.scrollIntoView({{block: 'center', inline: 'center'}});
  const r = el.getBoundingClientRect();
  const opts = {{bubbles: true, cancelable: true, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2}};
  el.dispatchEvent(new MouseEvent('mousedown', opts));
  el.dispatchEvent(new MouseEvent('mouseup', opts));
  el.click();
  return {{selector, tag: el.tagName.toLowerCase(), text: (el.innerText || el.value || '').trim().slice(0, 200)}};
}})()
"#,
        serde_json::to_string(selector)?
    );
    let result = browser.evaluate(&tab.websocket_url, &script, true).await?;
    Ok(sanitize_external_value(
        json!({"action": "click", "tab_id": tab.id, "result": result}),
    ))
}

async fn type_text(args: &Value) -> anyhow::Result<Value> {
    let selector = args["selector"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'selector' must be a string"))?;
    let text = args["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'text' must be a string"))?;
    let clear = args["clear"].as_bool().unwrap_or(false);
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let script = format!(
        r#"
(() => {{
  const selector = {};
  const text = {};
  const clear = {};
  const el = document.querySelector(selector);
  if (!el) throw new Error('selector not found: ' + selector);
  el.scrollIntoView({{block: 'center', inline: 'center'}});
  el.focus();
  if (el.isContentEditable) {{
    if (clear) el.textContent = '';
    el.textContent = (clear ? '' : el.textContent) + text;
  }} else {{
    if (clear) el.value = '';
    el.value = (clear ? '' : el.value) + text;
  }}
  el.dispatchEvent(new InputEvent('input', {{bubbles: true, inputType: 'insertText', data: text}}));
  el.dispatchEvent(new Event('change', {{bubbles: true}}));
  return {{selector, tag: el.tagName.toLowerCase(), value: (el.value || el.textContent || '').slice(0, 500)}};
}})()
"#,
        serde_json::to_string(selector)?,
        serde_json::to_string(text)?,
        clear
    );
    let result = browser.evaluate(&tab.websocket_url, &script, true).await?;
    Ok(sanitize_external_value(
        json!({"action": "type", "tab_id": tab.id, "result": result}),
    ))
}

async fn wait_for_selector(args: &Value) -> anyhow::Result<Value> {
    let selector = args["selector"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'selector' must be a string"))?;
    let timeout_ms = args["ms"].as_u64().unwrap_or(5000);
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let result = browser
        .evaluate(
            &tab.websocket_url,
            &format!(
                "new Promise((resolve,reject) => {{const el=document.querySelector('{}');if(el)return resolve(true);const t=setTimeout(()=>{{observer.disconnect();resolve(false)}},{});const observer=new MutationObserver(()=>{{if(document.querySelector('{}')){{clearTimeout(t);observer.disconnect();resolve(true)}}}});observer.observe(document.body,{{childList:true,subtree:true}})}})",
                selector.replace('\\', "\\\\").replace('\'', "\\'"),
                timeout_ms,
                selector.replace('\\', "\\\\").replace('\'', "\\'"),
            ),
            true,
        )
        .await?;
    Ok(sanitize_external_value(json!({
        "action": "wait_for_selector",
        "selector": selector,
        "found": result == "true" || result == "True",
    })))
}

async fn select_option(args: &Value) -> anyhow::Result<Value> {
    let selector = args["selector"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'selector' must be a string"))?;
    let value = args["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("'text' (option value/label) must be a string"))?;
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let escaped_sel = selector.replace('\\', "\\\\").replace('\'', "\\'");
    let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");
    let result = browser
        .evaluate(
            &tab.websocket_url,
            &format!(
                "(()=>{{const sel=document.querySelector('{sel}');if(!sel)return'not found';const opt=Array.from(sel.options).find(o=>o.value==='{val}'||o.text==='{val}');if(!opt)return'option not found';sel.value=opt.value;sel.dispatchEvent(new Event('change',{{bubbles:true}}));return opt.value}})()",
                sel = escaped_sel, val = escaped_val,
            ),
            true,
        )
        .await?;
    Ok(sanitize_external_value(json!({
        "action": "select_option",
        "selector": selector,
        "selected": result,
    })))
}

async fn screenshot(_args: &Value) -> anyhow::Result<Value> {
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let result = browser
        .command(
            &tab.websocket_url,
            "Page.captureScreenshot",
            json!({"format":"png"}),
        )
        .await?;
    let data = result["data"].as_str().unwrap_or("");
    let preview_len = data.len().min(200);
    Ok(sanitize_external_value(json!({
        "action": "screenshot",
        "tab_id": tab.id,
        "mime": "image/png",
        "data_length": data.len(),
        "base64_preview": &data[..preview_len],
    })))
}

async fn console_messages(_args: &Value) -> anyhow::Result<Value> {
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let _ = install_console_hook(browser, &tab.websocket_url).await;
    let result = browser
        .evaluate(
            &tab.websocket_url,
            "(() => window.__zhongshuConsole || [])()",
            true,
        )
        .await?;
    Ok(sanitize_external_value(
        json!({"action": "console", "tab_id": tab.id, "messages": result}),
    ))
}

async fn wait(args: &Value) -> anyhow::Result<Value> {
    let ms = args["ms"].as_u64().unwrap_or(1000).min(30_000);
    tokio::time::sleep(Duration::from_millis(ms)).await;
    Ok(json!({"action": "wait", "ms": ms}))
}

async fn ensure_browser() -> anyhow::Result<tokio::sync::MutexGuard<'static, Option<ManagedBrowser>>>
{
    {
        let mut guard = BROWSER.lock().await;
        let needs_start = match guard.as_mut() {
            Some(browser) => !browser.is_alive().await,
            None => true,
        };
        if needs_start {
            *guard = Some(ManagedBrowser::start().await?);
        }
    }
    Ok(BROWSER.lock().await)
}

impl ManagedBrowser {
    async fn start() -> anyhow::Result<Self> {
        let port = browser_port();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;

        if is_cdp_alive(&client, port).await {
            return Ok(ManagedBrowser {
                port,
                child: KillOnDrop(None),
                client,
            });
        }

        let chrome = find_chrome_executable()
            .ok_or_else(|| anyhow::anyhow!("Chrome/Chromium executable not found. Set ZHONGSHU_CHROME_BIN to the browser path."))?;
        let profile = profile_dir();
        std::fs::create_dir_all(&profile)?;

        let child_proc = Command::new(&chrome)
            .arg(format!("--remote-debugging-port={port}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--new-window")
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to start Chrome '{}': {e}", chrome.display()))?;

        for _ in 0..40 {
            if is_cdp_alive(&client, port).await {
                return Ok(ManagedBrowser {
                    port,
                    child: KillOnDrop(Some(child_proc)),
                    client,
                });
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        Err(anyhow::anyhow!(
            "Chrome started but DevTools did not become ready on 127.0.0.1:{port}"
        ))
    }

    async fn is_alive(&mut self) -> bool {
        if let Some(ref mut child) = self.child.0 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return false;
            }
        }
        is_cdp_alive(&self.client, self.port).await
    }

    async fn open_tab(&mut self, url: &str) -> anyhow::Result<TabInfo> {
        let escaped = percent_encode_url(url);
        let endpoint = format!("http://127.0.0.1:{}/json/new?{escaped}", self.port);
        let response = self.client.put(endpoint).send().await?;
        let text = response.text().await?;
        let tab = parse_tab(&text)?;
        Ok(tab)
    }

    async fn active_tab(&mut self) -> anyhow::Result<TabInfo> {
        let endpoint = format!("http://127.0.0.1:{}/json/list", self.port);
        let tabs: Vec<Value> = self.client.get(endpoint).send().await?.json().await?;
        tabs.into_iter()
            .filter(|t| t["type"].as_str() == Some("page"))
            .find_map(|t| parse_tab_value(&t).ok())
            .ok_or_else(|| anyhow::anyhow!("no Chrome page tab available; call action=open first"))
    }

    async fn evaluate(
        &mut self,
        ws_url: &str,
        expression: &str,
        await_promise: bool,
    ) -> anyhow::Result<Value> {
        let response = self
            .command(
                ws_url,
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": await_promise,
                    "returnByValue": true,
                    "userGesture": true,
                }),
            )
            .await?;
        if let Some(ex) = response.get("exceptionDetails") {
            return Err(anyhow::anyhow!("JavaScript exception: {ex}"));
        }
        Ok(response["result"]["result"]["value"].clone())
    }

    async fn command(
        &mut self,
        ws_url: &str,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let (mut ws, _) = tokio_tungstenite::connect_async(ws_url).await?;
        ws.send(Message::Text(
            json!({"id": id, "method": method, "params": params})
                .to_string()
                .into(),
        ))
        .await?;

        while let Some(msg) = ws.next().await {
            let msg = msg?;
            if !msg.is_text() {
                continue;
            }
            let value: Value = serde_json::from_str(msg.to_text()?)?;
            if value["id"].as_u64() == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(anyhow::anyhow!("CDP error for {method}: {error}"));
                }
                return Ok(value);
            }
        }
        Err(anyhow::anyhow!(
            "CDP connection closed before response for {method}"
        ))
    }
}

async fn install_console_hook(browser: &mut ManagedBrowser, ws_url: &str) -> anyhow::Result<()> {
    let script = r#"
(() => {
  if (window.__zhongshuConsoleInstalled) return true;
  window.__zhongshuConsoleInstalled = true;
  window.__zhongshuConsole = [];
  const push = (level, args) => {
    window.__zhongshuConsole.push({ts: Date.now(), level, text: Array.from(args).map(x => {
      try { return typeof x === 'string' ? x : JSON.stringify(x); } catch (_) { return String(x); }
    }).join(' ')});
    if (window.__zhongshuConsole.length > 300) window.__zhongshuConsole.shift();
  };
  for (const level of ['log','warn','error','info','debug']) {
    const original = console[level];
    console[level] = function(...args) { push(level, args); return original.apply(console, args); };
  }
  window.addEventListener('error', e => push('error', [e.message, e.filename + ':' + e.lineno]));
  window.addEventListener('unhandledrejection', e => push('error', ['unhandledrejection', e.reason]));
  return true;
})()
"#;
    let _ = browser
        .command(
            ws_url,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({"source": script}),
        )
        .await;
    let _ = browser.evaluate(ws_url, script, true).await?;
    Ok(())
}

#[derive(Debug)]
struct TabInfo {
    id: String,
    title: String,
    url: String,
    websocket_url: String,
}

fn parse_tab(text: &str) -> anyhow::Result<TabInfo> {
    let value: Value = serde_json::from_str(text)?;
    parse_tab_value(&value)
}

fn parse_tab_value(value: &Value) -> anyhow::Result<TabInfo> {
    Ok(TabInfo {
        id: value["id"].as_str().unwrap_or("").to_string(),
        title: value["title"].as_str().unwrap_or("").to_string(),
        url: value["url"].as_str().unwrap_or("").to_string(),
        websocket_url: value["webSocketDebuggerUrl"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("tab has no webSocketDebuggerUrl"))?
            .to_string(),
    })
}

async fn is_cdp_alive(client: &reqwest::Client, port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/json/version");
    client
        .get(url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn browser_port() -> u16 {
    std::env::var("ZHONGSHU_CHROME_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

fn profile_dir() -> PathBuf {
    if let Ok(path) = std::env::var("ZHONGSHU_CHROME_PROFILE") {
        return PathBuf::from(path);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/zhongshu/chrome-profile")
}

fn find_chrome_executable() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("ZHONGSHU_CHROME_BIN") {
        let p = PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "msedge",
    ];
    for name in names {
        if Command::new(name)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(PathBuf::from(name));
        }
    }
    None
}

fn percent_encode_url(url: &str) -> String {
    url.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

fn sanitize_external_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize_web_content(&s)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(sanitize_external_value).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, sanitize_external_value(value)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_preserves_url_syntax() {
        let encoded = percent_encode_url("https://example.com/a b?q=中书&x=1#frag");
        assert!(encoded.starts_with("https%3A%2F%2Fexample.com%2Fa%20b%3Fq%3D"));
        assert!(encoded.contains("%E4%B8%AD%E4%B9%A6"));
        assert!(encoded.ends_with("%26x%3D1%23frag"));
    }

    #[test]
    fn parse_tab_value_requires_websocket_url() {
        let value = json!({"id":"1","title":"T","url":"https://example.com","type":"page"});
        assert!(parse_tab_value(&value).is_err());
    }

    #[test]
    fn parse_tab_value_extracts_fields() {
        let value = json!({
            "id": "1",
            "title": "T",
            "url": "https://example.com",
            "webSocketDebuggerUrl": "ws://127.0.0.1/devtools/page/1"
        });
        let tab = parse_tab_value(&value).unwrap();
        assert_eq!(tab.id, "1");
        assert_eq!(tab.title, "T");
        assert_eq!(tab.url, "https://example.com");
        assert_eq!(tab.websocket_url, "ws://127.0.0.1/devtools/page/1");
    }

    #[test]
    fn sanitize_external_value_cleans_nested_browser_text() {
        let value = json!({
            "page": {
                "text": "hello\u{200B}\u{0000}world",
                "elements": [
                    {"label": "提交\u{200D}"}
                ]
            }
        });

        let cleaned = sanitize_external_value(value);

        assert_eq!(cleaned["page"]["text"], "helloworld");
        assert_eq!(cleaned["page"]["elements"][0]["label"], "提交");
    }
}

/// Classify browser action risk level.
fn action_risk(action: &str) -> &'static str {
    match action {
        "open" | "snapshot" | "eval" | "console" | "wait" | "scroll" | "screenshot" => "read",
        "click" | "type" | "press" | "select_option" | "wait_for_selector" => "interact",
        "new_tab" | "back" | "forward" => "navigate",
        _ => "unknown",
    }
}

async fn network_start(_args: &Value) -> anyhow::Result<Value> {
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    browser
        .command(&tab.websocket_url, "Network.enable", json!({}))
        .await?;
    let result = browser.evaluate(&tab.websocket_url,
        r#"if(!window.__zhongshuNetwork)window.__zhongshuNetwork=[];(()=>{const orig=fetch;window.fetch=function(){const args=arguments;return orig.apply(this,arguments).then(r=>{window.__zhongshuNetwork.push({url:args[0],status:r.status,ok:r.ok,time:Date.now()});return r}).catch(e=>{window.__zhongshuNetwork.push({url:args[0],error:e.message,time:Date.now()});throw e})}})();(()=>{const orig=XMLHttpRequest.prototype.open;XMLHttpRequest.prototype.open=function(){const url=arguments[1];this.addEventListener('loadend',function(){window.__zhongshuNetwork.push({url:url,status:this.status,ok:this.status>=200&&this.status<300,time:Date.now()})});return orig.apply(this,arguments)}})();true"#, true).await?;
    Ok(sanitize_external_value(
        json!({"action":"network_start","result":result}),
    ))
}

async fn network_events(_args: &Value) -> anyhow::Result<Value> {
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let result = browser.evaluate(&tab.websocket_url,
        r#"(()=>{const arr=window.__zhongshuNetwork||[];window.__zhongshuNetwork=[];return JSON.stringify(arr.slice(-50))})()"#, true).await?;
    Ok(sanitize_external_value(
        json!({"action":"network_events","events":result}),
    ))
}

async fn page_errors(_args: &Value) -> anyhow::Result<Value> {
    let mut browser = ensure_browser().await?;
    let browser = browser.as_mut().expect("browser initialized");
    let tab = browser.active_tab().await?;
    let result = browser.evaluate(&tab.websocket_url,
        r#"(()=>{const arr=window.__zhongshuErrors||[];window.__zhongshuErrors=[];try{window.onerror=(m,s,l,c,e)=>{window.__zhongshuErrors.push({msg:m,source:s,line:l,col:c,time:Date.now()})};window.addEventListener('unhandledrejection',e=>{window.__zhongshuErrors.push({msg:e.reason?.message||String(e.reason),type:'unhandledrejection',time:Date.now()})})}catch(e){}return JSON.stringify(arr.slice(-30))})()"#, true).await?;
    Ok(sanitize_external_value(
        json!({"action":"page_errors","errors":result}),
    ))
}
