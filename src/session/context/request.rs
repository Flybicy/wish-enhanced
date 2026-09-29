use crate::protocol::Request;
use crate::session::persistence::SessionTransaction;
use crate::session::statistics::Timestamp;
use crate::session::{Entry, EntryId, Session, SessionError};
use crate::storage::PAGE_SIZE;

impl Session {
  pub fn build_request(&self) -> Result<Request, SessionError> {
    let recorded_at = Timestamp::now();
    let mut record = self.record.clone();
    let key = self.key.clone();
    self.storage.transaction(move |tx| {
      SessionTransaction { record: &mut record, tx, key: &key, recorded_at }.build_request()
    })
  }
}
impl SessionTransaction<'_, '_> {
  pub fn build_request(&mut self) -> Result<Request, SessionError> {
    let generation = self.load_generation(self.record.active)?;
    let length = self.tx.list_len::<EntryId>(&generation.entries)?;
    let mut messages = Vec::new();
    let mut start = 0;
    while start < length {
      let page = self.tx.read_page::<EntryId>(&generation.entries, start, PAGE_SIZE as usize)?;
      for id in &page.items {
        let entry = self
          .tx
          .get_item::<Entry>(&self.record.entries, id.0 as u64)?
          .ok_or(SessionError::InvalidEntry(**id))?;
        messages.push(entry.message.clone());
      }
      start += page.items.len() as u64;
    }
    Ok(self.record.config.build_request(messages))
  }
}

use crate::protocol::{Message, model_use::request::ToolChoice};
use crate::session::SessionConfig;
impl SessionConfig {
  pub(crate) fn build_request(&self, conversation: Vec<Message>) -> Request {
    let conversation = self.with_goal(conversation);
    Request {
      model: self.model.clone(),
      stream: self.stream,
      tools: self.tools.clone(),
      tool_choice: Some(ToolChoice::Auto),
      max_output_tokens: self.max_output_tokens,
      reasoning: self.reasoning.clone(),
      cache: self.cache.clone(),
      conversation,
    }
  }

  /// Prepend the session goal, if any, as a pinned Developer instruction. It leads every request
  /// so the model always sees the objective; being synthesized here it never enters the stored
  /// entry log, so clearing the goal is a scalar change with no conversation surgery.
  fn with_goal(&self, conversation: Vec<Message>) -> Vec<Message> {
    let goal = self.goal.as_deref().map(str::trim).filter(|g| !g.is_empty());
    let Some(goal) = goal else { return conversation };
    let mut out = Vec::with_capacity(conversation.len() + 1);
    out.push(Message::Developer {
      metadata: serde_json::Value::Null,
      fixed: Some(true),
      content: vec![crate::protocol::ContentBlock::Text {
        text: format!("Current objective for this session:\n{goal}"),
      }],
    });
    out.extend(conversation);
    out
  }
}

impl crate::session::SessionHandle {
  /// Atomically snapshot committed context without borrowing the running session.
  /// An in-flight tool batch is excluded as a whole, including its assistant turn.
  pub fn build_context_snapshot(&self) -> Result<Request, SessionError> {
    let key = self.sender.key.clone();
    self.sender.storage.transaction(move |tx| {
      let mut record =
        (*tx.load_object::<crate::session::persistence::SessionRecord>(&key)?).clone();
      let pending_tools =
        matches!(record.state, crate::session::SessionState::ExecutingTools { .. });
      let mut request =
        SessionTransaction { record: &mut record, tx, key: &key, recorded_at: Timestamp::now() }
          .build_request()?;
      if pending_tools {
        while matches!(
          request.conversation.last(),
          Some(Message::Assistant { .. } | Message::Reasoning { .. } | Message::ToolUse { .. })
        ) {
          request.conversation.pop();
        }
      }
      crate::session::context::validate_tool_pairs(request.conversation.iter())?;
      Ok(request)
    })
  }
}

#[cfg(test)]
mod tests {
  use crate::protocol::{ContentBlock, Message};
  use crate::session::config::SessionConfig;

  fn user(text: &str) -> Message {
    Message::User { metadata: serde_json::Value::Null, content: vec![ContentBlock::Text { text: text.into() }] }
  }

  #[test]
  fn no_goal_leaves_conversation_untouched() {
    let config = SessionConfig::new("m");
    let out = config.with_goal(vec![user("hi")]);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], Message::User { .. }));
  }

  #[test]
  fn blank_goal_is_ignored() {
    let mut config = SessionConfig::new("m");
    config.goal = Some("   ".into());
    let out = config.with_goal(vec![user("hi")]);
    assert_eq!(out.len(), 1);
    assert!(matches!(out[0], Message::User { .. }));
  }

  #[test]
  fn goal_prepends_a_pinned_developer_instruction() {
    let mut config = SessionConfig::new("m");
    config.goal = Some("  ship it  ".into());
    let out = config.with_goal(vec![user("hi")]);
    assert_eq!(out.len(), 2);
    match &out[0] {
      Message::Developer { fixed, content, .. } => {
        assert_eq!(*fixed, Some(true));
        match &content[0] {
          ContentBlock::Text { text } => {
            assert_eq!(text, "Current objective for this session:\nship it");
          }
          _ => panic!("expected text block"),
        }
      }
      other => panic!("expected pinned Developer, got {other:?}"),
    }
    assert!(matches!(out[1], Message::User { .. }));
  }
}
