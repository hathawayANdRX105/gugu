<!-- canon: hathawayANdRX105/canon @ 14f6f78 (synced 2026-09-23) -->
#!/usr/bin/env python3
"""jev-ask — 调 TypeSafe System One (jev) 做结构化判断。零依赖，stdlib only。

用法:
  jev-ask.py questions.json state.txt      # state 为文件（文本或 JSON）
  cat diff.patch | jev-ask.py questions.json   # state 从 stdin
  jev-ask.py --check                       # 只验连通与鉴权（发一个 1-题 ping）

环境变量:
  TYPESAFE_API_KEY    必填。官方 key 或网关 key
  TYPESAFE_API_BASE   可选。默认 https://api.typesafe.ai；走网关填网关地址

模型不用变量：MODEL 常量直接写死在下面（当前 jev-latest → jev-1.13.0）。
模型不可用（下线/改名/拒绝）时自动 GET /v1/models 抓最新 jev-* 重试一次。

questions.json 形如（type ∈ noul | choice | score）:
  {
    "has_secret": {"type": "noul",
                   "instructions": "Does the diff add a hardcoded secret?",
                   "criteria": {"true": "credential-like literal added",
                                 "false": "none added"}},
    "severity":   {"type": "score", "instructions": "...",
                   "criteria": ["info", "warn", "critical"]}
  }

输出: answers JSON（stdout），错误信息走 stderr 并以非零码退出。
"""
import json
import os
import sys
import urllib.error
import urllib.request

TIMEOUT = 60

# 模型名直接开放写死，不走环境变量。别名 jev-latest 指向当前稳定版（写作时 jev-1.13.0）。
MODEL = "jev-latest"


def die(msg: str) -> None:
    print(f"jev-ask: {msg}", file=sys.stderr)
    sys.exit(1)


def _http(url: str, headers: dict, body: dict | None = None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers=headers)
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.loads(resp.read().decode())


def fetch_newest_model(base: str, headers: dict) -> str | None:
    """GET /v1/models 抓最新的 jev 模型。兼容官方形状 models[].name 与
    网关(OpenRouter 风格) data[].id；优先 jev-latest 别名，否则取版本号最大的 jev-*。"""
    try:
        payload = _http(f"{base}/v1/models", headers)
    except Exception:  # noqa: BLE001 — 列举失败就放弃兜底，按原错误退出
        return None
    # 网关(OpenRouter 风格)行内 id 是机器名(jev-latest)，name 是展示名("Jev Latest")——id 优先
    names = [(m.get("id") or m.get("name") or "").strip() for m in payload.get("models") or payload.get("data") or []]
    names = [n for n in names if n]

    def vkey(n: str):
        tail = n.lower().removeprefix("jev-")
        return [int(x) for x in tail.split(".") if x.isdigit()] or [0]

    lowered = [n.lower() for n in names]
    if "jev-latest" in lowered:
        return names[lowered.index("jev-latest")]
    versioned = sorted((n for n in names if n.lower().startswith("jev-") and n[4:5].isdigit()), key=vkey, reverse=True)
    return versioned[0] if versioned else None


def ask(base: str, headers: dict, body: dict) -> dict:
    """调一次 systemone。模型相关失败时抓最新 jev 模型重试一次。"""
    try:
        return _http(f"{base}/v1/systemone", headers, body)
    except urllib.error.HTTPError as e:
        detail = e.read().decode(errors="replace")[:500]
        if e.code not in (400, 404, 422) and "model" not in detail.lower():
            die(f"HTTP {e.code}: {detail}")
        newest = fetch_newest_model(base, headers)
        if not newest or newest == body.get("model"):
            die(f"HTTP {e.code}: {detail}")
        print(f"jev-ask: 模型 {body.get('model')} 不可用（HTTP {e.code}），改用抓到的最新模型 {newest} 重试", file=sys.stderr)
        return _http(f"{base}/v1/systemone", headers, {**body, "model": newest})
    except Exception as e:  # noqa: BLE001 — 网络层错误统一透出
        die(f"请求失败: {e}")


def main() -> int:
    base = os.environ.get("TYPESAFE_API_BASE", "https://api.typesafe.ai").rstrip("/")
    key = os.environ.get("TYPESAFE_API_KEY")
    if not key:
        die("TYPESAFE_API_KEY 未设置（fish: set -gx TYPESAFE_API_KEY sk-...; 重开 shell）")
    headers = {
        "Content-Type": "application/json",
        "Authorization": f"Bearer {key}",  # 官方 api.typesafe.ai 认这个
        "x-api-key": key,                  # 中转网关（knox.chat 等）认这个；双发自动兼容
    }

    if len(sys.argv) == 2 and sys.argv[1] == "--check":
        body = {"model": MODEL, "state": "ok",
                "questions": {"ping": {"type": "noul", "instructions": "Is the state non-empty?"}}}
    elif len(sys.argv) >= 2:
        try:
            with open(sys.argv[1]) as f:
                questions = json.load(f)
        except (OSError, json.JSONDecodeError) as e:
            die(f"问题文件读不了: {e}")
        if len(sys.argv) >= 3:
            with open(sys.argv[2]) as f:
                raw = f.read()
        elif not sys.stdin.isatty():
            raw = sys.stdin.read()
        else:
            die("缺 state：给文件参数或从 stdin 管道进来")
        try:
            state = json.loads(raw)  # JSON 对象/数组原样发，普通文本按字符串发
        except json.JSONDecodeError:
            state = raw
        if not str(state)[:1]:
            die("state 为空")
        body = {"model": MODEL, "state": state, "questions": questions}
    else:
        die(__doc__)

    payload = ask(base, headers, body)

    if len(sys.argv) == 2 and sys.argv[1] == "--check":
        print(f"✓ {payload.get('model', MODEL)} 连通正常")
        return 0
    print(json.dumps(payload.get("answers", {}), ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
