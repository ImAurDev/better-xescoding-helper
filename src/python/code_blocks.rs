use regex::Regex;

#[derive(Clone)]
pub struct CodeBlock {
    pub file_name: String,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
}

pub struct GoOutput {
    pub print_output: String,
    pub return_value: String,
    pub return_is_json: bool,
}

pub fn module_install_name(module: &str) -> &str {
    match module {
        "pgzrun" => "pgzero",
        "PIL" => "Pillow",
        "cv2" => "opencv-python",
        "sklearn" => "scikit-learn",
        "nx" => "networkx",
        "plt" => "matplotlib",
        "sp" => "scipy",
        "md" => "markdown",
        "yaml" => "pyyaml",
        "jieba" => "jieba",
        "bs4" => "beautifulsoup4",
        "grpc" => "grpcio",
        "tensorflow" => "tensorflow",
        "torch" => "torch",
        " telegram" => "python-telegram-bot",
        "telebot" => "pyTelegramBotAPI",
        "aiogram" => "aiogram",
        "discord" => "discord.py",
        "vk_api" => "vk-api",
        "qrcode" => "qrcode[pil]",
        "PIL.Image" => "Pillow",
        _ => module,
    }
}

pub fn parse_blocks(code: &str, prefix: &str) -> Vec<CodeBlock> {
    let start_re = Regex::new(&format!(r"^# --- {}:(.+?) ---\s*$", regex::escape(prefix))).unwrap();
    let end_re = Regex::new(r"^# --- end ---\s*$").unwrap();
    let comment_re = Regex::new(r"^#\s*").unwrap();

    let mut blocks = Vec::new();
    let lines: Vec<&str> = code.split('\n').collect();
    let mut in_block = false;
    let mut current_file = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    let mut block_start = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(cap) = start_re.captures(trimmed) {
            let fname = cap[1].trim().to_string();
            if fname.is_empty() {
                continue;
            }
            in_block = true;
            current_file = fname;
            current_lines.clear();
            block_start = i;
            continue;
        }
        if in_block && end_re.is_match(trimmed) {
            blocks.push(CodeBlock {
                file_name: current_file.clone(),
                content: current_lines.join("\n"),
                start_line: block_start,
                end_line: i,
            });
            in_block = false;
            current_file.clear();
            current_lines.clear();
            continue;
        }
        if in_block {
            let stripped = comment_re.replace(trimmed, "").to_string();
            current_lines.push(stripped);
        }
    }
    blocks
}

pub fn parse_go_output(stdout: &str) -> GoOutput {
    let mut print_lines: Vec<String> = Vec::new();
    let mut return_value = String::new();
    let mut return_is_json = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[return] ") {
            return_value = rest.to_string();
            return_is_json = false;
        } else if let Some(rest) = trimmed.strip_prefix("[return-json] ") {
            return_value = rest.to_string();
            return_is_json = true;
        } else {
            print_lines.push(line.to_string());
        }
    }
    GoOutput {
        print_output: print_lines.join("\n").trim_end().to_string(),
        return_value,
        return_is_json,
    }
}
