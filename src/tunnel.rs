//! Exposes the local server to the phone through ngrok (preferred) or a Cloudflare quick tunnel,
//! and announces the (changing) public URL over ntfy, signed with the shared token so it cannot be spoofed.
use crate::S;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

const REANNOUNCE: Duration = Duration::from_secs(600);

fn sign(token: &str, url: &str, ts: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("hmac accepts any key length");
    mac.update(format!("{url}|{ts}").as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

async fn announce(a: &S, url: &str) {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let message = json!({"u": url, "ts": ts, "sig": sign(&a.token, url, ts)}).to_string();
    let payload = json!({"topic": a.ntfy_topic, "title": "AgentBoard online", "message": message, "tags": ["agentboard-url"], "priority": 1});
    if let Err(e) = a.http.post(&a.ntfy_server).json(&payload).send().await {
        eprintln!("announce failed: {e}");
    }
}

/// A fresh tunnel hostname can take a while to resolve/route; announcing it too early makes phones
/// cache a failed DNS lookup. Wait until the public URL actually serves the app.
async fn wait_ready(a: &S, url: &str) -> bool {
    for _ in 0..60 {
        if a.http.get(url).timeout(Duration::from_secs(5)).send().await.is_ok_and(|r| r.status().is_success()) {
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

async fn installed(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|s| s.success())
}

/// True when ngrok has an authtoken (env var or its config file).
async fn ngrok_configured() -> bool {
    if std::env::var("NGROK_AUTHTOKEN").is_ok_and(|t| !t.is_empty()) {
        return true;
    }
    let Ok(out) = Command::new("ngrok").args(["config", "check"]).stdin(Stdio::null()).output().await else { return false };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(path) = text.split(" at ").nth(1).map(str::trim) else { return false };
    let Ok(content) = tokio::fs::read_to_string(path).await else { return false };
    regex::Regex::new(r"(?m)^\s*authtoken:\s*\S+").unwrap().is_match(&content)
}

async fn ngrok_usable() -> bool {
    installed("ngrok", "version").await && ngrok_configured().await
}

pub async fn status(a: &S) -> Value {
    let (ngrok, cloudflared) = tokio::join!(installed("ngrok", "version"), installed("cloudflared", "--version"));
    json!({
        "ngrok_installed": ngrok,
        "ngrok_configured": ngrok && ngrok_configured().await,
        "cloudflared_installed": cloudflared,
        "public_url": *a.public_url.lock().unwrap(),
    })
}

/// Saves the authtoken through ngrok's own CLI (stored in ngrok's config, never by AgentBoard).
pub async fn save_authtoken(a: &S, token: &str) -> Result<(), String> {
    if !regex::Regex::new(r"^[A-Za-z0-9_\-]{20,200}$").unwrap().is_match(token) {
        return Err("authtoken inválido".into());
    }
    let out = Command::new("ngrok")
        .args(["config", "add-authtoken", token])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|e| format!("ngrok não encontrado: {e}"))?;
    if !out.status.success() {
        return Err("o ngrok recusou o authtoken".into());
    }
    a.tunnel_restart.notify_one();
    Ok(())
}

fn set_url(a: &S, url: &str) {
    *a.public_url.lock().unwrap() = url.to_string();
    a.changed();
}

pub fn start(a: S, port: u16) {
    tokio::spawn(async move {
        if let Ok(url) = std::env::var("AGENTBOARD_PUBLIC_URL") {
            let url = url.trim_end_matches('/').to_string();
            set_url(&a, &url);
            loop {
                announce(&a, &url).await;
                tokio::time::sleep(REANNOUNCE).await;
            }
        }
        let forced = std::env::var("AGENTBOARD_TUNNEL").unwrap_or_default();
        if forced == "off" {
            return;
        }
        // A user-level ~/.cloudflared/config.yml (named tunnels) would otherwise hijack quick tunnels
        // with its own ingress rules (404 catch-all), so always pass our own minimal config.
        let config = a.dir.join("cloudflared.yml");
        if let Err(e) = tokio::fs::write(&config, format!("ingress:\n  - service: http://127.0.0.1:{port}\n")).await {
            eprintln!("não foi possível escrever {}: {e}", config.display());
            return;
        }
        let cloudflared_re = regex::Regex::new(r"https://[a-z0-9-]+\.trycloudflare\.com").unwrap();
        let ngrok_re = regex::Regex::new(r#""url":"(https://[^"]+)""#).unwrap();
        let wants_ngrok = forced != "cloudflared";

        let mut use_ngrok = wants_ngrok && ngrok_usable().await;
        let mut backoff = 5;
        loop {
            let mut cmd = if use_ngrok {
                let mut c = Command::new("ngrok");
                c.args(["http", &format!("127.0.0.1:{port}"), "--log=stdout", "--log-format=json"]);
                c.stdout(Stdio::piped()).stderr(Stdio::null());
                c
            } else {
                let mut c = Command::new("cloudflared");
                c.args(["tunnel", "--no-autoupdate", "--protocol", "http2", "--config"])
                    .arg(&config)
                    .args(["--url", &format!("http://127.0.0.1:{port}")]);
                c.stdout(Stdio::null()).stderr(Stdio::piped());
                c
            };
            let mut child = match cmd.stdin(Stdio::null()).kill_on_drop(true).spawn() {
                Ok(c) => c,
                Err(e) => {
                    // Nothing to tunnel with yet: wait until the user finishes the setup wizard.
                    eprintln!("túnel indisponível ({e}); aguardando configuração");
                    a.tunnel_restart.notified().await;
                    use_ngrok = wants_ngrok && ngrok_usable().await;
                    continue;
                }
            };
            let stream: Box<dyn tokio::io::AsyncRead + Unpin + Send> = match (use_ngrok, child.stdout.take(), child.stderr.take()) {
                (true, Some(o), _) => Box::new(o),
                (false, _, Some(e)) => Box::new(e),
                _ => return,
            };
            let mut lines = BufReader::new(stream).lines();
            let mut tick = tokio::time::interval(REANNOUNCE);
            let mut url: Option<String> = None;
            let mut restart = false;
            loop {
                tokio::select! {
                    line = lines.next_line() => match line {
                        Ok(Some(l)) => {
                            let found = if use_ngrok {
                                l.contains("started tunnel").then(|| ngrok_re.captures(&l).map(|c| c[1].to_string())).flatten()
                            } else {
                                cloudflared_re.find(&l).map(|m| m.as_str().to_string())
                            };
                            let Some(found) = found else { continue };
                            if wait_ready(&a, &found).await {
                                set_url(&a, &found);
                                announce(&a, &found).await;
                                url = Some(found);
                                backoff = 5;
                            } else {
                                eprintln!("túnel {found} não ficou acessível; reiniciando");
                                break;
                            }
                        }
                        _ => break,
                    },
                    _ = tick.tick() => if let Some(u) = &url { announce(&a, u).await },
                    _ = a.tunnel_restart.notified() => { restart = true; break },
                }
            }
            set_url(&a, "");
            if restart {
                use_ngrok = wants_ngrok && ngrok_usable().await;
                continue;
            }
            if use_ngrok && url.is_none() {
                // e.g. another ngrok session already uses the free plan's single slot.
                eprintln!("ngrok não iniciou o túnel; usando cloudflared");
                use_ngrok = false;
                continue;
            }
            eprintln!("túnel encerrou; reiniciando em {backoff}s");
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(60);
        }
    });
}
