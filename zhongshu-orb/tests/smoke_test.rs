use gtk::prelude::*;
use std::sync::Once;
use std::time::Duration;
use wry::WebViewBuilderExtUnix;

fn pump_gtk(ms: u64) {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(ms);
    while start.elapsed() < timeout {
        while glib::MainContext::default().iteration(false) {}
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Test that the chat HTML correctly renders messages via IPC.
#[test]
fn test_chat_html_renders_messages() {
    static GTK_INIT: Once = Once::new();
    GTK_INIT.call_once(|| {
        gtk::init().expect("GTK init failed");
    });

    let html = include_str!("../assets/chat.html");

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_default_size(520, 800);

    let webview = wry::WebViewBuilder::new()
        .with_html(html)
        .build_gtk(&window)
        .expect("WebView creation failed");

    // Let page load — pump GTK events
    pump_gtk(1000);

    // Step 1: Send history IPC
    webview.evaluate_script(
        r#"window.handleIpc(JSON.parse('{"type":"history","entries":[{"role":"user","content":"你好","tool_calls":[]},{"role":"assistant","content":"你好，说事。","tool_calls":[]}]}'));"#
    ).expect("history eval failed");
    pump_gtk(500);

    // Check JS state
    let (tx, rx) = std::sync::mpsc::channel();
    webview
        .evaluate_script_with_callback("JSON.stringify(messages.length)", move |r| {
            tx.send(r).ok();
        })
        .expect("check eval failed");
    pump_gtk(2000);
    let msg_count = rx.try_recv().unwrap_or_default();
    println!("messages.length = {msg_count}");
    // JS callback returns raw JSON string, e.g. '"2"' with quotes
    let msg_parsed: String = serde_json::from_str(&msg_count).unwrap_or_default();
    assert_eq!(msg_parsed, "2", "should have 2 messages, got '{msg_count}'");

    // Step 2: Stream a delta
    webview
        .evaluate_script(
            r#"
        window.handleIpc(JSON.parse('{"type":"state_change","state":"thinking"}'));
        window.handleIpc(JSON.parse('{"type":"delta","content":"这是一条流式消息"}'));
        window.handleIpc(JSON.parse('{"type":"complete"}'));
    "#,
        )
        .expect("delta eval failed");
    pump_gtk(500);

    // Check total messages
    let (tx2, rx2) = std::sync::mpsc::channel();
    webview
        .evaluate_script_with_callback("JSON.stringify(messages.length)", move |r| {
            tx2.send(r).ok();
        })
        .expect("check2 eval failed");
    pump_gtk(2000);
    let total = rx2.try_recv().unwrap_or_default();
    println!("total messages = {total}");
    let total_parsed: String = serde_json::from_str(&total).unwrap_or_default();
    assert_eq!(total_parsed, "3", "should have 3 messages, got '{total}'");

    // Check DOM rendered the streaming content
    let (tx3, rx3) = std::sync::mpsc::channel();
    let dom_check = r#"
        let chat = document.getElementById('chat');
        let allText = '';
        for (let i = 0; i < chat.children.length; i++) { allText += chat.children[i].textContent + ' | '; }
        JSON.stringify({children: chat.children.length, text: allText.substring(0,200)})
    "#;
    webview
        .evaluate_script_with_callback(dom_check, move |r| {
            tx3.send(r).ok();
        })
        .expect("dom check failed");
    pump_gtk(2000);
    let dom = rx3.try_recv().unwrap_or_default();
    println!("DOM: {dom}");
    assert!(dom.contains("流式"), "streaming text in DOM: {dom}");
    assert!(dom.contains("你好"), "history text in DOM: {dom}");

    // Step 3: Verify deltas from Rust-like JS (the actual IPC format)
    let delta_js = format!(
        "window.handleIpc(JSON.parse({}))",
        serde_json::to_string(&serde_json::json!({"type":"delta","content":"来自Rust的回复"}))
            .unwrap()
    );
    webview
        .evaluate_script(&delta_js)
        .expect("live delta eval failed");
    pump_gtk(2000);

    let (tx4, rx4) = std::sync::mpsc::channel();
    webview.evaluate_script_with_callback(
        "JSON.stringify({len:messages.length, last:messages[messages.length-1].content.substring(0,20)})",
        move |r| { tx4.send(r).ok(); },
    ).expect("check3 eval failed");
    pump_gtk(2000);
    let final_state = rx4.try_recv().unwrap_or_default();
    println!("Final: {final_state}");
    let final_parsed: serde_json::Value = serde_json::from_str(&final_state).unwrap_or_default();
    assert_eq!(final_parsed["len"], 4, "should now have 4 messages total");

    println!("SMOKE TEST PASSED");
    window.set_visible(false);
}
