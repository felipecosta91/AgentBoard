use crate::{rows, AppErr, S};
use axum::http::StatusCode;
use regex::Regex;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::VecDeque, fmt::Write as _, process::Stdio, sync::OnceLock};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const HISTORY_LIMIT: i64 = 40;
const ATTACHMENT_PROMPT_LIMIT: usize = 20_000;

const REFINE: &str = "Você é o agente de REFINAMENTO. Não escreva código nem altere arquivos. \
ANTES de perguntar, consulte a wiki acima e o código (Grep/Glob/Read) para descobrir onde a feature se encaixa e o que já existe; nunca pergunte o que o código já responde. Ajude o usuário a estruturar o projeto (objetivo, regras de negócio, layout, telas, dados, critérios de aceite) \
fazendo no máximo 3 perguntas objetivas por vez e aguardando a resposta. \
Quando não restarem dúvidas ou o usuário confirmar, finalize com EXATAMENTE um bloco:\n\
<refined>{\"context\":\"markdown autossuficiente com decisões, regras de negócio e layout\",\"checklist\":[\"tarefa de desenvolvimento 1\",\"tarefa 2\"]}</refined>\n\
O checklist deve listar tudo que precisa ser desenvolvido, em itens pequenos e verificáveis.";

const CREATE: &str = "Você é o agente de DESENVOLVIMENTO. Implemente o card no diretório atual seguindo o contexto e o checklist. \
Ao concluir cada item do checklist emita `<check id=\"N\"/>` (N = id do item). \
Se houver dúvida bloqueante, pergunte ao usuário e pare SEM emitir `<done/>`. \
Quando tudo estiver implementado emita `<done/>`.";

const QA: &str = "Você é o agente de Q.A. Valide a implementação contra as regras de negócio e o checklist: \
rode build e testes existentes, crie os testes que faltarem e verifique o comportamento. Não corrija código de produção. \
Finalize com `<qa verdict=\"pass\">resumo</qa>` ou `<qa verdict=\"fail\">problemas encontrados</qa>`.";

const CHAT: &str = "Você é um assistente do projeto. Responda ao usuário com base no contexto do card. Não altere arquivos.";

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// Don't leak a parent Claude Code session's env (OAuth scopes, base URL) into child CLIs.
pub fn scrub_env(cmd: &mut Command) {
    for (k, _) in std::env::vars() {
        if k.starts_with("CLAUDE_CODE") || k.starts_with("CLAUDE_AGENT") || k == "CLAUDECODE" || k == "ANTHROPIC_BASE_URL" {
            cmd.env_remove(k);
        }
    }
}

pub fn runs_on(kind: &str) -> bool {
    matches!(kind, "refine" | "create" | "qa")
}

/// Splits text in ~700-char chunks and (re)indexes them for the project-wide RAG.
pub fn index(c: &Connection, project: i64, card: i64, text: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM rag WHERE card_id=?", [card])?;
    let flush = |buf: &mut String| -> rusqlite::Result<()> {
        if !buf.trim().is_empty() {
            c.execute(
                "INSERT INTO rag(content,card_id,project_id) VALUES(?,?,?)",
                params![buf.trim(), card, project],
            )?;
        }
        buf.clear();
        Ok(())
    };
    let mut buf = String::new();
    for paragraph in text.split("\n\n") {
        buf.push_str(paragraph);
        buf.push_str("\n\n");
        if buf.len() > 700 {
            flush(&mut buf)?;
        }
    }
    flush(&mut buf)
}

fn related(c: &Connection, project: i64, card: i64, query_text: &str) -> rusqlite::Result<Vec<String>> {
    let mut words: Vec<String> = query_text
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|w| w.chars().count() >= 4)
        .map(|w| format!("\"{}\"*", w.to_lowercase()))
        .collect();
    words.sort();
    words.dedup();
    words.truncate(12);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let mut st = c.prepare(
        "SELECT content FROM rag WHERE rag MATCH ? AND project_id=? AND card_id!=? ORDER BY rank LIMIT 8",
    )?;
    let found = st.query_map(params![words.join(" OR "), project, card], |r| r.get(0))?;
    found.collect()
}

struct Job {
    kind: String,
    agent: String,
    model: String,
    path: String,
    prompt: String,
    images: Vec<String>,
}

fn load(c: &Connection, id: i64) -> rusqlite::Result<Job> {
    let card = rows(
        c,
        "SELECT c.*, l.kind, p.path FROM cards c JOIN lanes l ON l.id=c.lane_id JOIN projects p ON p.id=c.project_id WHERE c.id=?",
        [id],
    )?
    .pop()
    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let s = |k: &str| card[k].as_str().unwrap_or("").to_string();
    let (kind, title, body, context) = (s("kind"), s("title"), s("body"), s("context"));
    let project = card["project_id"].as_i64().unwrap_or(0);

    let mut p = String::new();
    let wiki: String = c.query_row("SELECT wiki FROM projects WHERE id=?", [project], |r| r.get(0))?;
    if !wiki.is_empty() {
        let _ = writeln!(p, "## Wiki do projeto (estrutura e módulos já existentes)\n{wiki}\nO diretório atual é a raiz deste projeto. Respeite sua estrutura e convenções.\n");
    }
    let _ = writeln!(p, "# Card #{id}: {title}\n{body}\n\n## Contexto refinado\n{}\n", if context.is_empty() { "(vazio)" } else { &context });

    let checklist = rows(c, "SELECT id,text,done FROM checklist WHERE card_id=? ORDER BY id", [id])?;
    if !checklist.is_empty() {
        p.push_str("## Checklist\n");
        for i in &checklist {
            let mark = if i["done"].as_i64() == Some(1) { "x" } else { " " };
            let _ = writeln!(p, "- [{mark}] (id={}) {}", i["id"], i["text"].as_str().unwrap_or(""));
        }
        p.push('\n');
    }

    let mut images = vec![];
    let atts = rows(c, "SELECT name,kind,path,text FROM attachments WHERE card_id=? ORDER BY id", [id])?;
    if !atts.is_empty() {
        p.push_str("## Anexos\n");
        for a in &atts {
            let (name, akind, path) = (a["name"].as_str().unwrap_or(""), a["kind"].as_str().unwrap_or(""), a["path"].as_str());
            match (akind, path) {
                ("image", Some(path)) => {
                    images.push(path.to_string());
                    let _ = writeln!(p, "- imagem {name}: {path}");
                }
                ("audio", _) => {
                    let _ = writeln!(p, "- transcrição do áudio {name}:\n{}", a["text"].as_str().unwrap_or(""));
                }
                ("url", _) => {
                    let text: String = a["text"].as_str().unwrap_or("").chars().take(ATTACHMENT_PROMPT_LIMIT).collect();
                    let _ = writeln!(p, "- página {name}:\n{text}");
                }
                ("text", _) => {
                    let text: String = a["text"].as_str().unwrap_or("").chars().take(ATTACHMENT_PROMPT_LIMIT).collect();
                    let _ = writeln!(p, "- arquivo {name}:\n{text}");
                }
                (_, Some(path)) => {
                    let _ = writeln!(p, "- arquivo {name}: {path}");
                }
                _ => {}
            }
        }
        p.push('\n');
    }

    let last_user = rows(c, "SELECT content FROM messages WHERE card_id=? AND role='user' ORDER BY id DESC LIMIT 1", [id])?
        .pop()
        .and_then(|m| m["content"].as_str().map(str::to_string))
        .unwrap_or_default();
    let rag = related(c, project, id, &format!("{title} {body} {last_user}"))?;
    if !rag.is_empty() {
        p.push_str("## Trechos relevantes da wiki do projeto e de outros cards (busca)\n");
        for r in rag {
            let r: String = r.chars().take(900).collect();
            let _ = writeln!(p, "---\n{r}");
        }
        p.push('\n');
    }

    let mut history = rows(c, "SELECT role,content FROM messages WHERE card_id=? AND role IN ('user','assistant') ORDER BY id DESC LIMIT ?", params![id, HISTORY_LIMIT])?;
    history.reverse();
    if !history.is_empty() {
        p.push_str("## Conversa até agora\n");
        for m in &history {
            let _ = writeln!(p, "{}: {}", m["role"].as_str().unwrap_or(""), m["content"].as_str().unwrap_or(""));
        }
        p.push('\n');
    }

    let role = match kind.as_str() {
        "refine" => REFINE,
        "create" => CREATE,
        "qa" => QA,
        _ => CHAT,
    };
    let _ = write!(p, "## Instruções\n{role}\nResponda em português do Brasil.\n");
    if history.last().is_none_or(|m| m["role"] != "user") {
        p.push_str("Comece agora.\n");
    }

    Ok(Job { kind, agent: s("agent"), model: s("model"), path: s("path"), prompt: p, images })
}

fn add_msg(a: &S, id: i64, role: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let _ = a.db(|c| c.execute("INSERT INTO messages(card_id,role,content) VALUES(?,?,?)", params![id, role, text]));
    a.changed();
}

static CHECK: OnceLock<Regex> = OnceLock::new();
static DONE: OnceLock<Regex> = OnceLock::new();
static REFINED: OnceLock<Regex> = OnceLock::new();
static QA_TAG: OnceLock<Regex> = OnceLock::new();

fn clean(text: &str) -> String {
    let t = re(&CHECK, r#"<check\s+id=["']?\d+["']?\s*/?>"#).replace_all(text, "");
    let t = re(&DONE, r"<done\s*/?>").replace_all(&t, "");
    let t = re(&REFINED, r"(?s)<refined>(.*?)</refined>").replace_all(&t, "✅ Refinamento concluído e registrado no card.");
    let t = re(&QA_TAG, r#"(?s)<qa\s+verdict=["']?(pass|fail)["']?\s*>(.*?)</qa>"#).replace_all(&t, "**Q.A: $1**\n$2");
    t.trim().to_string()
}

fn apply_checks(a: &S, id: i64, text: &str) {
    for m in re(&CHECK, r#"<check\s+id=["']?(\d+)["']?\s*/?>"#).captures_iter(text) {
        let item: i64 = m[1].parse().unwrap_or(0);
        let _ = a.db(|c| c.execute("UPDATE checklist SET done=1 WHERE id=? AND card_id=?", params![item, id]));
    }
}

/// Maps one CLI JSON line to chat events; sets `fail` on a fatal agent error.
fn events(agent: &str, v: &Value, fail: &mut Option<String>) -> Vec<(&'static str, String)> {
    let mut out = vec![];
    let ty = v["type"].as_str().unwrap_or("");
    if agent == "codex" {
        match ty {
            "item.completed" => {
                let item = &v["item"];
                match item["type"].as_str() {
                    Some("agent_message") => out.push(("assistant", item["text"].as_str().unwrap_or("").to_string())),
                    Some("command_execution") => out.push(("tool", format!("$ {}", item["command"].as_str().unwrap_or("")))),
                    _ => {}
                }
            }
            "turn.failed" => *fail = Some(v["error"]["message"].as_str().unwrap_or("turn failed").to_string()),
            _ => {}
        }
        return out;
    }
    match ty {
        "assistant" => {
            for b in v["message"]["content"].as_array().into_iter().flatten() {
                match b["type"].as_str() {
                    Some("text") => out.push(("assistant", b["text"].as_str().unwrap_or("").to_string())),
                    Some("tool_use") => {
                        let input: String = b["input"].to_string().chars().take(120).collect();
                        out.push(("tool", format!("{} {}", b["name"].as_str().unwrap_or("tool"), input)));
                    }
                    _ => {}
                }
            }
        }
        "result" if v["is_error"] == true => {
            *fail = Some(v["result"].as_str().unwrap_or("agent error").to_string());
        }
        _ => {}
    }
    out
}

struct Outcome {
    status: &'static str,
    push: &'static str,
    next_kind: Option<&'static str>,
}

async fn run(a: &S, id: i64) -> Result<Outcome, String> {
    let job = a.db(|c| load(c, id)).map_err(|e| e.1)?;
    let write = matches!(job.kind.as_str(), "create" | "qa");
    let codex = job.agent == "codex";

    let mut cmd = Command::new(if codex { "codex" } else { "claude" });
    if codex {
        cmd.args(["exec", "--json", "--skip-git-repo-check", "-s", if write { "workspace-write" } else { "read-only" }]);
        if !job.model.is_empty() {
            cmd.args(["-m", &job.model]);
        }
        for img in &job.images {
            cmd.args(["-i", img]);
        }
    } else {
        cmd.args(["-p", "--output-format", "stream-json", "--verbose"]);
        if !job.model.is_empty() {
            cmd.args(["--model", &job.model]);
        }
        if write {
            cmd.args(["--permission-mode", "acceptEdits", "--allowedTools", "Read,Grep,Glob,WebFetch,WebSearch,Bash,Edit,Write"]);
        } else {
            cmd.args(["--allowedTools", "Read,Grep,Glob,WebFetch,WebSearch"]);
        }
    }
    scrub_env(&mut cmd);
    cmd.current_dir(&job.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let bin = if codex { "codex" } else { "claude" };
    let mut child = cmd.spawn().map_err(|e| {
        let cwd_ok = std::path::Path::new(&job.path).is_dir();
        let path = std::env::var("PATH").unwrap_or_default();
        format!("não foi possível iniciar {bin}: {e} (pasta do projeto existe: {cwd_ok}; PATH: {path})")
    })?;
    let mut stdin = child.stdin.take().ok_or("stdin indisponível")?;
    stdin.write_all(job.prompt.as_bytes()).await.map_err(|e| e.to_string())?;
    drop(stdin);

    let stderr = child.stderr.take().ok_or("stderr indisponível")?;
    let err_tail = tokio::spawn(async move {
        let mut tail = VecDeque::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if tail.len() == 20 {
                tail.pop_front();
            }
            tail.push_back(l);
        }
        tail.into_iter().collect::<Vec<_>>().join("\n")
    });

    let mut lines = BufReader::new(child.stdout.take().ok_or("stdout indisponível")?).lines();
    let (mut full, mut fail) = (String::new(), None);
    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        for (role, text) in events(&job.agent, &v, &mut fail) {
            if role == "assistant" {
                full.push_str(&text);
                full.push('\n');
                apply_checks(a, id, &text);
                add_msg(a, id, role, &clean(&text));
            } else {
                add_msg(a, id, role, &text);
            }
        }
    }
    let exit = child.wait().await.map_err(|e| e.to_string())?;
    if fail.is_some() || !exit.success() {
        let tail = err_tail.await.unwrap_or_default();
        return Err(fail.unwrap_or(if tail.is_empty() { format!("{bin} saiu com {exit}") } else { tail }));
    }

    match job.kind.as_str() {
        "refine" => {
            let Some(m) = re(&REFINED, r"(?s)<refined>(.*?)</refined>").captures(&full) else {
                return Ok(Outcome { status: "idle", push: "Agente aguarda sua resposta", next_kind: None });
            };
            apply_refined(a, id, &m[1])?;
            Ok(Outcome { status: "idle", push: "Refinamento concluído", next_kind: Some("refine_done") })
        }
        "create" if re(&DONE, r"<done\s*/?>").is_match(&full) => {
            Ok(Outcome { status: "idle", push: "Desenvolvimento concluído", next_kind: Some("qa") })
        }
        "create" => Ok(Outcome { status: "idle", push: "Agente aguarda sua resposta", next_kind: None }),
        "qa" => match re(&QA_TAG, r#"(?s)<qa\s+verdict=["']?(pass|fail)["']?\s*>"#).captures(&full).map(|m| m[1].to_string()).as_deref() {
            Some("pass") => Ok(Outcome { status: "idle", push: "Q.A aprovou", next_kind: Some("done") }),
            Some(_) => Ok(Outcome { status: "failed", push: "Q.A reprovou", next_kind: None }),
            None => Ok(Outcome { status: "idle", push: "Q.A terminou sem veredito", next_kind: None }),
        },
        _ => Ok(Outcome { status: "idle", push: "Resposta pronta", next_kind: None }),
    }
}

fn apply_refined(a: &S, id: i64, raw: &str) -> Result<(), String> {
    let (start, end) = (raw.find('{'), raw.rfind('}'));
    let json = match (start, end) {
        (Some(s), Some(e)) if s < e => &raw[s..=e],
        _ => return Err("bloco <refined> sem JSON".into()),
    };
    let v: Value = serde_json::from_str(json).map_err(|e| format!("JSON de refinamento inválido: {e}"))?;
    let context = v["context"].as_str().unwrap_or("").to_string();
    let items: Vec<String> = v["checklist"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i.as_str().map(str::to_string))
        .collect();
    a.db(|c| {
        let project: i64 = c.query_row("SELECT project_id FROM cards WHERE id=?", [id], |r| r.get(0))?;
        c.execute("UPDATE cards SET context=? WHERE id=?", params![context, id])?;
        c.execute("DELETE FROM checklist WHERE card_id=?", [id])?;
        for t in &items {
            c.execute("INSERT INTO checklist(card_id,text) VALUES(?,?)", params![id, t])?;
        }
        index(c, project, id, &context)
    })
    .map_err(|e| e.1)
}

fn move_to_kind(a: &S, id: i64, kind: &str) -> bool {
    a.db(|c| {
        c.execute(
            "UPDATE cards SET pos=(SELECT COALESCE(MAX(pos),0)+1 FROM cards),
             lane_id=COALESCE((SELECT l.id FROM lanes l WHERE l.kind=?1 AND l.project_id=cards.project_id ORDER BY l.pos LIMIT 1), lane_id)
             WHERE id=?2 AND EXISTS (SELECT 1 FROM lanes l WHERE l.kind=?1 AND l.project_id=cards.project_id)",
            params![kind, id],
        )
    })
    .map(|n| n > 0)
    .unwrap_or(false)
}

pub fn start(a: &S, id: i64) -> Result<(), AppErr> {
    let mut running = a.running.lock().unwrap();
    if running.contains_key(&id) {
        return Err(AppErr::new(StatusCode::CONFLICT, "agent is already working on this card"));
    }
    let updated = a.db(|c| c.execute("UPDATE cards SET status='running' WHERE id=?", [id]))?;
    if updated == 0 {
        return Err(AppErr::new(StatusCode::NOT_FOUND, "not found"));
    }
    let a2 = a.clone();
    let handle = tokio::spawn(async move {
        let result = run(&a2, id).await;
        a2.running.lock().unwrap().remove(&id);
        let title = a2.card_title(id);
        match result {
            Ok(o) => {
                let _ = a2.db(|c| c.execute("UPDATE cards SET status=? WHERE id=?", params![o.status, id]));
                a2.push(o.push, &title);
                if let Some(kind) = o.next_kind {
                    if move_to_kind(&a2, id, kind) {
                        a2.push("Card movido", &format!("{title} → {kind}"));
                        if runs_on(kind) {
                            let _ = start(&a2, id);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("agent error on card {id}: {e}");
                add_msg(&a2, id, "system", &format!("⚠️ {e}"));
                let _ = a2.db(|c| c.execute("UPDATE cards SET status='error' WHERE id=?", [id]));
                a2.push("Erro no agente", &format!("{title}: {e}"));
            }
        }
    });
    running.insert(id, handle.abort_handle());
    drop(running);
    a.changed();
    Ok(())
}

pub fn stop(a: &S, id: i64) {
    let Some(handle) = a.running.lock().unwrap().remove(&id) else { return };
    handle.abort(); // drops the future; kill_on_drop terminates the CLI process
    add_msg(a, id, "system", "⏹ Agente interrompido.");
    let _ = a.db(|c| c.execute("UPDATE cards SET status='idle' WHERE id=?", [id]));
    a.changed();
}
