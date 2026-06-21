use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::cache_dir;
use crate::python::novel_theme::{Theme, ALL_THEMES};
use crate::python::runner::Runner;
use crate::websocket::webtty::{State, WsCmd};

const RESET: &str = "\x1b[0m";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";
const ENTER_ALT: &str = "\x1b[?1049h";
const LEAVE_ALT: &str = "\x1b[?1049l";

const MIN_COLS: usize = 60;
const MIN_ROWS: usize = 12;
const POLL_MS: u64 = 40;

#[derive(Debug, Clone)]
pub struct Novel {
    pub title: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub name: String,
    pub content: String,
}

pub fn parse_novel(code: &str) -> Option<Novel> {
    let first = code.lines().next()?.trim();
    if first != "# 此作品为小说" {
        return None;
    }
    let mut title = String::new();
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut current: Option<Chapter> = None;
    for line in code.lines().skip(1) {
        let trimmed = line.trim();
        if let Some(t) = trimmed.strip_prefix("小说标题:") {
            if title.is_empty() {
                title = t.trim().to_string();
            }
            continue;
        }
        if let Some(c) = trimmed.strip_prefix("小说章节:") {
            if let Some(ch) = current.take() {
                chapters.push(trim_chapter(ch));
            }
            current = Some(Chapter {
                name: c.trim().to_string(),
                content: String::new(),
            });
            continue;
        }
        if let Some(ref mut ch) = current {
            if !ch.content.is_empty() {
                ch.content.push('\n');
            }
            ch.content.push_str(line);
        }
    }
    if let Some(ch) = current {
        chapters.push(trim_chapter(ch));
    }
    if chapters.is_empty() {
        return None;
    }
    Some(Novel { title, chapters })
}

fn trim_chapter(mut ch: Chapter) -> Chapter {
    let mut start = 0;
    let bytes = ch.content.as_bytes();
    while start < bytes.len() {
        let nl = match bytes[start..].iter().position(|&b| b == b'\n') {
            Some(p) => p,
            None => break,
        };
        if nl > start {
            break;
        }
        start += 1;
    }
    if start > 0 {
        ch.content.drain(..start);
    }
    while ch.content.ends_with('\n') {
        ch.content.pop();
    }
    ch
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct NovelProgress {
    chapter_index: usize,
    page: usize,
    last_read_at: u64,
    total_pages_read: u64,
    total_read_secs: u64,
    theme_index: usize,
}

#[derive(Serialize, Deserialize, Default)]
struct ProgressStore {
    novels: std::collections::HashMap<String, NovelProgress>,
    default_theme: usize,
}

fn progress_path() -> PathBuf {
    cache_dir().join("novel_progress.json")
}

fn load_store() -> ProgressStore {
    let path = progress_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => ProgressStore::default(),
    }
}

fn save_store(store: &ProgressStore) {
    let path = progress_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(store) {
        let _ = std::fs::write(&path, json);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct Tui {
    novel: Novel,
    chapter_index: usize,
    page: usize,
    cols: usize,
    rows: usize,
    theme: &'static Theme,
    theme_index: usize,
    session_start: Instant,
    session_pages_read: u64,
    progress: NovelProgress,
    chapter_page_counts: Vec<usize>,
    global_total_pages: usize,
    finished: bool,
    too_small: bool,
}

fn char_width(c: char) -> usize {
    let cp = c as u32;
    if cp < 0x20 { return 0; }
    if cp < 0x7f { return 1; }
    match cp {
        0x1100..=0x115F | 0x2E80..=0x303E
        | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6 => 2,
        _ => 1,
    }
}

fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 { return vec![String::new()]; }
    let mut lines = Vec::new();
    if text.is_empty() { lines.push(String::new()); return lines; }
    for paragraph in text.split('\n') {
        if paragraph.is_empty() { lines.push(String::new()); continue; }
        let mut current = String::new();
        let mut current_width = 0usize;
        for ch in paragraph.chars() {
            let w = char_width(ch);
            if current_width + w > max_width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            current.push(ch);
            current_width += w;
        }
        if !current.is_empty() { lines.push(current); }
    }
    lines
}

fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut w = 0usize;
    let mut result = String::new();
    for ch in s.chars() {
        let cw = char_width(ch);
        if w + cw > max_width {
            if w < max_width { result.push('\u{2026}'); }
            break;
        }
        result.push(ch);
        w += cw;
    }
    result
}

fn render_too_small(theme: &Theme, cols: usize, rows: usize) -> String {
    let mut s = String::new();
    s.push_str("\x1b[2J\x1b[H");
    s.push_str(theme.bg);
    let msg1 = "请放大终端以获得更好的阅读体验";
    let msg2 = format!("当前大小: {} x {}", cols, rows);
    let msg3 = format!("建议大小: >= {} x {}", MIN_COLS, MIN_ROWS);
    let total_lines = 5usize;
    let top_pad = rows.saturating_sub(total_lines) / 2;
    for _ in 0..top_pad {
        s.push_str(&format!("{}{}{}", theme.bg, " ".repeat(cols), RESET));
        s.push_str("\r\n");
    }
    s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(cols.saturating_sub(display_width(msg1)) / 2), theme.fg_title, msg1));
    s.push_str(RESET);
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}", theme.bg, " ".repeat(cols), RESET));
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(cols.saturating_sub(display_width(&msg2)) / 2), theme.fg_chapter, msg2));
    s.push_str(RESET);
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(cols.saturating_sub(display_width(&msg3)) / 2), theme.fg_chapter, msg3));
    s.push_str(RESET);
    s.push_str("\r\n");
    let bottom_pad = rows.saturating_sub(top_pad + total_lines);
    for _ in 0..bottom_pad {
        s.push_str(&format!("{}{}{}", theme.bg, " ".repeat(cols), RESET));
        s.push_str("\r\n");
    }
    s.push_str(RESET);
    s
}

fn render_reading(tui: &Tui, wrapped: &[String], chapter_total_pages: usize) -> String {
    let theme = tui.theme;
    let cols = tui.cols;
    let rows = tui.rows;
    let mut s = String::new();
    s.push_str("\x1b[2J\x1b[H");
    s.push_str(theme.bg);

    let pad = 4usize;
    let header_rows = 4usize;
    let content_rows = rows.saturating_sub(header_rows + 2).max(1);

    let title = if tui.novel.title.is_empty() { "未命名作品".to_string() } else { tui.novel.title.clone() };
    let chapter = &tui.novel.chapters[tui.chapter_index];
    let chapter_title = if chapter.name.is_empty() { "未命名章节".to_string() } else { chapter.name.clone() };

    s.push_str(&format!("{}{}{}", theme.bg, " ".repeat(cols), RESET));
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(cols.saturating_sub(display_width(&title)) / 2), theme.fg_title, title));
    s.push_str(RESET);
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}", theme.bg, " ".repeat(cols), RESET));
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(cols.saturating_sub(display_width(&chapter_title)) / 2), theme.fg_chapter, chapter_title));
    s.push_str(RESET);
    s.push_str("\r\n");
    s.push_str(&format!("{}{}{}{}", theme.bg, theme.fg_rule, "\u{2500}".repeat(cols), RESET));
    s.push_str("\r\n");

    let start = tui.page * content_rows;
    for i in 0..content_rows {
        let idx = start + i;
        let line = wrapped.get(idx).cloned().unwrap_or_default();
        let w = display_width(&line);
        if w > cols {
            s.push_str(&format!("{}{}{}{}", theme.bg, " ".repeat(pad), theme.fg_body, truncate_to_width(&line, cols)));
            s.push_str(RESET);
        } else {
            let right = cols.saturating_sub(w + pad);
            s.push_str(&format!("{}{}{}{}{}{}", theme.bg, " ".repeat(pad), theme.fg_body, line, RESET, theme.bg));
            s.push_str(&" ".repeat(right));
            s.push_str(RESET);
        }
        s.push_str("\r\n");
    }

    s.push_str(&format!("{}{}{}{}", theme.bg, theme.fg_rule, "\u{2500}".repeat(cols), RESET));
    s.push_str("\r\n");

    let page_info = format!("{} / {}  第 {} / {} 章", tui.page + 1, chapter_total_pages.max(1), tui.chapter_index + 1, tui.novel.chapters.len());
    let hints = "Enter/n下一页  b/p上一页  1-9跳章  S主题  Q退出";
    let info_w = display_width(&page_info);
    let hints_w = display_width(hints);
    let sep = "  ";
    let sep_w = display_width(sep);
    if info_w + sep_w + hints_w <= cols {
        s.push_str(&format!("{}{}{}{}{}{}{}", theme.bg, theme.fg_footer, page_info, RESET, theme.bg, " ".repeat(cols - info_w - sep_w - hints_w), RESET));
        s.push_str(&format!("{}{}{}{}", theme.bg, theme.fg_accent, hints, RESET));
    } else {
        s.push_str(&format!("{}{}{}{}", theme.bg, theme.fg_footer, truncate_to_width(&page_info, cols), RESET));
    }
    s.push_str("\r\n");
    s.push_str(RESET);
    s
}

impl Runner {
    pub(super) async fn run_novel(&self, novel: Novel) {
        let (cols, rows) = {
            let wt = self.webtty.lock().await;
            (wt.terminal_cols.max(40) as usize, wt.terminal_rows.max(10) as usize)
        };
        let store = load_store();
        let title_key = if novel.title.is_empty() { "未命名作品".to_string() } else { novel.title.clone() };
        let saved = store.novels.get(&title_key).cloned().unwrap_or_default();
        let theme_index = if saved.theme_index < ALL_THEMES.len() { saved.theme_index } else { 0 };
        let mut tui = Tui {
            novel,
            chapter_index: 0,
            page: 0,
            cols,
            rows,
            theme: &ALL_THEMES[theme_index],
            theme_index,
            session_start: Instant::now(),
            session_pages_read: 0,
            progress: saved,
            chapter_page_counts: Vec::new(),
            global_total_pages: 1,
            finished: false,
            too_small: cols < MIN_COLS || rows < MIN_ROWS,
        };
        tui.chapter_index = tui.progress.chapter_index.min(tui.novel.chapters.len().saturating_sub(1));
        tui.page = tui.progress.page;
        let mut setup = String::new();
        setup.push_str(ENTER_ALT);
        setup.push_str(HIDE_CURSOR);
        setup.push_str(tui.theme.bg);
        self.webtty.lock().await.set_raw_mode(true);
        self.webtty.lock().await.send_msg(&WsCmd::BackendEvent { data: setup }).await;
        'main: while !tui.finished {
            let (cur_cols, cur_rows) = {
                let wt = self.webtty.lock().await;
                (wt.terminal_cols.max(40) as usize, wt.terminal_rows.max(10) as usize)
            };
            if cur_cols != tui.cols || cur_rows != tui.rows {
                tui.cols = cur_cols;
                tui.rows = cur_rows;
                tui.too_small = tui.cols < MIN_COLS || tui.rows < MIN_ROWS;
            }
            {
                let mut wt = self.webtty.lock().await;
                if wt.get_state() != State::Ready || !wt.client_tx_is_some() {
                    tui.finished = true;
                    break;
                }
            }
            if tui.too_small {
                let frame = render_too_small(tui.theme, tui.cols, tui.rows);
                self.webtty.lock().await.send_msg(&WsCmd::BackendEvent { data: frame }).await;
                loop {
                    let (cur_cols, cur_rows) = {
                        let wt = self.webtty.lock().await;
                        (wt.terminal_cols.max(40) as usize, wt.terminal_rows.max(10) as usize)
                    };
                    if cur_cols != tui.cols || cur_rows != tui.rows {
                        tui.cols = cur_cols;
                        tui.rows = cur_rows;
                        tui.too_small = tui.cols < MIN_COLS || tui.rows < MIN_ROWS;
                        break;
                    }
                    let input = self.webtty.lock().await.fetch_next_input();
                    if let Some(inp) = input {
                        let lower = inp.trim().strip_prefix('1').unwrap_or(inp.trim()).to_lowercase();
                        if lower == "q" || lower == "quit" || lower == "exit" {
                            tui.finished = true;
                            break 'main;
                        }
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(POLL_MS)).await;
                }
                continue;
            }
            let text_width = tui.cols.saturating_sub(8);
            let content_rows = tui.rows.saturating_sub(6).max(1);
            let chapter = &tui.novel.chapters[tui.chapter_index];
            let wrapped = wrap_text(&chapter.content, text_width);
            let chapter_total_pages = if wrapped.is_empty() { 1 } else { wrapped.len().div_ceil(content_rows) };
            if tui.page >= chapter_total_pages {
                tui.page = chapter_total_pages.saturating_sub(1);
            }
            tui.chapter_page_counts.clear();
            for c in &tui.novel.chapters {
                let w = wrap_text(&c.content, text_width);
                let pages = if w.is_empty() { 1 } else { w.len().div_ceil(content_rows) };
                tui.chapter_page_counts.push(pages);
            }
            tui.global_total_pages = tui.chapter_page_counts.iter().sum::<usize>().max(1);
            let frame = render_reading(&tui, &wrapped, chapter_total_pages);
            self.webtty.lock().await.send_msg(&WsCmd::BackendEvent { data: frame }).await;
            let mut page_advanced = false;
            let poll_deadline = Instant::now() + Duration::from_millis(POLL_MS);
            'input: loop {
                if Instant::now() >= poll_deadline { break 'input; }
                {
                    let mut wt = self.webtty.lock().await;
                    if wt.get_state() != State::Ready || !wt.client_tx_is_some() {
                        tui.finished = true;
                        break 'main;
                    }
                }
                let input = self.webtty.lock().await.fetch_next_input();
                if let Some(inp) = input {
                    let lower = inp.trim().strip_prefix('1').unwrap_or(inp.trim()).to_lowercase();
                    let stripped = inp.trim().strip_prefix('1').unwrap_or(inp.trim());
                    let cmd: char = lower.chars().next().unwrap_or('\0');
                    let is_empty = stripped.is_empty();
                    if lower == "q" || lower == "quit" || lower == "exit" {
                        tui.finished = true;
                        break 'main;
                    }
                    if cmd == 's' {
                        tui.theme_index = (tui.theme_index + 1) % ALL_THEMES.len();
                        tui.theme = ALL_THEMES[tui.theme_index];
                        tui.progress.theme_index = tui.theme_index;
                        save_progress(tui.theme_index);
                        break 'input;
                    }
                    if is_empty || cmd == ' ' || cmd == 'n' || cmd == 'j' || cmd == 'l' {
                        if tui.page + 1 < chapter_total_pages {
                            tui.page += 1;
                            page_advanced = true;
                        } else if tui.chapter_index + 1 < tui.novel.chapters.len() {
                            tui.chapter_index += 1;
                            tui.page = 0;
                            page_advanced = true;
                        }
                        if page_advanced { tui.session_pages_read += 1; }
                        break 'input;
                    }
                    if cmd == 'b' || cmd == 'p' || cmd == 'k' || cmd == 'h' {
                        if tui.page > 0 {
                            tui.page -= 1;
                        } else if tui.chapter_index > 0 {
                            tui.chapter_index -= 1;
                            tui.page = tui.chapter_page_counts[tui.chapter_index].saturating_sub(1);
                        }
                        tui.session_pages_read += 1;
                        break 'input;
                    }
                    if let Some(digit) = cmd.to_digit(10) {
                        if digit >= 1 && digit <= 9 {
                            let n = digit as usize;
                            if n <= tui.novel.chapters.len() {
                                tui.chapter_index = n - 1;
                                tui.page = 0;
                                tui.session_pages_read += 1;
                                break 'input;
                            }
                        }
                    }
                    if cmd == '0' {
                        tui.chapter_index = tui.novel.chapters.len().saturating_sub(1);
                        tui.page = 0;
                        tui.session_pages_read += 1;
                        break 'input;
                    }
                    break 'input;
                } else {
                    let now = Instant::now();
                    let sleep = poll_deadline.saturating_duration_since(now).min(Duration::from_millis(20));
                    if !sleep.is_zero() { tokio::time::sleep(sleep).await; }
                }
            }
            if page_advanced {
                save_progress_pos(&tui);
            }
        }
        let session_secs = tui.session_start.elapsed().as_secs();
        tui.progress.total_read_secs += session_secs;
        tui.progress.total_pages_read += tui.session_pages_read;
        tui.progress.chapter_index = tui.chapter_index;
        tui.progress.page = tui.page;
        tui.progress.last_read_at = now_secs();
        let mut store = load_store();
        store.novels.insert(title_key, tui.progress.clone());
        store.default_theme = tui.theme_index;
        save_store(&store);
        let mut teardown = String::new();
        teardown.push_str(RESET);
        teardown.push_str(SHOW_CURSOR);
        teardown.push_str(LEAVE_ALT);
        self.webtty.lock().await.set_raw_mode(false);
        self.webtty.lock().await.send_msg(&WsCmd::BackendEvent { data: teardown }).await;
        self.webtty.lock().await.send_msg(&WsCmd::CommandRun).await;
        let mut s = self.state.lock().await;
        s.main_is_running = false;
        s.process_ready = false;
    }
}

fn save_progress(theme_index: usize) {
    let mut store = load_store();
    store.default_theme = theme_index;
    save_store(&store);
}

fn save_progress_pos(tui: &Tui) {
    let mut progress = tui.progress.clone();
    progress.chapter_index = tui.chapter_index;
    progress.page = tui.page;
    progress.last_read_at = now_secs();
    let mut store = load_store();
    store.novels.insert(tui.novel.title.clone(), progress);
    save_store(&store);
}
