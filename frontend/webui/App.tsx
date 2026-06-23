import type React from "react";
import { useState, useEffect, useCallback } from "react";

interface PythonPath {
    path: string;
    selected: boolean;
}

interface GolangPath {
    path: string;
    selected: boolean;
}

interface BunPath {
    path: string;
    selected: boolean;
}

interface ServerError {
    message: string;
    type: string;
}

interface RunRecord {
    id: string;
    timestamp: number;
    code: string;
    output: string;
    hasGoBlocks: boolean;
    success: boolean;
    duration: number;
    project_id?: string;
    peak_rss_bytes?: number;
    auto_installs?: number;
    lint_issues?: number;
    exit_code?: number | null;
    imports?: string[];
    packages?: RunPackage[];
    ai_explanation?: string | null;
    sandboxed?: boolean;
}

interface RunPackage {
    name: string;
    version?: string | null;
    requires?: string[];
    required_by?: string[];
}

interface GraphEdge {
    from: string;
    to: string;
}

interface RunGraph {
    run_id: string;
    nodes: RunPackage[];
    edges: GraphEdge[];
}

interface PipPackage {
    name: string;
    version: string;
}

interface DashboardData {
    history_total: number;
    history_success: number;
    history_failed: number;
    success_rate: number;
    last_24h_runs: number;
    top_failing_imports: [string, number][];
    summary: { total_runs: number; success_rate: number; avg_duration_ms: number };
    server_version: string;
}

interface AiStatus {
    enabled: boolean;
    configured: boolean;
    base_url: string;
    model: string;
    auto_explain_on_error: boolean;
    timeout_secs: number;
    max_tokens: number;
}

interface AiSettings {
    enabled: boolean;
    base_url: string;
    api_key: string;
    model: string;
    timeout_secs: number;
    max_tokens: number;
    temperature: number;
    auto_explain_on_error: boolean;
    system_prompt: string;
}

interface UpdaterStatus {
    enabled: boolean;
    current_version: string;
    repo: string;
    target_asset: string;
    cached_release?: { tag_name: string; html_url: string; body?: string };
    update_available: boolean;
}

interface SandboxReport {
    backend: string;
    effective: boolean;
    memory_limit_bytes: number;
    cpu_time_limit_secs: number;
    no_network: boolean;
    notes: string[];
}

interface SandboxSettings {
    enabled: boolean;
    mode: string;
    memory_limit_bytes: number;
    cpu_time_limit_secs: number;
    no_network: boolean;
    read_only_paths: string[];
    writable_paths: string[];
    drop_capabilities: boolean;
}

type Tab = "settings" | "history" | "packages" | "v2";

function formatTime(ts: number): string {
    const d = new Date(ts);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function formatDuration(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
}

function formatBytes(b: number): string {
    if (b < 1024) return `${b}B`;
    if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)}KB`;
    if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)}MB`;
    return `${(b / 1024 / 1024 / 1024).toFixed(2)}GB`;
}

function colorizeOutput(output: string): React.ReactNode[] {
    const lines = output.split("\n");
    return lines.map((line, i) => {
        let colorClass = "";
        if (line.startsWith("[Go]") || line.startsWith("[Go错误]")) {
            colorClass = "text-go-tag";
        } else if (line.startsWith("[TS]") || line.startsWith("[TS错误]")) {
            colorClass = "text-ts-tag";
        }
        return (
            <span key={i} className={colorClass || undefined}>
                {line}
                {i < lines.length - 1 ? "\n" : ""}
            </span>
        );
    });
}

async function apiGet(path: string): Promise<any> {
    const r = await fetch(path);
    if (!r.ok) throw new Error(`${path}: ${r.status}`);
    return r.json();
}

async function apiPost(path: string, body?: any): Promise<any> {
    const r = await fetch(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: body ? JSON.stringify(body) : undefined,
    });
    const data = await r.json().catch(() => ({}));
    if (!r.ok && data.message) throw new Error(data.message);
    return data;
}

export default function App() {
    const [activeTab, setActiveTab] = useState<Tab>("settings");
    const [pythonPaths, setPythonPaths] = useState<PythonPath[]>([]);
    const [golangPaths, setGolangPaths] = useState<GolangPath[]>([]);
    const [bunPaths, setBunPaths] = useState<BunPath[]>([]);
    const [error, setError] = useState<ServerError | null>(null);
    const [loading, setLoading] = useState(true);
    const [saving, setSaving] = useState(false);
    const [serverOk, setServerOk] = useState(false);
    const [history, setHistory] = useState<RunRecord[]>([]);
    const [expandedId, setExpandedId] = useState<string | null>(null);
    const [installedPkgs, setInstalledPkgs] = useState<PipPackage[]>([]);
    const [pkgSearch, setPkgSearch] = useState("");
    const [pkgSearchResults, setPkgSearchResults] = useState<string[]>([]);
    const [pkgInstalling, setPkgInstalling] = useState<string | null>(null);
    const [darkMode, setDarkMode] = useState(() => {
        try {
            return localStorage.getItem("xes-dark") === "1";
        } catch {
            return false;
        }
    });

    // V2 state
    const [dashboard, setDashboard] = useState<DashboardData | null>(null);
    const [aiStatus, setAiStatus] = useState<AiStatus | null>(null);
    const [aiSettings, setAiSettings] = useState<AiSettings | null>(null);
    const [aiBusy, setAiBusy] = useState(false);
    const [aiMsg, setAiMsg] = useState<string>("");
    const [updater, setUpdater] = useState<UpdaterStatus | null>(null);
    const [updaterBusy, setUpdaterBusy] = useState(false);
    const [updaterMsg, setUpdaterMsg] = useState<string>("");
    const [sandboxReport, setSandboxReport] = useState<SandboxReport | null>(null);
    const [sandboxSettings, setSandboxSettings] = useState<SandboxSettings | null>(null);
    const [sandboxMsg, setSandboxMsg] = useState<string>("");
    const [depGraph, setDepGraph] = useState<RunGraph | null>(null);
    const [depGraphRunId, setDepGraphRunId] = useState<string>("");
    const [depGraphMsg, setDepGraphMsg] = useState<string>("");
    const [aiExplainById, setAiExplainById] = useState<Record<string, string>>({});
    const [aiExplainLoading, setAiExplainLoading] = useState<string | null>(null);

    useEffect(() => {
        document.documentElement.classList.toggle("dark", darkMode);
        try {
            localStorage.setItem("xes-dark", darkMode ? "1" : "0");
        } catch {}
    }, [darkMode]);

    const fetchInitial = useCallback(async () => {
        try {
            const [pathsRes, golangRes, bunRes, statusRes] = await Promise.all([
                fetch("/api/python-paths"),
                fetch("/api/golang-paths"),
                fetch("/api/bun-paths"),
                fetch("/api/status"),
            ]);
            const pathsData = await pathsRes.json();
            const golangData = await golangRes.json();
            const bunData = await bunRes.json();
            const statusData = await statusRes.json();
            if (statusData.error) {
                setError(statusData.error);
                setServerOk(false);
            } else {
                setError(null);
                setServerOk(true);
            }
            if (pathsData.paths && pathsData.paths.length > 0) {
                setPythonPaths(
                    pathsData.paths.map((path: string) => ({
                        path,
                        selected: path === pathsData.savedPath,
                    })),
                );
            } else {
                setPythonPaths([{ path: "python", selected: true }]);
            }
            if (golangData.paths && golangData.paths.length > 0) {
                setGolangPaths(
                    golangData.paths.map((path: string) => ({
                        path,
                        selected: path === golangData.savedPath,
                    })),
                );
            } else {
                setGolangPaths([{ path: "go", selected: true }]);
            }
            if (bunData.paths && bunData.paths.length > 0) {
                setBunPaths(
                    bunData.paths.map((path: string) => ({
                        path,
                        selected: path === bunData.savedPath,
                    })),
                );
            } else {
                setBunPaths([{ path: "bun", selected: true }]);
            }
        } catch (e) {
            setError({ message: "无法连接到服务器", type: "connection" });
        } finally {
            setLoading(false);
        }
    }, []);

    useEffect(() => {
        fetchInitial();
        const interval = setInterval(fetchInitial, 5000);
        return () => clearInterval(interval);
    }, [fetchInitial]);

    const refreshDashboard = useCallback(async () => {
        try {
            const d = await apiGet("/api/metrics/dashboard");
            setDashboard(d);
        } catch {}
    }, []);

    const refreshAi = useCallback(async () => {
        try {
            const [status, settings] = await Promise.all([
                apiGet("/api/ai/status"),
                apiGet("/api/settings/ai"),
            ]);
            setAiStatus(status);
            setAiSettings(settings);
        } catch {}
    }, []);

    const refreshUpdater = useCallback(async () => {
        try {
            const d = await apiGet("/api/updater/status");
            setUpdater(d);
        } catch {}
    }, []);

    const refreshSandbox = useCallback(async () => {
        try {
            const [report, settings] = await Promise.all([
                apiGet("/api/sandbox/status"),
                apiGet("/api/settings/sandbox"),
            ]);
            setSandboxReport(report.report);
            setSandboxSettings(settings);
        } catch {}
    }, []);

    useEffect(() => {
        if (activeTab !== "v2") return;
        refreshDashboard();
        refreshAi();
        refreshUpdater();
        refreshSandbox();
        const id = setInterval(() => {
            refreshDashboard();
        }, 2000);
        return () => clearInterval(id);
    }, [activeTab, refreshDashboard, refreshAi, refreshUpdater, refreshSandbox]);

    const fetchDepGraph = useCallback(async (runId: string) => {
        if (!runId) {
            setDepGraph(null);
            return;
        }
        try {
            const d = await apiGet(`/api/dependency-graph?run_id=${encodeURIComponent(runId)}`);
            setDepGraph(d.data);
            setDepGraphMsg("");
        } catch (e: any) {
            setDepGraph(null);
            setDepGraphMsg(e.message || "加载失败");
        }
    }, []);

    useEffect(() => {
        if (activeTab === "history") {
            fetch("/api/history")
                .then((r) => r.json())
                .then((d) => {
                    const recs: RunRecord[] = d.records || [];
                    setHistory(recs);
                    if (!depGraphRunId && recs.length > 0) {
                        setDepGraphRunId(recs[0].id);
                    }
                })
                .catch(() => {});
        }
    }, [activeTab, depGraphRunId]);

    useEffect(() => {
        if (depGraphRunId) fetchDepGraph(depGraphRunId);
    }, [depGraphRunId, fetchDepGraph]);

    useEffect(() => {
        if (activeTab === "packages") {
            fetch("/package/local")
                .then((r) => r.json())
                .then((d) => {
                    if (d.data && d.data.user) {
                        setInstalledPkgs(d.data.user);
                    }
                })
                .catch(() => {});
        }
    }, [activeTab]);

    const handleSelect = (path: string) => {
        setPythonPaths(
            pythonPaths.map((p) => ({
                ...p,
                selected: p.path === path,
            })),
        );
    };

    const handleGolangSelect = (path: string) => {
        setGolangPaths(
            golangPaths.map((p) => ({
                ...p,
                selected: p.path === path,
            })),
        );
    };

    const handleBunSelect = (path: string) => {
        setBunPaths(
            bunPaths.map((p) => ({
                ...p,
                selected: p.path === path,
            })),
        );
    };

    const handleSave = async () => {
        const selectedPython = pythonPaths.find((p) => p.selected);
        const selectedGolang = golangPaths.find((p) => p.selected);
        const selectedBun = bunPaths.find((p) => p.selected);
        setSaving(true);
        try {
            const promises: Promise<Response>[] = [];
            if (selectedPython) {
                promises.push(
                    fetch("/api/python-path", {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ path: selectedPython.path }),
                    }),
                );
            }
            if (selectedGolang) {
                promises.push(
                    fetch("/api/golang-path", {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ path: selectedGolang.path }),
                    }),
                );
            }
            if (selectedBun) {
                promises.push(
                    fetch("/api/bun-path", {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ path: selectedBun.path }),
                    }),
                );
            }
            const results = await Promise.all(promises);
            const errors: string[] = [];
            for (const res of results) {
                const data = await res.json();
                if (!data.success) errors.push(data.error || "未知错误");
            }
            if (errors.length === 0) {
                alert("设置保存成功！请刷新页面");
            } else {
                alert("部分保存失败: " + errors.join(", "));
            }
        } catch (e) {
            alert("保存失败");
        } finally {
            setSaving(false);
        }
    };

    const handleClearHistory = async () => {
        await fetch("/api/history/clear");
        setHistory([]);
        setDepGraph(null);
        setDepGraphRunId("");
    };

    const handleDeleteHistory = async (id: string) => {
        await fetch(`/api/history/delete?id=${encodeURIComponent(id)}`);
        setHistory((prev) => prev.filter((r) => r.id !== id));
        if (depGraphRunId === id) {
            setDepGraphRunId("");
            setDepGraph(null);
        }
    };

    const handleAiExplain = async (runId: string) => {
        setAiExplainLoading(runId);
        try {
            const d = await apiPost("/api/ai/explain", { run_id: runId });
            const text = d.data?.explanation || "(空)";
            setAiExplainById((prev) => ({ ...prev, [runId]: text }));
            setHistory((prev) =>
                prev.map((r) =>
                    r.id === runId ? { ...r, ai_explanation: text } : r,
                ),
            );
        } catch (e: any) {
            setAiExplainById((prev) => ({
                ...prev,
                [runId]: `❌ ${e.message || "AI 解释失败"}`,
            }));
        } finally {
            setAiExplainLoading(null);
        }
    };

    const handleSaveAiSettings = async () => {
        if (!aiSettings) return;
        setAiBusy(true);
        setAiMsg("");
        try {
            const payload: any = {
                enabled: aiSettings.enabled,
                base_url: aiSettings.base_url,
                model: aiSettings.model,
                timeout_secs: aiSettings.timeout_secs,
                max_tokens: aiSettings.max_tokens,
                temperature: aiSettings.temperature,
                auto_explain_on_error: aiSettings.auto_explain_on_error,
                system_prompt: aiSettings.system_prompt,
            };
            if (aiSettings.api_key && aiSettings.api_key !== "***") {
                payload.api_key = aiSettings.api_key;
            } else if (aiSettings.api_key === "") {
                payload.clear_api_key = true;
            }
            await apiPost("/api/settings/ai", payload);
            setAiMsg("已保存");
            refreshAi();
        } catch (e: any) {
            setAiMsg(`❌ ${e.message || "保存失败"}`);
        } finally {
            setAiBusy(false);
        }
    };

    const handleCheckUpdate = async () => {
        setUpdaterBusy(true);
        setUpdaterMsg("");
        try {
            await apiPost("/api/updater/check", { force: true });
            setUpdaterMsg("检查完成");
            refreshUpdater();
        } catch (e: any) {
            setUpdaterMsg(`❌ ${e.message || "检查失败"}`);
        } finally {
            setUpdaterBusy(false);
        }
    };

    const handleApplyUpdate = async () => {
        if (!confirm("确认应用更新?将替换当前可执行文件并需要重启")) return;
        setUpdaterBusy(true);
        setUpdaterMsg("下载并应用中…");
        try {
            const d = await apiPost("/api/updater/apply");
            setUpdaterMsg(`✅ 已更新到 ${d.data?.new_executable || "新版本"},请手动重启`);
        } catch (e: any) {
            setUpdaterMsg(`❌ ${e.message || "更新失败"}`);
        } finally {
            setUpdaterBusy(false);
        }
    };

    const handleSaveSandbox = async () => {
        if (!sandboxSettings) return;
        setSandboxMsg("");
        try {
            await apiPost("/api/settings/sandbox", {
                enabled: sandboxSettings.enabled,
                mode: sandboxSettings.mode,
                memory_limit_bytes: Number(sandboxSettings.memory_limit_bytes),
                cpu_time_limit_secs: Number(sandboxSettings.cpu_time_limit_secs),
                no_network: sandboxSettings.no_network,
                read_only_paths: sandboxSettings.read_only_paths,
                writable_paths: sandboxSettings.writable_paths,
                drop_capabilities: sandboxSettings.drop_capabilities,
            });
            setSandboxMsg("已保存");
            refreshSandbox();
        } catch (e: any) {
            setSandboxMsg(`❌ ${e.message || "保存失败"}`);
        }
    };

    const handleSearchPkg = async () => {
        if (!pkgSearch.trim()) return;
        try {
            const res = await fetch(
                `/package/search?name=${encodeURIComponent(pkgSearch.trim())}`,
            );
            const d = await res.json();
            if (d.data && Array.isArray(d.data)) {
                setPkgSearchResults(
                    d.data.map((item: any) => item.name || item),
                );
            }
        } catch {
            setPkgSearchResults([]);
        }
    };

    const handleInstallPkg = async (name: string) => {
        setPkgInstalling(name);
        try {
            await fetch("/package/install", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ name }),
            });
        } catch {}
        setTimeout(() => {
            setPkgInstalling(null);
            fetch("/package/local")
                .then((r) => r.json())
                .then((d) => {
                    if (d.data && d.data.user) setInstalledPkgs(d.data.user);
                })
                .catch(() => {});
        }, 3000);
    };

    const handleUninstallPkg = async (name: string) => {
        try {
            await fetch("/package/uninstall", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ name }),
            });
            setInstalledPkgs((prev) => prev.filter((p) => p.name !== name));
        } catch {}
    };

    if (loading) {
        return (
            <div className="min-h-screen bg-canvas-soft flex items-center justify-center">
                <span className="text-accents-5 text-sm">加载中...</span>
            </div>
        );
    }

    const tabs: { key: Tab; label: string }[] = [
        { key: "settings", label: "设置" },
        { key: "history", label: "运行历史" },
        { key: "packages", label: "依赖管理" },
        { key: "v2", label: "V2 仪表盘" },
    ];

    return (
        <div className="min-h-screen bg-canvas-soft flex items-start justify-center p-4 pt-16">
            <div className="w-full max-w-2xl flex flex-col gap-4">
                <div className="flex gap-1 bg-canvas rounded-md p-1 shadow-[0_0_0_1px_rgba(0,0,0,0.08)]">
                    {tabs.map((t) => (
                        <button
                            key={t.key}
                            onClick={() => setActiveTab(t.key)}
                            className={`flex-1 h-8 text-sm font-medium rounded-md transition-colors ${
                                activeTab === t.key
                                    ? "bg-ink text-canvas"
                                    : "text-accents-6 hover:text-ink"
                            }`}
                        >
                            {t.label}
                        </button>
                    ))}
                    <button
                        onClick={() => setDarkMode(!darkMode)}
                        className="w-8 h-8 flex items-center justify-center rounded-md text-accents-6 hover:text-ink transition-colors flex-shrink-0"
                        title={darkMode ? "切换到亮色模式" : "切换到暗色模式"}
                    >
                        {darkMode ? "☀" : "🌙"}
                    </button>
                </div>

                <div className="bg-canvas rounded-lg shadow-[0_0_0_1px_rgba(0,0,0,0.08),0_2px_4px_rgba(0,0,0,0.03),0_4px_8px_rgba(0,0,0,0.04)] p-6">
                    {activeTab === "settings" && (
                        <div className="flex flex-col gap-6">
                            <h1 className="text-2xl font-semibold text-ink tracking-[-0.03em]">
                                更好的学而思编程助手
                            </h1>
                            {error && (
                                <div className="bg-error-bg border border-error-border rounded-md px-4 py-3">
                                    <div className="flex items-center gap-2">
                                        <span className="text-sm font-medium text-error-text">错误</span>
                                    </div>
                                    <p className="text-sm text-error-text mt-1.5">
                                        {error.message}
                                    </p>
                                </div>
                            )}
                            {serverOk && (
                                <div className="bg-success-bg border border-success-border rounded-md px-4 py-3">
                                    <div className="flex items-center gap-2">
                                        <span className="text-sm font-medium text-success-text">服务正常运行</span>
                                    </div>
                                </div>
                            )}
                            <div className="flex flex-col gap-5">
                                <PathSelector
                                    label="Python 解释器"
                                    paths={pythonPaths}
                                    onSelect={handleSelect}
                                    name="pythonPath"
                                />
                                <PathSelector
                                    label="Golang 编译器"
                                    paths={golangPaths}
                                    onSelect={handleGolangSelect}
                                    name="golangPath"
                                />
                                <PathSelector
                                    label="Bun 运行时"
                                    paths={bunPaths}
                                    onSelect={handleBunSelect}
                                    name="bunPath"
                                />
                            </div>
                            <div className="flex justify-end pt-4 border-t border-hairline">
                                <button
                                    onClick={handleSave}
                                    disabled={saving}
                                    className="h-8 px-4 bg-ink hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 text-canvas text-sm font-medium rounded-md transition-colors"
                                >
                                    {saving ? "保存中..." : "保存设置"}
                                </button>
                            </div>
                        </div>
                    )}

                    {activeTab === "history" && (
                        <div className="flex flex-col gap-4">
                            <div className="flex items-center justify-between">
                                <h2 className="text-lg font-semibold text-ink tracking-[-0.02em]">
                                    运行历史
                                </h2>
                                {history.length > 0 && (
                                    <button
                                        onClick={handleClearHistory}
                                        className="h-7 px-3 text-xs font-medium text-accents-5 hover:text-error-text rounded-md border border-hairline hover:border-error-border transition-colors"
                                    >
                                        清空历史
                                    </button>
                                )}
                            </div>
                            {history.length === 0 ? (
                                <p className="text-sm text-accents-5 py-8 text-center">
                                    暂无运行记录
                                </p>
                            ) : (
                                <div className="flex flex-col gap-2">
                                    {history.map((r) => (
                                        <HistoryItem
                                            key={r.id}
                                            r={r}
                                            expanded={expandedId === r.id}
                                            onToggle={() =>
                                                setExpandedId(
                                                    expandedId === r.id
                                                        ? null
                                                        : r.id,
                                                )
                                            }
                                            onDelete={() =>
                                                handleDeleteHistory(r.id)
                                            }
                                            onSelect={() => setDepGraphRunId(r.id)}
                                            selectedForGraph={depGraphRunId === r.id}
                                            aiExplanation={
                                                aiExplainById[r.id] ??
                                                r.ai_explanation
                                            }
                                            aiLoading={aiExplainLoading === r.id}
                                            onAiExplain={() => handleAiExplain(r.id)}
                                        />
                                    ))}
                                </div>
                            )}
                            {history.length > 0 && (
                                <div className="border-t border-hairline pt-4 mt-2">
                                    <h3 className="text-sm font-semibold text-ink mb-2">
                                        依赖图
                                        <span className="text-xs text-accents-5 ml-2 font-normal">
                                            (基于选中运行记录的 import 与已安装包解析)
                                        </span>
                                    </h3>
                                    <div className="flex items-center gap-2 mb-3">
                                        <select
                                            value={depGraphRunId}
                                            onChange={(e) => setDepGraphRunId(e.target.value)}
                                            className="flex-1 h-8 px-3 text-sm text-ink bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                        >
                                            <option value="">选择一次运行…</option>
                                            {history.map((r) => (
                                                <option key={r.id} value={r.id}>
                                                    {formatTime(r.timestamp)} · {r.success ? "✓" : "✗"} · {r.code.slice(0, 40)}
                                                </option>
                                            ))}
                                        </select>
                                    </div>
                                    {depGraphMsg && (
                                        <p className="text-xs text-accents-5">{depGraphMsg}</p>
                                    )}
                                    {depGraph && (
                                        <div className="grid grid-cols-2 gap-3 text-xs">
                                            <div>
                                                <div className="text-[11px] font-medium text-accents-5 font-mono mb-1">
                                                    节点 ({depGraph.nodes.length})
                                                </div>
                                                <div className="flex flex-col gap-1 max-h-60 overflow-y-auto">
                                                    {depGraph.nodes.length === 0 && (
                                                        <span className="text-accents-5">无</span>
                                                    )}
                                                    {depGraph.nodes.map((n) => (
                                                        <div
                                                            key={n.name}
                                                            className="flex justify-between px-2 py-1 border border-hairline rounded"
                                                        >
                                                            <span className="text-accents-7 font-mono">{n.name}</span>
                                                            <span className="text-accents-5">{n.version || "?"}</span>
                                                        </div>
                                                    ))}
                                                </div>
                                            </div>
                                            <div>
                                                <div className="text-[11px] font-medium text-accents-5 font-mono mb-1">
                                                    依赖边 ({depGraph.edges.length})
                                                </div>
                                                <div className="flex flex-col gap-1 max-h-60 overflow-y-auto font-mono">
                                                    {depGraph.edges.length === 0 && (
                                                        <span className="text-accents-5">无</span>
                                                    )}
                                                    {depGraph.edges.map((e, i) => (
                                                        <div key={i} className="text-accents-6">
                                                            {e.from} → {e.to}
                                                        </div>
                                                    ))}
                                                </div>
                                            </div>
                                        </div>
                                    )}
                                </div>
                            )}
                        </div>
                    )}

                    {activeTab === "packages" && (
                        <div className="flex flex-col gap-5">
                            <h2 className="text-lg font-semibold text-ink tracking-[-0.02em]">
                                依赖管理
                            </h2>
                            <div className="flex gap-2">
                                <input
                                    type="text"
                                    value={pkgSearch}
                                    onChange={(e) => setPkgSearch(e.target.value)}
                                    onKeyDown={(e) => e.key === "Enter" && handleSearchPkg()}
                                    placeholder="搜索 Python 包…"
                                    className="flex-1 h-8 px-3 text-sm text-ink bg-canvas border border-hairline rounded-md outline-none focus:border-ink transition-colors"
                                />
                                <button
                                    onClick={handleSearchPkg}
                                    className="h-8 px-3 bg-ink text-canvas text-sm font-medium rounded-md hover:bg-accents-7 transition-colors"
                                >
                                    搜索
                                </button>
                            </div>
                            {pkgSearchResults.length > 0 && (
                                <div className="flex flex-col gap-1.5">
                                    <span className="text-[11px] font-medium text-accents-5 font-mono">
                                        搜索结果
                                    </span>
                                    {pkgSearchResults.map((name) => {
                                        const isInstalled = installedPkgs.some(
                                            (p) => p.name === name,
                                        );
                                        return (
                                            <div
                                                key={name}
                                                className="flex items-center justify-between px-3 py-2 border border-hairline rounded-md"
                                            >
                                                <span className="text-sm text-accents-7">{name}</span>
                                                {isInstalled ? (
                                                    <span className="text-xs text-accents-5">已安装</span>
                                                ) : (
                                                    <button
                                                        onClick={() => handleInstallPkg(name)}
                                                        disabled={pkgInstalling === name}
                                                        className="h-6 px-2.5 text-xs font-medium bg-ink text-canvas rounded-md hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 transition-colors"
                                                    >
                                                        {pkgInstalling === name ? "安装中…" : "安装"}
                                                    </button>
                                                )}
                                            </div>
                                        );
                                    })}
                                </div>
                            )}
                            <div className="flex flex-col gap-1.5">
                                <span className="text-[11px] font-medium text-accents-5 font-mono">
                                    已安装的包 ({installedPkgs.length})
                                </span>
                                {installedPkgs.length === 0 ? (
                                    <p className="text-sm text-accents-5 py-4 text-center">暂无已安装的包</p>
                                ) : (
                                    <div className="flex flex-col gap-1">
                                        {installedPkgs.map((p) => (
                                            <div
                                                key={p.name}
                                                className="flex items-center justify-between px-3 py-2 border border-hairline rounded-md"
                                            >
                                                <span className="text-sm text-accents-7">{p.name}</span>
                                                <div className="flex items-center gap-2">
                                                    <span className="text-xs text-accents-5">{p.version}</span>
                                                    <button
                                                        onClick={() => handleUninstallPkg(p.name)}
                                                        className="text-xs text-accents-5 hover:text-error-text transition-colors"
                                                    >
                                                        卸载
                                                    </button>
                                                </div>
                                            </div>
                                        ))}
                                    </div>
                                )}
                            </div>
                        </div>
                    )}

                    {activeTab === "v2" && (
                        <div className="flex flex-col gap-5">
                            <h2 className="text-lg font-semibold text-ink tracking-[-0.02em]">
                                V2 仪表盘
                                <span className="text-xs text-accents-5 ml-2 font-normal">
                                    {dashboard?.server_version ? `v${dashboard.server_version}` : ""}
                                </span>
                            </h2>

                            {/* 指标看板 */}
                            <Section title="运行指标" hint="2 秒自动刷新">
                                {dashboard ? (
                                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                                        <Stat label="总运行" value={String(dashboard.history_total)} />
                                        <Stat
                                            label="成功率"
                                            value={`${(dashboard.success_rate * 100).toFixed(1)}%`}
                                            tone={dashboard.success_rate >= 0.8 ? "ok" : dashboard.success_rate >= 0.5 ? "warn" : "err"}
                                        />
                                        <Stat label="成功" value={String(dashboard.history_success)} tone="ok" />
                                        <Stat label="失败" value={String(dashboard.history_failed)} tone={dashboard.history_failed > 0 ? "err" : undefined} />
                                        <Stat label="24h 运行" value={String(dashboard.last_24h_runs)} />
                                        <Stat label="平均时长" value={`${dashboard.summary.avg_duration_ms}ms`} />
                                        <Stat label="总次数" value={String(dashboard.summary.total_runs)} />
                                        <Stat label="总平均峰值" value={formatBytes(dashboard.summary.avg_peak_rss_bytes)} />
                                    </div>
                                ) : (
                                    <p className="text-xs text-accents-5">加载中…</p>
                                )}
                                {dashboard && dashboard.top_failing_imports.length > 0 && (
                                    <div className="mt-3">
                                        <div className="text-[11px] font-medium text-accents-5 font-mono mb-1">
                                            失败最多的 import
                                        </div>
                                        <div className="flex flex-wrap gap-1">
                                            {dashboard.top_failing_imports.map(([name, n]) => (
                                                <span
                                                    key={name}
                                                    className="text-[11px] px-2 py-0.5 bg-error-bg text-error-text rounded"
                                                >
                                                    {name} ×{n}
                                                </span>
                                            ))}
                                        </div>
                                    </div>
                                )}
                            </Section>

                            {/* AI 助手 */}
                            <Section
                                title="AI 助手"
                                hint={aiStatus ? (aiStatus.configured ? `已配置 · ${aiStatus.model}` : "未配置") : ""}
                            >
                                {aiSettings ? (
                                    <div className="flex flex-col gap-2">
                                        <Toggle
                                            label="启用 AI"
                                            checked={aiSettings.enabled}
                                            onChange={(v) =>
                                                setAiSettings({ ...aiSettings, enabled: v })
                                            }
                                        />
                                        <Field label="Base URL">
                                            <input
                                                className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                value={aiSettings.base_url}
                                                onChange={(e) =>
                                                    setAiSettings({ ...aiSettings, base_url: e.target.value })
                                                }
                                                placeholder="https://api.openai.com/v1"
                                            />
                                        </Field>
                                        <Field label="API Key">
                                            <input
                                                type="password"
                                                className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                value={aiSettings.api_key}
                                                onChange={(e) =>
                                                    setAiSettings({ ...aiSettings, api_key: e.target.value })
                                                }
                                                placeholder="(留空保留现有,填 *** 不变,清空则删除)"
                                            />
                                        </Field>
                                        <Field label="Model">
                                            <input
                                                className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                value={aiSettings.model}
                                                onChange={(e) =>
                                                    setAiSettings({ ...aiSettings, model: e.target.value })
                                                }
                                                placeholder="gpt-4o-mini"
                                            />
                                        </Field>
                                        <div className="grid grid-cols-3 gap-2">
                                            <Field label="Timeout (s)">
                                                <input
                                                    type="number"
                                                    className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                    value={aiSettings.timeout_secs}
                                                    onChange={(e) =>
                                                        setAiSettings({
                                                            ...aiSettings,
                                                            timeout_secs: Number(e.target.value) || 30,
                                                        })
                                                    }
                                                />
                                            </Field>
                                            <Field label="Max Tokens">
                                                <input
                                                    type="number"
                                                    className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                    value={aiSettings.max_tokens}
                                                    onChange={(e) =>
                                                        setAiSettings({
                                                            ...aiSettings,
                                                            max_tokens: Number(e.target.value) || 1024,
                                                        })
                                                    }
                                                />
                                            </Field>
                                            <Field label="Temperature">
                                                <input
                                                    type="number"
                                                    step="0.1"
                                                    min="0"
                                                    max="2"
                                                    className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                    value={aiSettings.temperature}
                                                    onChange={(e) =>
                                                        setAiSettings({
                                                            ...aiSettings,
                                                            temperature: Number(e.target.value) || 0,
                                                        })
                                                    }
                                                />
                                            </Field>
                                        </div>
                                        <Toggle
                                            label="运行失败时自动 AI 解释"
                                            checked={aiSettings.auto_explain_on_error}
                                            onChange={(v) =>
                                                setAiSettings({ ...aiSettings, auto_explain_on_error: v })
                                            }
                                        />
                                        <Field label="系统提示词 (可选)">
                                            <textarea
                                                className="w-full px-3 py-2 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                rows={2}
                                                value={aiSettings.system_prompt}
                                                onChange={(e) =>
                                                    setAiSettings({ ...aiSettings, system_prompt: e.target.value })
                                                }
                                            />
                                        </Field>
                                        <div className="flex items-center gap-2">
                                            <button
                                                onClick={handleSaveAiSettings}
                                                disabled={aiBusy}
                                                className="h-8 px-3 bg-ink text-canvas text-sm font-medium rounded-md hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 transition-colors"
                                            >
                                                {aiBusy ? "保存中…" : "保存配置"}
                                            </button>
                                            {aiMsg && (
                                                <span className={`text-xs ${aiMsg.startsWith("❌") ? "text-error-text" : "text-success-text"}`}>
                                                    {aiMsg}
                                                </span>
                                            )}
                                        </div>
                                    </div>
                                ) : (
                                    <p className="text-xs text-accents-5">加载中…</p>
                                )}
                            </Section>

                            {/* 自更新 */}
                            <Section title="自更新" hint={updater ? `当前 v${updater.current_version}` : ""}>
                                {updater ? (
                                    <div className="flex flex-col gap-2">
                                        <div className="grid grid-cols-2 gap-2 text-xs">
                                            <span className="text-accents-5">仓库</span>
                                            <span className="text-accents-7 font-mono">{updater.repo}</span>
                                            <span className="text-accents-5">目标资产</span>
                                            <span className="text-accents-7 font-mono">{updater.target_asset || "(无匹配)"}</span>
                                            <span className="text-accents-5">最新版本</span>
                                            <span className="text-accents-7 font-mono">
                                                {updater.cached_release?.tag_name || "(未检查)"}
                                            </span>
                                        </div>
                                        {updater.update_available && (
                                            <div className="text-xs px-2 py-1 bg-success-bg text-success-text rounded">
                                                ⬆ 发现新版本,点击「应用更新」下载并替换
                                            </div>
                                        )}
                                        <div className="flex gap-2">
                                            <button
                                                onClick={handleCheckUpdate}
                                                disabled={updaterBusy}
                                                className="h-8 px-3 bg-ink text-canvas text-sm font-medium rounded-md hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 transition-colors"
                                            >
                                                {updaterBusy ? "处理中…" : "检查更新"}
                                            </button>
                                            <button
                                                onClick={handleApplyUpdate}
                                                disabled={updaterBusy || !updater.update_available}
                                                className="h-8 px-3 bg-ink text-canvas text-sm font-medium rounded-md hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 transition-colors"
                                            >
                                                应用更新
                                            </button>
                                        </div>
                                        {updaterMsg && (
                                            <span className={`text-xs ${updaterMsg.startsWith("❌") ? "text-error-text" : "text-accents-6"}`}>
                                                {updaterMsg}
                                            </span>
                                        )}
                                        {updater.cached_release?.body && (
                                            <details className="text-xs">
                                                <summary className="cursor-pointer text-accents-6">发布说明</summary>
                                                <pre className="mt-2 p-2 bg-canvas-soft rounded text-accents-7 whitespace-pre-wrap max-h-40 overflow-y-auto">
                                                    {updater.cached_release.body}
                                                </pre>
                                            </details>
                                        )}
                                    </div>
                                ) : (
                                    <p className="text-xs text-accents-5">加载中…</p>
                                )}
                            </Section>

                            {/* 沙箱 */}
                            <Section
                                title="进程沙箱"
                                hint={sandboxReport ? `后端 ${sandboxReport.backend}` : ""}
                            >
                                {sandboxSettings ? (
                                    <div className="flex flex-col gap-2">
                                        <Toggle
                                            label="启用沙箱"
                                            checked={sandboxSettings.enabled}
                                            onChange={(v) =>
                                                setSandboxSettings({ ...sandboxSettings, enabled: v })
                                            }
                                        />
                                        <div className="grid grid-cols-2 gap-2">
                                            <Field label="模式 (auto/bwrap/unshare/sandbox-exec/process-group)">
                                                <input
                                                    className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                    value={sandboxSettings.mode}
                                                    onChange={(e) =>
                                                        setSandboxSettings({ ...sandboxSettings, mode: e.target.value })
                                                    }
                                                />
                                            </Field>
                                            <Field label="CPU 时间限制 (秒)">
                                                <input
                                                    type="number"
                                                    className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                    value={sandboxSettings.cpu_time_limit_secs}
                                                    onChange={(e) =>
                                                        setSandboxSettings({
                                                            ...sandboxSettings,
                                                            cpu_time_limit_secs: Number(e.target.value) || 0,
                                                        })
                                                    }
                                                />
                                            </Field>
                                        </div>
                                        <Field label="内存限制 (字节,0=不限制)">
                                            <input
                                                type="number"
                                                className="w-full h-8 px-3 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink"
                                                value={sandboxSettings.memory_limit_bytes}
                                                onChange={(e) =>
                                                    setSandboxSettings({
                                                        ...sandboxSettings,
                                                        memory_limit_bytes: Number(e.target.value) || 0,
                                                    })
                                                }
                                            />
                                        </Field>
                                        <Toggle
                                            label="禁用网络"
                                            checked={sandboxSettings.no_network}
                                            onChange={(v) =>
                                                setSandboxSettings({ ...sandboxSettings, no_network: v })
                                            }
                                        />
                                        <Field label="可写路径 (每行一个)">
                                            <textarea
                                                className="w-full px-3 py-2 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink font-mono"
                                                rows={2}
                                                value={sandboxSettings.writable_paths.join("\n")}
                                                onChange={(e) =>
                                                    setSandboxSettings({
                                                        ...sandboxSettings,
                                                        writable_paths: e.target.value.split(/\r?\n/).filter(Boolean),
                                                    })
                                                }
                                            />
                                        </Field>
                                        <Field label="只读路径 (每行一个)">
                                            <textarea
                                                className="w-full px-3 py-2 text-sm bg-canvas border border-hairline rounded-md outline-none focus:border-ink font-mono"
                                                rows={2}
                                                value={sandboxSettings.read_only_paths.join("\n")}
                                                onChange={(e) =>
                                                    setSandboxSettings({
                                                        ...sandboxSettings,
                                                        read_only_paths: e.target.value.split(/\r?\n/).filter(Boolean),
                                                    })
                                                }
                                            />
                                        </Field>
                                        <div className="flex items-center gap-2">
                                            <button
                                                onClick={handleSaveSandbox}
                                                className="h-8 px-3 bg-ink text-canvas text-sm font-medium rounded-md hover:bg-accents-7 transition-colors"
                                            >
                                                保存沙箱配置
                                            </button>
                                            {sandboxMsg && (
                                                <span className={`text-xs ${sandboxMsg.startsWith("❌") ? "text-error-text" : "text-success-text"}`}>
                                                    {sandboxMsg}
                                                </span>
                                            )}
                                        </div>
                                        {sandboxReport && (
                                            <div className="text-[11px] text-accents-5 font-mono mt-1">
                                                实际生效: {sandboxReport.effective ? "✅" : "❌"}
                                                {sandboxReport.notes.length > 0 && (
                                                    <ul className="mt-1 list-disc pl-4">
                                                        {sandboxReport.notes.map((n, i) => (
                                                            <li key={i}>{n}</li>
                                                        ))}
                                                    </ul>
                                                )}
                                            </div>
                                        )}
                                    </div>
                                ) : (
                                    <p className="text-xs text-accents-5">加载中…</p>
                                )}
                            </Section>
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}

function PathSelector({
    label,
    paths,
    onSelect,
    name,
}: {
    label: string;
    paths: { path: string; selected: boolean }[];
    onSelect: (path: string) => void;
    name: string;
}) {
    return (
        <div className="flex flex-col gap-2">
            <span className="text-[13px] font-medium text-accents-6 font-mono">{label}</span>
            <div className="flex flex-col gap-1.5">
                {paths.map((p) => (
                    <label
                        key={p.path}
                        className={`flex items-center gap-3 px-3 py-2.5 rounded-md border cursor-pointer transition-colors ${
                            p.selected
                                ? "border-ink bg-canvas-soft"
                                : "border-hairline hover:border-accents-4"
                        }`}
                    >
                        <span
                            className={`w-4 h-4 rounded-full border-2 flex items-center justify-center flex-shrink-0 transition-colors ${
                                p.selected ? "border-ink" : "border-accents-4"
                            }`}
                        >
                            {p.selected && <span className="w-2 h-2 rounded-full bg-ink" />}
                        </span>
                        <span className="text-sm text-accents-7">{p.path}</span>
                        <input
                            type="radio"
                            name={name}
                            checked={p.selected}
                            onChange={() => onSelect(p.path)}
                            className="sr-only"
                        />
                    </label>
                ))}
            </div>
        </div>
    );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "ok" | "warn" | "err" }) {
    const color =
        tone === "ok"
            ? "text-success-text"
            : tone === "warn"
              ? "text-amber-500"
              : tone === "err"
                ? "text-error-text"
                : "text-ink";
    return (
        <div className="border border-hairline rounded-md px-3 py-2">
            <div className="text-[10px] text-accents-5 font-mono uppercase tracking-wide">{label}</div>
            <div className={`text-lg font-semibold ${color} tabular-nums`}>{value}</div>
        </div>
    );
}

function Section({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
    return (
        <div className="border border-hairline rounded-md p-4 flex flex-col gap-2">
            <div className="flex items-baseline justify-between">
                <h3 className="text-sm font-semibold text-ink">{title}</h3>
                {hint && <span className="text-[11px] text-accents-5 font-mono">{hint}</span>}
            </div>
            {children}
        </div>
    );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
    return (
        <div>
            <span className="block text-[11px] font-medium text-accents-5 font-mono mb-1">{label}</span>
            {children}
        </div>
    );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
    return (
        <label className="flex items-center gap-2 cursor-pointer">
            <input
                type="checkbox"
                checked={checked}
                onChange={(e) => onChange(e.target.checked)}
                className="w-4 h-4"
            />
            <span className="text-sm text-accents-7">{label}</span>
        </label>
    );
}

function HistoryItem({
    r,
    expanded,
    onToggle,
    onDelete,
    onSelect,
    selectedForGraph,
    aiExplanation,
    aiLoading,
    onAiExplain,
}: {
    r: RunRecord;
    expanded: boolean;
    onToggle: () => void;
    onDelete: () => void;
    onSelect: () => void;
    selectedForGraph: boolean;
    aiExplanation?: string | null;
    aiLoading: boolean;
    onAiExplain: () => void;
}) {
    return (
        <div
            className={`border rounded-md overflow-hidden ${
                selectedForGraph ? "border-ink" : "border-hairline"
            }`}
        >
            <button
                onClick={onToggle}
                className="w-full flex items-center gap-3 px-3 py-2.5 hover:bg-canvas-soft transition-colors text-left"
            >
                <span
                    className={`w-2 h-2 rounded-full flex-shrink-0 ${
                        r.success ? "bg-success-text" : "bg-error-text"
                    }`}
                />
                <span className="text-sm text-accents-7 flex-1 truncate">
                    {r.code.slice(0, 60)}
                    {r.code.length > 60 ? "…" : ""}
                </span>
                {r.sandboxed && (
                    <span className="text-[10px] px-1.5 py-0.5 bg-canvas-soft text-accents-5 rounded font-mono flex-shrink-0">
                        沙箱
                    </span>
                )}
                <span className="text-xs text-accents-5 flex-shrink-0">
                    {formatTime(r.timestamp)}
                </span>
                <span className="text-xs text-accents-5 flex-shrink-0">
                    {formatDuration(r.duration)}
                </span>
                <button
                    onClick={(e) => {
                        e.stopPropagation();
                        onDelete();
                    }}
                    className="text-accents-5 hover:text-error-text flex-shrink-0"
                >
                    ✕
                </button>
            </button>
            {expanded && (
                <div className="border-t border-hairline px-3 py-3 flex flex-col gap-3">
                    <div className="flex flex-wrap items-center gap-2 text-[11px] text-accents-5 font-mono">
                        {r.exit_code != null && <span>exit={r.exit_code}</span>}
                        {r.peak_rss_bytes != null && r.peak_rss_bytes > 0 && (
                            <span>RSS={formatBytes(r.peak_rss_bytes)}</span>
                        )}
                        {r.auto_installs != null && r.auto_installs > 0 && (
                            <span>自动安装 ×{r.auto_installs}</span>
                        )}
                        {r.lint_issues != null && r.lint_issues > 0 && (
                            <span>lint ×{r.lint_issues}</span>
                        )}
                        {r.imports && r.imports.length > 0 && (
                            <span>
                                imports:{" "}
                                {r.imports.slice(0, 5).map((i) => (
                                    <span key={i} className="text-accents-6">
                                        {i}{" "}
                                    </span>
                                ))}
                                {r.imports.length > 5 && `+${r.imports.length - 5}`}
                            </span>
                        )}
                    </div>
                    <div>
                        <span className="text-[11px] font-medium text-accents-5 font-mono">代码</span>
                        <pre className="mt-1 text-xs text-accents-7 bg-canvas-soft rounded-md p-3 overflow-x-auto max-h-48 whitespace-pre-wrap">
                            {r.code}
                        </pre>
                    </div>
                    <div>
                        <span className="text-[11px] font-medium text-accents-5 font-mono">输出</span>
                        <pre className="mt-1 text-xs text-accents-7 bg-canvas-soft rounded-md p-3 overflow-x-auto max-h-48 whitespace-pre-wrap">
                            {r.output ? colorizeOutput(r.output) : "(无输出)"}
                        </pre>
                    </div>
                    <div className="flex flex-wrap gap-2 pt-1 border-t border-hairline">
                        {!r.success && (
                            <button
                                onClick={onAiExplain}
                                disabled={aiLoading}
                                className="h-7 px-3 text-xs font-medium bg-ink text-canvas rounded-md hover:bg-accents-7 disabled:bg-accents-4 disabled:text-accents-5 transition-colors"
                            >
                                {aiLoading ? "解释中…" : "🤖 AI 解释"}
                            </button>
                        )}
                        <button
                            onClick={onSelect}
                            className="h-7 px-3 text-xs font-medium text-accents-6 hover:text-ink rounded-md border border-hairline hover:border-accents-4 transition-colors"
                        >
                            📊 查看依赖图
                        </button>
                    </div>
                    {aiExplanation && (
                        <div>
                            <span className="text-[11px] font-medium text-accents-5 font-mono">
                                AI 解释
                            </span>
                            <pre className="mt-1 text-xs text-accents-7 bg-canvas-soft rounded-md p-3 overflow-x-auto max-h-60 whitespace-pre-wrap">
                                {aiExplanation}
                            </pre>
                        </div>
                    )}
                </div>
            )}
        </div>
    );
}
