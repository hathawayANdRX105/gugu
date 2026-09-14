# gugu（咕咕）工作约定

轻量 QQ 聊天客户端：Slint（软件渲染）UI + ricq（QQ 协议）。立项动机与整体路线见
`todo/ROADMAP.md`（gitignored，本地维护）。

## 开工前（按顺序读）

1. 读根 `AGENTS.md`（本文件）。
2. 读 `todo/ROADMAP.md`，确认当前里程碑（M0–M4）与本次任务的边界。
3. 参考仓库（只读，勿改动）：`~/projects/ricq`（协议层）、`~/projects/revite`（UI 参考）、
   `~/projects/fractal`（Rust IM 工程结构）。

本文件按阅读优先级排布：开工动作 → 目录 → Slint 约定 → 安全约定 → 硬约束。

## 开发方式

- 单目录单体项目（暂无 workspace/worktree）：直接在仓库根开发，改动保持手术式——
  每行改动都能追溯到 ROADMAP 的某个条目。
- 提交用 conventional commits（`feat:` / `fix:` / `chore:` / `docs:`），一次提交单一主题。
- 借鉴参考仓库时**只抄思路和结构**，不整文件复制（许可证与代码风格都不兼容）。

## 目录术语

| 术语 | 位置 | 含义 |
|---|---|---|
| **UI** | `ui.slint` | 全部 Slint 声明式界面；组件化用 `component`，数据用 `struct` + `in property` |
| **逻辑** | `src/main.rs` | 入口 + 状态管理 + GIF 帧播放器；协议桥接在 `src/onebot.rs` |
| **静态资源** | `assets/` | 测试贴纸与图标；运行时加载，不嵌进二进制 |
| **路线** | `todo/` | 实现路线与阶段文档（gitignored，不持久化） |
| **构建产物** | `target/` | gitignored；可随时删除重建 |

`src/` 按模块拆分（已落地：`onebot.rs` = OneBot v11 传输与桥接、`stickers.rs` = GIF 解帧
与贴纸包扫描），`main.rs` 只留入口、UI 接线与组装。

## Slint 约定（踩过坑的，直接遵守）

- `.slint` 表达式**没有** `substring`/`charAt`/`%`（用 `Math.mod`）：一切文本处理在 Rust
  侧算好放进 struct（如头像首字母 `initial`）。
- Slint **无原生 GIF 动画**（上游 #2081）：动画 = `image` crate 解帧 + `slint::Timer`
  换帧。帧通过 struct 的 `image` 字段传递，`set_row_data` 驱动。
- 组件属性名以 `~/projects/ricq` 之外的实际编译报错为准：`Button` 用 `text` 不是 `title`；
  数组类型写 `[color]` 不是 `color[8]`。
- **UI 线程外严禁直接碰 slint 状态**：协议/IO 事件一律经
  `slint::invoke_from_event_loop` 或回调桥接进主线程。
- smoke 验证 = 启动进程 + `grim` 截图 + 帧差对比（参考 M0 做法）；不引入截图以外的
  UI 测试框架。

## 安全与环境约定

### 文件删除与清理保护（硬约束）

1. **严禁 `git clean`**；**严禁 `rm` / `rm -rf` / `rmdir` / `unlink`**。
2. 唯一合规方式：`gio trash <path>`（进 FreeDesktop 回收站，可恢复）。
   找回：读 `~/.local/share/Trash/info/<name>.trashinfo` 取原路径，从 `files/` 移回。
3. git 跟踪文件的移除用 `git rm`。

### 密钥与账号（硬约束）

- 禁止提交：QQ 号、会话 token、设备信息、密码、真实网络抓包。
- 会话凭据落盘放 `data/`（gitignored）；示例用占位符。
- ricq 是逆向协议，**有封号风险**：开发与测试一律用小号，禁止主号登录。

### cpulimit（硬约束）

- CPU-heavy 命令必须套 `cpulimit -l 70 -i --`：`cargo build` / `cargo test` /
  `cargo clippy` / `npm` 等；`git`、`grep`、文件读写等轻量命令不需要。

### 测试分层

- 本地只跑 `cargo check`（验证编译）；不本地跑全量构建与重型测试。
- 行为验证用 smoke：启动真实二进制 + 截图 + 帧差；协议层（M2 起）用小号真发真收。
- GIF/图片处理的边界情况（disposal、0 delay、损坏文件）在 `src` 内联单测或
  `tests/` 覆盖，跑得快才允许本地跑。

### gate（`.githooks/`）

- 已接线 `core.hooksPath .githooks/hooks`，gate 二进制随仓库分发（684K，自包含）。
- pre-commit 拦截信息逐条读完再修根因；禁止 `--no-verify`、禁止截断输出忽略。
- FAIL 条目（`checklist.*` 格式）必须清零；WARN 说明理由后可放行。
- spec 中 ferrite 专属规则若对 gugu 误报，删对应 spec 文件并在提交说明里记录。

## Rust 编码风格

- 函数动宾命名：`load_gif_frames` 而非 `do_gif`；类型名说清角色。
- 公共函数写 `///` doc：用途、参数语义、错误情况；`src/main.rs` 顶部写模块级 `//!`。
- 占位用原生宏：未实现写 `todo!("TODO(#<issue>): 说明")`；TODO/FIXME 注释必须带 issue 号。
- 消息/状态结构体字段命名与 ricq 侧概念对齐（author/body/timestamp），别发明第二套词。

## 硬约束汇总

1. 内存是立项动机：新增常驻内存结构（缓存、模型、图片）必须说明预算；
   总目标 USS <100MB（见 ROADMAP M4）。
2. `assets/` 运行时加载，不 `@image-url` 嵌入大图进二进制。
3. 参考仓库只读；不把 revite/fractal 的文件拷进本仓库。
4. `todo/` 不持久化（gitignored）；路线变更直接改本地文档。
