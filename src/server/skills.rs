//! Prompt skills: markdown files with frontmatter, loaded from the data directory.
//! A skill's body is injected as one Developer message when the model asks for it
//! through `skill_search`, or when a session names it in its configuration.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// One skill file: its frontmatter and the markdown body below it.
#[derive(Clone, Debug)]
pub struct Skill {
  pub name: String,
  pub description: String,
  /// Trigger hints: substrings that suggest this skill applies to the task.
  pub triggers: Vec<String>,
  pub body: String,
  pub path: PathBuf,
}

impl Skill {
  /// Parses `---\nname: …\ndescription: …\ntriggers: a, b\n---\n<body>`.
  pub fn parse(path: &Path, text: &str) -> Option<Skill> {
    let rest = text.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let (frontmatter, body) = rest.split_at(end);
    let body = body.trim_start_matches("\n---").trim_start_matches('\n').to_owned();
    let mut name = None;
    let mut description = String::new();
    let mut triggers = Vec::new();
    for line in frontmatter.lines() {
      let Some((key, value)) = line.split_once(':') else { continue };
      let value = value.trim();
      match key.trim() {
        "name" => name = Some(value.to_owned()),
        "description" => description = value.to_owned(),
        "triggers" => {
          triggers = value.split(',').map(|item| item.trim().to_lowercase()).filter(|item| !item.is_empty()).collect();
        }
        _ => {}
      }
    }
    Some(Skill {
      name: name.unwrap_or_else(|| {
        path
          .file_stem()
          .map(|stem| stem.to_string_lossy().into_owned())
          .unwrap_or_default()
      }),
      description,
      triggers,
      body,
      path: path.to_owned(),
    })
  }

  /// Whether one of the trigger hints appears in the task text.
  pub fn matches(&self, text: &str) -> bool {
    let lower = text.to_lowercase();
    self.triggers.iter().any(|trigger| lower.contains(trigger))
  }
}

/// Scans directories for SKILL.md files (one skill per directory, or flat files).
pub fn scan(directories: &[PathBuf]) -> Vec<Skill> {
  let mut skills = Vec::new();
  for directory in directories {
    let Ok(entries) = std::fs::read_dir(directory) else { continue };
    for entry in entries.flatten() {
      let path = entry.path();
      if path.is_dir() {
        let skill_file = path.join("SKILL.md");
        if let Ok(text) = std::fs::read_to_string(&skill_file)
          && let Some(skill) = Skill::parse(&skill_file, &text)
        {
          skills.push(skill);
        }
      } else if path.extension().is_some_and(|ext| ext == "md") {
        if let Ok(text) = std::fs::read_to_string(&path)
          && let Some(skill) = Skill::parse(&path, &text)
        {
          skills.push(skill);
        }
      }
    }
  }
  skills.sort_by(|a, b| a.name.cmp(&b.name));
  skills
}

/// The skill_search tool: the model reads a skill's body on demand, never up front.
pub struct SkillSearchTool {
  skills: Vec<Skill>,
}

impl SkillSearchTool {
  pub fn new(directories: Vec<PathBuf>) -> Self {
    Self { skills: scan(&directories) }
  }

  pub fn get_specification(&self) -> crate::protocol::Tool {
    let summary: Vec<Value> = self
      .skills
      .iter()
      .map(|skill| {
        json!({
          "name": skill.name,
          "description": skill.description,
          "triggers": skill.triggers,
        })
      })
      .collect();
    crate::protocol::Tool {
      name: "skill_search".into(),
      description: format!(
        "List available skills and read one skill's full instructions. Available skills: {}",
        serde_json::to_string(&summary).unwrap_or_else(|_| "[]".into())
      ),
      input_schema: json!({
        "type": "object",
        "properties": {
          "name": {"type": "string", "description": "Read this skill's full body (omit to list skills)"},
          "query": {"type": "string", "description": "Match skills whose triggers appear in this text"}
        }
      }),
    }
  }

  pub fn execute(
    &self,
    call: &crate::executor::tool::ToolCall,
  ) -> crate::executor::tool::ToolOutcome {
    use crate::executor::tool::ToolOutcome;
    if let Some(name) = call.arguments.get("name").and_then(Value::as_str) {
      let Some(skill) = self.skills.iter().find(|skill| skill.name == name) else {
        return ToolOutcome::Failed(format!("no skill named {name}"));
      };
      return ToolOutcome::Success(json!({
        "name": skill.name,
        "description": skill.description,
        "path": skill.path.display().to_string(),
        "instructions": skill.body,
      }));
    }
    let query = call.arguments.get("query").and_then(Value::as_str);
    let listed: Vec<&Skill> = self
      .skills
      .iter()
      .filter(|skill| query.map(|text| skill.matches(text)).unwrap_or(true))
      .collect();
    let items: Vec<Value> = listed
      .iter()
      .map(|skill| json!({"name": skill.name, "description": skill.description}))
      .collect();
    ToolOutcome::Success(json!({"skills": items, "total": items.len()}))
  }
}