//! `doctor`: check binary health, optional auth env, local agent setup, and live MCP self-test.
//!
//! Modeled after `rust-anything/src/doctor.rs`: every problem reports its exact fix command.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::setup::{self, Agent, NAME, SKILL_FILES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Missing,
    Outdated,
    Foreign,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Missing => "missing",
            Status::Outdated => "outdated",
            Status::Foreign => "foreign",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub required: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub version: String,
    pub checks: Vec<Check>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.checks
            .iter()
            .all(|c| !c.required || c.status == Status::Ok)
    }
}

/// Run all checks including spawning `<exe> mcp` for a live stdio protocol self-test.
pub fn run() -> Report {
    let exe = current_exe_canonical();
    let mut checks = core_checks(&exe);
    checks.push(mcp_self_test(&exe));
    if let Ok(home) = setup::home() {
        checks.extend(agent_checks(&home, &exe));
    }
    Report {
        version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    }
}

/// Run checks without spawning a child MCP server (safe to invoke from inside `radar30_doctor` MCP tool).
pub fn run_without_mcp_selftest() -> Report {
    let exe = current_exe_canonical();
    let mut checks = core_checks(&exe);
    if let Ok(home) = setup::home() {
        checks.extend(agent_checks(&home, &exe));
    }
    Report {
        version: env!("CARGO_PKG_VERSION").to_string(),
        checks,
    }
}

fn current_exe_canonical() -> PathBuf {
    std::env::current_exe()
        .and_then(|p| std::fs::canonicalize(&p).or(Ok(p)))
        .unwrap_or_else(|_| PathBuf::from(NAME))
}

fn core_checks(exe: &Path) -> Vec<Check> {
    let mut checks = vec![Check {
        name: "binary".to_string(),
        status: Status::Ok,
        required: true,
        detail: format!("radar30 {} ({})", env!("CARGO_PKG_VERSION"), exe.display()),
        fix: None,
    }];

    for (key, unlocks) in crate::creds::KNOWN_KEYS {
        let origin = crate::creds::resolve_origin(key);
        let present = origin.is_some();
        checks.push(Check {
            name: format!("cred:{key}"),
            status: if present { Status::Ok } else { Status::Missing },
            required: false,
            detail: if present {
                format!(
                    "set via {} (unlocks {unlocks})",
                    origin.expect("present credential has origin").label()
                )
            } else {
                format!("absent (unlocks {unlocks}; lane stays unavailable)")
            },
            fix: (!present)
                .then(|| format!("radar30 creds set {key} --stdin  # pipe the secret via stdin")),
        });
    }

    let ytdlp = crate::process::on_path("yt-dlp");
    checks.push(Check {
        name: "bin:yt-dlp".to_string(),
        status: if ytdlp { Status::Ok } else { Status::Missing },
        required: false,
        detail: if ytdlp {
            "found on PATH (YouTube lane enabled)".to_string()
        } else {
            "not found (YouTube lane stays unavailable)".to_string()
        },
        fix: (!ytdlp).then(|| "brew install yt-dlp".to_string()),
    });

    checks
}

fn agent_checks(home: &Path, exe: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    for agent in Agent::ALL {
        if !agent.present(home) {
            continue;
        }
        if let Some(cfg) = agent.config(home) {
            let status = inspect_mcp_config(agent, &cfg, exe);
            let fix = (status != Status::Ok)
                .then(|| format!("radar30 setup --agent {}", agent_cli_name(agent)));
            out.push(Check {
                name: format!("agent:{}:mcp", agent.name()),
                status,
                required: false,
                detail: format!("{} ({})", status.label(), cfg.display()),
                fix,
            });
        }
        if let Some(skill_dir) = agent.skill_dir(home) {
            let status = inspect_skill_dir(&skill_dir);
            let fix = (status != Status::Ok)
                .then(|| format!("radar30 setup --agent {}", agent_cli_name(agent)));
            out.push(Check {
                name: format!("agent:{}:skill", agent.name()),
                status,
                required: false,
                detail: format!("{} ({})", status.label(), skill_dir.display()),
                fix,
            });
        }
    }
    out
}

fn agent_cli_name(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Cursor => "cursor",
        Agent::Gemini => "gemini",
        Agent::OpenCode => "opencode",
        Agent::Hermes => "hermes",
    }
}

fn inspect_mcp_config(agent: Agent, path: &Path, exe: &Path) -> Status {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Status::Missing;
    };
    match agent {
        Agent::Codex => {
            let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
                return Status::Foreign;
            };
            let Some(entry) = doc
                .get("mcp_servers")
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|t| t.get(NAME))
                .and_then(toml_edit::Item::as_table_like)
            else {
                return Status::Missing;
            };
            let cmd = entry.get("command").and_then(toml_edit::Item::as_str);
            let args_ok = entry
                .get("args")
                .and_then(toml_edit::Item::as_array)
                .is_some_and(|a| a.len() == 1 && a.get(0).and_then(|v| v.as_str()) == Some("mcp"));
            classify_command(cmd, args_ok, exe)
        }
        Agent::OpenCode => {
            let Ok(val) = serde_json::from_str::<Value>(&raw) else {
                return Status::Foreign;
            };
            let Some(entry) = val["mcp"]["servers"].get(NAME) else {
                return Status::Missing;
            };
            let arr = entry["command"].as_array();
            let cmd = arr.and_then(|a| a.first()).and_then(Value::as_str);
            let args_ok = arr.is_some_and(|a| a.len() == 2 && a[1].as_str() == Some("mcp"));
            classify_command(cmd, args_ok, exe)
        }
        _ => {
            let Ok(val) = serde_json::from_str::<Value>(&raw) else {
                return Status::Foreign;
            };
            let Some(entry) = val["mcpServers"].get(NAME) else {
                return Status::Missing;
            };
            let cmd = entry["command"].as_str();
            let args_ok = entry["args"]
                .as_array()
                .is_some_and(|a| a.len() == 1 && a[0].as_str() == Some("mcp"));
            classify_command(cmd, args_ok, exe)
        }
    }
}

fn classify_command(cmd: Option<&str>, args_ok: bool, exe: &Path) -> Status {
    let Some(cmd) = cmd else {
        return Status::Foreign;
    };
    if !args_ok {
        return Status::Foreign;
    }
    let path = Path::new(cmd);
    if path == exe || std::fs::canonicalize(path).is_ok_and(|p| p == exe) {
        Status::Ok
    } else {
        Status::Outdated
    }
}

fn inspect_skill_dir(dir: &Path) -> Status {
    let installed: Vec<Option<String>> = SKILL_FILES
        .iter()
        .map(|(rel, _)| std::fs::read_to_string(dir.join(rel)).ok())
        .collect();
    let Some(main) = installed.first().and_then(Option::as_deref) else {
        return if installed.iter().all(Option::is_none) {
            Status::Missing
        } else {
            Status::Outdated
        };
    };
    if installed
        .iter()
        .zip(SKILL_FILES)
        .all(|(have, (_, want))| have.as_deref() == Some(*want))
    {
        Status::Ok
    } else if main
        .lines()
        .take(10)
        .any(|l| l.trim() == format!("name: {NAME}"))
    {
        Status::Outdated
    } else {
        Status::Foreign
    }
}

/// Spawn `<exe> mcp`, send `initialize` + `tools/list`, and verify a clean round-trip.
pub fn mcp_self_test(exe: &Path) -> Check {
    let started = Instant::now();
    let mut child = match Command::new(exe)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Check {
                name: "mcp:self-test".to_string(),
                status: Status::Missing,
                required: true,
                detail: format!("failed to spawn {} mcp: {e}", exe.display()),
                fix: Some("rebuild radar30 with `cargo build --release`".to_string()),
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}});
        let list = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
        let _ = writeln!(stdin, "{init}");
        let _ = writeln!(stdin, "{list}");
        let _ = stdin.flush();
    }

    let stdout = child.stdout.take();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                lines.push(line);
                if lines.len() >= 2 {
                    break;
                }
            }
        }
        let _ = tx.send(lines);
    });

    let result = rx.recv_timeout(Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();

    match result {
        Ok(lines) if lines.len() >= 2 => {
            let init_ok = serde_json::from_str::<Value>(&lines[0])
                .is_ok_and(|v| v["result"]["serverInfo"]["name"] == NAME);
            let tool_count = serde_json::from_str::<Value>(&lines[1])
                .ok()
                .and_then(|v| v["result"]["tools"].as_array().map(Vec::len))
                .unwrap_or(0);
            if init_ok && tool_count > 0 {
                Check {
                    name: "mcp:self-test".to_string(),
                    status: Status::Ok,
                    required: true,
                    detail: format!(
                        "initialize + tools/list ({tool_count} tools) in {} ms",
                        started.elapsed().as_millis()
                    ),
                    fix: None,
                }
            } else {
                Check {
                    name: "mcp:self-test".to_string(),
                    status: Status::Outdated,
                    required: true,
                    detail: "unexpected MCP response payload".to_string(),
                    fix: Some("run `cargo test` to inspect MCP protocol compliance".to_string()),
                }
            }
        }
        _ => Check {
            name: "mcp:self-test".to_string(),
            status: Status::Missing,
            required: true,
            detail: "MCP server did not answer within 5s".to_string(),
            fix: Some("run `radar30 mcp` manually to inspect errors".to_string()),
        },
    }
}

/// Format `Report` as crisp terminal text.
pub fn format_text(report: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "radar30 doctor (v{})", report.version);
    for c in &report.checks {
        let req = if c.required { "required" } else { "optional" };
        let _ = writeln!(
            out,
            "  [{:8}] {:26} ({}) — {}",
            c.status.label(),
            c.name,
            req,
            c.detail
        );
        if let Some(ref fix) = c.fix {
            let _ = writeln!(out, "             fix: {fix}");
        }
    }
    let _ = writeln!(
        out,
        "\nOverall: {}",
        if report.ok() { "PASS" } else { "FAIL" }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_detects_missing_current_and_outdated_agent_configs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let exe = PathBuf::from("/usr/local/bin/radar30");

        std::fs::create_dir_all(home.join(".claude")).unwrap();
        assert_eq!(
            inspect_mcp_config(Agent::Claude, &home.join(".claude.json"), &exe),
            Status::Missing
        );

        let opts = setup::Options {
            home,
            agents: vec![Agent::Claude],
            command: exe.clone(),
            remove: false,
            dry_run: false,
        };
        setup::run(&opts).unwrap();
        assert_eq!(
            inspect_mcp_config(Agent::Claude, &home.join(".claude.json"), &exe),
            Status::Ok
        );
        assert_eq!(
            inspect_skill_dir(&home.join(".claude/skills/radar30")),
            Status::Ok
        );

        let other_exe = PathBuf::from("/opt/other/radar30");
        assert_eq!(
            inspect_mcp_config(Agent::Claude, &home.join(".claude.json"), &other_exe),
            Status::Outdated
        );
    }
}
