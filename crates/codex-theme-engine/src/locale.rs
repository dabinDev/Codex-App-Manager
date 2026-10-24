//! Manager-owned Codex renderer localization payload.
//!
//! The official Codex catalog remains preferred. This payload is only used
//! when the installed app exposes `localeOverride` but does not enable its
//! renderer i18n provider for the current account.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::{Result, ThemeEngineError};

const RUNTIME_TEMPLATE: &str = include_str!("runtime/locale-runtime.js");

const ZH_CN_ENTRIES: &[(&str, &str)] = &[
    ("New task", "新建任务"),
    ("New chat", "新建对话"),
    ("Home", "首页"),
    ("Tasks", "任务"),
    ("Projects", "项目"),
    ("Skills", "技能"),
    ("Automations", "自动化"),
    ("Settings", "设置"),
    ("Open settings", "打开设置"),
    ("Account", "账户"),
    ("Profile", "个人资料"),
    ("Sign in", "登录"),
    ("Sign out", "退出登录"),
    ("Log in", "登录"),
    ("Continue", "继续"),
    ("Cancel", "取消"),
    ("Confirm", "确认"),
    ("Close", "关闭"),
    ("Done", "完成"),
    ("Save", "保存"),
    ("Delete", "删除"),
    ("Remove", "移除"),
    ("Rename", "重命名"),
    ("Edit", "编辑"),
    ("Retry", "重试"),
    ("Refresh", "刷新"),
    ("Back", "返回"),
    ("Next", "下一步"),
    ("Previous", "上一步"),
    ("Search", "搜索"),
    ("Search tasks", "搜索任务"),
    ("Search settings", "搜索设置"),
    ("No results", "没有结果"),
    ("Loading", "正在加载"),
    ("Loading...", "正在加载..."),
    ("Something went wrong", "出现问题"),
    ("Try again", "重试"),
    ("Copy", "复制"),
    ("Copied", "已复制"),
    ("Copy response", "复制回复"),
    ("Share", "分享"),
    ("Archive", "归档"),
    ("Unarchive", "取消归档"),
    ("Pin", "置顶"),
    ("Unpin", "取消置顶"),
    ("Open", "打开"),
    ("Open folder", "打开文件夹"),
    ("Open in editor", "在编辑器中打开"),
    ("Open in terminal", "在终端中打开"),
    ("Select folder", "选择文件夹"),
    ("Choose a folder", "选择文件夹"),
    ("Add folder", "添加文件夹"),
    ("Workspace", "工作区"),
    ("Worktree", "工作树"),
    ("Repository", "仓库"),
    ("Branch", "分支"),
    ("Changes", "更改"),
    ("Files", "文件"),
    ("Review", "审查"),
    ("Review changes", "审查更改"),
    ("Apply", "应用"),
    ("Apply changes", "应用更改"),
    ("Discard", "放弃"),
    ("Undo", "撤销"),
    ("Redo", "重做"),
    ("Run", "运行"),
    ("Stop", "停止"),
    ("Start", "开始"),
    ("Pause", "暂停"),
    ("Resume", "继续"),
    ("Terminal", "终端"),
    ("Command", "命令"),
    ("Output", "输出"),
    ("Details", "详情"),
    ("Show details", "显示详情"),
    ("Hide details", "隐藏详情"),
    ("Expand", "展开"),
    ("Collapse", "收起"),
    ("Send", "发送"),
    ("Submit", "提交"),
    ("Ask anything", "输入任何问题"),
    ("Describe a task", "描述一个任务"),
    ("Add context", "添加上下文"),
    ("Attach files", "附加文件"),
    ("Add image", "添加图片"),
    ("Voice", "语音"),
    ("Model", "模型"),
    ("Reasoning", "推理"),
    ("Low", "低"),
    ("Medium", "中"),
    ("High", "高"),
    ("Auto", "自动"),
    ("Local", "本地"),
    ("Cloud", "云端"),
    ("Environment", "环境"),
    ("Permissions", "权限"),
    ("Allow", "允许"),
    ("Deny", "拒绝"),
    ("Approve", "批准"),
    ("Always allow", "始终允许"),
    ("Allow once", "仅允许一次"),
    ("Keep waiting", "继续等待"),
    ("Notification", "通知"),
    ("Notifications", "通知"),
    ("Appearance", "外观"),
    ("Theme", "主题"),
    ("System", "跟随系统"),
    ("Light", "浅色"),
    ("Dark", "深色"),
    ("Language", "语言"),
    ("General", "通用"),
    ("Personalization", "个性化"),
    ("Integrations", "集成"),
    ("Tools", "工具"),
    ("Keyboard shortcuts", "键盘快捷键"),
    ("Updates", "更新"),
    ("Check for updates", "检查更新"),
    ("Update available", "有可用更新"),
    ("Install update", "安装更新"),
    ("Restart to update", "重启以更新"),
    ("About", "关于"),
    ("Version", "版本"),
    ("Help", "帮助"),
    ("Feedback", "反馈"),
    ("Documentation", "文档"),
    ("Privacy", "隐私"),
    ("Terms", "条款"),
    ("Enabled", "已启用"),
    ("Disabled", "已禁用"),
    ("Connected", "已连接"),
    ("Disconnected", "未连接"),
    ("Online", "在线"),
    ("Offline", "离线"),
    ("Today", "今天"),
    ("Yesterday", "昨天"),
    ("Just now", "刚刚"),
    ("Learn more", "了解更多"),
];

#[derive(Debug, Clone)]
pub struct LocalePayload {
    pub payload: String,
    pub stamp: String,
    pub catalog_entries: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocaleRuntimeReport {
    pub version: u32,
    pub stamp: String,
    pub observer_active: bool,
    pub catalog_entries: usize,
    pub translated_nodes: usize,
}

pub fn validate_runtime_report(
    payload: &LocalePayload,
    report: &LocaleRuntimeReport,
) -> Result<()> {
    if report.version != 1
        || report.stamp != payload.stamp
        || !report.observer_active
        || report.catalog_entries != payload.catalog_entries
    {
        return Err(ThemeEngineError::Cdp(
            "codex-locale-runtime-install-failed: runtime verification mismatch".to_string(),
        ));
    }
    if report.translated_nodes == 0 {
        return Err(ThemeEngineError::Cdp(
            "codex-locale-zero-translated-ui: compatibility runtime translated no visible UI"
                .to_string(),
        ));
    }
    Ok(())
}

pub fn build_zh_cn_payload() -> Result<LocalePayload> {
    let catalog: BTreeMap<&str, &str> = ZH_CN_ENTRIES.iter().copied().collect();
    let catalog_json = serde_json::to_string(&catalog)
        .map_err(|error| ThemeEngineError::Theme(format!("locale catalog serialize: {error}")))?;
    let mut hasher = Sha1::new();
    hasher.update(RUNTIME_TEMPLATE.as_bytes());
    hasher.update(catalog_json.as_bytes());
    let digest = hasher.finalize();
    let short = digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let stamp = format!("cam-locale-v1:zh-CN:{short}");
    let stamp_json = serde_json::to_string(&stamp)
        .map_err(|error| ThemeEngineError::Theme(format!("locale stamp serialize: {error}")))?;
    let payload = RUNTIME_TEMPLATE
        .replace("__CAM_LOCALE_CATALOG__", &catalog_json)
        .replace("__CAM_LOCALE_STAMP__", &stamp_json);

    Ok(LocalePayload {
        payload,
        stamp,
        catalog_entries: catalog.len(),
    })
}

pub const CURRENT_LOCALE_REPORT_EXPRESSION: &str =
    r#"window.__camLocaleRuntimeV1?.report?.() ?? null"#;

pub const REMOVE_LOCALE_EXPRESSION: &str = r#"(() => {
  const runtime = window.__camLocaleRuntimeV1;
  if (runtime?.remove) runtime.remove();
  else document.documentElement?.removeAttribute("data-cam-locale");
  return !window.__camLocaleRuntimeV1;
})()"#;
