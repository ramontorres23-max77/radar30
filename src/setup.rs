//! `setup`: register `radar30` (MCP server + embedded skill) with local coding agents.
//!
//! Modeled directly after `rust-anything`'s `setup.rs`: preserves existing settings and key
//! order, creates `<file>.radar30.bak` backups, writes atomically via a temporary file,
//! refuses unparseable configs, and supports `--dry-run` and `--remove`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};

/// Server and skill name across all agents.
pub const NAME: &str = "radar30";

/// Embedded workflow skill shipped inside the binary.
pub const SKILL: &str = include_str!("../skills/radar30/SKILL.md");

pub(crate) const SKILL_FILES: &[(&str, &str)] = &[("SKILL.md", SKILL)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    /// Claude Code: `~/.claude.json`, skill in `~/.claude/skills/radar30`.
    Claude,
    /// OpenAI Codex CLI: `~/.codex/config.toml`, skill in `~/.codex/skills/radar30`.
    Codex,
    /// Cursor: `~/.cursor/mcp.json`.
    Cursor,
    /// Gemini CLI: `~/.gemini/settings.json`.
    Gemini,
    /// OpenCode: `~/.config/opencode/opencode.json`, skill in `~/.config/opencode/skills/radar30`.
    #[value(name = "opencode")]
    OpenCode,
    /// Hermes Agent: skill in `~/.hermes/skills/radar30` (MCP snippet printed for `~/.hermes/config.yaml`).
    Hermes,
}

impl Agent {
    pub const ALL: [Agent; 6] = [
        Agent::Claude,
        Agent::Codex,
        Agent::Cursor,
        Agent::Gemini,
        Agent::OpenCode,
        Agent::Hermes,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
            Agent::Gemini => "Gemini CLI",
            Agent::OpenCode => "OpenCode",
            Agent::Hermes => "Hermes Agent",
        }
    }

    fn markers(self) -> (&'static str, &'static str) {
        match self {
            Agent::Claude => (".claude", "claude"),
            Agent::Codex => (".codex", "codex"),
            Agent::Cursor => (".cursor", "cursor"),
            Agent::Gemini => (".gemini", "gemini"),
            Agent::OpenCode => (".config/opencode", "opencode"),
            Agent::Hermes => (".hermes", "hermes"),
        }
    }

    pub(crate) fn config(self, home: &Path) -> Option<PathBuf> {
        Some(home.join(match self {
            Agent::Claude => ".claude.json",
            Agent::Codex => ".codex/config.toml",
            Agent::Cursor => ".cursor/mcp.json",
            Agent::Gemini => ".gemini/settings.json",
            Agent::OpenCode => ".config/opencode/opencode.json",
            Agent::Hermes => return None,
        }))
    }

    pub(crate) fn skill_dir(self, home: &Path) -> Option<PathBuf> {
        match self {
            Agent::Claude => Some(home.join(".claude/skills").join(NAME)),
            Agent::Codex => Some(home.join(".codex/skills").join(NAME)),
            Agent::OpenCode => Some(home.join(".config/opencode/skills").join(NAME)),
            Agent::Hermes => Some(home.join(".hermes/skills").join(NAME)),
            Agent::Cursor | Agent::Gemini => None,
        }
    }

    pub(crate) fn present(self, home: &Path) -> bool {
        let (dir, cmd) = self.markers();
        home.join(dir).is_dir() || on_path(cmd)
    }
}

pub(crate) fn on_path(command: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let file = dir.join(command);
            file.is_file()
                || file.with_extension("exe").is_file()
                || file.with_extension("cmd").is_file()
        })
    })
}

pub struct Options<'a> {
    pub home: &'a Path,
    pub agents: Vec<Agent>,
    pub command: PathBuf,
    pub remove: bool,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct Change {
    pub agent: Agent,
    pub file: PathBuf,
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub command: PathBuf,
    pub agents: Vec<Agent>,
    pub changes: Vec<Change>,
    pub dry_run: bool,
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("no home directory (HOME is not set)")
}

pub fn run(opts: &Options) -> Result<Report> {
    let agents: Vec<Agent> = if opts.agents.is_empty() {
        Agent::ALL
            .into_iter()
            .filter(|a| a.present(opts.home))
            .collect()
    } else {
        opts.agents.clone()
    };
    if agents.is_empty() {
        bail!(
            "no coding agent found (checked ~/.claude, ~/.codex, ~/.cursor, ~/.gemini, ~/.config/opencode, ~/.hermes); specify one with --agent"
        );
    }

    let command = opts.command.to_string_lossy().into_owned();
    let mut changes = Vec::new();
    for &agent in &agents {
        if let Some(config) = agent.config(opts.home) {
            changes.push(match agent {
                Agent::Codex => codex_server(agent, &config, &command, opts)?,
                Agent::OpenCode => opencode_server(agent, &config, &command, opts)?,
                _ => json_server(agent, &config, &command, opts, agent == Agent::Claude)?,
            });
        }
        if let Some(dir) = agent.skill_dir(opts.home) {
            changes.push(install_skill(agent, &dir, opts)?);
        }
    }

    Ok(Report {
        command: opts.command.clone(),
        agents,
        changes,
        dry_run: opts.dry_run,
    })
}

fn json_server(
    agent: Agent,
    path: &Path,
    command: &str,
    opts: &Options,
    include_type_stdio: bool,
) -> Result<Change> {
    let mut root: Value = if path.is_file() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} (left untouched)", path.display()))?
    } else {
        json!({})
    };

    let obj = root
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;

    let desired = if include_type_stdio {
        json!({ "type": "stdio", "command": command, "args": ["mcp"] })
    } else {
        json!({ "command": command, "args": ["mcp"] })
    };

    let action = if opts.remove {
        let removed = obj
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .and_then(|s| s.remove(NAME))
            .is_some();
        if removed { "removed" } else { "absent" }
    } else {
        let servers = obj
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .with_context(|| format!("mcpServers in {} is not an object", path.display()))?;
        match servers.get(NAME) {
            Some(existing) if existing == &desired => "unchanged",
            Some(_) => {
                servers.insert(NAME.to_string(), desired);
                "updated"
            }
            None => {
                servers.insert(NAME.to_string(), desired);
                "added"
            }
        }
    };

    let mut backup_path = None;
    if matches!(action, "added" | "updated" | "removed") && !opts.dry_run {
        backup_path = backup(path)?;
        let rendered = format!("{}\n", serde_json::to_string_pretty(&root)?);
        write_atomic(path, &rendered)?;
    }
    Ok(Change {
        agent,
        file: path.to_path_buf(),
        action,
        backup: backup_path,
    })
}

fn opencode_server(agent: Agent, path: &Path, command: &str, opts: &Options) -> Result<Change> {
    let mut root: Value = if path.is_file() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parsing {} (left untouched)", path.display()))?
    } else {
        json!({})
    };

    let obj = root
        .as_object_mut()
        .with_context(|| format!("{} is not a JSON object", path.display()))?;
    let desired = json!({ "type": "local", "command": [command, "mcp"] });

    let action = if opts.remove {
        let removed = obj
            .get_mut("mcp")
            .and_then(Value::as_object_mut)
            .and_then(|m| m.get_mut("servers"))
            .and_then(Value::as_object_mut)
            .and_then(|s| s.remove(NAME))
            .is_some();
        if removed { "removed" } else { "absent" }
    } else {
        let mcp = obj
            .entry("mcp")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .with_context(|| format!("mcp in {} is not an object", path.display()))?;
        let servers = mcp
            .entry("servers")
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .with_context(|| format!("mcp.servers in {} is not an object", path.display()))?;
        match servers.get(NAME) {
            Some(existing) if existing == &desired => "unchanged",
            Some(_) => {
                servers.insert(NAME.to_string(), desired);
                "updated"
            }
            None => {
                servers.insert(NAME.to_string(), desired);
                "added"
            }
        }
    };

    let mut backup_path = None;
    if matches!(action, "added" | "updated" | "removed") && !opts.dry_run {
        backup_path = backup(path)?;
        let rendered = format!("{}\n", serde_json::to_string_pretty(&root)?);
        write_atomic(path, &rendered)?;
    }
    Ok(Change {
        agent,
        file: path.to_path_buf(),
        action,
        backup: backup_path,
    })
}

fn codex_server(agent: Agent, path: &Path, command: &str, opts: &Options) -> Result<Change> {
    let mut doc: toml_edit::DocumentMut = if path.is_file() {
        let raw =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        raw.parse()
            .with_context(|| format!("parsing {} (left untouched)", path.display()))?
    } else {
        toml_edit::DocumentMut::new()
    };

    let action = if opts.remove {
        let removed = doc
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_like_mut)
            .and_then(|t| t.remove(NAME))
            .is_some();
        if removed { "removed" } else { "absent" }
    } else {
        let mut table = toml_edit::Table::new();
        table.insert("command", toml_edit::value(command));
        let mut args = toml_edit::Array::new();
        args.push("mcp");
        table.insert("args", toml_edit::value(args));

        let existing_same = doc
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|m| m.get(NAME))
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|e| {
                e.get("command").and_then(toml_edit::Item::as_str) == Some(command)
                    && e.get("args")
                        .and_then(toml_edit::Item::as_array)
                        .is_some_and(|a| {
                            a.len() == 1 && a.get(0).and_then(|v| v.as_str()) == Some("mcp")
                        })
            });

        let had = doc
            .get("mcp_servers")
            .and_then(toml_edit::Item::as_table_like)
            .and_then(|m| m.get(NAME))
            .is_some();

        if existing_same {
            "unchanged"
        } else {
            if !doc.contains_key("mcp_servers") {
                let mut parent = toml_edit::Table::new();
                parent.set_implicit(true);
                doc.insert("mcp_servers", toml_edit::Item::Table(parent));
            }
            let servers = doc
                .get_mut("mcp_servers")
                .and_then(toml_edit::Item::as_table_like_mut)
                .with_context(|| format!("mcp_servers in {} is not a table", path.display()))?;
            servers.insert(NAME, toml_edit::Item::Table(table));
            if had { "updated" } else { "added" }
        }
    };

    let mut backup_path = None;
    if matches!(action, "added" | "updated" | "removed") && !opts.dry_run {
        backup_path = backup(path)?;
        write_atomic(path, &doc.to_string())?;
    }
    Ok(Change {
        agent,
        file: path.to_path_buf(),
        action,
        backup: backup_path,
    })
}

fn install_skill(agent: Agent, dir: &Path, opts: &Options) -> Result<Change> {
    let target = dir.join("SKILL.md");
    if opts.remove {
        let action = if dir.exists() {
            if !opts.dry_run {
                std::fs::remove_dir_all(dir)
                    .with_context(|| format!("removing {}", dir.display()))?;
            }
            "removed"
        } else {
            "absent"
        };
        return Ok(Change {
            agent,
            file: target,
            action,
            backup: None,
        });
    }

    let all_same = SKILL_FILES
        .iter()
        .all(|(rel, want)| std::fs::read_to_string(dir.join(rel)).ok().as_deref() == Some(*want));
    let action = if all_same {
        "unchanged"
    } else if target.is_file() {
        "updated"
    } else {
        "added"
    };

    if matches!(action, "added" | "updated") && !opts.dry_run {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        for (rel, content) in SKILL_FILES {
            write_atomic(&dir.join(rel), content)?;
        }
    }
    Ok(Change {
        agent,
        file: target,
        action,
        backup: None,
    })
}

fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".radar30.bak");
    let copy = path.with_file_name(name);
    std::fs::copy(path, &copy).with_context(|| format!("backing up {}", path.display()))?;
    Ok(Some(copy))
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = target.with_extension(format!("radar30.tmp.{}", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &target)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_round_trip_claude_codex_opencode_hermes_preserves_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(
            home.join(".claude.json"),
            r#"{"otherKey": 123, "mcpServers": {"keepMe": {"command": "foo"}}}"#,
        )
        .unwrap();

        let opts = Options {
            home,
            agents: vec![Agent::Claude, Agent::Codex, Agent::OpenCode, Agent::Hermes],
            command: PathBuf::from("/usr/local/bin/radar30"),
            remove: false,
            dry_run: false,
        };
        let report = run(&opts).unwrap();
        assert!(report.changes.iter().any(|c| c.action == "added"));

        // Existing Claude keys preserved and backup created.
        let claude_json: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(claude_json["otherKey"], 123);
        assert_eq!(claude_json["mcpServers"]["keepMe"]["command"], "foo");
        assert_eq!(
            claude_json["mcpServers"]["radar30"]["command"],
            "/usr/local/bin/radar30"
        );
        assert!(home.join(".claude.json.radar30.bak").is_file());
        assert!(home.join(".hermes/skills/radar30/SKILL.md").is_file());

        // Second run is idempotent.
        let second = run(&opts).unwrap();
        assert!(second.changes.iter().all(|c| c.action == "unchanged"));

        // Removal cleans up cleanly.
        let remove_opts = Options {
            remove: true,
            ..opts
        };
        let removed = run(&remove_opts).unwrap();
        assert!(removed.changes.iter().all(|c| c.action == "removed"));
        assert!(!home.join(".hermes/skills/radar30/SKILL.md").exists());
    }
}
