<!-- canon: hathawayANdRX105/canon @ 14f6f78 (synced 2026-09-23) -->
# 功能开发任务书指南

> 日期 2026-09-16。基线 main `235fcf1`。
> 用途：主控 agent 写任务书、子代理执行、主控验收，三方共用一份质量守则。
> 定位：**不是 workflow**。workflow 讲流程顺序（canon `manual/pr-dev-workflow.md`；收尾全流程见 canon `tasks/closeout.md`），本文讲**每个功能任务必须回答清楚的问题**，以及答案不合格长什么样。
> 强制规则见 `.githooks/spec/docs/SPEC_OVERVIEW.md`（`gate init` 播种副本；正本 canon `rules/docs/`，总手册 canon `manual/gate.md`），本文不重复规则，只讲怎么把规则落进任务书。

## 怎么用

| 角色 | 动作 |
|---|---|
| 主控 | 开工前填 §0 §1 §3 §5 §8 §9，产出任务书文件 `todo/handoff/<topic>.md`，派给子代理 |
| 子代理 | 先答 §2(调查)，再实现，交回执时必须逐条给 §4 §6 §7 的证据 |
| 主控验收 | 用文末"验收回执"核对；任何一格空着 = 打回，不是"大概过了" |

任务书缺哪一节，子代理就会在那一节自由发挥。**七问全答满才派工。**

---

## §0 出发前：基线与禁区

先钉死起点，否则事后无法判断"这条改动是谁引入的"。

必须写明：

- **基线 commit**：`git rev-parse --short origin/main` 的输出，写进任务书
- **worktree**：`git worktree add .wt/<issue编号>-<分支名> -b <分支名> origin/main`（硬规则见 canon `manual/worktree.md`；子代理禁止在根工作树改文件）
- **图基线**：`cg status` 确认 graph 的 commit 与基线一致，不一致先 `cg refresh`
- **禁改区**：`.githooks/` 全域(gate 规则归 gate 所有者)。例外只有用户显式指派

不合格：任务书写"基于最新 main"——两小时后 main 已经前进两个 commit，验收时无从对齐。

---

## §1 在什么地方

范围 = **文件级白名单 + 明确的非目标**。

要答：

- 每个要改的文件路径，逐个列出，标注"改什么"还是"新建"
- 新建文件的落位：Rust 测试**必须**放同层 `tests/`(checklist `rust_tests_in_tests_dir` 是 FAIL 硬门槛，src/ 留测试直接被拒)
- 非目标：明确写"不动 X"。跨 crate 顺手改是 diff 失焦的头号来源

合格形状：

```
- crates/composition/src/lib.rs        改：assemble() 注册两个插件
- crates/composition/Cargo.toml        改：加 compaction/instruction 依赖
- crates/composition/tests/assemble.rs 新建：注册顺序 + 重名拒绝
- 不动：crates/infra/daemon/src/server.rs(装配调用方下一个 PR 处理)
```

不合格："在 composition 相关代码里实现装配"。子代理会顺手改 daemon、改 orbit、改 Cargo.lock 里不相干的行。

查法：`cg summary <file>` 看文件里有什么；`cg importers <module>` 看谁依赖它，判断范围会不会外溢。

---

## §2 参考什么内容，技术栈，思路

这一节是**子代理的第一份产出**，不是主控替它写。要求：先调查，再动手；调查结论写进回执。

必答四项：

1. **参考实现**：仓内既有同类代码的路径 + 行号。dsh 复刻类任务额外给 dsh 侧路径(见 [todo/dsh/README.md](./dsh/README.md))
2. **技术栈约束**：能用哪些既有依赖，禁止新增什么。新依赖必须在任务书里预先批准，理由写清楚
3. **调用面**：改动符号的全部调用者，逐个说明"改 / 不改 / 为什么不用改"
4. **思路**：三到五行说清做法，以及为什么不选另一种

codegraph 用法(**结构问题走图，文本确认走 grep，两者都要，不许只用一个**)：

```bash
cg status                      # 图是否新鲜；旧了先 cg refresh
cg search "<自然语言>"          # 不知道符号名时先搜
cg callers <symbol>            # 改共享代码前必跑：全部调用者
cg callees <symbol>            # 这个函数依赖谁
cg tests <symbol>              # 已有哪些测试覆盖它
cg walk <query> 3              # 陌生子系统 BFS
cg summary <file>              # 文件里有什么
cg changes origin/main         # 当前 diff 的影响面 + risk 分
```

铁律：**`cg callers` 说有 5 个调用者，就要 grep 到那 5 行真实代码再决定改不改。** 图会漏动态分派，grep 会把注释里的同名词算进来，单用任何一个都会错。

不合格：回执写"参考了 orbit 的实现"不带路径行号；或跳过 `cg callers` 直接改共享函数，导致兄弟调用点仍是坏的。

---

## §3 实现什么功能

按**可观察行为**写，不按内部结构写。

要答：

- 新增/修改的公开签名，逐个写全(`pub fn foo(a: &A) -> Result<B, E>`)
- 行为契约：输入什么、输出什么、错误路径返回什么
- 边界：为空、超限、并发、失败重入时的行为
- 明确不做：延后的能力写"不做 X，理由 Y"，不留想象空间

合格形状：给出签名 + 一句行为契约 + 一条边界。

不合格："按方案实现装配""优化一下压缩逻辑"。子代理拿到这种描述只能猜，猜出来的东西验收时对不上。

规模红线：单文件 1500 行 / 35KB(checklist `file_size`)。实现到一半发现要突破，先停下报告，不要闷头写完再拆。

---

## §4 补充什么测试

**测试是长期负担，不是干活证明。** 判据只有一条：**一个真实可能发生的 bug 会让它红。**

该写：

- 行为契约、边界值、不变式、状态迁移、优先级、真实错误路径
- Bug fix 的复现测试：修复前红、修复后绿

不该写(写了要删)：

- 字段拷贝、默认值、参数转发、mock 回声、断言源码文本
- 同一条路径换参数刷行数、恒真断言、只判"不 panic"、只判"非空"
- 为了"让改动有测试"而写的测试 —— 这种情况改用一次性脚本，跑完删掉

硬门槛：

- 测试文件放同层 `tests/`，src/ 内不留(FAIL)
- 测试函数必须含 assert(checklist `rust_test_no_assert`)
- `tests/` 里不写 `#[cfg(test)]`

既有测试如果只钉住措辞、实现细节、无关默认值，**删掉**，不要改成钉新措辞。这属于本次范围，不管是谁写的。

要在回执里写清楚：新增了几个测试、每个测试"哪种 bug 会让它红"。答不出这句话的测试就是废测试。

查法：`cg tests <symbol>` 看既有覆盖，避免重复造。

---

## §5 有什么验收条件(必须标 commit)

验收条件 = **可复现的观察**，每条后面挂**实现它的 commit**。

### 验收条件的形状

每条写成"命令 + cwd + 期望结果"，不写"跑通了"：

```
- [x] cargo fmt --check 无输出                        — 8e7fd6c
- [x] cargo check -p omenic-composition 无 error       — 8e7fd6c
- [x] CI(PR #NNN)全绿：build / test / fmt              — CI run 链接
- [x] grep -rn 'assemble(' crates/ bin/ | grep -v 'fn assemble' 至少 1 个真实调用点 — 5050641
- [x] cg changes origin/main risk 不为 critical         — 记录实际 risk 值
```

### commit 标注规则

- 取法：`git log --oneline origin/main..HEAD`，把短 SHA 写在对应条目后面
- 一条验收条件对应多个 commit 就都写上；一个 commit 覆盖多条就重复标注
- **squash 合并后 SHA 会变**：合并当天在 PR 的 Delivery record 补一行 squash SHA，[ROADMAP.md](../ROADMAP.md) / [PROGRESS.md](../PROGRESS.md) 引用 squash SHA + PR 号(仓库既有写法)
- 不合格：勾了 checkbox 不带 SHA，或者标了一个跟这条验收无关的 commit

### 本地能跑什么、不能跑什么

这是仓库铁律([AGENTS.md](../AGENTS.md))，任务书必须原样传达给子代理：

| 动作 | 本地 | 说明 |
|---|---|---|
| `cargo fmt --check` | 允许，提交前必跑 | 秒级 |
| `cargo check -p <crate>` | 允许 | 禁止 `--workspace` / `--all-targets` |
| `cargo test` / `clippy` / 全量 build | **禁止** | 一律交 PR 的 CI |
| `cargo build --bin oi-web` | 允许，但必须 `cpulimit -l 65 -i --` | web UI 肉眼确认时的唯一例外 |

**验证节奏**：本地 `fmt --check` + 单 crate `check` → push → **CI 出结果才算验证过**。CI 红了看日志改，不在本地复现。

> 待用户裁决：§6 smoke 若需要 `oi` / daemon 的本地二进制，现行 AGENTS.md 只给 `oi-web` 开了例外。建议把例外统一成"单 bin + `cpulimit -l 65 -i --`，禁 `--workspace`"；若你不同意，这类 smoke 就必须搬到 CI job 里做，任务书里要写明走哪条。

---

## §6 模拟测试功能(smoke)

Smoke 是**跑真东西**，不是跑测试文件。测试证明代码符合预期，smoke 证明功能对用户成立。

按改动面选：

| 改动面 | smoke 做法 | 证据形状 |
|---|---|---|
| Web UI | 按 AGENTS.md 启动序列起 `oi-web`(npm install → touch tailwind-input.css → cpulimit build → 杀旧进程重启 → 浏览器硬刷新)，按对应 UI 契约 yaml 的抽查点逐项看 | 伺服页 `<style>` 字节数(非 0)+ 逐抽查点结论 |
| CLI(`oi`) | 真跑命令链，贴输入输出 | 命令 + cwd + 实际 stdout |
| daemon / RPC | 起 daemon，发一条真实 RPC，看 `event.subscribe` 流有增量 | 请求 + 收到的事件序列 |
| 纯库 crate(无入口) | 写一次性脚本调真实路径，跑完**删掉** | 脚本内容 + 输出，并声明已删除 |

要求：

- 记录**命令、cwd、观察到的结果**，三样齐全才算证据
- 现象与预期不符就报告，不粉饰、不"应该是环境问题"
- 确实做不了肉眼验证，明确写"无法 smoke，原因 X"，不要留白让人以为验过了

不合格：把 `cargo test` 的输出当 smoke；或写"功能正常"不带任何观察记录。

---

## §7 清理什么

收尾清单，逐项确认，不留尾巴：

- **脚手架**：临时脚本、调试打印、注释掉的旧实现、一次性 smoke 脚本
- **孤儿**：本次改动造成的未用 import / 变量 / 函数。**只清自己弄出来的**，既有死代码只报告不删
- **过渡层**：清切换 —— 迁移全部调用点后删掉旧路径、别名、re-export、`#[deprecated]` 壳；不留兼容 shim
- **`#[allow(dead_code)]`**：合并前清掉(checklist `rust_no_dead_code_allow`)
- **遗留标记**：注释与 `todo!()` / `unimplemented!()` 必须带 issue 号(checklist `rust_todo_needs_issue`)，形如 `todo!("#123: ...")`
- **文档同步**：只改真的变了的
  - 用户可见行为变了 → [ROADMAP.md](../ROADMAP.md) / [PROGRESS.md](../PROGRESS.md)(带 PR 号 + squash SHA)
  - 规则变了 → `.githooks/spec/docs/SPEC_OVERVIEW.md`(改规则必须更新总览，正本在 canon)
  - web 组件 class / 布局变了 → 同步 `.githooks/spec/` 下对应 UI 契约 yaml 的 `find` 锚点，并 grep 确认锚点字符串在实现文件里真实存在
  - 开发约定变了 → [AGENTS.md](../AGENTS.md)
- **worktree 与分支**：合并后 `git worktree remove .wt/<...>`，删本地 head 分支(GT-07 自动删，确认一下)

不合格：留一个"后续再删"的兼容层；或文档只改了 ROADMAP 忘了 UI 契约 yaml，下一个 PR 契约测试才红。

---

## 补充三节(七问之外，实测最常出事的地方)

### §8 禁止项(PROHIBITED)

七问写的是"做什么"，这一节写"越界即打回"。子代理没有会话历史，边界不写死就会自由发挥。

任务书里逐条列，例如：

- 禁止改 §1 白名单之外的任何文件
- 禁止新增依赖(需要就先回来问)
- 禁止 `git add .` / `git add -A`，只 add 白名单路径
- 禁止在根工作树开发
- 禁止本地跑 `cargo test` / `clippy` / 全量 build
- 禁止改 `.githooks/`
- 禁止顺手格式化无关文件、顺手重构没坏的代码
- 禁止扩大范围"顺便"加重试 / 校验 / 埋点 / 抽象层

出处：既有 handoff 任务书已有这一节(`todo/handoff/*.md` 的 PROHIBITED)，实测有效。

### §9 并发边界与文件所有权

多子代理并行时必写，否则同文件编辑不保证能合。

- **文件所有权表**：每个子代理独占哪些路径，一格一个主人
- **公共冲突点**：`Cargo.lock`、根 `Cargo.toml` 各自只动自己那几行；指定一个整合负责人
- **跨包契约先定**：接口签名、DTO 形状写在共享上下文里，不让子代理之间现场协商
- **并行期间全体跳过验证**：build / lint / 测试留到最后一次做，中途验证会互相阻塞
- **合并顺序**：写明 A → B → C，后合者 rebase

出处：`todo/archive/route-to-g5-2026-09-15.md` 的互斥文件所有权表。

### §10 交接与续接

任务跨会话或中途阻塞时，写一份交接，让下一个 agent 不靠猜：

- 当前 worktree 路径、分支、HEAD、基线
- 已完成 / 未完成，逐条对应 §5 的验收条件
- 阻塞点：卡在哪、试过什么、还缺什么信息
- 下一步的**具体命令**，不是"继续实现"

放 `todo/handoff/<topic>.md`。

---

## 任务书骨架(复制这份填)

```markdown
# <类型>(<scope>): <一句话目标>

基线：origin/main <短SHA>    worktree：.wt/<issue编号>-<分支名>    分支：<type>/<描述>-<issue编号>
关联：Fixes #<sub-issue>

## 1 在什么地方
- <path>    改/新建：<做什么>
- 不动：<path>(理由)

## 2 参考什么内容(子代理先答，写进回执)
- 参考实现：<path:line>
- 技术栈约束：可用 <既有依赖>；禁新增依赖
- 调用面：cg callers <symbol> → <N 个>，逐个"改/不改 + 理由"
- 思路：<3-5 行；为什么不选另一种>

## 3 实现什么功能
- 签名：<pub fn ...>
- 行为契约：<输入 → 输出 / 错误路径>
- 边界：<空 / 超限 / 并发 / 重入>
- 不做：<X>(理由)

## 4 补充什么测试
- <测试名>：<哪种 bug 会让它红>
- 位置：<crate>/tests/<file>.rs

## 5 验收条件(逐条带 commit)
- [ ] cargo fmt --check 无输出                — <SHA>
- [ ] cargo check -p <crate> 无 error          — <SHA>
- [ ] CI(PR #N)全绿                            — <run 链接>
- [ ] <功能可观察点：命令 + 期望>               — <SHA>
- [ ] cg changes origin/main risk = <值>，非 critical

## 6 模拟测试(smoke)
- 做法：<起什么、跑什么>
- 记录：命令 / cwd / 观察结果

## 7 清理什么
- [ ] 脚手架与一次性脚本已删
- [ ] 本次造成的孤儿 import/变量/函数已清
- [ ] 旧路径/别名/re-export 已删(清切换)
- [ ] 无 #[allow(dead_code)]；遗留标记带 issue 号
- [ ] 文档同步：ROADMAP/PROGRESS/UI 契约 yaml/SPEC_OVERVIEW 按需
- [ ] worktree 与本地分支已清

## 8 禁止项
- <逐条>

## 9 并发边界(多子代理时填)
- 独占路径：<...>    公共冲突点处理：<...>    合并顺序：<...>
```

---

## 验收回执(主控核对用)

| 项 | 要看到的东西 | 空着就打回 |
|---|---|---|
| 范围 | `git diff --name-only origin/main..HEAD` 全在 §1 白名单内 | 有白名单外文件 |
| 调查 | §2 四项都有具体路径行号 + `cg callers` 结论 | 泛泛而谈 |
| 功能 | §3 每条签名与契约在 diff 里对得上 | 签名与任务书不一致且未说明 |
| 测试 | 每个新测试能说出"哪种 bug 会让它红" | 说不出 = 废测试，删 |
| 验收 | 每条 checkbox 带 commit SHA；CI 绿 | 无 SHA / CI 未过 |
| smoke | 命令 + cwd + 观察结果三样齐全 | 只写"正常" |
| 清理 | §7 逐项已确认 | 留兼容层 / 文档未同步 |
| 合规 | commit 标题 conventional 且无 CJK；PR 标题英文、正文中文、heading 英文 | gate 会直接拒 |

---

## 一页速查

```
出发   基线 SHA + worktree + cg status，禁改 .githooks/
1 哪里 文件白名单 + 明确非目标；测试进同层 tests/
2 参考 cg search/callers/callees/tests + grep 复核，路径行号落纸
3 做啥 公开签名 + 行为契约 + 边界 + 不做什么
4 测试 一个真实 bug 会让它红，否则改一次性脚本
5 验收 命令+cwd+期望，逐条挂 commit SHA；本地只 fmt/check，CI 才算过
6 smoke 跑真东西：web 起服看契约点 / CLI 真跑 / daemon 发 RPC
7 清理 脚手架、自己造的孤儿、旧路径、dead_code、文档、worktree
8 禁止 白名单外、新依赖、git add .、根工作树、本地跑测试
9 并发 文件所有权表 + 公共冲突点主人 + 合并顺序 + 中途不验证
10 交接 worktree/HEAD/已完成/阻塞/下一步具体命令
```
