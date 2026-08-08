//! 端到端冒烟:验证 nuphus 桌面工具在文件桥环境真实可用。
//! 需要交互式桌面会话;非交互环境(CI/无头)会失败,标记 #[ignore]。

#[tokio::test]
#[ignore]
async fn desktop_tools_smoke() {
    let size = nuphus_mcp_core::tools::execute("desktop_screen_size", &serde_json::json!({}))
        .await
        .expect("execute");
    println!("screen_size: {}", size.text);
    assert!(!size.is_error, "screen_size failed: {}", size.text);

    let windows = nuphus_mcp_core::tools::execute("desktop_windows_list", &serde_json::json!({}))
        .await
        .expect("execute");
    println!("windows_list: {}", windows.text);
    assert!(!windows.is_error, "windows_list failed: {}", windows.text);
}
