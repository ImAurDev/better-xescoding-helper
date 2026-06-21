use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

use crate::websocket::DangerHit;

#[derive(Debug, Clone)]
struct DangerRule {
    pattern: Regex,
    label: &'static str,
    #[allow(dead_code)]
    severity: DangerSeverity,
    hint: &'static str,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DangerSeverity {
    Warn,
    Block,
}

static DANGER_RULES: Lazy<Vec<DangerRule>> = Lazy::new(|| {
    vec![
        (
            r"\bos\.system\s*\(",
            "os.system()",
            DangerSeverity::Block,
            "直接执行系统命令",
        ),
        (
            r"\bsubprocess\.(Popen|call|run|check_output)\s*\(",
            "subprocess 调用",
            DangerSeverity::Block,
            "调用外部程序",
        ),
        (
            r"\b__import__\s*\(",
            "__import__()",
            DangerSeverity::Block,
            "动态导入任意模块",
        ),
        (
            r"(?:^|[^.\w])eval\s*\(",
            "eval()",
            DangerSeverity::Block,
            "执行任意表达式",
        ),
        (
            r"(?:^|[^.\w])exec\s*\(",
            "exec()",
            DangerSeverity::Block,
            "执行任意代码字符串",
        ),
        (
            r#"\bcompile\s*\([^)]*['"]exec['"]"#,
            "compile(exec)",
            DangerSeverity::Block,
            "编译并执行代码",
        ),
        (
            r"\bshutil\.rmtree\s*\(",
            "shutil.rmtree()",
            DangerSeverity::Block,
            "递归删除目录",
        ),
        (
            r"\bos\.remove\s*\(",
            "os.remove()",
            DangerSeverity::Block,
            "删除文件",
        ),
        (
            r"\bos\.unlink\s*\(",
            "os.unlink()",
            DangerSeverity::Block,
            "删除文件",
        ),
        (
            r"\bos\.RemoveAll\s*\(",
            "os.RemoveAll()",
            DangerSeverity::Block,
            "Go 递归删除",
        ),
        (
            r"\bos\.exit\s*\(",
            "os.Exit()",
            DangerSeverity::Warn,
            "Go 直接退出进程",
        ),
        (
            r"child_process",
            "child_process",
            DangerSeverity::Block,
            "Node 调用子进程",
        ),
        (
            r"fs\.(unlink|rm|rmdir)\s*\(",
            "fs 删除操作",
            DangerSeverity::Block,
            "TS/JS 删除文件",
        ),
        (
            r#"\bopen\s*\(\s*['"]/dev/"#,
            "打开 /dev",
            DangerSeverity::Block,
            "访问设备文件",
        ),
        (
            r#"\bopen\s*\(\s*['"]/proc/"#,
            "打开 /proc",
            DangerSeverity::Block,
            "访问系统进程信息",
        ),
        (
            r#"\bopen\s*\(\s*['"]/sys/"#,
            "打开 /sys",
            DangerSeverity::Block,
            "访问系统信息",
        ),
        (
            r"while\s+True\s*:",
            "while True:",
            DangerSeverity::Warn,
            "死循环,请确认有 break",
        ),
        (
            r"for\s*\(\s*;\s*;\s*\)",
            "for(;;)",
            DangerSeverity::Warn,
            "Go 死循环,请确认有 break",
        ),
    ]
    .into_iter()
    .map(|(p, l, s, h)| DangerRule {
        pattern: Regex::new(p).expect("valid regex"),
        label: l,
        severity: s,
        hint: h,
    })
    .collect()
});

pub fn check_dangerous_code(code: &str) -> Vec<DangerHit> {
    let mut hits = Vec::new();
    let mut seen: HashSet<(String, usize)> = HashSet::new();
    for rule in DANGER_RULES.iter() {
        for (idx, line) in code.lines().enumerate() {
            if rule.pattern.is_match(line) {
                let key = (rule.label.to_string(), idx + 1);
                if seen.insert(key) {
                    hits.push(DangerHit {
                        label: rule.label.to_string(),
                        hint: rule.hint.to_string(),
                        line: idx + 1,
                        code: line.to_string(),
                    });
                }
            }
        }
    }
    hits
}
