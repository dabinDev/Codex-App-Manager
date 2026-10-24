# AGENTS.md — Agent 作业指南

Codex App Manager:官方 OpenAI Codex 桌面应用的安装/更新/卸载管理器(Tauri v2)。
前端 `src/`,Rust 引擎 `crates/` + `src-tauri/`,分发 Worker `cloudflare/`,官网 `website/`。

## 品牌配置与本地双品牌包

- 应用名称、多语言显示名、前端/安装包图标和内置站点唯一来源是根目录
  `brand.config.json`;修改后运行 `npm run brand:sync`,并用 `npm run check` 验证派生的
  `src-tauri/tauri.conf.json` 已同步。
- 当前需要维护的两个本地品牌包:

  | 品牌 | `productName` | `serviceName` | 登录/API Base URL |
  | --- | --- | --- | --- |
  | 酸橘子 | `酸橘子安装器` | `token.cylonai.cn · OpenAI` | `https://token.cylonai.cn` |
  | 烂番茄 | `烂番茄安装器` | `tomato.beinai.cc · OpenAI` | `https://tomato.beinai.cc` |

- 两个品牌的明暗前端 Logo 都使用 `src/app/assets/orange-logo.png`,Windows 安装包使用现有
  酸橘子 `icons/icon.ico` 与 NSIS 图片资源。
- 换牌不得修改 `mainBinaryName`、bundle `identifier`、Cargo/package 名称、持久化 key 或锁文件名。
  两个品牌因此共享 `io.github.wangnov.codexappmanager`,不支持在同一台电脑并行安装;后安装版本
  可能覆盖或接管同一应用身份。
- 双品牌打包必须在基于同一 `main` 提交的隔离工作树中完成,品牌配置只写入对应工作树,不得把
  临时品牌值或真实凭据写回 `main`,也不得在验证时改写本机真实 Codex 配置。

### Windows LLVM-MinGW 备用打包

当本机 MSVC 被应用控制策略阻止而改用 WSL + LLVM-MinGW 交叉编译时,必须遵守以下规则:

1. Rust 目标使用 `x86_64-pc-windows-gnullvm`,生产编译必须显式启用
   `--features tauri/custom-protocol`;直接运行不带该特性的 `cargo build --release` 会生成访问
   `http://127.0.0.1:1420` 的开发模式程序。
2. `tauri bundle` 必须传入与编译相同的 `--features tauri/custom-protocol`。
3. LLVM-MinGW 产物动态依赖 `libunwind.dll` 和 `WebView2Loader.dll`;两者必须使用 x64 版本,
   并通过 Tauri `bundle.resources` 映射到安装根目录,与 `codex-app-manager.exe` 同级。只把 DLL
   留在 Cargo target 目录不算完成。
4. 不要以二进制中是否出现 `127.0.0.1:1420` 判断生产模式:生产包仍可能包含来自 `devUrl` /
   `devCsp` 的静态配置字符串。应使用 `llvm-strings` 确认程序内嵌 `/index.html`、品牌资源和对应
   服务 URL,并检查生成的 `installer.nsi` 含 `/oname=WebView2Loader.dll` 与
   `/oname=libunwind.dll`。
5. 只做静态验证时不得运行或安装生成的 EXE。覆盖桌面同名包前先备份,复制后比较源文件、归档
   和桌面文件的 SHA-256。当前归档根目录为
   `E:\ForkProject\BuildArtifacts\Codex-App-Manager\<commit>`。

## 质量门与提交

- 本仓库是个人 fork,所有分支(包括 `main`)都允许直接提交和推送;任何分支都禁止 force push。
- 实质代码改动的收尾链路:`codex review --uncommitted`(或 `--base main`)**迭代到无意见** → 完整测试 → conventional commit → `git push`。
- PR 仅在需要协作评审时使用,不是提交或发版的强制步骤;远端若拒绝直接推送,报告实际保护规则,不要用 force push 绕过。
- 提交信息用英文 conventional 风格(`feat:` / `fix:` / `docs:` / `chore(release):`)。

## 发版流程(tag 驱动)

1. **bump 版本号,5 个文件 6 处**:`package.json`、`package-lock.json`(顶层 + `packages[""]` 两处)、
   `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`(只改
   `codex-app-manager` 那个 `[[package]]` 块——⚠️ lock 里 `winapi-util` 等依赖也是 `0.1.x`,
   **严禁全局替换**)。
2. **同一个发版提交里写 release note**:新增 `docs/releases/v<X.Y.Z>.md`。
   - 结构与双语风格照 `docs/releases/TEMPLATE.md`(banner 头图 → 一句话双语主旨 →
     ✨ 亮点 / 🐛 修复 双语成对行 → 安装升级表)。
   - 素材来源:`git log <上个tag>..HEAD` + **读关键 diff 还原细节**(squash 提交一句话
     背后常有数百行变更);只写用户可感知的变化,内部重构不写。
   - **事实核查**:渠道、数字必须核实——例:winget(`Wangnov.CodexAppManager`)在
     microsoft/winget-pkgs 合并前**不存在**,不要写 `winget install`。
   - 安装表、镜像直链、更新器工件说明是模板固化内容,不要改写;Full Changelog 由
     release.yml 自动追加,不要手写。
3. 提交信息 `chore(release): bump version to <X.Y.Z>`,直接推送到 `main`,并等待 Frontend + Rust
   检查通过。PR 专用的 `nsis` 检查不会因直接推送触发;发版前需在本机完成现有 NSIS 打包验证。
4. 检查通过后,在 main HEAD 打 annotated tag `v<X.Y.Z>` 并 push。`release.yml`
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
