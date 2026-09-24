<!-- canon: hathawayANdRX105/canon @ 794df0b (synced 2026-09-24) -->
# workflow-state — wf-* 工作流的项目本地状态文件约定

wf-* 九件套的"地图"。方法在全局技能里；**状态在项目仓里**。
每个 wf-* 任务开工前先读相关状态文件判位，收尾前把状态写回去。

## 目录与所有权矩阵

工件基目录 = **`.workflow/`**（点目录，不污染项目文档树，也不进文档站构建）。已经在用
`docs/scope/` 或 `docs/specs/` 的老仓保持原基目录，不迁移在途工件；下表按 `.workflow/` 写。

| 文件/目录 | 创建者 | 推进者（可写） | 只读者 | 内容 |
|---|---|---|---|---|
| `.workflow/scope/<feature>.md` | wf-scope | wf-scope | wf-architect/wf-develop | coarse scope（做什么、不做什么、验收） |
| `.workflow/scope/STATUS.md` | wf-scope | wf-scope/wf-sync | 全部 | 全项目功能状态表（queue/in progress/done） |
| `.workflow/specs/<feature>.md` | wf-architect | wf-architect | wf-develop/wf-check/wf-sync | build spec（决策完整；状态行：Draft→Spec→GA） |
| `.workflow/specs/<feature>-design.md` | wf-architect | wf-architect | wf-develop | 设计附录（图、时序、接口细节），可选 |
| `.workflow/reviews/<feature>-<date>.md` | wf-check | wf-check（追加 Dispositioned） | wf-sync | review/verify 工件：发现清单 + 处置记录（耐久文件） |
| `.workflow/audit/<feature>.md` | wf-audit | wf-audit/wf-develop（打勾） | wf-check | 基线缺口报告 + 验收条目 AC-n |
| `verify.md`（仓根） | 开发者 | 开发者 | **wf-test（只读！）** | 可执行核验：复现命令 + 期望结果 + 边界 |
| `test-preferences.json`（仓根） | wf-test 首次运行时 | 开发者 | wf-test | 测试框架/风格约定 |
| `.workflow/postmortems/`、`.workflow/releases/` | wf-document | wf-document | — | 复盘与发布说明 |

`verify.md` / `test-preferences.json` 例外留在仓根：两者都是开发者与 wf-test 直接维护的
入口文件，不是 wf-* 的过程工件，不搬进点目录。

## 三条硬规则

1. **所有权单一**：每类文件只有一个创建者、一个主推进者；别人要改 → 交给所有者技能（或在任务里指定），不许顺手改别人的状态。
2. **只读纪律**：wf-test 永远不写 verify.md；发现 verify.md 写不了/过时 → 报给开发者修，不代笔。
3. **状态行只由 wf-sync 翻**：spec 的 Draft→Spec→GA、scope STATUS 的 done，只有 wf-sync 在对账后改；其他技能最多**标记**"建议翻状态"。

## 判位（任务开工 30 秒）

1. 读 `.workflow/scope/STATUS.md`：该功能处于什么阶段（无 scope = 先 /wf-scope；有 scope 无 spec = 先 /wf-architect；有 spec 未 GA = 先让 spec 过审再建）。
2. 有 `.workflow/audit/` 条目 → 实现时对着 AC 逐条核。
3. 有 `.workflow/reviews/` 未处置发现 → 先处置（见 wf-check 处置硬规则），再继续新工作。

## 与 canon 其他分发的关系

- `closeout.md` 收尾任务书的"清理/资源释放"节操作的对象 = 本矩阵里的 `.workflow/` 工件与 worktree。
- `feature-dev-handbook.md` 七段式任务书的"验收/核验"段引用 verify.md 与 AC。
- 本文件只约定**形状与所有权**；各功能的实质内容在对应文件里。
