# gugu（咕咕）

轻量 QQ 聊天客户端：Slint 原生前端（软件渲染）+ OneBot v11 协议端。
立项动机：内存（release 目标 USS <100MB，实测软件渲染 ~12-16MB）。

## 模块

| 路径 | 职责 |
|---|---|
| `src/main.rs` | 入口、UI 接线、GIF 帧播放器、协议桥接 |
| `src/onebot.rs` | OneBot v11 forward-WS 传输层（连接/重连/事件/动作/roster/发送） |
| `src/stickers.rs` | GIF 解帧、贴纸包扫描 |
| `ui.slint` | 全部声明式界面 |
| `examples/mock_onebot.rs` | mock 协议端（smoke 用） |

## 构建

```bash
cargo build            # 软件渲染（默认）
cargo build --release  # release，opt-level="s" + lto
```

## 运行

- `data/config.toml` 存在 → 连 OneBot 后端（`ws://` + `access_token`）
- 缺失 → 演示模式（mock 数据，零行为变化）

渲染器选择（可选 feature）：

```bash
# GPU（skia + vulkan）：内存 +162MB，见 ~/projects/chatroom-bench/RENDERER.md
cargo build --no-default-features --features slint/renderer-skia-vulkan
```

## 测试

```bash
cargo test onebot      # 传输层单测（含真实 WS 回环）
cpulimit -l 70 -i -- cargo clippy --all-targets -- -D warnings
```

CI 四项（fmt / clippy / build / test）在 PR 上跑；本地只 `cargo check`。
