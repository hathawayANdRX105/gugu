<!-- canon: hathawayANdRX105/canon @ 14f6f78 (synced 2026-09-23) -->
# 版本统计任务书（版本口径唯一正本）

> **什么时候读**：要发版、统计版本号、或改版本口径的时候。**解决什么**：major/minor/patch
> 三段各由什么决定、功能域怎么判、按什么步骤跑——一份就够，别在项目里另建副本。

> 2026-09-23 合并：通用骨架（原 kime 版）与 silverq 项目真相源（原 `silverq/versioning.md`）
> 合体为一处——**版本口径只此一份**，项目特有内容在下文「项目篇」各节，冲突以项目篇为准
> （它绑定真实路径与发版分支）。用法：按「通用篇」定口径与流程，按「项目篇」拿命令与真相源。

## 通用篇：版本号三段（来源各不同，别混）

- **major = 用户确认**。breaking change 由用户拍板，不自动算。执行前先问用户当前 major。
- **minor = 功能域数**。用户/前端能直接感知的能力，逐项清点。
- **patch = fix 类型 commit 累计数**。机数：只数主题行以 `fix` 开头的提交，`perf` 不算 fix。

## 通用篇：判定「功能域 vs 管道」（minor 记不记，就这一条）

该能力若被移除，**用户/前端是否察觉**？

- 察觉（候选消失 / 上屏没了 / 某键行为没了）→ **功能域**，计入 minor。
- 不察觉（内部存储 / 解析 / 解码 / 状态机 / 配置 / 调度，行为由别的模块对外体现）→ **管道**，不计。

候选集 = 顶层公开能力（`pub mod` / 顶层 API / 平台前端通路）。新增模块：重跑枚举，
逐个过这条判定，再对照项目篇排除表核对。

## 执行步骤

### 1. 确认 major

问用户当前 major 值（无 breaking 保持 0；有 breaking 用户确认后 +1）。

### 2. 清点 / 核对 minor

对照项目篇的**真相源清单**（功能域表）与代码现状：

- 新增用户可感知能力 → 清单加一行，minor +1。
- 只在既有功能域内修 bug / 内部重构 → minor 不变。
- 枚举命令与固定排除表见项目篇；清单项数必须 == minor。

### 3. 统计 patch

```bash
git log <发版分支> --no-merges --format="%s" | grep -cE "^fix"
```

`--format="%s"` 取纯主题行（无 hash、无前导空格），`^fix` 才能匹配。
发版分支名见项目篇；本地 `feat/*` 分支上的 fix 合入前不计——patch 跟随发版分支。

### 4. 更新版本号与提交 tag

按项目篇的版本文件与命令更新（如 Rust 项目改 `Cargo.toml` 后 `cargo check` 刷新 lock），
提交信息用 `chore(release): v$MAJOR.$MINOR.$PATCH`，打同色 tag 并推送。

### 5. 发布核对（push tag 后）

按项目篇的产物清单回读 Release：标题、assets 齐全、CI 绿。**workflow 绿 ≠ 产物对**。
项目篇带「快照」块的（silverq），同一步先把三行数字刷新。

---

## 项目篇：silverq（版本口径项目真相源）

### 快照（每次发版更新这三行；过期即以发版时重跑值为准）

- 版本：v0.12.25（2026-09-21）
- 功能域（minor）：12（清单表待补，见下）
- patch（`grep -cE "^fix"` on master）：25（机械计数 26，#11「drop UPX」被 #12 回滚）

### 真相源正文

- **发版分支**：`master`。
- **功能域清单**：文档声称 12 项但清单表在原文件中缺失（2026-09-23 合并时发现）——
  下次发版前**先补清单再核对 minor**，别让 minor 继续悬空。
- **候选枚举**：
  ```bash
  find src -name '*.rs' | sort
  grep -E "^(pub )?(mod|fn|struct|enum) " src/lib.rs
  ```
- **排除管道**：`config`、`proxy/factory`、`scheduler/node` 等内部层不计。
- **更新版本**：
  ```bash
  MAJOR=<步骤1用户确认值> MINOR=<步骤2清单项数> PATCH=<步骤3命令输出>
  sed -i "s/^version = .*/version = \"$MAJOR.$MINOR.$PATCH\"/" Cargo.toml
  cargo check   # 刷新 Cargo.lock
  ```
- **提交与 tag**：`git add Cargo.toml Cargo.lock .agent/tasks/version-stats.md` →
  `chore(release): v$MAJOR.$MINOR.$PATCH` → `git tag v$MAJOR.$MINOR.$PATCH` → push 分支+tag。
- **发布产物**（tag 触发 `release.yml`：tag↔Cargo.toml 一致性 → 双变体 plain / meow-tun
  → UPX `--best --lzma` → 双二进制冒烟）：4 个 assets 齐全
  `silverq-X.Y.Z-x86_64-linux.tar.gz` + `.sha256`、`silverq-tun-X.Y.Z-x86_64-linux.tar.gz` + `.sha256`。
- **Release 坑**（都踩过）：
  1. GitHub Actions 的 `with:` 参数是字面字符串，`$VAR` 不插值（只 `run:` 步骤插值）——
     标题必须用 `${{ github.ref_name }}`，改动时别退回 `$VAR` 写法。
  2. Release notes 当前是 workflow 内手写 body（无 generate_release_notes，没有「by @作者」尾巴）；
     若改自动 notes，它会追加「by @作者 in #N/URL」，需加 strip 步骤（在 workflow 里用 `sed`/`grep -v` 去掉该行）。
  3. 调试期手动触发会留 stale draft（历史残留 v0.13.6 / v0.18.16），发布后 `gh release list` 扫一眼删掉。
- **2026-09-20 待决**：排除表里 `dataplane/tun` 的旧理由「未实现占位」已失效——TUN 已实现并在
  master（用户主动关闭不用）。按「移除后用户是否察觉」口径，倾向仍计管道；但这是用户确认项，
  别默默沿用旧理由。

## 项目篇：kime

- 只有通用篇流程；发版分支 / tag 格式 / 产物清单以 kime 仓实际为准（原骨架引用的
  "本项目 versioning.md" 在 kime 并不存在——要用时先在 kime 补齐项目篇一节）。

## 校验（全过才算完成）

- [ ] major = 用户确认值
- [ ] 功能域清单项数 == 版本文件的 minor（silverq：清单补齐后）
- [ ] 步骤 3 命令输出 == 版本文件的 patch
- [ ] 版本文件/lock 已更新，构建通过
- [ ] tag 已推送，CI success
- [ ] Release 标题 / notes / assets 回读无误，无 stale draft
