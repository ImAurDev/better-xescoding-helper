use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::config::{cache_dir, history_file, MAX_HISTORY_RECORDS};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub timestamp: i64,
    pub code: String,
    pub output: String,
    pub has_go_blocks: bool,
    pub success: bool,
    pub duration: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default)]
    pub peak_rss_bytes: u64,
    #[serde(default)]
    pub auto_installs: u32,
    #[serde(default)]
    pub lint_issues: u32,
    #[serde(default)]
    pub missing_imports_resolved: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_overrides: Option<std::collections::HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<RunPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_explanation: Option<String>,
    #[serde(default)]
    pub sandboxed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunPackage {
    pub name: String,
    pub version: Option<String>,
    #[serde(default)]
    pub required_by: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunGraph {
    pub run_id: String,
    pub nodes: Vec<RunPackage>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

pub struct HistoryStore {
    conn: Arc<StdMutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl HistoryStore {
    pub fn new() -> Self {
        let path = db_path();
        let conn = Connection::open(&path).unwrap_or_else(|e| {
            tracing::error!("打开 SQLite 失败,使用临时内存库: {e}");
            Connection::open_in_memory().expect("in-memory sqlite")
        });
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        let store = Self {
            conn: Arc::new(StdMutex::new(conn)),
            path,
        };
        store.migrate();
        store
    }

    pub async fn init(&mut self) {
        if let Some(parent) = self.path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
    }

    fn migrate(&self) {
        let conn = self.conn.lock().expect("history conn");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS runs (
                id TEXT PRIMARY KEY,
                timestamp INTEGER NOT NULL,
                code TEXT NOT NULL,
                output TEXT NOT NULL,
                has_go_blocks INTEGER NOT NULL DEFAULT 0,
                success INTEGER NOT NULL,
                duration INTEGER NOT NULL DEFAULT 0,
                project_id TEXT,
                peak_rss_bytes INTEGER NOT NULL DEFAULT 0,
                auto_installs INTEGER NOT NULL DEFAULT 0,
                lint_issues INTEGER NOT NULL DEFAULT 0,
                missing_imports_resolved INTEGER NOT NULL DEFAULT 0,
                exit_code INTEGER,
                env_overrides TEXT,
                ai_explanation TEXT,
                sandboxed INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_runs_ts ON runs(timestamp DESC);
            CREATE INDEX IF NOT EXISTS idx_runs_project ON runs(project_id);

            CREATE TABLE IF NOT EXISTS run_imports (
                run_id TEXT NOT NULL,
                module TEXT NOT NULL,
                PRIMARY KEY(run_id, module),
                FOREIGN KEY(run_id) REFERENCES runs(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS run_packages (
                run_id TEXT NOT NULL,
                name TEXT NOT NULL,
                version TEXT,
                PRIMARY KEY(run_id, name),
                FOREIGN KEY(run_id) REFERENCES runs(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS package_deps (
                run_id TEXT NOT NULL,
                parent TEXT NOT NULL,
                child TEXT NOT NULL,
                PRIMARY KEY(run_id, parent, child),
                FOREIGN KEY(run_id) REFERENCES runs(id) ON DELETE CASCADE
            );
            "#,
        )
        .ok();
    }

    pub async fn add(&mut self, record: RunRecord) {
        let _ = self.add_with_imports(record, Vec::new(), Vec::new()).await;
    }

    pub async fn add_with_imports(
        &mut self,
        record: RunRecord,
        imports: Vec<String>,
        packages: Vec<RunPackage>,
    ) {
        let conn = self.conn.clone();
        let res = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            insert_run(&conn, &record, &imports, &packages)
        })
        .await;
        if let Err(e) = res {
            tracing::error!("history add join error: {e}");
        }
        let _ = MAX_HISTORY_RECORDS;
    }

    pub async fn list(&self) -> Vec<RunRecord> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            list_runs(&conn)
        })
        .await
        .unwrap_or_default()
    }

    pub async fn get(&self, id: &str) -> Option<RunRecord> {
        let conn = self.conn.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            get_run(&conn, &id)
        })
        .await
        .ok()
        .flatten()
    }

    pub async fn get_run_imports(&self, id: &str) -> Vec<String> {
        let conn = self.conn.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            let mut stmt = match conn.prepare("SELECT module FROM run_imports WHERE run_id=?1 ORDER BY module") {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            stmt.query_map(params![id], |r| r.get::<_, String>(0))
                .map(|rows| rows.filter_map(|x| x.ok()).collect())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    pub async fn get_run_graph(&self, id: &str) -> RunGraph {
        let conn = self.conn.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            let mut graph = RunGraph {
                run_id: id.clone(),
                ..Default::default()
            };
            if let Ok(mut stmt) = conn.prepare(
                "SELECT name, version FROM run_packages WHERE run_id=?1 ORDER BY name",
            ) {
                if let Ok(rows) = stmt.query_map(params![id], |r| {
                    Ok(RunPackage {
                        name: r.get::<_, String>(0)?,
                        version: r.get::<_, Option<String>>(1)?,
                        required_by: Vec::new(),
                        requires: Vec::new(),
                    })
                }) {
                    for p in rows.flatten() {
                        graph.nodes.push(p);
                    }
                }
            }
            if let Ok(mut stmt) = conn.prepare(
                "SELECT parent, child FROM package_deps WHERE run_id=?1 ORDER BY parent, child",
            ) {
                if let Ok(rows) = stmt.query_map(params![id], |r| {
                    Ok(GraphEdge {
                        from: r.get::<_, String>(0)?,
                        to: r.get::<_, String>(1)?,
                    })
                }) {
                    for e in rows.flatten() {
                        graph.edges.push(e);
                    }
                }
            }
            graph
        })
        .await
        .unwrap_or_default()
    }

    pub async fn attach_ai_explanation(&self, id: &str, explanation: &str) {
        let conn = self.conn.clone();
        let id = id.to_string();
        let explanation = explanation.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            conn.execute(
                "UPDATE runs SET ai_explanation=?1 WHERE id=?2",
                params![explanation, id],
            )
            .ok();
        })
        .await;
    }

    pub async fn clear(&mut self) {
        let conn = self.conn.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            conn.execute("DELETE FROM runs", []).ok();
            conn.execute("DELETE FROM run_imports", []).ok();
            conn.execute("DELETE FROM run_packages", []).ok();
            conn.execute("DELETE FROM package_deps", []).ok();
        })
        .await;
    }

    pub async fn delete(&mut self, id: &str) -> bool {
        let conn = self.conn.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            let n = conn
                .execute("DELETE FROM runs WHERE id=?1", params![id])
                .unwrap_or(0);
            n > 0
        })
        .await
        .unwrap_or(false)
    }

    pub async fn export(&self, format: &str) -> Result<String, String> {
        let records = self.list().await;
        match format {
            "json" => Ok(serde_json::to_string_pretty(&records).unwrap_or_default()),
            "csv" => Ok(to_csv(&records)),
            "md" | "markdown" => Ok(to_markdown(&records)),
            _ => Err(format!("不支持的导出格式: {format}")),
        }
    }

    pub async fn recent(&self, limit: usize) -> Vec<RunRecord> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("history conn");
            let mut stmt = match conn.prepare(
                "SELECT id, timestamp, code, output, has_go_blocks, success, duration, project_id,
                        peak_rss_bytes, auto_installs, lint_issues, missing_imports_resolved,
                        exit_code, env_overrides, ai_explanation, sandboxed
                 FROM runs ORDER BY timestamp DESC LIMIT ?1",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            let rows = stmt
                .query_map(params![limit as i64], |r| {
                    Ok(RunRecord {
                        id: r.get(0)?,
                        timestamp: r.get(1)?,
                        code: r.get(2)?,
                        output: r.get(3)?,
                        has_go_blocks: r.get::<_, i64>(4)? != 0,
                        success: r.get::<_, i64>(5)? != 0,
                        duration: r.get(6)?,
                        project_id: r.get(7)?,
                        peak_rss_bytes: r.get::<_, i64>(8)? as u64,
                        auto_installs: r.get::<_, i64>(9)? as u32,
                        lint_issues: r.get::<_, i64>(10)? as u32,
                        missing_imports_resolved: r.get::<_, i64>(11)? as u32,
                        exit_code: r.get(12)?,
                        env_overrides: r
                            .get::<_, Option<String>>(13)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        imports: Vec::new(),
                        packages: Vec::new(),
                        ai_explanation: r.get(14)?,
                        sandboxed: r.get::<_, i64>(15)? != 0,
                    })
                })
                .ok();
            rows.map(|r| r.filter_map(|x| x.ok()).collect())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }
}

fn insert_run(
    conn: &Connection,
    record: &RunRecord,
    imports: &[String],
    packages: &[RunPackage],
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let env_json = record
        .env_overrides
        .as_ref()
        .and_then(|m| serde_json::to_string(m).ok());
    tx.execute(
        "INSERT OR REPLACE INTO runs
            (id, timestamp, code, output, has_go_blocks, success, duration, project_id,
             peak_rss_bytes, auto_installs, lint_issues, missing_imports_resolved,
             exit_code, env_overrides, ai_explanation, sandboxed)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
        params![
            record.id,
            record.timestamp,
            record.code,
            record.output,
            record.has_go_blocks as i64,
            record.success as i64,
            record.duration,
            record.project_id,
            record.peak_rss_bytes as i64,
            record.auto_installs as i64,
            record.lint_issues as i64,
            record.missing_imports_resolved as i64,
            record.exit_code,
            env_json,
            record.ai_explanation,
            record.sandboxed as i64,
        ],
    )?;
    tx.execute("DELETE FROM run_imports WHERE run_id=?1", params![record.id])?;
    for m in imports {
        tx.execute(
            "INSERT OR IGNORE INTO run_imports(run_id, module) VALUES(?1,?2)",
            params![record.id, m],
        )?;
    }
    tx.execute("DELETE FROM run_packages WHERE run_id=?1", params![record.id])?;
    tx.execute("DELETE FROM package_deps WHERE run_id=?1", params![record.id])?;
    for p in packages {
        tx.execute(
            "INSERT OR REPLACE INTO run_packages(run_id, name, version) VALUES(?1,?2,?3)",
            params![record.id, p.name, p.version],
        )?;
        for r in &p.requires {
            tx.execute(
                "INSERT OR IGNORE INTO package_deps(run_id, parent, child) VALUES(?1,?2,?3)",
                params![record.id, p.name, r],
            )?;
        }
        for r in &p.required_by {
            tx.execute(
                "INSERT OR IGNORE INTO package_deps(run_id, parent, child) VALUES(?1,?2,?3)",
                params![record.id, r, p.name],
            )?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn list_runs(conn: &Connection) -> Vec<RunRecord> {
    let mut stmt = match conn.prepare(
        "SELECT id, timestamp, code, output, has_go_blocks, success, duration, project_id,
                peak_rss_bytes, auto_installs, lint_issues, missing_imports_resolved,
                exit_code, env_overrides, ai_explanation, sandboxed
         FROM runs ORDER BY timestamp DESC",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let rows = stmt
        .query_map([], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                code: r.get(2)?,
                output: r.get(3)?,
                has_go_blocks: r.get::<_, i64>(4)? != 0,
                success: r.get::<_, i64>(5)? != 0,
                duration: r.get(6)?,
                project_id: r.get(7)?,
                peak_rss_bytes: r.get::<_, i64>(8)? as u64,
                auto_installs: r.get::<_, i64>(9)? as u32,
                lint_issues: r.get::<_, i64>(10)? as u32,
                missing_imports_resolved: r.get::<_, i64>(11)? as u32,
                exit_code: r.get(12)?,
                env_overrides: r
                    .get::<_, Option<String>>(13)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                imports: Vec::new(),
                packages: Vec::new(),
                ai_explanation: r.get(14)?,
                sandboxed: r.get::<_, i64>(15)? != 0,
            })
        })
        .ok();
    rows.map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

fn get_run(conn: &Connection, id: &str) -> Option<RunRecord> {
    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp, code, output, has_go_blocks, success, duration, project_id,
                    peak_rss_bytes, auto_installs, lint_issues, missing_imports_resolved,
                    exit_code, env_overrides, ai_explanation, sandboxed
             FROM runs WHERE id=?1",
        )
        .ok()?;
    let mut rec: RunRecord = stmt
        .query_row(params![id], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                timestamp: r.get(1)?,
                code: r.get(2)?,
                output: r.get(3)?,
                has_go_blocks: r.get::<_, i64>(4)? != 0,
                success: r.get::<_, i64>(5)? != 0,
                duration: r.get(6)?,
                project_id: r.get(7)?,
                peak_rss_bytes: r.get::<_, i64>(8)? as u64,
                auto_installs: r.get::<_, i64>(9)? as u32,
                lint_issues: r.get::<_, i64>(10)? as u32,
                missing_imports_resolved: r.get::<_, i64>(11)? as u32,
                exit_code: r.get(12)?,
                env_overrides: r
                    .get::<_, Option<String>>(13)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                imports: Vec::new(),
                packages: Vec::new(),
                ai_explanation: r.get(14)?,
                sandboxed: r.get::<_, i64>(15)? != 0,
            })
        })
        .ok()?;

    let mut stmt = conn
        .prepare("SELECT module FROM run_imports WHERE run_id=?1 ORDER BY module")
        .ok()?;
    rec.imports = stmt
        .query_map(params![id], |r| r.get::<_, String>(0))
        .ok()
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();

    let mut stmt = conn
        .prepare("SELECT name, version FROM run_packages WHERE run_id=?1 ORDER BY name")
        .ok()?;
    rec.packages = stmt
        .query_map(params![id], |r| {
            Ok(RunPackage {
                name: r.get(0)?,
                version: r.get(1)?,
                required_by: Vec::new(),
                requires: Vec::new(),
            })
        })
        .ok()
        .map(|r| r.filter_map(|x| x.ok()).collect())
        .unwrap_or_default();
    Some(rec)
}

fn db_path() -> PathBuf {
    let dir = cache_dir();
    dir.join("history.sqlite3")
}

fn to_csv(records: &[RunRecord]) -> String {
    let mut out = String::new();
    out.push_str("id,timestamp,project_id,success,duration_ms,exit_code,peak_rss_bytes,auto_installs,lint_issues,missing_imports_resolved,imports,code_preview,output_preview\n");
    for r in records {
        let preview = sanitize_csv_field(&preview_string(&r.code, 200));
        let output_preview = sanitize_csv_field(&preview_string(&r.output, 200));
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            sanitize_csv_field(&r.id),
            r.timestamp,
            sanitize_csv_field(r.project_id.as_deref().unwrap_or("")),
            r.success,
            r.duration,
            r.exit_code.map(|c| c.to_string()).unwrap_or_default(),
            r.peak_rss_bytes,
            r.auto_installs,
            r.lint_issues,
            r.missing_imports_resolved,
            sanitize_csv_field(&r.imports.join("|")),
            preview,
            output_preview,
        ));
    }
    out
}

fn to_markdown(records: &[RunRecord]) -> String {
    let mut out = String::new();
    out.push_str("# 运行历史\n\n");
    for r in records {
        let ts = format_ts(r.timestamp);
        let status = if r.success { "✅ 成功" } else { "❌ 失败" };
        out.push_str(&format!(
            "## {} · {}\n\n- 时长: {} ms · 内存峰值: {} KB · 自动安装: {}\n\n",
            ts,
            status,
            r.duration,
            r.peak_rss_bytes / 1024,
            r.auto_installs
        ));
        if !r.imports.is_empty() {
            out.push_str(&format!("**imports**: `{}`\n\n", r.imports.join(", ")));
        }
        out.push_str("```python\n");
        out.push_str(&preview_string(&r.code, 1500));
        out.push_str("\n```\n\n");
        if !r.output.is_empty() {
            out.push_str("**输出**:\n\n```\n");
            out.push_str(&preview_string(&r.output, 800));
            out.push_str("\n```\n\n");
        }
        if let Some(ai) = &r.ai_explanation {
            out.push_str(&format!("**AI 解释**:\n\n> {}\n\n", ai.replace('\n', "\n> ")));
        }
        out.push_str("---\n\n");
    }
    out
}

fn preview_string(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn sanitize_csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') || s.contains('\r') {
        let escaped = s.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

fn format_ts(ts: i64) -> String {
    let secs = ts / 1000;
    let nanos = ((ts % 1000) * 1_000_000) as u128;
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(t) => t
            .replace_nanosecond(nanos as u32)
            .ok()
            .map(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| ts.to_string())
            })
            .unwrap_or_else(|| ts.to_string()),
        Err(_) => ts.to_string(),
    }
}

pub fn now_millis() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp() * 1000
        + (time::OffsetDateTime::now_utc().nanosecond() as i64 / 1_000_000)
}

pub fn gen_id() -> String {
    use rand::RngExt;
    let ts = now_millis();
    let rand_part: String = rand::rng()
        .sample_iter(&rand::distr::Alphanumeric)
        .take(6)
        .map(|b| (b as char).to_ascii_lowercase())
        .collect();
    format!("{ts}_{rand_part}")
}

pub type SharedHistory = Arc<AsyncMutex<HistoryStore>>;
