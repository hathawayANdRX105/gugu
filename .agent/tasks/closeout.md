<!-- canon: hathawayANdRX105/canon @ 14f6f78 (synced 2026-09-23) -->
<!-- canon: tasks/closeout.md — 收尾任务书。来源: dotfiles deskctl snippets(tasks/closeout, closeout-pr, dev) + ferrite/omenic/kime/silverq 各仓 .agent 文档收录。 -->
# 项目收尾任务书（closeout）

> **什么时候读**：项目/PR 收尾阶段——功能开发完、要审查、修问题、合 PR、清场的时候。
> **解决什么**：收尾不是"提个 PR 等 CI"。收尾 = **CRG + gate 全栈审查（含 jev 模型层）→ 修复 → 记录到 PR → 清理工作树/分支 → 释放资源 → 汇报**，一条完整链路。
> **来源**：`dotfiles/config/deskctl/snippets/tasks/{dev,closeout,closeout-pr}`、ferrite `.agent/rules/{pr-workflow,gates,dev-env,conventions,testing-ci,web-lanes}.md`、omenic `.agent/task-templates-handbook.md`、kime/silverq `.agent/tasks/{version-stats,versioning}.md`。冲突时以本文件和项目 `.agent/` 本地文档为准。

---

## 一、收尾要解决什么（踩过的坑）

| 现象 | 真正原因 | 后果 |
|---|---|---|
| 到最后才做一次审查 | 审查只放在收尾 | 修复成本随问题堆积指数上升 |
| 审查工具喂整个 repo | 没分批 | 限流 / 输出噪音 / 大 diff 触发 jev API 400 静默失效 |
| 用 `head`/`grep -v` 看 gate 输出 | 过滤截断 | 后面的 FAIL 被吞，假绿 |
| worktree 嵌套 13 层、321G 重复产物 | 在 worktree 里再建 worktree | 磁盘事故（ferrite 真实发生过） |
| `pkill -f cargo` 清进程 | 误杀别的会话构建 | 别人 CI/本地构建被腰斩；cpulimit 节流的 T 状态≠死进程 |
| `nohup ... &` 起服务 | 工具调用结束进程组被回收 | 服务静默死亡，界面报 500 |
| 收完尾 worktree/分支留下 | 没清场清单 | 下个会话 `.wt/` 一堆僵尸目录 |
| 前端进程被"顺手"关掉 | 没区分资源归属 | 维护者要看的页面/共享后端没了 |

---

## 二、维护者要求（原则）

- 你是**主控**：审查、派子代理修、记录、清理。**收尾阶段不开发新功能**。
- 所有工作面登记为 PR；每轮"审查+修复"写**一条** PR comment；smoke 单独**一条**。
- FAIL 必须清零；WARN/INFO 逐条给结论，**不静默忽略**。
- gate / gh 拦截输出**完整读**，禁止 `head`/`tail`/`grep -v` 过滤后当没看见。
- 任何循环（fix→audit、CI、gate 重试）**同一问题 2 轮不过 → 停下向用户报备**已试方案，不无限循环。
- 只清**本会话自己创建**的 worktree/分支/进程；别人的、维护者要留的，不动。

---

## 三、前置检查（5 分钟）

```bash
# 1. gate spec 齐全（缺review_chain三件套就播种，只加不覆盖）
ls .githooks/spec/harness/review_chain.py .githooks/spec/quality/checklist_review_chain.yaml
gate init --rules-dir ~/projects/canon/rules   # 缺文件时

# 2. jev key 在环境里（fish conf.d/api_key.fish 已持久化；新 shell 自动加载）
echo $TYPESAFE_API_KEY   # 空则 source ~/.config/fish/conf.d/api_key.fish

# 3. 记录 base_sha（CRG/审查基线，**不写死 main**）
git merge-base HEAD origin/main   # 或 PR 创建时记录的 base
```

---

## 四、阶段 A：审查（CRG 结构层 + gate 全栈含 jev）

### A1. CRG 结构层

```bash
code-review-graph detect-changes --brief --base <base_sha>   # 确认改动范围与风险，逐条过
```
跨 3+ crate 的改动 gate 的 `crg_impact` 会 WARN——收尾时人工确认耦合是否合理。

### A2. gate 工具层（确定性，永远跑，不依赖任何 key）

```bash
GATE_BASE=<base_sha> gate check ccn antislop slop_comment duplication file_size clippy stale_api rust_todo_needs_issue
```
Rust 项目加 `clippy`；按语言加减。FAIL 清零；duplication 的 brace 噪音 WARN（sh 启发式老毛病）说明理由可放过，**真正判重复看 jev 的 `introduces_duplication`**。

### A3. gate 模型层（jev；scope 按改动状态选）

| 改动状态 | 命令 |
|---|---|
| 未提交改动 | `git diff \| python3 .githooks/spec/harness/review_chain.py .githooks/spec/harness/jev_questions_review.json 0.8 0.5` |
| 已合并/审历史范围 | `GATE_BASE=<base_sha> gate check review_chain` |
| 自动（推送即跑） | `git push` → pre-push hook 自动执行 |

问题集与阈值（`rules/harness/jev_questions_review.json`， per-question confidence-gated routing）：

| 问题 | fail 阈值 | 命中 |
|---|---|---|
| `introduces_complexity` | 0.80 | FAIL 硬拦 |
| `risky_without_test` | 0.85 | FAIL 硬拦 |
| `security_relevant_risk` | 0.90 | FAIL 硬拦 |
| 其余 | 0.80 | FAIL 硬拦；≥warn(0.5) WARN；否则 INFO 带 confidence |

**降级语义（二选一，工具层永远跑）**：jev 不可用 → 自动降级小模型（`REVIEW_LLM_BASE_URL/API_KEY/MODEL`）→ 都不可用 INFO 说明。任何基础设施失败（key 缺失/网络/超时/输出不可解析）只降级**不阻断**。大 diff 已自动截断（>24KB 取尾部=最新改动），400 问题已修。

### A4. ocr 深查（规范层，按模块分批）

```bash
ocr review --from <base_sha> --to HEAD          # 按文件/模块分批，不许一次喂全 repo
```
深边界 case（并发、错误路径、abi）再上 ocr；jev 是快筛，ocr 是深查，两者互补不重复。

### A5. 判定口径

- CRG/gate-jevs **FAIL** → 必须修（见阶段 B）。
- WARN/INFO → 逐条给"采纳/不采纳+理由"，写进 PR comment，不静默。
- omenic 任务模板口径：review 四面 = scope / CRG / code / simplicity，**P0/P1 未处置不能 done**（见 `.agent/task-templates-handbook.md` 的 `review` step）。

---

## 五、阶段 B：修复循环（fix → audit）

```
loop:
  fix   → 派子代理按问题范围做；最多并行 2 个互不冲突子任务；
           同文件/同模块写入必须串行；子任务 ≤5 文件、单一主题
  audit → 主控独立校验：真跑验收命令（不只看输出）、diff 只落在声明文件、
           查 root cause / 调用方 / 边界输入
失败 → 重拆或回 fix；**同一问题 2 轮不过 → 停下报备用户**
```

- 子代理必须在 `.wt/<branch>` 工作，prompt 写**全局绝对路径**，禁止仓库根目录写入。
- CPU-heavy 命令（build/test/install）套 `cpulimit -l 60`；本地只跑 <2min 针对性检查，其余推 PR CI。
- **CI 未绿不得进入后续阶段**；CI 失败当新问题回 loop。
- 修完重跑阶段 A 对应检查（修了复杂度就重跑 ccn+jev），直到干净。

---

## 六、阶段 C：记录到 PR（每轮一条 comment）

格式（follow gh 拦截 gate 的提示，不自行发明）：

```text
标题: Agent 🤖 - <topic>          # 如 "Agent 🤖 - review round 1: ccn + jev findings"
正文:
## 发现的问题        # 每条含: 来源(工具/jev p值/ocr)、严重度、位置
## 修复情况          # 每条含: 修法、修复 commit SHA、验证命令
```

- smoke 验证单独一条 comment（方法+结果）。
- comment 可多次出现（每轮审查+修复一条）。
- `gh` 命令必须走 `~/.local/bin/gh` 拦截版；PR 标题纯英文、正文小标题英文内容中文；type label 必须有。模板段落见项目 `.agent/rules/gates.md` / `.githooks/spec/github_pull_requests.yaml`。
- **关 issue 时**：`done_when_judge` 会把 Done when 每条交给 jev 按 p(未达标)≥0.85 硬拦——验收项要写具体、证据（PR diff）要真实；harness 缺失/超时只降 INFO，GT-04 机械 checkbox 门仍是兜底。

---

## 七、阶段 D：smoke（功能层最终验证）

- 真实用户路径跑一遍：CLI 命令 / 真实 URL / 真实进程；UI 用 `tab.ariaSnapshot()` 查 role/name/testid（ferrite 规范：禁只靠截图、禁 class 选择器定位）。
- CI 绿 ≠ 功能正确，smoke 不许拿 CI 替代；判据要可脚本化、主控可复跑。
- 发现问题 → 更新 PR 任务清单 → 回阶段 B → 通过后写 smoke comment。

---

## 八、阶段 E：merge 前置（全满足才合）

```bash
git diff --name-only <base_sha>..<branch>   # 复核改动只落在声明文件
hooks/merge --dry-run                        # 不绕过
```

清单：CI 全绿 □ 审查+smoke comment 齐全 □ 改动边界复核 □ merge --dry-run 过 □ gh gate 无拦截 □ FAIL 清零 □。
通过后 draft→ready，`gh pr merge <N> --squash --delete-branch`（远端+本地分支一并清）。

---

## 九、阶段 F：清理工作树与分支（只清本会话的）

```bash
git worktree list                          # 确认哪些是本次会话建的
git worktree remove .wt/<branch>           # 残留目录手动清；**只清本 session 自建**
git branch -d <branch>                     # 已合的分支；--delete-branch 已清的手动确认
```

硬规则（ferrite 13 层嵌套事故的教训）：
1. 建 worktree **必须先在仓库根** (`cd /path/to/repo`) 再 `git worktree add .wt/<名字> -b <分支>`——相对路径按当前目录解析，在 worktree 里建会嵌套。
2. 建完自检 `git worktree list`，路径出现两个 `.wt/` 立即 remove 重来。
3. gate 的 `checklist_no_nested_worktree` 提交/推送/合并时扫描，命中直接 FAIL。
4. 分支目录杂物（旧脚本/临时文件/废弃产物）：删或 `.gitignore`；**删除用 `gio trash`，严禁 `rm`/`git clean` 永久删**。
5. 代码 tidy：测试进 `tests/`、跑 formatter、清调试 log/commented-out code；formatter 改了文件 → 重跑最小验收+审查+smoke，更新 PR comment。文档同步（`AGENTS.md`/`README.md`/`docs/` 过期段落）。

---

## 十、阶段 G：释放资源（及时的，但留维护者要查的）

**要释放**（收尾即清）：
- 本会话 `hub start` 起的临时服务/ watcher / REPL → `hub stop <name>`。
- 一次性 mock 服务器、临时端口监听、临时构建容器。
- 本会话的后台 bash 任务（`hub jobs` 里已完成的）。

**要保留**（维护者要检查的，通知维护者即可）：
- **Web 前端进程/共享后端**：如 ferrite 的 `just dev-backend`（3211）、dx wasm 前端（8090）、docker 容器 `uf-local-postgres`——维护者要验收页面，**不关**。
- 其他会话在跑的进程一律不碰。

清进程前必须确认归属：`readlink /proc/<pid>/cwd`。**禁止 `pkill -f cargo`/`pkill -f rustc`**——多半是别人的构建；被 cpulimit 节流的进程处于 T（暂停）状态，**T≠死进程**，活会话的用 `kill -CONT` 恢复。长驻服务用持久后台任务（`hub start`）起，禁 `nohup ... &`（工具调用结束进程组被回收，服务静默死亡）。

---

## 十一、阶段 H：汇报

必报内容（deskctl closeout §7 + ferrite 阶段 8 合并口径）：
1. PR 链接 + base_sha + 改了哪些文件。
2. 跑了哪些检查：CRG / gate 工具层（列规则）/ jev（列 question+p值+结论）/ ocr / CI / smoke，逐项给结果。
3. 每轮"审查+修复"对应的 PR comment 链接（有新建未报备 = 违规）。
4. 清理记录：删了哪些 worktree/分支/进程；**保留了哪些资源及原因**。
5. 剩余风险：未跑的测试、已知问题、WARN/INFO 里未采纳项及理由。

**版本统计（silverq/kime 口径，收尾合入后跑一次）**：major=用户确认 / minor=功能域数（移除后用户是否感知）/ patch=发版分支 `^fix` commit 数；流程见 `.agent/tasks/version-stats.md`，项目特有真相源见各仓 `versioning.md`。

---

## 十二、硬性门禁汇总（违反即停）

| # | 门禁 |
|---|---|
| 1 | 审查必须 CRG + gate 全栈（工具层 jev ocr 三层都出结果才算审完） |
| 2 | FAIL 必须清零；WARN/INFO 逐条给结论写进 PR |
| 3 | gate/gh 输出完整读，禁过滤后装没看见 |
| 4 | 子代理只在 `.wt/<branch>`，prompt 给绝对路径；≤5 文件单主题 |
| 5 | 重活 `cpulimit -l 60`；测试推 CI；CI 未绿不进后续阶段 |
| 6 | 同一问题 2 轮不过 → 停下报备，不无限循环 |
| 7 | merge 前 `hooks/merge --dry-run` + diff 边界复核，不绕过 `.githooks/` |
| 8 | 只清本会话 worktree/分支；删文件用 `gio trash`；禁 `rm`/`git clean` |
| 9 | 禁 `pkill -f cargo/rustc`；T 状态进程先 `kill -CONT` 再说；归属用 `readlink /proc/<pid>/cwd` 确认 |
| 10 | 维护者要查的资源（web 前端/共享后端）不关，只报备 |

---

## 附：各项目本地文档索引（收尾前按项目读）

| 项目 | 路径 | 收尾相关 |
|---|---|---|
| ferrite | `.agent/rules/pr-workflow.md` | 九阶段全流程、worktree 硬规则 |
| ferrite | `.agent/rules/gates.md` | gate 输出判读、PR 模板、占位符规范 |
| ferrite | `.agent/rules/dev-env.md` | 进程/共享后端/缓存踩坑 |
| ferrite | `.agent/rules/{conventions,testing-ci,web-lanes}.md` | UI 验证、CI 静默失效三模式 |
| omenic | `.agent/task-templates-handbook.md` | Done when=phase、Construction plan=steps 选型；review/tidy/handoff step |
| kime / silverq | `.agent/tasks/version-stats.md` / `versioning.md` | 版本三段口径与项目真相源 |
