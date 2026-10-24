# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Codex App Manager:官方 OpenAI Codex 桌面应用的安装/更新/卸载管理器(Tauri v2)。
前端 `src/`,Rust 外壳 `src-tauri/` + 三个纯逻辑引擎 `crates/`,分发 Worker `cloudflare/`,官网 `website/`。
它消费上游 [codex-app-mirror](https://github.com/Wangnov/codex-app-mirror) 的镜像与 Sparkle 更新源来管理 Codex 本体,自身也经国内可达的镜像自更新;不构建、不修改、不重签 Codex。

## 常用命令(仓库根目录)

- `npm run tauri:dev` — 本地跑整个应用(会先 `brand:sync`);`npm run dev` 只起 Vite(`127.0.0.1:1420`,strictPort)。
- `npm run check` — 类型门:`brand:check` + `tsc --noEmit`(改了 `brand.config.json` 却没 sync 会在这里失败)。
- `npm run lint` — `eslint . --max-warnings=0`(零警告)。
- `npm run test` — Vitest 全量(jsdom)。**单测**:`npx vitest run src/app/views/Home.test.tsx`,或按名 `npx vitest run -t "<pattern>"`;监视 `npx vitest`(测试里从 `vitest` 显式 import,`globals:false`)。
- `npm run build` — `brand:check` + `tsc` + `vite build` + `check-renderer-entry`,产出 `dist/`。
- `npm run tauri:build` — 本地(未签名)桌面包。
- `npm run test:release` — 发版/镜像脚本的 Node 测试(`scripts/*.test.mjs`)。

## Rust 构建与测试(关键:不是一个 Cargo workspace)

`src-tauri` 与三个 `crates/*` 是**四个各自独立、各带 `Cargo.lock` 的包**,靠 path 依赖相连。所以 `src-tauri` 的
`--workspace` 覆盖不到引擎 crate 的测试,CI 会分别显式跑每个包——本地要复现 Rust 门也必须逐包跑:

- 编译前先 `npm run build`:`tauri-build` 在编译期读取 `../dist`,没有 `dist/` 会失败。
- `cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets -- -D warnings`
- `cargo test --manifest-path src-tauri/Cargo.toml --workspace`
- 三个引擎单独跑:`cargo test --manifest-path crates/codex-mac-engine/Cargo.toml --all-targets`(win / theme 同理)。
- **单个 Rust 测试**:`cargo test --manifest-path src-tauri/Cargo.toml <name>`。
- 引擎是纯逻辑、可跨平台跑;`src-tauri` 有大量 `#[cfg(target_os=...)]` 分支,mac/win 专属代码需在对应系统上验证(CI Rust 矩阵 = macos + windows)。

## Cloudflare Worker(`cloudflare/manager-download-router`)

独立 npm 工程(自带 `package-lock.json`),负责 `/manager/*` 下载按地域选路(R2 全球 / IHEP 中国大陆)。
`cd cloudflare/manager-download-router && npm ci && npm test`。CI 仅在 `cloudflare/**` 变更时跑其测试。

## 架构总览(需读多文件才能拼出的大图)

**前端 `src/`**(React 19 · TS · Vite · GSAP):`main.tsx` → `app/bootstrap.tsx` → `app/App.tsx`;各屏在
`app/views/`(Home / WinHome / Settings / About / CodexConfig / CodexThemes / Toolbox / Uninstall / apiConfig)。
i18n 11 语言 + RTL 在 `app/i18n.tsx`;设计系统集中在 `app/styles.css`——所有视觉/设计决策以根目录
`.impeccable.md` 的 Design Context 为准(温润材质、双主题对等、离线不引入 web 字体、固定 400×640 无边框窗口)。

**IPC 契约(改一端必同步另一端)**:`src/services/managerApi.ts` 是唯一 `invoke` Tauri 命令的入口;
`src/shared/types.ts` 逐字段镜像 Rust 的 serde(camelCase)负载,并写清了每个字段的语义。命令按平台/域命名:
`mac_*`、`win_*`、`codex_theme_*`、`api_config_*`(OrangeAPI 登录/取 key,密钥只留在 Rust)、
`toolbox_alignment_*`,操作租约 `begin_operation`/`arm_destructive`/`end_operation`/`get_operation_snapshot`,
以及 Manager 自更新 `manager_check_update`/`manager_install_update`。全部命令在 `src-tauri/src/lib.rs` 的
`generate_handler!` 注册。

**Rust 外壳/应用层 `src-tauri/src/`**(分层清晰):

- `lib.rs` — Tauri builder + 外壳安全:自建主窗、导航白名单(只允许打包 origin,拦截跳转/新窗)、退出与关闭的
  **相位感知策略**、前端就绪握手(generation + token,10s 超时进 native 降级弹窗)、单实例、macOS 原生菜单、
  Windows WebView2 浏览器加速键禁用与显示安全门、启动崩溃恢复(install_tx + staging + theme,顺序有讲究)。
- `commands.rs` — `#[tauri::command]` 层,薄封装,向下调 `app/`。
- `app/` — 应用服务:`mac_update` / `win_update`(平台更新引擎的编排层)、`codex_theme`、`orange_*`(OrangeAPI 会话)、
  `install_tx`(崩溃安全的安装事务)、`staging`、`provenance`、`settings_store`、`config_health`、
  `conversation_alignment`(Toolbox)、`oplock`(`OperationManager`:单飞操作租约 + 相位机 + `operation.lock` 文件锁)、
  `op_phase`(`QuitPolicy`)、`atomic_file`、`url_guard`、`brand`(用 `env!` 读品牌常量)。
- `domain/`(纯类型:manifest / settings / target)、`adapters/host.rs`(平台适配)、`state.rs`(`ManagerState` 托管态)、
  `errors.rs`(`AppError` → `CommandError`,camelCase `code`+`message`;`stale_expectation` = 现实已不符用户确认过的快照,需重查重确认)。

**引擎 `crates/`**(纯逻辑、平台特定、可独立测试):`codex-mac-engine`(Sparkle appcast 解析 + delta 感知的 macOS
更新规划,ed25519 字节级验签)、`codex-win-engine`(Windows MSIX 更新规划/暂存,sha256/zip)、
`codex-theme-engine`(资产化换肤,经 loopback CDP/WebSocket 注入,不改任何应用文件、签名完好)。

**操作模型**:任一时刻只有一个操作租约,相位 `idle→preparing→downloading→verifying→applying→committing→finishing`;
`committing` 是不可逆点,期间退出被阻断(native 兜底弹窗也不能穿透);渲染层重载后用 `get_operation_snapshot` 重新挂接。

**品牌是编译期注入**:`brand.config.json` → `npm run brand:sync` → `BRAND_*` 环境变量;Vite 经
`import.meta.env.VITE_BRAND_*` 读,Rust 经 `build.rs`(dotenvy)+ `app/brand.rs` 的 `env!` 读。这也是为什么
`dist/` 必须先于 cargo 构建、且换牌只能改 `brand.config.json` 一处。

## 品牌配置

- 应用名称、多语言显示名、前端/安装包图标和内置站点唯一来源是根目录 `brand.config.json`。
- 修改后运行 `npm run brand:sync`;`npm run check` 会拒绝未同步的 `src-tauri/tauri.conf.json`。
- 可见换牌不得顺带修改 `mainBinaryName`、bundle `identifier`、Cargo/package 名称、持久化 key 或锁文件名。

## 质量门与合并

- main 受保护:禁 force push、必须走 PR,必需检查 = Frontend + Rust (macos/windows)。
- 实质代码改动的收尾链路:`codex review --uncommitted`(或 `--base main`)**迭代到无意见** → PR → `gh pr merge --squash`。
- 提交信息用英文 conventional 风格(`feat:` / `fix:` / `docs:` / `chore(release):`)。

## 发版流程(tag 驱动)

1. **bump 版本号,5 个文件 6 处**:`package.json`、`package-lock.json`(顶层 + `packages[""]` 两处)、
   `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`(只改
   `codex-app-manager` 那个 `[[package]]` 块——⚠️ lock 里 `winapi-util` 等依赖也是 `0.1.x`,
   **严禁全局替换**)。
2. **同一个发版 PR 里写 release note**:新增 `docs/releases/v<X.Y.Z>.md`。
   - 结构与双语风格照 `docs/releases/TEMPLATE.md`(banner 头图 → 一句话双语主旨 →
     ✨ 亮点 / 🐛 修复 双语成对行 → 安装升级表)。
   - 素材来源:`git log <上个tag>..HEAD` + **读关键 diff 还原细节**(squash 提交一句话
     背后常有数百行变更);只写用户可感知的变化,内部重构不写。
   - **事实核查**:渠道、数字必须核实——例:winget(`Wangnov.CodexAppManager`)在
     microsoft/winget-pkgs 合并前**不存在**,不要写 `winget install`。
   - 安装表、镜像直链、更新器工件说明是模板固化内容,不要改写;Full Changelog 由
     release.yml 自动追加,不要手写。
3. PR 标题 `chore(release): bump version to <X.Y.Z>`。bump PR 会额外触发**非必需**的
   `nsis` 检查(~8min,期间 PR 状态 UNSTABLE)——等它也绿再合并。
4. squash 合并后,在 main HEAD 打 annotated tag `v<X.Y.Z>` 并 push。`release.yml`
   自动:四平台构建(macOS arm64/x64 + Windows x64/ARM64)→ 签名/公证 → R2 + IHEP 镜像同步(失败会阻断发布)→ GitHub
   Release(正文取 `docs/releases/<tag>.md`,缺失回退 `FALLBACK.md` 并告警,同时
   追加自动 What's Changed)。
5. 发布后会 dispatch `winget.yml`;`Wangnov.CodexAppManager` 已在 winget-pkgs 中可用,
   workflow 失败通常需要处理生成的提交/PR。README 下载链接是 `/manager/latest/*` 永久直链,
   发版**通常无需**改 README。

## website/ 子工程(官网,codexapp.agentsmirror.com)

- 独立 npm 工程,**不要在仓库根目录为它装依赖**(会污染主工程 package.json)。
- 文案唯一来源 `website/src/locales/{zh,en}.ts`;改文案后重跑 `npm run fonts`
  (中文显示字体按用字子集化,缺字会回退系统字体)。
- 素材管线:`assets/raw/`(git-ignored,AI 生成)→ `npm run images` → `public/img/`。
- 部署:`cd website && npm run build && npx wrangler deploy`。zone 路由:
  `/manager/*` → 本仓库下载路由器,`/latest/*` → mirror 仓库,`/*` → 官网,互不抢路。
- README banner(`assets/banner.svg`)由 `node website/scripts/readme-banner.mjs`
  再生;官网视觉资产更新后记得重新生成。
