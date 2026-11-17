//! Builds a "project wiki" by static analysis (no LLM calls): a structure overview
//! plus searchable chunks (docs, manifests, per-directory listings, code symbols).
use regex::Regex;
use std::{fs, path::Path};

const SKIP_DIRS: &[&str] = &[
    "node_modules", "target", "dist", "build", ".next", ".nuxt", "venv", ".venv", "__pycache__", "Pods", "vendor",
    "coverage", ".gradle", ".idea", ".vscode", "out", ".turbo", ".cache", "DerivedData", ".terraform", "bin", "obj",
];
const MANIFESTS: &[&str] = &[
    "package.json", "Cargo.toml", "pyproject.toml", "pom.xml", "go.mod", "build.gradle", "build.gradle.kts",
    "composer.json", "Gemfile", "requirements.txt", "serverless.yml", "docker-compose.yml", "Dockerfile",
];
const SOURCE_EXT: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "py", "rs", "go", "java", "kt", "swift", "rb", "php", "cs", "sql", "prisma", "graphql", "proto",
];
const DOC_EXT: &[&str] = &["md", "mdx", "rst"];

const MAX_FILES: usize = 40_000;
const MAX_DEPTH: usize = 8;
const MAX_CHUNKS: usize = 80_000;
const MAX_FILE_BYTES: u64 = 300_000;
const OVERVIEW_LIMIT: usize = 8_000;
const CHUNK_CHARS: usize = 700;

pub struct Scan {
    pub overview: String,
    pub chunks: Vec<String>,
    pub files: usize,
}

fn read_small(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

fn readme_line(dir: &Path) -> String {
    for name in ["README.md", "readme.md", "README"] {
        let Some(text) = read_small(&dir.join(name)) else { continue };
        let mut heading = String::new();
        for line in text.lines().take(40).map(str::trim).filter(|l| !l.is_empty()) {
            if let Some(h) = line.strip_prefix('#') {
                if heading.is_empty() {
                    heading = h.trim_start_matches('#').trim().to_string();
                }
            } else if line.chars().count() > 20 && !line.starts_with(['[', '!', '<', '-', '=', '|', '`']) {
                return line.chars().take(120).collect();
            }
        }
        return heading;
    }
    String::new()
}

fn describe(dir: &Path) -> String {
    let mut tags = vec![];
    let mut desc = String::new();
    let has = |f: &str| dir.join(f).exists();
    if let Some(v) = read_small(&dir.join("package.json")).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) {
        tags.push("node");
        desc = v["description"].as_str().unwrap_or("").to_string();
    }
    if has("Cargo.toml") { tags.push("rust"); }
    if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") { tags.push("java/kotlin"); }
    if has("go.mod") { tags.push("go"); }
    if has("pyproject.toml") || has("requirements.txt") { tags.push("python"); }
    if has("composer.json") { tags.push("php"); }
    if desc.is_empty() {
        desc = readme_line(dir);
    }
    match (tags.is_empty(), desc.is_empty()) {
        (true, true) => String::new(),
        (true, false) => desc,
        (false, _) => format!("[{}] {}", tags.join(","), desc).trim().to_string(),
    }
}

fn push_paragraphs(chunks: &mut Vec<String>, rel: &str, text: &str) {
    let mut buf = String::new();
    let mut n = 0;
    for p in text.split("\n\n") {
        buf.push_str(p);
        buf.push_str("\n\n");
        if buf.chars().count() > CHUNK_CHARS {
            chunks.push(format!("[{rel}] {}", buf.trim()));
            buf.clear();
            n += 1;
            if n >= 40 {
                return;
            }
        }
    }
    if !buf.trim().is_empty() {
        chunks.push(format!("[{rel}] {}", buf.trim()));
    }
}

fn manifest_chunk(rel: &str, name: &str, text: &str) -> String {
    if name == "package.json" {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            let keys = |k: &str| {
                v[k].as_object().map(|o| o.keys().take(40).cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default()
            };
            return format!(
                "[{rel}] package {} — {}. scripts: {}. dependencies: {}",
                v["name"].as_str().unwrap_or(""), v["description"].as_str().unwrap_or(""), keys("scripts"), keys("dependencies")
            );
        }
    }
    format!("[{rel}] {}", text.chars().take(1200).collect::<String>())
}

pub fn build(root: &Path) -> Result<Scan, String> {
    let symbol = Regex::new(
        r"^\s*(?:export\s+(?:default\s+)?)?(?:abstract\s+)?(?:async\s+)?(?:pub(?:\([a-z]+\))?\s+)?(?:class|interface|enum|struct|trait|fn|def|func|function|type|object)\s+([A-Za-z_][A-Za-z0-9_]*)",
    )
    .map_err(|e| e.to_string())?;
    let route = Regex::new(r#"@(Controller|Get|Post|Put|Patch|Delete|Module)\(\s*['"]?([^'")]*)"#).map_err(|e| e.to_string())?;

    let mut chunks: Vec<String> = vec![];
    let mut files = 0usize;
    let mut level1: Vec<String> = vec![];
    let mut level2: Vec<String> = vec![];
    let mut stack = vec![(root.to_path_buf(), 0usize)];

    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        let rel_dir = dir.strip_prefix(root).unwrap_or(&dir).to_string_lossy().to_string();
        let (mut names, mut subdirs) = (vec![], vec![]);

        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let path = e.path();
            if ft.is_dir() {
                if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) || depth + 1 > MAX_DEPTH {
                    continue;
                }
                subdirs.push(name.clone());
                stack.push((path, depth + 1));
                continue;
            }
            files += 1;
            if files > MAX_FILES || chunks.len() > MAX_CHUNKS {
                continue;
            }
            names.push(name.clone());
            let rel = if rel_dir.is_empty() { name.clone() } else { format!("{rel_dir}/{name}") };
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            if MANIFESTS.contains(&name.as_str()) {
                if let Some(t) = read_small(&path) {
                    chunks.push(manifest_chunk(&rel, &name, &t));
                }
            } else if DOC_EXT.contains(&ext.as_str()) {
                if let Some(t) = read_small(&path) {
                    push_paragraphs(&mut chunks, &rel, &t);
                }
            } else if SOURCE_EXT.contains(&ext.as_str()) {
                let Some(t) = read_small(&path) else { continue };
                let mut syms: Vec<String> = vec![];
                let mut routes: Vec<String> = vec![];
                for line in t.lines() {
                    if syms.len() < 60 {
                        if let Some(c) = symbol.captures(line) {
                            syms.push(c[1].to_string());
                        }
                    }
                    if routes.len() < 30 {
                        if let Some(c) = route.captures(line) {
                            routes.push(format!("{} {}", &c[1], &c[2]));
                        }
                    }
                }
                chunks.push(format!("[{rel}] symbols: {}{}", syms.join(", "), if routes.is_empty() { String::new() } else { format!("; routes: {}", routes.join(", ")) }));
            }
        }

        if !rel_dir.is_empty() {
            let listing: Vec<String> = subdirs.iter().map(|d| format!("{d}/")).chain(names.iter().cloned()).take(60).collect();
            chunks.push(format!("[{rel_dir}/] directory contents: {}", listing.join(", ")));
            if depth <= 2 {
                let desc = describe(&dir);
                let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let line = format!("{}{name}/ {}", "  ".repeat(depth - 1), desc).trim_end().to_string();
                if depth == 1 { level1.push(line) } else { level2.push(format!("{rel_dir}/ {desc}")) }
            }
        }
    }

    level1.sort();
    level2.sort();
    let root_desc = describe(root);
    let mut overview = format!("Raiz: {} {}\n", root.display(), root_desc);
    for l in level1.iter().chain(level2.iter()) {
        if overview.len() + l.len() > OVERVIEW_LIMIT {
            overview.push_str("… (veja o restante via busca)\n");
            break;
        }
        overview.push_str(l);
        overview.push('\n');
    }
    Ok(Scan { overview, chunks, files })
}
