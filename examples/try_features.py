"""
试用 better-xescoding-helper 新功能的 Python 示例
依赖: pip install requests
"""

import json
import sys
import time
import requests

BASE = "http://127.0.0.1:55820"
TIMEOUT = 10


def step(n, title):
    print(f"\n{'=' * 64}")
    print(f"  [{n}] {title}")
    print('=' * 64)


def show(resp, max_len=600):
    print(f"  → HTTP {resp.status_code}")
    try:
        data = resp.json()
        s = json.dumps(data, ensure_ascii=False, indent=2)
        if len(s) > max_len:
            s = s[:max_len] + f"\n  ... (截断,共 {len(s)} 字符)"
        print(s)
    except Exception:
        print(f"  {resp.text[:max_len]}")


def get(path, **kw):
    return requests.get(f"{BASE}{path}", timeout=TIMEOUT, **kw)


def post(path, body=None, **kw):
    return requests.post(f"{BASE}{path}", json=body or {}, timeout=TIMEOUT, **kw)


def main():
    try:
        get("/ping").raise_for_status()
    except Exception as e:
        print(f"✗ 连不上 {BASE}: {e}")
        print("  请先启动: target\\release\\xescoding_helper.exe")
        sys.exit(1)

    step("11", "健康检查 — /api/health")
    r = get("/api/health")
    show(r)
    if r.ok:
        h = r.json()
        print(f"\n  状态: {h['status']}  运行 {h['uptime_secs']}s")
        print(f"  内存峰值: {h['system']['process_rss_bytes'] / 1024 / 1024:.1f} MB")

    step("5", "运行指标 — /api/metrics")
    show(get("/api/metrics"))

    step("7", "环境变量 — /api/settings/env")
    show(post("/api/settings/env", {
        "project_id": "demo",
        "vars": {"MY_APP_NAME": "hello-xes", "MY_API_KEY": "secret-123"},
        "merge": False
    }))
    print("  → 这两个变量会在执行 demo 项目时注入到子进程环境")

    step("8", "代理 — /api/settings/proxy")
    show(post("/api/settings/proxy", {
        "http": "http://127.0.0.1:8888",
        "https": "http://127.0.0.1:8888",
        "no_proxy": "localhost,127.0.0.1,*.local"
    }))
    print("  → 此后 pip / 资源下载会走代理,可用 no_proxy 排除内网")

    step("3", "Venv — /api/settings/venv")
    show(post("/api/settings/venv", {
        "enabled": True,
        "inherit_base_packages": True,
        "pinned_packages": ["pip", "setuptools", "wheel"]
    }))
    print("  → 启用后每个 project_id 首次运行会自动 python -m venv")

    step("18", "预热 — /api/settings/prewarm + /api/prewarm")
    show(post("/api/settings/prewarm", {
        "enabled": True,
        "packages": ["xes-lib", "Pillow", "qrcode", "numpy"]
    }))
    print("  → 启动时自动预装,也可手动触发:")
    show(post("/api/prewarm", {"force": True}))

    step("1+2", "智能依赖 + 静态检查 — /api/settings/run-limits")
    show(post("/api/settings/run-limits", {
        "auto_install_on_missing": True,
        "lint_before_run": True,
        "detect_missing_imports": True
    }))
    print("  → 启用后,跑前会先 ruff/flake8 检查,缺包自动 pip install")

    step("13", "缓存清理策略 — /api/settings/cleanup")
    show(post("/api/settings/cleanup", {
        "max_asset_pool_bytes": 2 * 1024 ** 3,
        "max_snapshot_bytes": 100 * 1024 ** 2,
        "max_snapshot_count": 30,
        "run_metrics_history": 200
    }))

    step("17", "提交到运行队列 — /api/queue/submit")
    code = (
        "import os, sys, time\n"
        "print('hello from queue')\n"
        "print('python:', sys.executable)\n"
        "for k in sorted(os.environ):\n"
        "    if k.startswith('MY_'):\n"
        "        print(f'env {k}={os.environ[k]}')\n"
        "time.sleep(0.5)\n"
        "print('done')\n"
    )
    show(post("/api/queue/submit", {
        "project_id": "demo",
        "code": code,
        "path": "demo"
    }))
    time.sleep(1)
    print("\n  → 队列状态:")
    show(get("/api/queue"))
    print("\n  → 已注册项目:")
    show(get("/api/projects"))

    step("16", "重试 + 网络 — 触发资源下载")
    print("  资源下载接口会走 16. 断网重试(指数退避),")
    print("  可在 POST /api/path 时传错 project_id 制造 4xx 验证重试逻辑")

    step("14", "panic 捕获 — /api/logs")
    show(get("/api/logs?limit=20"))
    print("  → 如果运行过中有 panic,会出现在 panics 字段")

    step("6", "导出历史 — /api/history/export")
    for fmt in ("json", "csv", "md"):
        r = get(f"/api/history/export?format={fmt}")
        print(f"  {fmt:>4}: HTTP {r.status_code}, "
              f"{len(r.content):>6} bytes, "
              f"type={r.headers.get('content-type')}")
        if r.status_code == 200:
            print(f"         前 160 字符: {r.text[:160]!r}")

    step("13", "缓存信息 + 清理 — /api/cache/list + /api/cache/cleanup")
    show(get("/api/cache/list"))
    show(post("/api/cache/cleanup"))

    step("5", "运行后指标 — /api/metrics")
    show(get("/api/metrics"))
    show(get("/api/metrics/runs?limit=5"))

    step("11", "最终健康检查 — /api/health")
    r = get("/api/health")
    if r.ok:
        h = r.json()
        print(f"  状态={h['status']}  "
              f"uptime={h['uptime_secs']}s  "
              f"内存={h['system']['process_rss_bytes'] / 1024 / 1024:.1f}MB")
        print(f"  缓存={h['caches']['asset_pool_bytes'] / 1024 / 1024:.1f}MB  "
              f"快照={h['caches']['snapshot_count']} 个  "
              f"历史={h['history']['total']} 条")

    print("\n" + "=" * 64)
    print("  ✓ 全部端点已试完")
    print("  接下来:")
    print("    1. 打开调用端页面,跑一段 import 不存在的包,观察自动安装")
    print("    2. 写有 lint 问题的代码,观察跑前报告")
    print("    3. 启用 venv 后,跑几次,看 cache/venvs/<project_id>/ 目录")
    print("    4. POST /api/queue/submit 多提交几次,看队列处理")
    print("=" * 64)


if __name__ == "__main__":
    main()
